use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use storage_monitor_core::scan::{
    Node, NodeKind, ScanError, ScanOptions, ScanProgress, Tree, rescan_path, scan,
};
use storage_monitor_core::snapshot::{Snapshot, deltas, top_growers};
use tempfile::TempDir;

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = fs::File::create(path).unwrap();
    f.write_all(&vec![b'x'; bytes]).unwrap();
}

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("docs/report.txt"), 3_000);
    write(&root.join("docs/notes/a.md"), 100);
    write(&root.join("docs/notes/b.md"), 200);
    write(&root.join(".hidden/secret.bin"), 50);
    // Larger than docs/ even where directories occupy a 4 KiB block each (ext4).
    write(&root.join("big.bin"), 40_000);
    fs::create_dir_all(root.join("empty")).unwrap();
    dir
}

fn child<'a>(tree: &'a Tree, parent: u32, name: &str) -> (u32, &'a Node) {
    tree.children(parent)
        .map(|id| (id, tree.get(id).unwrap()))
        .find(|(_, n)| &*n.name == name)
        .unwrap_or_else(|| panic!("no child {name}"))
}

#[test]
fn scans_a_tree_with_logical_sizes_counts_and_kinds() {
    let dir = fixture();
    let progress = ScanProgress::default();
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &progress).unwrap();
    let tree = &result.tree;

    assert_eq!(&*tree.root().name, dir.path().to_string_lossy().as_ref());
    assert_eq!(tree.root().kind, NodeKind::Dir);
    assert_eq!(tree.root().logical_size, 3_000 + 100 + 200 + 50 + 40_000);
    assert!(
        tree.root().size >= tree.root().logical_size,
        "allocated size counts whole blocks"
    );
    assert_eq!(tree.root().file_count, 5);

    let (docs, docs_node) = child(tree, Tree::ROOT, "docs");
    assert_eq!(docs_node.logical_size, 3_300);
    assert_eq!(docs_node.file_count, 3);
    let (_, notes) = child(tree, docs, "notes");
    assert_eq!(notes.file_count, 2);
    let (_, hidden) = child(tree, Tree::ROOT, ".hidden");
    assert_eq!(hidden.file_count, 1, "hidden entries are scanned");
    let (empty_id, empty) = child(tree, Tree::ROOT, "empty");
    assert_eq!(empty.file_count, 0);
    assert!(!tree.has_children(empty_id));
    let (_, big) = child(tree, Tree::ROOT, "big.bin");
    assert_eq!(big.kind, NodeKind::File);
    assert_eq!(big.logical_size, 40_000);

    assert_eq!(result.stats.files, 5);
    assert_eq!(result.stats.dirs, 5, "root, docs, notes, .hidden, empty");
    assert_eq!(result.stats.errors, 0);
    assert!(!result.cancelled);
}

#[test]
fn children_are_sorted_by_size_descending() {
    let dir = fixture();
    let result = scan(
        &ScanOptions::new(dir.path().to_path_buf()),
        &ScanProgress::default(),
    )
    .unwrap();
    let tree = &result.tree;
    let sizes: Vec<u64> = tree
        .children(Tree::ROOT)
        .map(|id| tree.get(id).unwrap().size)
        .collect();
    let mut sorted = sizes.clone();
    sorted.sort_unstable_by_key(|size| std::cmp::Reverse(*size));
    assert_eq!(sizes, sorted);
    assert_eq!(
        &*tree.get(tree.children(Tree::ROOT).start).unwrap().name,
        "big.bin"
    );
}

#[test]
fn symlinks_are_recorded_but_never_followed() {
    let dir = fixture();
    let root = dir.path();
    std::os::unix::fs::symlink(root.join("docs"), root.join("docs-link")).unwrap();
    std::os::unix::fs::symlink(root, root.join("loop")).unwrap();
    let result = scan(
        &ScanOptions::new(root.to_path_buf()),
        &ScanProgress::default(),
    )
    .unwrap();
    let tree = &result.tree;
    let (link_id, link) = child(tree, Tree::ROOT, "docs-link");
    assert_eq!(link.kind, NodeKind::Symlink);
    assert!(!tree.has_children(link_id));
    assert!(link.logical_size < 1_000);
    let (loop_id, _) = child(tree, Tree::ROOT, "loop");
    assert!(!tree.has_children(loop_id));
    assert_eq!(result.stats.files, 7, "symlinks count as entries");
}

