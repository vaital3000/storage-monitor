import type { VerdictLevel } from '../lib/ipc';
import { VERDICT_LABELS } from '../lib/items';

const TONES: Record<VerdictLevel, string> = {
  safe: 'bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300',
  review: 'bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300',
  keep: 'bg-neutral-200 text-neutral-700 dark:bg-neutral-800 dark:text-neutral-300',
};

/** A module's verdict on an item, in a word and a colour — the word first, for whoever cannot see the colour. */
export default function VerdictBadge({ level }: { level: VerdictLevel }) {
  return (
    <span
      data-level={level}
      className={`inline-block rounded px-1.5 py-0.5 text-xs font-medium ${TONES[level]}`}
    >
      {VERDICT_LABELS[level]}
    </span>
  );
}
