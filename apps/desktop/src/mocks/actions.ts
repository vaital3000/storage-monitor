// The mock's half of `crates/core/src/action/{guards,engine}.rs` and of the desktop's
// `actions.rs`: which paths a batch may delete, and what deleting them does to the tree in
// `fixtures.ts`. The record of it is in `actionLog.ts`.
//
// Every rule below mirrors one the backend enforces, because this is the only oracle the UI
// tests have — a mock that says yes to everything makes them prove nothing. The cases in
// `crates/core/tests/fixtures/guard-cases.json` are run against both sides, so a rule that
// changes in one and not the other reddens a test rather than drifting quietly.

import type {
  BatchResult,
  BlockReason,
  DeletionMode,
  EntryOutcome,
  Outcome,
  Preview,
  PreviewEntry,
} from '../lib/ipc';
import {
  FIXTURE_ROOT,
  type FixtureNode,
  fixtureEntry,
  removeSubtree,
  resetFixtureTree,
} from './fixtures';
import { appendToLog, resetActionLog } from './actionLog';

/**
 * The standard denylist of `Limits::with_home` for a machine whose home folder is `home`.
 *
 * Two axes, as in the Rust: this one is the machine's, and `limitsFor` narrows it by the
 * root of the scan. The mock's home is the fixture root — `default_root` returns it exactly
 * as the app returns `$HOME` — so the fixture's whole `Library` subtree is undeletable here,
 * as `~/Library` is in the app until a module declares a path inside it.
 */
export function deniedFor(home: string): string[] {
  return [
    '/',
    '/System',
    '/usr',
    '/bin',
    '/sbin',
    '/Library',
    '/Applications',
    '/opt',
    '/cores',
    '/Users',
    '/Volumes',
    '/etc',
    '/var',
    '/tmp',
    '/private',
    `${home}/Library`,
    home,
  ];
}

/** Where a batch may delete: `Limits`, built once for a scan and handed to every entry. */
export interface Limits {
  /** The scan root: rules 4 and 5 measure every entry against it. */
  root: string;
  /** The denied paths that do not contain the root. */
  denied: readonly string[];
}

/**
 * `Limits::new`: the denylist of `home`, narrowed by `root` once rather than per entry.
 *
 * An entry that contains the root is dropped, so pointing the scan root at a denied folder
 * unlocks it. That is the policy and not an accident — and it is what makes the standard
 * list usable at all, since `/` is the ancestor of everything and would otherwise refuse
 * every entry of every batch.
 */
export function limitsFor(root: string, home: string = FIXTURE_ROOT): Limits {
  return { root, denied: deniedFor(home).filter((denied) => !isAtOrUnder(root, denied)) };
}

/** `a.starts_with(b)` for paths: component by component, so `/h/ab` is not inside `/h/a`. */
function isAtOrUnder(path: string, prefix: string): boolean {
  return prefix === '/' ? path.startsWith('/') : path === prefix || path.startsWith(`${prefix}/`);
}

/** The components of a path, the way `Path::components` reads them: `.` and `//` are noise. */
function componentsOf(path: string): string[] {
  return path.split('/').filter((part) => part !== '' && part !== '.');
}

