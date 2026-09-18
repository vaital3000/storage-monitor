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
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { countLabel, formatBytes, formatDate, formatDelta, formatPercent } from '../lib/format';
import type { ChildView, NodeId, NodeKind, NodeView } from '../lib/ipc';
import { describeNodeError, type NodeErrorMark } from '../lib/nodeErrors';

interface NodeTableProps {
  node: NodeView;
  /** Focus the first row when another directory arrives (the keyboard brought us there). */
  focusFirstRow?: boolean;
  /** A directory row was activated. */
  onOpen: (id: NodeId) => void;
  /** "Reveal in Finder" was clicked for the absolute `path`. */
  onReveal: (path: string) => void;
}

type SortKey = 'name' | 'size' | 'delta' | 'fileCount' | 'mtime';
type Direction = 'asc' | 'desc';

interface Sort {
  key: SortKey;
  direction: Direction;
}

interface Column {
  key: SortKey | 'percent' | 'actions';
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

// Fixed widths for the other columns (452 px in total), each sized for its widest value at
// 13 px (`2026-09-18`, `100.0%`, `−999.9 MB`, `123,456`); the name column takes the rest:
// 188 px at the window's minimum width of 900 px, which fits "Application Support" next
// to its marker.
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
  onOpen: (id: NodeId) => void;
  onReveal: (path: string) => void;
}

function Row({ child, parent, maxSize, onOpen, onReveal }: RowProps) {
  const isDir = child.kind === 'dir';
  const path = `${parent.path}/${child.name}`;
  const mark = child.error === null ? null : describeNodeError(child.error);
  const barWidth = maxSize > 0 ? (child.size / maxSize) * 100 : 0;
  const open = () => {
    if (isDir) onOpen(child.id);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTableRowElement>) => {
    if (event.key === 'Enter' && isDir) {
      event.preventDefault();
      open();
    }
  };
  const numeric = `${NUMERIC_PADDING} py-1.5 text-right whitespace-nowrap tabular-nums`;
  return (
    <tr
      tabIndex={0}
      data-kind={child.kind}
      onClick={open}
      onKeyDown={onKeyDown}
      className={`group border-t border-neutral-100 outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-inset dark:border-neutral-800 ${
        isDir
          ? 'cursor-pointer hover:bg-neutral-100 dark:hover:bg-neutral-800'
          : 'cursor-default hover:bg-neutral-50 dark:hover:bg-neutral-800/50'
      }`}
    >
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
          onKeyDown={(event) => event.stopPropagation()}
          className="rounded p-0.5 text-muted opacity-35 group-hover:opacity-100 group-focus-visible:opacity-100 hover:text-neutral-700 focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-blue-500 dark:hover:text-neutral-200"
        >
          <SquareArrowOutUpRight className="size-3.5" />
        </button>
      </td>
    </tr>
  );
}

/**
 * The children of one directory: sortable, keyboard-navigable (Enter opens a directory,
 * arrows move between rows), with a "Reveal in Finder" button per row.
 */
export default function NodeTable({
  node,
  focusFirstRow = false,
  onOpen,
  onReveal,
}: NodeTableProps) {
  const [sort, setSort] = useState<Sort>({ key: 'size', direction: 'desc' });
  const sorted = useMemo(() => sortChildren(node.children, sort), [node.children, sort]);
  const maxSize = useMemo(
    () => node.children.reduce((max, child) => Math.max(max, child.size), 0),
    [node.children],
  );
  const body = useRef<HTMLTableSectionElement>(null);
  const shownId = useRef(node.id);

  // After a keyboard navigation, keyboard users continue from the first row of the new
  // directory; a click leaves the focus where the pointer put it.
  useEffect(() => {
    if (shownId.current === node.id) return;
    shownId.current = node.id;
    if (focusFirstRow) body.current?.querySelector('tr')?.focus({ preventScroll: true });
  }, [node.id, focusFirstRow]);

  const toggle = (key: SortKey) => {
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
            {COLUMNS.map((column) => {
              const active = column.sortable && sort.key === column.key;
              return (
                <th
                  key={column.key}
                  scope="col"
                  title={column.title}
                  aria-sort={
                    active ? (sort.direction === 'asc' ? 'ascending' : 'descending') : undefined
                  }
                  className={`${column.width} ${column.padding} py-1.5 font-medium text-muted ${
                    column.numeric ? 'text-right' : 'text-left'
                  }`}
                >
                  {column.sortable ? (
                    <button
                      type="button"
                      onClick={() => toggle(column.key as SortKey)}
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
                onOpen={onOpen}
                onReveal={onReveal}
              />
            ))}
          </tbody>
        ) : (
          <tbody>
            <tr className="border-t border-neutral-100 dark:border-neutral-800">
              <td colSpan={COLUMNS.length} className="px-3 py-8 text-center text-muted">
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
