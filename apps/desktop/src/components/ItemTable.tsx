import { memo, useCallback, useLayoutEffect, useRef, type KeyboardEvent } from 'react';
import { countLabel, formatStampDate } from '../lib/format';
import type { Item } from '../lib/ipc';
import { itemSize } from '../lib/items';
import VerdictBadge from './VerdictBadge';

interface ItemTableProps {
  /** The rows to show, in the order to show them: largest first. */
  items: readonly Item[];
  /** How many items the modules hold in all, which can be more than the rows handed over. */
  total: number;
  moduleNames: ReadonlyMap<string, string>;
  selection: ReadonlySet<string>;
  onSelectionChange: (selection: ReadonlySet<string>) => void;
  /** The row the detail panel shows. */
  focused: string | null;
  onFocus: (id: string) => void;
}

const CHECKBOX_CLASS =
  'size-4 cursor-pointer align-middle accent-blue-600 focus-visible:outline-2 focus-visible:outline-blue-500';
const CELL = 'px-2 py-1.5';
const NUMERIC = `${CELL} text-right whitespace-nowrap tabular-nums`;

interface RowProps {
  item: Item;
  module: string;
  selected: boolean;
  focused: boolean;
  onToggle: (id: string, extend: boolean) => void;
  onFocus: (id: string) => void;
}

// Memoized like the Explorer's rows, for the same reason: a tick changes one row, and every
// prop here is a value or a handler that outlives the render.
const Row = memo(function Row({ item, module, selected, focused, onToggle, onFocus }: RowProps) {
  const onKeyDown = (event: KeyboardEvent<HTMLTableRowElement>) => {
    // Space ticks the row, as in the Explorer; Enter shows it in the detail panel, which is
    // what a click does. Neither scrolls the table.
    if (event.key === ' ') {
      event.preventDefault();
      onToggle(item.id, false);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      onFocus(item.id);
    }
  };
  const tone = selected
    ? 'bg-blue-50 hover:bg-blue-100 dark:bg-blue-950/50 dark:hover:bg-blue-900/50'
    : 'hover:bg-neutral-50 dark:hover:bg-neutral-800/50';
  const subtitle = [module, item.subtitle].filter((part) => part !== null && part !== '');
  return (
    <tr
      tabIndex={0}
      data-item={item.id}
      data-selected={selected ? 'true' : undefined}
      aria-current={focused ? 'true' : undefined}
      onClick={() => onFocus(item.id)}
      onKeyDown={onKeyDown}
      className={`cursor-default border-t border-neutral-100 outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-inset dark:border-neutral-800 ${tone} ${
        focused ? 'shadow-[inset_3px_0_0] shadow-blue-500' : ''
      }`}
    >
      {/* A click beside the box must not move the detail panel: the user is ticking. */}
      <td className={CELL} onClick={(event) => event.stopPropagation()}>
        <input
          type="checkbox"
          aria-label={`Select ${item.title}`}
          checked={selected}
          onChange={(event) =>
            onToggle(item.id, event.nativeEvent instanceof MouseEvent && event.nativeEvent.shiftKey)
          }
          // The box's own keys: the row would answer Space again, and Enter would move the
          // detail panel from under a user who is ticking.
          onKeyDown={(event) => {
            if (event.key === ' ' || event.key === 'Enter') event.stopPropagation();
          }}
          className={CHECKBOX_CLASS}
        />
      </td>
      <td className={`max-w-0 ${CELL}`}>
        <div className="flex min-w-0 flex-col">
          <span className="truncate" title={item.title}>
            {item.title}
          </span>
          {subtitle.length > 0 && (
            <span className="truncate text-xs text-muted">{subtitle.join(' · ')}</span>
          )}
        </div>
      </td>
      <td className={CELL}>
        <VerdictBadge level={item.verdict.level} />
      </td>
      <td className={NUMERIC}>{itemSize(item)}</td>
      <td className={`${NUMERIC} text-muted`}>
        {item.lastUsed === null ? '—' : formatStampDate(item.lastUsed)}
      </td>
    </tr>
  );
});

