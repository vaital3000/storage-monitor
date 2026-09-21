import { describe, expect, it } from 'vitest';
import {
  BLOCK_REASONS,
  DELETION_MODES,
  LOG_RESULTS,
  NODE_KINDS,
  logDetail,
  type ActivityEntry,
} from './ipc';

const entry = (fields: Partial<ActivityEntry>): ActivityEntry => ({
  at: '2026-09-18T09:30:00Z',
  path: '/Users/demo/Movies',
  kind: 'dir',
  mode: 'trash',
  result: 'removed',
  detail: null,
  bytes: 0,
  ...fields,
});

describe('the wire vocabularies', () => {
  it('carry every variant of the Rust enum they mirror', () => {
    expect(NODE_KINDS).toEqual(['dir', 'file', 'symlink', 'other']);
    expect(DELETION_MODES).toEqual(['trash', 'permanent']);
    expect(LOG_RESULTS).toEqual(['removed', 'failed', 'skipped']);
    // All nine of `BlockReason`, in the order `crates/core/src/action/model.rs` declares
    // them. A screen that renders a label per reason reads this list.
    expect(BLOCK_REASONS).toEqual([
      'outsideRoots',
      'denylisted',
      'shielded',
      'malformed',
      'isRoot',
      'nested',
      'missing',
      'unreadable',
      'kindChanged',
    ]);
  });
});

describe('logDetail', () => {
  it('reads a failure as a message', () => {
    expect(logDetail(entry({ result: 'failed', detail: 'cannot move to the Trash' }))).toEqual({
      kind: 'message',
      message: 'cannot move to the Trash',
    });
  });

  it('reads a skipped entry as a block reason', () => {
    expect(logDetail(entry({ result: 'skipped', detail: 'denylisted' }))).toEqual({
      kind: 'reason',
      reason: 'denylisted',
    });
  });

  it('reads result first: a failure whose message is a reason is still a message', () => {
    // The two kinds of string are not distinguishable by looking at them, which is why
    // nothing may read `detail` without `result`.
    expect(logDetail(entry({ result: 'failed', detail: 'denylisted' }))).toEqual({
      kind: 'message',
      message: 'denylisted',
    });
  });

  it('has nothing to say about a removed entry', () => {
    expect(logDetail(entry({ result: 'removed', detail: null }))).toBeNull();
    // A `removed` line carries no detail; one that does is a line this version cannot read,
    // and putting the string on screen would be worse than saying nothing.
    expect(logDetail(entry({ result: 'removed', detail: 'denylisted' }))).toBeNull();
  });

  it('says nothing about a reason from a later version', () => {
    expect(logDetail(entry({ result: 'skipped', detail: 'quarantined' }))).toBeNull();
  });

  it('accepts every reason this version knows', () => {
    for (const reason of BLOCK_REASONS) {
      expect(logDetail(entry({ result: 'skipped', detail: reason }))).toEqual({
        kind: 'reason',
        reason,
      });
    }
  });
});
