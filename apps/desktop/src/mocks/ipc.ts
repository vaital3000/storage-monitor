// Fake backend for unit tests, e2e and `just dev-web`: every command of
// `src-tauri/src/commands.rs` answered from the fixture, with a simulated scan that
// emits the same events as the real scan manager.

import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';
import {
  SCAN_DONE_EVENT,
  SCAN_PROGRESS_EVENT,
  type AppInfo,
  type BatchResult,
  type CleanupRequest,
  type DeletionMode,
  type ScanStatus,
} from '../lib/ipc';
import {
  FIXTURE_ROOT,
  fixtureDisk,
  fixtureGrowers,
  fixtureNodeView,
  fixtureNodes,
  fixtureStatusDone,
} from './fixtures';
import { type HeldScan, mockActionPreview, mockActionRun, resetMockActions } from './actions';
import { mockActivityTail } from './actionLog';
import {
  mockCleanupPreview,
  mockCleanupRun,
  resetMockCleanup,
  setMockCleanupFailure,
} from './cleanup';
import {
  mockCleanupItems,
  mockModulesList,
  mockModulesRefresh,
  resetMockModules,
  setMockDiscoveryDelay,
  setMockModuleFailure,
  setMockModuleUnavailable,
  setMockModules,
} from './modules';

export const MOCK_APP_INFO: AppInfo = { name: 'Storage Monitor', version: '0.0.0-mock' };

/** Paths the UI asked to reveal in Finder, oldest first. */
export const revealed: string[] = [];

/** Milliseconds between the simulated progress ticks; tests set it to 0. */
export let mockScanDelayMs = 150;

export function setMockScanDelay(ms: number): void {
  mockScanDelayMs = ms;
}

/**
 * Makes every simulated scan from now on end in `failed` with this message, until it is set
 * back to null. `Inner::fail` leaves a root and no result behind, which is the one state a
 * window can be in where the guards run and the plan has no sizes to carry — so this is how
 * a screen stages "a batch after a scan that did not finish".
 */
export function setMockScanFailure(message: string | null): void {
  scanFailure = message;
}

const PROGRESS_TICKS = 6;
const DEFAULT_CHILDREN_LIMIT = 500;
const DEFAULT_GROWERS_LIMIT = 10;
const DEFAULT_ACTIVITY_LIMIT = 100;
const DEFAULT_ITEMS_LIMIT = 2000;

/** Directories the simulated scan claims to be reading, one per tick, shallow to deep. */
const PROGRESS_PATHS: readonly string[] = (() => {
  const dirs = fixtureNodes.filter((n) => n.kind === 'dir' && n.error === null && n.id !== 0);
  return Array.from(
    { length: PROGRESS_TICKS },
    (_, i) => dirs[Math.floor((i * dirs.length) / PROGRESS_TICKS)].path,
  );
})();

const IDLE: ScanStatus = {
  state: 'idle',
  root: null,
  files: 0,
  dirs: 0,
  bytes: 0,
  errors: 0,
  currentPath: '',
  durationMs: 0,
  error: null,
  hasPrevious: false,
  previousTakenAt: null,
};

let status: ScanStatus = IDLE;
/** A tree is available: the last scan finished or was cancelled. */
let hasResult = false;
/** What the next scan fails with, or null while the machine is behaving. */
let scanFailure: string | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

type IpcHandler = Parameters<typeof mockIPC>[0];
type IpcArgs = Parameters<IpcHandler>[1];

function argument(args: IpcArgs, name: string): unknown {
  if (
    args === undefined ||
    Array.isArray(args) ||
    args instanceof ArrayBuffer ||
    ArrayBuffer.isView(args)
  ) {
    return undefined;
  }
  return args[name];
}

function stringArgument(args: IpcArgs, name: string): string | undefined {
  const value = argument(args, name);
  return typeof value === 'string' ? value : undefined;
}

