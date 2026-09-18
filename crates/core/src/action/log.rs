//! The append-only record of everything the app deleted.
//!
//! One JSON object per line, because of the two things this file has to survive: a write
//! interrupted halfway, and a reader that only wants the end. A line-based format loses at
//! most the line that was being written, and [`ActionLog::tail`] can drop what it cannot
//! parse instead of failing the whole read — the Activity screen is the only place a user
//! ever sees what was deleted, and a damaged line must not be able to close it.
//!
//! An [`Outcome`] is the only input, so every line of a batch carries the same `at` — the
//! instant the batch began, not the instant of that entry — and the same [`Mode`]. "Newest
//! first" therefore rests on the order of the lines in the file, which is the order the
//! batches were appended, and never on `at`.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scan::NodeKind;

use super::model::{BlockReason, EntryOutcome, EntryResult, Mode, Outcome};

/// What became of one entry, as the log says it: the verdict alone. What
/// [`EntryResult`] carries beside the verdict is in [`LogEntry::bytes`] and
/// [`LogEntry::detail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogResult {
    Removed,
    Failed,
    Skipped,
}

/// One line: one entry of one batch, readable on its own long after the dialog that
/// produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// When the batch began; shared by every line of it (see [`Outcome::at`]).
    pub at: DateTime<Utc>,
    pub path: String,
    /// What the entry was. For a removed one this is the kind the disk confirmed moments
    /// before the deletion; for a skipped one it can be the plan's unverified claim, as
    /// [`super::PreviewEntry::kind`] explains.
    pub kind: NodeKind,
    /// How the entry left, which is what makes `removed` readable: *moved, recoverable*
    /// under [`Mode::Trash`], *gone* under [`Mode::Permanent`].
    pub mode: Mode,
    pub result: LogResult,
    /// The failure message, or the wire name of the reason a skipped entry was refused
    /// (`"denylisted"`); `None` when the entry was removed and there is nothing to add.
    ///
    /// A failure message already names the path — it comes from `SystemError`'s `Display`
    /// — so a reader that shows `detail` next to `path` shows the path twice.
    pub detail: Option<String>,
    /// Bytes freed, and so 0 for anything that was not removed, because nothing was freed.
    /// Deliberate, and not a measure of the row's worth: a skipped row is a row the user
    /// asked for and did not get, and the Activity screen must not draw it as an empty one.
    pub bytes: u64,
}

impl LogEntry {
    /// The line for one entry of a batch that began at `at` and ran in `mode`.
    fn of(entry: &EntryOutcome, at: DateTime<Utc>, mode: Mode) -> Self {
        let (result, detail, bytes) = match &entry.result {
            EntryResult::Removed { bytes } => (LogResult::Removed, None, *bytes),
            EntryResult::Failed { message } => (LogResult::Failed, Some(message.clone()), 0),
            EntryResult::Skipped { reason } => (LogResult::Skipped, wire_name(*reason), 0),
        };
        Self {
            at,
            // Lossy, and lossless in fact: a path that is not valid UTF-8 never reaches a
            // deletion at all, for the reason the module docs of [`super::model`] give.
            path: entry.path.to_string_lossy().into_owned(),
            kind: entry.kind,
            mode,
            result,
            detail,
            bytes,
        }
    }
}

/// The name a [`BlockReason`] has on the wire (`"outsideRoots"`), taken from its own
/// `Serialize` so that the log cannot drift from the vocabulary the UI already maps. The
/// `None` is unreachable — every variant is a plain unit variant, which serializes as a
/// string — and is also exactly what the field should hold if it ever happened.
fn wire_name(reason: BlockReason) -> Option<String> {
    match serde_json::to_value(reason) {
        Ok(serde_json::Value::String(name)) => Some(name),
        _ => None,
    }
}

/// The JSONL log at `path`.
///
/// What is guaranteed when several writers share one file: a batch is a single `write_all`
/// into a file opened with `O_APPEND`, so the kernel places it at the end of the file as it
/// stands at that moment — two appends cannot interleave their lines, and neither can
/// overwrite the other. The file is opened and closed per call; nothing is held between
/// them, so a reader and a writer in one process need no coordination either.
///
/// What is not: that a [`tail`](ActionLog::tail) racing an [`append`](ActionLog::append)
/// sees a batch at all — it sees all of it or none of it — nor that the kernel never splits
/// a batch far larger than a page. A reader that catches half a line drops it like any
/// other damaged line rather than failing. Durability is the kernel's: `append` returns
/// before the bytes reach the disk.
///
/// Nothing here prunes the file — it is the record, and it grows by one line per entry the
/// app was asked to delete. [`ActionLog::tail`] reads all of it to return the end.
#[derive(Debug, Clone)]
pub struct ActionLog {
    path: PathBuf,
}

