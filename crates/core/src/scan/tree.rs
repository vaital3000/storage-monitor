//! Arena representation of a scanned directory tree.
//!
//! Nodes live in one `Vec` in breadth-first order: the root is id 0 and the children of a
//! node occupy a contiguous id range, largest first. Only the name needs a heap allocation
//! per node, so a scan of a few million entries stays within a few hundred megabytes.
//! Directory errors are rare and live in a side table.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Index of a node inside a [`Tree`].
pub type NodeId = u32;

const NO_PARENT: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    Dir,
    File,
    Symlink,
    Other,
}

/// One entry of a scanned tree. Links to relatives are private; navigate with
/// [`Tree::parent`], [`Tree::children`] and [`Tree::ancestors`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// File name; the root node holds its absolute path instead.
    pub name: Box<str>,
    pub kind: NodeKind,
    /// Allocated bytes on disk (`st_blocks * 512`); for directories the subtree total.
    pub size: u64,
    /// Logical bytes (`st_size`); for directories the subtree total.
    pub logical_size: u64,
    /// Entries in the subtree that are not directories (1 for a file, 0 for an empty directory).
    pub file_count: u32,
    /// Modification time in seconds since the Unix epoch.
    pub mtime: i64,
    parent: u32,
    first_child: u32,
    child_count: u32,
}

impl Node {
    /// A node without relatives; [`Subtree::flatten`] links it into a tree.
    pub fn new(
        name: &str,
        kind: NodeKind,
        size: u64,
        logical_size: u64,
        file_count: u32,
        mtime: i64,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            size,
            logical_size,
            file_count,
            mtime,
            parent: NO_PARENT,
            first_child: 0,
            child_count: 0,
        }
    }
}

/// Larger first, then by name, so listings are stable across scans.
fn by_size_then_name(a: &Node, b: &Node) -> Ordering {
    b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))
}

/// A scanned tree stored as an arena. The root is always node 0.
#[derive(Debug, Clone)]
pub struct Tree {
    nodes: Vec<Node>,
    /// Sorted by id.
    errors: Vec<(NodeId, Box<str>)>,
}

impl Tree {
    pub const ROOT: NodeId = 0;

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

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.get(id).map(|n| n.parent).filter(|p| *p != NO_PARENT)
    }

    /// Ids of the children of `id`, sorted by size descending then name ascending;
    /// empty for leaves and unknown ids.
    pub fn children(&self, id: NodeId) -> Range<NodeId> {
        match self.get(id) {
            Some(n) => n.first_child..n.first_child + n.child_count,
            None => 0..0,
        }
    }

    pub fn child_count(&self, id: NodeId) -> u32 {
        self.get(id).map_or(0, |n| n.child_count)
    }

    pub fn has_children(&self, id: NodeId) -> bool {
        self.child_count(id) > 0
    }

    /// Ids from the root down to `id`, inclusive.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut current = id;
        while let Some(parent) = self.parent(current) {
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
                path.push(Path::new(&*node.name));
            }
        }
        path
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().map(|(i, n)| (i as NodeId, n))
    }

    /// Why a directory could not be read (fully), if it could not.
    pub fn error(&self, id: NodeId) -> Option<&str> {
        self.errors
            .binary_search_by_key(&id, |(i, _)| *i)
            .ok()
            .map(|i| &*self.errors[i].1)
    }

    /// Every recorded error, sorted by id.
    pub fn errors(&self) -> &[(NodeId, Box<str>)] {
        &self.errors
    }
}

/// Nested form produced by the walker before it is flattened into an arena.
#[derive(Debug)]
pub struct Subtree {
    pub node: Node,
    pub children: Vec<Subtree>,
    /// `(dev, ino)` of an entry with more than one hard link; [`Subtree::flatten`] reports
    /// these so the scanner can count the shared data once.
    pub hardlink: Option<(u64, u64)>,
    /// Why the directory could not be read (fully); becomes [`Tree::error`].
    pub error: Option<Box<str>>,
}

