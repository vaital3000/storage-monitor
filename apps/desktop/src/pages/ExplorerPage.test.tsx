import { mockIPC } from '@tauri-apps/api/mocks';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { formatBytes, formatDate, formatDelta, formatPercent } from '../lib/format';
import type { BatchResult } from '../lib/ipc';
import {
  DIFFERENT_VOLUME,
  FIXTURE_ROOT,
  PARTIAL_READ,
  PERMISSION_DENIED,
  fixtureDisk,
  fixtureNode,
  fixtureNodeView,
  fixtureStatusDone,
} from '../mocks/fixtures';
import { installIpcMock, revealed, setMockScanDelay } from '../mocks/ipc';
import { charts, lastChart } from '../test/echarts';
import { holdReply, recordCommands, replyOnce } from '../test/invoke';
import { renderWithClient } from '../test/render';
import ExplorerPage from './ExplorerPage';

// jsdom has no canvas: the treemap draws into a recording fake (see src/test/echarts.ts).
vi.mock('echarts/core', async () => (await import('../test/echarts')).echartsCoreMock);
vi.mock('echarts/charts', () => ({ TreemapChart: {} }));
vi.mock('echarts/components', () => ({ TooltipComponent: {} }));
vi.mock('echarts/renderers', () => ({ CanvasRenderer: {} }));

const SCAN_NOTE =
  'Scans the home folder. Some folders in Library need Full Disk Access; they are reported, not skipped.';
const MTIME_TITLE = 'Directory modification time, not the newest content';

