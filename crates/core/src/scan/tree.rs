//! Arena representation of a scanned directory tree.
//!
//! Nodes live in one `Vec` in breadth-first order: the root is id 0 and the children of a
//! node occupy a contiguous id range, largest first. Only the name needs a heap allocation
//! per node, so a scan of a few million entries stays within a few hundred megabytes.
//! Directory errors are rare and live in a side table.

use std::cmp::{Ordering, Reverse};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs::Metadata;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

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

/// Larger first, then by name, so listings are stable across scans. One definition, used
/// both when a scan is flattened and when a patched group is sorted again, so a rebuilt
/// group ends up in the order a fresh walk would have put it in.
fn order_key(size: u64, name: &str) -> (Reverse<u64>, &str) {
    (Reverse(size), name)
}

fn by_size_then_name(a: &Node, b: &Node) -> Ordering {
    order_key(a.size, &a.name).cmp(&order_key(b.size, &b.name))
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

    /// The node at an absolute path, or `None` when the path is not in this tree. The
    /// inverse of [`Tree::path`], which is how a path that has been through the UI finds
    /// its node again.
    ///
    /// The match is component by component against the root's absolute name and then the
    /// names below it, so a trailing separator changes nothing and a path that merely
    /// starts with the root's name — `/roots` under a scan of `/root` — is not in the tree.
    /// Names are unique within a directory, so no backtracking is needed. Anything but a
    /// plain name below the root, `..` in particular, matches nothing rather than being
    /// resolved: a patch is spliced onto what the path says, and the path has to say it.
    ///
    /// **The path has to be spelled the way the scan spelled it.** Every comparison here is
    /// byte-exact per component: no symlink is resolved, no case is folded, no Unicode is
    /// normalized. Three paths that name the right file therefore still answer `None` —
    /// a canonicalized one (`/private/var/…` against a scan of `/var/…`, or any root
    /// reached through a symlink), a case-different one on a case-insensitive volume
    /// (`library` against `Library`), and NFD where the disk gave the walker NFC. Feed it
    /// what came out of [`Tree::path`], which is where the UI got it.
    ///
    /// In particular, do **not** feed it [`Checked::judged`](crate::action::Checked): the
    /// guards manufacture that value by canonicalizing, which is precisely the spelling
    /// this rejects. `Checked::path` is the one that still carries the caller's components.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let root = self.nodes.first()?;
        let below = path.strip_prefix(Path::new(&*root.name)).ok()?;
        let mut current = Self::ROOT;
        for component in below.components() {
            let Component::Normal(name) = component else {
                return None;
            };
            current = self
                .children(current)
                .find(|id| OsStr::new(&*self.nodes[*id as usize].name) == name)?;
        }
        Some(current)
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

/// The three subtree totals of a node: what a patch below it changes about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Totals {
    size: u64,
    logical_size: u64,
    file_count: u32,
}

impl Totals {
    const ZERO: Self = Self {
        size: 0,
        logical_size: 0,
        file_count: 0,
    };

    fn of(node: &Node) -> Self {
        Self {
            size: node.size,
            logical_size: node.logical_size,
            file_count: node.file_count,
        }
    }

    fn plus(self, other: Self) -> Self {
        Self {
            size: self.size.saturating_add(other.size),
            logical_size: self.logical_size.saturating_add(other.logical_size),
            file_count: self.file_count.saturating_add(other.file_count),
        }
    }

    /// Saturating, because this is where a directory's own blocks are recovered: a walked
    /// tree always weighs at least as much as its children, a tree assembled by hand need
    /// not, and patching one is not the place to panic over the difference.
    fn minus(self, other: Self) -> Self {
        Self {
            size: self.size.saturating_sub(other.size),
            logical_size: self.logical_size.saturating_sub(other.logical_size),
            file_count: self.file_count.saturating_sub(other.file_count),
        }
    }
}

/// One node of the arena being rebuilt.
#[derive(Clone, Copy)]
struct Item<'a> {
    /// Where the node, its error and its children come from: the tree being patched, or
    /// one of the replacements.
    tree: &'a Tree,
    id: NodeId,
    /// Whether `tree` is the tree being patched. Patches and recomputed totals are keyed by
    /// the ids of that tree alone — inside a replacement the same numbers name other nodes.
    patchable: bool,
    /// The name the node keeps: its own, except for a replacement's root.
    name: &'a str,
    /// What the node weighs once the patch is in.
    totals: Totals,
}

impl<'a> Item<'a> {
    fn of(tree: &'a Tree, id: NodeId, patchable: bool, totals: Totals) -> Self {
        Self {
            tree,
            id,
            patchable,
            name: &tree.nodes[id as usize].name,
            totals,
        }
    }

    /// The node as it goes into the new arena. The links to the children are left empty
    /// rather than copied from `source`, whose range indexes the arena the node came from
    /// and means nothing in this one; what fills them is the placement of that group, which
    /// every node with children goes through.
    fn into_node(self, parent: u32) -> Node {
        let source = &self.tree.nodes[self.id as usize];
        Node {
            name: self.name.into(),
            kind: source.kind,
            size: self.totals.size,
            logical_size: self.totals.logical_size,
            file_count: self.totals.file_count,
            mtime: source.mtime,
            parent,
            first_child: 0,
            child_count: 0,
        }
    }
}

