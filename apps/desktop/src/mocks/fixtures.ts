// A deterministic home folder for mock mode, unit tests and e2e: about 60 nodes and
// 180 GB, with the artifacts the cleanup modules will target, two folders the scanner
// cannot read, one it could read only in part, one mount point it skips, and a previous
// snapshot for deltas.
//
// The tree is mutable from `mockActionRun` down: a batch takes rows out of it exactly as a
// deletion followed by `ScanManager::patch_paths` does in the app. `resetMockActions` puts
// every node back, and the test setup calls it between tests.

import type {
  ActivityEntry,
  BatchResult,
  BlockReason,
  Crumb,
  Delta,
  DiskUsage,
  EntryOutcome,
  LogTail,
  Mode,
  NodeId,
  NodeKind,
  NodeView,
  Outcome,
  Preview,
  PreviewEntry,
  ScanStatus,
} from '../lib/ipc';

/** Root of the fixture tree; `default_root` returns it in mock mode. */
export const FIXTURE_ROOT = '/Users/demo';

/** What the scanner reports for a folder protected by Full Disk Access. */
export const PERMISSION_DENIED = 'Operation not permitted (os error 1)';
/** What the scanner reports for a directory on another volume. */
export const DIFFERENT_VOLUME = 'skipped: different volume';
/** What the scanner reports for a directory it could list only in part. */
export const PARTIAL_READ = '3 entries could not be read';
/** The entries behind `PARTIAL_READ`; the walker counts each of them as an error. */
const PARTIAL_READ_ERRORS = 3;

/** One node of the fixture, numbered like the backend's tree: breadth first, siblings largest first. */
export interface FixtureNode {
  id: NodeId;
  parent: NodeId | null;
  /** Last path component; `demo` for the root. */
  name: string;
  /** Absolute path. */
  path: string;
  kind: NodeKind;
  /** Allocated bytes; directories sum their children. */
  size: number;
  logicalSize: number;
  /** Non-directory entries in the subtree; 1 for a file or a symlink. */
  fileCount: number;
  /** Seconds since the Unix epoch. */
  mtime: number;
  error: string | null;
  /** Ids of the children, largest first. */
  children: NodeId[];
}

const KB = 1e3;
const MB = 1e6;
const GB = 1e9;
const DAY = 86_400;
/** When the fixture scan ran: 2026-09-18 09:30 UTC. */
const SCANNED_AT = Date.UTC(2026, 8, 18, 9, 30) / 1000;
const PREVIOUS_TAKEN_AT = '2026-09-11T08:15:42Z';
const SCAN_DURATION_MS = 4_812;

const kb = (n: number) => Math.round(n * KB);
const mb = (n: number) => Math.round(n * MB);
const gb = (n: number) => Math.round(n * GB);

interface Spec {
  name: string;
  kind: NodeKind;
  size: number;
  logicalSize: number;
  /** Age of the entry; null lets a directory take the age of its newest child. */
  daysAgo: number | null;
  error: string | null;
  children: Spec[];
}

function dir(name: string, children: Spec[]): Spec {
  return { name, kind: 'dir', size: 0, logicalSize: 0, daysAgo: null, error: null, children };
}

function file(
  name: string,
  size: number,
  daysAgo: number,
  extra: { kind?: NodeKind; logicalSize?: number } = {},
): Spec {
  return {
    name,
    kind: extra.kind ?? 'file',
    size,
    logicalSize: extra.logicalSize ?? size,
    daysAgo,
    error: null,
    children: [],
  };
}

/** A directory whose entries could not be listed: only its own metadata is known. */
function unreadable(name: string, error: string, daysAgo: number): Spec {
  return { name, kind: 'dir', size: 0, logicalSize: 0, daysAgo, error, children: [] };
}

/** A directory some of whose entries could not be read; the rest was walked as usual. */
function partial(name: string, error: string, children: Spec[]): Spec {
  return { ...dir(name, children), error };
}

