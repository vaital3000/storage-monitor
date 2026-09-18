//! The append-only record of everything the app deleted.
//!
//! One JSON object per line, because of the two things this file has to survive: a write
//! that stopped halfway, and a reader that only wants the end. A torn write costs the torn
//! line and nothing else — [`ActionLog::tail`] drops what it cannot parse instead of
//! failing the read, and [`ActionLog::append`] starts a fresh line when it finds the file
//! ending mid-line, so the remainder cannot swallow the batch that follows it. The Activity
//! screen is the only place a user ever sees what was deleted; neither a damaged line nor a
//! full volume may be able to close it or to quietly shorten it.
//!
//! An [`Outcome`] is the only input, so every line of a batch carries the same `at` — the
//! instant the batch began, not the instant of that entry — and the same [`Mode`]. "Newest
//! first" therefore rests on the order of the lines in the file, which is the order the
//! batches were appended, and never on `at`.
//!
//! Nothing is recorded until the batch is over: [`super::execute`] deletes, then this
//! writes. The window is the whole batch and it is not short — a single `remove` of a 50 GB
//! tree is one uninterruptible call — so a crash in the middle leaves deletions nobody can
//! see. Appending each entry as it happens would shrink the window to one entry, at the
//! price of the single-write guarantee below; it was not taken, because the other order is
//! worse in kind rather than in size: a line promising a deletion that then did not happen
//! sends a user hunting for a file that is still there. For the same reason an `Err` from
//! [`ActionLog::append`] is not a cosmetic failure — the caller has deleted something and
//! has no record of it, and must surface that rather than swallow it.
//!
//! Reading the file by hand: [`LogEntry::detail`] says different things depending on
//! [`LogEntry::result`] — a message for `failed`, a reason's wire name for `skipped` — so
//! it means nothing read on its own. And a line is the unit: nothing ties the lines of one
//! batch together, and nothing in phase 2a needs it to. When something does (an Activity
//! screen that wants *this action freed 4.2 GB across 37 entries*), it arrives as a new
//! field carrying `#[serde(default)]`, like every field added from here — see [`LogEntry`].

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

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
///
/// Every field added from here carries `#[serde(default)]`. The derive requires each field
/// it cannot default, and one required field that the lines already on disk do not have
/// makes the whole history unreadable at a stroke — no error, no count, just an Activity
/// screen that has forgotten everything the user deleted before the update.
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
    /// Whatever there is to add to [`LogEntry::result`], and meaningless without it: the
    /// failure message under [`LogResult::Failed`], the wire name of the reason
    /// (`"denylisted"`) under [`LogResult::Skipped`], `None` under [`LogResult::Removed`].
    /// The two kinds of string are not distinguishable from each other — a failure whose
    /// message happens to read `denylisted` is not a blocked entry — so nothing may read
    /// this field without reading `result` first.
    ///
    /// A failure message already names the path — it comes from `SystemError`'s `Display`
    /// — so a reader that shows `detail` next to `path` shows the path twice.
    pub detail: Option<String>,
    /// Bytes this entry freed — under [`Mode::Trash`], what it will free once the Trash is
    /// emptied (ADR 0003), which is the same promise [`super::Outcome::freed_bytes`] makes
    /// for the batch. 0 for anything that was not removed, because nothing was freed.
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

/// What [`ActionLog::tail`] found at the end of the log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tail {
    /// Newest first.
    pub entries: Vec<LogEntry>,
    /// How many lines of the stretch that was read could not be parsed, and are therefore
    /// missing from `entries`. A screen that shows the entries should say this number too:
    /// showing fewer rows than the user remembers deleting, with nothing to explain the
    /// gap, is the one thing a record of deletions must never do.
    pub damaged: usize,
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
/// sees a batch at all — it sees all of it or none of it — nor that a batch always reaches
/// the file in one piece. `write_all` loops, and two things split it: a buffer larger than
/// one `write(2)` takes (std caps that at `c_int::MAX`, about 2 GiB on Apple targets, which
/// no batch of ours comes near) and a short write on a volume that has just filled up,
/// which is the one to plan for — this app exists because the volume is nearly full. Both
/// leave a torn line and an `Err`, and [`ActionLog::append`] repairs the file before it
/// writes again, so the damage stays the line it happened to.
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

    /// The file this log is kept in, which may not exist yet.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one line per entry of `outcome`, creating the directory holding the file and
    /// the file itself — mode `0o600`, since it names every path the user has ever deleted.
    /// A batch with no entries writes nothing and creates nothing: nothing happened.
    ///
    /// When the file does not end in a newline, because someone's write was cut short, the
    /// batch begins with one. Without that, the torn remainder and this batch's first
    /// object share a line, and a reader loses both instead of one.
    ///
    /// `Ok` means the lines are on the volume: one `write_all`, then `sync_all`. The
    /// directory entry is not synced, so a crash immediately after the first batch ever
    /// written can still cost the file itself rather than its tail. `Err` means some prefix
    /// of the batch may be in the file already — what a later reader loses is the torn line
    /// — and that the caller has deleted something it has not recorded.
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
            .read(true)
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        if ends_mid_line(&mut file)? {
            lines.insert(0, '\n');
        }
        // One write for the whole batch: what keeps it together in the file.
        file.write_all(lines.as_bytes())?;
        file.sync_all()
    }

    /// The last `limit` entries, newest first — the end of the file, reversed — and how
    /// many lines in that stretch could not be read.
    ///
    /// A line that does not parse costs one `damaged` and nothing else: it does not use up
    /// a slot of `limit`, and it does not stop the read. An empty line is a separator, not
    /// damage. `damaged` counts only as far back as the read went, which is as far as
    /// `limit` entries reach. A file that is not there reads as nothing at all; any other
    /// error that stops the read is returned as the error it was.
    pub fn tail(&self, limit: usize) -> io::Result<Tail> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Tail::default()),
            Err(err) => return Err(err),
        };
        // Lossy rather than `read_to_string`: a write torn in the middle of a multi-byte
        // character would otherwise cost the whole read instead of one line. Borrowed, and
        // so free, for every file this writes itself.
        let text = String::from_utf8_lossy(&bytes);
        let mut tail = Tail::default();
        for line in text.lines().rev() {
            if tail.entries.len() == limit {
                break;
            }
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str(line) {
                Ok(entry) => tail.entries.push(entry),
                Err(_) => tail.damaged += 1,
            }
        }
        Ok(tail)
    }
}

