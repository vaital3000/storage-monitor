import { useQuery } from '@tanstack/react-query';
import Button from '../components/Button';
import { UNKNOWN_BLOCK_REASON, describeBlock } from '../lib/blockReasons';
import { commandLine } from '../lib/commandLine';
import { countLabel, formatBytes, formatTimestamp } from '../lib/format';
import {
  activityLog,
  logDetail,
  modulesList,
  type ActivityEntry,
  type DeletionMode,
  type LogResult,
} from '../lib/ipc';

/**
 * How much of the record this screen asks for.
 *
 * Nothing prunes `actions.jsonl` — it is the record of every path the user has ever
 * deleted — so the answer is the end of a file that only grows, and the screen says which
 * end it is showing whenever it gets a full `LIMIT` back.
 */
const LIMIT = 200;

/**
 * How the entry left, in the past tense. The dialog has a map of the same two words, and
 * they are not the same decision: there they name a choice being offered, here they name
 * what a line of the record says happened, and a screen that shared them would be pinning
 * the wording of a question to the wording of an answer.
 *
 * A mode this build does not know cannot reach here. `LogEntry::mode` is a Rust enum, so a
 * line carrying one fails to parse in the reader and is counted in `damaged` — unlike
 * `detail`, which is a free string and is why `logDetail` exists.
 */
const MODE_LABELS: Record<DeletionMode, string> = {
  trash: 'Trash',
  permanent: 'Permanent',
};

/**
 * The verdict alone. What it means is carried by the mode beside it, which is the pairing
 * `LogEntry::mode` describes: *moved, recoverable* under the Trash, *gone* under a
 * permanent deletion. `LogEntry::result` is an enum too, so this lookup cannot miss.
 */
const RESULT_LABELS: Record<LogResult, string> = {
  removed: 'Removed',
  failed: 'Failed',
  skipped: 'Skipped',
};

/** The second line of a row: what `detail` adds, and whether it is a failure. */
interface Detail {
  text: string;
  failure: boolean;
}

function describeDetail(entry: ActivityEntry): Detail | null {
  // `detail` holds a failure message under one verdict and a block reason under another,
  // and the two kinds of string cannot be told apart by looking at them — so it is read
  // through `logDetail`, which reads `result` first, and never directly.
  const detail = logDetail(entry);
  if (detail !== null) {
    return detail.kind === 'message'
      ? { text: detail.message, failure: true }
      : // A cleanup line's guards were built from the home folder, not from a scan.
        { text: describeBlock(detail.reason, entry.source ? 'home' : 'scan'), failure: false };
  }
  // `logDetail` also answers null for a `skipped` line whose reason this build has no name
  // for — a variant added to `BlockReason` after this build — and that is the one case
  // where saying nothing would leave a blocked row with no explanation at all. The dialog
  // answers the same sentence for the same reason. A `failed` line with no message keeps
  // its "Failed", which is already the whole of what the record says about it.
  return entry.result === 'skipped' ? { text: UNKNOWN_BLOCK_REASON, failure: false } : null;
}

/**
 * What a row says was freed. Bytes are 0 for everything that was not removed, where a
 * "0 B" beside "Failed" reads as a file that was deleted and happened to be empty.
 *
 * The rule is about the number rather than about the verdict, so a line that is not
 * `removed` and carries bytes anyway — which the backend does not write, and an older or
 * hand-edited log may — cannot be hidden by it.
 */
function describeBytes(entry: ActivityEntry): string | null {
  return entry.result === 'removed' || entry.bytes > 0 ? formatBytes(entry.bytes) : null;
}

/**
 * How the entry left — for the rows where something left at all.
 *
 * A `skipped` entry was refused before anything was touched, so "Trash" beside it would
 * read as a file that is now in the Trash. It is the same trade `describeBytes` makes one
 * function up, and the dialog makes it too: its Skipped list carries no mode either. A
 * `failed` row keeps its mode, because that batch did try, and *how* it tried is half of
 * what its message means.
 */
function describeMode(entry: ActivityEntry): string | null {
  return entry.result === 'skipped' ? null : MODE_LABELS[entry.mode];
}

const CELL = 'px-2 py-1.5 align-top';