const HOME: Spec = dir('demo', [
  dir('Library', [
    dir('Developer', [
      dir('Xcode', [
        dir('DerivedData', [
          dir('Dodo-cxjtbwqrnlvzmegakfoyuhpsdi', [
            dir('Build', [dir('Intermediates.noindex', [file('build.db', gb(26.83), 1)])]),
          ]),
          dir('StorageMonitor-eqzsvxkwdanbhjmurfgtlyopci', [
            dir('Build', [dir('Intermediates.noindex', [file('build.db', gb(12.61), 0)])]),
          ]),
          dir('Playground-ymhdcwuiraxnfkzgsotlvpjbeq', [
            dir('Build', [dir('Intermediates.noindex', [file('build.db', gb(6.24), 41)])]),
          ]),
        ]),
      ]),
    ]),
    dir('Caches', [
      dir('com.apple.dt.Xcode', [
        dir('Downloads', [file('iOS 18.0 Simulator Runtime.dmg', gb(7.49), 63)]),
      ]),
      dir('Homebrew', [
        file('9f3e2a1c--llvm--19.1.7.arm64_sequoia.bottle.tar.gz', gb(2.13), 12),
        file('llvm--19.1.7.arm64_sequoia.bottle.tar.gz', 0, 12, {
          kind: 'symlink',
          logicalSize: 64,
        }),
      ]),
    ]),
    dir('Containers', [
      dir('com.docker.docker', [
        dir('Data', [
          dir('vms', [
            dir('0', [
              // A sparse disk image: far more logical than allocated bytes.
              dir('data', [file('Docker.raw', gb(40.11), 2, { logicalSize: 64 * 1024 ** 3 })]),
            ]),
          ]),
        ]),
      ]),
      unreadable('com.apple.mail', PERMISSION_DENIED, 1),
    ]),
    partial('Application Support', PARTIAL_READ, [
      dir('Code', [file('CachedData', gb(1.26), 2)]),
      file('Slack.db', mb(640), 1),
    ]),
  ]),
  dir('Downloads', [
    file('macOS Sequoia 15.6 Installer.dmg', gb(13.87), 9),
    file('Xcode_16.4.xip', gb(14.81), 27),
    file('q3-report.pdf', mb(2.4), 5),
  ]),
  dir('Documents', [
    dir('Design', [file('hero-assets.psd', gb(4.41), 4), file('mockups.sketch', gb(1.78), 18)]),
    file('thesis.docx', mb(14.2), 33),
  ]),
  dir('src', [
    dir('storage-monitor', [
      dir('target', [
        dir('debug', [
          dir('deps', [file('libtauri-3f9a1c2e.rlib', gb(1.92), 0)]),
          file('storage-monitor', mb(712), 0),
        ]),
      ]),
    ]),
    dir('dodo-web', [
      dir('node_modules', [dir('next', [file('next-swc.darwin-arm64.node', mb(140.3), 21)])]),
      dir('.next', [dir('cache', [file('0.pack', gb(6.31), 6)])]),
    ]),
  ]),
  dir('Movies', [
    file('family-2025.mov', gb(18.94), 88),
    file('screen-recording.mp4', gb(3.21), 2),
  ]),
  dir('Pictures', [file('Photos Library.photoslibrary', gb(17.9), 1)]),
  unreadable('.Trash', PERMISSION_DENIED, 3),
  unreadable('OrbStack', DIFFERENT_VOLUME, 0),
  file('.zshrc', kb(3.1), 120),
]);

/**
 * Growth since the previous snapshot, by root-relative path; negative when a path shrank.
 * A directory whose growth is explained by one child carries the child's growth, like a
 * real pair of snapshots would, so `top_growers` names the deepest culprit.
 */