fn allocated(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().blocks() * 512
}

fn names(tree: &Tree, parent: u32) -> Vec<&str> {
    tree.children(parent)
        .map(|id| &*tree.get(id).unwrap().name)
        .collect()
}

#[test]
fn hard_links_are_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("one.bin"), 8_192);
    fs::hard_link(root.join("one.bin"), root.join("two.bin")).unwrap();
    let result = scan(
        &ScanOptions::new(root.to_path_buf()),
        &ScanProgress::default(),
    )
    .unwrap();
    let (_, one) = child(&result.tree, Tree::ROOT, "one.bin");
    let (_, two) = child(&result.tree, Tree::ROOT, "two.bin");
    assert_eq!(
        one.size,
        allocated(&root.join("one.bin")),
        "the smaller path keeps the data"
    );
    assert_eq!(two.size, 0, "the other link contributes nothing");
    assert_eq!(two.logical_size, 0);
    assert_eq!(result.tree.root().logical_size, 8_192);
    assert_eq!(result.stats.hardlinks_skipped, 1);
    // The root directory's own blocks count toward its size (0 on APFS, 4096 on ext4).
    assert_eq!(result.tree.root().size, allocated(root) + one.size);
    assert_eq!(result.stats.bytes, result.tree.root().size);
}

#[test]
fn hard_links_across_directories_belong_to_the_smallest_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("a/z.bin"), 8_192);
    write(&root.join("b/small.bin"), 4_096);
    fs::hard_link(root.join("a/z.bin"), root.join("b/a.bin")).unwrap();
    let result = scan(
        &ScanOptions::new(root.to_path_buf()),
        &ScanProgress::default(),
    )
    .unwrap();
    let tree = &result.tree;
    let (a, a_node) = child(tree, Tree::ROOT, "a");
    let (b, b_node) = child(tree, Tree::ROOT, "b");
    let (_, z) = child(tree, a, "z.bin");
    let (_, link) = child(tree, b, "a.bin");
    let (_, small) = child(tree, b, "small.bin");
    assert_eq!(
        z.size,
        allocated(&root.join("a/z.bin")),
        "a/z.bin < b/a.bin"
    );
    assert_eq!(link.size, 0);
    assert_eq!(link.logical_size, 0);
    assert_eq!(a_node.size, allocated(&root.join("a")) + z.size);
    assert_eq!(b_node.size, allocated(&root.join("b")) + small.size);
    assert_eq!(a_node.logical_size, 8_192);
    assert_eq!(
        b_node.logical_size, 4_096,
        "the logical size of the loser leaves b as well"
    );
    assert_eq!(tree.root().logical_size, 8_192 + 4_096);
    assert_eq!(
        tree.root().size,
        allocated(root) + a_node.size + b_node.size
    );
    assert_eq!(result.stats.bytes, tree.root().size);
    assert_eq!(result.stats.hardlinks_skipped, 1);
    // Listings reflect the attribution: a outranks b, the emptied link sinks to the end.
    assert_eq!(names(tree, Tree::ROOT), vec!["a", "b"]);
    assert_eq!(names(tree, b), vec!["small.bin", "a.bin"]);
}

#[test]
fn hard_link_attribution_is_deterministic() {
    // 64 links in 8 directories walked by several threads: which link a worker meets
    // first varies between runs, the attributed sizes must not.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("d0/f0.bin"), 8_192);
    for d in 0..8 {
        fs::create_dir_all(root.join(format!("d{d}"))).unwrap();
        for f in 0..8 {
            if (d, f) != (0, 0) {
                fs::hard_link(root.join("d0/f0.bin"), root.join(format!("d{d}/f{f}.bin"))).unwrap();
            }
        }
    }
    let listing = || {
        let result = scan(
            &ScanOptions::new(root.to_path_buf()),
            &ScanProgress::default(),
        )
        .unwrap();
        let sizes: Vec<(PathBuf, u64)> = result
            .tree
            .iter()
            .map(|(id, node)| (result.tree.path(id), node.size))
            .collect();
        (sizes, result.stats)
    };
    let (sizes, stats) = listing();
    assert_eq!(stats.hardlinks_skipped, 63);
    let data = allocated(&root.join("d0/f0.bin"));
    let winner = root.join("d0/f0.bin");
    for (path, size) in &sizes {
        if path.extension().is_some() {
            let expected = if *path == winner { data } else { 0 };
            assert_eq!(*size, expected, "{}", path.display());
        }
    }
    for _ in 1..5 {
        assert_eq!(listing(), (sizes.clone(), stats.clone()));
    }
}