/// A new arena with the listed nodes replaced by the given trees, or dropped when the
/// replacement is `None`. Ancestors are re-aggregated and the sibling groups that changed
/// are sorted again; everything else keeps its order. The root cannot be patched.
///
/// A replacement keeps the **name** of the node it takes the place of: a rescanned tree
/// names its root with the absolute path it was walked from, while the tree it joins holds
/// the file name there. Everything else is the replacement's, its kind included — a path
/// that is a file where the scan saw a directory is spliced in as the file it is now — and
/// its root is measured again from its own children, which a tree [`scan`](super::scan)
/// built answers with the totals it already carries.
///
/// Three kinds of patch are ignored, each naming a node the result cannot hold: the root
/// (an arena without one is not a tree), an id this tree does not know, and a patch below
/// another patch, whose target the outer replacement has already taken the place of. A
/// repeated id keeps the last patch given for it; otherwise the order of the patches does
/// not matter.
///
/// Only `size`, `logical_size` and `file_count` are recomputed, and only for the ancestors.
/// An ancestor keeps the `mtime` it was scanned with, although the deletion below it has
/// just changed that on disk, and hard links are not attributed again — a rescan that took
/// bytes back from a twin outside it keeps them (see [`rescan_path`](super::rescan_path)).
/// The next full scan puts both right.
///
/// The arena is rebuilt rather than edited: a sibling group is a contiguous range of ids,
/// so there is nowhere to grow one in place. That costs a copy of the tree — a few hundred
/// milliseconds for a few million nodes — and in exchange every invariant of the arena
/// holds by construction. Both arenas are alive at once, so peak memory is about twice the
/// tree: measured at +284 MiB over a 469 MiB one.
///
/// **The cost is per call, not per path**, and the gap is three orders of magnitude: one
/// call carrying 100 patches takes 120 ms, the same 100 patches one call each take ten
/// seconds. Collect a whole batch and splice it in one call.
///
/// A note for whoever changes this next: `Option<Option<Tree>>` says "drop" and "replace"
/// in a shape that has to be decoded at both match sites, and an
/// `enum Patch { Drop, Replace(Tree) }` would say it outright. It was considered and left
/// alone — it moves a public signature and the tests pinned to it for readability alone.
/// Worth doing if this file is opened for another reason.
pub fn replace_subtrees(tree: &Tree, patches: Vec<(NodeId, Option<Tree>)>) -> Tree {
    if tree.is_empty() {
        return tree.clone();
    }
    let replacements = applicable_patches(tree, patches);
    let totals = re_aggregate_ancestors(tree, &replacements);

    // Breadth-first placement, exactly as `Subtree::flatten` does it.
    let mut rebuild = Rebuild::new(tree, &replacements);
    let root = Item::of(
        tree,
        Tree::ROOT,
        true,
        patched_totals(tree, Tree::ROOT, &replacements, &totals),
    );
    rebuild.place(root, NO_PARENT);
    while let Some((parent, item)) = rebuild.pending.pop_front() {
        let mut children = children_of(&item, &replacements, &totals);
        // Only a group holding a node whose size changed can be out of order, and those are
        // the groups whose parent is an ancestor of a patch. Every other group keeps the
        // order the walker gave it.
        //
        // This is the third and weakest use of `patchable`, and the only one that is an
        // optimisation rather than a rule: sorting every group instead is *correct*, and
        // measured, barely slower — 96–105 ms either way over 3.7 M nodes, because the sort
        // finds the existing run in one pass and the rebuild is dominated by name
        // allocations. Keep it, but do not read it as a correctness argument and do not let
        // it vouch for the other two. The lookups in `children_of` are the lethal ones:
        // there `patchable` is what keeps an id from being read in the wrong arena.
        if item.patchable && totals.contains_key(&item.id) {
            children.sort_by(|a, b| {
                order_key(a.totals.size, a.name).cmp(&order_key(b.totals.size, b.name))
            });
        }
        // A node reaches this queue because it had children before the patch, so a group
        // that comes back empty is one the patch emptied. `first_child` stays 0 for it, the
        // value `flatten` leaves on a leaf: `children` reads the same `0..0` either way, but
        // two nodes that agree about having no children should not differ in the field, or
        // the first comparison of a patched `Node` against a walked one goes wrong.
        if !children.is_empty() {
            rebuild.nodes[parent as usize].first_child = rebuild.nodes.len() as NodeId;
            rebuild.nodes[parent as usize].child_count = children.len() as u32;
        }
        for child in children {
            rebuild.place(child, parent);
        }
    }
    let Rebuild { nodes, errors, .. } = rebuild;
    // Placed in id order, so the error table comes out sorted, as `Tree::error` needs.
    Tree { nodes, errors }
}

/// The arena being rebuilt, and the ceiling that keeps a defect in the rebuild from
/// becoming a hang.
struct Rebuild<'a> {
    nodes: Vec<Node>,
    errors: Vec<(NodeId, Box<str>)>,
    /// Nodes already placed whose children still have to be appended, in id order.
    pending: VecDeque<(NodeId, Item<'a>)>,
    /// The most nodes the result can hold: every node of the tree being patched, plus every
    /// node of every replacement that will be spliced in. A patched node's own branch goes
    /// away, so the result is this or fewer — only the no-op rebuild reaches it exactly —
    /// and a rebuild that wants one more has placed something twice.
    ceiling: usize,
}

impl<'a> Rebuild<'a> {
    fn new(tree: &Tree, replacements: &HashMap<NodeId, Option<Tree>>) -> Self {
        let ceiling = tree.len()
            + replacements
                .values()
                .flatten()
                .map(Tree::len)
                .sum::<usize>();
        Self {
            nodes: Vec::with_capacity(tree.len()),
            errors: Vec::new(),
            pending: VecDeque::new(),
            ceiling,
        }
    }

    /// Appends one node and queues its children.
    ///
    /// Every node is placed once, so a node that does not fit under the ceiling means the
    /// walk has re-entered something it already left — which the queue turns into a rebuild
    /// that never ends. Left to run it costs a hung suite that names nothing, so it is
    /// caught at the node that crosses the line, whose name says where the walk doubled
    /// back.
    fn place(&mut self, item: Item<'a>, parent: u32) {
        assert!(
            self.nodes.len() < self.ceiling,
            "the rebuild ran past {} nodes while placing {:?} under {parent}: a node is \
             being reached twice, so some lookup crossed between the tree being patched \
             and a replacement (see `Item::patchable`)",
            self.ceiling,
            item.name,
        );
        let id = self.nodes.len() as NodeId;
        self.nodes.push(item.into_node(parent));
        if let Some(message) = item.tree.error(item.id) {
            self.errors.push((id, message.into()));
        }
        if item.tree.child_count(item.id) > 0 {
            self.pending.push_back((id, item));
        }
    }
}

