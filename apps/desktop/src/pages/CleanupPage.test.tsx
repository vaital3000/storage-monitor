import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { createQueryClient } from '../lib/queryClient';
import { mockActivityTail } from '../mocks/actionLog';
import { setMockCleanupFailure } from '../mocks/cleanup';
import { installIpcMock, revealed } from '../mocks/ipc';
import { DEMO_SANDBOX, setMockModuleFailure } from '../mocks/modules';
import { renderWithClient } from '../test/render';
import CleanupPage from './CleanupPage';

beforeEach(() => {
  installIpcMock();
});

/** Opens the page and waits for the demo's items, which its first open discovers. */
async function open(client = createQueryClient()) {
  const view = renderWithClient(<CleanupPage />, client);
  await screen.findByTestId('item-rows');
  return view;
}

/** The titles of the rows, in the order the table shows them. */
function titles(): string[] {
  return within(screen.getByTestId('item-rows'))
    .getAllByRole('row')
    .map((row) => row.querySelector('[title]')?.textContent ?? '');
}

function row(title: string): HTMLElement {
  const found = within(screen.getByTestId('item-rows'))
    .getAllByRole('row')
    .find((candidate) => candidate.querySelector(`[title="${title}"]`) !== null);
  if (found === undefined) throw new Error(`no row ${title}; rows: ${titles().join(', ')}`);
  return found;
}

function tick(title: string): void {
  fireEvent.click(within(row(title)).getByRole('checkbox'));
}

function verdictFilter(label: RegExp): HTMLElement {
  return within(screen.getByRole('group', { name: 'Verdicts' })).getByRole('button', {
    name: label,
  });
}

function bar(): HTMLElement {
  return screen.getByTestId('cleanup-bar');
}

