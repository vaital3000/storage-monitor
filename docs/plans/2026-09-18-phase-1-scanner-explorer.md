# Phase 1: Scanner, Snapshots and Explorer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or subagent-driven-development) to implement this plan task-by-task.

**Goal:** Ship `v0.2.0`: scanning the home folder from the app and the CLI, a persisted snapshot per scan with "what grew" deltas, and an Explorer screen with breadcrumbs, a sortable table and a treemap.

**Architecture:** `storage-monitor-core` gains `scan` (parallel walker into an arena tree), `snapshot` (compact persisted snapshots + deltas), `disk` (volume usage) and `paths`. The CLI exposes `storage-monitor scan`. The desktop crate owns a `ScanManager` (state + background thread + progress events) and thin commands returning view models. The UI gets an app shell (sidebar) and the Explorer page driven by TanStack Query over `src/lib/ipc.ts`, with the mock layer simulating a scan through mocked events.

**Tech Stack:** Rust (rayon, serde, postcard + lz4_flex, chrono, nix statvfs, dirs, thiserror), Tauri 2 (+ `tauri-plugin-opener` for "Reveal in Finder"), React 19, TanStack Query 5, ECharts 6 (treemap), lucide-react icons, Vitest, Playwright.

**Design reference:** `docs/plans/2026-09-17-storage-monitor-design.md` sections 6 (core), 10 (UI), 11 (testing), 14 (phase 1 exit criteria: scan of the home folder with treemap and deltas).

**Decisions made for this phase (not in the design):**
- One root per scan (the home folder by default). Multiple roots and settings UI come with phase 2.
- Snapshots keep every directory plus files of 10 MiB or more, serialized with `postcard` and compressed with `lz4_flex`; a sidecar `.json` holds the metadata so listing does not decode the payload. The last 10 snapshots are kept.
- Deltas compare the new scan with the previous snapshot by absolute path. "Top growers" skips a directory when one of its children explains 80% or more of its growth.
- "Reveal in Finder" ships now (opener plugin); "Move to Trash" waits for the action engine in phase 2.
- The table is hand-rolled (sortable, few columns); TanStack Table is not needed yet. Icons from `lucide-react`.
- The app shell has the five sidebar entries from the design; only Explorer is live, the others render a placeholder.

**Conventions for every task:**
- Branch `feat/phase-1-scanner-explorer` in a worktree under `.worktrees/`. Never commit to `main`. Commit per task with conventional messages.
- TDD for core, CLI and UI logic; e2e for the screen.
- IPC structs carry `#[serde(rename_all = "camelCase")]`; every new command gets a mock handler; every new command gets a TypeScript wrapper in `src/lib/ipc.ts`.
- Versions: add crates/packages with caret ranges, let lockfiles pin. Verified current versions on 2026-09-18: rayon 1.12, postcard 1.1, lz4_flex 0.14, nix 0.31, dirs 7, chrono 0.4, thiserror 2, tempfile 3.27, tauri-plugin-opener 2.5, echarts 6.1, @tanstack/react-query 5.103, @tauri-apps/plugin-opener 2.5, lucide-react 1.47.

---

### Task 1: Tree model in core

**Files:**
- Create: `crates/core/src/scan/mod.rs`, `crates/core/src/scan/tree.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod scan;`), `crates/core/Cargo.toml`

**Step 1: Dependencies**

Add to `[workspace.dependencies]` in the root `Cargo.toml`:
```toml
rayon = "1"
thiserror = "2"
chrono = { version = "0.4", features = ["serde"] }
postcard = { version = "1", features = ["alloc"] }
lz4_flex = "0.14"
nix = { version = "0.31", features = ["fs"] }
dirs = "7"
tempfile = "3"
```
And to `crates/core/Cargo.toml`:
```toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
rayon = { workspace = true }
thiserror = { workspace = true }
chrono = { workspace = true }
postcard = { workspace = true }
lz4_flex = { workspace = true }
nix = { workspace = true }
dirs = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

**Step 2: Write the failing tests in `crates/core/src/scan/tree.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, kind: NodeKind, size: u64) -> Node {
        Node {
            name: name.to_owned(),
            kind,
            parent: None,
            size,
            logical_size: size,
            file_count: if kind == NodeKind::Dir { 0 } else { 1 },
            mtime: 0,
            error: None,
            children: Vec::new(),
        }
    }

    fn sample() -> Tree {
        // /root
        //   a/        (dir)
        //     x.bin   (30)
        //     y.bin    (5)
        //   b.bin     (10)
        let root = Subtree {
            node: node("/root", NodeKind::Dir, 0),
            children: vec![
                Subtree {
                    node: node("a", NodeKind::Dir, 0),
                    children: vec![
                        Subtree { node: node("y.bin", NodeKind::File, 5), children: vec![] },
                        Subtree { node: node("x.bin", NodeKind::File, 30), children: vec![] },
                    ],
                },
                Subtree { node: node("b.bin", NodeKind::File, 10), children: vec![] },
            ],
        };
        let mut tree = root.flatten();
        tree.sort_children_by_size();
        tree
    }

    #[test]
    fn flatten_assigns_parents_and_children() {
        let tree = sample();
        assert_eq!(tree.len(), 5);
        let root = tree.root();
        assert_eq!(root.parent, None);
        assert_eq!(root.children.len(), 2);
        let a = tree.get(root.children[0]).unwrap();
        assert_eq!(a.name, "a");
        assert_eq!(a.parent, Some(Tree::ROOT));
        assert_eq!(a.children.len(), 2);
    }

    #[test]
    fn children_are_sorted_by_size_descending() {
        let tree = sample();
        let names: Vec<&str> = tree
            .children(Tree::ROOT)
            .iter()
            .map(|id| tree.get(*id).unwrap().name.as_str())
            .collect();
        // a is a dir with size 0 here (aggregation happens in the walker), so b.bin (10) first
        assert_eq!(names, vec!["b.bin", "a"]);
        let a = tree.root().children[1];
        let inner: Vec<&str> = tree.children(a).iter().map(|id| tree.get(*id).unwrap().name.as_str()).collect();
        assert_eq!(inner, vec!["x.bin", "y.bin"]);
    }

    #[test]
    fn path_and_ancestors_are_reconstructed_from_names() {
        let tree = sample();
        let a = tree.root().children[1];
        let x = tree.children(a)[0];
        assert_eq!(tree.path(x), std::path::PathBuf::from("/root/a/x.bin"));
        assert_eq!(tree.ancestors(x), vec![Tree::ROOT, a, x]);
        assert_eq!(tree.path(Tree::ROOT), std::path::PathBuf::from("/root"));
    }

    #[test]
    fn node_kind_serializes_in_camel_case() {
        assert_eq!(serde_json::to_string(&NodeKind::Dir).unwrap(), "\"dir\"");
        assert_eq!(serde_json::to_string(&NodeKind::Symlink).unwrap(), "\"symlink\"");
    }
}
```

**Step 3: Run to verify failure**

Run: `cargo test -p storage-monitor-core scan::tree`
Expected: compile errors (`Node`, `Tree`, `Subtree` missing).

**Step 4: Implement `crates/core/src/scan/tree.rs`**

```rust
//! Arena representation of a scanned directory tree.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Index of a node inside a [`Tree`].
pub type NodeId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// File name; the root node holds its absolute path instead.
    pub name: String,
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// Allocated bytes on disk (`st_blocks * 512`); for directories the subtree total.
    pub size: u64,
    /// Logical bytes (`st_size`); for directories the subtree total.
    pub logical_size: u64,
    /// Entries in the subtree that are not directories (1 for a file, 0 for an empty directory).
    pub file_count: u64,
    /// Modification time in seconds since the Unix epoch.
    pub mtime: i64,
    /// Set when a directory could not be read (fully).
    pub error: Option<String>,
    /// Children, sorted by `size` descending after [`Tree::sort_children_by_size`].
    pub children: Vec<NodeId>,
}

/// A scanned tree stored as an arena. The root is always node 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    pub const ROOT: NodeId = 0;

    pub fn from_nodes(nodes: Vec<Node>) -> Self {
        assert!(!nodes.is_empty(), "a tree needs at least a root node");
        Self { nodes }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize)
    }

    pub fn root(&self) -> &Node {
        &self.nodes[Self::ROOT as usize]
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.get(id).map(|n| n.children.as_slice()).unwrap_or(&[])
    }

    /// Ids from the root down to `id`, inclusive.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut current = id;
        while let Some(parent) = self.get(current).and_then(|n| n.parent) {
            chain.push(parent);
            current = parent;
        }
        chain.reverse();
        chain
    }

    /// Absolute path of a node, rebuilt from the root's absolute name.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut path = PathBuf::new();
        for ancestor in self.ancestors(id) {
            if let Some(node) = self.get(ancestor) {
                path.push(Path::new(&node.name));
            }
        }
        path
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().map(|(i, n)| (i as NodeId, n))
    }

    pub fn sort_children_by_size(&mut self) {
        let sizes: Vec<u64> = self.nodes.iter().map(|n| n.size).collect();
        for node in &mut self.nodes {
            node.children
                .sort_by(|a, b| sizes[*b as usize].cmp(&sizes[*a as usize]).then_with(|| a.cmp(b)));
        }
    }
}

