//! Deleting: what a caller asks for, what the guards allow, and what happened.

mod engine;
mod guards;
mod model;

pub use engine::{execute, preview};
pub use guards::{Checked, Limits, drop_nested};
pub use model::{
    BlockReason, EntryOutcome, EntryResult, EntryStatus, Mode, Outcome, Plan, PlanEntry, Preview,
    PreviewEntry,
};