impl ActionLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Appends one line per entry of `outcome`, creating the file and the directory holding
    /// it if they are not there yet. A batch with no entries writes nothing and creates
    /// nothing: nothing happened.
    pub fn append(&self, outcome: &Outcome) -> io::Result<()> {
        if outcome.entries.is_empty() {
            return Ok(());
        }
        let mut lines = String::new();
        for entry in &outcome.entries {
            let line = LogEntry::of(entry, outcome.at, outcome.mode);
            // Unreachable for these fields, and deliberately not an `unwrap`: a line that
            // cannot be serialized must not take the process down right after a deletion.
            lines.push_str(&serde_json::to_string(&line).map_err(io::Error::other)?);
            lines.push('\n');
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        // One write for the whole batch: what keeps it together in the file.
        file.write_all(lines.as_bytes())
    }

    /// The last `limit` entries, newest first: the end of the file, reversed.
    ///
    /// Lines that do not parse — a torn write, a hand-edited file — are skipped and do not
    /// use up a slot of `limit`. A file that is not there reads as no entries; anything
    /// else that stops the read is returned as the error it was.
    pub fn tail(&self, limit: usize) -> io::Result<Vec<LogEntry>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        // Lossy rather than `read_to_string`: a write torn in the middle of a multi-byte
        // character would otherwise cost the whole read instead of one line. Borrowed, and
        // so free, for every file this writes itself.
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str(line).ok())
            .take(limit)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;

    fn outcome(path: &str, bytes: u64) -> EntryOutcome {
        EntryOutcome {
            path: path.into(),
            kind: NodeKind::Dir,
            result: EntryResult::Removed { bytes },
        }
    }

    /// The [`Outcome`] the engine would have built around `entries`: `freed_bytes` is the
    /// sum over the removed ones and nothing else.
    fn batch(mode: Mode, at: DateTime<Utc>, entries: Vec<EntryOutcome>) -> Outcome {
        let freed_bytes = entries.iter().fold(0u64, |sum, entry| match entry.result {
            EntryResult::Removed { bytes } => sum.saturating_add(bytes),
            _ => sum,
        });
        Outcome {
            entries,
            freed_bytes,
            at,
            mode,
        }
    }

    /// The file as it stands, one string per line, without parsing anything.
    fn raw_lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Appends `bytes` to the file exactly as given, as another writer would.
    fn append_raw(path: &std::path::Path, bytes: &[u8]) {
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    #[test]
    fn entries_are_appended_and_read_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let at = chrono::Utc::now();
        log.append(&batch(Mode::Trash, at, vec![outcome("/h/a", 10)]))
            .unwrap();
        log.append(&batch(
            Mode::Permanent,
            at,
            vec![outcome("/h/b", 20), outcome("/h/c", 30)],
        ))
        .unwrap();
        let entries = log.tail(10).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "/h/c");
        assert_eq!(entries[0].mode, Mode::Permanent);
        assert_eq!(entries[2].path, "/h/a");
        assert_eq!(entries[2].mode, Mode::Trash);
    }

    #[test]
    fn tail_returns_at_most_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        for i in 0..5 {
            log.append(&batch(
                Mode::Trash,
                chrono::Utc::now(),
                vec![outcome(&format!("/h/{i}"), 1)],
            ))
            .unwrap();
        }
        let entries = log.tail(2).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "/h/4");
    }

    #[test]
    fn a_damaged_line_is_skipped_instead_of_failing_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/a", 1)],
        ))
        .unwrap();
        std::fs::write(
            &path,
            format!("{{ truncated\n{}", std::fs::read_to_string(&path).unwrap()),
        )
        .unwrap();
        assert_eq!(log.tail(10).unwrap().len(), 1);
    }

    #[test]
    fn a_missing_log_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("nothing.jsonl"));
        assert!(log.tail(10).unwrap().is_empty());
    }

    #[test]
    fn failures_are_logged_too() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let entry = EntryOutcome {
            path: "/h/x".into(),
            kind: NodeKind::File,
            result: EntryResult::Failed {
                message: "permission denied".into(),
            },
        };
        log.append(&batch(Mode::Trash, chrono::Utc::now(), vec![entry]))
            .unwrap();
        let read = &log.tail(1).unwrap()[0];
        assert_eq!(read.bytes, 0);
        assert!(matches!(read.result, LogResult::Failed));
        assert_eq!(read.detail.as_deref(), Some("permission denied"));
    }

    #[test]
    fn a_line_is_one_camel_case_json_object_with_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(
            Mode::Permanent,
            DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            vec![EntryOutcome {
                path: "/h/a.bin".into(),
                kind: NodeKind::File,
                result: EntryResult::Removed { bytes: 10 },
            }],
        ))
        .unwrap();
        let lines = raw_lines(&path);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&lines[0]).unwrap(),
            serde_json::json!({
                "at": "2023-11-14T22:13:20Z",
                "path": "/h/a.bin",
                "kind": "file",
                "mode": "permanent",
                // A bare string, not the tagged shape of `EntryResult`: the payload it
                // carries lives in `bytes` and `detail`.
                "result": "removed",
                // Present and null rather than absent: the UI reads `string | null`.
                "detail": null,
                "bytes": 10,
            }),
            "the shape the Activity screen is written against"
        );
        assert!(
            std::fs::read_to_string(&path).unwrap().ends_with('\n'),
            "a batch ends its last line, or the next append glues two objects into one"
        );
    }

    #[test]
    fn a_name_with_a_newline_in_it_stays_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        // Legal on every filesystem this runs on, and the one input that could forge a
        // line of the log if it were written as it stands instead of JSON-escaped.
        let named_like_a_line = "/h/evil\n{\"path\":\"/h/forged\",\"bytes\":9000}";
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome(named_like_a_line, 1)],
        ))
        .unwrap();

        assert_eq!(raw_lines(&path).len(), 1, "one entry is one line");
        let read = log.tail(10).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].path, named_like_a_line, "and it comes back whole");
        assert_eq!(read[0].bytes, 1);
    }

    #[test]
    fn a_batch_logs_each_result_with_its_own_kind_bytes_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let entries = vec![
            EntryOutcome {
                path: "/h/gone".into(),
                kind: NodeKind::Dir,
                result: EntryResult::Removed { bytes: 4_096 },
            },
            EntryOutcome {
                path: "/h/stuck".into(),
                kind: NodeKind::File,
                result: EntryResult::Failed {
                    message: "cannot delete /h/stuck: permission denied".into(),
                },
            },
            EntryOutcome {
                path: "/h/link".into(),
                kind: NodeKind::Symlink,
                result: EntryResult::Skipped {
                    reason: BlockReason::Denylisted,
                },
            },
        ];
        log.append(&batch(Mode::Trash, chrono::Utc::now(), entries))
            .unwrap();
        let read = log.tail(10).unwrap();
        assert_eq!(read.len(), 3);

        let removed = &read[2];
        assert_eq!(removed.path, "/h/gone");
        assert_eq!(removed.kind, NodeKind::Dir);
        assert_eq!(removed.result, LogResult::Removed);
        assert_eq!(removed.bytes, 4_096);
        assert_eq!(removed.detail, None);

        let failed = &read[1];
        assert_eq!(failed.path, "/h/stuck");
        assert_eq!(failed.kind, NodeKind::File);
        assert_eq!(failed.result, LogResult::Failed);
        assert_eq!(failed.bytes, 0, "nothing was freed");
        assert_eq!(
            failed.detail.as_deref(),
            Some("cannot delete /h/stuck: permission denied")
        );

        let skipped = &read[0];
        assert_eq!(skipped.path, "/h/link");
        assert_eq!(skipped.kind, NodeKind::Symlink);
        assert_eq!(skipped.result, LogResult::Skipped);
        assert_eq!(skipped.bytes, 0, "nothing was freed");
        assert_eq!(
            skipped.detail.as_deref(),
            Some("denylisted"),
            "a skipped row says why, or the Activity screen cannot"
        );
    }

    #[test]
    fn a_skipped_entry_keeps_its_reason_under_the_name_the_ui_knows() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let reasons = [
            BlockReason::OutsideRoots,
            BlockReason::Denylisted,
            BlockReason::Malformed,
            BlockReason::IsRoot,
            BlockReason::Nested,
            BlockReason::Missing,
            BlockReason::Unreadable,
            BlockReason::KindChanged,
        ];
        let entries = reasons
            .iter()
            .map(|reason| EntryOutcome {
                path: format!("/h/{reason:?}").into(),
                kind: NodeKind::File,
                result: EntryResult::Skipped { reason: *reason },
            })
            .collect();
        log.append(&batch(Mode::Trash, chrono::Utc::now(), entries))
            .unwrap();

        let mut read = log.tail(reasons.len()).unwrap();
        read.reverse();
        assert_eq!(read.len(), reasons.len());
        for (entry, reason) in read.iter().zip(reasons) {
            assert_eq!(
                entry.detail,
                serde_json::to_value(reason)
                    .unwrap()
                    .as_str()
                    .map(str::to_owned),
                "every reason logs the name it has on the wire"
            );
        }
        assert_eq!(read[0].detail.as_deref(), Some("outsideRoots"));
        assert_eq!(read[7].detail.as_deref(), Some("kindChanged"));
    }

    #[test]
    fn the_order_in_the_file_wins_over_the_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let later = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let earlier = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        log.append(&batch(
            Mode::Trash,
            later,
            vec![outcome("/h/written-first", 1)],
        ))
        .unwrap();
        log.append(&batch(
            Mode::Trash,
            earlier,
            vec![outcome("/h/written-second", 2)],
        ))
        .unwrap();

        let read = log.tail(10).unwrap();
        assert_eq!(
            read[0].path, "/h/written-second",
            "newest is the last line written, not the largest `at`"
        );
        assert_eq!(read[1].path, "/h/written-first");
        assert_eq!(
            log.tail(1).unwrap()[0].path,
            "/h/written-second",
            "and the limit keeps the end of the file, not the latest `at`"
        );
    }

    #[test]
    fn the_batch_timestamp_reaches_every_line_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let at = chrono::Utc::now();
        log.append(&batch(
            Mode::Trash,
            at,
            vec![outcome("/h/a", 1), outcome("/h/b", 2)],
        ))
        .unwrap();
        let read = log.tail(10).unwrap();
        assert_eq!(read[0].at, at, "to the nanosecond");
        assert_eq!(read[1].at, at, "one instant for the whole batch");
    }

    #[test]
    fn every_entry_of_a_batch_is_written_once_and_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        let entries = (0..4).map(|i| outcome(&format!("/h/{i}"), 10)).collect();
        log.append(&batch(Mode::Trash, chrono::Utc::now(), entries))
            .unwrap();

        assert_eq!(
            raw_lines(&path).len(),
            4,
            "one line per entry, the last one included"
        );
        let read = log.tail(10).unwrap();
        assert_eq!(
            read.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["/h/3", "/h/2", "/h/1", "/h/0"]
        );
        assert!(log.tail(0).unwrap().is_empty(), "a limit of none is none");
    }

    #[test]
    fn a_damaged_line_does_not_consume_a_slot_of_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        let append = |name: &str| {
            log.append(&batch(
                Mode::Trash,
                chrono::Utc::now(),
                vec![outcome(name, 1)],
            ))
            .unwrap();
        };
        append("/h/a");
        append_raw(&path, b"} not json at all\n");
        append("/h/b");
        append("/h/c");

        let read = log.tail(3).unwrap();
        assert_eq!(
            read.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["/h/c", "/h/b", "/h/a"],
            "three entries, not two and a hole"
        );
    }

    #[test]
    fn a_line_cut_inside_a_character_costs_that_line_and_no_more() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/café", 1)],
        ))
        .unwrap();
        // What a `tail` racing an `append` can see: a line whose write stopped in the
        // middle of the two bytes of `é`, leaving the file invalid UTF-8.
        let line = raw_lines(&path).pop().unwrap();
        let cut = line.find("caf").unwrap() + "caf".len() + 1;
        append_raw(&path, &line.as_bytes()[..cut]);

        let read = log.tail(10).unwrap();
        assert_eq!(read.len(), 1, "the finished line survives the torn one");
        assert_eq!(read[0].path, "/h/café");
    }

    #[test]
    fn append_creates_the_directory_it_writes_into() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep/deeper/actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/a", 1)],
        ))
        .unwrap();
        assert!(path.is_file());
        assert_eq!(log.tail(10).unwrap().len(), 1);
    }

    #[test]
    fn an_empty_batch_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(Mode::Trash, chrono::Utc::now(), Vec::new()))
            .unwrap();
        assert!(!path.exists(), "nothing happened, so nothing is recorded");
        assert!(log.tail(10).unwrap().is_empty());
    }

    #[test]
    fn concurrent_batches_do_not_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        let (writers, batches, per_batch) = (8usize, 25usize, 40usize);
        std::thread::scope(|scope| {
            for writer in 0..writers {
                let log = &log;
                scope.spawn(move || {
                    for run in 0..batches {
                        let tag = format!("/h/w{writer}/b{run}");
                        let entries = (0..per_batch)
                            .map(|i| outcome(&format!("{tag}/entry-{i:030}"), 1))
                            .collect();
                        log.append(&batch(Mode::Trash, chrono::Utc::now(), entries))
                            .unwrap();
                    }
                });
            }
        });

        let read = log.tail(usize::MAX).unwrap();
        assert_eq!(
            read.len(),
            writers * batches * per_batch,
            "every line of every batch is there and parses"
        );
        // The tag of each line, in file order: a batch that was split around another
        // writer's lines would show its tag in two runs instead of one.
        let mut tags: Vec<&str> = read
            .iter()
            .rev()
            .map(|entry| entry.path.rsplit_once("/entry-").unwrap().0)
            .collect();
        tags.dedup();
        assert_eq!(
            tags.len(),
            writers * batches,
            "each batch is one contiguous run of lines"
        );
    }
}
