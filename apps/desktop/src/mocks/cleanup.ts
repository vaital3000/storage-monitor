// The mock's half of `crates/core/src/cleanup/` and of the desktop's `cleanup.rs`: which
// requests a batch may run, the steps the demo module plans for them, and what running them
// does to the mock's world.
//
// Every rule here mirrors one the backend enforces, because this is the only oracle the UI
// tests of the Cleanup screen have. `crates/core/tests/fixtures/cleanup-cases.json` is
// answered by both sides — by `screen` and `reversible` here and in `engine.rs` — so a rule
// that changes in one and not the other reddens a test rather than drifting quietly.

import {
  CLEANUP_PROGRESS_EVENT,
  type ActionSpec,
  type BlockReason,
  type CleanupEntry,
  type CleanupEntryOutcome,
  type CleanupOutcome,
  type CleanupPreview,
  type CleanupPreviews,
  type CleanupRequest,
  type CleanupResult,
  type DeletionMode,
  type EntryResult,
  type EntryStatus,
  type Item,
  type NodeKind,
  type StepEffect,
  type StepView,
} from '../lib/ipc';
import { commandLine } from '../lib/commandLine';
import { appendCleanupToLog } from './actionLog';
import { checkPath, limitsFor } from './actions';
import { FIXTURE_ROOT, fixtureEntry, patchTree, type FixtureNode } from './fixtures';
import {
  DEMO_LAST_CLEANUP,
  DEMO_SANDBOX,
  forgetItems,
  heldItems,
  modulesOf,
  mockModulesRefresh,
  publish,
} from './modules';

/** A path a step deletes, and what the module saw there. */
interface Target {
  path: string;
  kind: NodeKind;
}

/** `Step` of `module/step.rs`, with the command as its argv. */
export type MockStep =
  | { step: 'delete'; target: Target }
  | { step: 'run'; argv: string[]; effect: 'housekeeping' | 'destroys' }
  | { step: 'run'; argv: string[]; effect: 'removes'; target: Target };

/**
 * `screen`: rules 1 to 3 of the design (section 8), in order; the first that applies
 * decides. Disk-free, which is why the shared cases call it directly.
 */
export function screen(
  request: CleanupRequest,
  held: ReadonlyMap<string, Item>,
): { item: Item; action: ActionSpec } | { reason: BlockReason } {
  // 1. Not among the held results.
  const item = held.get(request.item);
  if (item === undefined) {
    return { reason: 'missing' };
  }
  // 2. The item is not what the screen showed: no such action, or no such option.
  const action = item.actions.find((spec) => spec.id === request.action);
  if (action === undefined) {
    return { reason: 'kindChanged' };
  }
  const optionOf = (id: string) => action.options.find((option) => option.id === id);
  if (request.options.some((id) => optionOf(id) === undefined)) {
    return { reason: 'kindChanged' };
  }
  // 3. A Keep verdict is lifted by a force option and by nothing else.
  const forced = request.options.some((id) => optionOf(id)?.force === true);
  if (item.verdict.level === 'keep' && !forced) {
    return { reason: 'kept' };
  }
  return { item, action };
}

/**
 * `reversible`: the mode is Trash, and every step is a deletion or housekeeping. Takes the
 * kinds of the steps, which is all it reads — the shared cases name nothing else.
 */
export function reversible(
  steps: ReadonlyArray<'delete' | StepEffect>,
  mode: DeletionMode,
): boolean {
  return mode === 'trash' && steps.every((kind) => kind === 'delete' || kind === 'housekeeping');
}

/** The kinds `reversible` reads, of the steps a module planned. */
function kindsOf(steps: readonly MockStep[]): Array<'delete' | StepEffect> {
  return steps.map((step) => (step.step === 'delete' ? 'delete' : step.effect));
}

/** `plan` of the demo module: two steps against one for a folder, `rm` for an object. */
function planDemo(item: Item, action: ActionSpec, mode: DeletionMode): MockStep[] {
  if (item.path === null) {
    return [];
  }
  if (item.kind === 'folder' && action.id === 'delete') {
    const remove: MockStep = { step: 'delete', target: { path: item.path, kind: 'dir' } };
    return mode === 'trash'
      ? [remove, { step: 'run', argv: ['touch', DEMO_LAST_CLEANUP], effect: 'housekeeping' }]
      : [remove];
  }
  if (item.kind === 'object' && action.id === 'remove') {
    return [
      {
        step: 'run',
        argv: ['rm', item.path],
        effect: 'removes',
        target: { path: item.path, kind: 'file' },
      },
    ];
  }
  return [];
}

