//! The vocabulary of a cleanup batch: what the Cleanup screen asks for, what the engine
//! would do, and what it did.
//!
//! Only [`Request`] is ever read from the wire. Everything else is written for the UI and
//! the record and derives `Serialize` alone: a preview of a cleanup carries commands, and a
//! preview that could be deserialized would be a way to hand the engine a command no module
//! planned. `execute` takes requests for that reason, never a preview.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::action::{EntryResult, EntryStatus, Mode};
use crate::scan::NodeKind;

/// One item the user asked to clean, with the action and the options chosen for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The item's id, `<module>:<native id>`.
    pub item: String,
    /// The id of one of the item's actions.
    pub action: String,
    /// The ids of the options turned on.
    #[serde(default)]
    pub options: Vec<String>,
}

/// One step as the dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "camelCase")]
pub enum StepView {
    /// A `Delete` in Trash mode.
    Trash { path: String },
    /// A `Delete` in Permanent mode.
    Delete { path: String },
    /// A program, by its command line (see `module::command_line`), with what it changes.
    Run {
        command: String,
        effect: EffectView,
        /// The path a `Removes` step deletes.
        path: Option<String>,
    },
}

/// What a `Run` step changes, without the target it may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectView {
    Housekeeping,
    Destroys,
    Removes,
}

/// One request, checked: what would happen to it, and whether it can happen at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupEntry {
    pub item: String,
    /// The module's id; empty for an item that is not held any more.
    pub module: String,
    /// The item's title, or its id when the item is not held any more.
    pub title: String,
    /// The action's label, or its id when the item does not offer it.
    pub action: String,
    /// What would run, in order. Empty when the entry was refused before it was planned.
    pub steps: Vec<StepView>,
    /// The action's `estimated_free`: what the dialog promises.
    pub size: u64,
    pub status: EntryStatus,
    /// Whether the Trash can undo it in this preview's mode: the mode is Trash and every
    /// step is a `Delete` or housekeeping.
    pub reversible: bool,
}

/// A checked cleanup batch in one mode: nothing was touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPreview {
    /// One per request, in the order they came, refused ones included.
    pub entries: Vec<CleanupEntry>,
    /// Sum of `size` over the ready entries.
    pub total_bytes: u64,
    pub mode: Mode,
}

/// How far a batch has got, reported before every entry and once at the end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// The title of the entry starting now; `None` once the batch is over.
    pub current: Option<String>,
}

/// What became of one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupEntryOutcome {
    pub item: String,
    pub module: String,
    pub title: String,
    pub action: String,
    /// The item's path, when it has one and was still held.
    pub path: Option<PathBuf>,
    /// The kind of the target at the item's path, when the plan had one.
    pub kind: Option<NodeKind>,
    /// Every target a step tried to delete, normalized — what the Explorer's tree is
    /// patched by afterwards, whether the step succeeded or not.
    pub targets: Vec<PathBuf>,
    /// How this entry left: the batch's mode when the entry is reversible, `permanent`
    /// otherwise. A line saying "removed, Trash" about a Docker image would promise a
    /// recovery that does not exist.
    pub mode: Mode,
    /// The argv of every `Run` step that started, in order.
    pub commands: Vec<Vec<String>>,
    pub result: EntryResult,
}

/// What a whole cleanup batch did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupOutcome {
    pub entries: Vec<CleanupEntryOutcome>,
    /// Bytes of the removed entries, the sum of their `estimated_free`.
    pub freed_bytes: u64,
    /// When the batch began, from `System::now`; shared by every entry.
    pub at: DateTime<Utc>,
    /// The mode the batch was asked to run in. Each entry says the mode it really left in.
    pub mode: Mode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_reads_without_options() {
        let request: Request =
            serde_json::from_value(json!({ "item": "demo:a", "action": "delete" })).unwrap();
        assert!(request.options.is_empty());
    }

    #[test]
    fn a_step_view_is_a_discriminated_union() {
        assert_eq!(
            serde_json::to_value(StepView::Trash {
                path: "/h/a".to_owned()
            })
            .unwrap(),
            json!({ "step": "trash", "path": "/h/a" })
        );
        assert_eq!(
            serde_json::to_value(StepView::Run {
                command: "rm /h/a".to_owned(),
                effect: EffectView::Removes,
                path: Some("/h/a".to_owned()),
            })
            .unwrap(),
            json!({ "step": "run", "command": "rm /h/a", "effect": "removes", "path": "/h/a" })
        );
    }

    #[test]
    fn a_preview_crosses_the_wire_in_camel_case() {
        let preview = CleanupPreview {
            entries: vec![CleanupEntry {
                item: "demo:a".to_owned(),
                module: "demo".to_owned(),
                title: "a".to_owned(),
                action: "Delete folder".to_owned(),
                steps: vec![StepView::Delete {
                    path: "/h/a".to_owned(),
                }],
                size: 10,
                status: EntryStatus::Ready,
                reversible: false,
            }],
            total_bytes: 10,
            mode: Mode::Permanent,
        };
        assert_eq!(
            serde_json::to_value(preview).unwrap(),
            json!({
                "entries": [{
                    "item": "demo:a",
                    "module": "demo",
                    "title": "a",
                    "action": "Delete folder",
                    "steps": [{ "step": "delete", "path": "/h/a" }],
                    "size": 10,
                    "status": { "state": "ready" },
                    "reversible": false,
                }],
                "totalBytes": 10,
                "mode": "permanent",
            })
        );
    }
}
