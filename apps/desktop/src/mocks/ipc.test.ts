import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  actionPreview,
  actionRun,
  activityLog,
  defaultRoot,
  diskUsage,
  getAppInfo,
  onScanDone,
  onScanProgress,
  revealInFinder,
  scanCancel,
  scanStart,
  scanStatus,
  topGrowers,
  treeNode,
  type ScanStatus,
} from '../lib/ipc';
import {
  FIXTURE_ROOT,
  fixtureDisk,
  fixtureGrowers,
  fixtureNode,
  fixtureNodes,
  fixtureStatusDone,
} from './fixtures';
import {
  installIpcMock,
  mockScanDelayMs,
  resetIpcMock,
  revealed,
  setMockScanDelay,
  setMockScanFailure,
} from './ipc';

/** Subscribes to both scan events; `done` resolves with the payload of `scan:done`. */
async function subscribe() {
  const progress: ScanStatus[] = [];
  let finish!: (status: ScanStatus) => void;
  const done = new Promise<ScanStatus>((resolve) => {
    finish = resolve;
  });
  await onScanProgress((status) => progress.push(status));
  await onScanDone(finish);
  return { progress, done };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 5));

/** The mocked `invoke`, for commands and arguments the typed layer does not expose. */
function rawInvoke(cmd: string, args: unknown = {}): Promise<unknown> {
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: { invoke(cmd: string, args?: unknown): Promise<unknown> };
    }
  ).__TAURI_INTERNALS__;
  return internals.invoke(cmd, args);
}

beforeEach(() => {
  installIpcMock();
  setMockScanDelay(0);
});

describe('installIpcMock', () => {
  it('keeps get_app_info and the fixed defaults', async () => {
    expect(await getAppInfo()).toEqual({ name: 'Storage Monitor', version: '0.0.0-mock' });
    expect(await defaultRoot()).toBe(FIXTURE_ROOT);
    expect(await diskUsage()).toEqual(fixtureDisk());
    expect((await diskUsage('/Volumes/Data')).path).toBe('/Volumes/Data');
  });

  it('starts idle with no tree and no growers', async () => {
    expect((await scanStatus()).state).toBe('idle');
    await expect(treeNode()).rejects.toBe('no scan result');
    expect(await topGrowers()).toEqual([]);
  });

  it('rejects an unknown command', async () => {
    await expect(rawInvoke('nope')).rejects.toThrow('Unmocked IPC command: nope');
  });
});

