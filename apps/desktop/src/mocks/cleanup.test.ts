import { beforeEach, describe, expect, it } from 'vitest';
import {
  cleanupItems,
  cleanupPreview,
  cleanupRun,
  modulesList,
  modulesRefresh,
  onCleanupProgress,
  onModulesState,
  type ActionOption,
  type CleanupProgress,
  type CleanupRequest,
  type Item,
  type ModuleView,
  type StepEffect,
  type VerdictLevel,
} from '../lib/ipc';
import sharedCases from '../../../../crates/core/tests/fixtures/cleanup-cases.json';
import { mockActionLog, mockActivityTail } from './actionLog';
import { commandLine } from '../lib/commandLine';
import { reversible, screen } from './cleanup';
import { installIpcMock } from './ipc';
import { DEMO_SANDBOX } from './modules';

/** Resolves once `modules:state` has said `status` about the demo. */
function waitFor(status: ModuleView['status']): Promise<ModuleView> {
  return new Promise((resolve) => {
    void onModulesState((view) => {
      if (view.id === 'demo' && view.status === status) resolve(view);
    });
  });
}

/** Installs the mock and discovers the demo, the way the Cleanup page does on its first open. */
async function discovered(): Promise<Item[]> {
  installIpcMock();
  const ready = waitFor('ready');
  await modulesRefresh();
  await ready;
  return (await cleanupItems()).items;
}

const request = (name: string, action: string, options: string[] = []): CleanupRequest => ({
  item: `demo:${name}`,
  action,
  options,
});

/** A folder and an object, the pair the Playwright spec cleans. */
const pair = () => [request('build-cache', 'delete'), request('old.object', 'remove')];

describe('the shared cleanup cases', () => {
  /** An item of the cases: one action, `go`, with the options the case lists. */
  function caseItem(entry: {
    id: string;
    level: string;
    options: Array<{ id: string; force: boolean }>;
  }): Item {
    const options: ActionOption[] = entry.options.map((option) => ({
      id: option.id,
      label: option.id,
      default: false,
      force: option.force,
    }));
    return {
      id: entry.id,
      module: 'test',
      kind: 'thing',
      title: entry.id,
      subtitle: null,
      path: null,
      size: { bytes: 0, estimated: false },
      lastUsed: null,
      verdict: { level: entry.level as VerdictLevel, reasons: [] },
      facts: [],
      actions: [{ id: 'go', label: 'Go', estimatedFree: 0, options }],
    };
  }

  it('are answered the way the engine answers them', () => {
    const held = new Map(sharedCases.items.map((entry) => [entry.id, caseItem(entry)]));
    expect(sharedCases.screen.length).toBeGreaterThan(0);
    for (const entry of sharedCases.screen) {
      const answer = screen(entry.request, held);
      expect('reason' in answer ? answer.reason : 'ok', entry.name).toBe(entry.expect);
    }
    expect(sharedCases.reversible.length).toBeGreaterThan(0);
    for (const entry of sharedCases.reversible) {
      const steps = entry.steps as Array<'delete' | StepEffect>;
      expect(reversible(steps, 'trash'), `${entry.name} (trash)`).toBe(entry.trash);
      expect(reversible(steps, 'permanent'), `${entry.name} (permanent)`).toBe(entry.permanent);
    }
  });
});

describe('commandLine', () => {
  it('quotes the way the backend does, for reading', () => {
    expect(commandLine(['git', '-C', '/h/repo', 'worktree', 'prune'])).toBe(
      'git -C /h/repo worktree prune',
    );
    expect(commandLine(['rm', '/h/Application Support/x.object'])).toBe(
      "rm '/h/Application Support/x.object'",
    );
    expect(commandLine(['echo', "it's"])).toBe("echo 'it'\\''s'");
    expect(commandLine(['tool', ''])).toBe("tool ''");
    expect(commandLine(['echo', '$HOME', '*', 'a;b'])).toBe("echo '$HOME' '*' 'a;b'");
  });
});

describe('the demo module of the mock', () => {
  beforeEach(() => {
    installIpcMock();
  });

  it('is idle until refreshed, then discovers its seed', async () => {
    const [idle] = await modulesList();
    expect(idle).toMatchObject({ id: 'demo', status: 'idle', itemCount: 0 });
    const items = await discovered();
    expect(items.map((item) => item.title)).toEqual([
      'fresh.object',
      'build-cache',
      'logs',
      'old.object',
      'keepsake',
    ]);
    const [ready] = await modulesList();
    expect(ready.status).toBe('ready');
    expect(ready.itemCount).toBe(5);
    expect(ready.safeBytes).toBe(2_015_232 + 1_003_520);
  });

  it('judges its items the way the Rust module does', async () => {
    const items = await discovered();
    const byTitle = new Map(items.map((item) => [item.title, item]));
    expect(byTitle.get('build-cache')?.verdict).toEqual({
      level: 'safe',
      reasons: [{ code: 'stale', text: 'Not modified for 45 days' }],
    });
    expect(byTitle.get('logs')?.verdict.reasons[0].text).toBe('Modified 3 days ago');
    expect(byTitle.get('keepsake')?.verdict.level).toBe('keep');
    expect(byTitle.get('keepsake')?.actions[0].options).toEqual([
      expect.objectContaining({ id: 'force', force: true }),
    ]);
    expect(byTitle.get('fresh.object')?.size).toEqual({ bytes: 3_002_368, estimated: true });
    expect(byTitle.get('build-cache')?.path).toBe(`${DEMO_SANDBOX}/build-cache`);
  });
});

