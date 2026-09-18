//! Deleting: what a caller asks for, what the guards allow, and what happened.

mod guards;
mod model;

pub use guards::{Limits, check, drop_nested};
pub use model::{
    BlockReason, EntryOutcome, EntryResult, EntryStatus, Mode, Outcome, Plan, PlanEntry, Preview,
    PreviewEntry,
};
