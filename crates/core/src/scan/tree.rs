//! Arena representation of a scanned directory tree.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Index of a node inside a [`Tree`].
pub type NodeId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// File name; the root node holds its absolute path instead.
    pub name: String,
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// Allocated bytes on disk (`st_blocks * 512`); for directories the subtree total.
    pub size: u64,
    /// Logical bytes (`st_size`); for directories the subtree total.
    pub logical_size: u64,
    /// Entries in the subtree that are not directories (1 for a file, 0 for an empty directory).
    pub file_count: u64,
    /// Modification time in seconds since the Unix epoch.
    pub mtime: i64,
    /// Set when a directory could not be read (fully).
    pub error: Option<String>,
    /// Children, sorted by `size` descending after [`Tree::sort_children_by_size`].
    pub children: Vec<NodeId>,
}

/// A scanned tree stored as an arena. The root is always node 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    pub const ROOT: NodeId = 0;

    pub fn from_nodes(nodes: Vec<Node>) -> Self {
        assert!(!nodes.is_empty(), "a tree needs at least a root node");
        Self { nodes }
    }

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

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.get(id).map(|n| n.children.as_slice()).unwrap_or(&[])
    }

    /// Ids from the root down to `id`, inclusive.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut current = id;
        while let Some(parent) = self.get(current).and_then(|n| n.parent) {
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
                path.push(Path::new(&node.name));
            }
        }
        path
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().map(|(i, n)| (i as NodeId, n))
    }

    pub fn sort_children_by_size(&mut self) {
        let sizes: Vec<u64> = self.nodes.iter().map(|n| n.size).collect();
        for node in &mut self.nodes {
            node.children.sort_by(|a, b| {
                sizes[*b as usize]
                    .cmp(&sizes[*a as usize])
                    .then_with(|| a.cmp(b))
            });
        }
    }
}

/// Nested form produced by the walker before it is flattened into an arena.
#[derive(Debug)]
pub struct Subtree {
    pub node: Node,
    pub children: Vec<Subtree>,
}

impl Subtree {
    pub fn flatten(self) -> Tree {
        let mut nodes = Vec::new();
        push(&mut nodes, self, None);
        Tree::from_nodes(nodes)
    }
}

fn push(nodes: &mut Vec<Node>, subtree: Subtree, parent: Option<NodeId>) -> NodeId {
    let id = nodes.len() as NodeId;
    let Subtree { mut node, children } = subtree;
    node.parent = parent;
    node.children = Vec::new();
    nodes.push(node);
    let ids: Vec<NodeId> = children
        .into_iter()
        .map(|c| push(nodes, c, Some(id)))
        .collect();
    nodes[id as usize].children = ids;
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, kind: NodeKind, size: u64) -> Node {
        Node {
            name: name.to_owned(),
            kind,
            parent: None,
            size,
            logical_size: size,
            file_count: if kind == NodeKind::Dir { 0 } else { 1 },
            mtime: 0,
            error: None,
            children: Vec::new(),
        }
    }

    fn sample() -> Tree {
        // /root
        //   a/        (dir)
        //     x.bin   (30)
        //     y.bin    (5)
        //   b.bin     (10)
        let root = Subtree {
            node: node("/root", NodeKind::Dir, 0),
            children: vec![
                Subtree {
                    node: node("a", NodeKind::Dir, 0),
                    children: vec![
                        Subtree {
                            node: node("y.bin", NodeKind::File, 5),
                            children: vec![],
                        },
                        Subtree {
                            node: node("x.bin", NodeKind::File, 30),
                            children: vec![],
                        },
                    ],
                },
                Subtree {
                    node: node("b.bin", NodeKind::File, 10),
                    children: vec![],
                },
            ],
        };
        let mut tree = root.flatten();
        tree.sort_children_by_size();
        tree
    }

    #[test]
    fn flatten_assigns_parents_and_children() {
        let tree = sample();
        assert_eq!(tree.len(), 5);
        let root = tree.root();
        assert_eq!(root.parent, None);
        assert_eq!(root.children.len(), 2);
        let a = tree.get(root.children[1]).unwrap();
        assert_eq!(a.name, "a");
        assert_eq!(a.parent, Some(Tree::ROOT));
        assert_eq!(a.children.len(), 2);
    }

    #[test]
    fn children_are_sorted_by_size_descending() {
        let tree = sample();
        let names: Vec<&str> = tree
            .children(Tree::ROOT)
            .iter()
            .map(|id| tree.get(*id).unwrap().name.as_str())
            .collect();
        // a is a dir with size 0 here (aggregation happens in the walker), so b.bin (10) first
        assert_eq!(names, vec!["b.bin", "a"]);
        let a = tree.root().children[1];
        let inner: Vec<&str> = tree
            .children(a)
            .iter()
            .map(|id| tree.get(*id).unwrap().name.as_str())
            .collect();
        assert_eq!(inner, vec!["x.bin", "y.bin"]);
    }

    #[test]
    fn path_and_ancestors_are_reconstructed_from_names() {
        let tree = sample();
        let a = tree.root().children[1];
        let x = tree.children(a)[0];
        assert_eq!(tree.path(x), std::path::PathBuf::from("/root/a/x.bin"));
        assert_eq!(tree.ancestors(x), vec![Tree::ROOT, a, x]);
        assert_eq!(tree.path(Tree::ROOT), std::path::PathBuf::from("/root"));
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
