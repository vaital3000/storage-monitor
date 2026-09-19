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

/// A fresh [`Tree`] rooted at `path`, or `None` when the path is gone. Used after a
/// deletion to replace one branch of a scan instead of walking the whole root again.
///
/// `options` supplies the excludes and the same-volume rule of the scan this patch belongs
/// to; its `root` is ignored. The root node carries `path` as its name, normalized exactly
/// like the root of a [`scan`], so the patch and the tree it joins name the same path.
///
/// The tree describes what is at `path` *now*: a directory is walked, anything else — a
/// file, a symlink, a socket — becomes a single node, so a path that changed kind since the
/// scan is reported as it is today. Unlike [`scan`], a symlinked `path` is never followed:
/// the path is the one that was just deleted, and resolving it would walk the target of a
/// link the user removed. A directory that exists but cannot be listed comes back as a
/// single node carrying the error, as it would inside a full scan; a path that cannot be
/// read at all is a [`ScanError::Root`], and only a missing path is `None`.
///
/// Hard-linked data is attributed within `path` alone: twins outside it keep whatever the
/// last full scan decided, and a single node keeps its size even when it is one of several
/// links to the same data.
pub fn rescan_path(path: &Path, options: &ScanOptions) -> Result<Option<Tree>, ScanError> {
    // Normalize before the stat, not after: `lstat` follows a symlink whose path ends in a
    // separator, so `deleted-link/` would otherwise report — and then walk — its target.
    let path: PathBuf = path.components().collect();
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ScanError::Root { path, source }),
    };
    if meta.is_dir() {
        // `scan` stats the root once more and would follow it if it were a symlink by then.
        // Closing that window means walking from a file descriptor, which the scanner does
        // not do; what it leaves open is a path swapped between these two stats.
        let result = scan(&rescan_options(path, options), &ScanProgress::default())?;
        return Ok(Some(result.tree));
    }
    let node = node_from_metadata(&path.to_string_lossy(), &meta);
    Ok(Some(Subtree::new(node).flatten().0))
}

/// The options of the scan being patched, re-rooted at the rescanned path: everything but
/// the root still describes that scan, and a field added later is carried over by default.
fn rescan_options(root: PathBuf, options: &ScanOptions) -> ScanOptions {
    ScanOptions {
        root,
        ..options.clone()
    }
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
                leaves.push(unread(child, "skipped: different volume"));
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
    use super::*;

    /// The same-volume rule has no fixture that can observe it — that needs a second
    /// volume — so both of its values are pinned here instead.
    #[test]
    fn a_rescan_keeps_every_option_of_the_scan_but_the_root() {
        for same_device in [true, false] {
            let options = ScanOptions {
                root: PathBuf::from("/home/me"),
                excludes: vec![PathBuf::from("/home/me/Library")],
                same_device,
            };
            let patched = rescan_options(PathBuf::from("/home/me/cache"), &options);
            assert_eq!(patched.root, PathBuf::from("/home/me/cache"));
            assert_eq!(patched.excludes, options.excludes);
            assert_eq!(
                patched.same_device, same_device,
                "the volume rule of the scan is kept"
            );
        }
    }
}
