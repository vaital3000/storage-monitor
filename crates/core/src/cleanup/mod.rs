//! Cleaning what the modules found (ADR 0008): a batch of requests, planned through the
//! modules and run through the guards of [`crate::action`].
//!
//! The Explorer's engine deletes paths the user picked; this one runs the steps a module
//! planned for the items the user picked. What they share is the part that decides what may
//! be touched at all — `Limits::check`, the stat and the kind before every deletion, the
//! Trash and the permanent removal of the port — and it is the Explorer's code, called from
//! here, not a copy of it.

mod engine;
mod model;

pub use engine::{Held, STEP_TIMEOUT, Screened, execute, preview, reversible, screen};
pub use model::{
    CleanupEntry, CleanupEntryOutcome, CleanupOutcome, CleanupPreview, EffectView, Progress,
    Request, StepView,
};