function numberArgument(args: IpcArgs, name: string): number | undefined {
  const value = argument(args, name);
  return typeof value === 'number' ? value : undefined;
}

/**
 * The two arguments of both action commands, refused the way Tauri refuses a body it cannot
 * deserialize into `Vec<String>` and `Mode`: with an error, never with a guess.
 */
function batchArguments(args: IpcArgs, cmd: string): { paths: string[]; mode: DeletionMode } {
  const paths = argument(args, 'paths');
  if (!Array.isArray(paths) || paths.some((path) => typeof path !== 'string')) {
    commandError(`invalid args \`paths\` for command \`${cmd}\`: expected a list of paths`);
  }
  const mode = argument(args, 'mode');
  if (mode !== 'trash' && mode !== 'permanent') {
    commandError(`invalid args \`mode\` for command \`${cmd}\`: expected trash or permanent`);
  }
  return { paths: paths as string[], mode };
}

/**
 * The requests of both cleanup commands, refused the way Tauri refuses a body it cannot
 * deserialize into `Vec<Request>`: every request an object with a string `item`, a string
 * `action` and, when present, a list of strings `options` — `#[serde(default)]` in Rust.
 */
function cleanupRequests(args: IpcArgs, cmd: string): CleanupRequest[] {
  const requests = argument(args, 'requests');
  const valid =
    Array.isArray(requests) &&
    requests.every((request: unknown) => {
      if (typeof request !== 'object' || request === null) return false;
      const { item, action, options } = request as Record<string, unknown>;
      return (
        typeof item === 'string' &&
        typeof action === 'string' &&
        (options === undefined ||
          (Array.isArray(options) && options.every((option) => typeof option === 'string')))
      );
    });
  if (!valid) {
    commandError(`invalid args \`requests\` for command \`${cmd}\`: expected a list of requests`);
  }
  return (requests as Array<{ item: string; action: string; options?: string[] }>).map(
    ({ item, action, options }) => ({ item, action, options: options ?? [] }),
  );
}

function modeArgument(args: IpcArgs, cmd: string): DeletionMode {
  const mode = argument(args, 'mode');
  if (mode !== 'trash' && mode !== 'permanent') {
    commandError(`invalid args \`mode\` for command \`${cmd}\`: expected trash or permanent`);
  }
  return mode;
}

/** A command's `Err(String)`: Tauri rejects with the string itself, not with an `Error`. */
function commandError(message: string): never {
  throw message;
}

function clearTimer(): void {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
}

function after(callback: () => void): void {
  timer = setTimeout(() => {
    timer = null;
    callback();
  }, mockScanDelayMs);
}

type RawInvoke = (cmd: string, args?: unknown, options?: unknown) => Promise<unknown>;

/** The IPC bridge installed by `mockIPC`, absent once a test cleared the mocks. */
function tauriInternals(): { invoke?: RawInvoke } | undefined {
  return (window as unknown as { __TAURI_INTERNALS__?: { invoke?: RawInvoke } })
    .__TAURI_INTERNALS__;
}

/**
 * Emits a copy of the status. A tick that fires after a test cleared the mock has nobody
 * to talk to; a listener that throws is not swallowed, so the test that owns it fails.
 */
function publish(event: string): void {
  if (typeof tauriInternals()?.invoke !== 'function') {
    return;
  }
  void emit(event, { ...status });
}

/**
 * The end of a scan. `holdsTree` is what `Inner::complete` does and `Inner::fail` does not:
 * a failed scan keeps its root and leaves no tree at all.
 */
function finish(final: ScanStatus, holdsTree = true): void {
  status = final;
  hasResult = holdsTree;
  publish(SCAN_DONE_EVENT);
}

