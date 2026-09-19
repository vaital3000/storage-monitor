import { fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import {
  BLOCK_REASONS,
  type BatchResult,
  type BlockReason,
  type DeletionMode,
  type EntryOutcome,
  type EntryResult,
  type NodeKind,
  type Preview,
  type PreviewEntry,
} from '../lib/ipc';
import ConfirmDeleteDialog, { type BatchStatus } from './ConfirmDeleteDialog';

// The mock cannot stage a failure, an unreadable entry, a changed kind, `recorded: false`
// or `treeStale: true` — it deletes from the fixture and always succeeds. This is a
// controlled component, so every test builds the payload it needs as a literal instead of
// routing through a batch that cannot produce it.

const ROOT = '/Users/demo';

function ready(path: string, size: number, kind: NodeKind = 'dir'): PreviewEntry {
  return { path, kind, size, status: { state: 'ready' } };
}

function blocked(path: string, reason: BlockReason, size = 0): PreviewEntry {
  return { path, kind: 'dir', size, status: { state: 'blocked', reason } };
}

/** What the dialog is handed: a `Preview` minus the mode it was computed with. */
type DialogPreview = Omit<Preview, 'mode'>;

/**
 * A preview the way the backend builds one, with `totalBytes` summed over the ready
 * entries. `totalBytes` can be given instead, for the tests that check which of the two
 * numbers reaches the screen.
 */
function previewOf(entries: PreviewEntry[], totalBytes?: number): DialogPreview {
  return {
    entries,
    totalBytes:
      totalBytes ??
      entries
        .filter((entry) => entry.status.state === 'ready')
        .reduce((sum, entry) => sum + entry.size, 0),
  };
}

const TWO_READY = previewOf([ready(`${ROOT}/Downloads`, 2_000_000_000), ready(`${ROOT}/src`, 1e9)]);

function outcomeEntry(path: string, result: EntryResult): EntryOutcome {
  return { path, kind: 'dir', result };
}

interface BatchOptions {
  mode?: DeletionMode;
  freedBytes?: number;
  recorded?: boolean;
  treeStale?: boolean;
}

function batch(entries: EntryOutcome[], options: BatchOptions = {}): BatchResult {
  const { mode = 'trash', recorded = true, treeStale = false } = options;
  const removed = entries.filter((entry) => entry.result.result === 'removed');
  return {
    outcome: {
      entries,
      freedBytes:
        options.freedBytes ??
        removed.reduce(
          (sum, entry) => sum + (entry.result.result === 'removed' ? entry.result.bytes : 0),
          0,
        ),
      at: '2026-09-19T10:00:00Z',
      mode,
    },
    recorded,
    treeStale,
  };
}

interface ShowOptions {
  status?: BatchStatus;
  onConfirm?: (mode: DeletionMode) => void;
  onClose?: () => void;
  onShowInTrash?: () => void;
}

function show(preview: DialogPreview, options: ShowOptions = {}) {
  const { status = { phase: 'asking' }, onConfirm = noop, onClose = noop } = options;
  return render(
    <ConfirmDeleteDialog
      preview={preview}
      status={status}
      onConfirm={onConfirm}
      onClose={onClose}
      onShowInTrash={options.onShowInTrash}
    />,
  );
}

const noop = () => undefined;

function dialog(): HTMLElement {
  return screen.getByRole('dialog');
}

function entryItems(): HTMLElement[] {
  return within(screen.getByTestId('delete-entries')).getAllByRole('listitem');
}

function itemFor(path: string): HTMLElement {
  const item = entryItems().find((candidate) => within(candidate).queryByText(path) !== null);
  if (item === undefined) {
    throw new Error(`no entry for ${path}; entries: ${entryItems().map((e) => e.textContent)}`);
  }
  return item;
}

function modeRadio(label: string): HTMLInputElement {
  return screen.getByRole<HTMLInputElement>('radio', { name: label });
}

function understandBox(): HTMLInputElement {
  return screen.getByRole<HTMLInputElement>('checkbox');
}

function confirmButton(): HTMLButtonElement {
  return screen.getByTestId<HTMLButtonElement>('confirm-delete');
}

function selectMode(label: string): void {
  fireEvent.click(modeRadio(label));
}

describe('ConfirmDeleteDialog', () => {
  it('is a modal dialog named by its own heading', () => {
    show(TWO_READY);
    expect(dialog()).toHaveAttribute('aria-modal', 'true');
    expect(dialog()).toHaveAccessibleName('Move 2 items to the Trash?');
  });

  it('lists an entry by its path, because two selected folders can carry one name', () => {
    // The fixture holds `/Users/demo/Downloads` and `/Users/demo/src/Downloads`. This is
    // the last screen before an irreversible action: a list of bare names would not say
    // which of the two is about to go.
    show(previewOf([ready(`${ROOT}/Downloads`, 1e9), ready(`${ROOT}/src/Downloads`, 2e9)]));
    const shown = entryItems().map((item) => item.textContent);
    expect(shown).toHaveLength(2);
    expect(shown[0]).toContain(`${ROOT}/Downloads`);
    expect(shown[1]).toContain(`${ROOT}/src/Downloads`);
    expect(shown[1]).not.toContain(`${ROOT}/Downloads `);
  });

  it('shows every entry with its size formatted the way the table formats one', () => {
    show(previewOf([ready(`${ROOT}/Downloads`, 2_000_000_000), ready(`${ROOT}/.zshrc`, 512)]));
    expect(itemFor(`${ROOT}/Downloads`)).toHaveTextContent('2.0 GB');
    expect(itemFor(`${ROOT}/.zshrc`)).toHaveTextContent('512 B');
  });

  it('mutes a blocked entry and spells out why in words', () => {
    show(previewOf([ready(`${ROOT}/src`, 1e9), blocked(`${ROOT}/src/build`, 'nested', 4e8)]));
    const nested = itemFor(`${ROOT}/src/build`);
    expect(nested).toHaveAttribute('data-state', 'blocked');
    expect(nested).toHaveClass('text-muted');
    // The backend's own words for this verdict (`engine.rs`), duplicates included.
    expect(nested).toHaveTextContent('Another entry contains it');

    const keeper = itemFor(`${ROOT}/src`);
    expect(keeper).toHaveAttribute('data-state', 'ready');
    expect(keeper).not.toHaveClass('text-muted');
  });

  it('has words for all eight block reasons', () => {
    for (const reason of BLOCK_REASONS) {
      const { unmount } = show(previewOf([blocked(`${ROOT}/thing`, reason)]));
      const label = within(itemFor(`${ROOT}/thing`)).getByTestId('block-reason').textContent ?? '';
      // Not the wire name, not empty, and a sentence rather than a word.
      expect(label).not.toBe(reason);
      expect(label.length).toBeGreaterThan(8);
      unmount();
    }
  });

  it('names a reason this build does not know instead of showing nothing', () => {
    // A ninth `BlockReason`, added in Rust and mirrored in `ipc.ts` after this file was
    // built. An unknown key must not render as a blank line next to a path.
    show(previewOf([blocked(`${ROOT}/thing`, 'quarantined' as BlockReason)]));
    expect(within(itemFor(`${ROOT}/thing`)).getByTestId('block-reason')).toHaveTextContent(
      'Blocked for a reason this version does not know',
    );
  });

  it('counts only the ready entries in the total', () => {
    show(
      previewOf([
        ready(`${ROOT}/Downloads`, 2e9),
        blocked(`${ROOT}/Library`, 'denylisted', 9e9),
        ready(`${ROOT}/src`, 1e9),
      ]),
    );
    expect(screen.getByTestId('delete-total')).toHaveTextContent('2 items · 3.0 GB');
    expect(screen.getByTestId('delete-total')).toHaveTextContent('1 blocked');
  });

  it('takes the total from the backend rather than adding the rows up again', () => {
    // The two numbers agree in practice; they are made to disagree here because only the
    // backend saw the real sizes, and the dialog is not allowed to have an opinion about
    // them. A dialog that sums what it renders would print 3.0 GB.
    show(previewOf([ready(`${ROOT}/Downloads`, 2e9), ready(`${ROOT}/src`, 1e9)], 7_000_000_000));
    expect(screen.getByTestId('delete-total')).toHaveTextContent('2 items · 7.0 GB');
  });

  it('opens on the Trash and explains what the Trash does', () => {
    show(TWO_READY);
    expect(modeRadio('Trash')).toBeChecked();
    expect(screen.getByTestId('mode-explanation')).toHaveTextContent(
      'Items move to the Trash. Space is freed when you empty it.',
    );
    expect(confirmButton()).toHaveTextContent('Move to the Trash');
    expect(confirmButton()).toBeEnabled();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });

  it('holds the permanent deletion until the user says they understand it', () => {
    const confirmed = vi.fn<(mode: DeletionMode) => void>();
    show(TWO_READY, { onConfirm: confirmed });

    selectMode('Permanent');
    expect(screen.getByTestId('mode-explanation')).toHaveTextContent(
      'Items are deleted immediately. This cannot be undone.',
    );
    expect(confirmButton()).toHaveTextContent('Delete permanently');
    expect(confirmButton()).toBeDisabled();

    fireEvent.click(confirmButton());
    expect(confirmed).not.toHaveBeenCalled();

    fireEvent.click(understandBox());
    expect(confirmButton()).toBeEnabled();
    fireEvent.click(confirmButton());
    expect(confirmed).toHaveBeenCalledExactlyOnceWith('permanent');
  });

  it('forgets the acknowledgement when the mode goes back to the Trash', () => {
    show(TWO_READY);
    selectMode('Permanent');
    fireEvent.click(understandBox());
    expect(confirmButton()).toBeEnabled();

    selectMode('Trash');
    expect(confirmButton()).toBeEnabled();
    expect(confirmButton()).toHaveTextContent('Move to the Trash');

    // The point of clearing it: a second trip to Permanent starts locked again, rather
    // than arming the irreversible button on one click.
    selectMode('Permanent');
    expect(understandBox()).not.toBeChecked();
    expect(confirmButton()).toBeDisabled();
  });

  it('speaks the selected mode, not the one the preview was computed with', () => {
    // A `Preview` carries the mode it was checked in, and the toggle is local: the guards
    // never look at the mode, so no verdict here changes with it. The dialog must still
    // never read that field, or it would explain the Trash over a button armed to delete.
    const confirmed = vi.fn<(mode: DeletionMode) => void>();
    const asChecked: Preview = { ...TWO_READY, mode: 'trash' };
    show(asChecked, { onConfirm: confirmed });

    selectMode('Permanent');
    fireEvent.click(understandBox());
    expect(dialog()).toHaveAccessibleName('Delete 2 items permanently?');
    expect(screen.getByTestId('mode-explanation')).toHaveTextContent('This cannot be undone.');
    fireEvent.click(confirmButton());
    expect(confirmed).toHaveBeenCalledExactlyOnceWith('permanent');
  });

  it('confirms with the mode it is showing', () => {
    const confirmed = vi.fn<(mode: DeletionMode) => void>();
    show(TWO_READY, { onConfirm: confirmed });
    fireEvent.click(confirmButton());
    expect(confirmed).toHaveBeenCalledExactlyOnceWith('trash');
  });

  it('refuses a batch in which nothing can be deleted', () => {
    show(previewOf([blocked(`${ROOT}/Library`, 'denylisted'), blocked('/', 'malformed')]));
    expect(confirmButton()).toBeDisabled();
    expect(screen.getByTestId('delete-total')).toHaveTextContent('0 items');
  });

  it('cancels on the button and on Escape', () => {
    const closed = vi.fn();
    show(TWO_READY, { onClose: closed });
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(closed).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(closed).toHaveBeenCalledTimes(2);
  });

  it('disables every control while the batch runs and says that it is running', () => {
    show(TWO_READY, { status: { phase: 'running' } });
    expect(screen.getByRole('status')).toHaveTextContent('Moving to the Trash…');
    expect(confirmButton()).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    expect(modeRadio('Permanent')).toBeDisabled();
  });

  it('ignores Escape while the batch runs, so its report cannot be lost', () => {
    // Nothing can stop the batch now. Closing here would throw away the only place that
    // says `recorded: false` or `treeStale: true`, and would let the page start a second
    // batch over the tree this one is still patching.
    const closed = vi.fn();
    show(TWO_READY, { status: { phase: 'running' }, onClose: closed });
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(closed).not.toHaveBeenCalled();
  });

  it('lands on Cancel when it opens and gives the focus back when it closes', () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            Move to the Trash
          </button>
          {open && (
            <ConfirmDeleteDialog
              preview={TWO_READY}
              status={{ phase: 'asking' }}
              onConfirm={noop}
              onClose={() => setOpen(false)}
            />
          )}
        </>
      );
    }
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: 'Move to the Trash' });
    // jsdom does not move the focus on a click the way a browser does; the trigger takes
    // it first, exactly as it would have by the time the dialog mounts.
    trigger.focus();
    fireEvent.click(trigger);

    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('keeps Tab inside itself', () => {
    show(TWO_READY);
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    const confirm = confirmButton();

    confirm.focus();
    // jsdom moves no focus on a Tab of its own: what is measured here is the boundary
    // handler — the preventDefault and the focus call that the browser's own Tab would
    // otherwise carry out of the dialog.
    expect(fireEvent.keyDown(confirm, { key: 'Tab' })).toBe(false);
    expect(modeRadio('Trash')).toHaveFocus();

    expect(fireEvent.keyDown(modeRadio('Trash'), { key: 'Tab', shiftKey: true })).toBe(false);
    expect(confirm).toHaveFocus();

    // A Tab in the middle belongs to the browser: the dialog neither stops it nor moves
    // the focus itself.
    cancel.focus();
    expect(fireEvent.keyDown(cancel, { key: 'Tab' })).toBe(true);
    expect(cancel).toHaveFocus();
  });
});