/// Nested form produced by the walker before it is flattened into an arena.
#[derive(Debug)]
pub struct Subtree {
    pub node: Node,
    pub children: Vec<Subtree>,
}

impl Subtree {
    pub fn flatten(self) -> Tree {
        let mut nodes = Vec::new();
        push(&mut nodes, self, None);
        Tree::from_nodes(nodes)
    }
}

fn push(nodes: &mut Vec<Node>, subtree: Subtree, parent: Option<NodeId>) -> NodeId {
    let id = nodes.len() as NodeId;
    let Subtree { mut node, children } = subtree;
    node.parent = parent;
    node.children = Vec::new();
    nodes.push(node);
    let ids: Vec<NodeId> = children.into_iter().map(|c| push(nodes, c, Some(id))).collect();
    nodes[id as usize].children = ids;
    id
}
```

`crates/core/src/scan/mod.rs`:
```rust
//! Directory scanning: a parallel walker that produces an arena [`Tree`].

mod progress;
mod tree;
mod walker;

pub use progress::{ProgressSnapshot, ScanProgress};
pub use tree::{Node, NodeId, NodeKind, Subtree, Tree};
pub use walker::{ScanError, ScanOptions, ScanResult, ScanStats, scan};
```
(Create empty `progress.rs` and `walker.rs` with only a doc comment for now so the module compiles; Task 2 fills them.) Add `pub mod scan;` to `crates/core/src/lib.rs`.

**Step 5: Run tests, lint, commit**

Run: `cargo test -p storage-monitor-core` (expected: previous 2 + 4 new tests pass) and `cargo clippy --workspace --all-targets -- -D warnings`.

```bash
git add Cargo.toml Cargo.lock crates/core
git commit -m "feat(core): add arena tree model for scans"
```

---

### Task 2: Parallel walker with progress, hard links, errors and cancellation

**Files:**
- Create: `crates/core/src/scan/progress.rs`, `crates/core/src/scan/walker.rs`, `crates/core/tests/walker.rs`

**Step 1: Write `crates/core/src/scan/progress.rs`** (tested through the walker tests)

```rust
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Live counters shared between a running scan and its observers.
#[derive(Debug, Default)]
pub struct ScanProgress {
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    cancelled: AtomicBool,
    current: Mutex<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressSnapshot {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub current_path: String,
    pub cancelled: bool,
}

impl ScanProgress {
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            current_path: self.current.lock().map(|c| c.clone()).unwrap_or_default(),
            cancelled: self.is_cancelled(),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub(crate) fn add_file(&self, bytes: u64) {
        self.files.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn add_dir(&self, bytes: u64) {
        self.dirs.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn add_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn enter(&self, path: &std::path::Path) {
        if let Ok(mut current) = self.current.lock() {
            current.clear();
            current.push_str(&path.to_string_lossy());
        }
    }
}
```

**Step 2: Write the failing integration tests `crates/core/tests/walker.rs`**

```rust
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use storage_monitor_core::scan::{NodeKind, ScanOptions, ScanProgress, Tree, scan};
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

fn child<'a>(tree: &'a Tree, parent: u32, name: &str) -> (u32, &'a storage_monitor_core::scan::Node) {
    tree.children(parent)
        .iter()
        .map(|id| (*id, tree.get(*id).unwrap()))
        .find(|(_, n)| n.name == name)
        .unwrap_or_else(|| panic!("no child {name}"))
}

#[test]
fn scans_a_tree_with_logical_sizes_counts_and_kinds() {
    let dir = fixture();
    let progress = ScanProgress::default();
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &progress).unwrap();
    let tree = &result.tree;

    assert_eq!(tree.root().name, dir.path().to_string_lossy());
    assert_eq!(tree.root().kind, NodeKind::Dir);
    assert_eq!(tree.root().logical_size, 3_000 + 100 + 200 + 50 + 20_000);
    assert!(tree.root().size >= tree.root().logical_size, "allocated size counts whole blocks");
    assert_eq!(tree.root().file_count, 5);

    let (docs, docs_node) = child(tree, Tree::ROOT, "docs");
    assert_eq!(docs_node.logical_size, 3_300);
    assert_eq!(docs_node.file_count, 3);
    let (_, notes) = child(tree, docs, "notes");
    assert_eq!(notes.file_count, 2);
    let (_, hidden) = child(tree, Tree::ROOT, ".hidden");
    assert_eq!(hidden.file_count, 1, "hidden entries are scanned");
    let (_, empty) = child(tree, Tree::ROOT, "empty");
    assert_eq!(empty.file_count, 0);
    assert!(empty.children.is_empty());
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
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &ScanProgress::default()).unwrap();
    let tree = &result.tree;
    let sizes: Vec<u64> = tree.children(Tree::ROOT).iter().map(|id| tree.get(*id).unwrap().size).collect();
    let mut sorted = sizes.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(sizes, sorted);
    assert_eq!(tree.get(tree.children(Tree::ROOT)[0]).unwrap().name, "big.bin");
}

#[test]
fn symlinks_are_recorded_but_never_followed() {
    let dir = fixture();
    let root = dir.path();
    std::os::unix::fs::symlink(root.join("docs"), root.join("docs-link")).unwrap();
    std::os::unix::fs::symlink(root, root.join("loop")).unwrap();
    let result = scan(&ScanOptions::new(root.to_path_buf()), &ScanProgress::default()).unwrap();
    let tree = &result.tree;
    let (_, link) = child(tree, Tree::ROOT, "docs-link");
    assert_eq!(link.kind, NodeKind::Symlink);
    assert!(link.children.is_empty());
    assert!(link.logical_size < 1_000);
    let (_, looped) = child(tree, Tree::ROOT, "loop");
    assert!(looped.children.is_empty());
    assert_eq!(result.stats.files, 7, "symlinks count as entries");
}

#[test]
fn hard_links_are_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("one.bin"), 8_192);
    fs::hard_link(root.join("one.bin"), root.join("two.bin")).unwrap();
    let result = scan(&ScanOptions::new(root.to_path_buf()), &ScanProgress::default()).unwrap();
    let (_, one) = child(&result.tree, Tree::ROOT, "one.bin");
    let (_, two) = child(&result.tree, Tree::ROOT, "two.bin");
    assert_eq!(one.size + two.size, one.size.max(two.size), "one of the links contributes 0");
    assert_eq!(result.stats.hardlinks_skipped, 1);
    assert_eq!(result.tree.root().size, one.size.max(two.size));
}

#[test]
fn sparse_files_report_allocated_size_below_logical_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparse.bin");
    let f = fs::File::create(&path).unwrap();
    f.set_len(50 * 1024 * 1024).unwrap();
    drop(f);
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &ScanProgress::default()).unwrap();
    let (_, sparse) = child(&result.tree, Tree::ROOT, "sparse.bin");
    assert_eq!(sparse.logical_size, 50 * 1024 * 1024);
    assert!(sparse.size < sparse.logical_size, "allocated {} should be below logical", sparse.size);
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
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &ScanProgress::default());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let result = result.unwrap();
    let (_, node) = child(&result.tree, Tree::ROOT, "locked");
    assert!(node.error.as_deref().unwrap_or("").contains("ermission"), "error: {:?}", node.error);
    assert_eq!(result.stats.errors, 1);
    assert_eq!(result.stats.files, 5, "the rest of the tree is still scanned");
}

#[test]
fn excluded_directories_are_skipped() {
    let dir = fixture();
    let mut options = ScanOptions::new(dir.path().to_path_buf());
    options.excludes.push(dir.path().join("docs"));
    let result = scan(&options, &ScanProgress::default()).unwrap();
    let names: Vec<String> = result.tree.children(Tree::ROOT).iter().map(|id| result.tree.get(*id).unwrap().name.clone()).collect();
    assert!(!names.contains(&"docs".to_owned()));
    assert_eq!(result.stats.files, 2);
}

#[test]
fn cancelled_scan_stops_early_and_says_so() {
    let dir = fixture();
    let progress = ScanProgress::default();
    progress.cancel();
    let result = scan(&ScanOptions::new(dir.path().to_path_buf()), &progress).unwrap();
    assert!(result.cancelled);
    assert!(result.tree.root().children.is_empty());
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
    assert!(scan(&ScanOptions::new(PathBuf::from("/definitely/missing")), &ScanProgress::default()).is_err());
}

unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}
```

**Step 3: Run to verify failure**

Run: `cargo test -p storage-monitor-core --test walker`
Expected: compile errors (`scan`, `ScanOptions` missing).

**Step 4: Implement `crates/core/src/scan/walker.rs`**

```rust
use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::progress::ScanProgress;
use super::tree::{Node, NodeKind, Subtree, Tree};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub root: PathBuf,
    /// Absolute directory paths that are not entered (and not listed).
    pub excludes: Vec<PathBuf>,
    /// Do not descend into directories on another volume.
    pub same_device: bool,
}

