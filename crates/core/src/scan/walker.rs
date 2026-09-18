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
        self.seen_inodes
            .lock()
            .map(|mut set| set.insert((dev, ino)))
            .unwrap_or(true)
    }
}

/// Scans `options.root` in parallel. Never follows symlinks, stays on the root's volume,
/// counts hard-linked data once and records unreadable directories instead of failing.
pub fn scan(options: &ScanOptions, progress: &ScanProgress) -> Result<ScanResult, ScanError> {
    let started = Instant::now();
    let started_at = Utc::now();
    let root = &options.root;
    let meta = fs::symlink_metadata(root).map_err(|source| ScanError::Root {
        path: root.clone(),
        source,
    })?;
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
        return Subtree {
            node,
            children: Vec::new(),
        };
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) => {
            node.error = Some(err.to_string());
            ctx.progress.add_error();
            ctx.progress.add_dir(node.size);
            return Subtree {
                node,
                children: Vec::new(),
            };
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
                leaves.push(Subtree {
                    node: child,
                    children: Vec::new(),
                });
                continue;
            }
            dirs.push((child_path, child));
        } else {
            if meta.nlink() > 1 && !ctx.first_sighting(meta.dev(), meta.ino()) {
                child.size = 0;
                ctx.hardlinks_skipped.fetch_add(1, Ordering::Relaxed);
            }
            ctx.progress.add_file(child.size);
            leaves.push(Subtree {
                node: child,
                children: Vec::new(),
            });
        }
    }

    let subdirs: Vec<Subtree> = dirs
        .into_par_iter()
        .map(|(p, n)| walk_dir(&p, n, ctx))
        .collect();

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
