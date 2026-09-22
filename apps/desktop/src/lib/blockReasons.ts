// The guards' verdicts in words. Two screens say them: the confirmation dialog, about a
// batch that has not run, and the Activity screen, about one that did — which is why the
// vocabulary lives here rather than in either of them, next to `nodeErrors.ts`, which does
// the same job for the walker's errors.

import type { BlockReason } from './ipc';

/**
 * Every `BlockReason` in words, from the guards' own vocabulary
 * (`crates/core/src/action/model.rs`).
 *
 * `missing` and `unreadable` are kept apart to the letter, because that is the whole point
 * of the backend telling them apart: one says the entry is gone, the other that something
 * on the way to it could not be read, and folding them together sends a user hunting for a
 * ghost instead of granting Full Disk Access.
 *
 * `nested` keeps the backend's phrasing even though an exact duplicate lands here too and
 * contains nothing: that trade-off is taken and explained in `engine.rs`, and a special
 * case here would only hide a verdict the Activity screen still reports plainly.
 */
const BLOCK_REASON_LABELS: Record<BlockReason, string> = {
  outsideRoots: 'Outside the folder that was scanned',
  denylisted: 'Inside a folder this app never deletes from',
  shielded: 'Not as a whole — open it and choose what inside',
  malformed: 'Not a path that names an entry',
  isRoot: 'The scanned folder itself, or one above it',
  nested: 'Another entry contains it',
  missing: 'Nothing is there any more',
  unreadable: 'Cannot be read — it may need Full Disk Access',
  kindChanged: 'No longer what the preview saw',
  kept: 'Marked keep — turn on its force option to clean it anyway',
};

/**
 * Where a batch's guards were built from, which two of the reasons name: the scanned folder
 * for a batch of the Explorer, and the home folder for a batch of Cleanup, whose modules may
 * delete anywhere in it (phase 2b design, section 8). Settings replaces the one root with a
 * list in phase 2c, and these words with it.
 */
export type GuardScope = 'scan' | 'home';

/** The two reasons that name the root, as a batch of Cleanup says them. */
const HOME_LABELS: Partial<Record<BlockReason, string>> = {
  outsideRoots: 'Outside the home folder',
  isRoot: 'The home folder itself, or one above it',
};

/**
 * What a verdict this build has no words for says. A `BlockReason` added to the backend
 * after this build was made arrives as a string with no entry above, and the two places a
 * user must not be shown a blank line are the list of what is about to be deleted and the
 * record of what was.
 */
export const UNKNOWN_BLOCK_REASON = 'Blocked for a reason this version does not know';

/** What a verdict says on screen, for a batch whose guards were built from `scope`. */
export function describeBlock(reason: BlockReason, scope: GuardScope = 'scan'): string {
  const home = scope === 'home' ? HOME_LABELS[reason] : undefined;
  return home ?? BLOCK_REASON_LABELS[reason] ?? UNKNOWN_BLOCK_REASON;
}