describe('a simulated scan', () => {
  it('resolves running, ticks progress and ends done', async () => {
    const { progress, done } = await subscribe();
    const started = await scanStart();
    expect(started.state).toBe('running');
    expect(started.root).toBe(FIXTURE_ROOT);
    expect(progress).toEqual([]);

    const final = await done;
    expect(final.state).toBe('done');
    expect(final.root).toBe(FIXTURE_ROOT);
    expect(final.bytes).toBe(fixtureNodes[0].size);
    expect(final.currentPath).toBe('');
    expect(final.hasPrevious).toBe(true);
    expect(final.previousTakenAt).not.toBeNull();
    expect(final.error).toBeNull();

    expect(progress).toHaveLength(6);
    for (const [i, status] of progress.entries()) {
      expect(status.state).toBe('running');
      expect(status.currentPath.startsWith(FIXTURE_ROOT)).toBe(true);
      if (i > 0) {
        expect(status.files).toBeGreaterThan(progress[i - 1].files);
        expect(status.bytes).toBeGreaterThan(progress[i - 1].bytes);
      }
    }
    expect(new Set(progress.map((s) => s.currentPath)).size).toBe(6);
    expect(progress[progress.length - 1].bytes).toBeLessThan(final.bytes);

    const status = await scanStatus();
    expect(status.state).toBe('done');
    expect(status).toEqual(final);
    expect(await topGrowers()).toEqual(fixtureGrowers().slice(0, 10));
    expect(await topGrowers(2)).toHaveLength(2);
  });

  it('echoes a custom root and refuses a second scan while running', async () => {
    const { done } = await subscribe();
    expect((await scanStart('/Volumes/Data')).root).toBe('/Volumes/Data');
    await expect(scanStart()).rejects.toBe('a scan is already running');
    await expect(treeNode()).rejects.toBe('no scan result');
    expect((await done).root).toBe('/Volumes/Data');
  });

  it('can be cancelled: the ticks stop and scan:done reports cancelled', async () => {
    const { progress, done } = await subscribe();
    await scanStart();
    const acknowledged = await scanCancel();
    expect(acknowledged.state).toBe('running');
    const final = await done;
    expect(final.state).toBe('cancelled');
    expect(final.hasPrevious).toBe(false);
    expect(final.currentPath).toBe('');
    const ticks = progress.length;
    expect(ticks).toBeLessThan(6);
    await tick();
    expect(progress).toHaveLength(ticks);
    expect((await scanStatus()).state).toBe('cancelled');
    expect((await treeNode()).name).toBe('demo');
    expect(await topGrowers()).toEqual([]);
  });

  it('ignores cancel while idle', async () => {
    const { done } = await subscribe();
    const spy = vi.fn();
    void done.then(spy);
    expect((await scanCancel()).state).toBe('idle');
    await tick();
    expect(spy).not.toHaveBeenCalled();
  });

  it('stops delivering to a listener that unsubscribed, without a warning', async () => {
    const warn = vi.spyOn(console, 'warn');
    try {
      const spy = vi.fn();
      const unlisten = await onScanProgress(spy);
      await unlisten();
      const { done } = await subscribe();
      await scanStart();
      await done;
      expect(spy).not.toHaveBeenCalled();
      expect(warn).not.toHaveBeenCalled();
    } finally {
      warn.mockRestore();
    }
  });

  it('exposes the test hooks on the window for Playwright', async () => {
    expect(window.__STORAGE_MONITOR_MOCK__?.revealed).toBe(revealed);
    window.__STORAGE_MONITOR_MOCK__?.setMockScanDelay(0);
    expect(mockScanDelayMs).toBe(0);
    expect(window.__STORAGE_MONITOR_MOCK__?.setMockScanFailure).toBe(setMockScanFailure);
    await revealInFinder('/x');
    expect(window.__STORAGE_MONITOR_MOCK__?.revealed).toEqual(['/x']);
  });
});

describe('tree_node after a scan', () => {
  beforeEach(async () => {
    const { done } = await subscribe();
    await scanStart();
    await done;
  });

  it('returns the fixture root with sorted children and one breadcrumb', async () => {
    const root = await treeNode();
    expect(root.id).toBe(0);
    expect(root.name).toBe('demo');
    expect(root.path).toBe(FIXTURE_ROOT);
    expect(root.breadcrumbs).toEqual([{ id: 0, name: 'demo' }]);
    const sizes = root.children.map((c) => c.size);
    expect(sizes).toEqual([...sizes].sort((a, b) => b - a));
    expect(root.children[0].name).toBe('Library');
    expect(root.truncated).toBe(false);
    expect((await treeNode(undefined, 3)).children).toHaveLength(3);
    expect((await treeNode(undefined, 3)).truncated).toBe(true);
  });

  it('returns a child with two breadcrumbs', async () => {
    const root = await treeNode();
    const child = root.children[0];
    const view = await treeNode(child.id);
    expect(view.id).toBe(child.id);
    expect(view.breadcrumbs).toEqual([
      { id: 0, name: 'demo' },
      { id: child.id, name: child.name },
    ]);
    expect(view.children.length).toBeGreaterThan(0);
  });

  it('rejects an unknown node', async () => {
    await expect(treeNode(999)).rejects.toBe('unknown node 999');
  });
});

describe('revealInFinder', () => {
  it('records the revealed path', async () => {
    await revealInFinder('/x');
    await revealInFinder('/Users/demo/Downloads/Xcode_16.4.xip');
    expect(revealed).toEqual(['/x', '/Users/demo/Downloads/Xcode_16.4.xip']);
  });
});

describe('resetIpcMock', () => {
  it('returns to idle, forgets revealed paths and stops a running scan', async () => {
    await revealInFinder('/x');
    const { progress, done } = await subscribe();
    const spy = vi.fn();
    void done.then(spy);
    await scanStart();
    resetIpcMock();
    expect((await scanStatus()).state).toBe('idle');
    expect(revealed).toEqual([]);
    await expect(treeNode()).rejects.toBe('no scan result');
    await tick();
    expect(progress).toEqual([]);
    expect(spy).not.toHaveBeenCalled();
  });

  it('runs on every install', async () => {
    await revealInFinder('/x');
    installIpcMock();
    expect(revealed).toEqual([]);
  });

  it('puts the fixture back, so an install hands out a tree nothing has deleted from', async () => {
    const { done } = await subscribe();
    await scanStart();
    await done;
    const movies = fixtureNode('Movies');
    await actionRun([movies.path], 'trash');
    expect((await activityLog()).entries).toHaveLength(1);

    resetIpcMock();

    expect(fixtureNode('Movies')).toBe(movies);
    expect(await activityLog()).toEqual({ entries: [], damaged: 0 });
    expect(fixtureNodes[0].size).toBe(fixtureStatusDone().bytes);
  });
});

