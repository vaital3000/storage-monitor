// The mock's half of `crates/core/src/action/log.rs`: the record of what a batch did, and
// the end of it that the Activity screen reads.
//
// The log is an array of JSONL lines rather than of entries, because that is what the file
// is: a torn line, a blank one and a line from a later version all have to read here the way
// they read there, and a test that stages one writes it as text.

import type {
  ActivityEntry,
  CleanupEntryOutcome,
  CleanupOutcome,
  DeletionMode,
  EntryOutcome,
  EntryResult,
  LogSource,
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

/** `verdict` of `log.rs`: an entry's result as the log says it. */
function verdict(result: EntryResult): Pick<ActivityEntry, 'result' | 'detail' | 'bytes'> {
  switch (result.result) {
    case 'removed':
      return { result: 'removed', detail: null, bytes: result.bytes };
    case 'failed':
      return { result: 'failed', detail: result.message, bytes: 0 };
    case 'skipped':
      return { result: 'skipped', detail: result.reason, bytes: 0 };
  }
}

/** `LogEntry::of`: the verdict alone, with what it carried split into `detail` and `bytes`. */
function logEntry(entry: EntryOutcome, outcome: Outcome): ActivityEntry {
  return {
    at: outcome.at,
    path: entry.path,
    kind: entry.kind,
    mode: outcome.mode,
    ...verdict(entry.result),
  };
}

/**
 * `ActionLog::append_cleanup`: one line per entry, in the order of the batch, each in the
 * mode its entry really left in, with its source and the commands that started. The keys go
 * in the order serde writes them, and an empty field is left out as serde leaves it out, so
 * the mock's file reads line for line like the app's.
 */
export function appendCleanupToLog(outcome: CleanupOutcome): void {
  for (const entry of outcome.entries) {
    mockActionLog.push(JSON.stringify(cleanupLine(entry, outcome.at)));
  }
}

/** `LogEntry::of_cleanup`. */
function cleanupLine(entry: CleanupEntryOutcome, at: string): ActivityEntry {
  return {
    at,
    ...(entry.path === null ? {} : { path: entry.path }),
    ...(entry.kind === null ? {} : { kind: entry.kind }),
    mode: entry.mode,
    ...verdict(entry.result),
    source: { module: entry.module, item: entry.item, title: entry.title, action: entry.action },
    ...(entry.commands.length === 0 ? {} : { commands: entry.commands }),
  };
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
 * - a **missing `detail`** is an entry with `detail: null`, and a missing `path`, `kind` or
 *   `source` an entry without one: `Option` fields serde fills in, `null` or absent alike. A
 *   line written by hand for a test — the natural way to get a `failed` row on screen —
 *   does not have to carry them; `commands` is a `Vec`, which takes absence and not `null`;
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
  // `Option` fields take `null` as well as absence; `Vec` fields take absence only.
  const path = entry.path ?? undefined;
  const kind = entry.kind ?? undefined;
  const source = entry.source ?? undefined;
  const complete =
    typeof entry.at === 'string' &&
    isTimestamp(entry.at) &&
    (path === undefined || typeof path === 'string') &&
    (kind === undefined || oneOf(kind, NODE_KINDS)) &&
    oneOf(entry.mode, DELETION_MODES) &&
    oneOf(entry.result, LOG_RESULTS) &&
    (detail === null || typeof detail === 'string') &&
    typeof entry.bytes === 'number' &&
    Number.isInteger(entry.bytes) &&
    entry.bytes >= 0 &&
    (source === undefined || isSource(source)) &&
    (entry.commands === undefined || isCommands(entry.commands));
  if (!complete) {
    return null;
  }
  // Field by field, so that an unknown one is dropped the way serde drops it.
  return {
    at: entry.at as string,
    ...(path === undefined ? {} : { path: path as string }),
    ...(kind === undefined ? {} : { kind: kind as NodeKind }),
    mode: entry.mode as DeletionMode,
    result: entry.result as LogResult,
    detail: detail as string | null,
    bytes: entry.bytes as number,
    ...(source === undefined ? {} : { source: source as LogSource }),
    ...(entry.commands === undefined ? {} : { commands: entry.commands as string[][] }),
  };
}

/** A `Source`: four strings, every one of them required. */
function isSource(value: unknown): boolean {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const source = value as Record<string, unknown>;
  return ['module', 'item', 'title', 'action'].every((key) => typeof source[key] === 'string');
}

/** A `Vec<Vec<String>>`. */
function isCommands(value: unknown): boolean {
  return (
    Array.isArray(value) &&
    value.every((argv) => Array.isArray(argv) && argv.every((word) => typeof word === 'string'))
  );
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

/** An empty log, with nothing a batch of an earlier test wrote. Half of `resetMockActions`. */
export function resetActionLog(): void {
  mockActionLog.length = 0;
}
