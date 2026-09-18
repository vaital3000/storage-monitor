//! Payloads that cross IPC to the UI: the status of a scan and one page of the tree.
//! `src/lib/ipc.ts` mirrors these types in camelCase.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use storage_monitor_core::scan::{NodeId, NodeKind, Tree};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    #[default]
    Idle,
    Running,
    Done,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub state: ScanState,
    pub root: Option<String>,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    /// Directory being read; empty unless the scan is running.
    pub current_path: String,
    pub duration_ms: u64,
    pub error: Option<String>,
    /// A previous snapshot exists, so deltas are available.
    pub has_previous: bool,
    pub previous_taken_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    pub id: NodeId,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildView {
    pub id: NodeId,
    pub name: String,
    pub kind: NodeKind,
    pub size: u64,
    pub logical_size: u64,
    pub file_count: u64,
    pub mtime: i64,
    pub error: Option<String>,
    /// Growth since the previous snapshot; absent when the snapshot has no entry.
    pub delta: Option<i64>,
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    pub path: String,
    pub kind: NodeKind,
    pub size: u64,
    pub logical_size: u64,
    pub file_count: u64,
    pub mtime: i64,
    pub error: Option<String>,
    pub delta: Option<i64>,
    /// From the root down to this node, inclusive.
    pub breadcrumbs: Vec<Crumb>,
    /// Largest first, at most `limit` of them.
    pub children: Vec<ChildView>,
    pub children_total: u32,
    /// True when `children` is shorter than `children_total`.
    pub truncated: bool,
}

impl NodeView {
    /// One page of the tree: the node at `id`, its breadcrumbs and its `limit` largest
    /// children. `previous` maps absolute paths to sizes in the previous snapshot; only
    /// paths present there get a delta. `None` for an unknown id.
    pub fn build(
        tree: &Tree,
        id: NodeId,
        limit: usize,
        previous: Option<&HashMap<String, u64>>,
    ) -> Option<Self> {
        let node = tree.get(id)?;
        let path = tree.path(id);
        let delta_of = |path: &Path, size: u64| {
            previous
                .and_then(|sizes| sizes.get(path.to_string_lossy().as_ref()))
                .map(|before| size as i64 - *before as i64)
        };
        let breadcrumbs = tree
            .ancestors(id)
            .into_iter()
            .map(|ancestor| Crumb {
                id: ancestor,
                name: display_name(tree, ancestor),
            })
            .collect();
        let ids = tree.children(id);
        let children_total = tree.child_count(id);
        let children = ids
            .take(limit)
            .map(|child_id| {
                let child = tree.get(child_id).expect("child ids are valid");
                ChildView {
                    id: child_id,
                    name: child.name.to_string(),
                    kind: child.kind,
                    size: child.size,
                    logical_size: child.logical_size,
                    file_count: u64::from(child.file_count),
                    mtime: child.mtime,
                    error: tree.error(child_id).map(str::to_owned),
                    delta: delta_of(&path.join(&*child.name), child.size),
                    has_children: tree.has_children(child_id),
                }
            })
            .collect();
        Some(Self {
            id,
            name: display_name(tree, id),
            path: path.to_string_lossy().into_owned(),
            kind: node.kind,
            size: node.size,
            logical_size: node.logical_size,
            file_count: u64::from(node.file_count),
            mtime: node.mtime,
            error: tree.error(id).map(str::to_owned),
            delta: delta_of(&path, node.size),
            breadcrumbs,
            children,
            children_total,
            truncated: children_total as usize > limit,
        })
    }
}

