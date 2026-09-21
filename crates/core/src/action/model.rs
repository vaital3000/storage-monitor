//! The vocabulary of a deletion: what a caller asks for, what the guards allow, and what
//! actually happened.
//!
//! Every type here crosses IPC, so the wire shapes are part of the contract: camelCase
//! fields, and enums tagged so that TypeScript sees discriminated unions instead of
//! serde's default externally tagged form. The tests at the bottom pin the four shapes the
//! UI is written against.
//!
//! Paths stay `PathBuf`, which serde refuses to serialize when it is not valid UTF-8. That
//! cannot be reached from here: a name that is not UTF-8 arrives in the tree with U+FFFD
//! in it, so it matches nothing on disk and cannot be deleted at all — the design's second
//! known limitation in section 6.

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
    /// The entry is a folder that may not be deleted as a whole, though what is inside it
    /// may. `~/Library` and the three folders of application data under it: losing any of
    /// them in one tick is expensive, and cleaning them one entry at a time is the point.
    ///
    /// The only reason here that leaves the user somewhere to go — the other eight say no,
    /// this one says *not like this* — which is why it is worth telling apart from
    /// [`Self::Denylisted`] rather than folding into it.
    Shielded,
    /// The path does not name an entry at all: `/`, or a path ending in `..`.
    Malformed,
    /// The scan root itself, or one of its ancestors.
    IsRoot,
    /// Another entry of the same batch contains it.
    Nested,
    /// Nothing is there any more.
    Missing,
    /// Something on the way to it cannot be read — permissions, a symlink loop, a
    /// component that is not a directory. Distinct from [`Self::Missing`] on purpose: the
    /// entry may well be there, and telling the user it vanished sends them hunting for a
    /// ghost instead of granting access.
    Unreadable,
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
    /// What the entry is — and which of two different things this is depends on the status:
    ///
    /// - [`EntryStatus::Ready`], and [`BlockReason::Nested`] which was ready a moment
    ///   earlier: the kind the disk reported during the preview, through the same
    ///   classifier the scan uses. The re-validation before the deletion compares against
    ///   this one, so it has to come from the disk.
    /// - every other [`BlockReason`]: the plan's claim, unverified, because nothing could
    ///   be looked at. Deliberately not an `Option` — a blocked row is still drawn, and its
    ///   icon comes from here. The status is what says which of the two a reader is holding.
    pub kind: NodeKind,
    /// The size the plan carried, from the scan — deliberately not re-read while the kind
    /// is. Re-reading it would mean walking the subtree of every selected directory just
    /// to show a dialog, and the number the user is about to confirm is the one the
    /// Explorer showed them. A stale size costs nothing; a stale kind deletes the wrong
    /// thing, which is why only that one is checked again.
    pub size: u64,
    pub status: EntryStatus,
}

/// A checked plan: no side effects, safe to show and to throw away.
///
/// Trustworthy only as long as it stays in the process that built it. One that arrives
/// from the wire is a claim, not a verdict: the command re-plans from the paths rather
/// than believing the statuses it is handed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub entries: Vec<PreviewEntry>,
    /// Sum of `size` over the [`EntryStatus::Ready`] entries only.
    ///
    /// Honest because of where the sizes come from: a scan attributes hard-linked data to
    /// one path and reports 0 for the other links, so summing two selected entries cannot
    /// count the same blocks twice. A plan built from somewhere else — a phase 3 module
    /// with its own sizes — has to keep that property, or this number over-promises what
    /// the disk will give back.
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
    ///
    /// A lower bound in one direction and an upper bound in the other, both by design. A
    /// tree that was part-deleted before something failed contributes nothing, since
    /// [`EntryResult::Failed`] carries no bytes. And a scan attributes hard-linked data to
    /// the lexicographically smallest path: deleting that one reports its full size here
    /// while the data lives on under the other links, so the volume gives back less than
    /// this number says. [`Preview::total_bytes`] promises the same figure beforehand; this
    /// is the one the user reads afterwards, which is the worse place to be surprised.
    pub freed_bytes: u64,
    /// When the batch ran, from `System::now`.
    pub at: DateTime<Utc>,
    /// How the entries left, mirroring [`Preview::mode`]. Without it [`EntryResult::Removed`]
    /// means two different things — *moved, recoverable, nothing freed yet* in
    /// [`Mode::Trash`], *gone* in [`Mode::Permanent`] — and a line of the action log has to
    /// be readable on its own, long after the dialog that produced it.
    pub mode: Mode,
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
        assert_eq!(
            serde_json::to_value(BlockReason::Unreadable).unwrap(),
            json!("unreadable")
        );
        assert_eq!(
            serde_json::to_value(BlockReason::Malformed).unwrap(),
            json!("malformed")
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
            mode: Mode::Permanent,
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
                // Not `trash`: a log line that cannot say whether the bytes are gone or
                // merely moved is a log line nobody can read.
                "mode": "permanent",
            })
        );
        assert_eq!(serde_json::from_value::<Outcome>(json).unwrap(), outcome);
    }

    #[test]
    fn a_plan_is_read_from_camel_case_json() {
        // `Plan` does not cross IPC — the commands take the paths and a mode and build it
        // in the backend. Its shape is pinned anyway: it is the vocabulary the UI mirrors,
        // and fixtures are written in this form.
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