/** Hands the page a selection only when it differs: a click that changed nothing is not news. */
function report(
  current: ReadonlySet<string>,
  next: ReadonlySet<string>,
  change: (selection: ReadonlySet<string>) => void,
): void {
  const same = next.size === current.size && [...next].every((id) => current.has(id));
  if (!same) change(next);
}

/**
 * What the modules found, one row per item: a tick box, the title, the verdict, the size and
 * the last use. Ticked the way the Explorer's table is — Space on a row, shift-click on a box
 * for a range, the header box over what is shown — and a click or Enter shows the row in the
 * detail panel.
 */
export default function ItemTable({
  items,
  total,
  moduleNames,
  selection,
  onSelectionChange,
  focused,
  onFocus,
}: ItemTableProps) {
  // The row a range extends from, by id, as in the Explorer's table.
  const anchor = useRef<string | null>(null);
  const shown = useRef({ items, selection, onSelectionChange });
  useLayoutEffect(() => {
    shown.current = { items, selection, onSelectionChange };
  });

  const toggle = useCallback((id: string, extend: boolean) => {
    const { items, selection, onSelectionChange } = shown.current;
    const index = (target: string | null) => items.findIndex((item) => item.id === target);
    const from =
      anchor.current !== null && selection.has(anchor.current) ? index(anchor.current) : -1;
    const to = index(id);
    anchor.current = id;
    const next = new Set(selection);
    if (extend && from !== -1 && to !== -1) {
      for (let i = Math.min(from, to); i <= Math.max(from, to); i += 1) {
        next.add(items[i].id);
      }
    } else if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    report(selection, next, onSelectionChange);
  }, []);

  const ticked = items.filter((item) => selection.has(item.id)).length;
  const all = ticked > 0 && ticked === items.length;
  const some = ticked > 0 && ticked < items.length;

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

  const header = `${CELL} font-medium text-muted`;
  return (
    <div className="overflow-hidden rounded-lg border border-neutral-200 bg-white dark:border-neutral-800 dark:bg-neutral-900">
      <table className="w-full table-fixed border-collapse text-base">
        <thead className="bg-neutral-50 text-sm dark:bg-neutral-800/60">
          <tr>
            <th scope="col" aria-label="Select" className={`w-9 ${header}`}>
              <input
                type="checkbox"
                aria-label={total > items.length ? 'Select all shown' : 'Select all'}
                checked={all}
                ref={(box) => {
                  if (box !== null) box.indeterminate = some;
                }}
                onChange={() =>
                  report(
                    selection,
                    all ? new Set() : new Set(items.map((item) => item.id)),
                    onSelectionChange,
                  )
                }
                className={CHECKBOX_CLASS}
              />
            </th>
            <th scope="col" className={`${header} text-left`}>
              Name
            </th>
            <th scope="col" className={`w-24 ${header} text-left`}>
              Verdict
            </th>
            <th scope="col" className={`w-24 ${header} text-right`}>
              Size
            </th>
            <th scope="col" className={`w-28 ${header} text-right`}>
              Last used
            </th>
          </tr>
        </thead>
        {items.length > 0 ? (
          <tbody data-testid="item-rows" onKeyDown={onBodyKeyDown}>
            {items.map((item) => (
              <Row
                key={item.id}
                item={item}
                module={moduleNames.get(item.module) ?? item.module}
                selected={selection.has(item.id)}
                focused={focused === item.id}
                onToggle={toggle}
                onFocus={onFocus}
              />
            ))}
          </tbody>
        ) : (
          <tbody>
            <tr className="border-t border-neutral-100 dark:border-neutral-800">
              <td colSpan={5} className="px-3 py-8 text-center text-muted">
                Nothing here with these filters
              </td>
            </tr>
          </tbody>
        )}
      </table>
      <p className="border-t border-neutral-100 px-3 py-1.5 text-xs text-muted dark:border-neutral-800">
        {countLabel(items.length, 'item')}
        {total > items.length && ` shown of ${total.toLocaleString('en-US')}`}
      </p>
    </div>
  );
}