impl Subtree {
    pub fn new(node: Node) -> Self {
        Self::with_children(node, Vec::new())
    }

    pub fn with_children(node: Node, children: Vec<Subtree>) -> Self {
        Self {
            node,
            children,
            hardlink: None,
            error: None,
        }
    }

    /// Nodes in this subtree, itself included.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Subtree::count).sum::<usize>()
    }

    /// Moves the nodes into an arena in breadth-first order (see [`Tree`]). Also returns
    /// `(dev, ino, id)` for every node that had [`Subtree::hardlink`] set, in id order.
    pub fn flatten(self) -> (Tree, Vec<(u64, u64, NodeId)>) {
        let mut arena = Arena {
            nodes: Vec::with_capacity(self.count()),
            errors: Vec::new(),
            hardlinks: Vec::new(),
            pending: VecDeque::new(),
        };
        arena.place(self, NO_PARENT);
        while let Some((parent, mut children)) = arena.pending.pop_front() {
            children.sort_unstable_by(|a, b| by_size_then_name(&a.node, &b.node));
            arena.nodes[parent as usize].first_child = arena.nodes.len() as NodeId;
            arena.nodes[parent as usize].child_count = children.len() as u32;
            for child in children {
                arena.place(child, parent);
            }
        }
        let Arena {
            nodes,
            errors,
            hardlinks,
            ..
        } = arena;
        (Tree { nodes, errors }, hardlinks)
    }
}

struct Arena {
    nodes: Vec<Node>,
    errors: Vec<(NodeId, Box<str>)>,
    hardlinks: Vec<(u64, u64, NodeId)>,
    /// Nodes already placed whose children still have to be appended, in id order.
    pending: VecDeque<(NodeId, Vec<Subtree>)>,
}

