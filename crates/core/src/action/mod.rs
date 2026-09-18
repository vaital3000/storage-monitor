//! Deleting: what a caller asks for, what the guards allow, and what happened.

mod engine;
mod guards;
mod log;
mod model;

pub use engine::{execute, preview};
pub use guards::{Checked, Limits, drop_nested};
// `self::`, because `log` is also the name of a crate this one could grow a dependency on.
pub use self::log::{ActionLog, LogEntry, LogResult};
pub use model::{
    BlockReason, EntryOutcome, EntryResult, EntryStatus, Mode, Outcome, Plan, PlanEntry, Preview,
    PreviewEntry,
};
