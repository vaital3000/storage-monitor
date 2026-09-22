// What the Cleanup screen says about an item in more than one place: its size and its
// verdict, in words.

import { formatBytes } from './format';
import type { Item, VerdictLevel } from './ipc';

export const VERDICT_LABELS: Record<VerdictLevel, string> = {
  safe: 'Safe',
  review: 'Review',
  keep: 'Keep',
};

/** An item's size, with `~` when the module could only estimate it. */
export function itemSize(item: Item): string {
  const size = formatBytes(item.size.bytes);
  return item.size.estimated ? `~${size}` : size;
}
