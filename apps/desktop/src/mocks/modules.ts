// The mock's half of `src-tauri/src/module_manager.rs` and of the demo module
// (`crates/modules/demo`): what the modules of a debug build find, and how a discovery
// reports its states through `modules:state`.
//
// The demo's items are the ones its Rust seed produces — the same names, sizes on an APFS
// volume, ages, reasons, facts and options — so that a screen drawn over the mock is the
// screen the app draws. The mock's "now" is the fixture's scan time rather than the clock, so
// that a screenshot does not drift with the day it was taken on.

import { emit } from '@tauri-apps/api/event';
import { countLabel } from '../lib/format';
import {
  MODULES_STATE_EVENT,
  type ActionOption,
  type Item,
  type ModuleStatus,
  type ModuleView,
  type Verdict,
} from '../lib/ipc';
import { FIXTURE_ROOT } from './fixtures';

/** The demo's sandbox: the app's data dir inside the fixture's home, as the app keeps it. */
export const DEMO_SANDBOX = `${FIXTURE_ROOT}/Library/Application Support/storage-monitor/demo`;

/** Where the housekeeping step of a folder batch writes. */
export const DEMO_LAST_CLEANUP = `${DEMO_SANDBOX}/.last-cleanup`;

const DAY_MS = 86_400_000;

/** The instant the mock's sandbox was seeded, and the "now" its verdicts are read at. */
const SEEDED_AT = Date.UTC(2026, 8, 18, 9, 30);

/** `STALE_AFTER_DAYS` of the demo module. */
const STALE_AFTER_DAYS = 30;

/** One entry of the demo's seed, with what the walk of the Rust module reports about it. */
interface Seeded {
  name: string;
  kind: 'folder' | 'object';
  /** Allocated bytes on an APFS volume: what the Rust module measures. */
  bytes: number;
  /** Files in a folder (its `KEEP` mark included); 1 for an object. */
  files: number;
  /** An object's logical size. */
  logical: number;
  ageDays: number;
  keep: boolean;
}

/** `FOLDERS` and `OBJECTS` of `seed.rs`, largest first, as discovery sorts them. */
const SEED: readonly Seeded[] = [
  {
    name: 'fresh.object',
    kind: 'object',
    bytes: 3_002_368,
    files: 1,
    logical: 3_000_000,
    ageDays: 1,
    keep: false,
  },
  {
    name: 'build-cache',
    kind: 'folder',
    bytes: 2_015_232,
    files: 4,
    logical: 0,
    ageDays: 45,
    keep: false,
  },
  {
    name: 'logs',
    kind: 'folder',
    bytes: 1_204_224,
    files: 2,
    logical: 0,
    ageDays: 3,
    keep: false,
  },
  {
    name: 'old.object',
    kind: 'object',
    bytes: 1_003_520,
    files: 1,
    logical: 1_000_000,
    ageDays: 60,
    keep: false,
  },
  {
    name: 'keepsake',
    kind: 'folder',
    bytes: 507_904,
    files: 2,
    logical: 0,
    ageDays: 200,
    keep: true,
  },
];

const FORCE: ActionOption = {
  id: 'force',
  label: 'Delete it although it is marked keep',
  default: false,
  force: true,
};

/** "1 day", "3 days". */
function days(n: number): string {
  return countLabel(n, 'day');
}

/** `verdict` of the demo module: a mark wins over any age; then 30 days untouched is Safe. */
function verdict(ageDays: number, keep: boolean): Verdict {
  if (keep) {
    return { level: 'keep', reasons: [{ code: 'marked-keep', text: 'It holds a KEEP file' }] };
  }
  if (ageDays >= STALE_AFTER_DAYS) {
    return {
      level: 'safe',
      reasons: [{ code: 'stale', text: `Not modified for ${days(ageDays)}` }],
    };
  }
  const text = ageDays < 1 ? 'Modified today' : `Modified ${days(ageDays)} ago`;
  return { level: 'review', reasons: [{ code: 'recent', text }] };
}

