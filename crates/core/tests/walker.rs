use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use storage_monitor_core::scan::{Node, NodeKind, ScanOptions, ScanProgress, Tree, scan};
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
    write(&root.join("big.bin"), 20_000);
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
    assert_eq!(tree.root().logical_size, 3_000 + 100 + 200 + 50 + 20_000);
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
    assert_eq!(big.logical_size, 20_000);

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
    sorted.sort_unstable_by(|a, b| b.cmp(a));
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
    assert_eq!(a_node.size, allocated(&root.join("a")) + z.size);
    assert_eq!(b_node.size, allocated(&root.join("b")) + small.size);
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
fn cancelled_scan_stops_early_and_says_so() {
    let dir = fixture();
    let progress = ScanProgress::default();
    progress.cancel();
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &progress).unwrap();
    assert!(result.cancelled);
    assert!(!result.tree.has_children(Tree::ROOT));
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
fn root_must_be_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file.txt");
    write(&file, 1);
    assert!(scan(&ScanOptions::new(file), &ScanProgress::default()).is_err());
    assert!(
        scan(
            &ScanOptions::new(PathBuf::from("/definitely/missing")),
            &ScanProgress::default()
        )
        .is_err()
    );
}

unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}