impl ScanOptions {
    pub fn new(root: PathBuf) -> Self {
        Self { root, excludes: Vec::new(), same_device: true }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    #[error("cannot read {path}: {source}")]
    Root { path: PathBuf, source: std::io::Error },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub hardlinks_skipped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub root: PathBuf,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub stats: ScanStats,
    pub cancelled: bool,
    pub tree: Tree,
}

struct Ctx<'a> {
    options: &'a ScanOptions,
    progress: &'a ScanProgress,
    root_dev: u64,
    seen_inodes: Mutex<HashSet<(u64, u64)>>,
    hardlinks_skipped: AtomicU64,
}

impl Ctx<'_> {
    fn is_excluded(&self, path: &Path) -> bool {
        self.options.excludes.iter().any(|e| e == path)
    }

    /// True the first time an inode is seen; false for later hard links to it.
    fn first_sighting(&self, dev: u64, ino: u64) -> bool {
        self.seen_inodes.lock().map(|mut set| set.insert((dev, ino))).unwrap_or(true)
    }
}

/// Scans `options.root` in parallel. Never follows symlinks, stays on the root's volume,
/// counts hard-linked data once and records unreadable directories instead of failing.
pub fn scan(options: &ScanOptions, progress: &ScanProgress) -> Result<ScanResult, ScanError> {
    let started = Instant::now();
    let started_at = Utc::now();
    let root = &options.root;
    let meta = fs::symlink_metadata(root).map_err(|source| ScanError::Root { path: root.clone(), source })?;
    if !meta.is_dir() {
        return Err(ScanError::NotADirectory(root.clone()));
    }
    let ctx = Ctx {
        options,
        progress,
        root_dev: meta.dev(),
        seen_inodes: Mutex::new(HashSet::new()),
        hardlinks_skipped: AtomicU64::new(0),
    };
    let root_node = node_from_metadata(root.to_string_lossy().into_owned(), &meta);
    let subtree = walk_dir(root, root_node, &ctx);
    let mut tree = subtree.flatten();
    tree.sort_children_by_size();
    let snap = progress.snapshot();
    Ok(ScanResult {
        root: root.clone(),
        started_at,
        duration_ms: started.elapsed().as_millis() as u64,
        stats: ScanStats {
            files: snap.files,
            dirs: snap.dirs,
            bytes: snap.bytes,
            errors: snap.errors,
            hardlinks_skipped: ctx.hardlinks_skipped.load(Ordering::Relaxed),
        },
        cancelled: snap.cancelled,
        tree,
    })
}

fn walk_dir(path: &Path, mut node: Node, ctx: &Ctx) -> Subtree {
    ctx.progress.enter(path);
    if ctx.progress.is_cancelled() {
        return Subtree { node, children: Vec::new() };
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) => {
            node.error = Some(err.to_string());
            ctx.progress.add_error();
            ctx.progress.add_dir(node.size);
            return Subtree { node, children: Vec::new() };
        }
    };

    let mut dirs: Vec<(PathBuf, Node)> = Vec::new();
    let mut leaves: Vec<Subtree> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                ctx.progress.add_error();
                continue;
            }
        };
        let child_path = entry.path();
        let meta = match fs::symlink_metadata(&child_path) {
            Ok(meta) => meta,
            Err(_) => {
                ctx.progress.add_error();
                continue;
            }
        };
        let mut child = node_from_metadata(entry.file_name().to_string_lossy().into_owned(), &meta);
        if meta.is_dir() {
            if ctx.is_excluded(&child_path) {
                continue;
            }
            if ctx.options.same_device && meta.dev() != ctx.root_dev {
                child.error = Some("skipped: different volume".to_owned());
                ctx.progress.add_dir(child.size);
                leaves.push(Subtree { node: child, children: Vec::new() });
                continue;
            }
            dirs.push((child_path, child));
        } else {
            if meta.nlink() > 1 && !ctx.first_sighting(meta.dev(), meta.ino()) {
                child.size = 0;
                ctx.hardlinks_skipped.fetch_add(1, Ordering::Relaxed);
            }
            ctx.progress.add_file(child.size);
            leaves.push(Subtree { node: child, children: Vec::new() });
        }
    }

    let subdirs: Vec<Subtree> = dirs.into_par_iter().map(|(p, n)| walk_dir(&p, n, ctx)).collect();

    let mut children = leaves;
    children.extend(subdirs);
    ctx.progress.add_dir(node.size);
    for child in &children {
        node.size += child.node.size;
        node.logical_size += child.node.logical_size;
        node.file_count += child.node.file_count;
    }
    Subtree { node, children }
}

fn node_from_metadata(name: String, meta: &Metadata) -> Node {
    let kind = if meta.is_dir() {
        NodeKind::Dir
    } else if meta.file_type().is_symlink() {
        NodeKind::Symlink
    } else if meta.is_file() {
        NodeKind::File
    } else {
        NodeKind::Other
    };
    Node {
        name,
        kind,
        parent: None,
        size: meta.blocks() * 512,
        logical_size: if kind == NodeKind::Dir { 0 } else { meta.len() },
        file_count: if kind == NodeKind::Dir { 0 } else { 1 },
        mtime: meta.mtime(),
        error: None,
        children: Vec::new(),
    }
}
```

Notes for the executor: `progress.bytes` equals the root's `size` because every node's own allocated size is added exactly once (`add_file` / `add_dir`). Directories' own blocks count toward the total, which matches `du`.

**Step 5: Run the tests**

Run: `cargo test -p storage-monitor-core --test walker`
Expected: 10 passed. If `sparse_files_report_allocated_size_below_logical_size` fails on a filesystem without sparse support, the test filesystem is the problem; macOS APFS and Linux ext4/tmpfs both support it.

**Step 6: Lint and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`.

```bash
git add crates/core
git commit -m "feat(core): parallel directory walker with progress, hard links and errors"
```

---

### Task 3: Snapshots, store and deltas

**Files:**
- Create: `crates/core/src/snapshot/mod.rs`, `crates/core/src/snapshot/model.rs`, `crates/core/src/snapshot/store.rs`, `crates/core/src/snapshot/delta.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod snapshot;`)

**Step 1: Write the failing tests** (inline `#[cfg(test)]` modules in each file; the store tests use `tempfile`)

`model.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Node, NodeKind, Subtree, Tree};
    use crate::scan::{ScanResult, ScanStats};

    fn node(name: &str, kind: NodeKind, size: u64) -> Node {
        Node { name: name.into(), kind, parent: None, size, logical_size: size, file_count: u64::from(kind != NodeKind::Dir), mtime: 7, error: None, children: vec![] }
    }

    fn result() -> ScanResult {
        let tree = Subtree {
            node: node("/home", NodeKind::Dir, 0),
            children: vec![
                Subtree { node: node("small.txt", NodeKind::File, 10), children: vec![] },
                Subtree { node: node("big.iso", NodeKind::File, 20_000), children: vec![] },
                Subtree { node: node("cache", NodeKind::Dir, 0), children: vec![Subtree { node: node("blob", NodeKind::File, 5_000), children: vec![] }] },
            ],
        }
        .flatten();
        ScanResult { root: "/home".into(), started_at: chrono::Utc::now(), duration_ms: 1, stats: ScanStats::default(), cancelled: false, tree }
    }

    #[test]
    fn snapshot_keeps_directories_and_files_above_threshold() {
        let snap = Snapshot::from_result(&result(), 1_000);
        let paths: Vec<&str> = snap.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/home", "/home/big.iso", "/home/cache", "/home/cache/blob"]);
        assert_eq!(snap.file_threshold, 1_000);
        assert_eq!(snap.format, SNAPSHOT_FORMAT);
    }

    #[test]
    fn snapshot_round_trips_through_bytes() {
        let snap = Snapshot::from_result(&result(), 1_000);
        let bytes = snap.encode().unwrap();
        let back = Snapshot::decode(&bytes).unwrap();
        assert_eq!(back.entries, snap.entries);
        assert_eq!(back.root, snap.root);
    }

    #[test]
    fn size_index_maps_paths_to_sizes() {
        let snap = Snapshot::from_result(&result(), 1_000);
        let index = snap.size_index();
        assert_eq!(index.get("/home/big.iso"), Some(&20_000));
        assert_eq!(index.get("/home/small.txt"), None);
    }
}
```

`store.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::model::{Snapshot, SnapshotEntry};
    use crate::scan::NodeKind;

    fn snapshot(taken_at: chrono::DateTime<chrono::Utc>, total: u64) -> Snapshot {
        Snapshot { format: super::super::model::SNAPSHOT_FORMAT, taken_at, root: "/home".into(), total_bytes: total, file_count: 1, file_threshold: 0,
            entries: vec![SnapshotEntry { path: "/home".into(), kind: NodeKind::Dir, size: total, file_count: 1, mtime: 0 }] }
    }

    #[test]
    fn save_list_load_and_prune() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("snapshots"));
        let t0 = chrono::Utc::now() - chrono::Duration::minutes(3);
        for i in 0..3u64 {
            store.save(&snapshot(t0 + chrono::Duration::minutes(i as i64), i)).unwrap();
        }
        let list = store.list().unwrap();
        assert_eq!(list.len(), 3);
        assert!(list[0].taken_at > list[1].taken_at, "newest first");
        assert_eq!(list[0].total_bytes, 2);
        let latest = store.latest().unwrap().unwrap();
        assert_eq!(latest.total_bytes, 2);
        assert_eq!(store.load(&list[2].path).unwrap().total_bytes, 0);
        assert_eq!(store.prune(2).unwrap(), 1);
        assert_eq!(store.list().unwrap().len(), 2);
        assert_eq!(store.list().unwrap()[1].total_bytes, 1);
    }

    #[test]
    fn empty_store_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("missing"));
        assert!(store.list().unwrap().is_empty());
        assert!(store.latest().unwrap().is_none());
    }
}
```

