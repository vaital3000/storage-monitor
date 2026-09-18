import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
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
import { FIXTURE_ROOT, fixtureDisk, fixtureGrowers, fixtureNodes } from './fixtures';
import { installIpcMock, mockScanDelayMs, resetIpcMock, revealed, setMockScanDelay } from './ipc';

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

/** The mocked `invoke`, for commands the typed layer does not expose. */
function rawInvoke(cmd: string): Promise<unknown> {
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: { invoke(cmd: string, args?: unknown): Promise<unknown> };
    }
  ).__TAURI_INTERNALS__;
  return internals.invoke(cmd, {});
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
});
