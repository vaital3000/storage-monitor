import { mockIPC } from '@tauri-apps/api/mocks';
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { formatBytes, formatDate, formatDelta, formatPercent } from '../lib/format';
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
import { renderWithClient } from '../test/render';
import ExplorerPage from './ExplorerPage';

const SCAN_NOTE =
  'Scans the home folder. Some folders in Library need Full Disk Access; they are reported, not skipped.';
const MTIME_TITLE = 'Directory modification time, not the newest content';

beforeEach(() => {
  installIpcMock();
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

/** The data rows of the node table, in display order. */
function rows(): HTMLElement[] {
  return within(screen.getByTestId('node-rows')).getAllByRole('row');
}

function nameOf(row: HTMLElement): string {
  return within(row).getAllByRole('cell')[0].textContent ?? '';
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
  return within(row(name)).getAllByRole('cell');
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
    await waitFor(() => {
      expect(screen.getByTestId('scan-current-path')).toHaveTextContent(/\S/);
      expect(screen.getByTestId('scan-files')).not.toHaveTextContent(/^0$/);
    });

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
    ).toHaveAttribute('data-marker', 'lock');

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

  it('rescans from the header and lands on the root of the new tree', async () => {
    await scanned();
    fireEvent.click(row('Library'));
    await waitFor(() => expect(crumbs()).toEqual(['demo', 'Library']));
    fireEvent.click(screen.getByRole('button', { name: 'Rescan' }));
    await waitFor(() => expect(crumbs()).toEqual(['demo']));
    expect(names()[0]).toBe('Library');
  });
});
