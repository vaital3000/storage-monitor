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
        self.entries
            .iter()
            .map(|e| (e.path.clone(), e.size))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Node, NodeKind, Subtree};
    use crate::scan::{ScanResult, ScanStats};

    fn node(name: &str, kind: NodeKind, size: u64) -> Node {
        Node {
            name: name.into(),
            kind,
            parent: None,
            size,
            logical_size: size,
            file_count: u64::from(kind != NodeKind::Dir),
            mtime: 7,
            error: None,
            children: vec![],
        }
    }

    fn result() -> ScanResult {
        let tree = Subtree {
            node: node("/home", NodeKind::Dir, 0),
            children: vec![
                Subtree {
                    node: node("small.txt", NodeKind::File, 10),
                    children: vec![],
                },
                Subtree {
                    node: node("big.iso", NodeKind::File, 20_000),
                    children: vec![],
                },
                Subtree {
                    node: node("cache", NodeKind::Dir, 0),
                    children: vec![Subtree {
                        node: node("blob", NodeKind::File, 5_000),
                        children: vec![],
                    }],
                },
            ],
        }
        .flatten();
        ScanResult {
            root: "/home".into(),
            started_at: chrono::Utc::now(),
            duration_ms: 1,
            stats: ScanStats::default(),
            cancelled: false,
            tree,
        }
    }

    #[test]
    fn snapshot_keeps_directories_and_files_above_threshold() {
        let snap = Snapshot::from_result(&result(), 1_000);
        let paths: Vec<&str> = snap.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/home", "/home/big.iso", "/home/cache", "/home/cache/blob"]
        );
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