/** What a cell shows when the record has nothing to put there — never a zero or a guess. */
const EMPTY_CELL = '—';

function Row({ entry, names }: { entry: ActivityEntry; names: ReadonlyMap<string, string> }) {
  const detail = describeDetail(entry);
  const bytes = describeBytes(entry);
  const mode = describeMode(entry);
  const { source } = entry;
  return (
    <tr data-result={entry.result} className="border-t border-neutral-100 dark:border-neutral-800">
      <td className={`${CELL} whitespace-nowrap text-muted tabular-nums`}>
        {formatTimestamp(entry.at)}
      </td>
      <td className={`max-w-0 ${CELL}`}>
        {source !== undefined && (
          // A cleanup line: the item by its title, and the module and the action it went
          // through, as they were named when the batch ran. A module this build no longer
          // has is named by its id — the line outlives the module.
          <>
            <span className="block truncate" title={source.title}>
              {source.title}
            </span>
            <span data-testid="activity-source" className="block text-xs text-muted">
              {names.get(source.module) ?? source.module} · {source.action}
            </span>
          </>
        )}
        {entry.path !== undefined && (
          <span className="block truncate font-mono text-xs" title={entry.path}>
            {entry.path}
          </span>
        )}
        {entry.commands?.map((argv, index) => (
          <span
            // By index: the commands of one line are text in the order they ran.
            key={index}
            data-testid="activity-command"
            className="block truncate font-mono text-xs text-muted"
          >
            $ {commandLine(argv)}
          </span>
        ))}
        {detail !== null && (
          // Wrapped rather than truncated into a `title`, for the reason the Explorer's
          // refusal banner records: a tooltip is not reachable by keyboard or touch, not
          // announced, and not selectable for a bug report.
          //
          // A failure message names the path itself (`SystemError`'s `Display`), so this
          // line repeats what the line above it says. Known, and left alone: the
          // alternative is guessing at which part of someone else's sentence is a path.
          <span
            data-testid="activity-detail"
            // Which of the two kinds of sentence this is, in a value a test can read —
            // the same trade `Button`'s `data-variant` makes, for the same reason: that a
            // failure reads as a failure is a decision about meaning, while what red
            // looks like is the stylesheet's business. Both branches below read the one
            // boolean, and the test asserts the pair, so an attribute cannot end up on a
            // line painted the other way.
            data-detail={detail.failure ? 'failure' : 'reason'}
            className={`block text-xs break-words ${
              detail.failure ? 'text-red-700 dark:text-red-400' : 'text-muted'
            }`}
          >
            {detail.text}
          </span>
        )}
      </td>
      <td className={CELL}>{mode ?? EMPTY_CELL}</td>
      <td className={CELL}>{RESULT_LABELS[entry.result]}</td>
      <td className={`${CELL} text-right tabular-nums`}>{bytes ?? EMPTY_CELL}</td>
    </tr>
  );
}

const HEADER_CLASS = 'px-2 py-1 font-medium';

function Entries({
  entries,
  names,
}: {
  entries: readonly ActivityEntry[];
  names: ReadonlyMap<string, string>;
}) {
  return (
    <table className="w-full table-fixed border-collapse text-sm">
      <thead>
        <tr className="border-b border-neutral-200 text-left text-xs text-muted dark:border-neutral-700">
          <th scope="col" className={`w-44 ${HEADER_CLASS}`}>
            When
          </th>
          {/* No column for `kind`: for a skipped entry it is the plan's unverified claim
              (`PreviewEntry::kind`), so an icon here would draw a folder over something
              this app never looked at. */}
          <th scope="col" className={HEADER_CLASS}>
            What
          </th>
          <th scope="col" className={`w-24 ${HEADER_CLASS}`}>
            Mode
          </th>
          <th scope="col" className={`w-24 ${HEADER_CLASS}`}>
            Result
          </th>
          <th scope="col" className={`w-24 text-right ${HEADER_CLASS}`}>
            Size
          </th>
        </tr>
      </thead>
      <tbody data-testid="activity-rows">
        {entries.map((entry, index) => (
          // By index, which is right here for the reason it is right in the dialog's list:
          // a row is text derived from its entry, with no state, no ref and nothing
          // focusable in it, so reusing a position for another entry is indistinguishable
          // from keying it. Nothing else identifies a line — one path can be deleted twice,
          // and one batch can hold the same path twice over.
          <Row key={index} entry={entry} names={names} />
        ))}
      </tbody>
    </table>
  );
}

