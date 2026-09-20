import { useEffect, useRef, useState } from 'react';
import { describeBlock } from '../lib/blockReasons';
import { tabStops } from '../lib/focusTrap';
import { countLabel, formatBytes } from '../lib/format';
import {
  DELETION_MODES,
  type BatchResult,
  type DeletionMode,
  type EntryOutcome,
  type Preview,
} from '../lib/ipc';
import Button from './Button';

/**
 * Where the batch this dialog asks about has got to.
 *
 * One union rather than a `busy` flag beside an optional outcome, because the four states
 * are exclusive and two of them must never be confused: `failed` is a rejected
 * `action_run`, which means **nothing was touched**, while `done` carries a batch that ran
 * and may still admit that it was not recorded or that the tree is stale. Flat props would
 * let a caller hand over both at once and leave the dialog to guess which one to believe.
 */
export type BatchStatus =
  | { phase: 'asking' }
  | { phase: 'running' }
  | { phase: 'done'; result: BatchResult }
  | { phase: 'failed'; message: string };

interface ConfirmDeleteDialogProps {
  /**
   * What the guards said, minus the mode it was checked in — deliberately, and enforced by
   * the type rather than by a rule someone has to remember.
   *
   * The guards never look at the mode: `crates/core/src/action/guards.rs` does not mention
   * it, and in `engine.rs` it is copied into the preview and read again only to choose
   * between `move_to_trash` and `remove`. So every verdict, size and `totalBytes` here
   * holds for both modes, the toggle below needs no fresh preview — and the mode the
   * preview was computed with can disagree with the one the user has now selected. A
   * dialog that read it would explain the Trash over a button armed to delete for good,
   * which is the worst thing this component could do; `Omit` makes that a type error.
   */
  preview: Omit<Preview, 'mode'>;
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

const MODE_EXPLANATIONS: Record<DeletionMode, string> = {
  trash: 'Items move to the Trash. Space is freed when you empty it.',
  permanent: 'Items are deleted immediately. This cannot be undone.',
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
const CONFIRM_LABELS: Record<DeletionMode, string> = {
  trash: 'Move to the Trash',
  permanent: 'Delete for good',
};

const RUNNING_LABELS: Record<DeletionMode, string> = {
  trash: 'Moving to the Trash…',
  permanent: 'Deleting…',
};

function askTitle(mode: DeletionMode, count: number): string {
  const items = countLabel(count, 'item');
  return mode === 'trash' ? `Move ${items} to the Trash?` : `Delete ${items} permanently?`;
}

/** One line of the result view: a path and what became of it, in words. */
interface ResultLine {
  path: string;
  detail: string;
}

function failedLines(entries: readonly EntryOutcome[]): ResultLine[] {
  return entries.flatMap((entry) =>
    entry.result.result === 'failed' ? [{ path: entry.path, detail: entry.result.message }] : [],
  );
}

/**
 * The entries the batch itself refused. Not the blocked entries of the preview: `missing`
 * and `kindChanged` are decided between the preview and the syscall, so these appear in no
 * preview — and a result view that listed failures alone would count them as deleted.
 */
function skippedLines(entries: readonly EntryOutcome[]): ResultLine[] {
  return entries.flatMap((entry) =>
    entry.result.result === 'skipped'
      ? [{ path: entry.path, detail: describeBlock(entry.result.reason) }]
      : [],
  );
}

function removedCount(entries: readonly EntryOutcome[]): number {
  return entries.reduce(
    (count, entry) => (entry.result.result === 'removed' ? count + 1 : count),
    0,
  );
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
  preview,
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

  const ready = preview.entries.filter((entry) => entry.status.state === 'ready');
  const blocked = preview.entries.length - ready.length;

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
          <Report result={status.result} titleId={titleId} onClose={onClose} />
        ) : status.phase === 'failed' ? (
          <RunError message={status.message} titleId={titleId} onClose={onClose} />
        ) : (
          <>
            <h2 id={titleId} className="text-lg font-semibold">
              {askTitle(mode, ready.length)}
            </h2>

            {/* A tab stop of its own, so that a user confirming two hundred entries can
                scroll the list without a pointer — inside a focus trap there is no other
                way to reach it. */}
            <ul
              data-testid="delete-entries"
              tabIndex={0}
              aria-label="Entries to delete"
              className={`flex flex-col gap-1 text-sm ${SCROLLER_CLASS} focus-visible:outline-2 focus-visible:outline-blue-500`}
            >
              {preview.entries.map((entry, index) => (
                // By index, which is right here and wrong almost everywhere else. A path is
                // not unique in this list — an exact duplicate comes back from the guards
                // as a second entry blocked as `nested` (`engine.rs`) — so a path key would
                // make React drop one of the two rows of the batch the user is confirming.
                //
                // An index key is safe because of what a row is, not because of the order
                // it arrives in: every `<li>` is text derived from its entry, with no local
                // state, no ref and nothing focusable in it. Reusing one position for a
                // different entry is then indistinguishable from keying it, even under a
                // reorder. Give a row state — a per-row checkbox, say — and this has to
                // become a key that identifies the entry, which the path cannot.
                <li
                  key={index}
                  data-state={entry.status.state}
                  className={`flex min-w-0 flex-col ${
                    entry.status.state === 'blocked' ? 'text-muted' : ''
                  }`}
                >
                  <span className="flex min-w-0 items-baseline gap-2">
                    <span className={PATH_CLASS} title={entry.path}>
                      {entry.path}
                    </span>
                    <span className="ml-auto shrink-0 tabular-nums">{formatBytes(entry.size)}</span>
                  </span>
                  {entry.status.state === 'blocked' && (
                    <span data-testid="block-reason" className="text-xs">
                      {describeBlock(entry.status.reason)}
                    </span>
                  )}
                </li>
              ))}
            </ul>

            <p data-testid="delete-total" className="text-sm font-medium tabular-nums">
              {countLabel(ready.length, 'item')} · {formatBytes(preview.totalBytes)}
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
                {MODE_EXPLANATIONS[mode]}
              </p>
            </fieldset>

            {mode === 'permanent' && (
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={understood}
                  disabled={running}
                  onChange={(event) => setUnderstood(event.target.checked)}
                  className="size-4 accent-blue-600"
                />
                I understand that this cannot be undone.
              </label>
            )}

            {running && (
              // The mode cannot change under a running batch — the radios are disabled —
              // so this is the mode the batch was confirmed with.
              <p role="status" className="text-sm text-muted">
                {RUNNING_LABELS[mode]}
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
                variant={mode === 'permanent' ? 'danger' : 'primary'}
                disabled={running || ready.length === 0 || (mode === 'permanent' && !understood)}
                onClick={() => onConfirm(mode)}
              >
                {CONFIRM_LABELS[mode]}
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

interface ReportProps {
  result: BatchResult;
  titleId: string;
  onClose: () => void;
}

/** What a batch that ran did — including the two things it may have to admit afterwards. */
function Report({ result, titleId, onClose }: ReportProps) {
  const { outcome, recorded, treeStale } = result;
  const failed = failedLines(outcome.entries);
  const skipped = skippedLines(outcome.entries);
  // The mode the batch ran in — the one place a mode is read from anything but the toggle,
  // because this sentence is about what happened and not about what is being asked.
  const moved = outcome.mode === 'trash';
  // Only the removed entries, and the bytes the batch reported: a count over every entry
  // would claim the failed and skipped ones left the disk, in the same sentence that
  // reports the bytes of the ones that did.
  const items = countLabel(removedCount(outcome.entries), 'item');
  const freed = formatBytes(outcome.freedBytes);
  return (
    <>
      <h2 id={titleId} data-testid="result-summary" className="text-lg font-semibold">
        {moved ? `Moved ${items} to the Trash · ${freed}` : `Deleted ${items} · ${freed}`}
      </h2>

      <div
        tabIndex={0}
        role="group"
        aria-label="What the batch did"
        className={`flex flex-col gap-3 ${SCROLLER_CLASS} focus-visible:outline-2 focus-visible:outline-blue-500`}
      >
        <EntryLines
          lines={failed}
          title="Could not be deleted"
          testId="result-failed"
          tone="text-red-700 dark:text-red-400"
        />
        <EntryLines lines={skipped} title="Skipped" testId="result-skipped" tone="text-muted" />
      </div>

      {!recorded && (
        <p data-testid="not-recorded" className="text-sm text-amber-700 dark:text-amber-400">
          Deleted, but not recorded: the Activity log has no line for this batch.
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
  titleId,
  onClose,
}: {
  message: string;
  titleId: string;
  onClose: () => void;
}) {
  return (
    <>
      <h2 id={titleId} className="text-lg font-semibold text-red-700 dark:text-red-400">
        The deletion did not run
      </h2>
      <p data-testid="run-error" className="font-mono text-sm break-words">
        {message}
      </p>
      <p className="text-sm text-muted">Nothing was deleted.</p>
      <div className="flex justify-end">
        <Button data-initial-focus="" variant="primary" onClick={onClose}>
          Close
        </Button>
      </div>
    </>
  );
}