function advance(tick: number): void {
  if (status.state !== 'running') {
    return;
  }
  const done = fixtureStatusDone();
  if (tick > PROGRESS_TICKS) {
    if (scanFailure !== null) {
      // The counters stay the walker's own, as they do in the app: with no result to read,
      // `ScanManager::status` answers from the live progress of the walk that stopped.
      finish(
        {
          ...status,
          state: 'failed',
          currentPath: '',
          error: scanFailure,
          hasPrevious: false,
          previousTakenAt: null,
        },
        false,
      );
      return;
    }
    finish({ ...done, root: status.root });
    return;
  }
  const share = tick / (PROGRESS_TICKS + 1);
  status = {
    ...status,
    files: Math.floor(done.files * share),
    dirs: Math.floor(done.dirs * share),
    bytes: Math.floor(done.bytes * share),
    errors: Math.floor(done.errors * share),
    currentPath: PROGRESS_PATHS[tick - 1],
    durationMs: Math.floor(done.durationMs * share),
  };
  publish(SCAN_PROGRESS_EVENT);
  after(() => advance(tick + 1));
}

function scanStart(root: string): ScanStatus {
  if (status.state === 'running') {
    commandError('a scan is already running');
  }
  clearTimer();
  hasResult = false;
  status = { ...IDLE, state: 'running', root, currentPath: root };
  after(() => advance(1));
  return { ...status };
}

/** Like the real manager, the reply is still `running`; `scan:done` follows shortly. */
function scanCancel(): ScanStatus {
  if (status.state === 'running') {
    clearTimer();
    after(() => {
      finish({
        ...status,
        state: 'cancelled',
        currentPath: '',
        hasPrevious: false,
        previousTakenAt: null,
      });
    });
  }
  return { ...status };
}

/**
 * What the window holds, as the guards and the plan see it: the root outlives the tree, so a
 * batch that arrives while a scan runs is guarded by the root and planned without sizes.
 */
function heldScan(): HeldScan {
  if (status.root === null) {
    return { held: 'nothing' };
  }
  return hasResult ? { held: 'tree', root: status.root } : { held: 'root', root: status.root };
}

/**
 * A batch, with the totals of the scan brought up to date afterwards.
 *
 * `patched_stats` does the same in the app: the files, the folders and the bytes come from
 * the tree the splice installed, while the read errors stay the ones the scan met. Without
 * it the header would go on claiming the bytes of rows that are gone.
 */
function runBatch(paths: string[], mode: DeletionMode): BatchResult {
  const batch = mockActionRun(paths, mode, heldScan());
  if (hasResult) {
    const patched = fixtureStatusDone();
    status = { ...status, files: patched.files, dirs: patched.dirs, bytes: patched.bytes };
  }
  return batch;
}

const handle: IpcHandler = (cmd, args) => {
  switch (cmd) {
    case 'get_app_info':
      return MOCK_APP_INFO;
    case 'default_root':
      return FIXTURE_ROOT;
    case 'scan_start':
      return scanStart(stringArgument(args, 'root') ?? FIXTURE_ROOT);
    case 'scan_status':
      return { ...status };
    case 'scan_cancel':
      return scanCancel();
    case 'tree_node': {
      if (!hasResult) {
        commandError('no scan result');
      }
      const id = numberArgument(args, 'id') ?? 0;
      const limit = numberArgument(args, 'limit') ?? DEFAULT_CHILDREN_LIMIT;
      try {
        return fixtureNodeView(id, limit);
      } catch (error) {
        return commandError(error instanceof Error ? error.message : String(error));
      }
    }
    case 'disk_usage':
      return {
        ...fixtureDisk(),
        path: stringArgument(args, 'path') ?? status.root ?? FIXTURE_ROOT,
      };
    case 'top_growers':
      return status.hasPrevious
        ? fixtureGrowers().slice(0, numberArgument(args, 'limit') ?? DEFAULT_GROWERS_LIMIT)
        : [];
    case 'action_preview': {
      const { paths, mode } = batchArguments(args, cmd);
      return mockActionPreview(paths, mode, heldScan());
    }
    case 'action_run': {
      const { paths, mode } = batchArguments(args, cmd);
      return runBatch(paths, mode);
    }
    case 'activity_log':
      return mockActivityTail(numberArgument(args, 'limit') ?? DEFAULT_ACTIVITY_LIMIT);
    case 'modules_list':
      return mockModulesList();
    case 'modules_refresh': {
      const ids = argument(args, 'ids');
      if (ids !== undefined && ids !== null && !Array.isArray(ids)) {
        commandError('invalid args `ids` for command `modules_refresh`: expected a list of ids');
      }
      return mockModulesRefresh(Array.isArray(ids) ? ids.map(String) : []);
    }
    case 'cleanup_items':
      return mockCleanupItems(numberArgument(args, 'limit') ?? DEFAULT_ITEMS_LIMIT);
    case 'cleanup_preview':
      return mockCleanupPreview(cleanupRequests(args, cmd));
    case 'cleanup_run':
      return mockCleanupRun(cleanupRequests(args, cmd), modeArgument(args, cmd));
    case 'plugin:opener|reveal_item_in_dir': {
      // `revealItemInDir(path)` sends `{ paths: [path] }`.
      const paths = argument(args, 'paths');
      for (const path of Array.isArray(paths) ? paths : [paths]) {
        revealed.push(String(path));
      }
      return null;
    }
    default:
      throw new Error(`Unmocked IPC command: ${cmd}`);
  }
};