const GROWTH: ReadonlyArray<[string, number]> = [
  ['', gb(5.6)],
  ['Library', gb(6.4)],
  ['Library/Developer', gb(6.2)],
  ['Library/Developer/Xcode', gb(6.2)],
  ['Library/Developer/Xcode/DerivedData', gb(6.2)],
  ['Library/Developer/Xcode/DerivedData/Dodo-cxjtbwqrnlvzmegakfoyuhpsdi', gb(3.1)],
  ['Library/Containers/com.docker.docker', gb(1.1)],
  ['Library/Containers/com.docker.docker/Data', gb(1.1)],
  ['Library/Containers/com.docker.docker/Data/vms', gb(1.1)],
  ['Library/Containers/com.docker.docker/Data/vms/0', gb(1.1)],
  ['Library/Containers/com.docker.docker/Data/vms/0/data', gb(1.1)],
  ['Library/Caches', -gb(0.9)],
  ['Downloads', -gb(4.2)],
  ['Documents/Design/hero-assets.psd', gb(0.6)],
  ['src', gb(1.6)],
  ['src/storage-monitor', gb(1.4)],
  ['src/storage-monitor/target', gb(1.4)],
  ['Movies', gb(1.2)],
];

interface Built extends Omit<Spec, 'daysAgo' | 'children'> {
  fileCount: number;
  mtime: number;
  children: Built[];
}

/** Aggregates sizes, file counts and modification times bottom-up, like the walker. */
function build(spec: Spec): Built {
  const children = spec.children.map(build);
  const isDir = spec.kind === 'dir';
  const sum = (pick: (child: Built) => number) => children.reduce((acc, c) => acc + pick(c), 0);
  const mtime =
    spec.daysAgo === null
      ? Math.max(SCANNED_AT - 30 * DAY, ...children.map((c) => c.mtime))
      : SCANNED_AT - spec.daysAgo * DAY;
  return {
    name: spec.name,
    kind: spec.kind,
    size: isDir ? sum((c) => c.size) : spec.size,
    logicalSize: isDir ? sum((c) => c.logicalSize) : spec.logicalSize,
    fileCount: isDir ? sum((c) => c.fileCount) : 1,
    mtime,
    error: spec.error,
    children,
  };
}

function bySizeThenName(a: Built, b: Built): number {
  return b.size - a.size || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}

/** Numbers the nodes breadth first with contiguous sibling ranges, like `Tree::flatten`. */
function flatten(root: Built): FixtureNode[] {
  const nodes: FixtureNode[] = [];
  const pending: Array<{ id: NodeId; children: Built[] }> = [];
  const place = (built: Built, parent: NodeId | null, path: string): NodeId => {
    const id = nodes.length;
    const { children, ...fields } = built;
    nodes.push({ ...fields, id, parent, path, children: [] });
    pending.push({ id, children });
    return id;
  };
  place(root, null, FIXTURE_ROOT);
  for (let next = 0; next < pending.length; next += 1) {
    const { id, children } = pending[next];
    for (const child of [...children].sort(bySizeThenName)) {
      nodes[id].children.push(place(child, id, `${nodes[id].path}/${child.name}`));
    }
  }
  return nodes;
}

/** Every node of the fixture, indexed by id. */
export const fixtureNodes: readonly FixtureNode[] = flatten(build(HOME));

const byPath = new Map(fixtureNodes.map((node) => [node.path, node]));

/**
 * Ids of the nodes a batch deleted. Nothing is spliced out of `fixtureNodes`, which is
 * indexed by id: a deleted node keeps its slot and leaves `byPath`, its parent's children
 * and every count instead.
 */
const removed = new Set<NodeId>();

/** Every node as the fixture was built, so that `resetMockActions` can put it back. */
const pristine = fixtureNodes.map((node) => ({
  size: node.size,
  logicalSize: node.logicalSize,
  fileCount: node.fileCount,
  children: [...node.children],
}));

/** The node at an absolute or root-relative path; throws for a path outside the fixture. */
export function fixtureNode(path: string): FixtureNode {
  const absolute =
    path === '' ? FIXTURE_ROOT : path.startsWith('/') ? path : `${FIXTURE_ROOT}/${path}`;
  const node = byPath.get(absolute);
  if (node === undefined) {
    throw new Error(`unknown fixture path ${absolute}`);
  }
  return node;
}

/** Sizes in the previous snapshot by absolute path; only these paths get a delta. */
export const previousSizes: ReadonlyMap<string, number> = new Map(
  GROWTH.map(([relative, growth]) => {
    const node = fixtureNode(relative);
    return [node.path, node.size - growth];
  }),
);