#[test]
fn sparse_files_report_allocated_size_below_logical_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparse.bin");
    let f = fs::File::create(&path).unwrap();
    f.set_len(50 * 1024 * 1024).unwrap();
    drop(f);
    let result = scan(
        &ScanOptions::new(dir.path().to_path_buf()),
        &ScanProgress::default(),
    )
    .unwrap();
    let (_, sparse) = child(&result.tree, Tree::ROOT, "sparse.bin");
    assert_eq!(sparse.logical_size, 50 * 1024 * 1024);
    assert!(
        sparse.size < sparse.logical_size,
        "allocated {} should be below logical",
        sparse.size
    );
}

#[test]
fn unreadable_directory_is_recorded_as_error_and_scan_continues() {
    if unsafe { libc_geteuid() } == 0 {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = fixture();
    let locked = dir.path().join("locked");
    write(&locked.join("inside.bin"), 10);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let result = scan(
        &ScanOptions::new(dir.path().to_path_buf()),
        &ScanProgress::default(),
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let result = result.unwrap();
    let (locked_id, _) = child(&result.tree, Tree::ROOT, "locked");
    let error = result.tree.error(locked_id);
    assert!(
        error.unwrap_or("").contains("ermission"),
        "error: {error:?}"
    );
    assert_eq!(result.stats.errors, 1);
    assert_eq!(
        result.stats.files, 5,
        "the rest of the tree is still scanned"
    );
}

#[test]
fn excluded_directories_are_skipped() {
    let dir = fixture();
    let mut options = ScanOptions::new(dir.path().to_path_buf());
    options.excludes.push(dir.path().join("docs"));
    let result = scan(&options, &ScanProgress::default()).unwrap();
    let names: Vec<&str> = result
        .tree
        .children(Tree::ROOT)
        .map(|id| &*result.tree.get(id).unwrap().name)
        .collect();
    assert!(!names.contains(&"docs"));
    assert_eq!(result.stats.files, 2);
}

#[test]
fn excludes_are_normalized_like_the_root() {
    let dir = fixture();
    let mut options = ScanOptions::new(dir.path().to_path_buf());
    options
        .excludes
        .push(PathBuf::from(format!("{}/docs/", dir.path().display())));
    options.excludes.push(dir.path().join("./.hidden"));
    let result = scan(&options, &ScanProgress::default()).unwrap();
    assert_eq!(names(&result.tree, Tree::ROOT), vec!["big.bin", "empty"]);
    assert_eq!(result.stats.files, 1);
}

#[test]
fn cancelled_scan_stops_early_and_says_so() {
    let dir = fixture();
    let progress = ScanProgress::default();
    progress.cancel();
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &progress).unwrap();
    assert!(result.cancelled);
    assert!(!result.tree.has_children(Tree::ROOT));
    assert_eq!(result.tree.error(Tree::ROOT), Some("scan cancelled"));
}

#[test]
fn cancelled_scan_keeps_bytes_consistent_with_the_root() {
    let dir = fixture();
    let progress = ScanProgress::default();
    progress.cancel();
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &progress).unwrap();
    assert_eq!(result.stats.bytes, result.tree.root().size);
    assert_eq!(
        result.stats.dirs, 1,
        "the root is still counted as a visited directory"
    );
}

