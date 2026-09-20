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
import { tabStops } from '../lib/focusTrap';
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
  initialMode?: DeletionMode;
  onConfirm?: (mode: DeletionMode) => void;
  onClose?: () => void;
}

const noop = () => undefined;

/** Renders the dialog, and can put it through a phase change without remounting it. */
function show(preview: DialogPreview, options: ShowOptions = {}) {
  const { status = { phase: 'asking' }, onConfirm = noop, onClose = noop, initialMode } = options;
  const dialogFor = (phase: BatchStatus) => (
    <ConfirmDeleteDialog
      preview={preview}
      status={phase}
      initialMode={initialMode}
      onConfirm={onConfirm}
      onClose={onClose}
    />
  );
  const view = render(dialogFor(status));
  return {
    ...view,
    setStatus: (next: BatchStatus) => view.rerender(dialogFor(next)),
  };
}

function dialog(): HTMLElement {
  return screen.getByRole('dialog');
}

function entryItems(): HTMLElement[] {
  return within(screen.getByTestId('delete-entries')).getAllByRole('listitem');
}

/** The path each row shows, in the order the dialog lists them. */
function shownPaths(): string[] {
  return entryItems().map((item) => item.querySelector('[title]')?.textContent ?? '');
}

function itemFor(path: string): HTMLElement {
  const item = entryItems().find((candidate) => within(candidate).queryByText(path) !== null);
  if (item === undefined) {
    throw new Error(`no entry for ${path}; entries: ${shownPaths().join(', ')}`);
  }
  return item;
}

/**
 * The dialog's tab ring, named so a test can read it: a radio by the mode it selects, the
 * acknowledgement as "checkbox", everything else by its label or its words.
 */
