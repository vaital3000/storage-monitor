import { describe, expect, it } from 'vitest';
import { UNKNOWN_BLOCK_REASON, describeBlock } from './blockReasons';
import { BLOCK_REASONS, type BlockReason } from './ipc';

/**
 * The vocabulary is shared by the confirmation dialog and the Activity screen, so its lock
 * lives beside it rather than inside either consumer: a test that reaches it through one
 * of them stops pinning anything the day that component is rewritten, and the other screen
 * would go on rendering whatever was left.
 */
describe('describeBlock', () => {
  it('says each of the eight reasons in its own words', () => {
    // Pinned pairwise, and not by a shape a permutation would also satisfy. The pair the
    // backend most insists on is `missing` against `unreadable` (`ipc.ts`): one sends the
    // user hunting for a file that is gone, the other to grant Full Disk Access.
    const words: Record<BlockReason, string> = {
      outsideRoots: 'Outside the folder that was scanned',
      denylisted: 'Inside a folder this app never deletes from',
      malformed: 'Not a path that names an entry',
      isRoot: 'The scanned folder itself, or one above it',
      nested: 'Another entry contains it',
      missing: 'Nothing is there any more',
      unreadable: 'Cannot be read — it may need Full Disk Access',
      kindChanged: 'No longer what the preview saw',
    };
    // A ninth reason mirrored into `ipc.ts` has to arrive here too, rather than falling
    // through to the unknown-variant line that exists for older builds in the wild.
    expect(Object.keys(words).sort()).toEqual([...BLOCK_REASONS].sort());

    for (const reason of BLOCK_REASONS) {
      expect(describeBlock(reason)).toBe(words[reason]);
    }
  });

  it('names a reason this build does not know instead of answering nothing', () => {
    // A ninth `BlockReason`, added in Rust and mirrored in `ipc.ts` after this build was
    // made. Both screens put this string in front of a user, so it is a sentence and not
    // a wire name — and never the empty string, which is what the lookup alone gives.
    expect(describeBlock('quarantined' as BlockReason)).toBe(UNKNOWN_BLOCK_REASON);
    expect(UNKNOWN_BLOCK_REASON).toBe('Blocked for a reason this version does not know');
  });
});
