import { fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import type { Item, VerdictLevel } from '../lib/ipc';
import ItemDetail from './ItemDetail';
import ItemTable from './ItemTable';

function item(name: string, bytes: number, level: VerdictLevel = 'safe', estimated = false): Item {
  return {
    id: `demo:${name}`,
    module: 'demo',
    kind: 'folder',
    title: name,
    subtitle: '2 files',
    path: `/x/${name}`,
    size: { bytes, estimated },
    lastUsed: '2026-08-04T09:30:00Z',
    verdict: { level, reasons: [{ code: 'stale', text: 'Not modified for 45 days' }] },
    facts: [],
    actions: [
      {
        id: 'delete',
        label: 'Delete folder',
        estimatedFree: bytes,
        options:
          level === 'keep'
            ? [{ id: 'force', label: 'Delete it anyway', default: false, force: true }]
            : [],
      },
    ],
  };
}

const ITEMS = [item('a', 3e6, 'safe', true), item('b', 2e6), item('c', 1e6), item('d', 5e5)];

/** The table over a selection of its own, the way the page holds one. */
function Harness({ onFocus = () => undefined }: { onFocus?: (id: string) => void }) {
  const [selection, setSelection] = useState<ReadonlySet<string>>(new Set());
  return (
    <>
      <p data-testid="ticked">{[...selection].sort().join(',')}</p>
      <ItemTable
        items={ITEMS}
        total={ITEMS.length}
        moduleNames={new Map([['demo', 'Demo']])}
        selection={selection}
        onSelectionChange={setSelection}
        focused={null}
        onFocus={onFocus}
      />
    </>
  );
}

function rowOf(title: string): HTMLElement {
  return screen.getByTitle(title).closest('tr') as HTMLElement;
}

const ticked = () => screen.getByTestId('ticked').textContent;

describe('ItemTable', () => {
  it('draws the size, estimated or not, the verdict and the last use', () => {
    render(<Harness />);
    expect(within(rowOf('a')).getByText('~3.0 MB')).toBeInTheDocument();
    expect(within(rowOf('b')).getByText('2.0 MB')).toBeInTheDocument();
    expect(within(rowOf('b')).getByText('Safe')).toHaveAttribute('data-level', 'safe');
    expect(within(rowOf('b')).getByText('2026-08-04')).toBeInTheDocument();
  });

  it('ticks a row on Space, and shows it on Enter or a click', () => {
    const onFocus = vi.fn();
    render(<Harness onFocus={onFocus} />);
    fireEvent.keyDown(rowOf('b'), { key: ' ' });
    expect(ticked()).toBe('demo:b');
    fireEvent.keyDown(rowOf('c'), { key: 'Enter' });
    fireEvent.click(rowOf('d'));
    expect(onFocus.mock.calls).toEqual([['demo:c'], ['demo:d']]);
    // A click on the box ticks and does not move the detail panel.
    fireEvent.click(within(rowOf('a')).getByRole('checkbox'));
    expect(onFocus).toHaveBeenCalledTimes(2);
    expect(ticked()).toBe('demo:a,demo:b');
  });

  it('takes in a range on a shift-click, from the last row ticked', () => {
    render(<Harness />);
    fireEvent.click(within(rowOf('a')).getByRole('checkbox'));
    fireEvent.click(within(rowOf('c')).getByRole('checkbox'), { shiftKey: true });
    expect(ticked()).toBe('demo:a,demo:b,demo:c');
  });

  it('ticks and unticks everything shown from the header', () => {
    render(<Harness />);
    const all = screen.getByRole('checkbox', { name: 'Select all' });
    fireEvent.click(all);
    expect(ticked()).toBe('demo:a,demo:b,demo:c,demo:d');
    fireEvent.click(all);
    expect(ticked()).toBe('');
  });
});

describe('ItemDetail', () => {
  it('draws a force option in the danger style, saying what it overrides', () => {
    const onChoice = vi.fn();
    const keep = item('k', 1e6, 'keep');
    render(
      <ItemDetail
        item={keep}
        moduleName="Demo"
        choice={{ action: 'delete', options: [] }}
        onChoice={onChoice}
        onReveal={() => undefined}
        onClean={() => undefined}
        busy={false}
      />,
    );
    const force = screen.getByRole('checkbox', { name: /Delete it anyway/ });
    expect(force.closest('label')).toHaveClass('text-red-700');
    expect(force.closest('label')).toHaveTextContent('Overrides the Keep verdict of this item.');
    fireEvent.click(force);
    expect(onChoice).toHaveBeenCalledWith({ action: 'delete', options: ['force'] });
  });
});
