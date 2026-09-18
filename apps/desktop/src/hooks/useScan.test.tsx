import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SCAN_DONE_EVENT, SCAN_PROGRESS_EVENT, type ScanStatus } from '../lib/ipc';
import { FIXTURE_ROOT, fixtureStatusDone } from '../mocks/fixtures';
import { installIpcMock, setMockScanDelay } from '../mocks/ipc';
import { holdReply, recordCommands, replyOnce } from '../test/invoke';
import { IDLE_STATUS, useScan } from './useScan';

beforeEach(() => {
  installIpcMock();
});

/** Renders the hook and records every status it exposed, in render order. */
function renderScan() {
  const seen: ScanStatus[] = [];
  const rendered = renderHook(() => {
    const scan = useScan();
    seen.push(scan.status);
    return scan;
  });
  return { ...rendered, seen };
}

async function ready(result: { current: { ready: boolean } }) {
  await waitFor(() => expect(result.current.ready).toBe(true));
}

/** One macrotask: enough for the mock's (asynchronous) listen and invoke to settle. */
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('useScan', () => {
  it('starts idle with the status reported by the backend', async () => {
    const { result } = renderScan();
    expect(result.current.status.state).toBe('idle');
    expect(result.current.ready).toBe(false);
    await ready(result);
    expect(result.current.status.state).toBe('idle');
    expect(result.current.hasResult).toBe(false);
    expect(result.current.cancelling).toBe(false);
  });

  it('gives every mount a generation of its own', async () => {
    const first = renderScan();
    await ready(first.result);
    first.unmount();
    const second = renderScan();
    await ready(second.result);
    expect(second.result.current.generation).not.toBe(first.result.current.generation);
  });

  it('runs a scan to completion through the events and bumps the generation', async () => {
    const { result, seen } = renderScan();
    await ready(result);
    const generation = result.current.generation;

    await act(() => result.current.start());
    expect(result.current.status.state).toBe('running');
    expect(result.current.status.root).toBe(FIXTURE_ROOT);
    expect(result.current.hasResult).toBe(false);
    expect(result.current.generation).toBe(generation);

    await waitFor(() => expect(result.current.status.state).toBe('done'));
    expect(result.current.generation).toBe(generation + 1);
    expect(result.current.hasResult).toBe(true);
    expect(result.current.status).toEqual(fixtureStatusDone());

    const progress = seen.filter((s) => s.state === 'running' && s.files > 0);
    expect(progress.length).toBeGreaterThan(0);
    expect(progress.some((s) => s.currentPath.startsWith(FIXTURE_ROOT))).toBe(true);
  });

  it('bumps the generation on every completed scan', async () => {
    const { result } = renderScan();
    await ready(result);
    const generation = result.current.generation;
    await act(() => result.current.start());
    await waitFor(() => expect(result.current.generation).toBe(generation + 1));
    await act(() => result.current.start());
    expect(result.current.status.state).toBe('running');
    await waitFor(() => expect(result.current.generation).toBe(generation + 2));
    expect(result.current.status.state).toBe('done');
  });

  it('cancels: the reply keeps it running until scan:done says cancelled', async () => {
    const { result } = renderScan();
    await ready(result);
    const generation = result.current.generation;
    // The mock's `scan:done` follows the cancel after one delay; at 0 ms it can beat the
    // flush of `act` and the assertions below would see the final state.
    setMockScanDelay(50);
    await act(() => result.current.start());
    await act(() => result.current.cancel());
    expect(result.current.cancelling).toBe(true);
    expect(result.current.status.state).toBe('running');

    await waitFor(() => expect(result.current.status.state).toBe('cancelled'));
    expect(result.current.cancelling).toBe(false);
    expect(result.current.hasResult).toBe(true);
    expect(result.current.generation).toBe(generation + 1);
    expect(result.current.status.hasPrevious).toBe(false);
  });

  it('ignores a progress event once the scan is finished', async () => {
    const { result } = renderScan();
    await ready(result);
    await act(() => result.current.start());
    await waitFor(() => expect(result.current.status.state).toBe('done'));
    const generation = result.current.generation;

    const stale: ScanStatus = { ...fixtureStatusDone(), state: 'running', files: 1, bytes: 1 };
    await act(() => emit(SCAN_PROGRESS_EVENT, stale));
    expect(result.current.status.state).toBe('done');
    expect(result.current.status).toEqual(fixtureStatusDone());
    expect(result.current.generation).toBe(generation);
  });

  it('applies progress while running and takes scan:done from any state', async () => {
    const { result } = renderScan();
    await ready(result);
    setMockScanDelay(10_000);
    const generation = result.current.generation;
    await act(() => result.current.start());

    const running: ScanStatus = { ...result.current.status, files: 42, currentPath: '/x' };
    await act(() => emit(SCAN_PROGRESS_EVENT, running));
    expect(result.current.status.files).toBe(42);
    expect(result.current.status.currentPath).toBe('/x');

    const failed: ScanStatus = { ...running, state: 'failed', error: 'disk on fire' };
    await act(() => emit(SCAN_DONE_EVENT, failed));
    expect(result.current.status.state).toBe('failed');
    expect(result.current.status.error).toBe('disk on fire');
    expect(result.current.hasResult).toBe(false);
    expect(result.current.generation).toBe(generation + 1);
  });

  it('is ready as soon as a progress event arrives, before scan_status answers', async () => {
    const release = holdReply('scan_status');
    const { result } = renderScan();
    await act(tick);
    expect(result.current.ready).toBe(false);

    const running: ScanStatus = { ...IDLE_STATUS, state: 'running', root: FIXTURE_ROOT, files: 7 };
    await act(() => emit(SCAN_PROGRESS_EVENT, running));
    expect(result.current.ready).toBe(true);
    expect(result.current.status).toEqual(running);

    release();
    await act(tick);
    // The late idle reply does not overwrite the fresher progress.
    expect(result.current.status).toEqual(running);
  });

  it('keeps a progress event that arrived before the start reply', async () => {
    const { result } = renderScan();
    await ready(result);
    setMockScanDelay(10_000);
    const release = holdReply('scan_start');
    let starting!: Promise<void>;
    act(() => {
      starting = result.current.start();
    });

    const progress: ScanStatus = {
      ...IDLE_STATUS,
      state: 'running',
      root: FIXTURE_ROOT,
      files: 3,
      currentPath: '/x',
    };
    await act(() => emit(SCAN_PROGRESS_EVENT, progress));
    expect(result.current.status).toEqual(progress);

    release();
    await act(() => starting);
    expect(result.current.status).toEqual(progress);
  });

  it('ignores cancel while no scan is running', async () => {
    const { result } = renderScan();
    await ready(result);
    const commands = recordCommands();
    await act(() => result.current.cancel());
    expect(result.current.cancelling).toBe(false);
    expect(commands).not.toContain('scan_cancel');
  });

  it('drops the cancelling flag when the reply says the scan already ended', async () => {
    const { result } = renderScan();
    await ready(result);
    setMockScanDelay(10_000);
    await act(() => result.current.start());
    replyOnce('scan_cancel', fixtureStatusDone);
    await act(() => result.current.cancel());
    expect(result.current.cancelling).toBe(false);
    expect(result.current.status.state).toBe('running');
  });

  it('keeps running when a second start is refused', async () => {
    const { result } = renderScan();
    await ready(result);
    setMockScanDelay(10_000);
    await act(() => result.current.start());
    await act(() => result.current.start());
    expect(result.current.status.state).toBe('running');
    expect(result.current.status.error).toBeNull();
  });

  it('reports a backend that cannot answer as a failed scan', async () => {
    mockIPC(() => {
      throw new Error('boom');
    });
    const { result } = renderScan();
    await ready(result);
    expect(result.current.status.state).toBe('failed');
    expect(result.current.status.error).toBe('Error: boom');

    await act(() => result.current.start());
    expect(result.current.status.state).toBe('failed');
    expect(result.current.status.error).toBe('Error: boom');
  });

  it('stops listening after unmount, without leaking a listener', async () => {
    const warn = vi.spyOn(console, 'warn');
    try {
      const { result, seen, unmount } = renderScan();
      await ready(result);
      setMockScanDelay(5);
      await act(() => result.current.start());
      unmount();
      const renders = seen.length;
      await new Promise((resolve) => setTimeout(resolve, 60));
      expect(seen).toHaveLength(renders);
      expect(warn).not.toHaveBeenCalled();
    } finally {
      warn.mockRestore();
    }
  });
});
