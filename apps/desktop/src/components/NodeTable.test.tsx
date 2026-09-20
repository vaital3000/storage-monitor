import { fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it, vi, type Mock } from 'vitest';
import type { NodeId, NodeView } from '../lib/ipc';
import {
  FIXTURE_ROOT,
  PARTIAL_READ,
  PERMISSION_DENIED,
  fixtureNode,
  fixtureNodeView,
  fixtureNodes,
} from '../mocks/fixtures';
import NodeTable from './NodeTable';

const noop = () => undefined;

describe('NodeTable', () => {
  it('says how many children there are and how many of them are shown', () => {
    const node = fixtureNodeView(0, 3);
    expect(node.truncated).toBe(true);
    render(<NodeTable node={node} onOpen={noop} onReveal={noop} />);
    expect(within(screen.getByTestId('node-rows')).getAllByRole('row')).toHaveLength(3);
    expect(
      screen.getByText(`${node.childrenTotal} items, showing the first 3`),
    ).toBeInTheDocument();
  });

  it('counts a complete page without a "showing" clause', () => {
    const node = fixtureNodeView(0);
    render(<NodeTable node={node} onOpen={noop} onReveal={noop} />);
    expect(screen.getByText(`${node.childrenTotal} items`)).toBeInTheDocument();
    expect(screen.queryByText(/showing the first/)).not.toBeInTheDocument();
  });

  it('marks a partially read directory with a warning, not a lock', () => {
    const library = fixtureNodeView(fixtureNode('Library').id);
    render(<NodeTable node={library} onOpen={noop} onReveal={noop} />);
    const marker = screen.getByRole('img', { name: PARTIAL_READ });
    expect(marker).toHaveAttribute('data-marker', 'partial');
    expect(marker).toHaveAttribute('title', PARTIAL_READ);
  });
});

type Change = (selection: ReadonlySet<NodeId>) => void;

/** The id of the child named `name`; throws rather than letting a typo select another row. */
function childId(node: NodeView, name: string): NodeId {
  const child = node.children.find((candidate) => candidate.name === name);
  if (child === undefined) {
    throw new Error(`no child named ${name} in ${node.path}`);
  }
  return child.id;
}

/**
 * The paths behind a reported selection, rooted at the fixture and sorted. Paths and not
 * names, because the fixture holds two nodes called `Downloads`; read from the whole
 * fixture and not from the node on screen, so an id the table should not have reported
 * shows up instead of vanishing from the comparison.
 */
function pathsOf(selection: ReadonlySet<NodeId>): string[] {
  return [...selection]
    .map((id) => fixtureNodes[id]?.path.replace(`${FIXTURE_ROOT}/`, '') ?? `unknown #${id}`)
    .sort();
}

/** The paths in the selection the table reported last, sorted. */
function reportedPaths(changed: Mock<Change>): string[] {
  const call = changed.mock.lastCall;
  if (call === undefined) {
    throw new Error('the table reported no selection');
  }
  return pathsOf(call[0]);
}

interface SelectableProps {
  node: NodeView;
  /** Rows selected before the user touches anything, by name. */
  selected?: readonly string[];
  /** Ids selected on top of `selected`, for nodes this table does not show. */
  alsoSelected?: readonly NodeId[];
  onSelectionChange?: Change;
  onOpen?: (id: NodeId) => void;
}

/**
 * The page owns the selection (Task 14), so every test drives the table through one: the
 * set goes down as a prop and comes back through the callback.
 *
 * This page deliberately keeps the selection when the node changes, which a real one does
 * not: it measures the table without that safety net under it. The clearing itself belongs
 * to Task 14 and to its test. "Clear from the page" stands in for the action bar, the one
 * thing that empties the selection without the table hearing about it.
 */
function Selectable({
  node,
  selected = [],
  alsoSelected = [],
  onSelectionChange,
  onOpen = noop,
}: SelectableProps) {
  const [selection, setSelection] = useState<ReadonlySet<NodeId>>(
    () => new Set([...selected.map((name) => childId(node, name)), ...alsoSelected]),
  );
  return (
    <>
      <button type="button" onClick={() => setSelection(new Set())}>
        Clear from the page
      </button>
      <NodeTable
        node={node}
        onOpen={onOpen}
        onReveal={noop}
        selection={selection}
        onSelectionChange={(next) => {
          setSelection(next);
          onSelectionChange?.(next);
        }}
      />
    </>
  );
}

function rows(): HTMLElement[] {
  return within(screen.getByTestId('node-rows')).getAllByRole('row');
}

