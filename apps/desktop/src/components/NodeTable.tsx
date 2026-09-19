import {
  ArrowDown,
  ArrowUp,
  File,
  Folder,
  Info,
  Link,
  Lock,
  SquareArrowOutUpRight,
  TriangleAlert,
  type LucideIcon,
} from 'lucide-react';
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react';
import { countLabel, formatBytes, formatDate, formatDelta, formatPercent } from '../lib/format';
import type { ChildView, NodeId, NodeKind, NodeView } from '../lib/ipc';
import { describeNodeError, type NodeErrorMark } from '../lib/nodeErrors';

interface BaseProps {
  node: NodeView;
  /** Focus the first row when another directory arrives (the keyboard brought us there). */
  focusFirstRow?: boolean;
  /** A directory row was activated. */
  onOpen: (id: NodeId) => void;
  /** "Reveal in Finder" was clicked for the absolute `path`. */
  onReveal: (path: string) => void;
}

/**
 * The selection belongs to the page, which keeps it across a sort and drops it on a
 * navigation and on a new generation: the table renders the set it is given and reports
 * the set the user asked for, and never edits one on its own. The two props travel
 * together — `selection` alone would tick boxes that can never change — and without them
 * there is no checkbox column.
 */
type SelectionProps =
  | {
      /** Ids of the selected rows. */
      selection: ReadonlySet<NodeId>;
      onSelectionChange: (selection: ReadonlySet<NodeId>) => void;
    }
  | { selection?: never; onSelectionChange?: never };

type NodeTableProps = BaseProps & SelectionProps;

type SortKey = 'name' | 'size' | 'delta' | 'fileCount' | 'mtime';
type Direction = 'asc' | 'desc';

interface Sort {
  key: SortKey;
  direction: Direction;
}

interface Column {
  key: SortKey | 'select' | 'percent' | 'actions';
  label: string;
  numeric: boolean;
  sortable: boolean;
  /** Width and horizontal padding, shared by the header and the cells of the column. */
  width: string;
  padding: string;
  title?: string;
}

const MTIME_TITLE = 'Directory modification time, not the newest content';

const NAME_PADDING = 'pl-3 pr-1';
const NUMERIC_PADDING = 'px-2';
const ACTIONS_PADDING = 'px-1';
const SELECT_PADDING = 'px-2';
const CHECKBOX_CLASS =
  'size-4 cursor-pointer align-middle accent-blue-600 focus-visible:outline-2 focus-visible:outline-blue-500';

/** The checkbox column, in front of the others when the page hands the table a selection. */
const SELECT_COLUMN: Column = {
  key: 'select',
  label: '',
  numeric: false,
  sortable: false,
  width: 'w-8',
  padding: SELECT_PADDING,
};

// Fixed widths for every column but the name, each sized for its widest value at 13 px
// (`2026-09-18`, `100.0%`, `−999.9 MB`, `123,456`): 452 px here, and 484 px once
// SELECT_COLUMN adds its 32 px. The name column takes the rest — 188 px at the window's
// minimum width of 900 px, 156 px with the checkboxes — and those 32 px are the slack
// "Application Support" was using: measured there, it fills its 120 px exactly, so with a
// checkbox in front of it the name truncates into its title.
const COLUMNS: readonly Column[] = [
  {
    key: 'name',
    label: 'Name',
    numeric: false,
    sortable: true,
    width: 'w-auto',
    padding: NAME_PADDING,
  },
  {
    key: 'size',
    label: 'Size',
    numeric: true,
    sortable: true,
    width: 'w-30',
    padding: NUMERIC_PADDING,
  },
  {
    key: 'percent',
    label: '%',
    numeric: true,
    sortable: false,
    width: 'w-14',
    padding: NUMERIC_PADDING,
  },
  {
    key: 'delta',
    label: 'Δ',
    numeric: true,
    sortable: true,
    width: 'w-22',
    padding: NUMERIC_PADDING,
  },
  {
    key: 'fileCount',
    label: 'Files',
    numeric: true,
    sortable: true,
    width: 'w-16',
    padding: NUMERIC_PADDING,
  },
  {
    key: 'mtime',
    label: 'Modified',
    numeric: true,
    sortable: true,
    width: 'w-24',
    padding: NUMERIC_PADDING,
    title: MTIME_TITLE,
  },
  {
    key: 'actions',
    label: '',
    numeric: false,
    sortable: false,
    width: 'w-7',
    padding: ACTIONS_PADDING,
  },
];

