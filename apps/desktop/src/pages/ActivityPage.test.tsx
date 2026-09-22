import { mockIPC } from '@tauri-apps/api/mocks';
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import type { ActivityEntry } from '../lib/ipc';
import { createQueryClient } from '../lib/queryClient';
import { mockActionLog } from '../mocks/actionLog';
import { FIXTURE_ROOT } from '../mocks/fixtures';
import { installIpcMock } from '../mocks/ipc';
import { renderWithClient } from '../test/render';
import ActivityPage from './ActivityPage';

beforeEach(() => {
  installIpcMock();
});

// Built from a local date and sent as the instant it is, so the row reads the same in
// every time zone — the trick `format.test.ts` uses for the same reason.
const AT = new Date(2026, 8, 18, 9, 15, 0).toISOString();
const AT_SHOWN = '2026-09-18 09:15:00';

const under = (relative: string) => `${FIXTURE_ROOT}/${relative}`;

/**
 * One line of the record, appended the way the file holds it — JSON text, oldest first —
 * so that a test can stage what the mock's own batches cannot: a failure, a torn line, a
 * reason from a newer backend.
 */
function record(entry: Partial<ActivityEntry> = {}): ActivityEntry {
  const line: ActivityEntry = {
    at: AT,
    path: under('Downloads/q3-report.pdf'),
    kind: 'file',
    mode: 'trash',
    result: 'removed',
    detail: null,
    bytes: 4_000_000_000,
    ...entry,
  };
  mockActionLog.push(JSON.stringify(line));
  return line;
}

const show = () => renderWithClient(<ActivityPage />);

function rows(): HTMLElement[] {
  return within(screen.getByTestId('activity-rows')).getAllByRole('row');
}

/** The cells of a row: when, path, mode, result, size. */
function cells(row: HTMLElement): HTMLElement[] {
  return within(row).getAllByRole('cell');
}

/** The one row naming `path`, found the way a reader finds it: by the path on screen. */
function rowFor(path: string): HTMLElement {
  const found = rows().find((row) => within(row).queryByTitle(path) !== null);
  if (found === undefined) {
    throw new Error(`no row for ${path}`);
  }
  return found;
}