impl Arena {
    fn place(&mut self, subtree: Subtree, parent: u32) {
        let id = self.nodes.len() as NodeId;
        let Subtree {
            mut node,
            children,
            hardlink,
            error,
        } = subtree;
        node.parent = parent;
        if let Some((dev, ino)) = hardlink {
            self.hardlinks.push((dev, ino, id));
        }
        if let Some(message) = error {
            self.errors.push((id, message));
        }
        if !children.is_empty() {
            self.pending.push_back((id, children));
        }
        self.nodes.push(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, kind: NodeKind, size: u64) -> Node {
        Node::new(name, kind, size, size, u32::from(kind != NodeKind::Dir), 0)
    }

    fn leaf(name: &str, size: u64) -> Subtree {
        Subtree::new(node(name, NodeKind::File, size))
    }

    fn dir(name: &str, size: u64, children: Vec<Subtree>) -> Subtree {
        Subtree::with_children(node(name, NodeKind::Dir, size), children)
    }

    fn names(tree: &Tree, ids: Range<NodeId>) -> Vec<&str> {
        ids.map(|id| &*tree.get(id).unwrap().name).collect()
    }

    fn sample() -> Tree {
        // /root
        //   a/        (dir)
        //     x.bin   (30)
        //     y.bin    (5)
        //   b.bin     (10)
        dir(
            "/root",
            0,
            vec![
                dir("a", 0, vec![leaf("y.bin", 5), leaf("x.bin", 30)]),
                leaf("b.bin", 10),
            ],
        )
        .flatten()
        .0
    }

    #[test]
    fn flatten_assigns_parents_and_children() {
        let tree = sample();
        assert_eq!(tree.len(), 5);
        assert!(!tree.is_empty());
        assert_eq!(tree.parent(Tree::ROOT), None);
        assert_eq!(tree.child_count(Tree::ROOT), 2);
        // a is a dir with size 0 here (aggregation happens in the walker), so b.bin (10) comes first
        let a = tree.children(Tree::ROOT).nth(1).unwrap();
        assert_eq!(&*tree.get(a).unwrap().name, "a");
        assert_eq!(tree.parent(a), Some(Tree::ROOT));
        assert_eq!(tree.child_count(a), 2);
        assert!(tree.has_children(a));
        let x = tree.children(a).next().unwrap();
        assert!(!tree.has_children(x));
        assert_eq!(tree.children(x), 0..0);
        assert!(tree.get(99).is_none());
        assert_eq!(tree.parent(99), None);
        assert_eq!(tree.children(99), 0..0);
        assert_eq!(tree.child_count(99), 0);
    }

    #[test]
    fn children_are_sorted_by_size_descending() {
        let tree = sample();
        assert_eq!(names(&tree, tree.children(Tree::ROOT)), vec!["b.bin", "a"]);
        let a = tree.children(Tree::ROOT).nth(1).unwrap();
        assert_eq!(names(&tree, tree.children(a)), vec!["x.bin", "y.bin"]);
    }

    #[test]
    fn children_ids_are_contiguous_and_ties_break_on_name() {
        // Ties on size are ordered by name; a directory's children follow all of its
        // parent's children (breadth first), so every sibling range is contiguous.
        let tree = dir(
            "/r",
            0,
            vec![
                leaf("c.bin", 10),
                dir("d", 10, vec![leaf("inner.bin", 10)]),
                leaf("a.bin", 10),
                leaf("b.bin", 20),
            ],
        )
        .flatten()
        .0;
        assert_eq!(tree.len(), 6);
        assert_eq!(tree.children(Tree::ROOT), 1..5);
        assert_eq!(names(&tree, 1..5), vec!["b.bin", "a.bin", "c.bin", "d"]);
        assert_eq!(tree.children(4), 5..6);
        assert_eq!(&*tree.get(5).unwrap().name, "inner.bin");
        assert_eq!(tree.parent(5), Some(4));
        let ids: Vec<NodeId> = tree.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn path_and_ancestors_are_reconstructed_from_names() {
        let tree = sample();
        let a = tree.children(Tree::ROOT).nth(1).unwrap();
        let x = tree.children(a).next().unwrap();
        assert_eq!(tree.path(x), PathBuf::from("/root/a/x.bin"));
        assert_eq!(tree.ancestors(x), vec![Tree::ROOT, a, x]);
        assert_eq!(tree.path(Tree::ROOT), PathBuf::from("/root"));
        assert_eq!(tree.ancestors(Tree::ROOT), vec![Tree::ROOT]);
    }

    #[test]
    fn errors_live_in_a_side_table_sorted_by_id() {
        let mut locked = dir("locked", 0, vec![]);
        locked.error = Some("permission denied".into());
        let mut root = dir("/r", 0, vec![leaf("big.bin", 50), locked]);
        root.error = Some("2 entries could not be read".into());
        let tree = root.flatten().0;
        assert_eq!(tree.error(Tree::ROOT), Some("2 entries could not be read"));
        assert_eq!(tree.error(1), None, "big.bin has no error");
        assert_eq!(tree.error(2), Some("permission denied"));
        assert_eq!(tree.error(99), None);
        let ids: Vec<NodeId> = tree.errors().iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![0, 2]);
    }

    #[test]
    fn flatten_counts_nodes_and_reports_hard_linked_ones() {
        let mut a = leaf("a.bin", 10);
        a.hardlink = Some((1, 42));
        let mut b = leaf("b.bin", 10);
        b.hardlink = Some((1, 42));
        let subtree = dir("/r", 0, vec![a, b, leaf("c.bin", 5)]);
        assert_eq!(subtree.count(), 4);
        let (tree, links) = subtree.flatten();
        assert_eq!(tree.len(), 4);
        assert_eq!(links, vec![(1, 42, 1), (1, 42, 2)]);
    }

    #[test]
    fn node_is_compact() {
        let size = std::mem::size_of::<Node>();
        assert!(size <= 64, "Node is {size} bytes");
    }

    #[test]
    fn node_kind_serializes_in_camel_case() {
        assert_eq!(serde_json::to_string(&NodeKind::Dir).unwrap(), "\"dir\"");
        assert_eq!(
            serde_json::to_string(&NodeKind::Symlink).unwrap(),
            "\"symlink\""
        );
    }
}