/** The direction a column starts with: names read A to Z, numbers largest first. */
const FIRST_DIRECTION: Record<SortKey, Direction> = {
  name: 'asc',
  size: 'desc',
  delta: 'desc',
  fileCount: 'desc',
  mtime: 'desc',
};

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

function compare(a: ChildView, b: ChildView, key: SortKey): number {
  switch (key) {
    case 'name':
      return collator.compare(a.name, b.name);
    case 'size':
      return a.size - b.size;
    case 'delta':
      // An unknown delta sorts like "no change".
      return (a.delta ?? 0) - (b.delta ?? 0);
    case 'fileCount':
      return a.fileCount - b.fileCount;
    case 'mtime':
      return a.mtime - b.mtime;
  }
}

function sortChildren(children: readonly ChildView[], sort: Sort): ChildView[] {
  const sign = sort.direction === 'asc' ? 1 : -1;
  return [...children].sort(
    (a, b) => sign * compare(a, b, sort.key) || collator.compare(a.name, b.name),
  );
}

const KIND_ICONS: Record<NodeKind, { icon: LucideIcon; className: string }> = {
  dir: { icon: Folder, className: 'text-sky-500' },
  file: { icon: File, className: 'text-neutral-400' },
  symlink: { icon: Link, className: 'text-neutral-400' },
  other: { icon: File, className: 'text-neutral-400' },
};

function KindIcon({ kind }: { kind: NodeKind }) {
  const { icon: Icon, className } = KIND_ICONS[kind];
  return <Icon className={`size-4 shrink-0 ${className}`} />;
}

const MARKER_ICONS: Record<NodeErrorMark['kind'], { icon: LucideIcon; className: string }> = {
  lock: { icon: Lock, className: 'text-amber-600 dark:text-amber-500' },
  partial: { icon: TriangleAlert, className: 'text-amber-600 dark:text-amber-500' },
  info: { icon: Info, className: 'text-neutral-400' },
};

/** The lock (could not read), warning (read in part) or info (chose not to read) next to a name. */
function Marker({ mark }: { mark: NodeErrorMark }) {
  const { icon: Icon, className } = MARKER_ICONS[mark.kind];
  return (
    <span
      role="img"
      aria-label={mark.title}
      title={mark.title}
      data-marker={mark.kind}
      className={`inline-flex shrink-0 ${className}`}
    >
      <Icon className="size-3.5" />
    </span>
  );
}

function deltaClass(delta: number | null): string {
  if (delta === null || delta === 0) return '';
  return delta > 0 ? 'text-red-600 dark:text-red-400' : 'text-emerald-700 dark:text-emerald-400';
}

interface RowProps {
  child: ChildView;
  parent: NodeView;
  /** Size of the largest sibling; the inline bar is relative to it. */
  maxSize: number;
  selected: boolean;
  /**
   * Null when the table has no checkbox column. `extend` is a shift-click: take in every
   * row between the anchor and this one. Values and one shared handler, not a closure per
   * row, so that `memo` below has something to compare.
   */
  onToggle: ((id: NodeId, extend: boolean) => void) | null;
  onOpen: (id: NodeId) => void;
  onReveal: (path: string) => void;
}