/// The children of an item, with the patches applied to them.
///
/// Both maps are keyed by the ids of the tree being patched, so both are consulted only for
/// an item that came from it. Below a splice the same numbers name the replacement's own
/// nodes: node 1 of a two-node patch is its single child, while patch 1 is whatever the
/// caller asked to replace in the other tree — looking it up there splices the replacement
/// in under itself, which no ancestor check catches because the ancestry it would consult
/// belongs to the wrong tree. `patchable` travels down with the children for the same
/// reason: an item is of the tree being patched only when its parent was.
fn children_of<'a>(
    item: &Item<'a>,
    replacements: &'a HashMap<NodeId, Option<Tree>>,
    totals: &HashMap<NodeId, Totals>,
) -> Vec<Item<'a>> {
    let mut children = Vec::with_capacity(item.tree.child_count(item.id) as usize);
    for id in item.tree.children(item.id) {
        match replacements.get(&id).filter(|_| item.patchable) {
            // Dropped: the branch is not walked, so its errors do not follow it either.
            Some(None) => {}
            Some(Some(replacement)) => {
                // An empty replacement cannot be built outside this module, and reaching
                // for a root it does not have would panic on a tree the user is browsing.
                if !replacement.is_empty() {
                    let mut spliced =
                        Item::of(replacement, Tree::ROOT, false, spliced_totals(replacement));
                    spliced.name = &item.tree.nodes[id as usize].name;
                    children.push(spliced);
                }
            }
            None => {
                let node = &item.tree.nodes[id as usize];
                let recomputed = if item.patchable {
                    totals.get(&id).copied()
                } else {
                    None
                };
                let totals = recomputed.unwrap_or_else(|| Totals::of(node));
                children.push(Item::of(item.tree, id, item.patchable, totals));
            }
        }
    }
    children
}

/// The patches that can be applied, by id: the root, an id the tree does not know and a
/// patch below another patch are dropped. A repeated id keeps the last patch given for it.
fn applicable_patches(
    tree: &Tree,
    patches: Vec<(NodeId, Option<Tree>)>,
) -> HashMap<NodeId, Option<Tree>> {
    let mut replacements: HashMap<NodeId, Option<Tree>> = HashMap::new();
    for (id, replacement) in patches {
        if id != Tree::ROOT && tree.get(id).is_some() {
            replacements.insert(id, replacement);
        }
    }
    let nested: Vec<NodeId> = replacements
        .keys()
        .copied()
        .filter(|id| {
            let mut current = *id;
            while let Some(parent) = tree.parent(current) {
                if replacements.contains_key(&parent) {
                    return true;
                }
                current = parent;
            }
            false
        })
        .collect();
    for id in nested {
        replacements.remove(&id);
    }
    replacements
}

/// A directory's own allocated blocks plus what its children weigh now. The blocks are what
/// is left of its total when its children's totals are taken out, and they survive a patch:
/// a directory of a million entries occupies megabytes that belong to no child.
fn aggregate(tree: &Tree, id: NodeId, child_totals: impl Fn(NodeId) -> Totals) -> Totals {
    let mut sum = Totals::of(&tree.nodes[id as usize]);
    for child in tree.children(id) {
        sum = sum.minus(Totals::of(&tree.nodes[child as usize]));
    }
    for child in tree.children(id) {
        sum = sum.plus(child_totals(child));
    }
    sum
}

/// What a replacement weighs once it is spliced in: its root, measured again from its own
/// children. A tree that came out of the walker answers with the totals it already carries,
/// and one assembled by hand cannot splice in a directory that disagrees with the children
/// listed right under it.
fn spliced_totals(replacement: &Tree) -> Totals {
    aggregate(replacement, Tree::ROOT, |child| {
        Totals::of(&replacement.nodes[child as usize])
    })
}

/// The new totals of every ancestor of a patch.
fn re_aggregate_ancestors(
    tree: &Tree,
    replacements: &HashMap<NodeId, Option<Tree>>,
) -> HashMap<NodeId, Totals> {
    let mut ancestors: HashSet<NodeId> = HashSet::new();
    for id in replacements.keys() {
        let mut current = *id;
        while let Some(parent) = tree.parent(current) {
            // Every chain is walked to the root, so a node already in means the rest is in.
            if !ancestors.insert(parent) {
                break;
            }
            current = parent;
        }
    }
    let mut order: Vec<NodeId> = ancestors.into_iter().collect();
    // Deepest first: a child's id is always larger than its parent's, so an ancestor is
    // aggregated only once every ancestor below it has been.
    order.sort_unstable_by(|a, b| b.cmp(a));
    let mut totals = HashMap::with_capacity(order.len());
    for id in order {
        let sum = aggregate(tree, id, |child| {
            patched_totals(tree, child, replacements, &totals)
        });
        totals.insert(id, sum);
    }
    totals
}