#[test]
fn partially_readable_directories_record_how_many_entries_failed() {
    if unsafe { libc_geteuid() } == 0 {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = fixture();
    let pair = dir.path().join("pair");
    write(&pair.join("inside.bin"), 10);
    write(&pair.join("other.bin"), 10);
    let single = dir.path().join("single");
    write(&single.join("only.bin"), 10);
    // Readable but not searchable: the names are listed, their metadata is not.
    for d in [&pair, &single] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o444)).unwrap();
    }
    let result = scan(
        &ScanOptions::new(dir.path().to_path_buf()),
        &ScanProgress::default(),
    );
    for d in [&pair, &single] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let result = result.unwrap();
    let (pair_id, pair_node) = child(&result.tree, Tree::ROOT, "pair");
    assert_eq!(
        result.tree.error(pair_id),
        Some("2 entries could not be read")
    );
    assert_eq!(pair_node.file_count, 0);
    let (single_id, _) = child(&result.tree, Tree::ROOT, "single");
    assert_eq!(
        result.tree.error(single_id),
        Some("1 entry could not be read")
    );
    assert_eq!(result.stats.errors, 3);
    assert_eq!(
        result.stats.files, 5,
        "the rest of the tree is still scanned"
    );
}

#[test]
fn deep_trees_do_not_overflow_the_stack() {
    // Nest directories until the OS refuses the path: about 480 levels on macOS and
    // 2000 on Linux, more than the default 2 MiB worker stacks accommodate. Every level
    // also has a second, empty directory so the parallel machinery is on the stack at
    // each level, as in a real tree.
    let dir = tempfile::tempdir().unwrap();
    let mut path = dir.path().to_path_buf();
    let mut depth = 0u64;
    while fs::create_dir(path.join("d")).is_ok() {
        fs::create_dir(path.join("e")).unwrap();
        path.push("d");
        depth += 1;
    }
    assert!(depth > 100, "only {depth} levels could be created");
    let result = scan(
        &ScanOptions::new(dir.path().to_path_buf()),
        &ScanProgress::default(),
    );
    // Delete the chain bottom-up before anything can fail: remove_dir_all recurses per
    // level as well and would overflow the test thread on its way out.
    for _ in 0..depth {
        fs::remove_dir(&path).unwrap();
        path.pop();
        fs::remove_dir(path.join("e")).unwrap();
    }
    let result = result.unwrap();
    assert_eq!(result.stats.dirs, 2 * depth + 1);
    assert_eq!(result.stats.errors, 0);
    assert!(result.tree.errors().is_empty());
}

#[test]
fn progress_counters_match_final_stats() {
    let dir = fixture();
    let progress = ScanProgress::default();
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &progress).unwrap();
    let snap = progress.snapshot();
    assert_eq!(snap.files, result.stats.files);
    assert_eq!(snap.dirs, result.stats.dirs);
    assert_eq!(snap.bytes, result.stats.bytes);
    assert_eq!(snap.bytes, result.tree.root().size);
}

#[test]
fn root_with_a_trailing_slash_is_normalized() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a/one.bin"), 1_000);
    let slashed = PathBuf::from(format!("{}/", dir.path().display()));
    let scan_now = || scan(&ScanOptions::new(slashed.clone()), &ScanProgress::default()).unwrap();
    let first = scan_now();
    assert_eq!(first.root, dir.path());
    assert_eq!(&*first.tree.root().name, dir.path().to_str().unwrap());
    let (a, _) = child(&first.tree, Tree::ROOT, "a");
    assert_eq!(first.tree.path(a), dir.path().join("a"));

    write(&dir.path().join("a/two.bin"), 50_000);
    let second = scan_now();
    let before = Snapshot::from_result(&first, u64::MAX);
    let after = Snapshot::from_result(&second, u64::MAX);
    let growers = top_growers(&deltas(&before, &after), 10);
    let paths: Vec<&str> = growers.iter().map(|g| g.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![dir.path().join("a").to_str().unwrap()],
        "the root's growth is explained by a, so the root itself is not listed"
    );
}

#[test]
fn symlinked_root_is_followed_and_keeps_its_own_path() {
    let dir = fixture();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("link");
    std::os::unix::fs::symlink(dir.path(), &link).unwrap();
    let result = scan(&ScanOptions::new(link.clone()), &ScanProgress::default()).unwrap();
    assert_eq!(result.root, link);
    assert_eq!(&*result.tree.root().name, link.to_str().unwrap());
    assert_eq!(result.tree.root().kind, NodeKind::Dir);
    assert_eq!(result.stats.files, 5);
    assert_eq!(result.stats.dirs, 5);
    let (docs, _) = child(&result.tree, Tree::ROOT, "docs");
    assert_eq!(result.tree.path(docs), link.join("docs"));
}