// A selection changes two rows and re-renders the table; at the 500 rows `tree_node` hands
// out, that is 500 rows of work for two ticks. Memoized, it is two — as long as every prop
// stays a value or a handler that outlives the render (`toggleRow`, `onOpen`, `onReveal`).
const Row = memo(function Row({
  child,
  parent,
  maxSize,
  selected,
  onToggle,
  onOpen,
  onReveal,
}: RowProps) {
  const isDir = child.kind === 'dir';
  const path = `${parent.path}/${child.name}`;
  const mark = child.error === null ? null : describeNodeError(child.error);
  const barWidth = maxSize > 0 ? (child.size / maxSize) * 100 : 0;
  const open = () => {
    if (isDir) onOpen(child.id);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTableRowElement>) => {
    // Space picks the row out; it must neither scroll the table nor open the directory.
    // From the row it never extends a range — there is no second row to point at — while
    // Shift+Space on the box itself does, through the click the browser synthesizes.
    if (event.key === ' ' && onToggle !== null) {
      event.preventDefault();
      onToggle(child.id, false);
      return;
    }
    if (event.key === 'Enter' && isDir) {
      event.preventDefault();
      open();
    }
  };
  const numeric = `${NUMERIC_PADDING} py-1.5 text-right whitespace-nowrap tabular-nums`;
  // A 16 px tick is not enough to see a range by, so the row says it in a colour too, and
  // `data-selected` lets a test say it in a value. Neither reaches assistive technology:
  // a row toggled with Space changes a descendant of the focused element and nothing
  // announces it, which needs a `grid` and `aria-selected` — deferred to the task that
  // owns the table's role, so that the e2e selectors move once instead of twice.
  const tone = selected
    ? 'bg-blue-50 hover:bg-blue-100 dark:bg-blue-950/50 dark:hover:bg-blue-900/50'
    : isDir
      ? 'hover:bg-neutral-100 dark:hover:bg-neutral-800'
      : 'hover:bg-neutral-50 dark:hover:bg-neutral-800/50';
  return (
    <tr
      tabIndex={0}
      data-kind={child.kind}
      data-selected={selected ? 'true' : undefined}
      onClick={open}
      onKeyDown={onKeyDown}
      className={`group border-t border-neutral-100 outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-inset dark:border-neutral-800 ${
        isDir ? 'cursor-pointer' : 'cursor-default'
      } ${tone}`}
    >
      {onToggle !== null && (
        // A click that lands beside the box must not open the directory: a mis-click of a
        // few pixels would navigate away, and the page drops the selection when it does.
        <td className={`${SELECT_PADDING} py-1.5`} onClick={(event) => event.stopPropagation()}>
          <input
            type="checkbox"
            aria-label={`Select ${child.name}`}
            checked={selected}
            // React reports a checkbox's change from the click that made it — including
            // the click a browser synthesizes for Space — so the modifier rides on the
            // native event; anything that is not a mouse event is a plain toggle.
            onChange={(event) =>
              onToggle(
                child.id,
                event.nativeEvent instanceof MouseEvent && event.nativeEvent.shiftKey,
              )
            }
            // Space is the box's own key, and the row would answer it again. Enter is
            // held back for a different reason: on a checkbox it does nothing, and the
            // row would turn that nothing into opening a directory — which costs a user
            // standing here mid-selection the whole selection, with no undo. A key whose
            // old cost was zero does not get to start discarding work.
            //
            // Everything else belongs to the table around it. The arrows that move
            // between rows run on the body, and a blanket guard would strand the focus
            // on the box a selecting user is standing on.
            onKeyDown={(event) => {
              if (event.key === ' ' || event.key === 'Enter') event.stopPropagation();
            }}
            className={CHECKBOX_CLASS}
          />
        </td>
      )}
      <td className={`max-w-0 ${NAME_PADDING} py-1.5`}>
        <div className="flex min-w-0 items-center gap-1.5">
          <KindIcon kind={child.kind} />
          <span className="truncate" title={child.name}>
            {child.name}
          </span>
          {mark !== null && <Marker mark={mark} />}
        </div>
      </td>
      <td className={numeric}>
        <div className="flex items-center justify-end gap-2">
          <div className="h-1.5 w-8 shrink-0 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700">
            <div className="h-full rounded-full bg-blue-500/70" style={{ width: `${barWidth}%` }} />
          </div>
          <span className="w-16">{formatBytes(child.size)}</span>
        </div>
      </td>
      <td className={`${numeric} text-muted`}>{formatPercent(child.size, parent.size)}</td>
      <td className={`${numeric} ${deltaClass(child.delta)}`}>{formatDelta(child.delta)}</td>
      <td className={`${numeric} text-muted`}>{child.fileCount.toLocaleString('en-US')}</td>
      <td className={`${numeric} text-muted`} title={isDir ? MTIME_TITLE : undefined}>
        {formatDate(child.mtime)}
      </td>
      <td className={`${ACTIONS_PADDING} py-1.5`}>
        <button
          type="button"
          aria-label="Reveal in Finder"
          title="Reveal in Finder"
          onClick={(event) => {
            event.stopPropagation();
            onReveal(path);
          }}
          // The two keys that press a button, held for the same reasons as at the box:
          // the row would open the directory on the Enter that revealed the row, and
          // toggle it on the Space that did. The arrows stay the table's, so the focus
          // is never stranded on this button either.
          onKeyDown={(event) => {
            if (event.key === ' ' || event.key === 'Enter') event.stopPropagation();
          }}
          className="rounded p-0.5 text-muted opacity-35 group-hover:opacity-100 group-focus-visible:opacity-100 hover:text-neutral-700 focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-blue-500 dark:hover:text-neutral-200"
        >
          <SquareArrowOutUpRight className="size-3.5" />
        </button>
      </td>
    </tr>
  );
});

/** How this table takes part in a selection: null when the page gave it none to render. */
interface Selecting {
  selected: ReadonlySet<NodeId>;
  change: (selection: ReadonlySet<NodeId>) => void;
}