function deltaOf(node: FixtureNode): number | null {
  const before = previousSizes.get(node.path);
  return before === undefined ? null : node.size - before;
}

/** One page of the tree, built like `NodeView::build`; throws for an unknown id. */
export function fixtureNodeView(id: NodeId = 0, limit = 500): NodeView {
  const node = fixtureNodes[id];
  // A deleted node is as unknown as one that never existed: after the real splice the whole
  // arena is rebuilt and none of its ids mean what they did.
  if (node === undefined || removed.has(id)) {
    throw new Error(`unknown node ${id}`);
  }
  const breadcrumbs: Crumb[] = [];
  for (let current: FixtureNode | undefined = node; current;) {
    breadcrumbs.unshift({ id: current.id, name: current.name });
    current = current.parent === null ? undefined : fixtureNodes[current.parent];
  }
  const children = node.children.slice(0, limit).map((childId) => {
    const child = fixtureNodes[childId];
    return {
      id: child.id,
      name: child.name,
      kind: child.kind,
      size: child.size,
      logicalSize: child.logicalSize,
      fileCount: child.fileCount,
      mtime: child.mtime,
      error: child.error,
      delta: deltaOf(child),
      hasChildren: child.children.length > 0,
    };
  });
  return {
    id: node.id,
    name: node.name,
    path: node.path,
    kind: node.kind,
    size: node.size,
    logicalSize: node.logicalSize,
    fileCount: node.fileCount,
    mtime: node.mtime,
    error: node.error,
    delta: deltaOf(node),
    breadcrumbs,
    children,
    childrenTotal: node.children.length,
    truncated: node.children.length > limit,
  };
}

function computeGrowers(): Delta[] {
  const growing: Delta[] = [];
  for (const [path, before] of previousSizes) {
    const node = fixtureNode(path);
    if (node.kind === 'dir' && node.size > before) {
      growing.push({ path, kind: node.kind, before, after: node.size, delta: node.size - before });
    }
  }
  const biggestChild = new Map<string, number>();
  for (const { path, delta } of growing) {
    const parent = path.slice(0, path.lastIndexOf('/'));
    biggestChild.set(parent, Math.max(biggestChild.get(parent) ?? 0, delta));
  }
  return growing
    .filter(({ path, delta }) => (biggestChild.get(path) ?? 0) < 0.8 * delta)
    .sort((a, b) => b.delta - a.delta || (a.path < b.path ? -1 : 1));
}

/** The growers as the finished scan left them; `fixtureGrowers` hands out copies. */
const GROWERS: readonly Delta[] = computeGrowers();

/**
 * Growing directories, largest first, like `snapshot::top_growers`: a directory is left
 * out when one child explains at least 80% of its growth (the child is listed instead).
 *
 * Read once, when the fixture is built, because that is when `ScanManager` computes the
 * list it hands to `top_growers` — a splice does not touch it. So a batch cannot change
 * these numbers, and a path a batch deleted still reports the size the scan recorded.
 */
export function fixtureGrowers(): Delta[] {
  return GROWERS.map((grower) => ({ ...grower }));
}

/** A 1 TB volume with the fixture's home folder on it. */
export function fixtureDisk(): DiskUsage {
  const total = 994_662_584_320;
  const free = 312_401_281_024;
  return { path: FIXTURE_ROOT, total, available: 298_184_282_112, free, used: total - free };
}

/** The status of the finished fixture scan, with a previous snapshot to compare against. */
export function fixtureStatusDone(): ScanStatus {
  const count = (test: (node: FixtureNode) => boolean) =>
    fixtureNodes.filter((node) => !removed.has(node.id) && test(node)).length;
  return {
    state: 'done',
    root: FIXTURE_ROOT,
    files: count((n) => n.kind !== 'dir'),
    dirs: count((n) => n.kind === 'dir'),
    bytes: fixtureNodes[0].size,
    errors: count((n) => n.error === PERMISSION_DENIED) + PARTIAL_READ_ERRORS,
    currentPath: '',
    durationMs: SCAN_DURATION_MS,
    error: null,
    hasPrevious: true,
    previousTakenAt: PREVIOUS_TAKEN_AT,
  };
}