/// Whether the file ends in the middle of a line, which is what a write cut short leaves
/// behind. Asked through the handle that is about to append, so it describes the file as it
/// is now; under `O_APPEND` that answer can only go stale in the harmless direction, since
/// another writer can only add whole batches and the cost is one blank line.
fn ends_mid_line(file: &mut File) -> io::Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
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
        let entries = log.tail(10).unwrap().entries;
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
        let entries = log.tail(2).unwrap().entries;
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
        assert_eq!(log.tail(10).unwrap().entries.len(), 1);
    }

    #[test]
    fn a_missing_log_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("nothing.jsonl"));
        assert!(log.tail(10).unwrap().entries.is_empty());
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
        let read = &log.tail(1).unwrap().entries[0];
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
        // Legal on every filesystem this runs on, and the one input that could forge a line
        // of the log if it were written as it stands instead of JSON-escaped. The payload
        // is a whole valid entry, so a forged line would be read back as one: both
        // assertions below have to hold, not just the count.
        let forged = serde_json::to_string(&LogEntry {
            at: chrono::Utc::now(),
            path: "/h/forged".to_owned(),
            kind: NodeKind::Dir,
            mode: Mode::Permanent,
            result: LogResult::Removed,
            detail: None,
            bytes: 9000,
        })
        .unwrap();
        let named_like_a_line = format!("/h/evil\n{forged}");
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome(&named_like_a_line, 1)],
        ))
        .unwrap();

        assert_eq!(raw_lines(&path).len(), 1, "one entry is one line");
        let read = log.tail(10).unwrap();
        assert_eq!(read.entries.len(), 1, "and no forged second one");
        assert_eq!(
            read.entries[0].path, named_like_a_line,
            "and it comes back whole"
        );
        assert_eq!(read.entries[0].bytes, 1);
        assert_eq!(read.damaged, 0);
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
        let read = log.tail(10).unwrap().entries;
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
        // Spelled out rather than derived from `BlockReason`'s own `Serialize`: these eight
        // strings are the vocabulary the Activity screen will map, so this has to fail when
        // they change — including on the day the enum stops serializing as a string at all.
        let reasons = [
            (BlockReason::OutsideRoots, "outsideRoots"),
            (BlockReason::Denylisted, "denylisted"),
            (BlockReason::Malformed, "malformed"),
            (BlockReason::IsRoot, "isRoot"),
            (BlockReason::Nested, "nested"),
            (BlockReason::Missing, "missing"),
            (BlockReason::Unreadable, "unreadable"),
            (BlockReason::KindChanged, "kindChanged"),
        ];
        let entries = reasons
            .iter()
            .map(|(reason, name)| EntryOutcome {
                path: format!("/h/{name}").into(),
                kind: NodeKind::File,
                result: EntryResult::Skipped { reason: *reason },
            })
            .collect();
        log.append(&batch(Mode::Trash, chrono::Utc::now(), entries))
            .unwrap();

        let mut read = log.tail(reasons.len()).unwrap().entries;
        read.reverse();
        assert_eq!(read.len(), reasons.len());
        for (entry, (_, name)) in read.iter().zip(reasons) {
            assert_eq!(entry.path, format!("/h/{name}"), "in the order written");
            assert_eq!(
                entry.detail.as_deref(),
                Some(name),
                "every reason logs the name the UI knows it by"
            );
        }
    }

    #[test]
    fn the_order_in_the_file_wins_over_the_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
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

        let read = log.tail(10).unwrap().entries;
        assert_eq!(
            read[0].path, "/h/written-second",
            "newest is the last line written, not the largest `at`"
        );
        assert_eq!(read[1].path, "/h/written-first");
        assert_eq!(
            log.tail(1).unwrap().entries[0].path,
            "/h/written-second",
            "and the limit keeps the end of the file, not the latest `at`"
        );
        assert_eq!(
            raw_lines(&path).len(),
            2,
            "two batches, two lines: a batch that lands on a terminated line adds no \
             separator of its own"
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
        let read = log.tail(10).unwrap().entries;
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
        let read = log.tail(10).unwrap().entries;
        assert_eq!(
            read.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["/h/3", "/h/2", "/h/1", "/h/0"]
        );
        assert!(
            log.tail(0).unwrap().entries.is_empty(),
            "a limit of none is none"
        );
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
        // A blank line is a separator someone's editor left behind, not a lost entry.
        append_raw(&path, b"\n");
        append("/h/c");

        let read = log.tail(3).unwrap();
        assert_eq!(
            read.entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["/h/c", "/h/b", "/h/a"],
            "three entries, not two and a hole"
        );
        assert_eq!(read.damaged, 1, "and the hole is counted, not hidden");
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
        assert_eq!(
            read.entries.len(),
            1,
            "the finished line survives the torn one"
        );
        assert_eq!(read.entries[0].path, "/h/café");
        assert_eq!(read.damaged, 1);

        // And the batch that comes next is not dragged down with it: the torn bytes have
        // no newline of their own, so the next line has to start one.
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/after-1", 1), outcome("/h/after-2", 2)],
        ))
        .unwrap();
        let read = log.tail(10).unwrap();
        assert_eq!(
            read.entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["/h/after-2", "/h/after-1", "/h/café"],
            "every entry of the new batch, and the one from before the tear"
        );
        assert_eq!(read.damaged, 1, "still the one torn line, and only it");
    }

    #[test]
    fn a_last_line_left_unterminated_is_data_and_not_damage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/a", 1)],
        ))
        .unwrap();
        // A whole line whose trailing newline never made it — or a file someone edited by
        // hand. Nothing is lost here, as long as the next batch does not land on its end.
        let whole = raw_lines(&path).pop().unwrap();
        append_raw(&path, whole.replace("/h/a", "/h/b").as_bytes());

        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/c", 3)],
        ))
        .unwrap();
        let read = log.tail(10).unwrap();
        assert_eq!(
            read.entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["/h/c", "/h/b", "/h/a"]
        );
        assert_eq!(read.damaged, 0, "nothing here was damaged");
    }

    #[test]
    fn a_file_that_ends_mid_line_does_not_swallow_the_next_batch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/before", 1)],
        ))
        .unwrap();
        // A write that stopped halfway: the volume filled up under it, or the process was
        // killed between the two `write` calls `write_all` makes.
        let torn = raw_lines(&path).pop().unwrap();
        append_raw(&path, &torn.as_bytes()[..torn.len() / 2]);

        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/a", 1), outcome("/h/b", 2), outcome("/h/c", 3)],
        ))
        .unwrap();

        let read = log.tail(10).unwrap();
        assert_eq!(
            read.entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["/h/c", "/h/b", "/h/a", "/h/before"],
            "the torn line is the only loss"
        );
        assert_eq!(read.damaged, 1);
    }

    #[test]
    fn append_creates_the_directory_it_writes_into() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep/deeper/actions.jsonl");
        let log = ActionLog::new(path.clone());
        assert_eq!(log.path(), path, "before it exists, too");
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/a", 1)],
        ))
        .unwrap();
        assert!(path.is_file());
        assert_eq!(log.tail(10).unwrap().entries.len(), 1);
    }

    #[test]
    fn the_file_is_readable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(
            Mode::Trash,
            chrono::Utc::now(),
            vec![outcome("/h/a", 1)],
        ))
        .unwrap();

        // It names every path the user has ever deleted. `0o600` is what `append` asks for;
        // the umask can only take bits away, so this is the part that cannot drift.
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o077,
            0,
            "no group or other access, got {:o}",
            mode & 0o777
        );
    }

    #[test]
    fn an_empty_batch_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(Mode::Trash, chrono::Utc::now(), Vec::new()))
            .unwrap();
        assert!(!path.exists(), "nothing happened, so nothing is recorded");
        assert!(log.tail(10).unwrap().entries.is_empty());
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
            read.entries.len(),
            writers * batches * per_batch,
            "every line of every batch is there and parses"
        );
        assert_eq!(read.damaged, 0, "and none of them was torn by another");
        // The tag of each line, in file order: a batch that was split around another
        // writer's lines would show its tag in two runs instead of one.
        let mut tags: Vec<&str> = read
            .entries
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