/** Position of the row with this id in the order the table shows, or -1 when it is gone. */
function indexOfRow(sorted: readonly ChildView[], id: NodeId): number {
  return sorted.findIndex((child) => child.id === id);
}

function countSelected(sorted: readonly ChildView[], selection: ReadonlySet<NodeId>): number {
  return sorted.reduce((count, child) => (selection.has(child.id) ? count + 1 : count), 0);
}

/** Hands the page a selection only when it differs: a click that changed nothing is not news. */
function report(selecting: Selecting, next: ReadonlySet<NodeId>): void {
  const same =
    next.size === selecting.selected.size && [...next].every((id) => selecting.selected.has(id));
  if (!same) selecting.change(next);
}

/**
 * The children of one directory: sortable, keyboard-navigable (Enter opens a directory,
 * Space selects a row, arrows move between rows), with a "Reveal in Finder" button per row
 * and, when the page owns a selection, a checkbox per row.
 */
export default function NodeTable({
  node,
  focusFirstRow = false,
  onOpen,
  onReveal,
  selection,
  onSelectionChange,
}: NodeTableProps) {
  const [sort, setSort] = useState<Sort>({ key: 'size', direction: 'desc' });
  const sorted = useMemo(() => sortChildren(node.children, sort), [node.children, sort]);
  const maxSize = useMemo(
    () => node.children.reduce((max, child) => Math.max(max, child.size), 0),
    [node.children],
  );
  const body = useRef<HTMLTableSectionElement>(null);
  const shownId = useRef(node.id);
  // The row a range extends from: the last one the user ticked, kept as an id rather than
  // as an index, because a re-sort moves every row and the range the user means is the one
  // the table shows now. It belongs to the rows on screen, and goes when they do.
  const anchor = useRef<NodeId | null>(null);

  // After a keyboard navigation, keyboard users continue from the first row of the new
  // directory; a click leaves the focus where the pointer put it.
  useEffect(() => {
    if (shownId.current === node.id) return;
    shownId.current = node.id;
    if (focusFirstRow) body.current?.querySelector('tr')?.focus({ preventScroll: true });
  }, [node.id, focusFirstRow]);

  // A `NodeId` only means something inside the arena that handed it out, and a rescan or
  // the splice after a batch replaces every id in it while the root keeps `node.id` — so
  // `node.id` cannot tell one generation of rows from the next. A new `children` array
  // can: the query refetches into a new one when the rows change, and keeps the old one
  // when nothing did. The anchor is the only id the page cannot reach, so it dies here
  // with the rows it was taken from.
  useEffect(() => {
    anchor.current = null;
  }, [node.children]);

  // One answer to "does this table select?", so the column, the boxes and the handlers can
  // never disagree about it.
  const selecting: Selecting | null =
    selection !== undefined && onSelectionChange !== undefined
      ? { selected: selection, change: onSelectionChange }
      : null;
  const selectedRows = selecting === null ? 0 : countSelected(sorted, selecting.selected);
  // An empty table has nothing selected, whatever `every` would say about no rows at all.
  const allSelected = selectedRows > 0 && selectedRows === sorted.length;
  const someSelected = selectedRows > 0 && selectedRows < sorted.length;

  // What a row's handler has to read, kept where a handler that never changes can find it.
  // In a layout effect, which runs as part of the commit: a passive one would also be
  // flushed before the next click, but only because React flushes those before a discrete
  // event, which is behaviour and not a promise — and it would leave a caller that is not
  // a discrete event (a drag-select, a `requestAnimationFrame`, an effect on the page)
  // reading one commit behind, in the one place where a stale `sorted` means ranging over
  // rows that are no longer the rows on screen.
  const shown = useRef({ sorted, selecting });
  useLayoutEffect(() => {
    shown.current = { sorted, selecting };
  });

  // One handler for every row, stable for the life of the table: 500 rows carrying 500 new
  // closures would re-render all of them for one tick, whatever `memo` says. `toggleAll`
  // below needs none of this and keeps reading the render it belongs to — there is one
  // header box, it re-renders with the table, and a ref there would buy nothing and cost
  // a second way of reading the same two values.
  const toggleRow = useCallback((id: NodeId, extend: boolean) => {
    const { sorted, selecting } = shown.current;
    if (selecting === null) return;
    // A range extends from a row that is still selected. Anything else — the page clearing
    // the selection behind the table, the header, the user unticking the anchor itself —
    // leaves nothing to extend from, and a plain toggle is what the user is looking at.
    const from =
      anchor.current !== null && selecting.selected.has(anchor.current)
        ? indexOfRow(sorted, anchor.current)
        : -1;
    const to = indexOfRow(sorted, id);
    anchor.current = id;
    const next = new Set(selecting.selected);
    if (extend && from !== -1 && to !== -1) {
      for (let i = Math.min(from, to); i <= Math.max(from, to); i += 1) {
        next.add(sorted[i].id);
      }
    } else if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    report(selecting, next);
  }, []);

  const toggleAll = () => {
    if (selecting === null) return;
    // The header is about the rows on screen: it reports exactly them, or nothing — never
    // a row the table did not show.
    report(selecting, allSelected ? new Set() : new Set(sorted.map((child) => child.id)));
  };

  const columns = selecting === null ? COLUMNS : [SELECT_COLUMN, ...COLUMNS];

  const toggleSort = (key: SortKey) => {
    setSort((current) =>
      current.key === key
        ? { key, direction: current.direction === 'asc' ? 'desc' : 'asc' }
        : { key, direction: FIRST_DIRECTION[key] },
    );
  };

  const onBodyKeyDown = (event: KeyboardEvent<HTMLTableSectionElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    if (!(event.target instanceof HTMLElement)) return;
    const row = event.target.closest('tr');
    const next = event.key === 'ArrowDown' ? row?.nextElementSibling : row?.previousElementSibling;
    if (next instanceof HTMLElement) {
      event.preventDefault();
      next.focus();
    }
  };

  return (
    <div className="overflow-hidden rounded-lg border border-neutral-200 bg-white dark:border-neutral-800 dark:bg-neutral-900">
      <table className="w-full table-fixed border-collapse text-base">
        <thead className="bg-neutral-50 text-sm dark:bg-neutral-800/60">
          <tr>
            {columns.map((column) => {
              const active = column.sortable && sort.key === column.key;
              return (
                <th
                  key={column.key}
                  scope="col"
                  // Without a label of its own the column would take the name of the box
                  // inside it, and a screen reader would call every row's box "Select all".
                  aria-label={column.key === 'select' ? 'Select' : undefined}
                  title={column.title}
                  aria-sort={
                    active ? (sort.direction === 'asc' ? 'ascending' : 'descending') : undefined
                  }
                  className={`${column.width} ${column.padding} py-1.5 font-medium text-muted ${
                    column.numeric ? 'text-right' : 'text-left'
                  }`}
                >
                  {column.key === 'select' ? (
                    <input
                      type="checkbox"
                      // What "all" covers is what the table shows, which is not every child
                      // of a truncated node: say so rather than promise the rest.
                      aria-label={node.truncated ? 'Select all shown' : 'Select all'}
                      checked={allSelected}
                      ref={(box) => {
                        if (box !== null) box.indeterminate = someSelected;
                      }}
                      onChange={toggleAll}
                      className={CHECKBOX_CLASS}
                    />
                  ) : column.sortable ? (
                    <button
                      type="button"
                      onClick={() => toggleSort(column.key as SortKey)}
                      className="inline-flex items-center gap-1 rounded hover:text-neutral-800 focus-visible:outline-2 focus-visible:outline-blue-500 dark:hover:text-neutral-200"
                    >
                      {column.label}
                      {active &&
                        (sort.direction === 'asc' ? (
                          <ArrowUp className="size-3" />
                        ) : (
                          <ArrowDown className="size-3" />
                        ))}
                    </button>
                  ) : (
                    column.label
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        {sorted.length > 0 ? (
          <tbody ref={body} data-testid="node-rows" onKeyDown={onBodyKeyDown}>
            {sorted.map((child) => (
              <Row
                key={child.id}
                child={child}
                parent={node}
                maxSize={maxSize}
                selected={selecting !== null && selecting.selected.has(child.id)}
                onToggle={selecting === null ? null : toggleRow}
                onOpen={onOpen}
                onReveal={onReveal}
              />
            ))}
          </tbody>
        ) : (
          <tbody>
            <tr className="border-t border-neutral-100 dark:border-neutral-800">
              <td colSpan={columns.length} className="px-3 py-8 text-center text-muted">
                {node.error === null ? 'Empty folder' : describeNodeError(node.error).title}
              </td>
            </tr>
          </tbody>
        )}
      </table>
      <p className="border-t border-neutral-100 px-3 py-1.5 text-xs text-muted dark:border-neutral-800">
        {countLabel(node.childrenTotal, 'item')}
        {node.truncated && `, showing the first ${node.children.length.toLocaleString('en-US')}`}
      </p>
    </div>
  );
}