describe('CleanupPage', () => {
  it('discovers the modules that never ran, and lists what they found without Keep', async () => {
    await open();
    expect(screen.getByTestId('module-demo')).toHaveAttribute('data-status', 'ready');
    expect(screen.getByTestId('module-demo')).toHaveTextContent('5 items · 7.7 MB');
    // Keep is off by default: the design's "do not offer by default".
    expect(titles()).toEqual(['fresh.object', 'build-cache', 'logs', 'old.object']);
    expect(verdictFilter(/Keep/)).toHaveAttribute('aria-pressed', 'false');
    expect(within(row('fresh.object')).getByText('~3.0 MB')).toBeInTheDocument();
    expect(within(row('build-cache')).getByText('Safe')).toBeInTheDocument();
    expect(within(row('build-cache')).getByText('Demo · 4 files')).toBeInTheDocument();
  });

  it('shows Keep when asked, and drops the ticks a filter hides', async () => {
    await open();
    fireEvent.click(verdictFilter(/Keep/));
    expect(titles()).toContain('keepsake');
    tick('keepsake');
    tick('logs');
    expect(bar()).toHaveTextContent('2 items selected');
    fireEvent.click(verdictFilter(/Keep/));
    expect(bar()).toHaveTextContent('1 item selected');
    // And the tick does not come back with the filter.
    fireEvent.click(verdictFilter(/Keep/));
    expect(within(row('keepsake')).getByRole('checkbox')).not.toBeChecked();
  });

  it('sums what the ticked rows promise in the bar, and says it once to assistive technology', async () => {
    await open();
    expect(bar()).toHaveAttribute('aria-hidden', 'true');
    tick('build-cache');
    tick('old.object');
    expect(bar()).not.toHaveAttribute('aria-hidden');
    expect(bar()).toHaveTextContent('2 items selected · 3.0 MB');
    expect(screen.getByRole('status')).toHaveTextContent('2 items selected · 3.0 MB');
  });

  it('shows the focused row in the detail panel: verdict, reasons, facts and the action', async () => {
    await open();
    fireEvent.click(row('build-cache'));
    const detail = screen.getByTestId('item-detail');
    expect(within(detail).getByRole('heading', { name: 'build-cache' })).toBeInTheDocument();
    expect(within(detail).getByTestId('reasons')).toHaveTextContent('Not modified for 45 days');
    const facts = within(detail).getByTestId('facts');
    expect(facts).toHaveTextContent('Files4');
    expect(facts).toHaveTextContent('Marked keepNo');
    expect(within(detail).getByText('Delete folder')).toBeInTheDocument();
    fireEvent.click(within(detail).getByRole('button', { name: 'Reveal in Finder' }));
    expect(revealed).toEqual([`${DEMO_SANDBOX}/build-cache`]);
  });

  it('cleans the ticked rows through the dialog, and they are gone afterwards', async () => {
    const client = createQueryClient();
    // A tree query of the Explorer's, from a scan before this batch.
    client.setQueryData(['treeNode', 1, 0], { stale: false });
    await open(client);
    tick('build-cache');
    tick('old.object');
    fireEvent.click(within(bar()).getByRole('button', { name: 'Clean…' }));

    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveAccessibleName('Clean 2 items?');
    expect(dialog).toHaveTextContent(`Move to the Trash${DEMO_SANDBOX}/build-cache`);
    // The object is removed by a command in either mode, so the Trash mode asks too.
    fireEvent.click(within(dialog).getByRole('checkbox'));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Clean' }));

    expect(await screen.findByTestId('result-summary')).toHaveTextContent(
      'Deleted 1 item and moved 1 to the Trash · 3.0 MB',
    );
    expect(client.getQueryState(['treeNode', 1, 0])?.isInvalidated).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(titles()).toEqual(['fresh.object', 'logs']));
    expect(bar()).toHaveAttribute('aria-hidden', 'true');
    expect(mockActivityTail(10).entries.map((entry) => entry.source?.title)).toEqual([
      'old.object',
      'build-cache',
    ]);
  });

  it('refuses a keep item until its force option is turned on in the detail panel', async () => {
    await open();
    fireEvent.click(verdictFilter(/Keep/));
    fireEvent.click(row('keepsake'));
    const detail = screen.getByTestId('item-detail');
    fireEvent.click(within(detail).getByRole('button', { name: 'Clean…' }));
    let dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByTestId('block-reason')).toHaveTextContent(
      'Marked keep — turn on its force option to clean it anyway',
    );
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));

    fireEvent.click(
      within(detail).getByRole('checkbox', { name: /Delete it although it is marked keep/ }),
    );
    fireEvent.click(within(detail).getByRole('button', { name: 'Clean…' }));
    dialog = await screen.findByRole('dialog');
    expect(within(dialog).queryByTestId('block-reason')).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Clean' }));
    expect(await screen.findByTestId('result-summary')).toHaveTextContent(
      'Moved 1 item to the Trash',
    );
  });

  it('reports a failure without calling the batch a failure', async () => {
    setMockCleanupFailure('demo:logs', 'Permission denied');
    await open();
    tick('logs');
    fireEvent.click(within(bar()).getByRole('button', { name: 'Clean…' }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Clean' }));
    const failed = await screen.findByTestId('result-failed');
    expect(failed).toHaveTextContent('Could not be cleaned');
    expect(failed).toHaveTextContent('logs');
    expect(failed).toHaveTextContent('Permission denied');
  });

  it('says what a failed discovery said, keeps what it had, and offers to try again', async () => {
    await open();
    setMockModuleFailure('demo', 'the daemon did not answer');
    fireEvent.click(screen.getByRole('button', { name: /Refresh/ }));
    const strip = screen.getByTestId('module-demo');
    await waitFor(() => expect(strip).toHaveAttribute('data-status', 'failed'));
    expect(strip).toHaveTextContent('Failed: the daemon did not answer');
    expect(titles()).toHaveLength(4);
    setMockModuleFailure('demo', null);
    fireEvent.click(within(strip).getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(strip).toHaveAttribute('data-status', 'ready'));
  });
});