/**
 * The record of what this app deleted: the end of `actions.jsonl`, newest first.
 *
 * Three things it refuses to do quietly, because a record that loses lines without saying
 * so is worse than no record: it counts the lines it could not parse, it never calls a log
 * it could not read empty, and it says when it is showing only the end of the file.
 */
export default function ActivityPage() {
  const log = useQuery({
    queryKey: ['activity'],
    queryFn: () => activityLog(LIMIT),
    // Every open is a fresh read, which is all the refreshing this screen needs and the
    // only kind it can have: the shell renders one page at a time, so a deletion from the
    // Explorer always happens while this one is unmounted, and an `invalidateQueries`
    // there would be aimed at a query nobody is observing. The cache outlives the unmount
    // and is shown while the re-read is in flight; the entries in it were true one page
    // ago. (`staleTime: 0` is the library's default and is written out because this is a
    // decision: `Infinity`, which the rest of the app uses, would show a record frozen at
    // the first open.)
    staleTime: 0,
  });
  // The names of the modules a cleanup line came from: the sidebar's own query, so nothing is
  // fetched twice. Until it answers, or for a module the build no longer ships, a line names
  // its module by id.
  const modules = useQuery({ queryKey: ['modules'], queryFn: modulesList, staleTime: 0 });
  const names = new Map((modules.data ?? []).map((view) => [view.id, view.name]));

  if (log.isError) {
    return (
      <section className="flex flex-col gap-4 p-5">
        <h2 className="text-xl font-semibold">Activity</h2>
        {/* Not an empty state. A failed read means the record is unknown, not absent, and
            "No actions yet" over a log full of the user's deletions is the silent loss
            `damaged` exists to prevent, one layer up (`activity_tail`'s `Err` contract). */}
        <div
          role="alert"
          className="rounded-xl border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950/40"
        >
          <h3 className="font-semibold text-red-800 dark:text-red-300">
            The record could not be read
          </h3>
          <p className="mt-2 font-mono text-sm break-words text-red-700 dark:text-red-300">
            {String(log.error)}
          </p>
          <Button className="mt-3" onClick={() => void log.refetch()}>
            Try again
          </Button>
        </div>
      </section>
    );
  }

  if (log.data === undefined) {
    return <p className="p-6 text-sm text-muted">Loading…</p>;
  }

  const { entries, damaged } = log.data;
  return (
    <section className="flex flex-col gap-4 p-5">
      <header className="min-w-0">
        <h2 className="text-xl font-semibold">Activity</h2>
        {entries.length > 0 && (
          <p data-testid="activity-summary" className="text-sm text-muted tabular-nums">
            {entries.length === LIMIT
              ? `The ${LIMIT} most recent entries; the record may hold more.`
              : countLabel(entries.length, 'entry', 'entries')}
          </p>
        )}
      </header>

      {damaged > 0 && (
        // Whether or not anything else was readable: the count is part of the answer, not
        // an error, and the whole point of carrying it this far is that it reaches a user.
        <p
          role="note"
          data-testid="activity-damaged"
          className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-300"
        >
          {countLabel(damaged, 'damaged entry', 'damaged entries')} hidden: lines of the record that
          could not be read.
        </p>
      )}

      {entries.length > 0 ? (
        <Entries entries={entries} names={names} />
      ) : (
        // Only when there is nothing at all — a log of nothing but damaged lines is not an
        // empty one, and the note above is what it has to say for itself.
        damaged === 0 && (
          <div data-testid="activity-empty" className="flex flex-col gap-1">
            <p className="text-sm font-medium">No actions yet</p>
            {/* "its size", not "what it freed": under the Trash nothing is freed until the
                Trash is emptied, which the dialog says in words and which the neutral
                "Size" header of the table this replaces gets right. */}
            <p className="text-sm text-muted">
              Deleting from the Explorer records a line here, with the path, the mode and its size.
            </p>
          </div>
        )
      )}
    </section>
  );
}
