//! Parallel directory walker.
//!
//! [`scan`] walks a root on a dedicated rayon pool, one task per directory, and builds a
//! nested [`Subtree`] that is then flattened into a [`Tree`]. It follows the root when the
//! root itself is a symlink (`/tmp` on macOS) but never a symlink below it, stays on the
//! root's volume by default, records unreadable directories as errors instead of failing
//! and attributes hard-linked data to one path. [`rescan_path`] rebuilds one branch of such
//! a tree after a deletion, under the options of the scan it patches.

use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::progress::ScanProgress;
use super::tree::{Node, NodeKind, Subtree, Tree};

/// The walk recurses once per directory level, and `PATH_MAX` allows about 2000 levels on
/// Linux (500 on macOS). A level costs up to 17 KiB of stack in debug builds (rayon's
/// frames included), far beyond the default 2 MiB; the reservation is virtual, pages are
/// committed only when a deep tree touches them.
const WORKER_STACK_SIZE: usize = 64 << 20;

/// Recorded on a directory the walk refuses to enter because it sits on another volume.
/// [`rescan_path`] produces the same leaf for the same path, so this stays one string.
const DIFFERENT_VOLUME: &str = "skipped: different volume";

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
        Self {
            root,
            excludes: Vec::new(),
            same_device: true,
        }
    }

    /// The same scan, walked from somewhere else: every rule of this scan, a different
    /// root. [`rescan_path`] patches one branch of a scan through it.
    pub fn rooted_at(&self, root: PathBuf) -> Self {
        Self {
            root,
            ..self.clone()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("{} is not a directory", .0.display())]
    NotADirectory(PathBuf),
    #[error("cannot read {}: {source}", path.display())]
    Root {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot start the scan workers: {0}")]
    Workers(#[from] rayon::ThreadPoolBuildError),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    /// Entries that are not directories (files, symlinks, devices).
    pub files: u64,
    pub dirs: u64,
    /// Allocated bytes of the whole tree; equals the root node's `size`.
    pub bytes: u64,
    /// Directories and entries that could not be read.
    pub errors: u64,
    /// Further links to data already counted under another path.
    pub hardlinks_skipped: u64,
}

#[derive(Debug, Clone)]
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
}

impl Ctx<'_> {
    fn is_excluded(&self, path: &Path) -> bool {
        self.options.excludes.iter().any(|e| e == path)
    }
}

/// Scans `options.root` in parallel. Follows the root itself when it is a symlink, never
/// a symlink below it; stays on the root's volume, counts hard-linked data once (under
/// the smallest path) and records unreadable directories instead of failing.
pub fn scan(options: &ScanOptions, progress: &ScanProgress) -> Result<ScanResult, ScanError> {
    let started = Instant::now();
    let started_at = Utc::now();
    // Drop trailing slashes and `.` components (no canonicalization), so the paths derived
    // from the root agree with the paths derived from its entries.
    let root: PathBuf = options.root.components().collect();
    // `metadata` follows a symlinked root; the entries below use `symlink_metadata`.
    let meta = fs::metadata(&root).map_err(|source| ScanError::Root {
        path: root.clone(),
        source,
    })?;
    if !meta.is_dir() {
        return Err(ScanError::NotADirectory(root));
    }
    let ctx = Ctx {
        options,
        progress,
        root_dev: meta.dev(),
    };
    let root_node = node_from_metadata(&root.to_string_lossy(), &meta);
    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(WORKER_STACK_SIZE)
        .build()?;
    let subtree = pool.install(|| walk_dir(&root, root_node, &ctx));
    // Read before flattening: a cancel that arrives while a complete tree is being
    // flattened must not label it as partial.
    let cancelled = progress.is_cancelled();
    let (mut tree, links) = subtree.flatten();
    let hardlinks_skipped = tree.attribute_hard_links(links);
    let snap = progress.snapshot();
    Ok(ScanResult {
        root,
        started_at,
        duration_ms: started.elapsed().as_millis() as u64,
        stats: ScanStats {
            files: snap.files,
            dirs: snap.dirs,
            bytes: tree.root().size,
            errors: snap.errors,
            hardlinks_skipped,
        },
        cancelled,
        tree,
    })
}

