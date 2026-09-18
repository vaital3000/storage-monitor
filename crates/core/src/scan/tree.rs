//! Arena representation of a scanned directory tree.
//!
//! Nodes live in one `Vec` in breadth-first order: the root is id 0 and the children of a
//! node occupy a contiguous id range, largest first. Only the name needs a heap allocation
//! per node, so a scan of a few million entries stays within a few hundred megabytes.
//! Directory errors are rare and live in a side table.

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::fs::Metadata;
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

impl NodeKind {
    /// Classifies an entry from metadata that did not follow symlinks. The one place that
    /// decides: a scan and the re-validation before a deletion must agree, or an entry
    /// that is neither a file nor a directory — a socket, a fifo — would look like it had
    /// changed kind and could never be deleted.
    pub fn from_metadata(meta: &Metadata) -> Self {
        if meta.is_dir() {
            Self::Dir
        } else if meta.file_type().is_symlink() {
            Self::Symlink
        } else if meta.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }
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

    /// Counts hard-linked data once. Among the nodes sharing a `(dev, ino)` the one with
    /// the smallest path keeps its sizes; the others drop to 0 (allocated and logical) and
    /// every ancestor shrinks by the same amounts, so the outcome does not depend on the
    /// order in which the walker met the links. Sibling ranges whose order changed are
    /// sorted again (children keep pointing at their parents, errors follow their nodes).
    /// Returns how many nodes were zeroed.
    pub(crate) fn attribute_hard_links(&mut self, links: Vec<(u64, u64, NodeId)>) -> u64 {
        let mut links: Vec<((u64, u64), PathBuf, NodeId)> = links
            .into_iter()
            .map(|(dev, ino, id)| ((dev, ino), self.path(id), id))
            .collect();
        links.sort_unstable();
        let mut losers = Vec::new();
        let mut previous = None;
        for (key, _, id) in links {
            if previous == Some(key) {
                losers.push(id);
            } else {
                previous = Some(key);
            }
        }
        if losers.is_empty() {
            return 0;
        }
        let mut dirty = vec![false; self.nodes.len()];
        for &id in &losers {
            let loser = &mut self.nodes[id as usize];
            let size = std::mem::take(&mut loser.size);
            let logical_size = std::mem::take(&mut loser.logical_size);
            let mut current = id;
            while let Some(parent) = self.parent(current) {
                let ancestor = &mut self.nodes[parent as usize];
                ancestor.size -= size;
                ancestor.logical_size -= logical_size;
                dirty[parent as usize] = true;
                current = parent;
            }
        }
        // Deeper ranges first: a node moves only when its parent's range is sorted, and by
        // then the range it owns (marked under its old id) has already been handled.
        let mut moved = HashMap::new();
        for id in (0..self.nodes.len()).rev() {
            if dirty[id] {
                self.resort_children(id as NodeId, &mut moved);
            }
        }
        if !moved.is_empty() {
            for (id, _) in &mut self.errors {
                if let Some(new) = moved.get(id) {
                    *id = *new;
                }
            }
            self.errors.sort_by_key(|(id, _)| *id);
        }
        losers.len() as u64
    }

