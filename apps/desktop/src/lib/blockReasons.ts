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
  malformed: 'Not a path that names an entry',
  isRoot: 'The scanned folder itself, or one above it',
  nested: 'Another entry contains it',
  missing: 'Nothing is there any more',
  unreadable: 'Cannot be read — it may need Full Disk Access',
  kindChanged: 'No longer what the preview saw',
};

/**
 * What a verdict this build has no words for says. A `BlockReason` added to the backend
 * after this build was made arrives as a string with no entry above, and the two places a
 * user must not be shown a blank line are the list of what is about to be deleted and the
 * record of what was.
 */
export const UNKNOWN_BLOCK_REASON = 'Blocked for a reason this version does not know';

/** What a verdict says on screen. */
export function describeBlock(reason: BlockReason): string {
  return BLOCK_REASON_LABELS[reason] ?? UNKNOWN_BLOCK_REASON;
}
