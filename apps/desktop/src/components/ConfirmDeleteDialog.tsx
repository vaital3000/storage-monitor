import { useEffect, useRef, useState } from 'react';
import type {
  BatchQuestions,
  BatchReport,
  BatchRow,
  BatchWording,
  ReportRow,
} from '../lib/batchQuestion';
import { describeBlock, type GuardScope } from '../lib/blockReasons';
import { tabStops } from '../lib/focusTrap';
import { countLabel, formatBytes } from '../lib/format';
import { DELETION_MODES, type CleanupProgress, type DeletionMode, type StepView } from '../lib/ipc';
import Button from './Button';

/**
 * Where the batch this dialog asks about has got to.
 *
 * One union rather than a `busy` flag beside an optional outcome, because the four states
 * are exclusive and two of them must never be confused: `failed` is a rejected
 * `action_run` or `cleanup_run`, which means **nothing was touched**, while `done` carries a
 * batch that ran and may still admit that it was not recorded or that the tree is stale.
 * Flat props would let a caller hand over both at once and leave the dialog to guess which
 * one to believe.
 *
 * A cleanup batch reports how far it has got while it runs; an Explorer batch is a handful of
 * paths and says nothing until it is done.
 */
export type BatchStatus =
  | { phase: 'asking' }
  | { phase: 'running'; progress?: CleanupProgress }
  | { phase: 'done'; report: BatchReport }
  | { phase: 'failed'; message: string };

interface ConfirmDeleteDialogProps {
  /**
   * The batch in both modes, the rows of the two aligned by index (`lib/batchQuestion.ts`).
   *
   * Both, and never "the preview", because the mode a preview was computed with can disagree
   * with the one the user has now selected. A dialog that read it would explain the Trash
   * over a button armed to delete for good, which is the worst thing this component could
   * do. Here the only mode there is to read is the toggle's own: the rows, the total and
   * the acknowledgement all come from `questions[mode]`, so a toggle needs no fresh preview
   * and cannot race one. For the Explorer the two are the same rows — the guards never look
   * at the mode — and for Cleanup they are not, because a module plans each mode apart.
   */
  questions: BatchQuestions;
  status: BatchStatus;
  /**
   * Which mode the dialog opens on; the Trash unless the caller says otherwise. The action
   * bar has two entry points, and a "Delete permanently" button that opened a dialog
   * headed "Move 2 items to the Trash?" would be asking about something else. It changes
   * nothing about safety: a permanent deletion still waits for the acknowledgement, so the
   * confirm button is disabled on open either way.
   */
  initialMode?: DeletionMode;
  onConfirm: (mode: DeletionMode) => void;
  /**
   * Asks the caller to unmount the dialog. It says nothing about whether a batch ran —
   * that is what `status` is for — so it is as true after a report as it is over a
   * cancelled confirmation.
   */
  onClose: () => void;
}

const MODE_LABELS: Record<DeletionMode, string> = {
  trash: 'Trash',
  permanent: 'Permanent',
};

const MODE_EXPLANATIONS: Record<BatchWording, Record<DeletionMode, string>> = {
  delete: {
    trash: 'Items move to the Trash. Space is freed when you empty it.',
    permanent: 'Items are deleted immediately. This cannot be undone.',
  },
  clean: {
    trash: 'What can go to the Trash goes there. Space is freed when you empty it.',
    permanent: 'Everything is deleted immediately. This cannot be undone.',
  },
};

/**
 * What the confirming button says. Neither label repeats the entry point that opened the
 * dialog, and for the same reason both times: two live buttons a few hundred pixels apart,
 * reading the same words, is one mis-click.
 *
 * The Trash pair is separated on two axes — "Move to Trash" against "Move to the Trash",
 * and `primary` against the bar's ordinary style. The permanent pair had neither: the same
 * string *and* the same `danger` variant. So this one says **Delete for good**.
 *
 * It states the consequence rather than the mechanism, which is what a confirming button
 * is for: "without the Trash" names a route the user still has to translate into
 * "unrecoverable". The price is that this label no longer derives from the heading above
 * it ("Delete 2 items permanently?"), which the Trash label does; that is worth paying to
 * keep the two red buttons apart.
 */