`delta.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;
    use crate::snapshot::model::{SNAPSHOT_FORMAT, Snapshot, SnapshotEntry};

    fn snap(entries: &[(&str, NodeKind, u64)]) -> Snapshot {
        Snapshot { format: SNAPSHOT_FORMAT, taken_at: chrono::Utc::now(), root: "/h".into(), total_bytes: 0, file_count: 0, file_threshold: 0,
            entries: entries.iter().map(|(p, k, s)| SnapshotEntry { path: (*p).into(), kind: *k, size: *s, file_count: 0, mtime: 0 }).collect() }
    }

    #[test]
    fn deltas_cover_added_removed_and_changed_paths() {
        let before = snap(&[("/h", NodeKind::Dir, 100), ("/h/a", NodeKind::Dir, 60), ("/h/gone", NodeKind::Dir, 40)]);
        let after = snap(&[("/h", NodeKind::Dir, 150), ("/h/a", NodeKind::Dir, 110), ("/h/new", NodeKind::Dir, 40)]);
        let d = deltas(&before, &after);
        let find = |p: &str| d.iter().find(|x| x.path == p).unwrap();
        assert_eq!(find("/h").delta, 50);
        assert_eq!(find("/h/a").delta, 50);
        assert_eq!(find("/h/gone").delta, -40);
        assert_eq!(find("/h/new").delta, 40);
        assert_eq!(find("/h/new").before, 0);
    }

    #[test]
    fn top_growers_prefer_the_child_that_explains_the_growth() {
        let before = snap(&[("/h", NodeKind::Dir, 100), ("/h/lib", NodeKind::Dir, 50), ("/h/lib/xcode", NodeKind::Dir, 40), ("/h/docs", NodeKind::Dir, 50)]);
        let after = snap(&[("/h", NodeKind::Dir, 200), ("/h/lib", NodeKind::Dir, 140), ("/h/lib/xcode", NodeKind::Dir, 125), ("/h/docs", NodeKind::Dir, 60)]);
        let top = top_growers(&deltas(&before, &after), 10);
        let paths: Vec<&str> = top.iter().map(|d| d.path.as_str()).collect();
        // /h (+100) is explained by /h/lib (+90 >= 80%); /h/lib (+90) by /h/lib/xcode (+85); /h/docs (+10) stands alone
        assert_eq!(paths, vec!["/h/lib/xcode", "/h/docs"]);
    }

    #[test]
    fn top_growers_ignores_shrinking_and_files_and_respects_limit() {
        let before = snap(&[("/h", NodeKind::Dir, 100), ("/h/a", NodeKind::Dir, 10), ("/h/b", NodeKind::Dir, 10), ("/h/f.iso", NodeKind::File, 80)]);
        let after = snap(&[("/h", NodeKind::Dir, 90), ("/h/a", NodeKind::Dir, 30), ("/h/b", NodeKind::Dir, 20), ("/h/f.iso", NodeKind::File, 40)]);
        let top = top_growers(&deltas(&before, &after), 1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].path, "/h/a");
    }
}
```

**Step 2: Run to verify failure**: `cargo test -p storage-monitor-core snapshot` → compile errors.

**Step 3: Implement**

`crates/core/src/snapshot/mod.rs`:
```rust
//! Persisted scan snapshots and the deltas between them.

mod delta;
mod model;
mod store;

pub use delta::{Delta, deltas, top_growers};
pub use model::{SNAPSHOT_FORMAT, Snapshot, SnapshotEntry};
pub use store::{SnapshotMeta, SnapshotStore};
```

`crates/core/src/snapshot/model.rs`:
```rust
use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scan::{NodeKind, ScanResult, Tree};

/// Bump when the encoded layout changes; older files are ignored by the store.
pub const SNAPSHOT_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEntry {
    /// Absolute path.
    pub path: String,
    pub kind: NodeKind,
    pub size: u64,
    pub file_count: u64,
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub format: u32,
    pub taken_at: DateTime<Utc>,
    pub root: PathBuf,
    pub total_bytes: u64,
    pub file_count: u64,
    /// Files smaller than this were not recorded.
    pub file_threshold: u64,
    pub entries: Vec<SnapshotEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("encode: {0}")]
    Encode(#[from] postcard::Error),
    #[error("decompress: {0}")]
    Decompress(#[from] lz4_flex::block::DecompressError),
    #[error("unsupported snapshot format {0}")]
    Format(u32),
}

impl Snapshot {
    /// Every directory plus files whose allocated size is at least `file_threshold`.
    pub fn from_result(result: &ScanResult, file_threshold: u64) -> Self {
        let tree: &Tree = &result.tree;
        let mut entries = Vec::new();
        for (id, node) in tree.iter() {
            let keep = node.kind == NodeKind::Dir || node.size >= file_threshold;
            if keep {
                entries.push(SnapshotEntry {
                    path: tree.path(id).to_string_lossy().into_owned(),
                    kind: node.kind,
                    size: node.size,
                    file_count: node.file_count,
                    mtime: node.mtime,
                });
            }
        }
        Self {
            format: SNAPSHOT_FORMAT,
            taken_at: result.started_at,
            root: result.root.clone(),
            total_bytes: tree.root().size,
            file_count: tree.root().file_count,
            file_threshold,
            entries,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let raw = postcard::to_allocvec(self)?;
        Ok(lz4_flex::compress_prepend_size(&raw))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let raw = lz4_flex::decompress_size_prepended(bytes)?;
        let snapshot: Snapshot = postcard::from_bytes(&raw)?;
        if snapshot.format != SNAPSHOT_FORMAT {
            return Err(CodecError::Format(snapshot.format));
        }
        Ok(snapshot)
    }

    pub fn size_index(&self) -> HashMap<String, u64> {
        self.entries.iter().map(|e| (e.path.clone(), e.size)).collect()
    }
}
```

`crates/core/src/snapshot/store.rs`:
```rust
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::model::Snapshot;

/// Metadata written next to each snapshot so listing never decodes payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    #[serde(skip)]
    pub path: PathBuf,
    pub taken_at: DateTime<Utc>,
    pub root: PathBuf,
    pub total_bytes: u64,
    pub file_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Codec(#[from] super::model::CodecError),
    #[error("meta: {0}")]
    Meta(#[from] serde_json::Error),
}

/// A directory of `<stamp>.snap` (payload) and `<stamp>.json` (meta) pairs.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self, snapshot: &Snapshot) -> Result<PathBuf, StoreError> {
        fs::create_dir_all(&self.dir)?;
        let stamp = snapshot.taken_at.format("%Y%m%dT%H%M%S%.3fZ").to_string();
        let path = self.dir.join(format!("{stamp}.snap"));
        fs::write(&path, snapshot.encode()?)?;
        let meta = SnapshotMeta {
            path: path.clone(),
            taken_at: snapshot.taken_at,
            root: snapshot.root.clone(),
            total_bytes: snapshot.total_bytes,
            file_count: snapshot.file_count,
        };
        fs::write(path.with_extension("json"), serde_json::to_vec_pretty(&meta)?)?;
        Ok(path)
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<SnapshotMeta>, StoreError> {
        let mut metas = Vec::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(metas),
            Err(err) => return Err(err.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let snap_path = path.with_extension("snap");
            if !snap_path.exists() {
                continue;
            }
            let mut meta: SnapshotMeta = serde_json::from_slice(&fs::read(&path)?)?;
            meta.path = snap_path;
            metas.push(meta);
        }
        metas.sort_by(|a, b| b.taken_at.cmp(&a.taken_at));
        Ok(metas)
    }

    pub fn load(&self, path: &Path) -> Result<Snapshot, StoreError> {
        Ok(Snapshot::decode(&fs::read(path)?)?)
    }

    pub fn latest(&self) -> Result<Option<Snapshot>, StoreError> {
        match self.list()?.first() {
            Some(meta) => Ok(Some(self.load(&meta.path)?)),
            None => Ok(None),
        }
    }

    /// Deletes everything but the `keep` newest snapshots; returns how many were removed.
    pub fn prune(&self, keep: usize) -> Result<usize, StoreError> {
        let list = self.list()?;
        let mut removed = 0;
        for meta in list.iter().skip(keep) {
            fs::remove_file(&meta.path)?;
            let _ = fs::remove_file(meta.path.with_extension("json"));
            removed += 1;
        }
        Ok(removed)
    }
}
```