beforeEach(() => {
  installIpcMock();
  charts.length = 0;
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

/** The data rows of the node table, in display order. */
function rows(): HTMLElement[] {
  return within(screen.getByTestId('node-rows')).getAllByRole('row');
}

/**
 * The cells of a row that hold data, numbered from the name: name, size, %, Δ, files,
 * modified, actions. The checkbox the page hands the table sits in front of them, so the
 * numbering is taken from the end — where it stays put whether or not a column is added in
 * front of it.
 */
const DATA_CELLS = 7;

function cellsOf(row: HTMLElement): HTMLElement[] {
  return within(row).getAllByRole('cell').slice(-DATA_CELLS);
}

function nameOf(row: HTMLElement): string {
  return cellsOf(row)[0].textContent ?? '';
}

function names(): string[] {
  return rows().map(nameOf);
}

function row(name: string): HTMLElement {
  const found = rows().find((r) => nameOf(r) === name);
  if (found === undefined) {
    throw new Error(`no row named ${name}; rows: ${names().join(', ')}`);
  }
  return found;
}

function cells(name: string): HTMLElement[] {
  return cellsOf(row(name));
}

/** The tick box of a row, by the label the table gives it. */
function box(name: string): HTMLElement {
  return within(row(name)).getByRole('checkbox');
}

function crumbs(): string[] {
  return within(screen.getByRole('navigation', { name: 'Breadcrumb' }))
    .getAllByRole('listitem')
    .map((li) => li.textContent ?? '');
}

/** Renders the page, starts a scan and waits for the table of the root. */
async function scanned(): Promise<void> {
  renderWithClient(<ExplorerPage />);
  fireEvent.click(await screen.findByRole('button', { name: 'Scan' }));
  await screen.findByRole('table');
}

describe('ExplorerPage before a scan', () => {
  it('shows the default root, the Scan button and the Full Disk Access note', async () => {
    renderWithClient(<ExplorerPage />);
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeInTheDocument();
    expect(await screen.findByText(FIXTURE_ROOT)).toBeInTheDocument();
    expect(screen.getByText(SCAN_NOTE)).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('shows why the folder is unknown when the backend cannot name it', async () => {
    replyOnce('default_root', () => {
      throw 'no home directory';
    });
    renderWithClient(<ExplorerPage />);
    expect(await screen.findByText('no home directory')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Scan' })).toBeInTheDocument();
    expect(screen.queryByText('…')).not.toBeInTheDocument();
  });

  it('shows the error and a Retry button when the backend cannot answer', async () => {
    mockIPC(() => {
      throw new Error('boom');
    });
    renderWithClient(<ExplorerPage />);
    expect(await screen.findByText('Error: boom')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Scan' })).not.toBeInTheDocument();
  });
});

describe('ExplorerPage while scanning', () => {
  it('shows the progress with counters, the current path and Cancel, then the table', async () => {
    setMockScanDelay(30);
    renderWithClient(<ExplorerPage />);
    fireEvent.click(await screen.findByRole('button', { name: 'Scan' }));

    expect(await screen.findByRole('progressbar', { name: 'Scanning' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Scan' })).not.toBeInTheDocument();
    // Only the static line is announced; the counters and the path change too often.
    const status = screen.getByRole('status');
    expect(status).toHaveTextContent(/^Scanning…$/);
    expect(within(status).queryByTestId('scan-files')).not.toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId('scan-current-path')).toHaveTextContent(/\S/);
      expect(screen.getByTestId('scan-files')).not.toHaveTextContent(/^0$/);
    });
    expect(screen.getByTestId('scan-current-path').closest('[role="status"]')).toBeNull();

    expect(await screen.findByRole('table', {}, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  });

  it('keeps the partial results of a cancelled scan and says so', async () => {
    setMockScanDelay(30);
    renderWithClient(<ExplorerPage />);
    fireEvent.click(await screen.findByRole('button', { name: 'Scan' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));
    expect(screen.getByRole('button', { name: 'Cancelling…' })).toBeDisabled();

    expect(await screen.findByRole('table')).toBeInTheDocument();
    expect(screen.getByText(/partial results/)).toBeInTheDocument();
    expect(names()[0]).toBe('Library');
  });
});

describe('ExplorerPage after a scan', () => {
  it('lists the root children largest first under a summary header', async () => {
    await scanned();
    const view = fixtureNodeView(0);
    expect(names()).toEqual(view.children.map((c) => c.name));
    expect(names()[0]).toBe('Library');

    const status = fixtureStatusDone();
    expect(screen.getByRole('heading', { name: 'demo' })).toBeInTheDocument();
    const summary = screen.getByTestId('scan-summary');
    expect(summary).toHaveTextContent(formatBytes(status.bytes));
    expect(summary).toHaveTextContent(`${status.files} files`);
    expect(summary).toHaveTextContent('4.8 s');
    expect(summary).toHaveTextContent(`${status.errors} read errors`);
    expect(screen.getByRole('button', { name: 'Rescan' })).toBeInTheDocument();
    expect(screen.queryByText(/partial results/)).not.toBeInTheDocument();
    expect(crumbs()).toEqual(['demo']);
  });

  it('shows the usage of the volume', async () => {
    await scanned();
    const disk = fixtureDisk();
    const meter = await screen.findByRole('meter', { name: 'Disk usage' });
    expect(meter).toHaveAttribute(
      'aria-valuenow',
      String(Math.round((disk.used / disk.total) * 100)),
    );
    expect(screen.getByTestId('disk-usage')).toHaveTextContent(
      `${formatBytes(disk.used)} used of ${formatBytes(disk.total)}`,
    );
    expect(screen.getByTestId('disk-usage')).toHaveTextContent(
      `${formatBytes(disk.available)} available`,
    );
  });

  it('shows size, share, delta, files and date per row', async () => {
    await scanned();
    const view = fixtureNodeView(0);
    const library = view.children.find((c) => c.name === 'Library')!;
    const [, size, percent, delta, files, modified] = cells('Library');
    expect(size).toHaveTextContent(formatBytes(library.size));
    expect(percent).toHaveTextContent(formatPercent(library.size, view.size));
    expect(delta).toHaveTextContent('+6.4 GB');
    expect(delta).toHaveTextContent(formatDelta(library.delta));
    expect(files).toHaveTextContent(String(library.fileCount));
    expect(modified).toHaveTextContent(formatDate(library.mtime));
    expect(modified).toHaveAttribute('title', MTIME_TITLE);

    expect(cells('Downloads')[3]).toHaveTextContent('−4.2 GB');
    expect(cells('Pictures')[3].textContent).toBe('');
    expect(cells('.zshrc')[5]).not.toHaveAttribute('title');
  });

  it('marks unreadable and skipped directories differently', async () => {
    await scanned();
    const lock = within(row('.Trash')).getByRole('img', { name: PERMISSION_DENIED });
    expect(lock).toHaveAttribute('data-marker', 'lock');
    expect(lock).toHaveAttribute('title', PERMISSION_DENIED);

    const info = within(row('OrbStack')).getByRole('img', {
      name: 'Not scanned: different volume',
    });
    expect(info).toHaveAttribute('data-marker', 'info');
    expect(
      within(row('OrbStack')).queryByRole('img', { name: DIFFERENT_VOLUME }),
    ).not.toBeInTheDocument();

    expect(within(row('Library')).queryAllByRole('img')).toHaveLength(0);
  });

  it('drills into a directory row and back through the root crumb', async () => {
    await scanned();
    fireEvent.click(row('Library'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
    const library = fixtureNodeView(fixtureNode('Library').id);
    expect(names()).toEqual(library.children.map((c) => c.name));
    expect(names()[0]).toBe('Developer');

    fireEvent.click(
      within(screen.getByRole('navigation', { name: 'Breadcrumb' })).getByRole('button', {
        name: 'demo',
      }),
    );
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(names()[0]).toBe('Library');
  });

  it('does not navigate into a file', async () => {
    await scanned();
    fireEvent.click(row('.zshrc'));
    fireEvent.keyDown(row('.zshrc'), { key: 'Enter' });
    await settle();
    expect(crumbs()).toEqual(['demo']);
  });

  it('opens a focused directory with Enter and goes up with Backspace', async () => {
    await scanned();
    const library = row('Library');
    library.focus();
    expect(library).toHaveFocus();
    fireEvent.keyDown(library, { key: 'Enter' });
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));

    fireEvent.click(row('Developer'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library', 'Developer']));

    fireEvent.keyDown(document.body, { key: 'Backspace' });
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
    fireEvent.keyDown(document.body, { key: 'Backspace' });
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    fireEvent.keyDown(document.body, { key: 'Backspace' });
    await settle();
    expect(crumbs()).toEqual(['demo']);
  });

  it('leaves Backspace with a modifier key alone', async () => {
    await scanned();
    fireEvent.click(row('Library'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
    for (const modifier of ['metaKey', 'ctrlKey', 'altKey']) {
      fireEvent.keyDown(document.body, { key: 'Backspace', [modifier]: true });
    }
    await settle();
    expect(crumbs()).toEqual(['demo', 'Library']);
  });

  it('focuses the first row after a keyboard navigation, but not after a click', async () => {
    const focus = vi.spyOn(HTMLElement.prototype, 'focus');
    try {
      await scanned();
      fireEvent.pointerDown(row('Library'));
      fireEvent.click(row('Library'));
      await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
      expect(rows()[0]).not.toHaveFocus();

      const developer = row('Developer');
      developer.focus();
      fireEvent.keyDown(developer, { key: 'Enter' });
      await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library', 'Developer']));
      expect(rows()[0]).toHaveFocus();
      expect(focus).toHaveBeenLastCalledWith({ preventScroll: true });

      fireEvent.keyDown(document.body, { key: 'Backspace' });
      await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
      expect(rows()[0]).toHaveFocus();
    } finally {
      focus.mockRestore();
    }
  });

  it('moves the focus between rows with the arrow keys', async () => {
    await scanned();
    const [first, second] = rows();
    first.focus();
    fireEvent.keyDown(first, { key: 'ArrowDown' });
    expect(second).toHaveFocus();
    fireEvent.keyDown(second, { key: 'ArrowUp' });
    expect(first).toHaveFocus();
    fireEvent.keyDown(first, { key: 'ArrowUp' });
    expect(first).toHaveFocus();
  });

  it('keeps a partially readable directory navigable, with its size and a marker', async () => {
    await scanned();
    fireEvent.click(row('Library'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
    const partial = fixtureNode('Library/Application Support');
    expect(cells('Application Support')[1]).toHaveTextContent(formatBytes(partial.size));
    expect(cells('Application Support')[4]).toHaveTextContent(String(partial.fileCount));
    expect(
      within(row('Application Support')).getByRole('img', { name: PARTIAL_READ }),
    ).toHaveAttribute('data-marker', 'partial');

    fireEvent.click(row('Application Support'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library', 'Application Support']));
    expect(names()).toEqual(['Code', 'Slack.db']);
    expect(screen.getByRole('note')).toHaveTextContent(PARTIAL_READ);
  });

  it('explains an empty unreadable directory instead of showing rows', async () => {
    await scanned();
    fireEvent.click(row('.Trash'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', '.Trash']));
    expect(screen.queryByTestId('node-rows')).not.toBeInTheDocument();
    expect(screen.getByRole('table')).toHaveTextContent(PERMISSION_DENIED);
  });

  it('shows the error of a directory that cannot be read and offers the way back', async () => {
    await scanned();
    replyOnce('tree_node', () => {
      throw 'tree is gone';
    });
    fireEvent.click(row('Library'));
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('tree is gone');
    expect(screen.queryByRole('table')).not.toBeInTheDocument();

    fireEvent.click(within(alert).getByRole('button', { name: 'Back to the top' }));
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(names()[0]).toBe('Library');
  });

  it('reveals a row in Finder without navigating', async () => {
    await scanned();
    fireEvent.click(within(row('Downloads')).getByRole('button', { name: 'Reveal in Finder' }));
    await waitFor(() => expect(revealed).toEqual([`${FIXTURE_ROOT}/Downloads`]));
    await settle();
    expect(crumbs()).toEqual(['demo']);
  });

  it('sorts by name on a header click, flips on the second, and back by size', async () => {
    await scanned();
    const bySize = names();
    const byName = [...bySize].sort((a, b) =>
      a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' }),
    );
    expect(screen.getByRole('columnheader', { name: 'Size' })).toHaveAttribute(
      'aria-sort',
      'descending',
    );

    fireEvent.click(screen.getByRole('button', { name: 'Name' }));
    expect(names()).toEqual(byName);
    expect(screen.getByRole('columnheader', { name: 'Name' })).toHaveAttribute(
      'aria-sort',
      'ascending',
    );
    expect(screen.getByRole('columnheader', { name: 'Size' })).not.toHaveAttribute('aria-sort');

    fireEvent.click(screen.getByRole('button', { name: 'Name' }));
    expect(names()).toEqual([...byName].reverse());
    expect(screen.getByRole('columnheader', { name: 'Name' })).toHaveAttribute(
      'aria-sort',
      'descending',
    );

    fireEvent.click(screen.getByRole('button', { name: 'Size' }));
    expect(names()).toEqual(bySize);
  });

  it('sorts by delta with unknown deltas counted as zero', async () => {
    await scanned();
    fireEvent.click(screen.getByRole('button', { name: 'Δ' }));
    expect(names()[0]).toBe('Library');
    expect(names()[names().length - 1]).toBe('Downloads');
  });

  it('draws the treemap of the current directory and navigates from a cell click', async () => {
    await scanned();
    expect(screen.getByTestId('treemap')).toBeInTheDocument();
    const chart = lastChart();
    const option = chart.lastOption() as {
      series: Array<{ data: Array<{ name: string; nodeId: number | null; kind: string }> }>;
    };
    const cells = option.series[0].data;
    expect(cells.map((cell) => cell.name)).toEqual(
      fixtureNodeView(0)
        .children.filter((c) => c.size > 0)
        .map((c) => c.name),
    );

    const library = cells.find((cell) => cell.name === 'Library')!;
    chart.trigger('click', { data: library });
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
    expect(charts).toHaveLength(1);
    const redrawn = chart.lastOption() as typeof option;
    expect(redrawn.series[0].data[0].name).toBe('Developer');
  });

  it('rescans from the header and lands on the root of the new tree', async () => {
    await scanned();
    fireEvent.click(row('Library'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
    fireEvent.click(screen.getByRole('button', { name: 'Rescan' }));
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(names()[0]).toBe('Library');
  });

  it('does not show the old tree while the root of a rescan loads', async () => {
    await scanned();
    fireEvent.click(row('Library'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));

    const release = holdReply('tree_node');
    fireEvent.click(screen.getByRole('button', { name: 'Rescan' }));
    expect(await screen.findByText('Loading…')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Rescan' })).toBeInTheDocument();
    expect(screen.queryByRole('navigation', { name: 'Breadcrumb' })).not.toBeInTheDocument();
    expect(screen.queryByTestId('node-rows')).not.toBeInTheDocument();

    release();
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(names()[0]).toBe('Library');
  });
});

describe('ExplorerPage deleting the selection', () => {
  /** Ticks every row of `rows` and opens the dialog from one of the bar's two buttons. */
  async function ask(rows: string[], button: string): Promise<HTMLElement> {
    for (const name of rows) {
      fireEvent.click(box(name));
    }
    fireEvent.click(screen.getByRole('button', { name: button }));
    return screen.findByRole('dialog');
  }

  /** The dialog's confirming button, which is not the bar's button of the same name. */
  function confirmButton(dialog: HTMLElement): HTMLElement {
    return within(dialog).getByTestId('confirm-delete');
  }

  /** The paths listed in the dialog, in the order it shows them. */
  function listed(dialog: HTMLElement): string[] {
    return within(within(dialog).getByTestId('delete-entries'))
      .getAllByTitle(new RegExp(`^${FIXTURE_ROOT}`))
      .map((entry) => entry.textContent ?? '');
  }

  /** Two clicks React cannot re-render between, which is what a state guard would miss. */
  function clickTwice(button: HTMLElement): void {
    act(() => {
      button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
  }

  it('shows the count and the summed size of the ticked rows, and announces them', async () => {
    await scanned();
    // By role, because a live region is what it is: an element that says its content when
    // the content changes. `data-testid` names it; `role` is what makes it work.
    const announcement = screen.getByRole('status');
    expect(announcement).toHaveAttribute('data-testid', 'selection-status');
    expect(announcement.textContent).toBe('');
    expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument();

    const total = fixtureNode('Downloads').size + fixtureNode('Movies').size;
    fireEvent.click(box('Downloads'));
    fireEvent.click(box('Movies'));
    const bar = screen.getByTestId('selection-bar');
    expect(bar).toHaveTextContent(`2 items selected · ${formatBytes(total)}`);
    // A row cannot carry `aria-selected` while the table is not a grid, so the count is what
    // a screen reader hears when Space ticks a row — once, from the region that is always
    // mounted, and not a second time from the words next to the buttons.
    expect(announcement).toHaveTextContent(`2 items selected · ${formatBytes(total)}`);
    expect(within(bar).getByText(/selected/)).toHaveAttribute('aria-hidden', 'true');

    fireEvent.click(box('Downloads'));
    fireEvent.click(box('Movies'));
    expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument();
    expect(announcement.textContent).toBe('');
  });

  it('opens the dialog over exactly the ticked rows, in the Trash mode', async () => {
    await scanned();
    const total = fixtureNode('Downloads').size + fixtureNode('Movies').size;
    const dialog = await ask(['Downloads', 'Movies'], 'Move to Trash');

    expect(listed(dialog)).toEqual([`${FIXTURE_ROOT}/Downloads`, `${FIXTURE_ROOT}/Movies`]);
    expect(within(dialog).getByTestId('delete-total')).toHaveTextContent(
      `2 items · ${formatBytes(total)}`,
    );
    expect(within(dialog).getByRole('radio', { name: 'Trash' })).toBeChecked();
    expect(confirmButton(dialog)).toBeEnabled();
  });

  it('opens in the Permanent mode from the danger button, still behind the tick', async () => {
    await scanned();
    const dialog = await ask(['Downloads'], 'Delete permanently');

    expect(within(dialog).getByRole('radio', { name: 'Permanent' })).toBeChecked();
    expect(within(dialog).getByTestId('mode-explanation')).toHaveTextContent('cannot be undone');
    expect(confirmButton(dialog)).toBeDisabled();

    fireEvent.click(within(dialog).getByRole('checkbox', { name: /I understand/ }));
    expect(confirmButton(dialog)).toBeEnabled();
  });

  it('deletes the ticked rows, drops the selection and shrinks the header', async () => {
    await scanned();
    const downloads = fixtureNode('Downloads').size;
    const scanned0 = fixtureStatusDone();
    const dialog = await ask(['Downloads'], 'Move to Trash');

    fireEvent.click(confirmButton(dialog));
    expect(await within(dialog).findByTestId('result-summary')).toHaveTextContent(
      `Moved 1 item to the Trash · ${formatBytes(downloads)}`,
    );

    await waitFor(() => expect(names()).not.toContain('Downloads'));
    expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByTestId('scan-summary')).toHaveTextContent(
        formatBytes(scanned0.bytes - downloads),
      ),
    );

    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });

  it('runs the mode the dialog was switched to, not the one it was opened in', async () => {
    await scanned();
    const downloads = fixtureNode('Downloads').size;
    const dialog = await ask(['Downloads'], 'Move to Trash');

    fireEvent.click(within(dialog).getByRole('radio', { name: 'Permanent' }));
    fireEvent.click(within(dialog).getByRole('checkbox', { name: /I understand/ }));
    fireEvent.click(confirmButton(dialog));

    // The report reads `outcome.mode`, which is the mode `action_run` was called with — so
    // this sentence is the one place the page's choice of mode can be seen from outside.
    expect(await within(dialog).findByTestId('result-summary')).toHaveTextContent(
      `Deleted 1 item · ${formatBytes(downloads)}`,
    );
    expect(within(dialog).queryByText(/Moved/)).not.toBeInTheDocument();
  });

  it('holds the dialog on the running batch until the report arrives', async () => {
    await scanned();
    const dialog = await ask(['Downloads'], 'Move to Trash');
    const release = holdReply('action_run');

    fireEvent.click(confirmButton(dialog));
    expect(within(dialog).getByRole('status')).toHaveTextContent('Moving to the Trash…');
    expect(confirmButton(dialog)).toBeDisabled();
    expect(within(dialog).getByRole('button', { name: 'Cancel' })).toBeDisabled();

    release();
    expect(await within(dialog).findByTestId('result-summary')).toBeInTheDocument();
  });

  it('is ready for the next batch once one is done', async () => {
    await scanned();
    const first = await ask(['Downloads'], 'Move to Trash');
    fireEvent.click(confirmButton(first));
    await within(first).findByTestId('result-summary');
    fireEvent.click(within(first).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(names()).not.toContain('Downloads'));

    // The guard that keeps two batches from overlapping is a ref, and a ref nobody puts
    // back is an app that deletes once and then quietly does nothing for the rest of the
    // session — with both buttons still enabled.
    const second = await ask(['Movies'], 'Move to Trash');
    fireEvent.click(confirmButton(second));
    await within(second).findByTestId('result-summary');
    fireEvent.click(within(second).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(names()).not.toContain('Movies'));
  });

  it('re-reads the tree and the volume the batch changed', async () => {
    await scanned();
    const dialog = await ask(['Downloads'], 'Move to Trash');
    const commands = recordCommands();

    fireEvent.click(confirmButton(dialog));
    await within(dialog).findByTestId('result-summary');

    // The cache is keyed by a generation the splice never reaches, so nothing upstream
    // refetches: without these three the Explorer keeps drawing the tree that was thrown
    // away, the disk bar keeps the free space of before, and the header keeps the bytes.
    await waitFor(() => expect(commands).toContain('tree_node'));
    await waitFor(() => expect(commands).toContain('disk_usage'));
    await waitFor(() => expect(commands).toContain('scan_status'));
  });

  it('deletes what it can when one entry is blocked, and names the blocked one', async () => {
    await scanned();
    const downloads = fixtureNode('Downloads').size;
    const dialog = await ask(['Library', 'Downloads'], 'Move to Trash');
    expect(within(dialog).getByTestId('delete-total')).toHaveTextContent('1 blocked');

    fireEvent.click(confirmButton(dialog));
    expect(await within(dialog).findByTestId('result-summary')).toHaveTextContent(
      `Moved 1 item to the Trash · ${formatBytes(downloads)}`,
    );
    const skipped = within(dialog).getByTestId('result-skipped');
    expect(skipped).toHaveTextContent(`${FIXTURE_ROOT}/Library`);
    expect(skipped).toHaveTextContent('Inside a folder this app never deletes from');

    await waitFor(() => expect(names()).not.toContain('Downloads'));
    expect(names()).toContain('Library');
  });

  it('leaves everything alone when the dialog is cancelled', async () => {
    await scanned();
    const dialog = await ask(['Downloads'], 'Move to Trash');
    const commands = recordCommands();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await settle();
    expect(commands).not.toContain('action_run');
    expect(names()).toContain('Downloads');
    // The ticks are still there: nothing was deleted, so nothing about them is stale.
    expect(box('Downloads')).toBeChecked();
    expect(screen.getByTestId('selection-bar')).toBeInTheDocument();
  });

  it('drops the selection when the directory changes', async () => {
    await scanned();
    fireEvent.click(box('Downloads'));
    expect(screen.getByTestId('selection-bar')).toBeInTheDocument();

    fireEvent.click(row('Movies'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Movies']));
    expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument();

    fireEvent.click(
      within(screen.getByRole('navigation', { name: 'Breadcrumb' })).getByRole('button', {
        name: 'demo',
      }),
    );
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(box('Downloads')).not.toBeChecked();
    expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument();
  });

  it('drops the selection when a rescan renumbers the tree', async () => {
    await scanned();
    fireEvent.click(box('Downloads'));
    expect(screen.getByTestId('selection-bar')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Rescan' }));
    await waitFor(() => expect(screen.getByTestId('node-rows')).toBeInTheDocument());
    // A `NodeId` means nothing across a scan: the row that takes id 6 in the new tree is not
    // the row that was ticked in the old one.
    await waitFor(() => expect(box('Downloads')).not.toBeChecked());
    expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument();
  });

  it('says it is checking while the preview is on its way, and checks once', async () => {
    await scanned();
    const release = holdReply('action_preview');
    const commands = recordCommands();
    fireEvent.click(box('Downloads'));
    clickTwice(screen.getByRole('button', { name: 'Move to Trash' }));

    expect(screen.getByTestId('selection-status')).toHaveTextContent(
      'Checking what would be deleted…',
    );
    expect(screen.getByRole('button', { name: 'Move to Trash' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Delete permanently' })).toBeDisabled();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    release();
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByRole('radio', { name: 'Trash' })).toBeChecked();
    expect(commands.filter((cmd) => cmd === 'action_preview')).toHaveLength(1);
  });

  it('says a refused preview for what it is: nothing was deleted', async () => {
    await scanned();
    replyOnce('action_preview', () => {
      throw 'no scan result';
    });
    fireEvent.click(box('Downloads'));
    fireEvent.click(screen.getByRole('button', { name: 'Move to Trash' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Could not check what would be deleted');
    expect(alert).toHaveTextContent('no scan result');
    expect(alert).toHaveTextContent('Nothing was deleted.');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    // The sentence of a batch that ran and failed belongs to a batch that ran.
    expect(screen.queryByText('The deletion did not run')).not.toBeInTheDocument();

    // A second attempt is allowed, and takes the error away with it.
    fireEvent.click(screen.getByRole('button', { name: 'Move to Trash' }));
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(screen.queryByTestId('preview-error')).not.toBeInTheDocument();
  });

  it('keeps the rows and the ticks when the batch itself is refused', async () => {
    await scanned();
    const dialog = await ask(['Downloads'], 'Move to Trash');
    replyOnce('action_run', () => {
      throw 'the volume went away';
    });

    fireEvent.click(confirmButton(dialog));
    expect(await within(dialog).findByTestId('run-error')).toHaveTextContent(
      'the volume went away',
    );
    expect(within(dialog).getByRole('heading', { name: 'The deletion did not run' })).toBeVisible();
    expect(within(dialog).queryByTestId('result-summary')).not.toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(names()).toContain('Downloads');
    expect(box('Downloads')).toBeChecked();
  });

  it('runs one batch, whatever the dialog manages to ask for', async () => {
    await scanned();
    const dialog = await ask(['Downloads'], 'Move to Trash');
    const commands = recordCommands();

    // Two overlapping batches resolve their ids against one generation, and whichever
    // splices second is dropped — leaving rows for entries that are gone. The dialog cannot
    // stop this: it renders the status it is handed, and neither click has been answered.
    clickTwice(confirmButton(dialog));
    await within(dialog).findByTestId('result-summary');
    expect(commands.filter((cmd) => cmd === 'action_run')).toHaveLength(1);
  });

  it('goes back to the top of the tree, because the splice renumbered it', async () => {
    await scanned();
    fireEvent.click(row('Downloads'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Downloads']));
    const dialog = await ask(['q3-report.pdf'], 'Move to Trash');

    fireEvent.click(confirmButton(dialog));
    await within(dialog).findByTestId('result-summary');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));

    // `replace_subtrees` rebuilds the arena and re-sorts every group holding a node whose
    // size changed, so the id this page navigates by can name another directory now. The
    // root is the one id a splice cannot move.
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(names()).toContain('Downloads');
  });

  it('stays where it is when the batch removed nothing', async () => {
    await scanned();
    fireEvent.click(row('Downloads'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Downloads']));
    const dialog = await ask(['q3-report.pdf'], 'Move to Trash');

    // What the guards cannot see coming: an entry that goes missing between the preview and
    // the syscall. Nothing was removed and nothing failed, so `touched` sends the splice no
    // paths at all and every id the page holds still means what it did.
    const skippedOnly: BatchResult = {
      outcome: {
        entries: [
          {
            path: `${FIXTURE_ROOT}/Downloads/q3-report.pdf`,
            kind: 'file',
            result: { result: 'skipped', reason: 'missing' },
          },
        ],
        freedBytes: 0,
        at: '2026-09-18T09:31:00Z',
        mode: 'trash',
      },
      recorded: true,
      treeStale: false,
    };
    replyOnce('action_run', () => skippedOnly);
    const commands = recordCommands();

    fireEvent.click(confirmButton(dialog));
    await within(dialog).findByTestId('result-summary');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
    await settle();

    expect(crumbs()).toEqual(['demo', 'Downloads']);
    expect(names()).toContain('q3-report.pdf');
    expect(commands).not.toContain('tree_node');
    // The selection goes anyway: the question it was asked for has been answered.
    expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument();
  });

  it('patches the tree for an entry that failed, although nothing was removed', async () => {
    await scanned();
    fireEvent.click(row('Downloads'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Downloads']));
    const dialog = await ask(['q3-report.pdf'], 'Move to Trash');

    // A deletion that failed halfway leaves a branch nobody can describe from here, so
    // `touched` sends it to the splice next to the entries that went — and the ids move for
    // a batch that freed not one byte.
    const oneFailed: BatchResult = {
      outcome: {
        entries: [
          {
            path: `${FIXTURE_ROOT}/Downloads/q3-report.pdf`,
            kind: 'file',
            result: { result: 'failed', message: 'Operation not permitted (os error 1)' },
          },
        ],
        freedBytes: 0,
        at: '2026-09-18T09:31:00Z',
        mode: 'trash',
      },
      recorded: true,
      treeStale: false,
    };
    replyOnce('action_run', () => oneFailed);
    const commands = recordCommands();

    fireEvent.click(confirmButton(dialog));
    expect(await within(dialog).findByTestId('result-failed')).toHaveTextContent(
      'Operation not permitted',
    );
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(commands).toContain('tree_node');
  });

  it('leaves Backspace alone while the dialog is open', async () => {
    await scanned();
    fireEvent.click(row('Downloads'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Downloads']));
    await ask(['q3-report.pdf'], 'Move to Trash');

    fireEvent.keyDown(document.body, { key: 'Backspace' });
    await settle();
    expect(crumbs()).toEqual(['demo', 'Downloads']);
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });
});