const MOVED = outcomeEntry(`${ROOT}/Downloads`, { result: 'removed', bytes: 2e9 });
const ALSO_MOVED = outcomeEntry(`${ROOT}/src`, { result: 'removed', bytes: 1e9 });

describe('ConfirmDeleteDialog, after the batch', () => {
  it('says what a Trash batch did, counting only what it removed', () => {
    // One of each arm, so that `removed` and `entries.length` cannot agree: a count taken
    // from the whole list would say "Moved 4 items" in the same sentence that reports the
    // bytes of two, and overstate what left the disk on the screen that reports it.
    show(TWO_READY, {
      status: {
        phase: 'done',
        result: batch([
          MOVED,
          ALSO_MOVED,
          outcomeEntry(`${ROOT}/Movies`, { result: 'failed', message: 'Permission denied' }),
          outcomeEntry(`${ROOT}/Pictures`, { result: 'skipped', reason: 'missing' }),
        ]),
      },
    });
    expect(screen.getByTestId('result-summary')).toHaveTextContent(
      'Moved 2 items to the Trash · 3.0 GB',
    );
    expect(screen.queryByTestId('delete-total')).not.toBeInTheDocument();
  });

  it('says what a permanent batch did, in its own words', () => {
    show(TWO_READY, {
      status: { phase: 'done', result: batch([MOVED], { mode: 'permanent' }) },
    });
    expect(screen.getByTestId('result-summary')).toHaveTextContent('Deleted 1 item · 2.0 GB');
  });

  it('takes the freed bytes from the batch rather than adding the rows up again', () => {
    // The entries carry 3.0 GB between them; only the backend knows what the disk gave
    // back, and a summary that added the rows up would print that number instead.
    show(TWO_READY, {
      status: { phase: 'done', result: batch([MOVED, ALSO_MOVED], { freedBytes: 4e9 }) },
    });
    expect(screen.getByTestId('result-summary')).toHaveTextContent(
      'Moved 2 items to the Trash · 4.0 GB',
    );
  });

  it('names every entry that failed, with the message it failed with', () => {
    show(TWO_READY, {
      status: {
        phase: 'done',
        result: batch([
          MOVED,
          outcomeEntry(`${ROOT}/Movies`, {
            result: 'failed',
            message: 'Permission denied (os 13)',
          }),
        ]),
      },
    });
    const failed = within(screen.getByTestId('result-failed')).getAllByRole('listitem');
    expect(failed).toHaveLength(1);
    expect(failed[0]).toHaveTextContent(`${ROOT}/Movies`);
    expect(failed[0]).toHaveTextContent('Permission denied (os 13)');
  });

  it('names every entry the batch skipped, with the reason in words', () => {
    // `missing` and `kindChanged` are decided while the batch runs, so they appear in no
    // preview. A result view that listed failures alone would say "Moved 1 item" and let
    // the user believe the other one went too.
    show(TWO_READY, {
      status: {
        phase: 'done',
        result: batch([
          MOVED,
          outcomeEntry(`${ROOT}/src`, { result: 'skipped', reason: 'kindChanged' }),
        ]),
      },
    });
    const skipped = within(screen.getByTestId('result-skipped')).getAllByRole('listitem');
    expect(skipped).toHaveLength(1);
    expect(skipped[0]).toHaveTextContent(`${ROOT}/src`);
    expect(skipped[0]).toHaveTextContent('No longer what the preview saw');
  });

  it('admits a batch that deleted without being recorded', () => {
    show(TWO_READY, { status: { phase: 'done', result: batch([MOVED], { recorded: false }) } });
    expect(screen.getByTestId('not-recorded')).toHaveTextContent(
      'Deleted, but not recorded: the Activity log has no line for this batch.',
    );
  });

  it('admits that the Explorer may be showing what it deleted', () => {
    show(TWO_READY, { status: { phase: 'done', result: batch([MOVED], { treeStale: true }) } });
    expect(screen.getByTestId('tree-stale')).toHaveTextContent(
      'The Explorer may be stale until the next scan.',
    );
  });

  it('admits neither when the batch was recorded and the tree was patched', () => {
    show(TWO_READY, { status: { phase: 'done', result: batch([MOVED]) } });
    expect(screen.queryByTestId('not-recorded')).not.toBeInTheDocument();
    expect(screen.queryByTestId('tree-stale')).not.toBeInTheDocument();
  });

  it('offers the Trash folder only for a Trash batch, and only to a caller that can open it', () => {
    const shown = vi.fn();
    const trashed = batch([MOVED]);
    const { unmount } = show(TWO_READY, {
      status: { phase: 'done', result: trashed },
      onShowInTrash: shown,
    });
    fireEvent.click(screen.getByRole('button', { name: 'Show in Trash' }));
    expect(shown).toHaveBeenCalledTimes(1);
    // The Trash folder, never a single entry: `move_to_trash` returns no post-move URL.
    expect(screen.queryByText(/Put Back/i)).not.toBeInTheDocument();
    unmount();

    // A caller with no way to resolve the Trash gets no button that lies about opening it.
    const { unmount: second } = show(TWO_READY, { status: { phase: 'done', result: trashed } });
    expect(screen.queryByRole('button', { name: 'Show in Trash' })).not.toBeInTheDocument();
    second();

    show(TWO_READY, {
      status: { phase: 'done', result: batch([MOVED], { mode: 'permanent' }) },
      onShowInTrash: shown,
    });
    expect(screen.queryByRole('button', { name: 'Show in Trash' })).not.toBeInTheDocument();
  });

  it('closes the result view on Escape and on its button', () => {
    const closed = vi.fn();
    show(TWO_READY, { status: { phase: 'done', result: batch([MOVED]) }, onClose: closed });
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(closed).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(closed).toHaveBeenCalledTimes(2);
  });

  it('tells a batch that did not run apart from one that ran badly', () => {
    // A rejected `action_run` means nothing was touched. It must never borrow the words
    // of a batch that deleted and could not say so afterwards.
    show(TWO_READY, { status: { phase: 'failed', message: 'another batch is running' } });
    expect(dialog()).toHaveAccessibleName('The deletion did not run');
    expect(screen.getByTestId('run-error')).toHaveTextContent('another batch is running');
    expect(screen.getByRole('dialog')).toHaveTextContent('Nothing was deleted.');
    expect(screen.queryByTestId('result-summary')).not.toBeInTheDocument();
    expect(screen.queryByTestId('not-recorded')).not.toBeInTheDocument();
    expect(screen.queryByTestId('tree-stale')).not.toBeInTheDocument();
  });
});