/** What the Rust module's discovery reports for one entry of its sandbox. */
function demoItem(seed: Seeded): Item {
  const path = `${DEMO_SANDBOX}/${seed.name}`;
  const modified = new Date(SEEDED_AT - seed.ageDays * DAY_MS).toISOString();
  const common = {
    id: `demo:${seed.name}`,
    module: 'demo',
    title: seed.name,
    path,
    lastUsed: modified,
    verdict: verdict(seed.ageDays, seed.keep),
  };
  if (seed.kind === 'folder') {
    return {
      ...common,
      kind: 'folder',
      subtitle: countLabel(seed.files, 'file'),
      size: { bytes: seed.bytes, estimated: false },
      facts: [
        { key: 'files', label: 'Files', value: { type: 'count', value: seed.files } },
        { key: 'modified', label: 'Modified', value: { type: 'date', value: modified } },
        { key: 'keep', label: 'Marked keep', value: { type: 'flag', value: seed.keep } },
      ],
      actions: [
        {
          id: 'delete',
          label: 'Delete folder',
          estimatedFree: seed.bytes,
          options: seed.keep ? [FORCE] : [],
        },
      ],
    };
  }
  return {
    ...common,
    kind: 'object',
    subtitle: 'Stored object',
    size: { bytes: seed.bytes, estimated: true },
    facts: [
      { key: 'modified', label: 'Modified', value: { type: 'date', value: modified } },
      { key: 'logical', label: 'Logical size', value: { type: 'bytes', value: seed.logical } },
      { key: 'format', label: 'Format', value: { type: 'text', value: 'Demo object' } },
    ],
    actions: [{ id: 'remove', label: 'Remove object', estimatedFree: seed.bytes, options: [] }],
  };
}

/** One module of the mock's registry, and where it stands — a `Slot` of the manager. */
interface MockModule {
  id: string;
  name: string;
  description: string;
  status: ModuleStatus;
  reason: string | null;
  /** What the last discovery that worked found; kept through a failed refresh. */
  items: Item[] | null;
  discoveredAt: string | null;
  /** Bumped by every refresh, so an older discovery cannot land over a newer one. */
  generation: number;
}

const DEMO = {
  id: 'demo',
  name: 'Demo',
  description:
    "Sample files in the app's data folder, to try cleaning without touching anything of " +
    'yours. Debug builds only.',
};

/** Milliseconds a mock discovery takes; unit tests set it to 0. */
let discoveryDelayMs = 150;

/** The names of the demo's sandbox that are still there. */
let sandbox = new Set(SEED.map((seed) => seed.name));
/** Whether the registry holds the demo — false stands for a release build. */
let registered = true;
/** What the next discoveries of a module fail with, by module id. */
const failures = new Map<string, string>();
/** Why a module is unavailable, by module id. */
const unavailable = new Map<string, string>();

let modules: MockModule[] = [];
const timers = new Set<ReturnType<typeof setTimeout>>();

function freshModules(): MockModule[] {
  if (!registered) {
    return [];
  }
  return [
    {
      ...DEMO,
      status: 'idle',
      reason: null,
      items: null,
      discoveredAt: null,
      generation: 0,
    },
  ];
}

type RawInvoke = (cmd: string, args?: unknown, options?: unknown) => Promise<unknown>;

/** The IPC bridge installed by `mockIPC`, absent once a test cleared the mocks. */
function bridged(): boolean {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: RawInvoke } })
    .__TAURI_INTERNALS__;
  return typeof internals?.invoke === 'function';
}

/** Emits `event` unless the mock is already gone: a late timer has nobody to talk to. */
export function publish(event: string, payload: unknown): void {
  if (bridged()) {
    void emit(event, payload);
  }
}

/** `view` of the manager: the totals are over the items it holds. */
function viewOf(module: MockModule): ModuleView {
  const items = module.items ?? [];
  return {
    id: module.id,
    name: module.name,
    description: module.description,
    status: module.status,
    reason: module.reason,
    itemCount: items.length,
    totalBytes: items.reduce((sum, item) => sum + item.size.bytes, 0),
    safeBytes: items
      .filter((item) => item.verdict.level === 'safe')
      .reduce((sum, item) => sum + item.size.bytes, 0),
    discoveredAt: module.discoveredAt,
  };
}

/** `ModuleManager::list`. */
export function mockModulesList(): ModuleView[] {
  return modules.map(viewOf);
}

/** What the demo finds now: the seed, minus what was cleaned. */
function discoverDemo(): Item[] {
  return SEED.filter((seed) => sandbox.has(seed.name)).map(demoItem);
}