/// What a node of the old tree weighs after the patch: nothing when it is dropped, the
/// replacement's totals when it is replaced, the recomputed ones when it is an ancestor of
/// a patch, its own otherwise.
fn patched_totals(
    tree: &Tree,
    id: NodeId,
    replacements: &HashMap<NodeId, Option<Tree>>,
    totals: &HashMap<NodeId, Totals>,
) -> Totals {
    match replacements.get(&id) {
        Some(None) => Totals::ZERO,
        Some(Some(replacement)) if replacement.is_empty() => Totals::ZERO,
        Some(Some(replacement)) => spliced_totals(replacement),
        None => totals
            .get(&id)
            .copied()
            .unwrap_or_else(|| Totals::of(&tree.nodes[id as usize])),
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

    /// A directory whose totals are aggregated the way [`walk_dir`](super::walker) does it:
    /// its own allocated blocks plus everything below it. The `dir` helper above leaves the
    /// totals to the caller, which is what the arena tests want and what the patching ones
    /// cannot use: re-aggregation is only meaningful over a tree that was aggregated once.
    fn aggregated(name: &str, own: u64, children: Vec<Subtree>) -> Subtree {
        let mut node = node(name, NodeKind::Dir, own);
        node.logical_size = 0;
        for child in &children {
            node.size += child.node.size;
            node.logical_size += child.node.logical_size;
            node.file_count += child.node.file_count;
        }
        Subtree::with_children(node, children)
    }

    /// ```text
    /// /root            own 20, total 200
    ///   a              own 65, total 100
    ///     inner        own  5, total  35
    ///       x.bin (30)
    ///   b.bin (80)
    /// ```
    fn nested() -> Tree {
        aggregated(
            "/root",
            20,
            vec![
                aggregated(
                    "a",
                    65,
                    vec![aggregated("inner", 5, vec![leaf("x.bin", 30)])],
                ),
                leaf("b.bin", 80),
            ],
        )
        .flatten()
        .0
    }

    fn find(tree: &Tree, path: &str) -> NodeId {
        tree.find(Path::new(path))
            .unwrap_or_else(|| panic!("no node at {path}"))
    }

    fn totals_of(tree: &Tree, path: &str) -> (u64, u64, u32) {
        let node = tree.get(find(tree, path)).unwrap();
        (node.size, node.logical_size, node.file_count)
    }

    /// The three totals as plain numbers, so the arithmetic below is written out by hand
    /// instead of going through the [`Totals`] the rebuild itself sums with.
    fn weight(node: &Node) -> (u64, u64, u32) {
        (node.size, node.logical_size, node.file_count)
    }

    /// Nodes in the subtree rooted at `id`, itself included — `subtree_size` of
    /// `crates/core/tests/walker.rs`, which verifies a rescan the same way.
    fn subtree_len(tree: &Tree, id: NodeId) -> usize {
        let mut total = 0;
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            total += 1;
            stack.extend(tree.children(current));
        }
        total
    }

    /// Every error of a tree keyed by the path it sits on, which is what has to survive a
    /// rebuild: the ids around it move.
    fn errors_by_path(tree: &Tree) -> Vec<(String, &str)> {
        tree.errors()
            .iter()
            .map(|(id, message)| (tree.path(*id).display().to_string(), &**message))
            .collect()
    }

    /// Every structural invariant of the arena, which a rebuild has to re-establish and a
    /// small fixture would not notice losing: breadth-first order, sibling ranges that
    /// partition the arena, back-pointers, sorted groups, a sorted error table.
    fn assert_invariants(tree: &Tree) {
        if tree.is_empty() {
            assert!(tree.errors().is_empty(), "errors without nodes");
            return;
        }
        assert_eq!(tree.nodes[0].parent, NO_PARENT, "the root has no parent");
        let mut next = 1;
        for id in 0..tree.len() as NodeId {
            let node = &tree.nodes[id as usize];
            if node.child_count == 0 {
                // A group the patch emptied has to come out looking like a leaf the walker
                // built, down to the unused field: `children` reads `0..0` either way, so
                // nothing here notices — until two `Node`s are compared across a splice, or
                // a test asserts `children(x) == 0..0` on a patched tree the way
                // `flatten_assigns_parents_and_children` does on a flattened one.
                assert_eq!(
                    node.first_child, 0,
                    "{id} has no children but still points at {}",
                    node.first_child
                );
                continue;
            }
            assert_eq!(
                node.first_child as usize, next,
                "the children of {id} do not follow the previous group"
            );
            next = (node.first_child + node.child_count) as usize;
            assert!(
                next <= tree.len(),
                "the children of {id} run past the arena"
            );
            for child in tree.children(id) {
                assert_eq!(tree.nodes[child as usize].parent, id, "parent of {child}");
                assert!(child > id, "child {child} comes before its parent {id}");
            }
            let group = &tree.nodes[node.first_child as usize..next];
            assert!(
                group.is_sorted_by(|a, b| by_size_then_name(a, b).is_le()),
                "the children of {id} are not sorted: {:?}",
                group.iter().map(|n| (&n.name, n.size)).collect::<Vec<_>>()
            );
        }
        assert_eq!(next, tree.len(), "some node is not anybody's child");
        let ids: Vec<NodeId> = tree.errors().iter().map(|(id, _)| *id).collect();
        assert!(
            ids.windows(2).all(|w| w[0] < w[1]),
            "the error table is not sorted: {ids:?}"
        );
        assert!(
            ids.iter().all(|id| (*id as usize) < tree.len()),
            "an error points outside the arena: {ids:?}"
        );
    }

    /// The recursive comparison `crates/core/tests/walker.rs` verifies a rescan with, so a
    /// splice is not checked more weakly than the tree being spliced: kind, sizes, counts,
    /// mtime, errors and child names, all the way down. The roots' names are not compared —
    /// a replacement names its root with the absolute path it was walked from, and the
    /// splice keeps the name of the node it takes the place of.
    fn assert_same_subtree(a: &Tree, a_id: NodeId, b: &Tree, b_id: NodeId) {
        let (x, y) = (a.get(a_id).unwrap(), b.get(b_id).unwrap());
        let at = b.path(b_id).display().to_string();
        assert_eq!(x.kind, y.kind, "kind at {at}");
        assert_eq!(x.size, y.size, "size at {at}");
        assert_eq!(x.logical_size, y.logical_size, "logical size at {at}");
        assert_eq!(x.file_count, y.file_count, "file count at {at}");
        assert_eq!(x.mtime, y.mtime, "mtime at {at}");
        assert_eq!(a.error(a_id), b.error(b_id), "error at {at}");
        assert_eq!(a.child_count(a_id), b.child_count(b_id), "children of {at}");
        for (a_child, b_child) in a.children(a_id).zip(b.children(b_id)) {
            assert_eq!(
                &*a.get(a_child).unwrap().name,
                &*b.get(b_child).unwrap().name,
                "child names under {at}"
            );
            assert_same_subtree(a, a_child, b, b_child);
        }
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

    #[test]
    fn find_locates_a_node_by_its_absolute_path() {
        let tree = sample(); // /root with a/{x.bin,y.bin} and b.bin
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        assert_eq!(tree.get(a).unwrap().name.as_ref(), "a");
        assert_eq!(tree.find(std::path::Path::new("/root")), Some(Tree::ROOT));
        assert_eq!(tree.find(std::path::Path::new("/root/nope")), None);
        assert_eq!(tree.find(std::path::Path::new("/elsewhere")), None);
    }

    #[test]
    fn dropping_a_node_shrinks_its_ancestors() {
        let tree = sample();
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        let patched = replace_subtrees(&tree, vec![(a, None)]);
        assert_eq!(patched.find(std::path::Path::new("/root/a")), None);
        assert_eq!(patched.root().size, 10, "only b.bin is left");
        assert_eq!(patched.root().file_count, 1);
        assert_eq!(patched.children(Tree::ROOT).len(), 1);
    }

    #[test]
    fn a_replacement_subtree_takes_the_place_of_the_old_one() {
        let tree = sample();
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        let remainder = Subtree::with_children(
            Node::new("whatever", NodeKind::Dir, 0, 0, 0, 7),
            vec![Subtree::new(Node::new("y.bin", NodeKind::File, 5, 5, 1, 7))],
        );
        let (replacement, _) = remainder.flatten();
        let patched = replace_subtrees(&tree, vec![(a, Some(replacement))]);
        let a = patched.find(std::path::Path::new("/root/a")).unwrap();
        assert_eq!(
            patched.get(a).unwrap().name.as_ref(),
            "a",
            "the old name is kept"
        );
        assert_eq!(patched.get(a).unwrap().size, 5);
        assert_eq!(patched.root().size, 15);
        assert_eq!(patched.child_count(a), 1);
    }

    #[test]
    fn children_of_a_changed_group_are_sorted_again() {
        let tree = sample(); // children of /root: b.bin (10), a (35)
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        let (small, _) = Subtree::new(Node::new("a", NodeKind::Dir, 1, 1, 0, 7)).flatten();
        let patched = replace_subtrees(&tree, vec![(a, Some(small))]);
        let names: Vec<&str> = patched
            .children(Tree::ROOT)
            .map(|id| patched.get(id).unwrap().name.as_ref())
            .collect();
        assert_eq!(names, vec!["b.bin", "a"], "largest first still holds");
    }

    #[test]
    fn errors_of_surviving_nodes_are_kept_and_of_dropped_ones_are_not() {
        let mut root = Subtree::with_children(
            Node::new("/root", NodeKind::Dir, 0, 0, 0, 7),
            vec![
                Subtree::new(Node::new("locked", NodeKind::Dir, 0, 0, 0, 7)),
                Subtree::new(Node::new("kept", NodeKind::Dir, 0, 0, 0, 7)),
            ],
        );
        root.children[0].error = Some("permission denied".into());
        root.children[1].error = Some("partially read".into());
        let (tree, _) = root.flatten();
        let locked = tree.find(std::path::Path::new("/root/locked")).unwrap();
        let patched = replace_subtrees(&tree, vec![(locked, None)]);
        let kept = patched.find(std::path::Path::new("/root/kept")).unwrap();
        assert_eq!(patched.error(kept), Some("partially read"));
        assert_eq!(patched.errors().len(), 1);
    }

    #[test]
    fn patching_nothing_returns_an_equal_tree() {
        let tree = sample();
        let patched = replace_subtrees(&tree, vec![]);
        assert_eq!(patched.len(), tree.len());
        assert_eq!(patched.root().size, tree.root().size);
    }

    // The six tests above are the contract of this task, and between them they leave the
    // rebuild wide open: `sample()` is a hand-built tree whose directories weigh nothing,
    // so its groups happen to stay in order and its ancestors happen to have no blocks of
    // their own. What follows measures the parts that fixture cannot reach.

    #[test]
    fn patching_nothing_rebuilds_the_tree_node_for_node() {
        // The identity case is the cheapest check that the rebuild itself is faithful: an
        // arena that came out of `flatten` must come out of `replace_subtrees` unchanged.
        let tree = nested();
        let patched = replace_subtrees(&tree, vec![]);
        assert_invariants(&patched);
        assert_same_subtree(&patched, Tree::ROOT, &tree, Tree::ROOT);
        assert_eq!(patched.len(), tree.len());
        assert_eq!(&*patched.root().name, &*tree.root().name);
    }

    #[test]
    fn find_looks_at_one_directory_at_a_time() {
        // The same name under two directories, and a name that only exists one level
        // deeper than the path says: a lookup that is not anchored to the children of the
        // node it has reached splices a patch into the wrong branch.
        let (tree, _) = aggregated(
            "/root",
            0,
            vec![
                aggregated("a", 0, vec![leaf("dup.bin", 7)]),
                aggregated("b", 0, vec![leaf("dup.bin", 3)]),
            ],
        )
        .flatten();
        let in_a = find(&tree, "/root/a/dup.bin");
        let in_b = find(&tree, "/root/b/dup.bin");
        assert_ne!(in_a, in_b);
        assert_eq!(tree.get(in_a).unwrap().size, 7);
        assert_eq!(tree.get(in_b).unwrap().size, 3);
        assert_eq!(tree.find(Path::new("/root/dup.bin")), None, "one level up");
        assert_eq!(tree.find(Path::new("/root/a/b")), None, "another branch");
    }

    #[test]
    fn find_walks_whole_components_only() {
        let tree = sample();
        let a = find(&tree, "/root/a");
        assert_eq!(tree.find(Path::new("/root/a/")), Some(a), "trailing slash");
        assert_eq!(tree.find(Path::new("/root/a/x.bin")), Some(3));
        // A name that merely starts with the root's, and a path that leaves the tree and
        // comes back: both would splice a patch onto a node it does not describe.
        assert_eq!(tree.find(Path::new("/rootx")), None);
        assert_eq!(tree.find(Path::new("/rootx/a")), None);
        assert_eq!(tree.find(Path::new("/root/a/../a")), None);
        assert_eq!(tree.find(Path::new("/root/a/x.bin/deeper")), None);
        assert_eq!(tree.find(Path::new("root/a")), None, "not absolute");
        assert_eq!(tree.find(Path::new("")), None);
        assert_eq!(tree.find(Path::new("/")), None);
    }

    #[test]
    fn find_strips_the_root_by_components_and_not_by_bytes() {
        // `/rootx` shares its first five bytes with the root and names another directory
        // entirely. A prefix taken off the string leaves `x`, which the scan below really
        // does hold — so the lookup succeeds and a patch lands on a node the caller never
        // named. The fixture above cannot show it: it has nothing called `x`.
        let (tree, _) = aggregated("/root", 0, vec![leaf("x", 5)]).flatten();
        assert_eq!(tree.find(Path::new("/root/x")), Some(1));
        assert_eq!(tree.find(Path::new("/rootx")), None);
        assert_eq!(tree.find(Path::new("/rootx/")), None);
    }

    #[test]
    fn find_refuses_a_component_that_is_not_a_plain_name() {
        // `Path::components` hands `..` over as it is rather than resolving it, so the guard
        // in `find` is what decides. A directory cannot be called `..` on a filesystem; a
        // tree can hold the name, and this is what keeps `/root/..` from answering with it
        // instead of with nothing.
        let (tree, _) = aggregated("/root", 0, vec![leaf("..", 5), leaf(".", 3)]).flatten();
        assert_eq!(tree.find(Path::new("/root/..")), None);
        assert_eq!(tree.find(Path::new("/root/.")), Some(Tree::ROOT), "dropped");
    }

    #[test]
    fn find_round_trips_the_path_of_every_node() {
        for tree in [sample(), nested()] {
            for (id, _) in tree.iter() {
                assert_eq!(tree.find(&tree.path(id)), Some(id), "{:?}", tree.path(id));
            }
        }
    }

    #[test]
    fn every_ancestor_is_re_aggregated_and_keeps_its_own_blocks() {
        // /root/a/inner/x.bin goes: `inner` keeps its 5 own blocks, `a` its 65, the root its
        // 20, and the root's children swap places because `a` now weighs less than b.bin.
        let tree = nested();
        let x = find(&tree, "/root/a/inner/x.bin");
        let patched = replace_subtrees(&tree, vec![(x, None)]);
        assert_invariants(&patched);
        assert_eq!(totals_of(&patched, "/root/a/inner"), (5, 0, 0));
        assert_eq!(totals_of(&patched, "/root/a"), (70, 0, 0));
        assert_eq!(totals_of(&patched, "/root"), (170, 80, 1));
        assert_eq!(
            names(&patched, patched.children(Tree::ROOT)),
            vec!["b.bin", "a"],
            "a is lighter than b.bin now"
        );
        assert_eq!(patched.len(), tree.len() - 1);
    }

    #[test]
    fn dropping_any_node_takes_exactly_its_weight_out_of_exactly_its_ancestors() {
        // The same arithmetic as the test above, but stated over every node of the tree
        // rather than over one the author picked: the ancestors lose precisely what the
        // node weighed, every node off that chain keeps its totals to the byte, and the
        // arena loses precisely the nodes of that branch. A patch that reached one node too
        // far, or one too few, shows up here whichever node it is.
        //
        // Aggregated fixtures only: the subtraction is written out with `-` rather than
        // with the saturating `Totals::minus` the rebuild uses, so a hand-built tree whose
        // directories weigh less than their children — `sample`, whose directories weigh
        // nothing — would fail the test rather than the code.
        let wide = aggregated(
            "/root",
            7,
            vec![
                aggregated(
                    "a",
                    5,
                    vec![
                        aggregated("inner", 3, vec![leaf("x.bin", 30), leaf("y.bin", 12)]),
                        leaf("t.bin", 9),
                    ],
                ),
                aggregated("b", 2, vec![leaf("u.bin", 50)]),
                leaf("c.bin", 1),
                aggregated("empty", 4, vec![]),
            ],
        )
        .flatten()
        .0;
        for tree in [nested(), wide] {
            for (id, node) in tree.iter() {
                if id == Tree::ROOT {
                    continue;
                }
                let (gone, path) = (weight(node), tree.path(id));
                let ancestors: HashSet<NodeId> = tree
                    .ancestors(id)
                    .into_iter()
                    .filter(|a| *a != id)
                    .collect();
                let patched = replace_subtrees(&tree, vec![(id, None)]);

                assert_invariants(&patched);
                assert_eq!(patched.find(&path), None, "{} survived", path.display());
                assert_eq!(
                    patched.len(),
                    tree.len() - subtree_len(&tree, id),
                    "dropping {}",
                    path.display()
                );
                for (other, before) in tree.iter() {
                    let at = tree.path(other);
                    // Everything below the dropped node went with it; names are unique in a
                    // directory, so no surviving node answers to one of those paths.
                    let Some(after) = patched.find(&at) else {
                        continue;
                    };
                    let (s, l, f) = weight(before);
                    let expected = if ancestors.contains(&other) {
                        (s - gone.0, l - gone.1, f - gone.2)
                    } else {
                        (s, l, f)
                    };
                    assert_eq!(
                        weight(patched.get(after).unwrap()),
                        expected,
                        "{} after dropping {}",
                        at.display(),
                        path.display()
                    );
                }
            }
        }
    }

    #[test]
    fn a_replacement_is_spliced_in_whole_and_keeps_only_the_old_name() {
        // The replacement is deeper and heavier than what it replaces, and its own ids
        // collide with the ids of the patched node's ancestors — which is why patches and
        // recomputed totals may only be looked up for nodes of the tree being patched.
        let tree = nested();
        let inner = find(&tree, "/root/a/inner");
        let (replacement, _) = aggregated(
            "/root/a/inner",
            7,
            vec![
                aggregated("deep", 3, vec![leaf("p.bin", 40), leaf("q.bin", 30)]),
                leaf("r.bin", 20),
            ],
        )
        .flatten();
        assert!(
            replacement.len() as NodeId > inner,
            "the replacement's own ids have to reach the patched id and its ancestors, \
             or this proves nothing"
        );
        let patched = replace_subtrees(&tree, vec![(inner, Some(replacement.clone()))]);
        assert_invariants(&patched);

        let spliced = find(&patched, "/root/a/inner");
        assert_eq!(
            patched.get(spliced).unwrap().name.as_ref(),
            "inner",
            "the old name is kept, the absolute path of the patch is not"
        );
        assert_same_subtree(&patched, spliced, &replacement, Tree::ROOT);
        // `inner` and the x.bin below it go, the replacement's five nodes arrive.
        assert_eq!(patched.len(), tree.len() - 2 + replacement.len());
        // 7 + 3 + 40 + 30 + 20 = 100
        assert_eq!(totals_of(&patched, "/root/a/inner"), (100, 90, 3));
        assert_eq!(totals_of(&patched, "/root/a"), (165, 90, 3));
        assert_eq!(totals_of(&patched, "/root"), (265, 170, 4));
    }

    #[test]
    fn a_replacement_of_another_kind_is_taken_as_it_is() {
        // A directory that is a file today: the rescan reports what is there now.
        let tree = nested();
        let inner = find(&tree, "/root/a/inner");
        let (file, _) = Subtree::new(node("/root/a/inner", NodeKind::File, 12)).flatten();
        let patched = replace_subtrees(&tree, vec![(inner, Some(file))]);
        assert_invariants(&patched);
        let spliced = find(&patched, "/root/a/inner");
        assert_eq!(patched.get(spliced).unwrap().kind, NodeKind::File);
        assert_eq!(patched.get(spliced).unwrap().name.as_ref(), "inner");
        assert!(!patched.has_children(spliced));
        assert_eq!(totals_of(&patched, "/root/a"), (77, 12, 1));
        assert_eq!(totals_of(&patched, "/root"), (177, 92, 2));
    }

    #[test]
    fn a_spliced_node_breaks_a_size_tie_on_the_name_it_kept() {
        // The name half of the sort key, which nothing else in this file measures: every
        // other fixture separates its siblings by size, so the comparator could read any
        // name at all — or none — and still pass.
        //
        // `zz` is patched down from 90 to 40, which ties it with `m.bin`, so the group is
        // decided on names alone. The two candidates fall on opposite sides: the kept name
        // `zz` sorts after `m.bin`, while the patch root's absolute path `/root/zz` sorts
        // before it, because `/` is below every letter. So reading the patch's name — or
        // dropping the name term and leaving the tie in arrival order — puts the spliced
        // node first, which is the user watching a directory they just shrank jump to the
        // top of the list.
        let (tree, _) = aggregated(
            "/root",
            0,
            vec![
                leaf("m.bin", 40),
                aggregated("zz", 0, vec![leaf("inner.bin", 90)]),
            ],
        )
        .flatten();
        assert_eq!(names(&tree, tree.children(Tree::ROOT)), vec!["zz", "m.bin"]);

        let (replacement, _) = aggregated("/root/zz", 0, vec![leaf("left.bin", 40)]).flatten();
        let patched = replace_subtrees(&tree, vec![(find(&tree, "/root/zz"), Some(replacement))]);

        assert_invariants(&patched);
        let spliced = find(&patched, "/root/zz");
        assert_eq!(patched.get(spliced).unwrap().name.as_ref(), "zz");
        assert_eq!(
            totals_of(&patched, "/root/zz").0,
            totals_of(&patched, "/root/m.bin").0,
            "the fixture only proves anything while the two tie",
        );
        assert_eq!(
            names(&patched, patched.children(Tree::ROOT)),
            vec!["m.bin", "zz"],
            "a tie is broken on the name the node kept, not on the patch's path",
        );
    }

    #[test]
    fn an_error_on_a_replacements_own_root_is_kept() {
        // `rescan_path` puts an error on the root of the tree it returns in two shapes that
        // both ship: the one-node answer for a mount point it refused to enter, and a
        // directory that has become partly unreadable since the scan. Both land here as a
        // replacement whose root — not whose children — carries the message, and the
        // splice has to carry it across.
        let mut refused = Subtree::new(node("/root/mnt", NodeKind::Dir, 8));
        refused.error = Some("skipped: different volume".into());
        let mut partial = aggregated("/root/docs", 4, vec![leaf("kept.bin", 10)]);
        partial.error = Some("1 entry could not be read".into());

        let mut root = aggregated(
            "/root",
            0,
            vec![
                aggregated("docs", 1, vec![leaf("old.bin", 70)]),
                aggregated("mnt", 1, vec![leaf("stale.bin", 60)]),
            ],
        );
        root.children[0].error = Some("the old message".into());
        let (tree, _) = root.flatten();
        assert_eq!(tree.errors().len(), 1);

        let patched = replace_subtrees(
            &tree,
            vec![
                (find(&tree, "/root/docs"), Some(partial.flatten().0)),
                (find(&tree, "/root/mnt"), Some(refused.flatten().0)),
            ],
        );

        assert_invariants(&patched);
        assert_eq!(
            errors_by_path(&patched),
            vec![
                ("/root/docs".to_owned(), "1 entry could not be read"),
                ("/root/mnt".to_owned(), "skipped: different volume"),
            ],
            "each replacement's own root keeps its message and the old one is gone",
        );
        // The refused mount point is the single-node shape: its error is on a leaf.
        assert!(!patched.has_children(find(&patched, "/root/mnt")));
        assert_eq!(totals_of(&patched, "/root/mnt"), (8, 8, 0));
        assert_eq!(totals_of(&patched, "/root/docs"), (14, 10, 1));
    }

    #[test]
    fn siblings_patched_in_one_call_do_not_lose_each_other() {
        let (tree, _) = aggregated(
            "/root",
            0,
            vec![
                leaf("w.bin", 40),
                aggregated("x", 30, vec![]),
                leaf("y.bin", 20),
                leaf("z.bin", 10),
            ],
        )
        .flatten();
        let (small, _) = Subtree::new(node("/root/x", NodeKind::Dir, 1)).flatten();
        // Deepest id first, so the order the patches arrive in cannot be what makes it work.
        let patched = replace_subtrees(
            &tree,
            vec![
                (find(&tree, "/root/z.bin"), None),
                (find(&tree, "/root/x"), Some(small)),
            ],
        );
        assert_invariants(&patched);
        assert_eq!(
            names(&patched, patched.children(Tree::ROOT)),
            vec!["w.bin", "y.bin", "x"]
        );
        assert_eq!(patched.len(), 4);
        assert_eq!(patched.root().size, 61);
        assert_eq!(patched.root().file_count, 2);
    }

    #[test]
    fn patches_in_two_branches_reach_the_common_ancestor() {
        // A grandparent whose total is stale while both parents are right is exactly what a
        // one-level re-aggregation produces.
        let (tree, _) = aggregated(
            "/root",
            1,
            vec![
                aggregated("left", 2, vec![leaf("l.bin", 100)]),
                aggregated("right", 4, vec![leaf("r.bin", 200)]),
            ],
        )
        .flatten();
        assert_eq!(tree.root().size, 307);
        let patched = replace_subtrees(
            &tree,
            vec![
                (find(&tree, "/root/left/l.bin"), None),
                (find(&tree, "/root/right/r.bin"), None),
            ],
        );
        assert_invariants(&patched);
        assert_eq!(totals_of(&patched, "/root/left"), (2, 0, 0));
        assert_eq!(totals_of(&patched, "/root/right"), (4, 0, 0));
        assert_eq!(totals_of(&patched, "/root"), (7, 0, 0));
        assert_eq!(
            names(&patched, patched.children(Tree::ROOT)),
            vec!["right", "left"]
        );
    }

    #[test]
    fn errors_follow_their_nodes_when_the_ids_move() {
        // The patch is heavier in nodes than what it replaces and lighter in bytes, so the
        // group is reordered and every id after it shifts: an error table copied by offset
        // lands on the wrong nodes.
        let mut big = aggregated("big", 0, vec![leaf("b1.bin", 60), leaf("b2.bin", 40)]);
        big.error = Some("big error".into());
        let mut small = aggregated("small", 0, vec![leaf("s.bin", 10)]);
        small.error = Some("small error".into());
        let mut root = aggregated("/root", 0, vec![big, small]);
        root.error = Some("root error".into());
        let (tree, _) = root.flatten();
        assert_eq!(
            errors_by_path(&tree),
            vec![
                ("/root".to_owned(), "root error"),
                ("/root/big".to_owned(), "big error"),
                ("/root/small".to_owned(), "small error"),
            ]
        );

        let mut sub = aggregated("sub", 0, vec![leaf("p.bin", 5)]);
        sub.error = Some("sub error".into());
        let (replacement, _) = aggregated("/root/big", 0, vec![sub]).flatten();
        let patched = replace_subtrees(&tree, vec![(find(&tree, "/root/big"), Some(replacement))]);

        assert_invariants(&patched);
        assert_eq!(
            names(&patched, patched.children(Tree::ROOT)),
            vec!["small", "big"]
        );
        assert_eq!(
            errors_by_path(&patched),
            vec![
                ("/root".to_owned(), "root error"),
                ("/root/small".to_owned(), "small error"),
                ("/root/big/sub".to_owned(), "sub error"),
            ],
            "the replaced node's error is gone, every other one follows its node"
        );
        assert_eq!(
            patched
                .errors()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![0, 1, 4]
        );
    }

    #[test]
    fn the_root_cannot_be_patched() {
        let tree = nested();
        let (replacement, _) = Subtree::new(node("/elsewhere", NodeKind::Dir, 9)).flatten();
        for patch in [None, Some(replacement)] {
            let patched = replace_subtrees(&tree, vec![(Tree::ROOT, patch)]);
            assert_invariants(&patched);
            assert_same_subtree(&patched, Tree::ROOT, &tree, Tree::ROOT);
            assert_eq!(&*patched.root().name, "/root");
        }
    }

    #[test]
    fn an_id_the_tree_does_not_know_is_ignored() {
        let tree = nested();
        let patched = replace_subtrees(&tree, vec![(99, None), (u32::MAX, None)]);
        assert_invariants(&patched);
        assert_same_subtree(&patched, Tree::ROOT, &tree, Tree::ROOT);
    }

    #[test]
    fn a_patch_below_another_patch_is_ignored() {
        // The outer replacement already says what the whole branch holds; the inner id
        // names a node that is no longer there.
        let tree = nested();
        let a = find(&tree, "/root/a");
        let inner = find(&tree, "/root/a/inner");
        let x = find(&tree, "/root/a/inner/x.bin");
        let (replacement, _) = aggregated("/root/a", 3, vec![leaf("kept.bin", 9)]).flatten();
        let patched = replace_subtrees(
            &tree,
            vec![(inner, None), (a, Some(replacement)), (x, None)],
        );
        assert_invariants(&patched);
        assert_eq!(
            names(&patched, patched.children(find(&patched, "/root/a"))),
            vec!["kept.bin"]
        );
        assert_eq!(totals_of(&patched, "/root/a"), (12, 9, 1));
        assert_eq!(totals_of(&patched, "/root"), (112, 89, 2));
        assert_eq!(patched.len(), 4);

        // The same nesting the other way round: the branch goes, the inner patch with it.
        let (replacement, _) = aggregated("/root/a/inner", 3, vec![leaf("kept.bin", 9)]).flatten();
        let patched = replace_subtrees(&tree, vec![(a, None), (inner, Some(replacement))]);
        assert_invariants(&patched);
        assert_eq!(patched.find(Path::new("/root/a")), None);
        assert_eq!(patched.len(), 2);
        assert_eq!(totals_of(&patched, "/root"), (100, 80, 1));
    }

    #[test]
    fn a_nested_patch_is_dropped_before_the_rebuild_starts() {
        // The test above pins the result, which the rebuild reaches by never looking a
        // nested patch up: below a splice the ids belong to the replacement, and
        // `patched_totals` answers from `replacements` before it consults the recomputed
        // ones. So the whole list would survive the rebuild unread, and only a reader of
        // `applicable_patches` would be told what a patch list means. The contract is
        // stated there, so it is measured there: two mechanisms happening to agree is not
        // the same as a rule.
        let tree = nested();
        let (a, inner, x) = (
            find(&tree, "/root/a"),
            find(&tree, "/root/a/inner"),
            find(&tree, "/root/a/inner/x.bin"),
        );
        let (replacement, _) = Subtree::new(node("/root/a", NodeKind::Dir, 1)).flatten();
        let mut kept: Vec<NodeId> = applicable_patches(
            &tree,
            vec![(inner, None), (a, Some(replacement)), (x, None)],
        )
        .into_keys()
        .collect();
        kept.sort_unstable();
        assert_eq!(kept, vec![a], "only the outermost patch of the branch");

        // A grandchild of a dropped node goes too, and a patch in another branch stays.
        let b = find(&tree, "/root/b.bin");
        let mut kept: Vec<NodeId> =
            applicable_patches(&tree, vec![(x, None), (a, None), (b, None)])
                .into_keys()
                .collect();
        kept.sort_unstable();
        assert_eq!(kept, vec![a, b]);
    }

    #[test]
    fn the_last_patch_for_a_node_wins_and_the_order_does_not_matter() {
        let tree = nested();
        let inner = find(&tree, "/root/a/inner");
        let x = find(&tree, "/root/a/inner/x.bin");
        let (first, _) = Subtree::new(node("/root/a/inner", NodeKind::Dir, 11)).flatten();
        let (second, _) = Subtree::new(node("/root/a/inner", NodeKind::Dir, 22)).flatten();
        let patched = replace_subtrees(&tree, vec![(inner, Some(first)), (inner, Some(second))]);
        assert_invariants(&patched);
        assert_eq!(totals_of(&patched, "/root/a/inner").0, 22);

        // Two independent patches, both orders, same tree.
        let (small, _) = Subtree::new(node("/root/b.bin", NodeKind::File, 1)).flatten();
        let b = find(&tree, "/root/b.bin");
        let forwards = replace_subtrees(&tree, vec![(x, None), (b, Some(small.clone()))]);
        let backwards = replace_subtrees(&tree, vec![(b, Some(small)), (x, None)]);
        assert_same_subtree(&forwards, Tree::ROOT, &backwards, Tree::ROOT);
        assert_eq!(totals_of(&forwards, "/root"), (91, 1, 1));
    }

    #[test]
    fn an_empty_tree_is_neither_patched_nor_spliced_in() {
        // Unreachable from outside the crate — `flatten` always places at least the root —
        // but both ends of the rebuild reach for node 0, and neither may do so blindly on
        // a tree the user is browsing.
        let empty = Tree {
            nodes: Vec::new(),
            errors: Vec::new(),
        };
        let patched = replace_subtrees(&empty, vec![(0, None)]);
        assert!(patched.is_empty());
        assert_eq!(empty.find(Path::new("/root")), None);

        let tree = nested();
        let patched = replace_subtrees(&tree, vec![(find(&tree, "/root/a"), Some(empty))]);
        assert_invariants(&patched);
        assert_eq!(
            patched.find(Path::new("/root/a")),
            None,
            "nothing to splice"
        );
        assert_eq!(totals_of(&patched, "/root"), (100, 80, 1));
    }
}