describe('the deletion commands', () => {
  it('block every path before a scan instead of failing, unlike tree_node', async () => {
    await expect(treeNode()).rejects.toBe('no scan result');
    const movies = fixtureNode('Movies');
    const preview = await actionPreview([movies.path], 'trash');
    // No root and no tree: the guards refuse the path, and the plan behind it is empty —
    // `with_result` has nothing to read the kind and the size from.
    expect(preview.entries).toEqual([
      {
        path: movies.path,
        kind: 'other',
        size: 0,
        status: { state: 'blocked', reason: 'outsideRoots' },
      },
    ]);
    expect(preview.totalBytes).toBe(0);
    const idle = await scanStatus();
    const batch = await actionRun([movies.path], 'trash');
    expect(batch.outcome.entries[0]).toEqual({
      path: movies.path,
      kind: 'other',
      result: { result: 'skipped', reason: 'outsideRoots' },
    });
    expect(batch.outcome.freedBytes).toBe(0);
    // A batch that deleted nothing has no totals to bring up to date, and an idle window
    // has none to show: the counters of a scan that never ran stay at zero.
    expect(await scanStatus()).toEqual(idle);
    expect(idle.bytes).toBe(0);
    expect(await activityLog()).toEqual({
      entries: [expect.objectContaining({ result: 'skipped', detail: 'outsideRoots' })],
      damaged: 0,
    });
  });

  it('guard a batch that arrives while a scan runs, without sizes for it', async () => {
    // The browser pace keeps the scan in flight for the length of the test; `start` clears
    // the tree and keeps the root, and the mock answers from that state as the manager does.
    setMockScanDelay(50);
    const movies = fixtureNode('Movies');
    expect((await scanStart()).state).toBe('running');
    await expect(treeNode()).rejects.toBe('no scan result');

    const preview = await actionPreview([movies.path, `${FIXTURE_ROOT}/Library`], 'trash');
    expect(preview.entries).toEqual([
      { path: movies.path, kind: 'dir', size: 0, status: { state: 'ready' } },
      {
        path: `${FIXTURE_ROOT}/Library`,
        kind: 'other',
        size: 0,
        status: { state: 'blocked', reason: 'denylisted' },
      },
    ]);
    expect(preview.totalBytes).toBe(0);

    const batch = await actionRun([movies.path], 'trash');
    expect(batch.outcome.entries[0].result).toEqual({ result: 'removed', bytes: 0 });
    expect(batch.outcome.freedBytes).toBe(0);
    // Nothing to bring up to date either: the totals of a running scan are the walker's.
    expect((await scanStatus()).state).toBe('running');
  });

  it('guard a batch after a scan that failed, which leaves a root and no tree', async () => {
    setMockScanFailure('the scan panicked: the volume went away');
    const { done } = await subscribe();
    await scanStart();
    const final = await done;
    expect(final.state).toBe('failed');
    expect(final.error).toBe('the scan panicked: the volume went away');
    await expect(treeNode()).rejects.toBe('no scan result');

    const movies = fixtureNode('Movies');
    const preview = await actionPreview([movies.path], 'trash');
    // The root the user chose still decides what may be deleted; the tree that would have
    // carried the sizes is the thing that did not arrive.
    expect(preview.entries).toEqual([
      { path: movies.path, kind: 'dir', size: 0, status: { state: 'ready' } },
    ]);
    expect(preview.totalBytes).toBe(0);
  });

  it('carry the tree of a cancelled scan, which keeps its partial walk', async () => {
    const { done } = await subscribe();
    await scanStart();
    await scanCancel();
    expect((await done).state).toBe('cancelled');
    const movies = fixtureNode('Movies');
    // `Inner::complete` installs a cancelled scan's tree like any other, so the plan behind
    // a batch has its sizes — the one classification of the three that is not obvious.
    expect((await actionPreview([movies.path], 'trash')).totalBytes).toBe(movies.size);
  });

  it('reject arguments that are not a list of paths and a mode', async () => {
    await expect(rawInvoke('action_preview', { paths: '/Users/demo/Movies' })).rejects.toMatch(
      /paths/,
    );
    await expect(rawInvoke('action_run', { paths: [1, 2], mode: 'trash' })).rejects.toMatch(
      /paths/,
    );
    await expect(rawInvoke('action_run', { paths: [], mode: 'bin' })).rejects.toMatch(/mode/);
    await expect(rawInvoke('action_preview', { paths: [] })).rejects.toMatch(/mode/);
  });

  describe('after a scan', () => {
    beforeEach(async () => {
      const { done } = await subscribe();
      await scanStart();
      await done;
    });

    it('preview a selection against the scan root', async () => {
      const film = fixtureNode('Movies/family-2025.mov');
      const preview = await actionPreview([film.path, fixtureNode('Library').path], 'permanent');
      expect(preview.mode).toBe('permanent');
      expect(preview.entries.map((entry) => entry.status)).toEqual([
        { state: 'ready' },
        { state: 'blocked', reason: 'denylisted' },
      ]);
      expect(preview.totalBytes).toBe(film.size);
    });

    it('run a batch: the rows leave the tree and the scanned totals shrink', async () => {
      const before = await scanStatus();
      const movies = fixtureNode('Movies');
      const [size, files] = [movies.size, movies.fileCount];
      const batch = await actionRun([movies.path], 'trash');
      expect(batch).toEqual({
        outcome: {
          entries: [{ path: movies.path, kind: 'dir', result: { result: 'removed', bytes: size } }],
          freedBytes: size,
          at: expect.any(String),
          mode: 'trash',
        },
        recorded: true,
        treeStale: false,
      });
      const root = await treeNode();
      expect(root.children.map((child) => child.name)).not.toContain('Movies');
      expect(root.size).toBe(before.bytes - size);
      const after = await scanStatus();
      expect(after.bytes).toBe(before.bytes - size);
      expect(after.files).toBe(before.files - files);
      expect(after.dirs).toBe(before.dirs - 1);
      // Like `patched_stats`, a patch leaves the read errors of the scan alone.
      expect(after.errors).toBe(before.errors);
      expect(after.state).toBe('done');
      expect(after.durationMs).toBe(before.durationMs);
    });

    it('leave the scanned totals alone when a batch deletes nothing', async () => {
      const before = await scanStatus();
      await actionRun([fixtureNode('Library').path], 'trash');
      expect(await scanStatus()).toEqual(before);
    });

    it('keep the read errors of a deleted unreadable directory', async () => {
      const before = await scanStatus();
      await actionRun([fixtureNode('.Trash').path], 'trash');
      const after = await scanStatus();
      expect(after.errors).toBe(before.errors);
      expect(after.dirs).toBe(before.dirs - 1);
      expect(after.bytes).toBe(before.bytes);
    });

    it('read the batch back through activity_log, newest first', async () => {
      const report = fixtureNode('Downloads/q3-report.pdf');
      const size = report.size;
      await actionRun([report.path, `${FIXTURE_ROOT}/nope`], 'permanent');
      const log = await activityLog();
      expect(log.damaged).toBe(0);
      expect(log.entries).toEqual([
        expect.objectContaining({ path: `${FIXTURE_ROOT}/nope`, result: 'skipped' }),
        expect.objectContaining({
          path: report.path,
          kind: 'file',
          mode: 'permanent',
          result: 'removed',
          detail: null,
          bytes: size,
        }),
      ]);
    });

    it('return the last hundred entries to a caller that asks for no limit', async () => {
      const paths = Array.from({ length: 101 }, (_, i) => `${FIXTURE_ROOT}/nope-${i}`);
      await actionRun(paths, 'trash');
      expect((await activityLog()).entries).toHaveLength(100);
      expect((await activityLog(101)).entries).toHaveLength(101);
      expect((await activityLog(2)).entries.map((entry) => entry.path)).toEqual([
        `${FIXTURE_ROOT}/nope-100`,
        `${FIXTURE_ROOT}/nope-99`,
      ]);
    });

    it('start every test with the fixture whole again', async () => {
      expect((await treeNode()).children.map((child) => child.name)).toContain('Movies');
      expect(await activityLog()).toEqual({ entries: [], damaged: 0 });
    });
  });
});