/** The targets a plan deletes. */
function targetsOf(steps: readonly MockStep[]): Target[] {
  return steps.flatMap((step) => ('target' in step ? [step.target] : []));
}

/**
 * Whether a target of the demo's sandbox is on the mock's disk. The demo lives outside the
 * fixture tree, so this asks the demo's own list: the item is held exactly while its entry is.
 */
function onDisk(target: Target, held: ReadonlyMap<string, Item>): boolean {
  if (target.path === DEMO_SANDBOX || target.path.startsWith(`${DEMO_SANDBOX}/`)) {
    return [...held.values()].some((item) => item.path === target.path);
  }
  return fixtureEntry(target.path) !== undefined;
}

/** One request, planned and checked. */
interface Planned {
  request: CleanupRequest;
  item: Item | undefined;
  title: string;
  action: string;
  steps: MockStep[];
  judged: string[];
  size: number;
  status: EntryStatus;
}

/** Rules 1 to 6 over one batch, in one mode. */
function planAll(
  requests: readonly CleanupRequest[],
  held: ReadonlyMap<string, Item>,
  mode: DeletionMode,
): Planned[] {
  const limits = limitsFor(FIXTURE_ROOT);
  const planned = requests.map((request): Planned => {
    const known = held.get(request.item);
    const spec = known?.actions.find((action) => action.id === request.action);
    const entry: Planned = {
      request,
      item: known,
      title: known?.title ?? request.item,
      action: spec?.label ?? request.action,
      steps: [],
      judged: [],
      size: spec?.estimatedFree ?? 0,
      status: { state: 'ready' },
    };
    const screened = screen(request, held);
    if ('reason' in screened) {
      entry.status = { state: 'blocked', reason: screened.reason };
      return entry;
    }
    // 4. The module plans; an empty plan would be reported as done having done nothing.
    entry.steps = planDemo(screened.item, screened.action, mode);
    if (entry.steps.length === 0) {
      entry.status = { state: 'blocked', reason: 'malformed' };
      return entry;
    }
    // 5. Every target through the guards and the disk. The mock's folders and files are
    //    what the module says they are, so the kind comparison has nothing to refuse.
    for (const target of targetsOf(entry.steps)) {
      const checked = checkPath(limits, target.path);
      if ('reason' in checked) {
        entry.status = { state: 'blocked', reason: checked.reason };
        return entry;
      }
      if (!onDisk(target, held)) {
        entry.status = { state: 'blocked', reason: 'missing' };
        return entry;
      }
      entry.judged.push(checked.judged);
    }
    return entry;
  });
  // 6. Across the batch, over the still-ready entries: `drop_nested`'s predicate, comparing
  //    only targets of different entries.
  const ready = planned
    .map((entry, index) => ({ entry, index }))
    .filter(({ entry }) => entry.status.state === 'ready');
  const inside = (path: string, outer: string) => path === outer || path.startsWith(`${outer}/`);
  for (const { entry, index } of ready) {
    const nested = entry.judged.some((target) =>
      ready.some(
        (other) =>
          other.index !== index &&
          other.entry.judged.some(
            (outer) => inside(target, outer) && (outer !== target || other.index < index),
          ),
      ),
    );
    if (nested) {
      entry.status = { state: 'blocked', reason: 'nested' };
    }
  }
  return planned;
}

function view(step: MockStep, mode: DeletionMode): StepView {
  if (step.step === 'delete') {
    return mode === 'trash'
      ? { step: 'trash', path: step.target.path }
      : { step: 'delete', path: step.target.path };
  }
  return {
    step: 'run',
    command: commandLine(step.argv),
    effect: step.effect,
    path: step.effect === 'removes' ? step.target.path : null,
  };
}

function previewIn(
  requests: readonly CleanupRequest[],
  held: ReadonlyMap<string, Item>,
  mode: DeletionMode,
): CleanupPreview {
  const entries = planAll(requests, held, mode).map((planned): CleanupEntry => ({
    item: planned.request.item,
    module: planned.item?.module ?? '',
    title: planned.title,
    action: planned.action,
    steps: planned.steps.map((step) => view(step, mode)),
    size: planned.size,
    status: planned.status,
    reversible: reversible(kindsOf(planned.steps), mode),
  }));
  const totalBytes = entries
    .filter((entry) => entry.status.state === 'ready')
    .reduce((sum, entry) => sum + entry.size, 0);
  return { entries, totalBytes, mode };
}