function paths(): string[] {
  // The path line, found by the `title` only it carries rather than by its place in the
  // cell: the detail of a failed or skipped entry sits under it, so the cell's own text is
  // the two run together and its last child is the wrong one. Every test that compares
  // paths stages a row with a detail, so a helper that went back to either would fail.
  return rows().map((row) => within(row).getByTitle(/^\//).textContent ?? '');
}

/** Waits for the record to arrive: every test here starts with the query in flight. */
async function shown(): Promise<HTMLElement[]> {
  await screen.findByTestId('activity-rows');
  return rows();
}

describe('ActivityPage', () => {
  it('says nothing has been deleted when the record is empty', async () => {
    show();
    expect(await screen.findByTestId('activity-empty')).toHaveTextContent('No actions yet');
    expect(screen.queryByTestId('activity-rows')).not.toBeInTheDocument();
    expect(screen.queryByTestId('activity-damaged')).not.toBeInTheDocument();
    // Nor "0 entries" over it: the summary counts what is on screen, and there is no
    // screen to count.
    expect(screen.queryByTestId('activity-summary')).not.toBeInTheDocument();
  });

  it('heads the screen and its columns, which is what says which column is which', async () => {
    // "Trash" in the third cell means "the mode" only because of the row above it, and a
    // header a screen reader cannot reach is not there at all — so the columns are read
    // by role, in order, rather than counted.
    record();
    show();
    await shown();
    expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Activity');
    expect(screen.getAllByRole('columnheader').map((cell) => cell.textContent)).toEqual([
      'When',
      'What',
      'Mode',
      'Result',
      'Size',
    ]);
  });

  it('shows when an entry was deleted, what it was, how, and what it freed', async () => {
    record({ path: under('Movies/holiday.mov'), mode: 'permanent', bytes: 4_000_000_000 });
    show();
    const row = (await shown())[0];
    const cell = cells(row);
    expect(cell[0]).toHaveTextContent(AT_SHOWN);
    expect(cell[1]).toHaveTextContent(under('Movies/holiday.mov'));
    expect(cell[2]).toHaveTextContent('Permanent');
    expect(cell[3]).toHaveTextContent('Removed');
    expect(cell[4]).toHaveTextContent('4.0 GB');
    // The verdict in a value, for a reader that is not reading the words: the e2e specs
    // and anything that later wants to style a row by what became of it.
    expect(row).toHaveAttribute('data-result', 'removed');
  });

  it('reads the record from its end, in the order the log read it and never by `at`', async () => {
    // Two batches, the later line stamped earlier — a clock that moved back, or a machine
    // that woke up. `ActionLog` is explicit that "newest first" is the order of the lines
    // in the file and never the `at` they carry, and a screen that sorted by the stamp
    // would put the older batch on top and claim it was the last thing the user deleted.
    //
    // The second line carries a detail as well, which is what keeps `paths()` honest: a
    // row with two lines in its path cell is the case a helper reading the cell's text,
    // or its last child, gets wrong.
    record({ path: under('first'), at: new Date(2026, 8, 18, 9, 0, 0).toISOString() });
    record({
      path: under('second'),
      at: new Date(2026, 8, 18, 8, 0, 0).toISOString(),
      result: 'skipped',
      detail: 'denylisted',
      bytes: 0,
    });
    show();
    await shown();
    expect(paths()).toEqual([under('second'), under('first')]);
  });

  it('shows the path the record holds, without looking it up anywhere', async () => {
    // What the guards normalized, which under a symlinked scan root is not the spelling
    // the Explorer showed. There is nothing to match it against and nothing tries.
    const normalized = '/System/Volumes/Data/Users/demo/Downloads/q3-report.pdf';
    record({ path: normalized });
    show();
    expect(cells((await shown())[0])[1]).toHaveTextContent(normalized);
  });

  it('shows a failure with the message the record kept', async () => {
    const message = 'cannot delete /Users/demo/locked: Permission denied (os error 13)';
    record({ path: under('locked'), result: 'failed', detail: message, bytes: 0 });
    show();
    await shown();
    const row = rowFor(under('locked'));
    expect(row).toHaveAttribute('data-result', 'failed');
    expect(cells(row)[3]).toHaveTextContent('Failed');
    expect(row).toHaveTextContent(message);
  });

  it('marks a failure as one, and wears the mark', async () => {
    // Both halves, the way `Button.test.tsx` holds `data-variant` to its colours: on its
    // own the attribute pins the decision and would stay truthful with every line painted
    // the same, which is the emphasis a reader of a record of deletions needs most.
    record({ path: under('locked'), result: 'failed', detail: 'no', bytes: 0 });
    record({ path: under('Library'), result: 'skipped', detail: 'denylisted', bytes: 0 });
    show();
    await shown();
    const failure = within(rowFor(under('locked'))).getByTestId('activity-detail');
    const reason = within(rowFor(under('Library'))).getByTestId('activity-detail');
    expect(failure).toHaveAttribute('data-detail', 'failure');
    // Both themes. Half the app's users read it in the other one, and a failure line that
    // keeps `text-red-700` alone falls to about 3.4:1 on the dark panel — under the 4.5:1
    // floor — while the attribute above goes on saying "failure".
    expect(failure).toHaveClass('text-red-700', 'dark:text-red-400');
    expect(reason).toHaveAttribute('data-detail', 'reason');
    // `text-muted` is the app's own utility and carries its dark shade inside it.
    expect(reason).toHaveClass('text-muted');
  });

  it('reads `result` first, so a failure that reads like a reason stays a failure', async () => {
    // The two kinds of string in `detail` cannot be told apart by looking at them, which
    // is why `logDetail` exists. A message that happens to read `denylisted` is a message.
    record({ path: under('odd'), result: 'failed', detail: 'denylisted', bytes: 0 });
    show();
    await shown();
    const row = rowFor(under('odd'));
    expect(row).toHaveTextContent('denylisted');
    expect(row).not.toHaveTextContent('Inside a folder this app never deletes from');
  });

  it('says a skipped entry’s reason in the words the dialog uses', async () => {
    record({ path: under('Library'), result: 'skipped', detail: 'denylisted', bytes: 0 });
    show();
    await shown();
    const row = rowFor(under('Library'));
    expect(cells(row)[3]).toHaveTextContent('Skipped');
    expect(row).toHaveTextContent('Inside a folder this app never deletes from');
    // The wire name is the backend's vocabulary, not a sentence to put in front of a user.
    expect(row).not.toHaveTextContent('denylisted');
  });

  it('names a reason this build does not know instead of showing the wire name', async () => {
    // A ninth `BlockReason`, added in Rust after this build was made: `logDetail` refuses
    // to call it a reason, and the row must still not be left bare.
    record({ path: under('sealed'), result: 'skipped', detail: 'quarantined', bytes: 0 });
    // The other way `logDetail` answers null over a blocked row: a line with no `detail`
    // at all, which `parseLogLine` accepts on purpose — `Option<String>` is a field serde
    // fills in, so a line written by hand does not have to carry it. Both are "this build
    // does not know why", and both get the sentence rather than an empty cell.
    record({ path: under('bare'), result: 'skipped', detail: null, bytes: 0 });
    show();
    await shown();
    const row = rowFor(under('sealed'));
    expect(row).toHaveTextContent('Blocked for a reason this version does not know');
    expect(row).not.toHaveTextContent('quarantined');
    expect(rowFor(under('bare'))).toHaveTextContent(
      'Blocked for a reason this version does not know',
    );
  });

  it('shows a size wherever there is one, and nothing where nothing was freed', async () => {
    record({ path: under('gone'), result: 'removed', bytes: 512 });
    record({ path: under('kept'), result: 'skipped', detail: 'isRoot', bytes: 0 });
    // The case the rule is *for*: a deletion that ran and did not happen. "0 B" here
    // reads as a file that was deleted and happened to be empty.
    record({ path: under('locked'), result: 'failed', detail: 'denied', bytes: 0 });
    // A line that failed and freed bytes is not one the backend writes; the rule is about
    // the number and not about the verdict, so it cannot hide one that is there.
    record({ path: under('half'), result: 'failed', detail: 'half a tree', bytes: 4_096 });
    // Removed and freed nothing — an empty folder — which is a number and not an absence:
    // the row says what it did, and the dash is kept for the rows that did not do it.
    record({ path: under('empty'), result: 'removed', bytes: 0 });
    show();
    await shown();
    expect(cells(rowFor(under('gone')))[4]).toHaveTextContent('512 B');
    expect(cells(rowFor(under('kept')))[4]).toHaveTextContent('—');
    expect(cells(rowFor(under('locked')))[4]).toHaveTextContent('—');
    expect(cells(rowFor(under('half')))[4]).toHaveTextContent('4.1 KB');
    expect(cells(rowFor(under('empty')))[4]).toHaveTextContent('0 B');
  });

  it('leaves the mode blank on a row nothing was done to', async () => {
    // A skipped entry was refused before anything was touched, so "Trash" beside it would
    // say the file is in the Trash. A failed one did go that way and failed going, so it
    // keeps its mode — and a removed one is where the word is load-bearing: it is what
    // makes "Removed" mean *moved, recoverable* rather than *gone*.
    record({ path: under('Library'), result: 'skipped', detail: 'denylisted', bytes: 0 });
    record({ path: under('locked'), result: 'failed', detail: 'denied', bytes: 0 });
    record({ path: under('gone'), result: 'removed', mode: 'trash', bytes: 512 });
    show();
    await shown();
    const skipped = rowFor(under('Library'));
    expect(cells(skipped)[2]).toHaveTextContent('—');
    expect(cells(skipped)[2]).not.toHaveTextContent('Trash');
    expect(skipped).toHaveAttribute('data-result', 'skipped');
    expect(cells(rowFor(under('locked')))[2]).toHaveTextContent('Trash');
    expect(cells(rowFor(under('gone')))[2]).toHaveTextContent('Trash');
  });

  it('shows a stamp it cannot read as the record wrote it', async () => {
    // A leap second: `chrono` writes one and the reader takes it, `Date.parse` answers
    // NaN. The row says the stamp rather than `NaN-NaN-NaN`.
    const leap = '2026-06-30T23:59:60Z';
    record({ at: leap });
    show();
    expect(cells((await shown())[0])[0]).toHaveTextContent(leap);
  });

  it('counts the one line it could not read instead of dropping it', async () => {
    record({ path: under('gone') });
    mockActionLog.push('{ torn');
    show();
    await shown();
    // By role as well as by test id, because the role is the whole difference between a
    // note about the record and a sentence that happens to be near it.
    expect(screen.getByRole('note')).toBe(screen.getByTestId('activity-damaged'));
    expect(screen.getByRole('note')).toHaveTextContent('1 damaged entry hidden');
    expect(rows()).toHaveLength(1);
  });

  it('does not call a record of nothing but damage empty', async () => {
    // Every line unreadable: entries are empty, and "No actions yet" over a log full of
    // the user's deletions is the silent loss `damaged` exists to prevent. Two of them,
    // so that the rule being held is "none at all" and not "exactly one".
    mockActionLog.push('{ torn');
    mockActionLog.push('not json at all');
    show();
    expect(await screen.findByRole('note')).toHaveTextContent('2 damaged entries hidden');
    expect(screen.queryByTestId('activity-empty')).not.toBeInTheDocument();
    expect(screen.queryByTestId('activity-rows')).not.toBeInTheDocument();
  });

  it('says the record could not be read rather than showing an empty one', async () => {
    const message = 'cannot read the action log at /tmp/actions.jsonl: permission denied';
    mockIPC((cmd) => {
      if (cmd === 'activity_log') {
        throw message;
      }
      throw new Error(`Unmocked IPC command: ${cmd}`);
    });
    show();
    expect(await screen.findByRole('alert')).toHaveTextContent(message);
    expect(screen.queryByTestId('activity-empty')).not.toBeInTheDocument();
    expect(screen.queryByTestId('activity-rows')).not.toBeInTheDocument();
  });

  it('reads the record again when the error offers to, and shows it', async () => {
    // The one control on the error screen. A read can fail for a reason that goes away —
    // a network volume that was not mounted yet, `STORAGE_MONITOR_DATA_DIR` on one — and
    // a button that only looks like a way out of it is worse than no button.
    mockIPC((cmd) => {
      if (cmd === 'activity_log') {
        throw 'cannot read the action log at /tmp/actions.jsonl: permission denied';
      }
      throw new Error(`Unmocked IPC command: ${cmd}`);
    });
    show();
    await screen.findByRole('alert');

    installIpcMock();
    record({ path: under('gone') });
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    await shown();
    expect(paths()).toEqual([under('gone')]);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('reads the record again every time the screen is opened', async () => {
    // The shell unmounts this page when another is opened, and the cache outlives it: a
    // batch deleted from the Explorer while this screen is gone has to be here on the way
    // back, and nothing invalidates the query for it.
    const client = createQueryClient();
    record({ path: under('first') });
    const first = renderWithClient(<ActivityPage />, client);
    await shown();
    expect(paths()).toEqual([under('first')]);
    first.unmount();

    record({ path: under('second') });
    renderWithClient(<ActivityPage />, client);
    await waitFor(() => expect(paths()).toEqual([under('second'), under('first')]));
  });

  it('counts what it is showing, and says so when that is only the end of the record', async () => {
    for (let i = 0; i < 201; i += 1) {
      record({ path: under(`file-${i}`) });
    }
    show();
    await waitFor(() => expect(rows()).toHaveLength(200));
    expect(screen.getByTestId('activity-summary')).toHaveTextContent('200 most recent');
    // The oldest line of the 201 is past the end of what was asked for.
    expect(screen.queryByTitle(under('file-0'))).not.toBeInTheDocument();
  });

  it('counts the entries plainly when it is showing all of them', async () => {
    record({ path: under('one') });
    record({ path: under('two') });
    show();
    await shown();
    expect(screen.getByTestId('activity-summary')).toHaveTextContent('2 entries');
    expect(screen.getByTestId('activity-summary')).not.toHaveTextContent('most recent');
  });

  describe('a cleanup line', () => {
    const SANDBOX = `${FIXTURE_ROOT}/Library/Application Support/storage-monitor/demo`;
    const source = {
      module: 'demo',
      item: 'demo:old.object',
      title: 'old.object',
      action: 'Remove object',
    };

    it('shows the item, the module and the action, the path and the command that ran', async () => {
      record({
        path: `${SANDBOX}/old.object`,
        mode: 'permanent',
        bytes: 1_003_520,
        source,
        commands: [['rm', `${SANDBOX}/old.object`]],
      });
      show();
      const row = await screen.findByTitle('old.object');
      const cell = row.closest('td') as HTMLElement;
      // The module's name once the list of modules has answered, and its id before.
      await waitFor(() =>
        expect(within(cell).getByTestId('activity-source')).toHaveTextContent(
          'Demo · Remove object',
        ),
      );
      expect(within(cell).getByTitle(`${SANDBOX}/old.object`)).toBeInTheDocument();
      expect(within(cell).getByTestId('activity-command')).toHaveTextContent(
        `$ rm '${SANDBOX}/old.object'`,
      );
    });

    it('stands on its title when it has no path', async () => {
      mockActionLog.push(
        JSON.stringify({
          at: AT,
          mode: 'trash',
          result: 'skipped',
          detail: 'missing',
          bytes: 0,
          source: { ...source, item: 'demo:gone', title: 'demo:gone' },
        }),
      );
      show();
      const title = await screen.findByTitle('demo:gone');
      expect(title.closest('tr')).toHaveTextContent('Nothing is there any more');
    });

    it('names the home folder where a line of the Explorer names the scanned folder', async () => {
      record({ result: 'skipped', detail: 'outsideRoots', bytes: 0, source });
      show();
      await screen.findByTitle('old.object');
      expect(screen.getByTestId('activity-detail')).toHaveTextContent('Outside the home folder');
    });
  });
});
