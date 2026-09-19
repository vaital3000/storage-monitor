import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { countLabel, formatBytes } from '../lib/format';
import {
  DELETION_MODES,
  type BatchResult,
  type BlockReason,
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
  onConfirm: (mode: DeletionMode) => void;
  /**
   * Asks the caller to unmount the dialog. It says nothing about whether a batch ran —
   * that is what `status` is for — so it is as true after a report as it is over a
   * cancelled confirmation.
   */
  onClose: () => void;
  /**
   * Reveals the Trash folder, when the caller has a way to. Without it the result view
   * shows no such button, rather than one that cannot keep its promise.
   *
   * The dialog holds no path of its own: `AppInfo` carries none, and the home folder is
   * only reachable through `default_root`, whose meaning is "the folder scanned when the
   * UI does not pick one" and not "the Trash lives here". Opening a folder outright would
   * also need `opener:allow-open-path` in `src-tauri/capabilities/default.json`, which is
   * a privilege grant and not a formality: `open_path` reaches macOS `open`, which
   * launches applications, so granting it lets the frontend ask the OS to open any path it
   * can name. That is a decision to take deliberately — with an ADR, on a branch that just
   * finished closing the Content Security Policy — and not a line added in passing to make
   * a button work.
   */
  onShowInTrash?: () => void;
}

/**
 * Every `BlockReason` in words, from the guards' own vocabulary
 * (`crates/core/src/action/model.rs`).
 *
 * `nested` keeps the backend's phrasing even though an exact duplicate lands here too and
 * contains nothing: that trade-off is taken and explained in `engine.rs`, and a special
 * case here would only hide a verdict the Activity screen still reports plainly.
 */
const BLOCK_REASON_LABELS: Record<BlockReason, string> = {
  outsideRoots: 'Outside the folder that was scanned',
  denylisted: 'Inside a folder this app never deletes from',
  malformed: 'Not a path that names an entry',
  isRoot: 'The scanned folder itself, or one above it',
  nested: 'Another entry contains it',
  missing: 'Nothing is there any more',
  unreadable: 'Cannot be read — it may need Full Disk Access',
  kindChanged: 'No longer what the preview saw',
};

const UNKNOWN_BLOCK_REASON = 'Blocked for a reason this version does not know';

/**
 * What a verdict says on screen. A `BlockReason` added to the backend after this build was
 * made arrives as a string with no entry above, and the one place a user must not be shown
 * a blank line is the list of what is about to be deleted.
 */
function describeBlock(reason: BlockReason): string {
  return BLOCK_REASON_LABELS[reason] ?? UNKNOWN_BLOCK_REASON;
}

const MODE_LABELS: Record<DeletionMode, string> = {
  trash: 'Trash',
  permanent: 'Permanent',
};

const MODE_EXPLANATIONS: Record<DeletionMode, string> = {
  trash: 'Items move to the Trash. Space is freed when you empty it.',
  permanent: 'Items are deleted immediately. This cannot be undone.',
};

const CONFIRM_LABELS: Record<DeletionMode, string> = {
  trash: 'Move to the Trash',
  permanent: 'Delete permanently',
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

/** Tab has to reach these; everything else in the panel is text. */
const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])';

const PATH_CLASS = 'truncate font-mono text-xs';

/**
 * One group of the result view, under a heading that says which group it is. The heading
 * and not the colour, because the two mean different things to whoever reads them: a
 * failure may have left a tree half torn down, while a skipped entry was never touched.
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
  onConfirm,
  onClose,
  onShowInTrash,
}: ConfirmDeleteDialogProps) {
  const [mode, setMode] = useState<DeletionMode>('trash');
  const [understood, setUnderstood] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const titleId = 'confirm-delete-title';

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

  const running = status.phase === 'running';
  const ready = preview.entries.filter((entry) => entry.status.state === 'ready');
  const blocked = preview.entries.length - ready.length;

  const chooseMode = (next: DeletionMode) => {
    setMode(next);
    // Every change of mode clears the acknowledgement, so that a trip through the Trash
    // and back does not leave the irreversible button armed by a tick made minutes ago.
    setUnderstood(false);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      // Nothing can stop a batch that is already running, and closing here would throw
      // away the only report of `recorded` and `treeStale` — and hand the page back, free
      // to start a second batch over the tree this one is still patching.
      if (!running) {
        onClose();
      }
      return;
    }
    if (event.key !== 'Tab' || panel.current === null) {
      return;
    }
    // jsdom moves no focus on Tab and a browser moves it out of the dialog: what is
    // implemented here is only the boundary — the wrap that keeps a modal modal.
    const focusable = [...panel.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (first === undefined || last === undefined) {
      return;
    }
    if (event.shiftKey ? document.activeElement === first : document.activeElement === last) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-busy={running}
        tabIndex={-1}
        onKeyDown={onKeyDown}
        className="flex max-h-full w-full max-w-lg flex-col gap-4 overflow-hidden rounded-xl border border-neutral-200 bg-white p-5 shadow-xl outline-none dark:border-neutral-800 dark:bg-neutral-900"
      >
        {status.phase === 'done' ? (
          <Report
            result={status.result}
            titleId={titleId}
            onClose={onClose}
            onShowInTrash={onShowInTrash}
          />
        ) : status.phase === 'failed' ? (
          <RunError message={status.message} titleId={titleId} onClose={onClose} />
        ) : (
          <>
            <h2 id={titleId} className="text-lg font-semibold">
              {askTitle(mode, ready.length)}
            </h2>

            <ul
              data-testid="delete-entries"
              className="flex max-h-60 flex-col gap-1 overflow-y-auto text-sm"
            >
              {preview.entries.map((entry) => {
                const isBlocked = entry.status.state === 'blocked';
                return (
                  <li
                    key={entry.path}
                    data-state={entry.status.state}
                    className={`flex min-w-0 flex-col ${isBlocked ? 'text-muted' : ''}`}
                  >
                    <span className="flex min-w-0 items-baseline gap-2">
                      <span className={PATH_CLASS} title={entry.path}>
                        {entry.path}
                      </span>
                      <span className="ml-auto shrink-0 tabular-nums">
                        {formatBytes(entry.size)}
                      </span>
                    </span>
                    {entry.status.state === 'blocked' && (
                      <span data-testid="block-reason" className="text-xs">
                        {describeBlock(entry.status.reason)}
                      </span>
                    )}
                  </li>
                );
              })}
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
                variant="primary"
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
  onShowInTrash?: () => void;
}

/** What a batch that ran did — including the two things it may have to admit afterwards. */
function Report({ result, titleId, onClose, onShowInTrash }: ReportProps) {
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

      <div className="flex max-h-60 flex-col gap-3 overflow-y-auto">
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

      <div className="flex justify-end gap-2">
        {moved && onShowInTrash !== undefined && (
          <Button onClick={onShowInTrash}>Show in Trash</Button>
        )}
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
