use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use storage_monitor_core::scan::{Node, NodeKind, ScanOptions, ScanProgress, Tree, scan};
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
        one.size + two.size,
        one.size.max(two.size),
        "one of the links contributes 0"
    );
    assert_eq!(result.stats.hardlinks_skipped, 1);
    // The root directory's own blocks count toward its size (0 on APFS, 4096 on ext4).
    let root_own = fs::symlink_metadata(root).unwrap().blocks() * 512;
    assert_eq!(result.tree.root().size, root_own + one.size.max(two.size));
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
