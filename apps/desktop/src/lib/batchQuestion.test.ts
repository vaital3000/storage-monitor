import { describe, expect, it } from 'vitest';
import {
  questionsFromCleanup,
  questionsFromPreview,
  reportFromBatch,
  reportFromCleanup,
} from './batchQuestion';
import type { CleanupEntry, CleanupPreviews, CleanupResult } from './ipc';

describe('questionsFromPreview', () => {
  const preview = {
    entries: [
      { path: '/Users/demo/a', kind: 'dir', size: 10, status: { state: 'ready' } },
      {
        path: '/Users/demo/b',
        kind: 'dir',
        size: 5,
        status: { state: 'blocked', reason: 'missing' },
      },
    ],
    totalBytes: 10,
  } as const;

  it('asks the same rows in both modes, irreversible only when permanent', () => {
    const questions = questionsFromPreview(preview);
    expect(questions.trash.rows.map((row) => row.title)).toEqual([
      '/Users/demo/a',
      '/Users/demo/b',
    ]);
    expect(questions.permanent.rows.map((row) => row.status)).toEqual(
      questions.trash.rows.map((row) => row.status),
    );
    expect(questions.trash.rows.every((row) => !row.irreversible)).toBe(true);
    expect(questions.permanent.rows.every((row) => row.irreversible)).toBe(true);
    expect(questions.trash.totalBytes).toBe(10);
    expect(questions.trash.rows[0]).not.toHaveProperty('steps');
    expect(questions.wording).toBe('delete');
    expect(questions.scope).toBe('scan');
  });

  it('reports every entry in the mode of the batch, titled by its path', () => {
    const report = reportFromBatch({
      outcome: {
        entries: [{ path: '/Users/demo/a', kind: 'dir', result: { result: 'removed', bytes: 10 } }],
        freedBytes: 10,
        at: '2026-09-19T10:00:00Z',
        mode: 'permanent',
      },
      recorded: false,
      treeStale: true,
    });
    expect(report.rows).toEqual([
      { title: '/Users/demo/a', mode: 'permanent', result: { result: 'removed', bytes: 10 } },
    ]);
    expect(report).toMatchObject({ freedBytes: 10, recorded: false, treeStale: true });
  });
});

describe('questionsFromCleanup', () => {
  const entry = (fields: Partial<CleanupEntry>): CleanupEntry => ({
    item: 'demo:old.object',
    module: 'demo',
    title: 'old.object',
    action: 'Remove object',
    steps: [{ step: 'run', command: 'rm /x', effect: 'removes', path: '/x' }],
    size: 7,
    status: { state: 'ready' },
    reversible: false,
    ...fields,
  });

  const previews: CleanupPreviews = {
    trash: {
      entries: [entry({}), entry({ item: 'demo:gone', module: '', title: 'demo:gone', steps: [] })],
      totalBytes: 7,
      mode: 'trash',
    },
    permanent: { entries: [entry({})], totalBytes: 7, mode: 'permanent' },
  };

  it('titles a row by its item, with the module and the action as context', () => {
    const questions = questionsFromCleanup(previews, new Map([['demo', 'Demo']]));
    expect(questions.trash.rows[0]).toMatchObject({
      title: 'old.object',
      context: 'Demo · Remove object',
      size: 7,
      irreversible: true,
    });
    expect(questions.trash.rows[0].steps).toEqual(previews.trash.entries[0].steps);
    // An item that is not held any more has no module to name.
    expect(questions.trash.rows[1].context).toBe('Remove object');
    expect(questions.wording).toBe('clean');
    expect(questions.scope).toBe('home');
  });

  it('falls back to the module id for a module it has no name for', () => {
    expect(questionsFromCleanup(previews, new Map()).trash.rows[0].context).toBe(
      'demo · Remove object',
    );
  });

  it('reports every row in the mode its entry really left in', () => {
    const result: CleanupResult = {
      outcome: {
        entries: [
          {
            item: 'demo:a',
            module: 'demo',
            title: 'a',
            action: 'Delete folder',
            path: '/x/a',
            kind: 'dir',
            targets: ['/x/a'],
            mode: 'trash',
            commands: [],
            result: { result: 'removed', bytes: 1 },
          },
          {
            item: 'demo:b.object',
            module: 'demo',
            title: 'b.object',
            action: 'Remove object',
            path: '/x/b.object',
            kind: 'file',
            targets: ['/x/b.object'],
            mode: 'permanent',
            commands: [['rm', '/x/b.object']],
            result: { result: 'removed', bytes: 2 },
          },
        ],
        freedBytes: 3,
        at: '2026-09-19T10:00:00Z',
        mode: 'trash',
      },
      recorded: true,
      treeStale: false,
    };
    const report = reportFromCleanup(result);
    expect(report.rows.map((row) => [row.title, row.mode])).toEqual([
      ['a', 'trash'],
      ['b.object', 'permanent'],
    ]);
    expect(report).toMatchObject({ mode: 'trash', wording: 'clean', scope: 'home' });
  });
});