/// The root node holds its absolute path; only its last component is shown (the whole
/// path when there is none, as for `/`).
fn display_name(tree: &Tree, id: NodeId) -> String {
    let name = tree.get(id).map_or("", |node| &*node.name);
    if id != Tree::ROOT {
        return name.to_owned();
    }
    Path::new(name).file_name().map_or_else(
        || name.to_owned(),
        |last| last.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_monitor_core::scan::{Node, Subtree};

    fn file(name: &str, size: u64) -> Subtree {
        Subtree::new(Node::new(name, NodeKind::File, size, size, 1, 7))
    }

    /// Directory totals are aggregated by the walker, so fixtures state them.
    fn dir(name: &str, size: u64, file_count: u32, children: Vec<Subtree>) -> Subtree {
        Subtree::with_children(
            Node::new(name, NodeKind::Dir, size, size, file_count, 7),
            children,
        )
    }

    /// /home/me
    ///   a/          35
    ///     x.bin     30
    ///     y.bin      5
    ///   b.bin       10
    ///   locked/      0  (unreadable)
    fn sample() -> Tree {
        let mut locked = dir("locked", 0, 0, vec![]);
        locked.error = Some("permission denied".into());
        dir(
            "/home/me",
            45,
            3,
            vec![
                dir("a", 35, 2, vec![file("y.bin", 5), file("x.bin", 30)]),
                file("b.bin", 10),
                locked,
            ],
        )
        .flatten()
        .0
    }

    fn child_ids(tree: &Tree) -> (NodeId, NodeId, NodeId) {
        let mut ids = tree.children(Tree::ROOT);
        (
            ids.next().unwrap(),
            ids.next().unwrap(),
            ids.next().unwrap(),
        )
    }

    #[test]
    fn root_page_lists_children_largest_first() {
        let tree = sample();
        let view = NodeView::build(&tree, Tree::ROOT, 500, None).unwrap();
        assert_eq!(view.id, Tree::ROOT);
        assert_eq!(view.name, "me", "the root shows its last component");
        assert_eq!(view.path, "/home/me");
        assert_eq!(view.kind, NodeKind::Dir);
        assert_eq!((view.size, view.logical_size, view.file_count), (45, 45, 3));
        assert_eq!(view.mtime, 7);
        assert_eq!(view.error, None);
        assert_eq!(view.delta, None);
        assert_eq!(
            view.breadcrumbs,
            vec![Crumb {
                id: Tree::ROOT,
                name: "me".into()
            }]
        );
        let names: Vec<&str> = view.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b.bin", "locked"]);
        assert_eq!(view.children_total, 3);
        assert!(!view.truncated);

        let (a, b, locked) = child_ids(&tree);
        let [va, vb, vlocked] = view.children.as_slice() else {
            panic!("three children")
        };
        assert_eq!((va.id, vb.id, vlocked.id), (a, b, locked));
        assert_eq!(va.kind, NodeKind::Dir);
        assert!(va.has_children);
        assert_eq!((va.size, va.logical_size, va.file_count), (35, 35, 2));
        assert_eq!(vb.kind, NodeKind::File);
        assert!(!vb.has_children);
        assert_eq!(vb.file_count, 1);
        assert!(!vlocked.has_children);
        assert_eq!(vlocked.error.as_deref(), Some("permission denied"));
        assert!(view.children.iter().all(|c| c.delta.is_none()));
    }

    #[test]
    fn breadcrumbs_lead_from_the_root_to_the_node() {
        let tree = sample();
        let (a, _, _) = child_ids(&tree);
        let x = tree.children(a).next().unwrap();
        let view = NodeView::build(&tree, x, 500, None).unwrap();
        assert_eq!(view.name, "x.bin");
        assert_eq!(view.path, "/home/me/a/x.bin");
        let crumbs: Vec<(NodeId, &str)> = view
            .breadcrumbs
            .iter()
            .map(|c| (c.id, c.name.as_str()))
            .collect();
        assert_eq!(crumbs, vec![(Tree::ROOT, "me"), (a, "a"), (x, "x.bin")]);
        assert!(view.children.is_empty());
        assert_eq!(view.children_total, 0);
        assert!(!view.truncated);
    }

    #[test]
    fn limit_truncates_the_children_but_not_the_total() {
        let tree = sample();
        let view = NodeView::build(&tree, Tree::ROOT, 1, None).unwrap();
        assert_eq!(view.children.len(), 1);
        assert_eq!(view.children[0].name, "a");
        assert_eq!(view.children_total, 3);
        assert!(view.truncated);
        let exact = NodeView::build(&tree, Tree::ROOT, 3, None).unwrap();
        assert_eq!(exact.children.len(), 3);
        assert!(!exact.truncated);
    }

    #[test]
    fn deltas_exist_only_for_paths_in_the_previous_snapshot() {
        let tree = sample();
        let previous: HashMap<String, u64> =
            [("/home/me".to_owned(), 30), ("/home/me/a".to_owned(), 40)]
                .into_iter()
                .collect();
        let view = NodeView::build(&tree, Tree::ROOT, 500, Some(&previous)).unwrap();
        assert_eq!(view.delta, Some(15));
        let deltas: Vec<Option<i64>> = view.children.iter().map(|c| c.delta).collect();
        assert_eq!(deltas, vec![Some(-5), None, None]);
        let (a, _, _) = child_ids(&tree);
        let inner = NodeView::build(&tree, a, 500, Some(&previous)).unwrap();
        assert_eq!(inner.delta, Some(-5));
        assert!(inner.children.iter().all(|c| c.delta.is_none()));
    }

    #[test]
    fn a_root_without_a_last_component_keeps_its_path() {
        let tree = dir("/", 1, 1, vec![file("f", 1)]).flatten().0;
        let view = NodeView::build(&tree, Tree::ROOT, 500, None).unwrap();
        assert_eq!(view.name, "/");
        assert_eq!(view.path, "/");
        assert_eq!(view.breadcrumbs[0].name, "/");
        assert_eq!(view.children[0].name, "f");
    }

    #[test]
    fn an_unknown_id_has_no_page() {
        assert!(NodeView::build(&sample(), 99, 500, None).is_none());
    }

    #[test]
    fn payloads_serialize_in_camel_case() {
        assert_eq!(
            serde_json::to_string(&ScanState::Running).unwrap(),
            "\"running\""
        );
        let status = ScanStatus {
            state: ScanState::Done,
            root: Some("/home/me".into()),
            files: 1,
            dirs: 1,
            bytes: 1,
            errors: 0,
            current_path: String::new(),
            duration_ms: 5,
            error: None,
            has_previous: true,
            previous_taken_at: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "done");
        assert_eq!(json["hasPrevious"], true);
        assert_eq!(json["durationMs"], 5);
        assert!(json["previousTakenAt"].is_null());
        let view = NodeView::build(&sample(), Tree::ROOT, 1, None).unwrap();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["childrenTotal"], 3);
        assert_eq!(json["children"][0]["hasChildren"], true);
        assert_eq!(json["children"][0]["kind"], "dir");
        assert_eq!(json["children"][0]["fileCount"], 2);
    }
}
