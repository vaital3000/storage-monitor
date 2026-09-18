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
    #[error("postcard: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("decompress: {0}")]
    Decompress(#[from] lz4_flex::block::DecompressError),
    #[error("unsupported snapshot format {0}")]
    Format(u32),
}

impl Snapshot {
    /// Every directory plus files whose allocated size is at least `file_threshold`,
    /// depth first (children largest first, as in the tree): a subtree's entries share
    /// their path prefix and sit next to each other, which compresses well.
    pub fn from_result(result: &ScanResult, file_threshold: u64) -> Self {
        let tree: &Tree = &result.tree;
        let mut entries = Vec::new();
        let mut stack = vec![Tree::ROOT];
        while let Some(id) = stack.pop() {
            let node = tree.get(id).expect("ids on the stack come from the tree");
            let keep = node.kind == NodeKind::Dir || node.size >= file_threshold;
            if keep {
                entries.push(SnapshotEntry {
                    path: tree.path(id).to_string_lossy().into_owned(),
                    kind: node.kind,
                    size: node.size,
                    file_count: u64::from(node.file_count),
                    mtime: node.mtime,
                });
            }
            // Reversed, so the first child is the next one popped.
            stack.extend(tree.children(id).rev());
        }
        Self {
            format: SNAPSHOT_FORMAT,
            taken_at: result.started_at,
            root: result.root.clone(),
            total_bytes: tree.root().size,
            file_count: u64::from(tree.root().file_count),
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
        // `format` is the first field, so another layout is reported as such and not as
        // a garbled payload.
        let (format, _) = postcard::take_from_bytes::<u32>(&raw)?;
        if format != SNAPSHOT_FORMAT {
            return Err(CodecError::Format(format));
        }
        Ok(postcard::from_bytes(&raw)?)
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
        Node::new(name, kind, size, size, u32::from(kind != NodeKind::Dir), 7)
    }

    fn result() -> ScanResult {
        let (tree, _) = Subtree::with_children(
            node("/home", NodeKind::Dir, 0),
            vec![
                Subtree::new(node("small.txt", NodeKind::File, 10)),
                Subtree::new(node("big.iso", NodeKind::File, 20_000)),
                Subtree::with_children(
                    node("cache", NodeKind::Dir, 0),
                    vec![Subtree::new(node("blob", NodeKind::File, 5_000))],
                ),
            ],
        )
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
    fn entries_are_in_depth_first_order() {
        // Arena order is breadth first (/r, d1, f, d2, big1, big2); the snapshot lists a
        // subtree in one run so its shared prefixes sit next to each other.
        let (tree, _) = Subtree::with_children(
            node("/r", NodeKind::Dir, 750),
            vec![
                Subtree::with_children(
                    node("d2", NodeKind::Dir, 200),
                    vec![Subtree::new(node("big2", NodeKind::File, 200))],
                ),
                Subtree::new(node("f", NodeKind::File, 250)),
                Subtree::with_children(
                    node("d1", NodeKind::Dir, 300),
                    vec![Subtree::new(node("big1", NodeKind::File, 300))],
                ),
            ],
        )
        .flatten();
        let result = ScanResult {
            root: "/r".into(),
            started_at: chrono::Utc::now(),
            duration_ms: 1,
            stats: ScanStats::default(),
            cancelled: false,
            tree,
        };
        let snap = Snapshot::from_result(&result, 0);
        let paths: Vec<&str> = snap.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/r", "/r/d1", "/r/d1/big1", "/r/f", "/r/d2", "/r/d2/big2"]
        );
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
    fn decode_rejects_another_format_before_reading_the_payload() {
        // A format 99 snapshot may have any layout: only the leading varint is inspected.
        let raw = postcard::to_allocvec(&(99u32, 0xFFu8)).unwrap();
        let err = Snapshot::decode(&lz4_flex::compress_prepend_size(&raw)).unwrap_err();
        assert!(matches!(err, CodecError::Format(99)), "{err}");
        let err = Snapshot::decode(&lz4_flex::compress_prepend_size(&[])).unwrap_err();
        assert!(matches!(err, CodecError::Postcard(_)), "{err}");
    }

    #[test]
    fn size_index_maps_paths_to_sizes() {
        let snap = Snapshot::from_result(&result(), 1_000);
        let index = snap.size_index();
        assert_eq!(index.get("/home/big.iso"), Some(&20_000));
        assert_eq!(index.get("/home/small.txt"), None);
    }
}