/// A fresh [`Tree`] for one branch of a scan, or `None` when nothing is at `path` any
/// more. Used after a deletion, so the tree can be patched instead of walked again.
///
/// `path` must be absolute: a patch is spliced by path, and a relative one matches nothing.
/// It is normalized exactly like the root of a [`scan`] and the root node carries it as its
/// name, so the patch and the tree it joins name the same path.
///
/// The tree describes what is at `path` *now*: a directory is walked, anything else — a
/// file, a symlink, a socket — becomes a single node, so a path that changed kind since the
/// scan is reported as it is today. Unlike [`scan`], a symlinked `path` is never followed:
/// it is the path that was just deleted, and resolving it would walk the target of a link
/// the user removed.
///
/// Two rules of the scan being patched are kept, and `options.root` is used for nothing
/// else: `excludes` hold back the directories below `path` (`path` itself is walked either
/// way, like the root of a scan), and `same_device` compares against the volume of
/// `options.root` — a `path` on another volume comes back as the single skipped node the
/// walker leaves in the tree, never as a second volume walked into a total that describes
/// one.
///
/// `None` means the path is gone: deleted, or under something that is no longer a
/// directory. Everything else keeps the branch. A directory that exists but cannot be
/// listed comes back as a single node carrying the error, as it would inside a full scan; a
/// path that cannot be read at all — a permission, a symlink loop — is a
/// [`ScanError::Root`], as is an `options.root` the volume rule cannot be resolved against.
/// [`ScanError::NotADirectory`] and [`ScanError::Workers`] escape from the walk as well.
///
/// Hard links are re-attributed inside `path` alone. A link there whose bytes the last full
/// scan gave to a twin outside takes them back, so splicing the patch can make the totals
/// above it *grow* although nothing was added; the next full scan puts it right. Holding
/// the whole tree's `(dev, ino)` set is what the design rejected.
///
/// The walk is synchronous, cannot be cancelled and reports no progress, and a directory
/// builds its own rayon pool: one call per deleted path, not a loop over thousands. The
/// [`ScanStats`] are dropped — a patch has nothing to say about the counts of the scan.
pub fn rescan_path(path: &Path, options: &ScanOptions) -> Result<Option<Tree>, ScanError> {
    // Normalize before the stat, not after: `lstat` follows a symlink whose path ends in a
    // separator, so `deleted-link/` would otherwise report — and then walk — its target.
    let path: PathBuf = path.components().collect();
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(source) if is_gone(&source) => return Ok(None),
        Err(source) => return Err(ScanError::Root { path, source }),
    };
    if !meta.is_dir() {
        return Ok(Some(single_node(&path, &meta, None)));
    }
    // The walk stops at a mount point below its root; re-rooting `scan` here would make the
    // patch measure the volume against itself and walk the one the scan never entered.
    if options.same_device && meta.dev() != root_device(options)? {
        return Ok(Some(single_node(&path, &meta, Some(DIFFERENT_VOLUME))));
    }
    walk_from(path, options)
}

/// The whole tree of a path that is not walked into: one node named with the path, plus the
/// reason it was not entered when there is one.
fn single_node(path: &Path, meta: &Metadata, error: Option<&str>) -> Tree {
    let node = node_from_metadata(&path.to_string_lossy(), meta);
    let subtree = match error {
        Some(reason) => unread(node, reason),
        None => Subtree::new(node),
    };
    subtree.flatten().0
}