const CONFIRM_LABELS: Record<BatchWording, Record<DeletionMode, string>> = {
  delete: { trash: 'Move to the Trash', permanent: 'Delete for good' },
  // "Clean", not "Move to the Trash": a cleanup batch in Trash mode can still run commands
  // that destroy, and the button must not promise what the steps above it contradict.
  clean: { trash: 'Clean', permanent: 'Clean for good' },
};

const RUNNING_LABELS: Record<BatchWording, Record<DeletionMode, string>> = {
  delete: { trash: 'Moving to the Trash…', permanent: 'Deleting…' },
  clean: { trash: 'Cleaning…', permanent: 'Cleaning…' },
};

function askTitle(wording: BatchWording, mode: DeletionMode, count: number): string {
  const items = countLabel(count, 'item');
  if (wording === 'clean') {
    return mode === 'trash' ? `Clean ${items}?` : `Clean ${items} permanently?`;
  }
  return mode === 'trash' ? `Move ${items} to the Trash?` : `Delete ${items} permanently?`;
}

/** What a running batch says: how far it has got, when it reports that at all. */
function runningLabel(wording: BatchWording, mode: DeletionMode, progress?: CleanupProgress) {
  if (progress === undefined) {
    return RUNNING_LABELS[wording][mode];
  }
  if (progress.current === null) {
    return 'Finishing…';
  }
  return `Cleaning ${progress.done + 1} of ${progress.total} · ${progress.current}`;
}

/** What each kind of step is called on its line. */
function stepLine(step: StepView): { verb: string; what: string; quiet: boolean } {
  switch (step.step) {
    case 'trash':
      return { verb: 'Move to the Trash', what: step.path, quiet: false };
    case 'delete':
      return { verb: 'Delete', what: step.path, quiet: false };
    case 'run':
      // Housekeeping is drawn quieter than the rest: it is what the other steps leave
      // behind to tidy up, not a thing that goes.
      return { verb: 'Run', what: step.command, quiet: step.effect === 'housekeeping' };
  }
}

/** One line of the result view: a path and what became of it, in words. */
interface ResultLine {
  path: string;
  detail: string;
}

function failedLines(rows: readonly ReportRow[]): ResultLine[] {
  return rows.flatMap((row) =>
    row.result.result === 'failed' ? [{ path: row.title, detail: row.result.message }] : [],
  );
}

/**
 * The entries the batch itself refused. Not the blocked entries of the preview: `missing`
 * and `kindChanged` are decided between the preview and the syscall, so these appear in no
 * preview — and a result view that listed failures alone would count them as deleted.
 */
function skippedLines(rows: readonly ReportRow[], scope: GuardScope): ResultLine[] {
  return rows.flatMap((row) =>
    row.result.result === 'skipped'
      ? [{ path: row.title, detail: describeBlock(row.result.reason, scope) }]
      : [],
  );
}

/**
 * The headline of a report: what left, by how it left. Only the removed rows, and the bytes
 * the batch reported: a count over every row would claim the failed and skipped ones left
 * the disk, in the same sentence that reports the bytes of the ones that did.
 *
 * Counted by each row's own mode, because a cleanup batch can do both — a folder to the
 * Trash, an object removed for good. An Explorer batch has every row in its own mode, so its
 * sentence is the one it always was; a batch that removed nothing speaks in the mode it was
 * asked to run in.
 */
function headline(report: BatchReport): string {
  const removed = report.rows.filter((row) => row.result.result === 'removed');
  const moved = removed.filter((row) => row.mode === 'trash').length;
  const deleted = removed.length - moved;
  const freed = formatBytes(report.freedBytes);
  if (deleted === 0 && (moved > 0 || report.mode === 'trash')) {
    return `Moved ${countLabel(moved, 'item')} to the Trash · ${freed}`;
  }
  if (moved === 0) {
    return `Deleted ${countLabel(deleted, 'item')} · ${freed}`;
  }
  return `Deleted ${countLabel(deleted, 'item')} and moved ${moved} to the Trash · ${freed}`;
}

