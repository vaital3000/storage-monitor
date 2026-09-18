use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
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

/// Scans `options.root` in parallel. Never follows symlinks, stays on the root's volume,
/// counts hard-linked data once (under the smallest path) and records unreadable
/// directories instead of failing.
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
    };
    let root_node = node_from_metadata(&root.to_string_lossy(), &meta);
    let subtree = walk_dir(root, root_node, &ctx);
    let (mut tree, links) = subtree.flatten();
    let hardlinks_skipped = tree.attribute_hard_links(links);
    let snap = progress.snapshot();
    Ok(ScanResult {
        root: root.clone(),
        started_at,
        duration_ms: started.elapsed().as_millis() as u64,
        stats: ScanStats {
            files: snap.files,
            dirs: snap.dirs,
            bytes: tree.root().size,
            errors: snap.errors,
            hardlinks_skipped,
        },
        cancelled: snap.cancelled,
        tree,
    })
}

fn walk_dir(path: &Path, mut node: Node, ctx: &Ctx) -> Subtree {
    ctx.progress.enter(path);
    if ctx.progress.is_cancelled() {
        return Subtree::new(node);
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) => {
            ctx.progress.add_error();
            ctx.progress.add_dir(node.size);
            let mut subtree = Subtree::new(node);
            subtree.error = Some(err.to_string().into());
            return subtree;
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
        let child = node_from_metadata(&entry.file_name().to_string_lossy(), &meta);
        if meta.is_dir() {
            if ctx.is_excluded(&child_path) {
                continue;
            }
            if ctx.options.same_device && meta.dev() != ctx.root_dev {
                ctx.progress.add_dir(child.size);
                let mut subtree = Subtree::new(child);
                subtree.error = Some("skipped: different volume".into());
                leaves.push(subtree);
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
    ctx.progress.add_dir(node.size);
    for child in &children {
        node.size += child.node.size;
        node.logical_size += child.node.logical_size;
        node.file_count += child.node.file_count;
    }
    Subtree::with_children(node, children)
}

fn node_from_metadata(name: &str, meta: &Metadata) -> Node {
    let kind = if meta.is_dir() {
        NodeKind::Dir
    } else if meta.file_type().is_symlink() {
        NodeKind::Symlink
    } else if meta.is_file() {
        NodeKind::File
    } else {
        NodeKind::Other
    };
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