/**
 * `ModuleManager::refresh`: every named module (every module when none is named) is marked
 * `discovering` and says so, the answer is taken right then, and each discovery lands after
 * the mock's delay — unless a newer refresh of the same module started in the meantime.
 */
export function mockModulesRefresh(ids: readonly string[] = []): ModuleView[] {
  const targets = modules.filter((module) => ids.length === 0 || ids.includes(module.id));
  for (const module of targets) {
    module.generation += 1;
    module.status = 'discovering';
    module.reason = null;
    publish(MODULES_STATE_EVENT, viewOf(module));
  }
  const answer = mockModulesList();
  for (const module of targets) {
    const generation = module.generation;
    const timer = setTimeout(() => {
      timers.delete(timer);
      finish(module, generation);
    }, discoveryDelayMs);
    timers.add(timer);
  }
  return answer;
}

function finish(module: MockModule, generation: number): void {
  if (module.generation !== generation || !modules.includes(module)) {
    return;
  }
  const missing = unavailable.get(module.id);
  const failure = failures.get(module.id);
  if (missing !== undefined) {
    module.status = 'unavailable';
    module.reason = missing;
    module.items = null;
    module.discoveredAt = null;
  } else if (failure !== undefined) {
    // The items of the last discovery that worked stay: a failed refresh says so, it does
    // not empty the screen.
    module.status = 'failed';
    module.reason = failure;
  } else {
    module.status = 'ready';
    module.reason = null;
    module.items = discoverDemo();
    module.discoveredAt = new Date(SEEDED_AT).toISOString();
  }
  publish(MODULES_STATE_EVENT, viewOf(module));
}

/** Every held item with the module it came from: what the engine plans against. */
export function heldItems(): Map<string, Item> {
  const held = new Map<string, Item>();
  for (const module of modules) {
    for (const item of module.items ?? []) {
      held.set(item.id, item);
    }
  }
  return held;
}

/** `ModuleManager::items`: every held item, largest first, at most `limit`, and the count. */
export function mockCleanupItems(limit: number): { items: Item[]; total: number } {
  const items = [...heldItems().values()].sort(
    (a, b) => b.size.bytes - a.size.bytes || a.title.localeCompare(b.title),
  );
  return { items: items.slice(0, limit), total: items.length };
}

/**
 * What a batch removed: gone from the sandbox, so the next discovery does not find it, and
 * forgotten at once, so the screen does not show it while that discovery runs.
 */
export function forgetItems(itemIds: readonly string[]): void {
  for (const id of itemIds) {
    const [module, name] = id.split(':', 2);
    if (module === 'demo' && name !== undefined) {
      sandbox.delete(name);
    }
  }
  for (const module of modules) {
    if (module.items !== null) {
      module.items = module.items.filter((item) => !itemIds.includes(item.id));
    }
  }
}

/** The ids of the modules these items belong to, in the registry's order. */
export function modulesOf(itemIds: readonly string[]): string[] {
  return modules
    .map((module) => module.id)
    .filter((id) => itemIds.some((item) => item.split(':', 1)[0] === id));
}

/** Makes discoveries take `ms`; unit tests use 0, the browser the default 150. */
export function setMockDiscoveryDelay(ms: number): void {
  discoveryDelayMs = ms;
}

/**
 * Whether the registry holds the demo module. `false` is a release build of phase 2b, where
 * the Cleanup section says "soon". Takes effect at once: every module is forgotten.
 */
export function setMockModules(present: boolean): void {
  registered = present;
  modules = freshModules();
}

/** Makes every discovery of `id` fail with `message` from now on, until set back to null. */
export function setMockModuleFailure(id: string, message: string | null): void {
  if (message === null) failures.delete(id);
  else failures.set(id, message);
}

/** Makes `id` unavailable with `reason` from now on, until set back to null. */
export function setMockModuleUnavailable(id: string, reason: string | null): void {
  if (reason === null) unavailable.delete(id);
  else unavailable.set(id, reason);
}

/** Every module idle again, the sandbox whole, no failures, no pending discovery. */
export function resetMockModules(): void {
  for (const timer of timers) {
    clearTimeout(timer);
  }
  timers.clear();
  sandbox = new Set(SEED.map((seed) => seed.name));
  registered = true;
  failures.clear();
  unavailable.clear();
  modules = freshModules();
}

modules = freshModules();
