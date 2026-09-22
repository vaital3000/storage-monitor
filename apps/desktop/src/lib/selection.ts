// The Cleanup screen's selection: which items are ticked, and what is to be done to each —
// the action and the options, the defaults until the detail panel says otherwise.

import type { CleanupRequest, Item } from './ipc';

/** What is to be done to one item. */
export interface Choice {
  action: string;
  /** The ids of the options turned on. */
  options: readonly string[];
}

/** The first action, with the options that start turned on: what a tick asks for by itself. */
export function defaultChoice(item: Item): Choice {
  const [action] = item.actions;
  if (action === undefined) {
    return { action: '', options: [] };
  }
  return {
    action: action.id,
    options: action.options.filter((option) => option.default).map((option) => option.id),
  };
}

/** The choice for `item`: the one the detail panel made, or the default. */
export function choiceFor(item: Item, choices: ReadonlyMap<string, Choice>): Choice {
  return choices.get(item.id) ?? defaultChoice(item);
}

/**
 * The ticks that are still on screen. A tick the filters hide is dropped, not kept aside:
 * a batch is always exactly the ticked rows the user can see, and a filter turned back on
 * does not bring back a selection nobody was looking at.
 */
export function prune(selection: ReadonlySet<string>, visible: readonly Item[]): Set<string> {
  const shown = new Set(visible.map((item) => item.id));
  return new Set([...selection].filter((id) => shown.has(id)));
}

/**
 * The requests of a batch, in the order the rows are shown — largest first — so that the
 * dialog lists them the way the table did.
 */
export function requestsOf(
  items: readonly Item[],
  selection: ReadonlySet<string>,
  choices: ReadonlyMap<string, Choice>,
): CleanupRequest[] {
  return items
    .filter((item) => selection.has(item.id))
    .map((item) => {
      const { action, options } = choiceFor(item, choices);
      return { item: item.id, action, options: [...options] };
    });
}

/** What the ticked rows promise to free: the chosen action's estimate, for each of them. */
export function selectedBytes(
  items: readonly Item[],
  selection: ReadonlySet<string>,
  choices: ReadonlyMap<string, Choice>,
): number {
  return items
    .filter((item) => selection.has(item.id))
    .reduce((sum, item) => {
      const { action } = choiceFor(item, choices);
      return sum + (item.actions.find((spec) => spec.id === action)?.estimatedFree ?? 0);
    }, 0);
}