/** `preview_cleanup`: the batch checked in both modes, with nothing touched. */
export function mockCleanupPreview(requests: readonly CleanupRequest[]): CleanupPreviews {
  const held = heldItems();
  return {
    trash: previewIn(requests, held, 'trash'),
    permanent: previewIn(requests, held, 'permanent'),
  };
}

/** What the next batch does to an item instead of cleaning it, by item id. */
const failures = new Map<string, string>();

/**
 * Makes the first step of `itemId` fail with `message` in every batch from now on, until set
 * back to null — the mock's stand-in for a port that refuses, which the dialog's report of a
 * failure is written against.
 */
export function setMockCleanupFailure(itemId: string, message: string | null): void {
  if (message === null) failures.delete(itemId);
  else failures.set(itemId, message);
}

export function resetMockCleanup(): void {
  failures.clear();
}

/**
 * `run_cleanup`: plans the requests again, runs them with `cleanup:progress` events, records
 * the batch, forgets what went, patches the Explorer's tree and starts a rediscovery.
 *
 * The two warnings come back clear: the mock has no log that can fail to be written and no
 * patch that can be dropped.
 */
export function mockCleanupRun(
  requests: readonly CleanupRequest[],
  mode: DeletionMode,
): CleanupResult {
  const held = heldItems();
  const at = new Date().toISOString();
  const planned = planAll(requests, held, mode);
  const entries: CleanupEntryOutcome[] = [];
  let freedBytes = 0;
  for (const [done, entry] of planned.entries()) {
    publish(CLEANUP_PROGRESS_EVENT, { done, total: planned.length, current: entry.title });
    const targets: string[] = [];
    const commands: string[][] = [];
    let result: EntryResult;
    if (entry.status.state === 'blocked') {
      result = { result: 'skipped', reason: entry.status.reason };
    } else {
      const failure = failures.get(entry.request.item);
      const first = entry.steps[0];
      if (failure !== undefined) {
        // The first step was attempted and refused: its target counts as touched.
        if ('target' in first) targets.push(first.target.path);
        if (first.step === 'run') commands.push(first.argv);
        const label = first.step === 'run' ? `\`${commandLine(first.argv)}\`: ` : '';
        const where = entry.steps.length > 1 ? `step 1 of ${entry.steps.length}, ` : '';
        result = { result: 'failed', message: `${where}${label}${failure}` };
      } else {
        for (const step of entry.steps) {
          if ('target' in step) targets.push(step.target.path);
          if (step.step === 'run') commands.push(step.argv);
        }
        result = { result: 'removed', bytes: entry.size };
        freedBytes += entry.size;
      }
    }
    const target = entry.item?.path ?? null;
    const own = targetsOf(entry.steps).find((candidate) => candidate.path === target);
    entries.push({
      item: entry.request.item,
      module: entry.item?.module ?? '',
      title: entry.title,
      action: entry.action,
      path: target,
      kind: own?.kind ?? null,
      targets,
      mode: reversible(kindsOf(entry.steps), mode) ? mode : 'permanent',
      commands,
      result,
    });
  }
  publish(CLEANUP_PROGRESS_EVENT, { done: planned.length, total: planned.length, current: null });
  const outcome: CleanupOutcome = { entries, freedBytes, at, mode };
  appendCleanupToLog(outcome);
  const removed = entries
    .filter((entry) => entry.result.result === 'removed')
    .map((entry) => entry.item);
  forgetItems(removed);
  // Only targets the fixture tree knows: a patch with nothing to splice moves no id, as
  // `patch_paths` over paths the tree cannot find installs nothing. The demo's sandbox is not
  // in the fixture, which is the real "the scan never saw it".
  const gone = entries
    .filter((entry) => entry.result.result === 'removed' || entry.result.result === 'failed')
    .flatMap((entry) => entry.targets)
    .map((path) => fixtureEntry(path))
    .filter((node): node is FixtureNode => node !== undefined);
  if (gone.length > 0) {
    patchTree(gone, true);
  }
  const involved = modulesOf(requests.map((request) => request.item));
  if (involved.length > 0) {
    mockModulesRefresh(involved);
  }
  return { outcome, recorded: true, treeStale: false };
}
