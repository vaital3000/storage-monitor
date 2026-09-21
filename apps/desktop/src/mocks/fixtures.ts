// The scanned tree the mock answers from: a deterministic home folder for mock mode, unit
// tests and e2e — about 60 nodes and 180 GB, with the artifacts the cleanup modules will
// target, two folders the scanner cannot read, one it could read only in part, one mount
// point it skips, and a previous snapshot for deltas.
//
// This file is the tree and nothing else. `actions.ts` mirrors the guards and the engine
// over it, and `actionLog.ts` mirrors the record of what they did.
//
// The tree is mutable through `patchTree` and nowhere else: a batch takes rows out of it
// and then every id is handed out again over what is left, exactly as a deletion followed
// by `ScanManager::patch_paths` and `install_patches` does in the app. `resetFixtureTree`
// puts the whole tree back, and `resetMockActions` calls it between tests.

import type { Crumb, Delta, DiskUsage, NodeId, NodeKind, NodeView, ScanStatus } from '../lib/ipc';

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
    // Small, and here for its verdict rather than its size: the one row of the fixture that
    // is denied outright inside a shielded folder, which is what tells the two lists apart
    // on screen. `guard-cases.json` names it, so it has to exist on this side too.
    dir('Keychains', [file('login.keychain-db', mb(18), 0)]),
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

/** Siblings largest first, ties by name: the order `Tree::flatten` numbers them in. */
function bySizeThenName(a: { size: number; name: string }, b: { size: number; name: string }) {
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

/** Every node as the walker left it; `resetFixtureTree` puts this back. */
const SCANNED: readonly FixtureNode[] = flatten(build(HOME));

function copyOf(nodes: readonly FixtureNode[]): FixtureNode[] {
  return nodes.map((node) => ({ ...node, children: [...node.children] }));
}

/**
 * Every node of the tree **as it is now**, indexed by id.
 *
 * Reassigned, not mutated, by `renumberTree` below — which is why it is a `let` and why
 * everything here reads it through the binding rather than through a copy taken at import.
 */
export let fixtureNodes: readonly FixtureNode[] = copyOf(SCANNED);

let byPath = new Map(fixtureNodes.map((node) => [node.path, node]));

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

/**
 * One page of the tree, built like `NodeView::build`; throws for an id the arena has not
 * handed out.
 *
 * An id that named a node the last batch deleted does **not** throw — it names whatever
 * node holds that slot now, which is the whole hazard `docs/adr/0005` is about: nothing
 * upstream renumbers the ids the UI is holding, so asking again with one of them quietly
 * answers about another directory.
 */
export function fixtureNodeView(id: NodeId = 0, limit = 500): NodeView {
  const node = fixtureNodes[id];
  if (node === undefined) {
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
  return {
    state: 'done',
    root: FIXTURE_ROOT,
    // The three `patched_stats` reads from the tree, from where it reads them: the root's
    // own file count, the directories still in the arena, and the root's size.
    files: fixtureNodes[0].fileCount,
    dirs: fixtureNodes.filter((node) => node.kind === 'dir').length,
    bytes: fixtureNodes[0].size,
    // The errors are the scan's, and a patch does not touch them — `patched_stats` carries
    // `stats.errors` through untouched. Deleting a folder the walker could not read does not
    // unmake the moment it could not read it, so this counts over `SCANNED`, the tree as it
    // was walked, and not over the one a batch has since taken rows out of.
    errors: SCANNED.filter((node) => node.error === PERMISSION_DENIED).length + PARTIAL_READ_ERRORS,
    currentPath: '',
    durationMs: SCAN_DURATION_MS,
    error: null,
    hasPrevious: true,
    previousTakenAt: PREVIOUS_TAKEN_AT,
  };
}

/** The entry at an absolute path, or `undefined` when nothing is there: the mock's `stat`. */
export function fixtureEntry(path: string): FixtureNode | undefined {
  return byPath.get(path);
}

/** The parent of a node, or `undefined` for the root. */
export function parentOf(node: FixtureNode): FixtureNode | undefined {
  return node.parent === null ? undefined : fixtureNodes[node.parent];
}

/**
 * Takes a node and everything under it out of the tree, and shrinks every ancestor by what
 * it held — which is what a deletion followed by `patch_paths` leaves behind.
 *
 * The arithmetic stands in for the rescan the manager really does. In the fixture the two
 * agree, because nothing else can have changed on a disk that is this array.
 *
 * Not exported: on its own it leaves the arena holding nodes that are gone, and
 * `fixtureStatusDone` counts directories over that arena. `patchTree` is the way in.
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
    byPath.delete(gone[next].path);
    for (const child of gone[next].children) {
      gone.push(fixtureNodes[child]);
    }
  }
}

/**
 * Hands out new ids: breadth first, siblings largest first, over what is left — which is
 * what `install_patches` does to the arena, and `replace_subtrees` to every sibling group
 * holding a node whose size changed.
 *
 * Every id the caller was holding now means a different node, or nothing; the nodes
 * themselves are new objects, so a reference taken before this is a snapshot of the old
 * arena and not a window onto the new one. Both of those are the app's own behaviour, and
 * until the fixture did this no UI test could see either.
 */
function renumberTree(): void {
  const old = fixtureNodes;
  const nodes: FixtureNode[] = [];
  const pending: Array<{ id: NodeId; children: readonly NodeId[] }> = [];
  const place = (node: FixtureNode, parent: NodeId | null): NodeId => {
    const id = nodes.length;
    nodes.push({ ...node, id, parent, children: [] });
    pending.push({ id, children: node.children });
    return id;
  };
  place(old[0], null);
  for (let next = 0; next < pending.length; next += 1) {
    const { id, children } = pending[next];
    for (const child of children.map((childId) => old[childId]).sort(bySizeThenName)) {
      nodes[id].children.push(place(child, id));
    }
  }
  fixtureNodes = nodes;
  byPath = new Map(nodes.map((node) => [node.path, node]));
}

/**
 * The splice, as `ScanManager::patch_paths` and `install_patches` do it together: the
 * subtrees in `gone` leave the tree, and then every id is handed out again over what is
 * left. The only way to change this tree, so that the two halves cannot come apart — the
 * arena is left holding nodes that no longer exist if the second one is skipped, and
 * `fixtureStatusDone` counts its directories.
 *
 * `touched` is not `gone.length > 0`, and that is the whole reason it is a parameter. The
 * app splices for every path in `touched` — every entry **removed or failed** — and a
 * failed entry is still on disk: its subtree is rescanned rather than dropped, and the
 * arena is rebuilt all the same. So a batch that only failed removes no node and moves
 * every id. `ExplorerPage`'s `afterBatch` reads the same two verdicts to decide whether to
 * re-anchor, and these two have to agree.
 */
export function patchTree(gone: readonly FixtureNode[], touched: boolean): void {
  for (const node of gone) {
    removeSubtree(node);
  }
  if (touched) {
    renumberTree();
  }
}

/** Every node back as the fixture was built. Half of `resetMockActions`. */
export function resetFixtureTree(): void {
  fixtureNodes = copyOf(SCANNED);
  byPath = new Map(fixtureNodes.map((node) => [node.path, node]));
}