`crates/core/src/snapshot/delta.rs`:
```rust
use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::Snapshot;
use crate::scan::NodeKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    pub path: String,
    pub kind: NodeKind,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
}

/// One entry per path present in either snapshot; absent paths count as 0.
pub fn deltas(previous: &Snapshot, current: &Snapshot) -> Vec<Delta> {
    let mut before: HashMap<&str, (NodeKind, u64)> =
        previous.entries.iter().map(|e| (e.path.as_str(), (e.kind, e.size))).collect();
    let mut out = Vec::with_capacity(current.entries.len());
    for entry in &current.entries {
        let b = before.remove(entry.path.as_str()).map(|(_, s)| s).unwrap_or(0);
        out.push(Delta { path: entry.path.clone(), kind: entry.kind, before: b, after: entry.size, delta: entry.size as i64 - b as i64 });
    }
    for (path, (kind, size)) in before {
        out.push(Delta { path: path.to_owned(), kind, before: size, after: 0, delta: -(size as i64) });
    }
    out
}

/// Growing directories, largest first, skipping a directory when one child explains
/// at least 80% of its growth (the child is listed instead).
pub fn top_growers(deltas: &[Delta], limit: usize) -> Vec<Delta> {
    let mut max_child_delta: HashMap<&str, i64> = HashMap::new();
    for d in deltas.iter().filter(|d| d.kind == NodeKind::Dir && d.delta > 0) {
        if let Some(parent) = Path::new(&d.path).parent().and_then(|p| p.to_str()) {
            let slot = max_child_delta.entry(parent).or_insert(0);
            *slot = (*slot).max(d.delta);
        }
    }
    let mut growers: Vec<Delta> = deltas
        .iter()
        .filter(|d| d.kind == NodeKind::Dir && d.delta > 0)
        .filter(|d| {
            let biggest_child = max_child_delta.get(d.path.as_str()).copied().unwrap_or(0);
            (biggest_child as f64) < 0.8 * d.delta as f64
        })
        .cloned()
        .collect();
    growers.sort_by(|a, b| b.delta.cmp(&a.delta).then_with(|| a.path.cmp(&b.path)));
    growers.truncate(limit);
    growers
}
```

Add `pub mod snapshot;` to `crates/core/src/lib.rs`.

**Step 4: Run tests, lint, commit**

Run: `cargo test -p storage-monitor-core` (expected: all previous + 8 new pass), fmt, clippy.

```bash
git add crates/core
git commit -m "feat(core): snapshots with a file store and growth deltas"
```

---

### Task 4: Disk usage and application paths

**Files:**
- Create: `crates/core/src/disk.rs`, `crates/core/src/paths.rs`
- Modify: `crates/core/src/lib.rs`

**Step 1: Failing tests** (inline)

`disk.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_of_the_temp_dir_is_consistent() {
        let usage = disk_usage(std::env::temp_dir().as_path()).unwrap();
        assert!(usage.total > 0);
        assert!(usage.available <= usage.total);
        assert_eq!(usage.used + usage.free, usage.total);
    }

    #[test]
    fn usage_of_a_missing_path_is_an_error() {
        assert!(disk_usage(std::path::Path::new("/definitely/missing")).is_err());
    }
}
```

`paths.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_honors_the_environment_override() {
        let dir = with_data_dir_override(Some("/tmp/sm-test-data"));
        assert_eq!(dir, std::path::PathBuf::from("/tmp/sm-test-data"));
    }

    #[test]
    fn snapshots_dir_is_inside_the_data_dir() {
        let dir = with_data_dir_override(Some("/tmp/sm-test-data"));
        assert_eq!(snapshots_dir_in(&dir), std::path::PathBuf::from("/tmp/sm-test-data/snapshots"));
    }
}
```

**Step 2: Implement**

`crates/core/src/disk.rs`:
```rust
//! Volume usage through `statvfs`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub path: PathBuf,
    pub total: u64,
    /// Bytes free for this user (`f_bavail`).
    pub available: u64,
    /// Bytes free overall (`f_bfree`).
    pub free: u64,
    pub used: u64,
}

pub fn disk_usage(path: &Path) -> io::Result<DiskUsage> {
    let stat = nix::sys::statvfs::statvfs(path).map_err(io::Error::from)?;
    let frag = stat.fragment_size() as u64;
    let total = stat.blocks() as u64 * frag;
    let free = stat.blocks_free() as u64 * frag;
    let available = stat.blocks_available() as u64 * frag;
    Ok(DiskUsage { path: path.to_path_buf(), total, available, free, used: total.saturating_sub(free) })
}
```
(If the installed `nix` returns different integer widths, cast with `as u64` as above; if `io::Error::from(nix::Error)` is not available, use `io::Error::from_raw_os_error(err as i32)`.)

`crates/core/src/paths.rs`:
```rust
//! Where the app keeps its data.

use std::path::{Path, PathBuf};

pub const DATA_DIR_ENV: &str = "STORAGE_MONITOR_DATA_DIR";

/// `$STORAGE_MONITOR_DATA_DIR` or `~/Library/Application Support/storage-monitor`.
pub fn data_dir() -> PathBuf {
    with_data_dir_override(std::env::var_os(DATA_DIR_ENV).map(PathBuf::from))
}

pub(crate) fn with_data_dir_override(override_dir: Option<impl Into<PathBuf>>) -> PathBuf {
    match override_dir {
        Some(dir) => dir.into(),
        None => dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("storage-monitor"),
    }
}

pub fn snapshots_dir() -> PathBuf {
    snapshots_dir_in(&data_dir())
}

pub(crate) fn snapshots_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("snapshots")
}

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}
```
Add `pub mod disk; pub mod paths;` to `lib.rs`.

**Step 3: Tests, lint, commit**: `cargo test -p storage-monitor-core` all green.

```bash
git add crates/core
git commit -m "feat(core): disk usage via statvfs and application data paths"
```

---

### Task 5: CLI `scan` command

**Files:**
- Modify: `crates/cli/src/main.rs`, `crates/cli/Cargo.toml`
- Create: `crates/cli/src/scan_cmd.rs`, `crates/cli/src/report.rs`, `crates/cli/tests/scan.rs`

Behavior:
- `storage-monitor scan [ROOT] [--json] [--depth N] [--top N] [--save] [--threshold BYTES]`.
- `ROOT` defaults to the home folder. `--depth` (default 2) limits the reported tree, `--top` (default 20) limits children per directory, sorted by size. `--save` persists a snapshot into `paths::snapshots_dir()` and reports `topGrowers` against the previous snapshot; `--threshold` (default 10485760) is the file threshold for the snapshot.
- Progress goes to stderr every 500 ms only when stderr is a terminal.
- JSON output is the `ScanReport` below; text output prints a summary and the top-level children with human-readable sizes.
- Exit code 0 on success, 1 on error (message on stderr), 130 if cancelled (not reachable yet; keep the mapping).

**Step 1: Failing test `crates/cli/tests/scan.rs`**

```rust
use std::fs;
use std::io::Write;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_storage-monitor"))
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, size) in [("a/one.bin", 30_000usize), ("a/two.bin", 10_000), ("b.bin", 5_000)] {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path).unwrap().write_all(&vec![0u8; size]).unwrap();
    }
    dir
}

#[test]
fn scan_json_reports_the_tree_and_stats() {
    let dir = fixture();
    let out = bin().args(["scan", dir.path().to_str().unwrap(), "--json", "--depth", "1"]).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["root"], dir.path().to_str().unwrap());
    assert_eq!(json["stats"]["files"], 3);
    assert_eq!(json["tree"]["fileCount"], 3);
    let children = json["tree"]["children"].as_array().unwrap();
    assert_eq!(children[0]["name"], "a");
    assert_eq!(children[0]["kind"], "dir");
    assert!(children[0]["children"].as_array().unwrap().is_empty(), "depth 1 stops here");
    assert_eq!(children[0]["truncated"], true);
    assert!(json["disk"]["total"].as_u64().unwrap() > 0);
    assert!(json["snapshot"].is_null());
}

#[test]
fn scan_save_writes_a_snapshot_and_reports_growers_on_the_second_run() {
    let dir = fixture();
    let data = tempfile::tempdir().unwrap();
    let run = |extra: &[&str]| {
        bin().env("STORAGE_MONITOR_DATA_DIR", data.path())
            .args(["scan", dir.path().to_str().unwrap(), "--json", "--save", "--threshold", "1"])
            .args(extra)
            .output().unwrap()
    };
    let first = run(&[]);
    assert!(first.status.success());
    let json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert!(json["snapshot"].as_str().unwrap().ends_with(".snap"));
    assert!(json["topGrowers"].as_array().unwrap().is_empty());

    fs::File::create(dir.path().join("a/three.bin")).unwrap().write_all(&vec![0u8; 40_000]).unwrap();
    let second = run(&[]);
    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let growers = json["topGrowers"].as_array().unwrap();
    assert!(!growers.is_empty());
    assert_eq!(growers[0]["path"], dir.path().join("a").to_str().unwrap());
    assert_eq!(fs::read_dir(data.path().join("snapshots")).unwrap().count(), 4, "two .snap + two .json");
}

#[test]
fn scan_text_output_mentions_the_root_and_a_child() {
    let dir = fixture();
    let out = bin().args(["scan", dir.path().to_str().unwrap()]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(dir.path().to_str().unwrap()));
    assert!(text.contains("a/") || text.contains(" a"), "text: {text}");
}

#[test]
fn scan_of_a_missing_root_fails_with_exit_code_1() {
    let out = bin().args(["scan", "/definitely/missing", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error"));
}
```