const PATH_CLASS = 'truncate font-mono text-xs';

/** The height at which a list starts scrolling, and so needs a key to scroll it. */
const SCROLLER_CLASS = 'max-h-60 overflow-y-auto';

/**
 * One group of the result view, under a heading that says which group it is. The heading
 * and not the colour, because the two mean different things to whoever reads them: a
 * failure may have left a tree half torn down, while a skipped entry was never touched.
 *
 * Keyed by path, which is safe here in a way it is not in the preview: one entry has one
 * outcome, and two lines with the same path would have to be a removed duplicate and a
 * skipped one — which land in different lists, since a list holds one arm of `EntryResult`.
 */
function EntryLines({
  lines,
  title,
  testId,
  tone,
}: {
  lines: ResultLine[];
  title: string;
  testId: string;
  tone: string;
}) {
  if (lines.length === 0) {
    return null;
  }
  return (
    <section data-testid={testId} className="flex flex-col gap-1">
      <h3 className="text-sm font-medium">{title}</h3>
      <ul className="flex flex-col gap-1 text-sm">
        {lines.map((line) => (
          <li key={line.path} className="flex min-w-0 flex-col">
            <span className={PATH_CLASS} title={line.path}>
              {line.path}
            </span>
            <span className={tone}>{line.detail}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * The last screen before an irreversible action: what is about to go, how, and afterwards
 * what became of it.
 *
 * Controlled throughout — the caller owns the preview, the batch and the dialog's own
 * lifetime; the only state here is the mode the user has chosen and whether they have
 * acknowledged a permanent deletion.
 */
export default function ConfirmDeleteDialog({
  questions,
  status,
  initialMode = 'trash',
  onConfirm,
  onClose,
}: ConfirmDeleteDialogProps) {
  const [mode, setMode] = useState<DeletionMode>(initialMode);
  const [understood, setUnderstood] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const titleId = 'confirm-delete-title';
  const running = status.phase === 'running';

  // The element the dialog took the focus from, given it back when the dialog goes. Read
  // in an effect of its own, declared before the one that moves the focus, so that it is
  // still the trigger and not this dialog's own Cancel button.
  useEffect(() => {
    const trigger = document.activeElement;
    return () => {
      // A trigger the caller unmounted along the way (the action bar disappears with the
      // selection) can no longer take it; the browser falls back to the document.
      if (trigger instanceof HTMLElement && trigger.isConnected) {
        trigger.focus();
      }
    };
  }, []);

  // Cancel when the dialog opens, Close when the report replaces it: after a phase change
  // the element that had the focus is gone, and a modal that leaves the focus on the body
  // lets the next Tab walk into the page behind it. While the batch runs every control is
  // disabled and the panel itself holds the focus.
  useEffect(() => {
    const target =
      panel.current?.querySelector<HTMLElement>('[data-initial-focus]:not([disabled])') ??
      panel.current;
    target?.focus();
  }, [status.phase]);

  // An acknowledgement covers the batch it was given for. Whatever happened to that batch,
  // the question coming back is a new one, and a tick made for the old one would leave the
  // irreversible button armed for a batch the user has not agreed to — the same rule as
  // the one the mode toggle keeps below, for the same reason.
  //
  // On the way back in rather than on the way out, so that the run itself still shows the
  // tick that authorised it: clearing on the way out paints an unticked "I understand"
  // above "Deleting…", which reads like the app forgetting why it is deleting.
  //
  // Adjusted while rendering rather than in an effect, which is what
  // `react-hooks/set-state-in-effect` asks for: no second render is scheduled behind the
  // first.
  const [shownPhase, setShownPhase] = useState(status.phase);
  if (shownPhase !== status.phase) {
    setShownPhase(status.phase);
    if (status.phase === 'asking') {
      setUnderstood(false);
    }
  }

  // On the document rather than on the panel, because the panel only hears a key while
  // something inside it has the focus — and a click on the backdrop blurs to the body,
  // after which Escape would stop closing the dialog and Tab would step into the page
  // behind it. Modality is not a property of where the focus happens to be.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        // Nothing can stop a batch that is already running, and closing here would throw
        // away the only report of `recorded` and `treeStale` — and hand the page back,
        // free to start a second batch over the tree this one is still patching.
        if (!running) {
          onClose();
        }
        return;
      }
      if (event.key !== 'Tab') {
        return;
      }
      const current = panel.current;
      if (current === null) {
        return;
      }
      const stops = tabStops(current);
      const first = stops[0];
      const last = stops[stops.length - 1];
      const active = document.activeElement;
      const inside = active instanceof Node && current.contains(active);
      // Nothing inside to land on, so the panel takes the focus itself — it holds it
      // without being a stop of its own (`tabIndex={-1}`). Unreachable while the list of
      // entries is a stop, which it always is here; the branch stays because `tabStops`
      // belongs to `lib/` and its contract allows an empty ring, and a dialog should not
      // lean on its own markup forever.
      if (first === undefined || last === undefined) {
        event.preventDefault();
        current.focus();
        return;
      }
      // Three ways the next step would leave the dialog: the two ends of the ring, the
      // panel itself, which holds the focus whenever a phase change found no control to
      // give it to, and a focus that has already fallen outside — a click on the backdrop,
      // which blurs to the body. All three land on the end the direction asks for, rather
      // than on the panel, which would cost another keypress to get anywhere.
      if (!inside || active === current || (event.shiftKey ? active === first : active === last)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [running, onClose]);

  const { wording, scope } = questions;
  const question = questions[mode];
  const ready = question.rows.filter((row) => row.status.state === 'ready');
  const blocked = question.rows.length - ready.length;
  // The acknowledgement follows the rows, not the mode: it is asked for whenever a row that
  // will run cannot be undone in the mode now selected. For the Explorer that is exactly
  // "Permanent"; for Cleanup it is also any command that destroys, whatever the mode.
  const irreversible = ready.filter((row) => row.irreversible).length;
  const needsAck = irreversible > 0;
  // Marked row by row only when the rows disagree; when all of them are irreversible, the
  // sentence of the acknowledgement already says so about every one of them.
  const markRows = needsAck && irreversible < ready.length;

  const chooseMode = (next: DeletionMode) => {
    setMode(next);
    // Every change of mode clears the acknowledgement, so that a trip through the Trash
    // and back does not leave the irreversible button armed by a tick made minutes ago.
    setUnderstood(false);
  };

  return (
    // The backdrop takes no click: this dialog stands in front of an irreversible action,
    // and a mis-click beside the panel must not be the thing that dismisses it.
    <div
      data-testid="delete-backdrop"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
    >
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-busy={running}
        tabIndex={-1}
        className="flex max-h-full w-full max-w-lg flex-col gap-4 overflow-hidden rounded-xl border border-neutral-200 bg-white p-5 shadow-xl outline-none dark:border-neutral-800 dark:bg-neutral-900"
      >
        {status.phase === 'done' ? (
          <Report report={status.report} titleId={titleId} onClose={onClose} />
        ) : status.phase === 'failed' ? (
          <RunError
            message={status.message}
            wording={wording}
            titleId={titleId}
            onClose={onClose}
          />
        ) : (
          <>
            <h2 id={titleId} className="text-lg font-semibold">
              {askTitle(wording, mode, ready.length)}
            </h2>

            {/* A tab stop of its own, so that a user confirming two hundred entries can
                scroll the list without a pointer — inside a focus trap there is no other
                way to reach it. */}
            <ul
              data-testid="delete-entries"
              tabIndex={0}
              aria-label={wording === 'clean' ? 'Items to clean' : 'Entries to delete'}
              className={`flex flex-col gap-1 text-sm ${SCROLLER_CLASS} focus-visible:outline-2 focus-visible:outline-blue-500`}
            >
              {question.rows.map((row, index) => (
                // By index, which is right here and wrong almost everywhere else. A path is
                // not unique in this list — an exact duplicate comes back from the guards
                // as a second entry blocked as `nested` (`engine.rs`) — so a path key would
                // make React drop one of the two rows of the batch the user is confirming.
                //
                // An index key is safe because of what a row is, not because of the order
                // it arrives in: every `<li>` is text derived from its row, with no local
                // state, no ref and nothing focusable in it. Reusing one position for a
                // different row is then indistinguishable from keying it, even under a
                // reorder. Give a row state — a per-row checkbox, say — and this has to
                // become a key that identifies the row, which the path cannot.
                <Row key={index} row={row} scope={scope} mark={markRows && row.irreversible} />
              ))}
            </ul>

            <p data-testid="delete-total" className="text-sm font-medium tabular-nums">
              {countLabel(ready.length, 'item')} · {formatBytes(question.totalBytes)}
              {blocked > 0 && ` · ${blocked} blocked`}
            </p>

            <fieldset className="flex flex-col gap-2">
              <legend className="sr-only">How to delete</legend>
              <div className="flex gap-4">
                {DELETION_MODES.map((candidate) => (
                  <label key={candidate} className="flex items-center gap-1.5 text-sm">
                    <input
                      type="radio"
                      name="deletion-mode"
                      value={candidate}
                      checked={mode === candidate}
                      disabled={running}
                      onChange={() => chooseMode(candidate)}
                      className="size-4 accent-blue-600"
                    />
                    {MODE_LABELS[candidate]}
                  </label>
                ))}
              </div>
              <p data-testid="mode-explanation" className="text-sm text-muted">
                {MODE_EXPLANATIONS[wording][mode]}
              </p>
            </fieldset>

            {needsAck && (
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={understood}
                  disabled={running}
                  onChange={(event) => setUnderstood(event.target.checked)}
                  className="size-4 accent-blue-600"
                />
                {irreversible === ready.length
                  ? 'I understand that this cannot be undone.'
                  : `I understand that ${countLabel(irreversible, 'item')} cannot be undone.`}
              </label>
            )}

            {running && (
              // The mode cannot change under a running batch — the radios are disabled —
              // so this is the mode the batch was confirmed with.
              <p role="status" className="text-sm text-muted">
                {runningLabel(
                  wording,
                  mode,
                  status.phase === 'running' ? status.progress : undefined,
                )}
              </p>
            )}

            <div className="flex justify-end gap-2">
              <Button data-initial-focus="" disabled={running} onClick={onClose}>
                Cancel
              </Button>
              <Button
                data-testid="confirm-delete"
                // Red for the deletion that cannot be taken back, and the ordinary emphasis
                // for the one that can. `primary` for both was the original choice, made
                // before a `danger` style existed; leaving it there would put the blue
                // "this is the way on" on the most dangerous control in the app, under a
                // red button that only opened a dialog.
                variant={needsAck ? 'danger' : 'primary'}
                disabled={running || ready.length === 0 || (needsAck && !understood)}
                onClick={() => onConfirm(mode)}
              >
                {CONFIRM_LABELS[wording][mode]}
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * One row of the question: its title, what it is and what will run, its size, and whatever
 * stands in its way.
 *
 * The title keeps the `title` attribute it has always carried, first in the row, so that a
 * long path can still be read in full; the step lines below it carry none, being the
 * details of the row rather than its name.
 */
function Row({ row, scope, mark }: { row: BatchRow; scope: GuardScope; mark: boolean }) {
  return (
    <li
      data-state={row.status.state}
      className={`flex min-w-0 flex-col ${row.status.state === 'blocked' ? 'text-muted' : ''}`}
    >
      <span className="flex min-w-0 items-baseline gap-2">
        <span className={PATH_CLASS} title={row.title}>
          {row.title}
        </span>
        <span className="ml-auto shrink-0 tabular-nums">{formatBytes(row.size)}</span>
      </span>
      {row.context !== undefined && (
        <span data-testid="row-context" className="text-xs text-muted">
          {row.context}
        </span>
      )}
      {row.steps !== undefined && row.steps.length > 0 && (
        <ol data-testid="row-steps" className="flex flex-col text-xs">
          {row.steps.map((step, index) => {
            const { verb, what, quiet } = stepLine(step);
            return (
              <li
                // By index: a step is text derived from its row, like the row itself.
                key={index}
                data-quiet={quiet ? '' : undefined}
                className={`flex min-w-0 gap-1 ${quiet ? 'text-muted' : ''}`}
              >
                <span className="shrink-0">{verb}</span>
                <span className="truncate font-mono">{what}</span>
              </li>
            );
          })}
        </ol>
      )}
      {mark && (
        <span data-testid="irreversible" className="text-xs text-red-700 dark:text-red-400">
          Cannot be undone
        </span>
      )}
      {row.status.state === 'blocked' && (
        <span data-testid="block-reason" className="text-xs">
          {describeBlock(row.status.reason, scope)}
        </span>
      )}
    </li>
  );
}

interface ReportProps {
  report: BatchReport;
  titleId: string;
  onClose: () => void;
}

/** What a batch that ran did — including the two things it may have to admit afterwards. */
function Report({ report, titleId, onClose }: ReportProps) {
  const { rows, recorded, treeStale, wording, scope } = report;
  const failed = failedLines(rows);
  const skipped = skippedLines(rows, scope);
  return (
    <>
      <h2 id={titleId} data-testid="result-summary" className="text-lg font-semibold">
        {headline(report)}
      </h2>

      <div
        tabIndex={0}
        role="group"
        aria-label="What the batch did"
        className={`flex flex-col gap-3 ${SCROLLER_CLASS} focus-visible:outline-2 focus-visible:outline-blue-500`}
      >
        <EntryLines
          lines={failed}
          title={wording === 'clean' ? 'Could not be cleaned' : 'Could not be deleted'}
          testId="result-failed"
          tone="text-red-700 dark:text-red-400"
        />
        <EntryLines lines={skipped} title="Skipped" testId="result-skipped" tone="text-muted" />
      </div>

      {!recorded && (
        <p data-testid="not-recorded" className="text-sm text-amber-700 dark:text-amber-400">
          {wording === 'clean' ? 'Cleaned' : 'Deleted'}, but not recorded: the Activity log has no
          line for this batch.
        </p>
      )}
      {treeStale && (
        <p data-testid="tree-stale" className="text-sm text-amber-700 dark:text-amber-400">
          The Explorer may be stale until the next scan.
        </p>
      )}

      <div className="flex justify-end">
        <Button data-initial-focus="" variant="primary" onClick={onClose}>
          Close
        </Button>
      </div>
    </>
  );
}

/**
 * A rejected `action_run`: the batch did not start, so nothing here may borrow the words of
 * one that deleted and could not say so afterwards.
 */
function RunError({
  message,
  wording,
  titleId,
  onClose,
}: {
  message: string;
  wording: BatchWording;
  titleId: string;
  onClose: () => void;
}) {
  return (
    <>
      <h2 id={titleId} className="text-lg font-semibold text-red-700 dark:text-red-400">
        {wording === 'clean' ? 'The cleanup did not run' : 'The deletion did not run'}
      </h2>
      <p data-testid="run-error" className="font-mono text-sm break-words">
        {message}
      </p>
      <p className="text-sm text-muted">
        {wording === 'clean' ? 'Nothing was cleaned.' : 'Nothing was deleted.'}
      </p>
      <div className="flex justify-end">
        <Button data-initial-focus="" variant="primary" onClick={onClose}>
          Close
        </Button>
      </div>
    </>
  );
}