// The mock's half of `crates/core/src/action`: the guards, the deletion and the record of
// it, over the fixture tree instead of a disk. Every rule below mirrors one the backend
// enforces, because this is the only oracle the UI tests have — a mock that says yes to
// everything makes them prove nothing.

/**
 * Where deletion is never allowed: the standard denylist of `Limits::with_home`, with the
 * fixture root as the home folder — which is what it is, since `default_root` returns it
 * exactly as the app returns `$HOME`. So the fixture's whole `Library` subtree is
 * undeletable here, as `~/Library` is in the app until a module declares a path inside it.
 */
const DENIED: readonly string[] = [
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
  `${FIXTURE_ROOT}/Library`,
  FIXTURE_ROOT,
];

/** The action log, oldest line first: the JSONL file the backend appends to, as an array. */
export const mockActionLog: string[] = [];

/** `a.starts_with(b)` for paths: component by component, so `/h/ab` is not inside `/h/a`. */
function isAtOrUnder(path: string, prefix: string): boolean {
  return prefix === '/' ? path.startsWith('/') : path === prefix || path.startsWith(`${prefix}/`);
}

/** The components of a path, the way `Path::components` reads them: `.` and `//` are noise. */
function componentsOf(path: string): string[] {
  return path.split('/').filter((part) => part !== '' && part !== '.');
}

/** `Limits::check`: the normalized path to delete, or the reason the rules refuse it. */
function checkPath(path: string, root: string): { judged: string } | { reason: BlockReason } {
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
  if (isAtOrUnder(root, judged)) {
    return { reason: 'isRoot' };
  }
  // 5. Component-wise, so `/h/ab` is not inside `/h/a`.
  if (!isAtOrUnder(judged, root)) {
    return { reason: 'outsideRoots' };
  }
  // 6. The denied entry itself and everything below it — except an entry that contains the
  //    root, which `Limits::new` drops so that scanning a denied folder unlocks it.
  if (DENIED.some((denied) => !isAtOrUnder(root, denied) && isAtOrUnder(judged, denied))) {
    return { reason: 'denylisted' };
  }
  return { judged };
}

/**
 * `preview_batch`: every path against the guards and the fixture, with nothing touched.
 *
 * `root` is the scan root the window holds; `null` is the state where nothing has been
 * scanned, which `refused_preview` answers by blocking every entry as `outsideRoots` —
 * nothing is inside a root that does not exist.
 */