Add to `crates/cli/Cargo.toml` `[dependencies]`: `serde = { workspace = true }`, `chrono = { workspace = true }`; `[dev-dependencies]`: `tempfile = { workspace = true }`.

**Step 2: Implement**

`crates/cli/src/report.rs`:
```rust
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;
use storage_monitor_core::disk::DiskUsage;
use storage_monitor_core::scan::{NodeId, NodeKind, ScanResult, ScanStats, Tree};
use storage_monitor_core::snapshot::Delta;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub root: PathBuf,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub stats: ScanStats,
    pub cancelled: bool,
    pub disk: Option<DiskUsage>,
    pub tree: ReportNode,
    pub snapshot: Option<PathBuf>,
    pub top_growers: Vec<Delta>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportNode {
    pub name: String,
    pub kind: NodeKind,
    pub size: u64,
    pub logical_size: u64,
    pub file_count: u64,
    pub mtime: i64,
    pub error: Option<String>,
    pub children: Vec<ReportNode>,
    /// True when children were cut by `--top` or `--depth`.
    pub truncated: bool,
}

pub fn report_tree(result: &ScanResult, depth: usize, top: usize) -> ReportNode {
    build(&result.tree, Tree::ROOT, depth, top)
}

fn build(tree: &Tree, id: NodeId, depth: usize, top: usize) -> ReportNode {
    let node = tree.get(id).expect("node exists");
    let ids = tree.children(id);
    let (children, truncated) = if depth == 0 {
        (Vec::new(), !ids.is_empty())
    } else {
        let taken: Vec<ReportNode> = ids.iter().take(top).map(|c| build(tree, *c, depth - 1, top)).collect();
        (taken, ids.len() > top)
    };
    ReportNode {
        name: node.name.clone(),
        kind: node.kind,
        size: node.size,
        logical_size: node.logical_size,
        file_count: node.file_count,
        mtime: node.mtime,
        error: node.error.clone(),
        children,
        truncated,
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}
```

`crates/cli/src/scan_cmd.rs`:
```rust
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use storage_monitor_core::disk::disk_usage;
use storage_monitor_core::scan::{ScanOptions, ScanProgress, scan};
use storage_monitor_core::snapshot::{Snapshot, SnapshotStore, deltas, top_growers};
use storage_monitor_core::{paths, scan::Tree};

use crate::report::{ScanReport, human_bytes, report_tree};

pub struct ScanArgs {
    pub root: Option<PathBuf>,
    pub json: bool,
    pub depth: usize,
    pub top: usize,
    pub save: bool,
    pub threshold: u64,
}

pub fn run(args: ScanArgs) -> Result<(), String> {
    let root = match args.root {
        Some(root) => root,
        None => paths::home_dir().ok_or("cannot determine the home folder")?,
    };
    let progress = Arc::new(ScanProgress::default());
    let ticker = spawn_progress_printer(Arc::clone(&progress));
    let result = scan(&ScanOptions::new(root.clone()), &progress).map_err(|e| e.to_string());
    ticker.stop();
    let result = result?;

    let (snapshot_path, growers) = if args.save {
        let store = SnapshotStore::new(paths::snapshots_dir());
        let previous = store.latest().map_err(|e| e.to_string())?;
        let current = Snapshot::from_result(&result, args.threshold);
        let path = store.save(&current).map_err(|e| e.to_string())?;
        store.prune(10).map_err(|e| e.to_string())?;
        let growers = previous.map(|p| top_growers(&deltas(&p, &current), 20)).unwrap_or_default();
        (Some(path), growers)
    } else {
        (None, Vec::new())
    };

    let report = ScanReport {
        root: result.root.clone(),
        started_at: result.started_at,
        duration_ms: result.duration_ms,
        stats: result.stats.clone(),
        cancelled: result.cancelled,
        disk: disk_usage(&result.root).ok(),
        tree: report_tree(&result, args.depth, args.top),
        snapshot: snapshot_path,
        top_growers: growers,
    };

    let mut out = io::stdout().lock();
    let written = if args.json {
        serde_json::to_writer_pretty(&mut out, &report).map_err(io::Error::from).and_then(|_| writeln!(out))
    } else {
        write_text(&mut out, &report, &result.tree)
    };
    match written {
        Ok(()) => out.flush().or_else(quiet_on_broken_pipe),
        Err(err) => quiet_on_broken_pipe(err),
    }
    .map_err(|e| e.to_string())
}

fn quiet_on_broken_pipe(err: io::Error) -> io::Result<()> {
    if err.kind() == io::ErrorKind::BrokenPipe { Ok(()) } else { Err(err) }
}

fn write_text(out: &mut impl Write, report: &ScanReport, tree: &Tree) -> io::Result<()> {
    writeln!(out, "{}  {}  ({} files, {} dirs, {} errors, {:.1}s)", report.root.display(), human_bytes(report.tree.size),
        report.stats.files, report.stats.dirs, report.stats.errors, report.duration_ms as f64 / 1000.0)?;
    if let Some(disk) = &report.disk {
        writeln!(out, "volume: {} used of {} ({} available)", human_bytes(disk.used), human_bytes(disk.total), human_bytes(disk.available))?;
    }
    let total = report.tree.size.max(1) as f64;
    for child in &report.tree.children {
        let suffix = if child.kind == storage_monitor_core::scan::NodeKind::Dir { "/" } else { "" };
        writeln!(out, "{:>10}  {:5.1}%  {}{}", human_bytes(child.size), child.size as f64 * 100.0 / total, child.name, suffix)?;
    }
    if !report.top_growers.is_empty() {
        writeln!(out, "\ngrew since the previous snapshot:")?;
        for g in &report.top_growers {
            writeln!(out, "{:>10}  {}", format!("+{}", human_bytes(g.delta.max(0) as u64)), g.path)?;
        }
    }
    let _ = tree;
    Ok(())
}

struct Ticker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Ticker {
    fn stop(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_progress_printer(progress: Arc<ScanProgress>) -> Ticker {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if !io::stderr().is_terminal() {
        return Ticker { stop, handle: None };
    }
    let flag = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut printed = false;
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(500));
            let p = progress.snapshot();
            eprint!("\r\x1b[2Kscanning: {} files, {}  {}", p.files, human_bytes(p.bytes), shorten(&p.current_path, 60));
            printed = true;
        }
        if printed {
            eprint!("\r\x1b[2K");
        }
    });
    Ticker { stop, handle: Some(handle) }
}

fn shorten(path: &str, max: usize) -> String {
    if path.chars().count() <= max { path.to_owned() } else { format!("…{}", path.chars().skip(path.chars().count() - max + 1).collect::<String>()) }
}
```

`crates/cli/src/main.rs`: add `mod report; mod scan_cmd;`, extend `Command`:
```rust
    /// Scan a folder (default: the home folder) and report where the space goes
    Scan {
        /// Folder to scan
        root: Option<PathBuf>,
        /// Print as JSON
        #[arg(long)]
        json: bool,
        /// Depth of the reported tree
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Children per directory in the report, largest first
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Persist a snapshot and report growth against the previous one
        #[arg(long)]
        save: bool,
        /// Smallest file kept in the snapshot, in bytes
        #[arg(long, default_value_t = 10 * 1024 * 1024)]
        threshold: u64,
    },
```
and in `main`, map `Command::Scan { .. } => scan_cmd::run(ScanArgs { .. })`. Make `main` handle both `io::Error` (from `info`) and `String` errors: simplest is to convert `print_info`'s result with `.map_err(|e| e.to_string())` after the BrokenPipe check, so `main` works with `Result<(), String>` and prints `error: {msg}` + exit code 1.

**Step 3: Run**: `cargo test -p storage-monitor-cli` (expected: 2 old + 4 new pass), fmt, clippy.

```bash
git add crates/cli Cargo.lock
git commit -m "feat(cli): scan command with JSON report, snapshots and growers"
```

---

### Task 6: Desktop: scan manager, commands and progress events

**Files:**
- Create: `apps/desktop/src-tauri/src/scan_manager.rs`, `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/views.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/capabilities/default.json`

Add to the desktop `Cargo.toml` `[dependencies]`: `tauri-plugin-opener = "2"`, `serde = { workspace = true }`, `serde_json = { workspace = true }`, `chrono = { workspace = true }`. Capability `permissions`: `["core:default", "opener:default"]` (`opener:default` includes `allow-reveal-item-in-dir`).

**Commands and payloads (all camelCase over IPC):**

