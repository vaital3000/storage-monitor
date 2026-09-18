//! Directory scanning: a parallel walker that produces an arena [`Tree`].

mod progress;
mod tree;
mod walker;

pub use progress::{ProgressSnapshot, ScanProgress};
pub use tree::{Node, NodeId, NodeKind, Subtree, Tree};
pub use walker::{ScanError, ScanOptions, ScanResult, ScanStats, scan};