#[test]
fn root_must_be_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file.txt");
    write(&file, 1);
    assert!(scan(&ScanOptions::new(file.clone()), &ScanProgress::default()).is_err());
    let file_link = dir.path().join("file-link");
    std::os::unix::fs::symlink(&file, &file_link).unwrap();
    assert!(scan(&ScanOptions::new(file_link), &ScanProgress::default()).is_err());
    let dangling = dir.path().join("dangling");
    std::os::unix::fs::symlink(dir.path().join("gone"), &dangling).unwrap();
    assert!(scan(&ScanOptions::new(dangling), &ScanProgress::default()).is_err());
    assert!(
        scan(
            &ScanOptions::new(PathBuf::from("/definitely/missing")),
            &ScanProgress::default()
        )
        .is_err()
    );
}

#[test]
fn rescan_of_a_deleted_path_reports_nothing() {
    let dir = fixture();
    let gone = dir.path().join("docs");
    fs::remove_dir_all(&gone).unwrap();
    let options = ScanOptions::new(dir.path().to_path_buf());
    assert!(rescan_path(&gone, &options).unwrap().is_none());
}

#[test]
fn rescan_of_a_directory_returns_its_current_contents() {
    let dir = fixture();
    let docs = dir.path().join("docs");
    fs::remove_file(docs.join("report.txt")).unwrap();
    let options = ScanOptions::new(dir.path().to_path_buf());
    let tree = rescan_path(&docs, &options).unwrap().unwrap();
    assert_eq!(tree.root().file_count, 2, "only the two notes are left");
    assert_eq!(tree.root().logical_size, 300);
    // docs/ and docs/notes/ hold the same two files, so the counts above cannot tell the
    // rescanned directory from its child: pin the shape as well.
    assert_eq!(tree.len(), 4, "docs, notes and the two notes");
    assert_eq!(names(&tree, Tree::ROOT), vec!["notes"]);
}

#[test]
fn rescan_of_a_single_file_returns_one_node() {
    let dir = fixture();
    let file = dir.path().join("big.bin");
    let options = ScanOptions::new(dir.path().to_path_buf());
    let tree = rescan_path(&file, &options).unwrap().unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.root().kind, NodeKind::File);
    assert_eq!(tree.root().logical_size, 40_000);
}

#[test]
fn rescan_of_a_symlink_does_not_follow_it() {
    let dir = fixture();
    let link = dir.path().join("docs-link");
    std::os::unix::fs::symlink(dir.path().join("docs"), &link).unwrap();
    let options = ScanOptions::new(dir.path().to_path_buf());
    let tree = rescan_path(&link, &options).unwrap().unwrap();
    assert_eq!(tree.root().kind, NodeKind::Symlink);
    assert_eq!(tree.len(), 1);
}

#[test]
fn rescan_names_its_root_with_the_absolute_path() {
    let dir = fixture();
    let options = ScanOptions::new(dir.path().to_path_buf());
    let file = dir.path().join("big.bin");
    let tree = rescan_path(&file, &options).unwrap().unwrap();
    assert_eq!(&*tree.root().name, file.to_str().unwrap());
    // `Path` compares component-wise, so this cannot fail while the string above holds;
    // it is here to name what Task 7 needs, not as a second check. Keep both.
    assert_eq!(tree.path(Tree::ROOT), file, "a patch is spliced by path");
    let docs = dir.path().join("docs");
    let tree = rescan_path(&docs, &options).unwrap().unwrap();
    assert_eq!(&*tree.root().name, docs.to_str().unwrap());
    assert_eq!(tree.path(Tree::ROOT), docs);
}