/// Walks a directory under the options of the scan being patched. `scan` stats the root
/// once more, so a directory deleted between that stat and the one above is gone, not an
/// error; it would also follow the root if it had become a symlink by then, a window that
/// can only be closed by walking from a file descriptor, which the scanner does not do.
fn walk_from(root: PathBuf, options: &ScanOptions) -> Result<Option<Tree>, ScanError> {
    match scan(&options.rooted_at(root), &ScanProgress::default()) {
        Ok(result) => Ok(Some(result.tree)),
        Err(ScanError::Root { source, .. }) if is_gone(&source) => Ok(None),
        Err(other) => Err(other),
    }
}

/// The volume the scan being patched measures, so its rule compares against the same device
/// the walk did. Follows a symlinked root, exactly as [`scan`] does.
fn root_device(options: &ScanOptions) -> Result<u64, ScanError> {
    let root: PathBuf = options.root.components().collect();
    match fs::metadata(&root) {
        Ok(meta) => Ok(meta.dev()),
        Err(source) => Err(ScanError::Root { path: root, source }),
    }
}

/// Whether an error means nothing is at the path any more: it was deleted, or one of its
/// parents is no longer a directory. Every other failure — a permission, a symlink loop, a
/// name too long — says nothing about whether the path is there, so the branch stays.
fn is_gone(source: &std::io::Error) -> bool {
    matches!(
        source.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

fn walk_dir(path: &Path, mut node: Node, ctx: &Ctx) -> Subtree {
    ctx.progress.enter(path);
    if ctx.progress.is_cancelled() {
        ctx.progress.add_dir(node.size);
        return unread(node, "scan cancelled");
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) => {
            ctx.progress.add_error();
            ctx.progress.add_dir(node.size);
            return unread(node, &err.to_string());
        }
    };

    let mut dirs: Vec<(PathBuf, Node)> = Vec::new();
    let mut leaves: Vec<Subtree> = Vec::new();
    let mut unreadable = 0u64;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        let child_path = entry.path();
        let meta = match fs::symlink_metadata(&child_path) {
            Ok(meta) => meta,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        let child = node_from_metadata(&entry.file_name().to_string_lossy(), &meta);
        if meta.is_dir() {
            if ctx.is_excluded(&child_path) {
                continue;
            }
            if ctx.options.same_device && meta.dev() != ctx.root_dev {
                ctx.progress.add_dir(child.size);
                leaves.push(unread(child, DIFFERENT_VOLUME));
                continue;
            }
            dirs.push((child_path, child));
        } else {
            ctx.progress.add_file(child.size);
            let mut leaf = Subtree::new(child);
            if meta.nlink() > 1 {
                leaf.hardlink = Some((meta.dev(), meta.ino()));
            }
            leaves.push(leaf);
        }
    }

    let subdirs: Vec<Subtree> = dirs
        .into_par_iter()
        .map(|(p, n)| walk_dir(&p, n, ctx))
        .collect();

    let mut children = leaves;
    children.extend(subdirs);
    // Drop the growth slack: the nested tree is the peak of the scan's memory.
    children.shrink_to_fit();
    ctx.progress.add_dir(node.size);
    for child in &children {
        node.size += child.node.size;
        node.logical_size += child.node.logical_size;
        node.file_count += child.node.file_count;
    }
    let mut subtree = Subtree::with_children(node, children);
    if unreadable > 0 {
        for _ in 0..unreadable {
            ctx.progress.add_error();
        }
        let noun = if unreadable == 1 { "entry" } else { "entries" };
        subtree.error = Some(format!("{unreadable} {noun} could not be read").into());
    }
    subtree
}

/// A directory whose entries could not be listed.
fn unread(node: Node, error: &str) -> Subtree {
    let mut subtree = Subtree::new(node);
    subtree.error = Some(error.into());
    subtree
}

fn node_from_metadata(name: &str, meta: &Metadata) -> Node {
    let kind = NodeKind::from_metadata(meta);
    let is_dir = kind == NodeKind::Dir;
    Node::new(
        name,
        kind,
        meta.blocks() * 512,
        if is_dir { 0 } else { meta.len() },
        u32::from(!is_dir),
        meta.mtime(),
    )
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use super::*;

    /// Pins the plumbing only: that the flags reach the walk. Whether the same-volume rule
    /// then *holds* is a different claim, and the one that matters — a re-rooted scan
    /// measures the volume against the new root — so it is tested against a real mount
    /// point in `tests/walker.rs`.
    #[test]
    fn a_rescan_keeps_every_option_of_the_scan_but_the_root() {
        for same_device in [true, false] {
            let options = ScanOptions {
                root: PathBuf::from("/home/me"),
                excludes: vec![PathBuf::from("/home/me/Library")],
                same_device,
            };
            let patched = options.rooted_at(PathBuf::from("/home/me/cache"));
            assert_eq!(patched.root, PathBuf::from("/home/me/cache"));
            assert_eq!(patched.excludes, options.excludes);
            assert_eq!(
                patched.same_device, same_device,
                "the volume rule of the scan is kept"
            );
        }
    }

    /// The directory can be deleted between the stat in [`rescan_path`] and the one inside
    /// [`scan`]; a scan of a root that is not there reaches the same arm without the race.
    #[test]
    fn a_directory_that_is_gone_by_the_time_the_walk_starts_is_not_an_error() {
        let options = ScanOptions::new(PathBuf::from("/home/me"));
        let gone = PathBuf::from("/definitely/missing");
        assert_eq!(fs::metadata(&gone).unwrap_err().kind(), ErrorKind::NotFound);
        assert!(walk_from(gone, &options).unwrap().is_none());
    }

    #[test]
    fn a_directory_that_cannot_be_read_keeps_its_error() {
        let options = ScanOptions::new(PathBuf::from("/home/me"));
        // A root that exists and is not a directory: `scan` reports it, the patch passes it on.
        match walk_from(PathBuf::from("/etc/hosts"), &options) {
            Err(ScanError::NotADirectory(path)) => assert_eq!(path, Path::new("/etc/hosts")),
            other => panic!("expected NotADirectory, got {other:?}"),
        }
    }

    #[test]
    fn only_a_missing_path_and_a_missing_parent_count_as_gone() {
        for kind in [ErrorKind::NotFound, ErrorKind::NotADirectory] {
            assert!(is_gone(&std::io::Error::from(kind)), "{kind:?}");
        }
        // `ELOOP` and `ENAMETOOLONG` have no nameable `ErrorKind` on stable yet; they fall
        // through the same `matches!` and are covered against a real loop in the
        // integration tests.
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::TimedOut,
            ErrorKind::Other,
        ] {
            assert!(!is_gone(&std::io::Error::from(kind)), "{kind:?}");
        }
    }

    #[test]
    fn the_volume_of_a_scan_root_that_cannot_be_read_is_an_error() {
        // The root is normalized before it is reported, exactly as `scan` reports it.
        let options = ScanOptions::new(PathBuf::from("/definitely/missing/"));
        match root_device(&options) {
            Err(ScanError::Root { path, source }) => {
                assert_eq!(path.to_str().unwrap(), "/definitely/missing");
                assert_eq!(source.kind(), ErrorKind::NotFound);
            }
            other => panic!("expected a Root error, got {other:?}"),
        }
    }

    #[test]
    fn the_volume_of_a_symlinked_scan_root_is_the_one_it_points_at() {
        // `scan` follows a symlinked root, so the rule must compare against what it walked.
        let dir = tempfile::tempdir().unwrap();
        let here = fs::metadata(dir.path()).unwrap().dev();
        let there = fs::metadata("/dev").unwrap().dev();
        if here == there {
            eprintln!("skipped: /dev is on the volume of the temp dir");
            return;
        }
        let link = dir.path().join("root");
        std::os::unix::fs::symlink("/dev", &link).unwrap();
        let options = ScanOptions::new(link);
        assert_eq!(root_device(&options).unwrap(), there);
    }
}
