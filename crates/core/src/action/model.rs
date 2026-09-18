//! The vocabulary of a deletion: what a caller asks for, what the guards allow, and what
//! actually happened.
//!
//! Every type here crosses IPC, so the wire shapes are part of the contract: camelCase
//! fields, and enums tagged so that TypeScript sees discriminated unions instead of
//! serde's default externally tagged form. The tests at the bottom pin the four shapes the
//! UI is written against.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scan::NodeKind;

/// How an entry leaves the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Move to the Trash; the bytes come back only when the Trash is emptied.
    Trash,
    /// Delete outright.
    Permanent,
}

/// One entry a caller asks to delete, as the scanned tree knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    /// Path as the tree reports it; the guards normalize it before anything is touched.
    pub path: PathBuf,
    pub kind: NodeKind,
    /// Allocated bytes recorded by the scan: what the UI promises to free.
    pub size: u64,
}

/// A batch submitted for deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub entries: Vec<PlanEntry>,
    pub mode: Mode,
}

/// Why an entry will not be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockReason {
    /// Not inside the scan root.
    OutsideRoots,
    /// Inside a place the app never deletes from.
    Denylisted,
    /// The scan root itself, or one of its ancestors.
    IsRoot,
    /// Another entry of the same batch contains it.
    Nested,
    /// Nothing is there any more.
    Missing,
    /// The entry is no longer what the preview saw.
    KindChanged,
}

/// The verdict of the guards for one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "camelCase")]
pub enum EntryStatus {
    Ready,
    Blocked(BlockReason),
}

/// One checked entry. Blocked entries keep their place in the list so the UI can show them
/// with their reason instead of silently dropping them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEntry {
    /// The normalized path, which is exactly what would be deleted.
    pub path: PathBuf,
    /// The kind the disk reports now, not the one the plan carried.
    pub kind: NodeKind,
    pub size: u64,
    pub status: EntryStatus,
}

/// A checked plan: no side effects, safe to show and to throw away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub entries: Vec<PreviewEntry>,
    /// Sum of `size` over the [`EntryStatus::Ready`] entries only.
    pub total_bytes: u64,
    pub mode: Mode,
}

/// What became of one entry.
///
/// Every variant carries its payload in a named field rather than as a tuple: serde's
/// internal tagging, which is what gives TypeScript the discriminated union it wants,
/// refuses a newtype variant holding anything but a map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum EntryResult {
    /// Gone, and `bytes` freed (or, in [`Mode::Trash`], freed once the Trash is emptied).
    Removed { bytes: u64 },
    /// The deletion was attempted and failed; `message` is for the user.
    Failed { message: String },
    /// Never touched: the guards refused it, here or at execution time.
    Skipped { reason: BlockReason },
}

/// One line of the action log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryOutcome {
    pub path: PathBuf,
    pub kind: NodeKind,
    pub result: EntryResult,
}

/// What a whole batch did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub entries: Vec<EntryOutcome>,
    /// Bytes of the entries that were really removed. In [`Mode::Trash`] this is what will
    /// be freed once the Trash is emptied, and the UI says exactly that (ADR 0003).
    pub freed_bytes: u64,
    /// When the batch ran, from `System::now`.
    pub at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_mode_is_a_camel_case_string() {
        assert_eq!(serde_json::to_value(Mode::Trash).unwrap(), json!("trash"));
        assert_eq!(
            serde_json::to_value(Mode::Permanent).unwrap(),
            json!("permanent")
        );
    }

    #[test]
    fn a_block_reason_is_a_camel_case_string() {
        assert_eq!(
            serde_json::to_value(BlockReason::KindChanged).unwrap(),
            json!("kindChanged")
        );
        assert_eq!(
            serde_json::to_value(BlockReason::OutsideRoots).unwrap(),
            json!("outsideRoots")
        );
    }

    #[test]
    fn an_entry_status_is_a_discriminated_union() {
        assert_eq!(
            serde_json::to_value(EntryStatus::Ready).unwrap(),
            json!({ "state": "ready" })
        );
        assert_eq!(
            serde_json::to_value(EntryStatus::Blocked(BlockReason::Missing)).unwrap(),
            json!({ "state": "blocked", "reason": "missing" })
        );
    }

    #[test]
    fn an_entry_result_is_a_discriminated_union() {
        assert_eq!(
            serde_json::to_value(EntryResult::Removed { bytes: 10 }).unwrap(),
            json!({ "result": "removed", "bytes": 10 })
        );
        assert_eq!(
            serde_json::to_value(EntryResult::Failed {
                message: "boom".to_owned()
            })
            .unwrap(),
            json!({ "result": "failed", "message": "boom" })
        );
        assert_eq!(
            serde_json::to_value(EntryResult::Skipped {
                reason: BlockReason::Nested
            })
            .unwrap(),
            json!({ "result": "skipped", "reason": "nested" })
        );
    }

    #[test]
    fn a_preview_crosses_the_wire_in_camel_case_and_comes_back() {
        let preview = Preview {
            entries: vec![PreviewEntry {
                path: PathBuf::from("/h/a.bin"),
                kind: NodeKind::File,
                size: 10,
                status: EntryStatus::Ready,
            }],
            total_bytes: 10,
            mode: Mode::Trash,
        };
        let json = serde_json::to_value(&preview).unwrap();
        assert_eq!(
            json,
            json!({
                "entries": [{
                    "path": "/h/a.bin",
                    "kind": "file",
                    "size": 10,
                    "status": { "state": "ready" },
                }],
                "totalBytes": 10,
                "mode": "trash",
            })
        );
        assert_eq!(serde_json::from_value::<Preview>(json).unwrap(), preview);
    }

    #[test]
    fn an_outcome_crosses_the_wire_in_camel_case_and_comes_back() {
        let outcome = Outcome {
            entries: vec![EntryOutcome {
                path: PathBuf::from("/h/a.bin"),
                kind: NodeKind::File,
                result: EntryResult::Removed { bytes: 10 },
            }],
            freed_bytes: 10,
            at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(
            json,
            json!({
                "entries": [{
                    "path": "/h/a.bin",
                    "kind": "file",
                    "result": { "result": "removed", "bytes": 10 },
                }],
                "freedBytes": 10,
                "at": "2023-11-14T22:13:20Z",
            })
        );
        assert_eq!(serde_json::from_value::<Outcome>(json).unwrap(), outcome);
    }

    #[test]
    fn a_plan_is_read_from_camel_case_json() {
        // The one type that travels the other way: the UI submits it.
        let plan: Plan = serde_json::from_value(json!({
            "entries": [{ "path": "/h/a.bin", "kind": "dir", "size": 10 }],
            "mode": "permanent",
        }))
        .unwrap();
        assert_eq!(
            plan,
            Plan {
                entries: vec![PlanEntry {
                    path: PathBuf::from("/h/a.bin"),
                    kind: NodeKind::Dir,
                    size: 10,
                }],
                mode: Mode::Permanent,
            }
        );
    }
}