#[test]
fn rescan_normalizes_the_path_before_reading_it() {
    // `lstat` resolves a symlink whose path ends in a separator, so a trailing slash would
    // otherwise walk the target of the link that was just deleted.
    let dir = fixture();
    let root = dir.path();
    std::os::unix::fs::symlink(root.join("docs"), root.join("docs-link")).unwrap();
    let options = ScanOptions::new(root.to_path_buf());
    let slashed = |name: &str| PathBuf::from(format!("{}/{name}/", root.display()));

    let tree = rescan_path(&slashed("docs-link"), &options)
        .unwrap()
        .unwrap();
    assert_eq!(
        tree.root().kind,
        NodeKind::Symlink,
        "the link is not followed"
    );
    assert_eq!(tree.len(), 1);
    assert_eq!(&*tree.root().name, root.join("docs-link").to_str().unwrap());

    let tree = rescan_path(&slashed("docs"), &options).unwrap().unwrap();
    assert_eq!(&*tree.root().name, root.join("docs").to_str().unwrap());
    assert_eq!(tree.root().file_count, 3);
}

#[test]
fn rescan_of_a_dangling_symlink_reports_the_link_itself() {
    let dir = fixture();
    let link = dir.path().join("dangling");
    std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();
    assert!(!link.exists(), "the target is missing, the link is not");
    let options = ScanOptions::new(dir.path().to_path_buf());
    for path in [link.clone(), PathBuf::from(format!("{}/", link.display()))] {
        let tree = rescan_path(&path, &options).unwrap().unwrap();
        assert_eq!(tree.root().kind, NodeKind::Symlink, "{}", path.display());
        assert_eq!(tree.len(), 1);
    }
}

#[test]
fn rescan_carries_the_excludes_of_the_scan() {
    let dir = fixture();
    let docs = dir.path().join("docs");
    let mut options = ScanOptions::new(dir.path().to_path_buf());
    options.excludes.push(docs.join("notes"));
    let tree = rescan_path(&docs, &options).unwrap().unwrap();
    assert_eq!(names(&tree, Tree::ROOT), vec!["report.txt"]);
    assert_eq!(tree.root().file_count, 1);
    assert_eq!(tree.root().logical_size, 3_000);
}

#[test]
fn rescan_of_an_unreadable_path_reports_the_error_it_hit() {
    if unsafe { libc_geteuid() } == 0 {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = fixture();
    let locked = dir.path().join("locked");
    let inside = locked.join("inside.bin");
    write(&inside, 10);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let options = ScanOptions::new(dir.path().to_path_buf());
    // The directory itself can be stat'ed but not listed; its entry cannot even be stat'ed.
    let listed = rescan_path(&locked, &options);
    let entry = rescan_path(&inside, &options);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    let tree = listed.unwrap().unwrap();
    assert_eq!(tree.root().kind, NodeKind::Dir);
    assert_eq!(tree.len(), 1);
    let error = tree.error(Tree::ROOT);
    assert!(
        error.unwrap_or("").contains("ermission"),
        "error: {error:?}"
    );

    match entry {
        Err(ScanError::Root { path, source }) => {
            assert_eq!(path, inside);
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
        }
        other => panic!("expected a Root error, got {other:?}"),
    }
}

#[test]
fn rescan_of_a_path_under_a_file_is_gone() {
    // The tree still lists docs/report.txt/inner because the scan saw a directory there;
    // what is at that path now is nothing, and `ENOTDIR` is how the kernel says so.
    let dir = fixture();
    let stale = dir.path().join("docs/report.txt/inner");
    let options = ScanOptions::new(dir.path().to_path_buf());
    assert_eq!(
        fs::symlink_metadata(&stale).unwrap_err().kind(),
        std::io::ErrorKind::NotADirectory,
        "the parent is a file"
    );
    assert!(rescan_path(&stale, &options).unwrap().is_none());
}

#[test]
fn rescan_of_a_path_behind_a_symlink_loop_is_an_error_not_a_gone_path() {
    // `ELOOP` says the path could not be resolved, not that it is missing: dropping the
    // branch on it would delete a subtree from the tree over a transient failure.
    let dir = fixture();
    let loop_link = dir.path().join("loop");
    std::os::unix::fs::symlink(&loop_link, &loop_link).unwrap();
    let behind = loop_link.join("inner");
    // `ELOOP` is 62 on macOS and 40 on Linux; ask the kernel for it rather than spell it.
    let eloop = fs::metadata(&loop_link).unwrap_err().raw_os_error();
    assert!(eloop.is_some());
    let options = ScanOptions::new(dir.path().to_path_buf());
    match rescan_path(&behind, &options) {
        Err(ScanError::Root { path, source }) => {
            assert_eq!(path, behind);
            assert_eq!(source.raw_os_error(), eloop);
        }
        other => panic!("expected a Root error, got {other:?}"),
    }
}

/// Entries of `/` that are not `keep`, so a scan of `/` stats its children and walks
/// nothing else. Excludes only ever hold directories back, which is all this needs.
fn siblings_at_the_root_of_the_volume(keep: &str) -> Vec<PathBuf> {
    fs::read_dir("/")
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path != Path::new(keep))
        .collect()
}