function boxOf(row: HTMLElement): HTMLInputElement {
  return within(row).getByRole<HTMLInputElement>('checkbox');
}

/** The name a row shows: the span titled with it, the way `e2e/explorer.spec.ts` reads it. */
function nameOf(row: HTMLElement): string {
  return row.querySelector('span[title]:not([data-marker])')?.textContent ?? '';
}

/** The row names in display order; works with or without the checkbox column. */
function shownNames(): string[] {
  return rows().map(nameOf);
}

/** The names of the rows whose box is ticked, in display order: what the user sees. */
function tickedInOrder(): string[] {
  return rows()
    .filter((row) => boxOf(row).checked)
    .map(nameOf);
}

function boxFor(name: string): HTMLInputElement {
  return within(screen.getByTestId('node-rows')).getByRole<HTMLInputElement>('checkbox', {
    name: `Select ${name}`,
  });
}

function headerBox(): HTMLInputElement {
  return within(screen.getAllByRole('columnheader')[0]).getByRole<HTMLInputElement>('checkbox');
}

function rowFor(name: string): HTMLElement {
  const row = rows().find((candidate) => nameOf(candidate) === name);
  if (row === undefined) {
    throw new Error(`no row named ${name}; rows: ${shownNames().join(', ')}`);
  }
  return row;
}

function sortBy(label: string): void {
  fireEvent.click(screen.getByRole('button', { name: label }));
}

function clearFromThePage(): void {
  fireEvent.click(screen.getByRole('button', { name: 'Clear from the page' }));
}

/** The fixture root's children in the order the table shows by default: largest first. */
const BY_SIZE = [
  'Library',
  'Downloads',
  'Movies',
  'Pictures',
  'src',
  'Documents',
  '.zshrc',
  '.Trash',
  'OrbStack',
];

/**
 * The same children in the order the Name column sorts them, spelled out rather than
 * derived. Under BY_SIZE the display order, the order the backend sends and the order of
 * the ids are one order — the fixture numbers siblings largest first — so a range taken
 * from any of them looks right. Sorting by Name is the only thing that tells them apart,
 * and a computed expectation would just be the component's collator written twice.
 */
const BY_NAME = [
  '.Trash',
  '.zshrc',
  'Documents',
  'Downloads',
  'Library',
  'Movies',
  'OrbStack',
  'Pictures',
  'src',
];