/** `Limits::check`: the normalized path to delete, or the reason the rules refuse it. */
export function checkPath(
  limits: Limits,
  path: string,
): { judged: string } | { reason: BlockReason } {
  // 1. No last component to speak of: `/`, or a path ending in `..`, both of which name
  //    something other than they appear to. Judged as written, before anything is resolved.
  const written = componentsOf(path);
  if (written.length === 0 || written[written.length - 1] === '..') {
    return { reason: 'malformed' };
  }
  // 2. Only the parent is resolved, and the mock's filesystem has no working directory, so
  //    the parent of a relative path is exactly the one that cannot be resolved.
  if (!path.startsWith('/')) {
    return { reason: 'missing' };
  }
  // 3. Lexical where the backend calls `canonicalize`: no directory of the fixture is a
  //    symlink, so resolving `..` is the whole of the difference.
  const resolved: string[] = [];
  for (const part of written) {
    if (part === '..') resolved.pop();
    else resolved.push(part);
  }
  const judged = `/${resolved.join('/')}`;
  // 4. The root itself and every ancestor of it.
  if (isAtOrUnder(limits.root, judged)) {
    return { reason: 'isRoot' };
  }
  // 5. Component-wise, so `/h/ab` is not inside `/h/a`.
  if (!isAtOrUnder(judged, limits.root)) {
    return { reason: 'outsideRoots' };
  }
  // 6. The denied entry itself and everything below it; `limitsFor` has already dropped the
  //    entries that contain the root.
  if (limits.denied.some((denied) => isAtOrUnder(judged, denied))) {
    return { reason: 'denylisted' };
  }
  return { judged };
}

/**
 * What the window holds when a batch arrives — which is two questions, not one, because the
 * backend asks two different seams: `ScanManager::root` decides the guards, and
 * `with_result` decides whether the plan can carry a kind and a size at all.
 *
 * The states are the ones `limits_of` documents. A scan that is **running**, and one that
 * **failed**, hold a root and no tree: `start` clears the result and keeps the root. The
 * guards are then exactly the rules the user's choice of root implies, while every entry of
 * the plan is `(other, 0)` — `with_result` answers `None` and `plan_for` has nothing to read.
 * A **cancelled** scan installs its partial tree like a finished one and is `tree`. A tree
 * without a root is not representable here, because the manager cannot be in that state.
 */
export type HeldScan =
  { held: 'nothing' } | { held: 'root'; root: string } | { held: 'tree'; root: string };

/**
 * `Tree::find`: the node the scan recorded, under the spelling the caller sent.
 *
 * Four of its rules are visible from here. It strips the root's own path first, and strips
 * it from an **absolute** base — so a relative path finds nothing at all, and a path outside
 * the root finds nothing whatever else exists. Then `Path::components` treats `.` and `//`
 * as noise, while a `..` is a component of its own that matches no child: `a/b/.` names the
 * same node as `a/b`, and `a/x/../b` names none.
 */
function treeFind(path: string, root: string): FixtureNode | undefined {
  if (!path.startsWith('/')) {
    return undefined;
  }
  const spelled = `/${componentsOf(path).join('/')}`;
  return isAtOrUnder(spelled, root) ? fixtureEntry(spelled) : undefined;
}

/** The root the guards judge against, or null when nothing has been scanned. */
function rootOf(scan: HeldScan): string | null {
  return scan.held === 'nothing' ? null : scan.root;
}

/** The root of the tree the plan reads kinds and sizes from, or null when there is none. */
function treeOf(scan: HeldScan): string | null {
  return scan.held === 'tree' ? scan.root : null;
}

/**
 * `preview_batch`: every path against the guards and the fixture, with nothing touched.
 *
 * With no scan root — nothing scanned at all — `refused_preview` blocks every entry as
 * `outsideRoots`, because nothing is inside a root that does not exist. It copies the kind
 * and the size from the plan, which in that state has neither.
 */
