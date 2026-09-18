//! The JSON report of `storage-monitor scan` and the helpers of its text form.

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
        let taken: Vec<ReportNode> = ids
            .clone()
            .take(top)
            .map(|c| build(tree, c, depth - 1, top))
            .collect();
        (taken, ids.len() > top)
    };
    ReportNode {
        name: node.name.to_string(),
        kind: node.kind,
        size: node.size,
        logical_size: node.logical_size,
        file_count: u64::from(node.file_count),
        mtime: node.mtime,
        error: tree.error(id).map(str::to_owned),
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
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_monitor_core::scan::{Node, ScanStats, Subtree};

    fn file(name: &str, size: u64) -> Subtree {
        Subtree::new(Node::new(name, NodeKind::File, size, size, 1, 0))
    }

    /// Directory totals are aggregated by the walker, so fixtures state them.
    fn dir(name: &str, size: u64, file_count: u32, children: Vec<Subtree>) -> Subtree {
        Subtree::with_children(
            Node::new(name, NodeKind::Dir, size, size, file_count, 0),
            children,
        )
    }

    fn result() -> ScanResult {
        let mut locked = dir("locked", 0, 0, vec![]);
        locked.error = Some("permission denied".into());
        let (tree, _) = dir(
            "/root",
            45,
            3,
            vec![
                dir("a", 35, 2, vec![file("x.bin", 30), file("y.bin", 5)]),
                file("b.bin", 10),
                locked,
            ],
        )
        .flatten();
        ScanResult {
            root: "/root".into(),
            started_at: Utc::now(),
            duration_ms: 1,
            stats: ScanStats::default(),
            cancelled: false,
            tree,
        }
    }

    #[test]
    fn depth_cuts_the_tree_and_marks_the_cut() {
        let tree = report_tree(&result(), 1, 20);
        assert_eq!(tree.name, "/root");
        assert_eq!(tree.file_count, 3);
        assert!(!tree.truncated);
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b.bin", "locked"]);
        assert!(tree.children[0].children.is_empty());
        assert!(tree.children[0].truncated, "a has children below the depth");
        assert!(!tree.children[1].truncated, "a file has nothing to cut");
        assert_eq!(tree.children[2].error.as_deref(), Some("permission denied"));
    }

    #[test]
    fn top_keeps_the_largest_children_and_marks_the_cut() {
        let tree = report_tree(&result(), 2, 1);
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].name, "a");
        assert!(tree.truncated);
        assert_eq!(tree.children[0].children.len(), 1);
        assert_eq!(tree.children[0].children[0].name, "x.bin");
        assert!(tree.children[0].truncated);
    }

    #[test]
    fn human_bytes_uses_decimal_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_000), "1.0 KB");
        assert_eq!(human_bytes(1_536_000), "1.5 MB");
        assert_eq!(human_bytes(2_000_000_000_000), "2.0 TB");
        assert_eq!(human_bytes(u64::MAX), "18446.7 PB");
    }
}