    /// Sorts the children of `parent` again after their sizes changed, recording every
    /// node that changed id in `moved` (old id to new id).
    fn resort_children(&mut self, parent: NodeId, moved: &mut HashMap<NodeId, NodeId>) {
        let range = self.children(parent);
        let (start, end) = (range.start as usize, range.end as usize);
        let block = &mut self.nodes[start..end];
        if block.is_sorted_by(|a, b| by_size_then_name(a, b).is_le()) {
            return;
        }
        // Both sorts are stable and use the same comparator, so afterwards `block[new]`
        // is the former `block[perm[new]]`.
        let mut perm: Vec<usize> = (0..block.len()).collect();
        perm.sort_by(|&a, &b| by_size_then_name(&block[a], &block[b]));
        block.sort_by(by_size_then_name);
        for (new, &old) in perm.iter().enumerate() {
            if new != old {
                moved.insert((start + old) as NodeId, (start + new) as NodeId);
            }
        }
        for id in start..end {
            let first = self.nodes[id].first_child as usize;
            let count = self.nodes[id].child_count as usize;
            for child in &mut self.nodes[first..first + count] {
                child.parent = id as NodeId;
            }
        }
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
        let mut total = 0;
        let mut stack = vec![self];
        while let Some(subtree) = stack.pop() {
            total += 1;
            stack.extend(&subtree.children);
        }
        total
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
    fn kinds_come_from_metadata_that_did_not_follow_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.bin");
        std::fs::write(&file, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        // A socket is neither a file nor a directory; the action engine has to see it as
        // `Other` both when it scans and when it re-checks before deleting.
        let socket = dir.path().join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let kind = |path: &Path| NodeKind::from_metadata(&std::fs::symlink_metadata(path).unwrap());
        assert_eq!(kind(dir.path()), NodeKind::Dir);
        assert_eq!(kind(&file), NodeKind::File);
        assert_eq!(kind(&link), NodeKind::Symlink);
        assert_eq!(kind(&socket), NodeKind::Other);
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
    fn hard_links_go_to_the_smallest_path_and_siblings_are_resorted() {
        // /r
        //   a/  z.bin (8, inode 7)             winner: /r/a/z.bin sorts before /r/b/a.bin
        //   b/  a.bin (8, inode 7), s.bin (4)  b carries an error that must follow it when it moves
        //   c.bin (5)
        let mut z = leaf("z.bin", 8);
        z.hardlink = Some((1, 7));
        let mut a_link = leaf("a.bin", 8);
        a_link.hardlink = Some((1, 7));
        let mut b = dir("b", 12, vec![a_link, leaf("s.bin", 4)]);
        b.error = Some("1 entry could not be read".into());
        let (mut tree, links) =
            dir("/r", 25, vec![dir("a", 8, vec![z]), b, leaf("c.bin", 5)]).flatten();
        assert_eq!(
            names(&tree, tree.children(Tree::ROOT)),
            vec!["b", "a", "c.bin"]
        );
        assert_eq!(tree.error(1), Some("1 entry could not be read"));

        assert_eq!(tree.attribute_hard_links(links), 1);

        assert_eq!(tree.root().size, 17);
        assert_eq!(tree.root().logical_size, 17, "logical bytes follow");
        assert_eq!(
            names(&tree, tree.children(Tree::ROOT)),
            vec!["a", "c.bin", "b"]
        );
        let a = tree.children(Tree::ROOT).next().unwrap();
        let b = tree.children(Tree::ROOT).nth(2).unwrap();
        assert_eq!(tree.get(a).unwrap().size, 8);
        assert_eq!(tree.get(b).unwrap().size, 4);
        assert_eq!(tree.get(b).unwrap().logical_size, 4);
        assert_eq!(tree.error(a), None);
        assert_eq!(tree.error(b), Some("1 entry could not be read"));
        assert_eq!(tree.errors().len(), 1);
        assert_eq!(names(&tree, tree.children(b)), vec!["s.bin", "a.bin"]);
        let a_link = tree.children(b).nth(1).unwrap();
        assert_eq!(tree.get(a_link).unwrap().size, 0);
        assert_eq!(tree.get(a_link).unwrap().logical_size, 0);
        assert_eq!(tree.parent(a_link), Some(b));
        assert_eq!(tree.path(a_link), PathBuf::from("/r/b/a.bin"));
        let z = tree.children(a).next().unwrap();
        assert_eq!(tree.get(z).unwrap().size, 8);
        assert_eq!(tree.parent(z), Some(a));
        assert_eq!(tree.path(z), PathBuf::from("/r/a/z.bin"));
    }

    #[test]
    fn attribution_without_duplicates_changes_nothing() {
        let mut only = leaf("only.bin", 8);
        only.hardlink = Some((1, 7));
        let (mut tree, links) = dir("/r", 8, vec![only]).flatten();
        assert_eq!(tree.attribute_hard_links(links), 0);
        assert_eq!(tree.root().size, 8);
        assert_eq!(tree.attribute_hard_links(Vec::new()), 0);
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