/**
 * Back to idle: no scan, no tree, no revealed paths, no pending ticks, and the fixture whole
 * again with an empty action log. Keeps the delay.
 */
export function resetIpcMock(): void {
  clearTimer();
  status = IDLE;
  hasResult = false;
  scanFailure = null;
  revealed.length = 0;
  resetMockActions();
  resetMockModules();
  resetMockCleanup();
}

declare global {
  interface Window {
    /** Test hooks of the fake backend; present in mock mode and in unit tests only. */
    __STORAGE_MONITOR_MOCK__?: {
      revealed: string[];
      setMockScanDelay: (ms: number) => void;
      setMockScanFailure: (message: string | null) => void;
      setMockModules: (present: boolean) => void;
      setMockModuleFailure: (id: string, message: string | null) => void;
      setMockModuleUnavailable: (id: string, reason: string | null) => void;
      setMockCleanupFailure: (itemId: string, message: string | null) => void;
      setMockDiscoveryDelay: (ms: number) => void;
    };
  }
}

/**
 * Works around a mismatch inside `@tauri-apps/api` 2.11: `event.js` unlistens with
 * `invoke('plugin:event|unlisten', { event, eventId })`, while the remover in `mocks.js`
 * looks the listener up under `args.id`. Left alone, listeners are never dropped and every
 * later emit warns about a missing callback. Passing `id` next to `eventId` lets the mock
 * find it; once upstream reads `eventId`, the extra field is ignored and the shim is
 * harmless, so it can stay until the dependency is bumped past the fix.
 */
function fixUnlisten(): void {
  const internals = tauriInternals();
  const mocked = internals?.invoke;
  if (internals === undefined || mocked === undefined) {
    return;
  }
  internals.invoke = (cmd, args, options) => {
    if (cmd === 'plugin:event|unlisten' && typeof args === 'object' && args !== null) {
      return mocked(cmd, { ...args, id: (args as { eventId?: unknown }).eventId }, options);
    }
    return mocked(cmd, args, options);
  };
}

/**
 * Installs fake handlers for every backend command and routes `emit` to `listen`.
 * Used by unit tests, e2e and browser dev; resets the mock state first.
 */
export function installIpcMock(): void {
  resetIpcMock();
  mockIPC(handle, { shouldMockEvents: true });
  fixUnlisten();
  window.__STORAGE_MONITOR_MOCK__ = {
    revealed,
    setMockScanDelay,
    setMockScanFailure,
    setMockModules,
    setMockModuleFailure,
    setMockModuleUnavailable,
    setMockCleanupFailure,
    setMockDiscoveryDelay,
  };
}