describe('a cleanup preview of the mock', () => {
  it('answers both modes, aligned, with the steps of each', async () => {
    await discovered();
    const { trash, permanent } = await cleanupPreview(pair());
    expect(trash.entries.map((entry) => entry.title)).toEqual(['build-cache', 'old.object']);
    expect(permanent.entries.map((entry) => entry.title)).toEqual(['build-cache', 'old.object']);
    expect(trash.entries[0].steps).toEqual([
      { step: 'trash', path: `${DEMO_SANDBOX}/build-cache` },
      {
        step: 'run',
        command: `touch '${DEMO_SANDBOX}/.last-cleanup'`,
        effect: 'housekeeping',
        path: null,
      },
    ]);
    expect(permanent.entries[0].steps).toEqual([
      { step: 'delete', path: `${DEMO_SANDBOX}/build-cache` },
    ]);
    expect(trash.entries[0].reversible).toBe(true);
    expect(trash.entries[1]).toMatchObject({ reversible: false, action: 'Remove object' });
    expect(trash.entries[1].steps).toEqual(permanent.entries[1].steps);
    expect(trash.totalBytes).toBe(2_015_232 + 1_003_520);
  });

  it('refuses a keep item until its force option is on', async () => {
    await discovered();
    const kept = await cleanupPreview([request('keepsake', 'delete')]);
    expect(kept.trash.entries[0].status).toEqual({ state: 'blocked', reason: 'kept' });
    expect(kept.trash.entries[0].steps).toEqual([]);
    const forced = await cleanupPreview([request('keepsake', 'delete', ['force'])]);
    expect(forced.trash.entries[0].status).toEqual({ state: 'ready' });
  });

  it('refuses an item that is not held, and one asked for with an action it does not have', async () => {
    await discovered();
    const { trash } = await cleanupPreview([
      request('nothing', 'delete'),
      request('logs', 'remove'),
    ]);
    expect(trash.entries[0]).toMatchObject({
      title: 'demo:nothing',
      module: '',
      status: { state: 'blocked', reason: 'missing' },
    });
    expect(trash.entries[1].status).toEqual({ state: 'blocked', reason: 'kindChanged' });
    expect(trash.totalBytes).toBe(0);
  });

  it('touches nothing', async () => {
    await discovered();
    await cleanupPreview(pair());
    expect(mockActionLog).toHaveLength(0);
    expect((await cleanupItems()).items).toHaveLength(5);
  });
});

describe('a cleanup batch of the mock', () => {
  it('cleans, reports progress, records, forgets and rediscovers', async () => {
    await discovered();
    const progress: CleanupProgress[] = [];
    await onCleanupProgress((step) => progress.push(step));
    const rediscovered = waitFor('ready');

    const { outcome, recorded, treeStale } = await cleanupRun(pair(), 'trash');
    expect(outcome.entries.map((entry) => entry.result)).toEqual([
      { result: 'removed', bytes: 2_015_232 },
      { result: 'removed', bytes: 1_003_520 },
    ]);
    expect(outcome.freedBytes).toBe(2_015_232 + 1_003_520);
    expect(outcome.entries.map((entry) => entry.mode)).toEqual(['trash', 'permanent']);
    expect(outcome.entries[1].commands).toEqual([['rm', `${DEMO_SANDBOX}/old.object`]]);
    expect(outcome.entries[0].targets).toEqual([`${DEMO_SANDBOX}/build-cache`]);
    expect(recorded).toBe(true);
    expect(treeStale).toBe(false);

    await rediscovered;
    expect(progress.map((step) => [step.done, step.total, step.current])).toEqual([
      [0, 2, 'build-cache'],
      [1, 2, 'old.object'],
      [2, 2, null],
    ]);
    const titles = (await cleanupItems()).items.map((item) => item.title);
    expect(titles).toEqual(['fresh.object', 'logs', 'keepsake']);

    const tail = mockActivityTail(10);
    expect(tail.entries).toHaveLength(2);
    expect(tail.entries[0]).toMatchObject({
      path: `${DEMO_SANDBOX}/old.object`,
      kind: 'file',
      mode: 'permanent',
      result: 'removed',
      source: {
        module: 'demo',
        item: 'demo:old.object',
        title: 'old.object',
        action: 'Remove object',
      },
      commands: [['rm', `${DEMO_SANDBOX}/old.object`]],
    });
    expect(tail.entries[1]).toMatchObject({ kind: 'dir', mode: 'trash' });
  });

  it('skips a kept item and says why in the record', async () => {
    await discovered();
    const { outcome } = await cleanupRun([request('keepsake', 'delete')], 'permanent');
    expect(outcome.entries[0].result).toEqual({ result: 'skipped', reason: 'kept' });
    expect(mockActivityTail(10).entries[0]).toMatchObject({
      result: 'skipped',
      detail: 'kept',
      bytes: 0,
    });
    expect((await cleanupItems()).items.map((item) => item.title)).toContain('keepsake');
  });
});