| Command | Args | Returns |
|---|---|---|
| `default_root` | | `string` (home folder) |
| `scan_start` | `root?: string` | `ScanStatus` |
| `scan_status` | | `ScanStatus` |
| `scan_cancel` | | `ScanStatus` |
| `tree_node` | `id?: number` (default root), `limit?: number` (default 500) | `NodeView` |
| `disk_usage` | `path?: string` (default scan root or home) | `DiskUsage` |
| `top_growers` | `limit?: number` (default 10) | `Delta[]` |

Events: `scan:progress` (`ScanStatus`, every 250 ms while running) and `scan:done` (`ScanStatus`, once).

```rust
// views.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanState { Idle, Running, Done, Cancelled, Failed }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub state: ScanState,
    pub root: Option<String>,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub current_path: String,
    pub duration_ms: u64,
    pub error: Option<String>,
    /// A previous snapshot exists, so deltas are available.
    pub has_previous: bool,
    pub previous_taken_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb { pub id: u32, pub name: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildView {
    pub id: u32, pub name: String, pub kind: NodeKind, pub size: u64, pub logical_size: u64,
    pub file_count: u64, pub mtime: i64, pub error: Option<String>, pub delta: Option<i64>, pub has_children: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub id: u32, pub name: String, pub path: String, pub kind: NodeKind, pub size: u64, pub logical_size: u64,
    pub file_count: u64, pub mtime: i64, pub error: Option<String>, pub delta: Option<i64>,
    pub breadcrumbs: Vec<Crumb>, pub children: Vec<ChildView>, pub children_total: u32, pub truncated: bool,
}
```

`NodeView::build(tree, id, limit, previous: Option<&HashMap<String,u64>>)`: breadcrumbs from `tree.ancestors(id)` (name of the root crumb = last path component of the root, or the full path if it has none); children from `tree.children(id)` (already sorted), `delta = previous.get(path).map(|b| size as i64 - b as i64)`, `has_children = !node.children.is_empty()`; `truncated = children_total > limit`. Unit-test `build` on the small tree from Task 1 (breadcrumb names, truncation, delta present only for known paths).

**`scan_manager.rs`:**
```rust
#[derive(Clone, Default)]
pub struct ScanManager(Arc<Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    state: ScanState (default Idle),
    root: Option<PathBuf>,
    progress: Arc<ScanProgress>,
    started: Option<Instant>,
    duration_ms: u64,
    result: Option<Arc<ScanResult>>,
    previous_sizes: Option<Arc<HashMap<String, u64>>>,
    previous_taken_at: Option<DateTime<Utc>>,
    growers: Vec<Delta>,
    error: Option<String>,
}

impl ScanManager {
    pub fn status(&self) -> ScanStatus;         // merges progress snapshot and state
    pub fn start(&self, app: AppHandle, root: PathBuf) -> Result<ScanStatus, String>;
    pub fn cancel(&self) -> ScanStatus;         // sets progress.cancel(); state becomes Cancelled when the thread finishes
    pub fn with_result<T>(&self, f: impl FnOnce(&ScanResult, Option<&HashMap<String,u64>>) -> T) -> Option<T>;
    pub fn growers(&self, limit: usize) -> Vec<Delta>;
}
```
`start`: if `Running`, return `Err("a scan is already running")`. Otherwise reset `Inner` (fresh `ScanProgress`, state `Running`, root, started), then spawn a `std::thread` that: runs `scan(&ScanOptions::new(root), &progress)`; on `Ok(result)`: `store = SnapshotStore::new(paths::snapshots_dir())`, `previous = store.latest()` (before saving), `current = Snapshot::from_result(&result, 10 MiB)`, `store.save(&current)`, `store.prune(10)`, `growers = previous.as_ref().map(|p| top_growers(&deltas(p, &current), 50))`, `previous_sizes = previous.map(|p| p.size_index())`, then set `state = if result.cancelled { Cancelled } else { Done }`, `result`, `duration_ms`; on `Err(e)`: `state = Failed`, `error = Some(e.to_string())`. Store errors do not fail the scan: log to stderr, keep `Done`. Finally `app.emit("scan:done", &status)`. A second thread emits `scan:progress` every 250 ms while `state == Running` (`use tauri::Emitter;`). Wrap the whole worker body so a panic cannot leave `state == Running` (use `std::panic::catch_unwind` around the scan and map a panic to `Failed`).

**`commands.rs`:** thin `#[tauri::command]` functions over the manager (`tauri::State<'_, ScanManager>`, `tauri::AppHandle`), mapping errors to `String`. `tree_node` returns `Err("no scan result")` when idle/running. `disk_usage` defaults to the scan root, else the home folder. `default_root` returns `paths::home_dir()` as a string or an error.

**`lib.rs`:** `.plugin(tauri_plugin_opener::init())`, `.manage(ScanManager::default())`, register the seven commands (keep `get_app_info`).

**Tests:** `views.rs` unit tests for `NodeView::build`; `scan_manager.rs` unit test that runs `start` against a temp directory with `STORAGE_MONITOR_DATA_DIR` pointing at a temp dir, polls `status()` until `Done`, and asserts `with_result` sees the tree and a `.snap` file exists. `start` needs an `AppHandle` for events; make the manager generic over an `Emit` trait (`fn emit(&self, event: &str, status: &ScanStatus)`) implemented for `AppHandle` and for a test recorder, so the test does not need a Tauri app. Run `cargo test -p storage-monitor-desktop`, clippy, and `pnpm --filter @storage-monitor/desktop tauri build --debug --no-bundle` to prove the desktop crate still builds.

```bash
git add apps/desktop/src-tauri Cargo.lock
git commit -m "feat(desktop): scan manager with progress events and tree view commands"
```

---

### Task 7: Frontend: IPC layer, mocks and fixtures

**Files:**
- Modify: `apps/desktop/src/lib/ipc.ts`, `apps/desktop/src/mocks/ipc.ts`
- Create: `apps/desktop/src/lib/format.ts`, `apps/desktop/src/lib/format.test.ts`, `apps/desktop/src/mocks/fixtures.ts`, `apps/desktop/src/mocks/ipc.test.ts`

**Step 1: Install**: `pnpm --filter @storage-monitor/desktop add @tanstack/react-query @tauri-apps/plugin-opener lucide-react echarts`.

**Step 2: `src/lib/ipc.ts`** mirrors Task 6 exactly (interfaces `ScanState`, `ScanStatus`, `Crumb`, `ChildView`, `NodeView`, `DiskUsage`, `Delta`, `NodeKind = 'dir' | 'file' | 'symlink' | 'other'`), functions `defaultRoot()`, `scanStart(root?)`, `scanStatus()`, `scanCancel()`, `treeNode(id?, limit?)`, `diskUsage(path?)`, `topGrowers(limit?)`, plus `onScanProgress(cb): Promise<UnlistenFn>` and `onScanDone(cb)` using `listen` from `@tauri-apps/api/event`, and `revealInFinder(path)` calling `revealItemInDir` from `@tauri-apps/plugin-opener`. `invoke` stays confined to this file (the opener import is allowed here too).

**Step 3: `src/lib/format.ts`** (TDD, tests first in `format.test.ts`): `formatBytes(n)` (1000-based: `0 B`, `999 B`, `1.0 KB`, `4.5 GB`, `1.2 TB`), `formatDelta(n | null)` (`+1.2 GB`, `−340 MB` with a real minus sign, `''` for null/0), `formatPercent(part, whole)` (`12.3%`, `0%` when whole is 0), `formatDate(unixSeconds)` (`2026-09-18`), `shortenPath(path, max)`.

**Step 4: `src/mocks/fixtures.ts`**: a deterministic home-like tree of about 60 nodes rooted at `/Users/demo` (Library/Developer/Xcode/DerivedData with 3 projects, Library/Caches, Documents, Downloads with two big files, src with two repos containing `node_modules` and `target`, Movies, `.Trash`), sizes chosen so the treemap and table look realistic (root about 180 GB), a few permission-denied directories (`error: 'Permission denied (os error 13)'`), and a `previousSizes` map giving deltas for about ten paths (some negative). Export `fixtureNodeView(id, limit)` that builds `NodeView`s (breadcrumbs, sorted children, deltas) from the fixture, `fixtureGrowers()`, `fixtureDisk()` and `FIXTURE_ROOT`.

**Step 5: `src/mocks/ipc.ts`**: `installIpcMock()` now calls `mockIPC(handler, { shouldMockEvents: true })` and handles every command from Task 6 plus `plugin:opener|reveal_item_in_dir` (records the path in an exported `revealed: string[]` array). `scan_start` moves an internal status through `running` with 6 progress ticks 150 ms apart (files/bytes/currentPath increasing) emitted via `emit('scan:progress', status)` from `@tauri-apps/api/event`, then `done` with `hasPrevious: true` and `emit('scan:done', status)`. `scan_cancel` during a run stops the ticks and emits `done` with state `cancelled`. Export `mockScanDelayMs` so tests can set it to 0. `resetIpcMock()` restores idle state and clears `revealed`.

**Step 6: `src/mocks/ipc.test.ts`**: with the mock installed, `scanStart()` resolves with `running`; after the ticks `scanStatus()` is `done`; `treeNode()` returns the fixture root with sorted children and breadcrumbs; `treeNode(childId)` has two breadcrumbs; `revealInFinder('/x')` records the path; an unknown command rejects.