export function mockActionPreview(
  paths: readonly string[],
  mode: DeletionMode,
  scan: HeldScan,
): Preview {
  const entries: PreviewEntry[] = [];
  /** Where each still-ready entry sits, and the form the batch comparison needs. */
  const ready: Array<{ index: number; judged: string }> = [];
  // Built once for the batch, as `limits_of` builds it once for the command.
  const root = rootOf(scan);
  const limits = root === null ? null : limitsFor(root);
  const treeRoot = treeOf(scan);
  for (const path of paths) {
    // `plan_for`: the kind and the size come from the tree the window holds. Without one —
    // while a scan runs, or after a failed one — every entry of the plan is `(other, 0)`,
    // and the dialog promises no bytes it cannot name.
    const planned = treeRoot === null ? undefined : treeFind(path, treeRoot);
    const kind = planned?.kind ?? 'other';
    const size = planned?.size ?? 0;
    const checked = limits === null ? { reason: 'outsideRoots' as const } : checkPath(limits, path);
    if ('reason' in checked) {
      // Without a normalized path, the only honest thing to show is what was asked for.
      entries.push({ path, kind, size, status: { state: 'blocked', reason: checked.reason } });
      continue;
    }
    // The disk, not the tree: `check_entry` stats the entry whatever the window holds. The
    // fixture is both here, so an entry the scan could not list does not exist here either.
    const onDisk = fixtureEntry(checked.judged);
    if (onDisk === undefined) {
      const status = { state: 'blocked', reason: 'missing' } as const;
      entries.push({ path: checked.judged, kind, size, status });
      continue;
    }
    ready.push({ index: entries.length, judged: checked.judged });
    // Only the kind is re-read, as `check_entry` does: a stale size costs nothing, and a
    // stale kind deletes the wrong thing.
    entries.push({ path: checked.judged, kind: onDisk.kind, size, status: { state: 'ready' } });
  }
  // `drop_nested`, over the still-ready entries only: one that will not be deleted cannot
  // swallow the one below it. A strict ancestor always wins; between two spellings of one
  // entry, the earlier one does.
  for (const [i, entry] of ready.entries()) {
    const swallowed = ready.some(
      ({ judged }, j) =>
        i !== j && isAtOrUnder(entry.judged, judged) && (judged !== entry.judged || j < i),
    );
    if (swallowed) {
      entries[entry.index].status = { state: 'blocked', reason: 'nested' };
    }
  }
  const totalBytes = entries
    .filter((entry) => entry.status.state === 'ready')
    .reduce((sum, entry) => sum + entry.size, 0);
  return { entries, totalBytes, mode };
}

/**
 * `run_batch`: deletes what the guards allow, records the batch and patches the tree.
 *
 * The two warnings always come back clear. The mock has no port that can refuse a deletion,
 * no log that can fail to be written and no patch that can be dropped, so a batch that got
 * this far did all three.
 */
export function mockActionRun(
  paths: readonly string[],
  mode: DeletionMode,
  scan: HeldScan,
): BatchResult {
  const preview = mockActionPreview(paths, mode, scan);
  // One instant for the whole batch, read before the first deletion.
  const at = new Date().toISOString();
  const entries: EntryOutcome[] = [];
  let freedBytes = 0;
  for (const entry of preview.entries) {
    if (entry.status.state === 'blocked') {
      // Reported where the user left it, with the reason they were shown.
      const result = { result: 'skipped', reason: entry.status.reason } as const;
      entries.push({ path: entry.path, kind: entry.kind, result });
      continue;
    }
    // `run_entry` stats the entry again before it deletes, because the world can change
    // between the dialog and the confirmation. Mirrored deliberately, and unreachable here:
    // the mock's world is one array, and nothing else holds it while a batch runs.
    const node = fixtureEntry(entry.path);
    if (node === undefined) {
      const result = { result: 'skipped', reason: 'missing' } as const;
      entries.push({ path: entry.path, kind: entry.kind, result });
      continue;
    }
    removeSubtree(node);
    freedBytes += entry.size;
    // The size the plan carried and the dialog promised, never a re-read of a subtree that
    // is no longer there.
    const result = { result: 'removed', bytes: entry.size } as const;
    entries.push({ path: entry.path, kind: entry.kind, result });
  }
  const outcome: Outcome = { entries, freedBytes, at, mode };
  appendToLog(outcome);
  return { outcome, recorded: true, treeStale: false };
}

/**
 * Puts the tree back as it was built and forgets the log. `resetIpcMock` calls it, and so
 * does the test setup, so a batch in one test is never visible in the next.
 */
export function resetMockActions(): void {
  resetFixtureTree();
  resetActionLog();
}
