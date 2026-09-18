//! Directory scanning: a parallel walker that produces an arena [`Tree`].

mod progress;
mod tree;
mod walker;

pub use tree::{Node, NodeId, NodeKind, Subtree, Tree};