/// The only test that holds the same-volume rule of a patch, because a mount point cannot
/// be built into a fixture: it uses the one this machine already has. Do not delete it as
/// environment-dependent — without it, a rescan walks whole foreign volumes into a total
/// that describes one.
#[test]
fn rescan_of_another_volume_is_skipped_exactly_as_the_walker_skips_it() {
    // `/dev` is devfs on macOS and a container mount on Linux; `/` is the boot volume.
    // Asserted, not skipped around: a guard that returns early would leave the rule
    // untested on a machine where it silently stopped holding.
    let root = PathBuf::from("/");
    let other = PathBuf::from("/dev");
    assert_ne!(
        fs::metadata(&root).unwrap().dev(),
        fs::metadata(&other).unwrap().dev(),
        "no second volume to compare against"
    );
    let mut options = ScanOptions::new(root.clone());
    assert!(options.same_device, "the rule under test");
    let tree = rescan_path(&other, &options).unwrap().unwrap();
    assert_eq!(tree.len(), 1, "the other volume is not walked");
    assert_eq!(tree.error(Tree::ROOT), Some("skipped: different volume"));
    assert_eq!(tree.root().kind, NodeKind::Dir);
    assert_eq!(tree.root().file_count, 0);
    assert_eq!(&*tree.root().name, "/dev", "the root carries the path");

    // The leaf a real walk of `/` leaves behind for the same mount point, to the byte.
    options.excludes = siblings_at_the_root_of_the_volume("/dev");
    let full = scan(&options, &ScanProgress::default()).unwrap();
    let (leaf_id, leaf) = child(&full.tree, Tree::ROOT, "dev");
    assert_eq!(full.tree.error(leaf_id), tree.error(Tree::ROOT));
    assert_eq!(leaf.kind, tree.root().kind);
    assert_eq!(leaf.size, tree.root().size);
    assert_eq!(leaf.logical_size, tree.root().logical_size);
    assert_eq!(leaf.file_count, tree.root().file_count);
    assert_eq!(leaf.mtime, tree.root().mtime);
    assert!(!full.tree.has_children(leaf_id));

    // Without the rule the patch walks it, which is what the scan it joins never did.
    options.same_device = false;
    let walked = rescan_path(&other, &options).unwrap().unwrap();
    assert!(walked.len() > 1, "only {} nodes", walked.len());
    assert_eq!(walked.error(Tree::ROOT), None);

    // The rule is the walker's: it stops a *descent*, so it never touches an entry that is
    // not a directory, however foreign the volume that entry sits on.
    let options = ScanOptions::new(root);
    let null = rescan_path(Path::new("/dev/null"), &options)
        .unwrap()
        .unwrap();
    assert_eq!(null.len(), 1);
    assert_eq!(null.root().kind, NodeKind::Other);
    assert_eq!(null.error(Tree::ROOT), None, "a leaf is not skipped");
}

fn subtree_size(tree: &Tree, id: u32) -> usize {
    1 + tree
        .children(id)
        .map(|child| subtree_size(tree, child))
        .sum::<usize>()
}

