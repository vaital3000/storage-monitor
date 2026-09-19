// The mock's half of `crates/core/src/action/log.rs`: the record of what a batch did, and
// the end of it that the Activity screen reads.
//
// The log is an array of JSONL lines rather than of entries, because that is what the file
// is: a torn line, a blank one and a line from a later version all have to read here the way
// they read there, and a test that stages one writes it as text.

import type {
  ActivityEntry,
  DeletionMode,
  EntryOutcome,
  LogTail,
  NodeKind,
  Outcome,
} from '../lib/ipc';
import { DELETION_MODES, LOG_RESULTS, NODE_KINDS, type LogResult } from '../lib/ipc';

/** The action log, oldest line first: the JSONL file the backend appends to, as an array. */
export const mockActionLog: string[] = [];

/** `ActionLog::append`: one line per entry, in the order of the batch. */
export function appendToLog(outcome: Outcome): void {
  for (const entry of outcome.entries) {
    mockActionLog.push(JSON.stringify(logEntry(entry, outcome)));
  }
}

/** `LogEntry::of`: the verdict alone, with what it carried split into `detail` and `bytes`. */
function logEntry(entry: EntryOutcome, outcome: Outcome): ActivityEntry {
  const line = { at: outcome.at, path: entry.path, kind: entry.kind, mode: outcome.mode };
  switch (entry.result.result) {
    case 'removed':
      return { ...line, result: 'removed', detail: null, bytes: entry.result.bytes };
    case 'failed':
      return { ...line, result: 'failed', detail: entry.result.message, bytes: 0 };
    case 'skipped':
      return { ...line, result: 'skipped', detail: entry.result.reason, bytes: 0 };
  }
}

/**
 * One of the values of a wire enum. The lists come from `ipc.ts`, where each is the type's
 * own definition rather than a copy of it: a variant added there reaches this reader, and a
 * variant that only the Rust has does not pass — which is the same answer serde gives.
 */
const oneOf = (field: unknown, names: readonly string[]) =>
  typeof field === 'string' && names.includes(field);

/**
 * `chrono`'s grammar for a `DateTime<Utc>`, which is not the platform's.
 *
 * It takes `T`, `t` or a space between the date and the time, needs `Z`, `z` or an offset —
 * never nothing — and allows any number of fractional digits, a leap second and surrounding
 * space. It refuses a bare date, a time without seconds, and a day the month does not have.
 *
 * `Date.parse` is no substitute, in **both** directions: it reads `2026-09-18` and a stamp
 * with no offset at all, and refuses the leap second and the trailing space that `chrono`
 * reads. Both grammars were measured against the real `LogEntry`, and this one agrees with
 * it on every string that was tried.
 *
 * It matters because the plan sends the next task to hand-write log lines. A line whose `Z`
 * was forgotten has to be damaged here too — otherwise it renders in the mock and vanishes
 * in the app, which is the worst possible way to find out about the difference.
 */
const TIMESTAMP =
  /^\s*(\d{4})-(\d{2})-(\d{2})[Tt ](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:[Zz]|[+-](\d{2}):?(\d{2}))\s*$/;

function isTimestamp(value: string): boolean {
  const parts = TIMESTAMP.exec(value);
  if (parts === null) {
    return false;
  }
  const [year, month, day, hour, minute, second, offsetHour, offsetMinute] = parts
    .slice(1)
    .map((part) => (part === undefined ? 0 : Number(part)));
  // A leap second is a second; a day the month does not have is not a day, and rolling it
  // through `Date.UTC` is what says so — arithmetic, not another parser's grammar.
  const rolled = new Date(Date.UTC(year, month - 1, day));
  return (
    rolled.getUTCMonth() === month - 1 &&
    rolled.getUTCDate() === day &&
    hour < 24 &&
    minute < 60 &&
    second < 61 &&
    offsetHour < 24 &&
    offsetMinute < 60
  );
}

/**
 * One line of the log, or `null` when it is not an entry — as far as possible the line serde
 * refuses, measured against the real `LogEntry` rather than guessed:
 *
 * - a **missing `detail`** is an entry with `detail: null`. Every other field is required,
 *   but `Option<String>` is one serde fills in, and a line written by hand for a test — the
 *   natural way to get a `failed` row on screen — does not have to carry it;
 * - an unknown field is ignored, so a line from a later version still reads;
 * - `at` has to be a timestamp in `chrono`'s grammar (see [`isTimestamp`], which is where
 *   that grammar is written down); `bytes` a whole number that is not negative, because the
 *   field is a `u64` and serde takes neither `-1` nor `1.5`.
 */
function parseLogLine(line: string): ActivityEntry | null {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    return null;
  }
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const entry = value as Record<string, unknown>;
  const detail = entry.detail ?? null;
  const complete =
    typeof entry.at === 'string' &&
    isTimestamp(entry.at) &&
    typeof entry.path === 'string' &&
    oneOf(entry.kind, NODE_KINDS) &&
    oneOf(entry.mode, DELETION_MODES) &&
    oneOf(entry.result, LOG_RESULTS) &&
    (detail === null || typeof detail === 'string') &&
    typeof entry.bytes === 'number' &&
    Number.isInteger(entry.bytes) &&
    entry.bytes >= 0;
  if (!complete) {
    return null;
  }
  // Field by field, so that an unknown one is dropped the way serde drops it.
  return {
    at: entry.at as string,
    path: entry.path as string,
    kind: entry.kind as NodeKind,
    mode: entry.mode as DeletionMode,
    result: entry.result as LogResult,
    detail: detail as string | null,
    bytes: entry.bytes as number,
  };
}

/**
 * `ActionLog::tail`: the last `limit` entries, newest first, and how many lines of the
 * stretch that was read could not be parsed.
 *
 * A damaged line costs one `damaged` and nothing else: it does not use up a slot of `limit`,
 * and it does not stop the read. A blank line is a separator, not damage. `damaged` counts
 * only as far back as the read went, which is as far as `limit` entries reach.
 */
export function mockActivityTail(limit: number): LogTail {
  const entries: ActivityEntry[] = [];
  let damaged = 0;
  for (let line = mockActionLog.length - 1; line >= 0; line -= 1) {
    if (entries.length === limit) {
      break;
    }
    const text = mockActionLog[line];
    if (text.trim() === '') {
      continue;
    }
    const entry = parseLogLine(text);
    if (entry === null) {
      damaged += 1;
    } else {
      entries.push(entry);
    }
  }
  return { entries, damaged };
}

/**
 * Puts every node of the tree back as it was built and forgets the log. `resetIpcMock` calls
 * it, and so does the test setup, so a batch in one test is never visible in the next.
 */

/** An empty log, with nothing a batch of an earlier test wrote. Half of `resetMockActions`. */
export function resetActionLog(): void {
  mockActionLog.length = 0;
}