describe('NodeTable selection', () => {
  it('shows no checkbox column until a page hands it a selection', () => {
    render(<NodeTable node={fixtureNodeView(0)} onOpen={noop} onReveal={noop} />);
    expect(screen.queryAllByRole('checkbox')).toHaveLength(0);
    expect(screen.getAllByRole('columnheader')).toHaveLength(7);
    expect(shownNames()).toEqual(BY_SIZE);
    const first = within(screen.getByTestId('node-rows')).getAllByRole('row')[0];
    expect(within(first).getAllByRole('cell')[0]).toHaveTextContent('Library');
  });

  it('leaves Space alone in a table without checkboxes', () => {
    const opened = vi.fn();
    render(<NodeTable node={fixtureNodeView(0)} onOpen={opened} onReveal={noop} />);
    // Nothing to toggle and nothing to swallow: Space belongs to the page until the page
    // owns a selection. `fireEvent` returns true when no handler called `preventDefault`.
    expect(fireEvent.keyDown(rowFor('Library'), { key: ' ' })).toBe(true);
    expect(opened).not.toHaveBeenCalled();
  });

  it('puts the checkboxes in a first column and labels each one with its row', () => {
    render(<Selectable node={fixtureNodeView(0)} />);
    const headers = screen.getAllByRole('columnheader');
    expect(headers).toHaveLength(8);
    // The column is named for itself, not for the box standing in it.
    expect(headers[0]).toHaveAccessibleName('Select');
    expect(within(headers[0]).getByRole('checkbox')).toBeInTheDocument();
    expect(headers[1]).toHaveTextContent('Name');

    const cells = within(rows()[0]).getAllByRole('cell');
    expect(within(cells[0]).getByRole('checkbox')).toHaveAccessibleName('Select Library');
    expect(cells[1]).toHaveTextContent('Library');
  });

  it('ticks exactly the rows it is given', () => {
    render(<Selectable node={fixtureNodeView(0)} selected={['Movies', 'Documents']} />);
    expect(shownNames()).toEqual(BY_SIZE);
    expect(tickedInOrder()).toEqual(['Movies', 'Documents']);
  });

  it('marks the selected row itself, not only its box', () => {
    render(<Selectable node={fixtureNodeView(0)} selected={['Movies']} />);
    expect(rowFor('Movies')).toHaveAttribute('data-selected', 'true');
    expect(rowFor('Movies')).toHaveClass('bg-blue-50');
    expect(rowFor('Library')).not.toHaveAttribute('data-selected');
    expect(rowFor('Library')).not.toHaveClass('bg-blue-50');

    fireEvent.keyDown(rowFor('Library'), { key: ' ' });
    expect(rowFor('Library')).toHaveAttribute('data-selected', 'true');
    expect(rowFor('Library')).toHaveClass('bg-blue-50');
  });

  it('selects a row on a click and reports it, and unselects it on the next click', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);

    fireEvent.click(boxFor('Downloads'));
    expect(changed).toHaveBeenCalledTimes(1);
    expect(reportedPaths(changed)).toEqual(['Downloads']);
    expect(tickedInOrder()).toEqual(['Downloads']);

    fireEvent.click(boxFor('Downloads'));
    expect(reportedPaths(changed)).toEqual([]);
    expect(tickedInOrder()).toEqual([]);
  });

  it('leaves the other selected rows alone when one row is toggled', () => {
    const changed = vi.fn<Change>();
    render(
      <Selectable
        node={fixtureNodeView(0)}
        selected={['Movies', 'Pictures']}
        onSelectionChange={changed}
      />,
    );
    fireEvent.click(boxFor('Downloads'));
    expect(reportedPaths(changed)).toEqual(['Downloads', 'Movies', 'Pictures']);

    fireEvent.click(boxFor('Movies'));
    expect(reportedPaths(changed)).toEqual(['Downloads', 'Pictures']);
  });

  it('does not open a directory when its checkbox is clicked', () => {
    const opened = vi.fn();
    render(<Selectable node={fixtureNodeView(0)} onOpen={opened} />);
    fireEvent.click(boxFor('Library'));
    expect(opened).not.toHaveBeenCalled();
    expect(tickedInOrder()).toEqual(['Library']);
  });

  it('carves the checkbox column out of the row, and no more than that', () => {
    const changed = vi.fn<Change>();
    const opened = vi.fn();
    const node = fixtureNodeView(0);
    render(<Selectable node={node} onSelectionChange={changed} onOpen={opened} />);
    // The box is 16 px in a 32 px column: a mis-click beside it must cost nothing, because
    // the page drops the selection when the Explorer changes directory.
    fireEvent.click(within(rowFor('Library')).getAllByRole('cell')[0]);
    expect(opened).not.toHaveBeenCalled();
    expect(changed).not.toHaveBeenCalled();

    // The rest of the row still opens the directory, checkbox column or not.
    fireEvent.click(within(rowFor('Library')).getAllByRole('cell')[1]);
    expect(opened).toHaveBeenCalledWith(childId(node, 'Library'));
  });

  it('selects the range between two clicks in the order the table shows', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    sortBy('Name');
    expect(shownNames()).toEqual(BY_NAME);

    fireEvent.click(boxFor('Documents'));
    fireEvent.click(boxFor('Movies'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Documents', 'Downloads', 'Library', 'Movies']);
    expect(tickedInOrder()).toEqual(['Documents', 'Downloads', 'Library', 'Movies']);
  });

  it('takes the range from the order on screen now, not the one the anchor was clicked in', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    sortBy('Name');
    fireEvent.click(boxFor('Documents'));

    sortBy('Size');
    expect(shownNames()).toEqual(BY_SIZE);
    fireEvent.click(boxFor('Movies'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Documents', 'Movies', 'Pictures', 'src']);
  });

  it('extends the range upwards as well as downwards', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    sortBy('Name');
    fireEvent.click(boxFor('Pictures'));
    fireEvent.click(boxFor('Downloads'), { shiftKey: true });
    // Downloads to Pictures by name; by size — where Pictures comes before Downloads —
    // the same two clicks would mean Downloads, Movies, Pictures.
    expect(reportedPaths(changed)).toEqual([
      'Downloads',
      'Library',
      'Movies',
      'OrbStack',
      'Pictures',
    ]);
  });

  it('adds each range to what was selected before', () => {
    const changed = vi.fn<Change>();
    render(
      <Selectable node={fixtureNodeView(0)} selected={['OrbStack']} onSelectionChange={changed} />,
    );
    fireEvent.click(boxFor('Library'));
    fireEvent.click(boxFor('Movies'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Downloads', 'Library', 'Movies', 'OrbStack']);

    fireEvent.click(boxFor('src'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual([
      'Downloads',
      'Library',
      'Movies',
      'OrbStack',
      'Pictures',
      'src',
    ]);
  });

  it('extends the next range from the last row clicked, not from the first', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    fireEvent.click(boxFor('Library'));
    fireEvent.click(boxFor('src'));

    // A plain click moves the anchor too, so the range is src to Documents. Held at
    // Library it would swallow the four rows between them, which nobody asked for.
    fireEvent.click(boxFor('Documents'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Documents', 'Library', 'src']);
    expect(tickedInOrder()).toEqual(['Library', 'src', 'Documents']);
  });

  it('selects one row when the first thing the user does is shift-click', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    fireEvent.click(boxFor('Movies'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Movies']);
  });

  it('does not extend from a row the page has deselected behind its back', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    fireEvent.click(boxFor('Library'));

    // The action bar of Task 14 clears through the same prop, and the table never hears
    // about it: a range must not bring back a row the page has just let go of.
    clearFromThePage();
    expect(tickedInOrder()).toEqual([]);
    changed.mockClear();

    fireEvent.click(boxFor('Movies'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Movies']);
    expect(tickedInOrder()).toEqual(['Movies']);
  });

  it('reports nothing when a shift-click changes nothing', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    fireEvent.click(boxFor('Library'));
    changed.mockClear();

    // The anchor and the clicked row are the same row, and it is already selected.
    fireEvent.click(boxFor('Library'), { shiftKey: true });
    expect(changed).not.toHaveBeenCalled();
    expect(tickedInOrder()).toEqual(['Library']);
  });

  it('selects every row the table shows from the header, and clears them on the next click', () => {
    const changed = vi.fn<Change>();
    const node = fixtureNodeView(0, 3);
    expect(node.childrenTotal).toBe(9);
    render(<Selectable node={node} onSelectionChange={changed} />);

    fireEvent.click(headerBox());
    expect(reportedPaths(changed)).toEqual(['Downloads', 'Library', 'Movies']);
    expect(tickedInOrder()).toEqual(['Library', 'Downloads', 'Movies']);

    fireEvent.click(headerBox());
    expect(reportedPaths(changed)).toEqual([]);
    expect(tickedInOrder()).toEqual([]);
  });

  it('reports only the rows it shows, even when the selection holds other nodes', () => {
    const changed = vi.fn<Change>();
    // As many hidden nodes as there are rows, so that a selection of the same size is not
    // the same selection: what changed here is every member of it.
    const hidden = ['Library/Developer', 'Library/Caches', 'Library/Containers'].map(
      (path) => fixtureNode(path).id,
    );
    render(
      <Selectable node={fixtureNodeView(0, 3)} alsoSelected={hidden} onSelectionChange={changed} />,
    );
    fireEvent.click(headerBox());
    expect(reportedPaths(changed)).toEqual(['Downloads', 'Library', 'Movies']);
  });

  it('clears the whole selection from the header, including a node it does not show', () => {
    const changed = vi.fn<Change>();
    render(
      <Selectable
        node={fixtureNodeView(0, 3)}
        selected={['Library', 'Downloads', 'Movies']}
        alsoSelected={[fixtureNode('Library/Developer').id]}
        onSelectionChange={changed}
      />,
    );
    expect(headerBox()).toBeChecked();

    // Nothing selected may outlive a clear: an id the user cannot see is one the action
    // bar would still count, and delete.
    fireEvent.click(headerBox());
    expect(reportedPaths(changed)).toEqual([]);
  });

  it('shows the header box as mixed while only some rows are selected', () => {
    render(<Selectable node={fixtureNodeView(0, 3)} />);
    expect(headerBox()).not.toBeChecked();
    expect(headerBox()).not.toBePartiallyChecked();

    fireEvent.click(boxFor('Library'));
    expect(headerBox()).not.toBeChecked();
    expect(headerBox()).toBePartiallyChecked();

    fireEvent.click(boxFor('Downloads'));
    fireEvent.click(boxFor('Movies'));
    expect(headerBox()).toBeChecked();
    expect(headerBox()).not.toBePartiallyChecked();
  });

  it('says that the header selects what is shown when the table is truncated', () => {
    const { unmount } = render(<Selectable node={fixtureNodeView(0, 3)} />);
    expect(headerBox()).toHaveAccessibleName('Select all shown');
    unmount();

    render(<Selectable node={fixtureNodeView(0)} />);
    expect(headerBox()).toHaveAccessibleName('Select all');
  });

  it('has nothing to select, and nothing to report, in a directory that could not be read', () => {
    const changed = vi.fn<Change>();
    render(
      <Selectable node={fixtureNodeView(fixtureNode('.Trash').id)} onSelectionChange={changed} />,
    );
    expect(headerBox()).not.toBeChecked();
    expect(headerBox()).not.toBePartiallyChecked();

    fireEvent.click(headerBox());
    expect(changed).not.toHaveBeenCalled();
  });

  it('starts the next range from the clicked row after the header cleared the selection', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    fireEvent.click(boxFor('Library'));
    fireEvent.click(headerBox());
    fireEvent.click(headerBox());
    expect(reportedPaths(changed)).toEqual([]);

    fireEvent.click(boxFor('Movies'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Movies']);
  });

  it('toggles the focused row on Space without opening the directory', () => {
    const changed = vi.fn<Change>();
    const opened = vi.fn();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} onOpen={opened} />);
    const library = rowFor('Library');
    library.focus();

    // `fireEvent` returns false when a handler called `preventDefault`: Space must not scroll.
    expect(fireEvent.keyDown(library, { key: ' ' })).toBe(false);
    expect(opened).not.toHaveBeenCalled();
    expect(reportedPaths(changed)).toEqual(['Library']);
    expect(tickedInOrder()).toEqual(['Library']);

    fireEvent.keyDown(rowFor('Library'), { key: ' ' });
    expect(tickedInOrder()).toEqual([]);
  });

  it('toggles a file row on Space too, and anchors the next range on it', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    fireEvent.keyDown(rowFor('.zshrc'), { key: ' ' });
    expect(reportedPaths(changed)).toEqual(['.zshrc']);

    fireEvent.click(boxFor('src'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['.zshrc', 'Documents', 'src']);
  });

  it('still opens a directory on Enter, and selects nothing', () => {
    const changed = vi.fn<Change>();
    const opened = vi.fn();
    const node = fixtureNodeView(0);
    render(<Selectable node={node} onSelectionChange={changed} onOpen={opened} />);
    fireEvent.keyDown(rowFor('Library'), { key: 'Enter' });
    expect(opened).toHaveBeenCalledWith(childId(node, 'Library'));
    expect(changed).not.toHaveBeenCalled();
    expect(tickedInOrder()).toEqual([]);
  });

  it('answers Space on a checkbox once, and never opens a directory from one', () => {
    const changed = vi.fn<Change>();
    const opened = vi.fn();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} onOpen={opened} />);
    // The browser turns Space on a focused box into a click; the row handler must not
    // toggle it a second time on the way past.
    fireEvent.keyDown(boxFor('Library'), { key: ' ' });
    expect(changed).not.toHaveBeenCalled();

    // Enter does nothing to a checkbox, and the row would turn that nothing into a
    // navigation — which costs the user standing here the selection they are building.
    fireEvent.keyDown(boxFor('Library'), { key: 'Enter' });
    expect(opened).not.toHaveBeenCalled();
    expect(changed).not.toHaveBeenCalled();

    fireEvent.click(boxFor('Library'));
    expect(changed).toHaveBeenCalledTimes(1);
    expect(tickedInOrder()).toEqual(['Library']);
  });

  it('moves between rows with the arrows from the box the user is standing on', () => {
    render(<Selectable node={fixtureNodeView(0)} />);
    const box = boxFor('Library');
    box.focus();

    // Tab to a box, Space, arrow, Space: the arrow step runs on the body, so the box may
    // not swallow it — three Tabs per row is not a way through 500 of them.
    fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(rowFor('Downloads')).toHaveFocus();
    fireEvent.keyDown(rowFor('Downloads'), { key: ' ' });
    expect(tickedInOrder()).toEqual(['Downloads']);
  });

  it('leaves the arrows to the table from the Reveal button too, and the rest to itself', () => {
    const changed = vi.fn<Change>();
    const opened = vi.fn();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} onOpen={opened} />);
    const reveal = within(rowFor('Library')).getByRole('button', { name: 'Reveal in Finder' });
    reveal.focus();

    fireEvent.keyDown(reveal, { key: 'ArrowDown' });
    expect(rowFor('Downloads')).toHaveFocus();

    // Enter and Space press the button; the row behind it must neither open nor tick.
    fireEvent.keyDown(reveal, { key: 'Enter' });
    fireEvent.keyDown(reveal, { key: ' ' });
    expect(opened).not.toHaveBeenCalled();
    expect(changed).not.toHaveBeenCalled();
  });

  it('selects a directory the scanner could not read', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    const trash = rowFor('.Trash');
    expect(within(trash).getByRole('img', { name: PERMISSION_DENIED })).toHaveAttribute(
      'data-marker',
      'lock',
    );
    expect(boxOf(trash)).toBeEnabled();

    fireEvent.click(boxOf(trash));
    expect(reportedPaths(changed)).toEqual(['.Trash']);
    expect(tickedInOrder()).toEqual(['.Trash']);
  });

  it('keeps the selection when the table is sorted again', () => {
    const changed = vi.fn<Change>();
    render(<Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />);
    fireEvent.click(boxFor('Documents'));
    fireEvent.click(boxFor('.Trash'));
    changed.mockClear();

    sortBy('Name');
    expect(tickedInOrder()).toEqual(['.Trash', 'Documents']);
    sortBy('Size');
    expect(tickedInOrder()).toEqual(['Documents', '.Trash']);
    expect(changed).not.toHaveBeenCalled();
  });

  it('never changes the selection by itself', () => {
    const changed = vi.fn<Change>();
    const node = fixtureNodeView(0);
    const { rerender } = render(
      <Selectable node={node} selected={['Movies']} onSelectionChange={changed} />,
    );
    rerender(<Selectable node={node} selected={['Movies']} onSelectionChange={changed} />);
    expect(tickedInOrder()).toEqual(['Movies']);

    rerender(
      <Selectable
        node={fixtureNodeView(fixtureNode('Library').id)}
        selected={['Movies']}
        onSelectionChange={changed}
      />,
    );
    expect(shownNames()).toEqual(['Developer', 'Containers', 'Caches', 'Application Support']);
    expect(changed).not.toHaveBeenCalled();
  });

  it('does not extend a range into another directory after a navigation', () => {
    const changed = vi.fn<Change>();
    const { rerender } = render(
      <Selectable node={fixtureNodeView(0)} onSelectionChange={changed} />,
    );
    fireEvent.click(boxFor('Library'));

    rerender(
      <Selectable node={fixtureNodeView(fixtureNode('Library').id)} onSelectionChange={changed} />,
    );
    fireEvent.click(boxFor('Caches'), { shiftKey: true });
    expect(reportedPaths(changed)).toEqual(['Library', 'Library/Caches']);
  });

  it('drops the anchor when a rescan renumbers the rows under the same node', () => {
    const root = fixtureNodeView(0);
    const { rerender } = render(<Selectable node={root} />);
    fireEvent.click(boxFor('Documents'));

    // A rescan and the splice after a batch both replace every id in the arena while the
    // root keeps its own: `node.id` is the same node, and every row under it is a new one.
    // The id the anchor held now names the row below the one it was taken from.
    const renumbered: NodeView = {
      ...root,
      children: root.children.map((child) => ({ ...child, id: child.id + 1 })),
    };
    rerender(<Selectable node={renumbered} />);
    expect(shownNames()).toEqual(BY_SIZE);
    expect(tickedInOrder()).toEqual(['src']);

    fireEvent.click(boxFor('Movies'), { shiftKey: true });
    expect(tickedInOrder()).toEqual(['Movies', 'src']);
  });

  it('drops the anchor with the rows it came from, even when an id comes back', () => {
    const changed = vi.fn<Change>();
    const root = fixtureNodeView(0);
    const { rerender } = render(<Selectable node={root} onSelectionChange={changed} />);
    fireEvent.click(boxFor('Documents'));

    // Another directory, from a scan that numbered the tree again: its first row carries
    // the id the anchor held. A range must not extend from a row that shares a number.
    const library = fixtureNodeView(fixtureNode('Library').id);
    const renumbered: NodeView = {
      ...library,
      children: library.children.map((child, index) =>
        index === 0 ? { ...child, id: childId(root, 'Documents') } : child,
      ),
    };
    rerender(<Selectable node={renumbered} onSelectionChange={changed} />);
    expect(shownNames()).toEqual(['Developer', 'Containers', 'Caches', 'Application Support']);
    expect(tickedInOrder()).toEqual(['Developer']);

    fireEvent.click(boxFor('Caches'), { shiftKey: true });
    expect(tickedInOrder()).toEqual(['Developer', 'Caches']);
  });
});