export function mockActionPreview(
  paths: readonly string[],
  mode: Mode,
  root: string | null,
): Preview {
  const entries: PreviewEntry[] = [];
  /** Where each still-ready entry sits, and the form the batch comparison needs. */
  const ready: Array<{ index: number; judged: string }> = [];
  for (const path of paths) {
    // `plan_for`: the kind and the size come from the tree, under the spelling the caller
    // sent, which is the spelling the scan recorded. A path it does not know carries neither.
    const planned = byPath.get(path);
    const kind = planned?.kind ?? 'other';
    const size = planned?.size ?? 0;
    const checked = root === null ? { reason: 'outsideRoots' as const } : checkPath(path, root);
    if ('reason' in checked) {
      // Without a normalized path, the only honest thing to show is what was asked for.
      entries.push({ path, kind, size, status: { state: 'blocked', reason: checked.reason } });
      continue;
    }
    const onDisk = byPath.get(checked.judged);
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
  mode: Mode,
  root: string | null,
): BatchResult {
  const preview = mockActionPreview(paths, mode, root);
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
    const node = byPath.get(entry.path);
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

/** The parent of a node, or `undefined` for the root. */
function parentOf(node: FixtureNode): FixtureNode | undefined {
  return node.parent === null ? undefined : fixtureNodes[node.parent];
}

/**
 * Takes a node and everything under it out of the tree, and shrinks every ancestor by what
 * it held — which is what a deletion followed by `patch_paths` leaves behind.
 */
function removeSubtree(node: FixtureNode): void {
  const parent = parentOf(node);
  if (parent !== undefined) {
    parent.children.splice(parent.children.indexOf(node.id), 1);
  }
  for (let ancestor = parent; ancestor !== undefined; ancestor = parentOf(ancestor)) {
    ancestor.size -= node.size;
    ancestor.logicalSize -= node.logicalSize;
    ancestor.fileCount -= node.fileCount;
  }
  const gone: FixtureNode[] = [node];
  for (let next = 0; next < gone.length; next += 1) {
    removed.add(gone[next].id);
    byPath.delete(gone[next].path);
    for (const child of gone[next].children) {
      gone.push(fixtureNodes[child]);
    }
  }
}

/** `ActionLog::append`: one line per entry, in the order of the batch. */
function appendToLog(outcome: Outcome): void {
  for (const entry of outcome.entries) {
    mockActionLog.push(JSON.stringify(logEntry(entry, outcome)));
  }
}

/** `LogEntry::of`: the verdict alone, with what it carried split into `detail` and `bytes`. */
function logEntry(entry: EntryOutcome, outcome: Outcome): ActivityEntry {
  const line = { at: outcome.at, path: entry.path, kind: entry.kind, mode: outcome.mode };
  switch (entry.result.result) {
    case 'removed':
      return { ...line, result: 'removed', detail: null, bytes: entry.result.bytes };
    case 'failed':
      return { ...line, result: 'failed', detail: entry.result.message, bytes: 0 };
    case 'skipped':
      return { ...line, result: 'skipped', detail: entry.result.reason, bytes: 0 };
  }
}

const KINDS: readonly string[] = ['dir', 'file', 'symlink', 'other'];
const MODES: readonly string[] = ['trash', 'permanent'];
const RESULTS: readonly string[] = ['removed', 'failed', 'skipped'];

const oneOf = (field: unknown, names: readonly string[]) =>
  typeof field === 'string' && names.includes(field);

/** One line of the log, or `null` when it is not an entry — which is what serde answers. */
function parseLogLine(line: string): ActivityEntry | null {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    return null;
  }
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const entry = value as Record<string, unknown>;
  const complete =
    typeof entry.at === 'string' &&
    typeof entry.path === 'string' &&
    oneOf(entry.kind, KINDS) &&
    oneOf(entry.mode, MODES) &&
    oneOf(entry.result, RESULTS) &&
    (entry.detail === null || typeof entry.detail === 'string') &&
    typeof entry.bytes === 'number';
  return complete ? (value as ActivityEntry) : null;
}

/**
 * `ActionLog::tail`: the last `limit` entries, newest first, and how many lines of the
 * stretch that was read could not be parsed.
 *
 * A damaged line costs one `damaged` and nothing else: it does not use up a slot of `limit`,
 * and it does not stop the read. A blank line is a separator, not damage. `damaged` counts
 * only as far back as the read went, which is as far as `limit` entries reach.
 */
export function mockActivityTail(limit: number): LogTail {
  const entries: ActivityEntry[] = [];
  let damaged = 0;
  for (let line = mockActionLog.length - 1; line >= 0; line -= 1) {
    if (entries.length === limit) {
      break;
    }
    const text = mockActionLog[line];
    if (text.trim() === '') {
      continue;
    }
    const entry = parseLogLine(text);
    if (entry === null) {
      damaged += 1;
    } else {
      entries.push(entry);
    }
  }
  return { entries, damaged };
}

/**
 * Puts every node of the tree back as it was built and forgets the log. `resetIpcMock` calls
 * it, and so does the test setup, so a batch in one test is never visible in the next.
 */
export function resetMockActions(): void {
  mockActionLog.length = 0;
  removed.clear();
  for (const node of fixtureNodes) {
    const was = pristine[node.id];
    node.size = was.size;
    node.logicalSize = was.logicalSize;
    node.fileCount = was.fileCount;
    node.children.splice(0, node.children.length, ...was.children);
    byPath.set(node.path, node);
  }
}
