//! Persisted scan snapshots and the deltas between them.

mod delta;
mod model;
mod store;

pub use delta::{Delta, deltas, top_growers};
pub use model::{CodecError, SNAPSHOT_FORMAT, Snapshot, SnapshotEntry};
pub use store::{SnapshotMeta, SnapshotStore, StoreError};