function ring(): string[] {
  return tabStops(dialog()).map((element) => {
    if (element instanceof HTMLInputElement) {
      return element.type === 'radio' ? `radio:${element.value}` : 'checkbox';
    }
    return element.getAttribute('aria-label') ?? element.textContent ?? '';
  });
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

function cancelButton(): HTMLElement {
  return screen.getByRole('button', { name: 'Cancel' });
}

function selectMode(label: string): void {
  fireEvent.click(modeRadio(label));
}

describe('ConfirmDeleteDialog', () => {
  it('is a modal dialog named by its own heading', () => {
    show(TWO_READY);
    expect(dialog()).toHaveAttribute('aria-modal', 'true');
    expect(dialog()).toHaveAttribute('aria-busy', 'false');
    expect(dialog()).toHaveAccessibleName('Move 2 items to the Trash?');
  });

  it('lists an entry by its path, because two selected folders can carry one name', () => {
    // The fixture holds `/Users/demo/Downloads` and `/Users/demo/src/Downloads`. This is
    // the last screen before an irreversible action: a list of bare names would not say
    // which of the two is about to go.
    show(previewOf([ready(`${ROOT}/Downloads`, 1e9), ready(`${ROOT}/src/Downloads`, 2e9)]));
    expect(shownPaths()).toEqual([`${ROOT}/Downloads`, `${ROOT}/src/Downloads`]);
  });

  it('lists an exact duplicate twice, the way the backend reports it', () => {
    // Selecting one row twice reaches the guards as two entries, the second blocked as
    // `nested` (`engine.rs`). Keyed by path, React would drop one of the two rows of the
    // batch the user is confirming — and say so only in a console warning.
    const errors: unknown[][] = [];
    const console_error = vi.spyOn(console, 'error').mockImplementation((...args: unknown[]) => {
      errors.push(args);
    });
    show(previewOf([ready(`${ROOT}/src`, 1e9), blocked(`${ROOT}/src`, 'nested', 1e9)]));
    console_error.mockRestore();

    expect(shownPaths()).toEqual([`${ROOT}/src`, `${ROOT}/src`]);
    expect(entryItems()[0]).toHaveAttribute('data-state', 'ready');
    expect(entryItems()[1]).toHaveAttribute('data-state', 'blocked');
    expect(JSON.stringify(errors)).not.toContain('same key');
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

  it('says each of the eight block reasons in its own words', () => {
    // Pinned pairwise, and not by a shape a permutation would also satisfy. The pair the
    // backend most insists on is `missing` against `unreadable` (`ipc.ts`): one sends the
    // user hunting for a file that is gone, the other to grant Full Disk Access.
    const words: Record<BlockReason, string> = {
      outsideRoots: 'Outside the folder that was scanned',
      denylisted: 'Inside a folder this app never deletes from',
      malformed: 'Not a path that names an entry',
      isRoot: 'The scanned folder itself, or one above it',
      nested: 'Another entry contains it',
      missing: 'Nothing is there any more',
      unreadable: 'Cannot be read — it may need Full Disk Access',
      kindChanged: 'No longer what the preview saw',
    };
    // A ninth reason mirrored into `ipc.ts` has to arrive here too, rather than falling
    // through to the unknown-variant line that exists for older builds in the wild.
    expect(Object.keys(words).sort()).toEqual([...BLOCK_REASONS].sort());

    for (const reason of BLOCK_REASONS) {
      const { unmount } = show(previewOf([blocked(`${ROOT}/thing`, reason)]));
      expect(within(itemFor(`${ROOT}/thing`)).getByTestId('block-reason')).toHaveTextContent(
        words[reason],
      );
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
    // The heading counts what will go, like the total under it — three rows are listed.
    expect(dialog()).toHaveAccessibleName('Move 2 items to the Trash?');
    expect(entryItems()).toHaveLength(3);
  });

  it('takes the total from the backend rather than adding the rows up again', () => {
    // The two numbers agree in practice; they are made to disagree here because only the
    // backend saw the real sizes, and the dialog is not allowed to have an opinion about
    // them. A dialog that sums what it renders would print 3.0 GB.
    show(previewOf([ready(`${ROOT}/Downloads`, 2e9), ready(`${ROOT}/src`, 1e9)], 7_000_000_000));
    // Anchored: with nothing blocked the line ends there, rather than admitting "0 blocked".
    expect(screen.getByTestId('delete-total')).toHaveTextContent(/^2 items · 7\.0 GB$/);
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

  it('opens on the mode the caller asked for, still behind the acknowledgement', () => {
    // The action bar has a "Delete permanently" entry point of its own; a dialog that
    // opened on the Trash would be answering a question nobody asked. Safety is unchanged.
    const confirmed = vi.fn<(mode: DeletionMode) => void>();
    show(TWO_READY, { initialMode: 'permanent', onConfirm: confirmed });
    expect(dialog()).toHaveAccessibleName('Delete 2 items permanently?');
    expect(modeRadio('Permanent')).toBeChecked();
    expect(understandBox()).not.toBeChecked();
    expect(confirmButton()).toBeDisabled();

    fireEvent.click(understandBox());
    fireEvent.click(confirmButton());
    expect(confirmed).toHaveBeenCalledExactlyOnceWith('permanent');
  });

  it('wears the danger style only where it means a permanent deletion', () => {
    // The emphasis of `primary` says "this is the way on", which is the right thing for a
    // reversible move to the Trash and the wrong thing for the most dangerous button on
    // the screen — the more so since the entry point that opened this dialog is red.
    // `data-variant` pins the choice; what red looks like stays the stylesheet's business.
    show(TWO_READY);
    expect(confirmButton()).toHaveAttribute('data-variant', 'primary');

    selectMode('Permanent');
    expect(confirmButton()).toHaveAttribute('data-variant', 'danger');

    selectMode('Trash');
    expect(confirmButton()).toHaveAttribute('data-variant', 'primary');
  });

  it('holds the permanent deletion until the user says they understand it', () => {
    const confirmed = vi.fn<(mode: DeletionMode) => void>();
    show(TWO_READY, { onConfirm: confirmed });

    selectMode('Permanent');
    expect(screen.getByTestId('mode-explanation')).toHaveTextContent(
      'Items are deleted immediately. This cannot be undone.',
    );
    // Not "Delete permanently", which is what the action bar's entry point says: the two
    // are the same red, and two identical red buttons on one screen is a mis-click.
    expect(confirmButton()).toHaveTextContent('Delete for good');
    expect(confirmButton()).toBeDisabled();

    fireEvent.click(confirmButton());
    expect(confirmed).not.toHaveBeenCalled();

    fireEvent.click(understandBox());
    expect(confirmButton()).toBeEnabled();
    fireEvent.click(confirmButton());
    expect(confirmed).toHaveBeenCalledExactlyOnceWith('permanent');
  });

  it('disarms the button again when the acknowledgement is taken back', () => {
    // The tick is a switch, not a door that only opens: a user who reads the list again
    // and changes their mind has to be able to put the safety back on.
    show(TWO_READY, { initialMode: 'permanent' });
    fireEvent.click(understandBox());
    expect(confirmButton()).toBeEnabled();

    fireEvent.click(understandBox());
    expect(understandBox()).not.toBeChecked();
    expect(confirmButton()).toBeDisabled();
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

  it('forgets the acknowledgement when a run that failed brings the question back', () => {
    // The same rule as the mode toggle's, for the same reason: a tick belongs to the batch
    // it was given for. A rejected `action_run` returns to this view, and the button must
    // not still be armed by an agreement made before a batch that never ran.
    const view = show(TWO_READY, { initialMode: 'permanent' });
    fireEvent.click(understandBox());
    expect(confirmButton()).toBeEnabled();

    // Out of the question and back is enough; it does not matter what happened while it
    // was away, and the rule must not depend on which phase it went through.
    view.setStatus({ phase: 'running' });
    view.setStatus({ phase: 'asking' });
    expect(understandBox()).not.toBeChecked();

    fireEvent.click(understandBox());
    view.setStatus({ phase: 'running' });
    view.setStatus({ phase: 'failed', message: 'another batch is running' });
    view.setStatus({ phase: 'asking' });

    expect(dialog()).toHaveAccessibleName('Delete 2 items permanently?');
    expect(understandBox()).not.toBeChecked();
    expect(confirmButton()).toBeDisabled();
  });

  it('forgets the acknowledgement on a route back that never ran anything', () => {
    // asking -> failed -> asking, with no running in between. The rule is about the
    // question coming back, not about the phase it travelled through: a batch that was
    // refused outright is exactly when a user is likeliest to press the button again.
    const view = show(TWO_READY, { initialMode: 'permanent' });
    fireEvent.click(understandBox());
    view.setStatus({ phase: 'failed', message: 'another batch is running' });
    view.setStatus({ phase: 'asking' });
    expect(understandBox()).not.toBeChecked();
    expect(confirmButton()).toBeDisabled();
  });

  it('keeps the tick on screen while the batch it authorised runs', () => {
    // Cleared on the way back in, not on the way out: an unticked "I understand" above
    // "Deleting…" reads like the app forgetting why it is deleting.
    const view = show(TWO_READY, { initialMode: 'permanent' });
    fireEvent.click(understandBox());
    view.setStatus({ phase: 'running' });
    expect(understandBox()).toBeChecked();
    expect(understandBox()).toBeDisabled();
  });

  it('speaks the selected mode, not the one the preview was computed with', () => {
    // A `Preview` carries the mode it was checked in, and the toggle is local: the guards
    // never look at the mode, so no verdict here changes with it. The preview below says
    // `permanent` while the dialog must open on the Trash — a dialog that seeded itself
    // from that field, or explained itself from it, would be arming one thing and
    // describing another.
    const confirmed = vi.fn<(mode: DeletionMode) => void>();
    const asChecked: Preview = { ...TWO_READY, mode: 'permanent' };
    show(asChecked, { onConfirm: confirmed });

    expect(modeRadio('Trash')).toBeChecked();
    expect(dialog()).toHaveAccessibleName('Move 2 items to the Trash?');
    expect(screen.getByTestId('mode-explanation')).toHaveTextContent('Space is freed when you');
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();

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

  it('refuses a batch in which nothing can be deleted, and promises no bytes back', () => {
    show(previewOf([blocked(`${ROOT}/Library`, 'denylisted'), blocked('/', 'malformed')]));
    expect(confirmButton()).toBeDisabled();
    expect(screen.getByTestId('delete-total')).toHaveTextContent('0 items · 0 B · 2 blocked');
  });

  it('cancels on the button and on Escape', () => {
    const closed = vi.fn();
    show(TWO_READY, { onClose: closed });
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(closed).toHaveBeenCalledTimes(1);

    fireEvent.click(cancelButton());
    expect(closed).toHaveBeenCalledTimes(2);
  });

  it('is not dismissed by a click beside the panel, and still hears Escape afterwards', () => {
    const closed = vi.fn();
    show(TWO_READY, { onClose: closed });
    fireEvent.click(screen.getByTestId('delete-backdrop'));
    expect(closed).not.toHaveBeenCalled();
    expect(dialog()).toBeInTheDocument();

    // jsdom does not move the focus on a click; a real click on the backdrop blurs to the
    // body, which is what makes this worth testing — a listener on the panel would never
    // hear another key.
    (document.activeElement as HTMLElement | null)?.blur();
    fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it('disables every control while the batch runs and says that it is running', () => {
    show(TWO_READY, { status: { phase: 'running' } });
    expect(dialog()).toHaveAttribute('aria-busy', 'true');
    expect(screen.getByRole('status')).toHaveTextContent('Moving to the Trash…');
    expect(confirmButton()).toBeDisabled();
    expect(cancelButton()).toBeDisabled();
    expect(modeRadio('Permanent')).toBeDisabled();
  });

  it('says which kind of deletion is running', () => {
    show(TWO_READY, { initialMode: 'permanent', status: { phase: 'running' } });
    expect(screen.getByRole('status')).toHaveTextContent('Deleting…');
    expect(understandBox()).toBeDisabled();
  });

  it('ignores Escape when the batch starts under an already open dialog', () => {
    // Every other Escape test mounts straight into the phase it measures. This is the
    // route a user actually takes, and the one a handler that read `running` once, when it
    // was still false, gets wrong — closing the dialog mid-batch and throwing away the
    // only report of `recorded` and `treeStale`.
    const closed = vi.fn();
    const view = show(TWO_READY, { onClose: closed });
    view.setStatus({ phase: 'running' });
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(closed).not.toHaveBeenCalled();

    // And hears it again as soon as there is a report to dismiss.
    view.setStatus({ phase: 'done', result: batch([MOVED]) });
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(closed).toHaveBeenCalledTimes(1);
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

    expect(cancelButton()).toHaveFocus();
    fireEvent.keyDown(dialog(), { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('moves the focus with the phase, and never leaves it on the page behind', () => {
    const view = show(TWO_READY);
    expect(cancelButton()).toHaveFocus();

    // Every control is disabled now, and the button that had the focus is one of them: a
    // browser drops the focus to the body, from where Tab walks into the Explorer.
    view.setStatus({ phase: 'running' });
    expect(dialog()).toHaveFocus();

    view.setStatus({ phase: 'done', result: batch([]) });
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
  });

  it('keeps Tab inside itself', () => {
    show(TWO_READY);
    const list = screen.getByTestId('delete-entries');
    const confirm = confirmButton();

    confirm.focus();
    // jsdom performs no Tab of its own — `fireEvent` dispatches the key and nothing else —
    // so what is measured here is the boundary handler: the preventDefault and the focus
    // call that stand in for the step a browser would otherwise take out of the dialog.
    expect(fireEvent.keyDown(confirm, { key: 'Tab' })).toBe(false);
    expect(list).toHaveFocus();

    expect(fireEvent.keyDown(list, { key: 'Tab', shiftKey: true })).toBe(false);
    expect(confirm).toHaveFocus();

    // A Tab in the middle belongs to the browser: the dialog neither stops it nor moves
    // the focus itself.
    cancelButton().focus();
    expect(fireEvent.keyDown(cancelButton(), { key: 'Tab' })).toBe(true);
    expect(cancelButton()).toHaveFocus();
  });

  it('keeps Tab inside itself while the batch runs, from the panel that holds the focus', () => {
    // The phase where modality matters most is the one where every control is disabled.
    // The list of entries stays reachable — a user may still want to read what is going —
    // and the focus sits on the panel, from where a browser would step backwards into the
    // page behind the overlay.
    show(TWO_READY, { status: { phase: 'running' } });
    expect(ring()).toEqual(['Entries to delete']);
    expect(dialog()).toHaveFocus();

    const list = screen.getByTestId('delete-entries');
    expect(fireEvent.keyDown(dialog(), { key: 'Tab' })).toBe(false);
    expect(list).toHaveFocus();

    // One stop is both ends of the ring: Tab and Shift+Tab both come back to it.
    expect(fireEvent.keyDown(list, { key: 'Tab', shiftKey: true })).toBe(false);
    expect(list).toHaveFocus();
  });

  it('takes Shift+Tab back from the panel too', () => {
    show(TWO_READY, { status: { phase: 'running' } });
    expect(dialog()).toHaveFocus();
    expect(fireEvent.keyDown(dialog(), { key: 'Tab', shiftKey: true })).toBe(false);
    expect(screen.getByTestId('delete-entries')).toHaveFocus();
  });

  it('takes Tab back to the end the direction asks for when the focus has fallen out', () => {
    show(TWO_READY);
    (document.activeElement as HTMLElement | null)?.blur();
    expect(fireEvent.keyDown(document.body, { key: 'Tab' })).toBe(false);
    expect(screen.getByTestId('delete-entries')).toHaveFocus();

    (document.activeElement as HTMLElement | null)?.blur();
    expect(fireEvent.keyDown(document.body, { key: 'Tab', shiftKey: true })).toBe(false);
    expect(confirmButton()).toHaveFocus();
  });

  it('never takes a tab stop from the page it covers', () => {
    // The ring is read inside the panel, and the trap's two ends have to be elements of
    // the dialog: an end found on the page behind it is the trap handing the focus out
    // through its own boundary.
    render(
      <>
        <button type="button">Behind</button>
        <ConfirmDeleteDialog
          preview={TWO_READY}
          status={{ phase: 'asking' }}
          onConfirm={noop}
          onClose={noop}
        />
      </>,
    );
    expect(tabStops(dialog())).not.toContain(screen.getByRole('button', { name: 'Behind' }));
    expect(ring()).toEqual(['Entries to delete', 'radio:trash', 'Cancel', 'Move to the Trash']);

    confirmButton().focus();
    expect(fireEvent.keyDown(confirmButton(), { key: 'Tab' })).toBe(false);
    expect(screen.getByTestId('delete-entries')).toHaveFocus();
  });

  it('counts a radio group as the one stop the browser stops at, and skips disabled buttons', () => {
    // The ring is asserted rather than tabbed through: jsdom moves no focus on Tab, so the
    // membership and the order of the two ends cannot be seen from the outside. An
    // unchecked radio is not a tab stop — a boundary taken from one lets Shift+Tab step
    // backwards out of the dialog — and neither is a button that is disabled.
    show(TWO_READY);
    expect(ring()).toEqual(['Entries to delete', 'radio:trash', 'Cancel', 'Move to the Trash']);

    selectMode('Permanent');
    expect(ring()).toEqual(['Entries to delete', 'radio:permanent', 'checkbox', 'Cancel']);

    fireEvent.click(understandBox());
    expect(ring()).toEqual([
      'Entries to delete',
      'radio:permanent',
      'checkbox',
      'Cancel',
      'Delete for good',
    ]);
  });

  it('gives the list of entries a key to scroll it by', () => {
    // Inside a focus trap there is no other way to reach it, and a batch of two hundred
    // entries is exactly when a user wants to read to the end before confirming.
    show(TWO_READY);
    const list = screen.getByTestId('delete-entries');
    expect(list).toHaveAttribute('tabindex', '0');
    expect(list).toHaveAccessibleName('Entries to delete');
    expect(list).toHaveClass('overflow-y-auto');
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
    // `NsFileManager` may leave no "Put Back" entry, so the app never promises one
    // (design section 11).
    expect(screen.queryByText(/Put Back/i)).not.toBeInTheDocument();
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
    // The heading, not the colour, is what says which list this is: a failure may have
    // torn a tree half down, where a skip touched nothing at all.
    expect(screen.getByRole('heading', { name: 'Could not be deleted' })).toBeInTheDocument();
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
    expect(screen.getByRole('heading', { name: 'Skipped' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Could not be deleted' })).not.toBeInTheDocument();
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
    // And no empty groups under the summary: a heading over nothing is a question the
    // user then has to answer by counting rows.
    expect(screen.queryByTestId('result-failed')).not.toBeInTheDocument();
    expect(screen.queryByTestId('result-skipped')).not.toBeInTheDocument();
  });

  it('gives the report a key to scroll it by', () => {
    show(TWO_READY, {
      status: {
        phase: 'done',
        result: batch([outcomeEntry(`${ROOT}/Movies`, { result: 'failed', message: 'denied' })]),
      },
    });
    const report = screen.getByRole('group', { name: 'What the batch did' });
    expect(report).toHaveAttribute('tabindex', '0');
    expect(report).toHaveClass('overflow-y-auto');
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
