//! Persisted scan snapshots and the deltas between them.

mod delta;
mod model;
mod store;

/// How many snapshots the store keeps by default (across all roots).
pub const DEFAULT_KEEP: usize = 10;
/// Files smaller than this are not recorded in a snapshot by default (10 MiB).
pub const DEFAULT_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;

pub use delta::{Delta, deltas, top_growers};
pub use model::{CodecError, SNAPSHOT_FORMAT, Snapshot, SnapshotEntry};
pub use store::{SnapshotMeta, SnapshotStore, StoreError};