Run `pnpm --filter @storage-monitor/desktop test` (expected: 2 old + new tests pass), `typecheck`, `lint`.

```bash
git add apps/desktop/src apps/desktop/package.json pnpm-lock.yaml
git commit -m "feat(desktop): typed IPC for scanning and a simulated scan in mock mode"
```

---

### Task 8: Frontend: app shell and the Explorer page (without treemap)

**Files:**
- Create: `apps/desktop/src/components/AppShell.tsx`, `Sidebar.tsx`, `TitleBar.tsx`, `DiskUsageBar.tsx`, `ScanProgress.tsx`, `Breadcrumbs.tsx`, `NodeTable.tsx`, `EmptyState.tsx`, `apps/desktop/src/pages/ExplorerPage.tsx`, `apps/desktop/src/pages/PlaceholderPage.tsx`, `apps/desktop/src/hooks/useScan.ts`, `apps/desktop/src/hooks/useScan.test.tsx`, `apps/desktop/src/pages/ExplorerPage.test.tsx`
- Modify: `apps/desktop/src/App.tsx`, `apps/desktop/src/main.tsx` (QueryClientProvider), `apps/desktop/src/index.css`, `apps/desktop/src/App.test.tsx`

**Design brief (macOS-like, restrained):** system font, 13 px base, neutral palette with `dark:` variants driven by `color-scheme`. Left sidebar 220 px with the five entries (Overview, Explorer, Cleanup, Activity, Settings; only Explorer enabled, the rest `opacity-50` with a "soon" hint), lucide icons. The top 38 px of the window is a drag region (`data-tauri-drag-region`) with 80 px left padding for the traffic lights (Overlay title bar). Content area scrolls independently. Numbers right-aligned in tabular figures (`tabular-nums`). Row hover, focus ring, keyboard: Enter opens a directory, Backspace goes up.

**`useScan` hook:** holds `ScanStatus` (initial from `scanStatus()`), subscribes to `scan:progress`/`scan:done`, exposes `start(root?)`, `cancel()`, `status`, and `generation` (increments on every `done`, used in query keys so tree queries refetch after a rescan).

**Explorer page states:**
1. Idle, no result: `EmptyState` with the root path (from `defaultRoot()`), a "Scan" primary button, and the note "Scans the home folder. Some folders in Library need Full Disk Access; they are reported, not skipped."
2. Running: `ScanProgress` card: indeterminate bar, `files` and `bytes` counters (`formatBytes`), current path (`shortenPath`, monospace, one line), Cancel button.
3. Done/Cancelled: header row with root name, total size, file count, scan duration, "Rescan" button, and `DiskUsageBar` (used/available of the volume, from `diskUsage()`); a note when the scan was cancelled ("partial results"). Then `Breadcrumbs` (clickable), the treemap slot (Task 9) and `NodeTable`.
4. Failed: error message and a Retry button.

**`NodeTable` columns:** Name (icon: folder / file / link; lock icon with the error text as `title` when `error`), Size (formatBytes + inline bar proportional to the largest child), `%` of parent, `Δ` (formatDelta, green/red tint, blank when null), Files, Modified (formatDate), actions (a "Reveal in Finder" icon button). Sortable by Name, Size, Δ, Files, Modified (click header toggles; default Size desc). Directory rows navigate on click; files do not. Shows `children_total` and "showing first 500" when `truncated`.

**Tests (Vitest + Testing Library, mocked IPC, `mockScanDelayMs = 0`):**
- `useScan.test.tsx`: starts idle, `start()` → running → done via events; `generation` increments; `cancel()` yields `cancelled`.
- `ExplorerPage.test.tsx`: renders the empty state with the default root; clicking Scan shows progress then the table with the fixture's top-level rows sorted by size; clicking a directory row navigates (breadcrumbs grow, rows change); clicking the root crumb goes back; Backspace goes up; the Δ column shows `+`/`−` values for paths in `previousSizes` and is blank otherwise; a permission-denied row shows the lock icon with the error title; clicking "Reveal in Finder" records the path in the mock; sorting by Name reorders rows.
- `App.test.tsx`: keep the existing tests (the version now lives in the sidebar footer, e.g. `Storage Monitor v0.0.0-mock`), add: sidebar shows five entries and Explorer is selected by default.

Run `pnpm --filter @storage-monitor/desktop test`, `typecheck`, `lint`, and look at the page in a browser with `just dev-web` (or the Playwright screenshot in Task 10).

```bash
git add apps/desktop/src
git commit -m "feat(desktop): app shell and Explorer page with scan flow and node table"
```

---

### Task 9: Treemap and "Reveal in Finder" wiring

**Files:**
- Create: `apps/desktop/src/components/Treemap.tsx`, `apps/desktop/src/components/Treemap.test.tsx`
- Modify: `apps/desktop/src/pages/ExplorerPage.tsx`

**Treemap:** ECharts via `echarts/core` with `TreemapChart`, `TooltipComponent`, `CanvasRenderer` registered once. Props: `children: ChildView[]`, `parentSize`, `onSelect(id)`. Data: the top 60 children by size plus one aggregated "Other (n items)" cell (not clickable) when more exist; directories in a blue-grey ramp, files in a warm grey, permission-denied cells hatched via a lighter color and a lock in the label. Options: `roam: false`, `nodeClick: false`, `breadcrumb.show: false`, `label` with name and `formatBytes(value)`, `tooltip` with name, size, `%` of parent and delta. Click on a directory cell calls `onSelect(id)`. Resize with a `ResizeObserver`; dispose on unmount. Height 320 px; renders nothing (a friendly "Nothing to show" label) for an empty directory.

**Tests:** in Vitest, mock `echarts/core` (`init` returning a fake chart with `setOption`, `on`, `resize`, `dispose`), `echarts/charts`, `echarts/components`, `echarts/renderers`; assert `setOption` receives one treemap series with the expected number of data items (60 + "Other") and that a click callback for a directory calls `onSelect` with its id. In the real browser, Playwright (Task 10) checks the canvas exists.

Wire the treemap into the Explorer page above the table; clicking a treemap cell navigates like a row click.

```bash
git add apps/desktop/src
git commit -m "feat(desktop): treemap of the current directory"
```

---

### Task 10: Playwright e2e for the Explorer flow

**Files:**
- Create: `apps/desktop/e2e/explorer.spec.ts`
- Modify: `apps/desktop/e2e/smoke.spec.ts` (the version now lives in the sidebar footer; keep the assertions valid)

Tests (mock mode, `mockScanDelayMs` at its default so progress is visible):
1. Empty state shows the root and the Scan button; after clicking, the progress panel appears, then the table with the fixture's largest child first and a treemap `<canvas>` inside the treemap container.
2. Drill down through a directory row, verify breadcrumbs and rows, go back through the root crumb.
3. Δ column shows a positive value for a known grown path and a negative one for a known shrunk path.
4. A permission-denied row shows the lock indicator.
5. Screenshot `explorer.png` of the loaded page via `test.info().outputPath('explorer.png')` (full page), plus `explorer-dark.png` after `page.emulateMedia({ colorScheme: 'dark' })`.

Run `pnpm --filter @storage-monitor/desktop e2e`; view both screenshots with the Read tool and make sure the layout is not broken (no overlapping title bar, aligned numbers, readable treemap labels). Fix styling issues before committing.

```bash
git add apps/desktop/e2e
git commit -m "test(desktop): Explorer e2e with drill-down, deltas and screenshots"
```

---

### Task 11: Docs, PR, release `v0.2.0`

1. `CLAUDE.md`: layout gains `crates/core/src/{scan,snapshot}`, `apps/desktop/src/{components,pages,hooks}`; commands gain `storage-monitor scan --json`; a "Data" line: snapshots live in `~/Library/Application Support/storage-monitor/snapshots` (override `STORAGE_MONITOR_DATA_DIR`), last 10 kept, files ≥ 10 MiB. Conventions: "tree queries are keyed by scan generation".
2. `README.md`: status paragraph (phase 1 done: scan, Explorer, deltas), a "What it does today" list, the CLI example, and the screenshot `docs/images/explorer.png` copied from the Playwright output (light theme).
3. `docs/adr/0004-snapshot-format.md`: postcard + lz4, sidecar meta, threshold, retention; why not SQLite yet.
4. Design doc section 14: mark phase 1 done with the date.
5. `just ci` green; push; `gh pr create --title "feat: scanner, snapshots and Explorer (phase 1)"` with the test plan and both screenshots attached (upload via the PR description is not possible from the API; reference the CI artifact and the committed `docs/images/explorer.png`); wait for green; squash-merge.
6. release-please opens `chore(main): release 0.2.0` (the `feat` commit bumps minor). Close/reopen it for CI, verify the diff (8 files), merge, watch `Release`, then download and check the assets like in phase 0; run the released app once on this Mac and scan the home folder for real; note the scan time and any errors in the plan's Outcome section.

Exit criteria: scanning the home folder from the app shows a treemap and table with deltas on the second run; `storage-monitor scan --json --save` works from a terminal; `v0.2.0` assets are on GitHub.