/// Asserts that two subtrees hold the same nodes: same shape, sizes, counts and errors.
/// Names are compared per child; the roots are not, since a patch names its root with the
/// absolute path while the scan it joins holds the file name there.
fn assert_same_subtree(patch: &Tree, patch_id: u32, full: &Tree, full_id: u32) {
    let (a, b) = (patch.get(patch_id).unwrap(), full.get(full_id).unwrap());
    let where_ = full.path(full_id);
    let at = where_.display();
    assert_eq!(a.kind, b.kind, "kind at {at}");
    assert_eq!(a.size, b.size, "size at {at}");
    assert_eq!(a.logical_size, b.logical_size, "logical size at {at}");
    assert_eq!(a.file_count, b.file_count, "file count at {at}");
    assert_eq!(a.mtime, b.mtime, "mtime at {at}");
    assert_eq!(patch.error(patch_id), full.error(full_id), "error at {at}");
    assert_eq!(
        patch.child_count(patch_id),
        full.child_count(full_id),
        "children of {at}"
    );
    for (a_child, b_child) in patch.children(patch_id).zip(full.children(full_id)) {
        assert_eq!(
            &*patch.get(a_child).unwrap().name,
            &*full.get(b_child).unwrap().name,
            "child names under {at}"
        );
        assert_same_subtree(patch, a_child, full, b_child);
    }
}

#[test]
fn rescan_matches_the_branch_a_full_scan_produces() {
    // The property Task 7 splices on: a patch is the branch the walker would have built.
    // No hard links here on purpose — those are attributed across the whole scan, and a
    // patch cannot reproduce a decision that was made outside it.
    let as_root = unsafe { libc_geteuid() } == 0;
    let dir = fixture();
    let docs = dir.path().join("docs");
    write(&docs.join("deep/nested/leaf.bin"), 4_096);
    fs::create_dir_all(docs.join("empty")).unwrap();
    std::os::unix::fs::symlink(docs.join("report.txt"), docs.join("link")).unwrap();
    std::os::unix::fs::symlink(docs.join("gone"), docs.join("dangling")).unwrap();
    let locked = docs.join("locked");
    write(&locked.join("inside.bin"), 10);
    if !as_root {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    }

    let options = ScanOptions::new(dir.path().to_path_buf());
    let full = scan(&options, &ScanProgress::default());
    let patch = rescan_path(&docs, &options);
    if !as_root {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let full = full.unwrap().tree;
    let patch = patch.unwrap().unwrap();

    let (docs_id, _) = child(&full, Tree::ROOT, "docs");
    assert_same_subtree(&patch, Tree::ROOT, &full, docs_id);
    // Not a fixed number: as root the unreadable directory is readable and holds one node
    // more. The point is that the patch loses none of them.
    assert_eq!(patch.len(), subtree_size(&full, docs_id), "no node is lost");
    assert!(patch.len() >= 12, "a trivial fixture proves nothing");
    assert_eq!(&*patch.root().name, docs.to_str().unwrap());
    assert_eq!(&*full.get(docs_id).unwrap().name, "docs");
    if !as_root {
        assert_eq!(patch.errors().len(), 1, "the unreadable directory");
    }
}

#[test]
fn rescan_re_attributes_hard_links_inside_the_branch() {
    // A known limitation of patching rather than rescanning, and the expensive direction:
    // attribution runs over one branch, so a link whose bytes the full scan gave to a twin
    // outside takes them back, and splicing the patch makes the totals above it grow. The
    // next full scan corrects it; holding the whole tree's (dev, ino) set is what the
    // design rejected.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("a/z.bin"), 8_192);
    fs::create_dir_all(root.join("b")).unwrap();
    fs::hard_link(root.join("a/z.bin"), root.join("b/a.bin")).unwrap();
    let options = ScanOptions::new(root.to_path_buf());
    let full = scan(&options, &ScanProgress::default()).unwrap().tree;
    let (b, b_node) = child(&full, Tree::ROOT, "b");
    let (_, loser) = child(&full, b, "a.bin");
    assert_eq!(
        loser.size, 0,
        "a/z.bin is the smaller path and keeps the data"
    );
    let data = allocated(&root.join("a/z.bin"));

    let patch = rescan_path(&root.join("b"), &options).unwrap().unwrap();
    let (_, patched) = child(&patch, Tree::ROOT, "a.bin");
    assert_eq!(patched.size, data, "the link takes its bytes back");
    assert_eq!(
        patch.root().size,
        b_node.size + data,
        "so the branch weighs {data} bytes more than the scan says"
    );
}

unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}
