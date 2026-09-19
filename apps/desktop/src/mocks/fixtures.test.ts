import { describe, expect, it } from 'vitest';
import type { BlockReason, Mode, PreviewEntry } from '../lib/ipc';
import {
  FIXTURE_ROOT,
  PARTIAL_READ,
  PERMISSION_DENIED,
  fixtureDisk,
  fixtureGrowers,
  fixtureNode,
  fixtureNodeView,
  fixtureNodes,
  fixtureStatusDone,
  type HeldScan,
  mockActionLog,
  mockActionPreview,
  mockActionRun,
  mockActivityTail,
  previousSizes,
  resetMockActions,
} from './fixtures';

const GB = 1e9;

describe('fixture tree', () => {
  it('is a home folder of about 60 nodes and about 180 GB', () => {
    expect(fixtureNodes.length).toBeGreaterThanOrEqual(55);
    expect(fixtureNodes.length).toBeLessThanOrEqual(70);
    const root = fixtureNodes[0];
    expect(root.path).toBe(FIXTURE_ROOT);
    expect(root.name).toBe('demo');
    expect(root.size).toBeGreaterThan(170 * GB);
    expect(root.size).toBeLessThan(190 * GB);
  });

  it('numbers nodes breadth first with contiguous, size-sorted sibling ranges', () => {
    for (const node of fixtureNodes) {
      expect(fixtureNodes[node.id]).toBe(node);
      for (let i = 1; i < node.children.length; i += 1) {
        expect(node.children[i]).toBe(node.children[i - 1] + 1);
        const [before, after] = [
          fixtureNodes[node.children[i - 1]],
          fixtureNodes[node.children[i]],
        ];
        expect(before.size >= after.size).toBe(true);
        if (before.size === after.size) expect(before.name < after.name).toBe(true);
      }
    }
  });

  it('keeps directory totals equal to the sum of their children', () => {
    for (const node of fixtureNodes) {
      const children = node.children.map((id) => fixtureNodes[id]);
      if (node.kind === 'dir') {
        expect(node.size).toBe(children.reduce((sum, c) => sum + c.size, 0));
        expect(node.logicalSize).toBe(children.reduce((sum, c) => sum + c.logicalSize, 0));
        expect(node.fileCount).toBe(children.reduce((sum, c) => sum + c.fileCount, 0));
        expect(node.mtime).toBe(Math.max(node.mtime, ...children.map((c) => c.mtime)));
      } else {
        expect(children).toEqual([]);
        expect(node.fileCount).toBe(1);
      }
      for (const child of children) {
        expect(child.parent).toBe(node.id);
        expect(child.path).toBe(`${node.path}/${child.name}`);
      }
    }
  });

  it('contains the developer folders the design calls for', () => {
    const derived = fixtureNode('Library/Developer/Xcode/DerivedData');
    expect(derived.children).toHaveLength(3);
    expect(fixtureNode('src').children.length).toBeGreaterThanOrEqual(2);
    expect(fixtureNode('src/storage-monitor/target').kind).toBe('dir');
    expect(fixtureNode('src/dodo-web/node_modules').kind).toBe('dir');
    const downloads = fixtureNode('Downloads').children.map((id) => fixtureNodes[id]);
    expect(downloads.filter((n) => n.kind === 'file' && n.size > 10 * GB)).toHaveLength(2);
    for (const name of ['Library/Caches', 'Movies', '.Trash', 'Documents']) {
      expect(fixtureNode(name).kind).toBe('dir');
    }
    expect(fixtureNodes.some((n) => n.kind === 'symlink')).toBe(true);
    expect(fixtureNodes.filter((n) => n.parent === 0 && n.kind === 'file').length).toBeGreaterThan(
      0,
    );
  });

  it('has two permission-denied directories and one skipped volume', () => {
    const denied = fixtureNodes.filter((n) => n.error === 'Operation not permitted (os error 1)');
    expect(denied).toHaveLength(2);
    const skipped = fixtureNodes.filter((n) => n.error === 'skipped: different volume');
    expect(skipped).toHaveLength(1);
    for (const node of [...denied, ...skipped]) {
      expect(node.kind).toBe('dir');
      expect(node.size).toBe(0);
      expect(node.children).toEqual([]);
    }
    expect(fixtureNodes.filter((n) => n.error !== null)).toHaveLength(4);
  });

  it('has one partially readable directory that still has children and a size', () => {
    const partial = fixtureNodes.filter((n) => n.error === PARTIAL_READ);
    expect(partial).toHaveLength(1);
    expect(partial[0]).toBe(fixtureNode('Library/Application Support'));
    expect(partial[0].kind).toBe('dir');
    expect(partial[0].children.length).toBeGreaterThan(0);
    expect(partial[0].size).toBeGreaterThan(GB);
    expect(partial[0].fileCount).toBeGreaterThan(0);
  });

  it('resolves absolute and root-relative paths', () => {
    expect(fixtureNode('')).toBe(fixtureNodes[0]);
    expect(fixtureNode(FIXTURE_ROOT)).toBe(fixtureNodes[0]);
    expect(fixtureNode('Library')).toBe(fixtureNode(`${FIXTURE_ROOT}/Library`));
    expect(() => fixtureNode('nope')).toThrow('unknown fixture path /Users/demo/nope');
  });
});

describe('previous sizes', () => {
  it('cover about twenty existing paths with growth and shrinkage', () => {
    expect(previousSizes.size).toBeGreaterThanOrEqual(15);
    expect(previousSizes.size).toBeLessThanOrEqual(24);
    const deltas = [...previousSizes].map(([path, before]) => fixtureNode(path).size - before);
    expect(deltas.filter((d) => d > 0).length).toBeGreaterThanOrEqual(5);
    expect(deltas.filter((d) => d < 0).length).toBeGreaterThanOrEqual(2);
    expect(deltas).not.toContain(0);
    for (const before of previousSizes.values()) expect(before).toBeGreaterThanOrEqual(0);
  });

  it('give the root page positive and negative deltas next to blank ones', () => {
    const view = fixtureNodeView();
    expect(view.delta).not.toBeNull();
    const deltas = view.children.map((c) => c.delta);
    expect(deltas.some((d) => d !== null && d > 0)).toBe(true);
    expect(deltas.some((d) => d !== null && d < 0)).toBe(true);
    expect(deltas.some((d) => d === null)).toBe(true);
  });
});

describe('fixtureNodeView', () => {
  it('builds the root page like the backend does', () => {
    const view = fixtureNodeView();
    expect(view.id).toBe(0);
    expect(view.name).toBe('demo');
    expect(view.path).toBe(FIXTURE_ROOT);
    expect(view.kind).toBe('dir');
    expect(view.error).toBeNull();
    expect(view.breadcrumbs).toEqual([{ id: 0, name: 'demo' }]);
    expect(view.children.length).toBeGreaterThan(5);
    expect(view.childrenTotal).toBe(view.children.length);
    expect(view.truncated).toBe(false);
    const sizes = view.children.map((c) => c.size);
    expect(sizes).toEqual([...sizes].sort((a, b) => b - a));
    expect(view.children[0].name).toBe('Library');
    expect(view.children[0].hasChildren).toBe(true);
    expect(view.children.find((c) => c.name === '.zshrc')?.hasChildren).toBe(false);
    expect(view.children.find((c) => c.name === '.zshrc')?.fileCount).toBe(1);
    expect(view.children.find((c) => c.name === '.Trash')?.error).toBe(
      'Operation not permitted (os error 1)',
    );
    expect(view.children.find((c) => c.name === 'OrbStack')?.error).toBe(
      'skipped: different volume',
    );
  });

  it('leads the breadcrumbs from the root down to the node', () => {
    const docker = fixtureNode('Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw');
    const view = fixtureNodeView(docker.id);
    expect(view.kind).toBe('file');
    expect(view.breadcrumbs.map((c) => c.name)).toEqual([
      'demo',
      'Library',
      'Containers',
      'com.docker.docker',
      'Data',
      'vms',
      '0',
      'data',
      'Docker.raw',
    ]);
    expect(view.breadcrumbs[0].id).toBe(0);
    expect(view.breadcrumbs[view.breadcrumbs.length - 1]).toEqual({
      id: docker.id,
      name: 'Docker.raw',
    });
    expect(view.children).toEqual([]);
    expect(view.childrenTotal).toBe(0);
    expect(view.logicalSize).toBeGreaterThan(view.size);
  });

  it('truncates the children but not the total', () => {
    const view = fixtureNodeView(0, 2);
    expect(view.children).toHaveLength(2);
    expect(view.childrenTotal).toBe(fixtureNodes[0].children.length);
    expect(view.truncated).toBe(true);
    expect(fixtureNodeView(0, view.childrenTotal).truncated).toBe(false);
  });

  it('gives deltas only to paths of the previous snapshot', () => {
    const library = fixtureNode('Library');
    const view = fixtureNodeView(library.id);
    expect(view.delta).toBe(library.size - previousSizes.get(library.path)!);
    for (const child of view.children) {
      const before = previousSizes.get(`${library.path}/${child.name}`);
      expect(child.delta).toBe(before === undefined ? null : child.size - before);
    }
  });

  it('rejects an unknown id with the backend message', () => {
    expect(() => fixtureNodeView(999)).toThrow('unknown node 999');
  });
});

describe('fixtureGrowers', () => {
  it('lists growing directories, largest first, skipping parents explained by a child', () => {
    const growers = fixtureGrowers();
    expect(growers.length).toBeGreaterThanOrEqual(4);
    for (const g of growers) {
      expect(g.kind).toBe('dir');
      expect(g.delta).toBeGreaterThan(0);
      expect(g.after - g.before).toBe(g.delta);
      expect(g.after).toBe(fixtureNode(g.path).size);
    }
    const deltas = growers.map((g) => g.delta);
    expect(deltas).toEqual([...deltas].sort((a, b) => b - a));
    const paths = growers.map((g) => g.path);
    expect(paths).not.toContain(FIXTURE_ROOT);
    // The chain above DerivedData grew by the same amount: the deepest culprit is named.
    expect(paths[0]).toBe(`${FIXTURE_ROOT}/Library/Developer/Xcode/DerivedData`);
    for (const parent of ['Library', 'Library/Developer', 'Library/Developer/Xcode', 'src']) {
      expect(paths).not.toContain(`${FIXTURE_ROOT}/${parent}`);
    }
    expect(paths).toContain(`${FIXTURE_ROOT}/src/storage-monitor/target`);
    expect(paths).toContain(`${FIXTURE_ROOT}/Library/Containers/com.docker.docker/Data/vms/0/data`);
  });
});

describe('fixtureDisk and fixtureStatusDone', () => {
  it('describe a consistent volume', () => {
    const disk = fixtureDisk();
    expect(disk.path).toBe(FIXTURE_ROOT);
    expect(disk.used).toBe(disk.total - disk.free);
    expect(disk.available).toBeLessThanOrEqual(disk.free);
    expect(disk.free).toBeLessThanOrEqual(disk.total);
    expect(disk.used).toBeGreaterThan(fixtureNodes[0].size);
  });

  it('report the finished scan of the fixture', () => {
    const status = fixtureStatusDone();
    expect(status.state).toBe('done');
    expect(status.root).toBe(FIXTURE_ROOT);
    expect(status.bytes).toBe(fixtureNodes[0].size);
    expect(status.files).toBe(fixtureNodes.filter((n) => n.kind !== 'dir').length);
    expect(status.dirs).toBe(fixtureNodes.filter((n) => n.kind === 'dir').length);
    // Two unreadable directories plus the three entries of the partially read one.
    expect(status.errors).toBe(5);
    expect(status.currentPath).toBe('');
    expect(status.error).toBeNull();
    expect(status.durationMs).toBeGreaterThan(0);
    expect(status.hasPrevious).toBe(true);
    expect(status.previousTakenAt).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
  });
});

/** An absolute path under the fixture root, for entries the fixture does not have. */
const at = (relative: string) => `${FIXTURE_ROOT}/${relative}`;

/** The window after a finished scan of the fixture: a root and the tree that came with it. */
const scanned = (root: string = FIXTURE_ROOT): HeldScan => ({ held: 'tree', root });
/** A scan of the fixture root in flight: the root is set and the tree is gone. */
const scanning = (root: string = FIXTURE_ROOT): HeldScan => ({ held: 'root', root });
const unscanned: HeldScan = { held: 'nothing' };

const preview = (paths: string[], scan: HeldScan = scanned()) =>
  mockActionPreview(paths, 'trash', scan);

const run = (paths: string[], mode: Mode = 'trash', scan: HeldScan = scanned()) =>
  mockActionRun(paths, mode, scan);

const blocked = (reason: BlockReason) => ({ state: 'blocked', reason });

const statuses = (entries: PreviewEntry[]) => entries.map((entry) => entry.status);

describe('mockActionPreview', () => {
  it('reports a path of the tree as ready, with the kind and size the tree has', () => {
    const xcode = fixtureNode('Downloads/Xcode_16.4.xip');
    const result = preview([xcode.path]);
    expect(result).toEqual({
      entries: [{ path: xcode.path, kind: 'file', size: xcode.size, status: { state: 'ready' } }],
      totalBytes: xcode.size,
      mode: 'trash',
    });
    expect(result.totalBytes).toBe(14_810_000_000);
  });

  it('carries the mode it was asked about', () => {
    expect(mockActionPreview([], 'permanent', scanned()).mode).toBe('permanent');
    expect(mockActionPreview([], 'trash', scanned())).toEqual({
      entries: [],
      totalBytes: 0,
      mode: 'trash',
    });
  });

  it('keeps a directory the scan could not read deletable', () => {
    const trash = fixtureNode('.Trash');
    expect(trash.error).toBe(PERMISSION_DENIED);
    expect(preview([trash.path]).entries).toEqual([
      { path: trash.path, kind: 'dir', size: 0, status: { state: 'ready' } },
    ]);
  });

  it('blocks a path outside the scan root as outsideRoots', () => {
    const result = preview(['/etc/hosts', '/Users/other/notes.txt', '/Volumes/Backup']);
    expect(statuses(result.entries)).toEqual([
      blocked('outsideRoots'),
      blocked('outsideRoots'),
      blocked('outsideRoots'),
    ]);
    expect(result.entries[0]).toEqual({
      path: '/etc/hosts',
      kind: 'other',
      size: 0,
      status: blocked('outsideRoots'),
    });
    expect(result.totalBytes).toBe(0);
  });

  it('blocks the scan root and every ancestor of it as isRoot', () => {
    const root = fixtureNodes[0];
    const result = preview([FIXTURE_ROOT, '/Users']);
    expect(statuses(result.entries)).toEqual([blocked('isRoot'), blocked('isRoot')]);
    // A blocked entry keeps the plan's claim about kind and size: nothing was re-read.
    expect(result.entries[0]).toEqual({
      path: FIXTURE_ROOT,
      kind: 'dir',
      size: root.size,
      status: blocked('isRoot'),
    });
  });

  it('blocks a path with no last component as malformed', () => {
    const result = preview(['/', '', at('Movies/..'), at('..')]);
    expect(statuses(result.entries)).toEqual([
      blocked('malformed'),
      blocked('malformed'),
      blocked('malformed'),
      blocked('malformed'),
    ]);
  });

  it('blocks the Library folder of the scanned home, and everything under it', () => {
    const library = fixtureNode('Library');
    const derived = fixtureNode('Library/Developer/Xcode/DerivedData');
    const result = preview([library.path, derived.path]);
    expect(statuses(result.entries)).toEqual([blocked('denylisted'), blocked('denylisted')]);
    expect(result.totalBytes).toBe(0);
  });

  it('blocks a path the tree does not have as missing, with the plan empty', () => {
    const result = preview([at('nope'), 'Movies']);
    expect(result.entries).toEqual([
      { path: at('nope'), kind: 'other', size: 0, status: blocked('missing') },
      // No working directory in the mock, so the parent of a relative path never resolves.
      { path: 'Movies', kind: 'other', size: 0, status: blocked('missing') },
    ]);
  });

  it('normalizes the path of an entry the guards let through, and only that one', () => {
    const result = preview([at('Downloads/../nope'), at('Library/../Library')]);
    expect(result.entries[0]).toEqual({
      path: at('nope'),
      kind: 'other',
      size: 0,
      status: blocked('missing'),
    });
    // Refused by the guards, so there is no normalized form to show: the path as asked for.
    expect(result.entries[1]).toEqual({
      path: at('Library/../Library'),
      kind: 'other',
      size: 0,
      status: blocked('denylisted'),
    });
  });

  it('resolves a spelling the tree does not know, keeping the size the plan could not find', () => {
    const movies = fixtureNode('Movies');
    expect(preview([at('Downloads/../Movies')]).entries).toEqual([
      { path: movies.path, kind: 'dir', size: 0, status: { state: 'ready' } },
    ]);
  });

  it('blocks an entry another one of the batch contains, whichever order they arrive in', () => {
    const movies = fixtureNode('Movies');
    const film = fixtureNode('Movies/family-2025.mov');
    expect(statuses(preview([movies.path, film.path]).entries)).toEqual([
      { state: 'ready' },
      blocked('nested'),
    ]);
    expect(statuses(preview([film.path, movies.path]).entries)).toEqual([
      blocked('nested'),
      { state: 'ready' },
    ]);
    // Between two spellings of one entry the first survives.
    expect(statuses(preview([movies.path, at('Documents/../Movies')]).entries)).toEqual([
      { state: 'ready' },
      blocked('nested'),
    ]);
  });

  it('compares paths component by component, not as strings', () => {
    // `/Users/demo2` is not inside `/Users/demo`, and `/Users/dem` is not an ancestor of it;
    // both are string prefixes, and the same comparison decides the nesting pass.
    expect(statuses(preview(['/Users/demo2/Movies', '/Users/dem']).entries)).toEqual([
      blocked('outsideRoots'),
      blocked('outsideRoots'),
    ]);
  });

  it('keeps the denylist under a scan root of the whole volume', () => {
    // `/` is dropped from the denylist as the ancestor of everything — without that every
    // entry of every batch would be refused — and `/Users` then protects the fixture.
    expect(
      statuses(preview([fixtureNode('Movies').path, '/System/Library'], scanned('/')).entries),
    ).toEqual([blocked('denylisted'), blocked('denylisted')]);
  });

  it('totals the ready entries only', () => {
    const movies = fixtureNode('Movies');
    const film = fixtureNode('Movies/family-2025.mov');
    const library = fixtureNode('Library');
    const thesis = fixtureNode('Documents/thesis.docx');
    const result = preview([movies.path, film.path, library.path, thesis.path]);
    expect(result.totalBytes).toBe(movies.size + thesis.size);
    expect(result.entries).toHaveLength(4);
  });

  it('refuses every entry as outsideRoots when nothing has been scanned', () => {
    const movies = fixtureNode('Movies');
    const result = preview([movies.path, at('nope')], unscanned);
    // No tree, so no kind and no size for any of them: `refused_preview` copies both from a
    // plan that `with_result` never filled in.
    expect(result).toEqual({
      entries: [
        { path: movies.path, kind: 'other', size: 0, status: blocked('outsideRoots') },
        { path: at('nope'), kind: 'other', size: 0, status: blocked('outsideRoots') },
      ],
      totalBytes: 0,
      mode: 'trash',
    });
  });

  it('guards a batch that arrives while a scan runs, and promises it no bytes', () => {
    const movies = fixtureNode('Movies');
    const result = preview([movies.path, fixtureNode('Library').path, at('nope')], scanning());
    // The root outlives the tree, so the rules are the same ones; the sizes are not there to
    // be had, and a dialog in this state names no bytes it cannot account for.
    expect(result.entries).toEqual([
      { path: movies.path, kind: 'dir', size: 0, status: { state: 'ready' } },
      { path: fixtureNode('Library').path, kind: 'other', size: 0, status: blocked('denylisted') },
      { path: at('nope'), kind: 'other', size: 0, status: blocked('missing') },
    ]);
    expect(result.totalBytes).toBe(0);
    // The kind of a ready entry still comes from the disk, which no scan state hides.
    expect(result.entries[0].kind).toBe('dir');
  });

  it('blocks the fixture against a scan root of its own elsewhere', () => {
    const result = preview([fixtureNode('Movies').path], scanned('/Volumes/Backup'));
    // Blocked, and with nothing to say about the entry: the tree of another root cannot
    // contain this path, whatever the fixture happens to hold.
    expect(result.entries).toEqual([
      {
        path: fixtureNode('Movies').path,
        kind: 'other',
        size: 0,
        status: blocked('outsideRoots'),
      },
    ]);
  });

  it('reads the plan under the spellings Tree::find accepts, and no others', () => {
    const movies = fixtureNode('Movies');
    // `.` is noise to `Path::components`, so this is the same node with the same size.
    expect(preview([at('./Movies')]).entries[0]).toEqual({
      path: movies.path,
      kind: 'dir',
      size: movies.size,
      status: { state: 'ready' },
    });
    // `..` is a component of its own and matches no child: the tree has no entry so spelled,
    // and the guards still resolve it for the deletion itself.
    expect(preview([at('Downloads/../Movies')]).entries[0]).toEqual({
      path: movies.path,
      kind: 'dir',
      size: 0,
      status: { state: 'ready' },
    });
  });

  it('touches nothing: two previews of one path agree', () => {
    const film = fixtureNode('Movies/family-2025.mov');
    expect(preview([film.path])).toEqual(preview([film.path]));
    expect(mockActionLog).toEqual([]);
  });
});

describe('mockActionRun', () => {
  it('removes a file, shrinks every ancestor and leaves the siblings alone', () => {
    const root = fixtureNodes[0];
    const movies = fixtureNode('Movies');
    const film = fixtureNode('Movies/family-2025.mov');
    const [rootSize, rootFiles, moviesSize, moviesFiles] = [
      root.size,
      root.fileCount,
      movies.size,
      movies.fileCount,
    ];
    const result = run([film.path]);
    expect(result).toEqual({
      outcome: {
        entries: [
          { path: film.path, kind: 'file', result: { result: 'removed', bytes: film.size } },
        ],
        freedBytes: film.size,
        at: expect.any(String),
        mode: 'trash',
      },
      recorded: true,
      treeStale: false,
    });
    expect(movies.size).toBe(moviesSize - film.size);
    expect(movies.fileCount).toBe(moviesFiles - 1);
    expect(root.size).toBe(rootSize - film.size);
    expect(root.fileCount).toBe(rootFiles - 1);
    expect(() => fixtureNode('Movies/family-2025.mov')).toThrow(
      'unknown fixture path /Users/demo/Movies/family-2025.mov',
    );
    expect(fixtureNodeView(movies.id).children.map((child) => child.name)).toEqual([
      'screen-recording.mp4',
    ]);
    expect(fixtureNodeView(movies.id).childrenTotal).toBe(1);
    expect(() => fixtureNodeView(film.id)).toThrow(`unknown node ${film.id}`);
  });

  it('takes the whole subtree with a directory', () => {
    const documents = fixtureNode('Documents');
    const design = fixtureNode('Documents/Design');
    const psd = fixtureNode('Documents/Design/hero-assets.psd');
    const thesis = fixtureNode('Documents/thesis.docx');
    expect(run([design.path]).outcome.freedBytes).toBe(design.size);
    expect(() => fixtureNode('Documents/Design/hero-assets.psd')).toThrow('unknown fixture path');
    expect(() => fixtureNodeView(psd.id)).toThrow(`unknown node ${psd.id}`);
    expect(documents.children).toEqual([thesis.id]);
    expect(documents.size).toBe(thesis.size);
    expect(documents.logicalSize).toBe(thesis.logicalSize);
    expect(documents.fileCount).toBe(1);
  });

  it('skips every blocked entry with the reason the preview gave, and keeps it in place', () => {
    const library = fixtureNode('Library');
    const librarySize = library.size;
    const report = fixtureNode('Downloads/q3-report.pdf');
    const result = run([library.path, at('nope'), report.path, FIXTURE_ROOT]);
    expect(result.outcome.entries).toEqual([
      { path: library.path, kind: 'dir', result: { result: 'skipped', reason: 'denylisted' } },
      { path: at('nope'), kind: 'other', result: { result: 'skipped', reason: 'missing' } },
      { path: report.path, kind: 'file', result: { result: 'removed', bytes: report.size } },
      { path: FIXTURE_ROOT, kind: 'dir', result: { result: 'skipped', reason: 'isRoot' } },
    ]);
    expect(result.outcome.freedBytes).toBe(report.size);
    expect(library.size).toBe(librarySize);
    expect(fixtureNode('Library').children.length).toBeGreaterThan(0);
  });

  it('deletes what is ready in a batch that also nests', () => {
    const movies = fixtureNode('Movies');
    const film = fixtureNode('Movies/family-2025.mov');
    const result = run([movies.path, film.path]);
    expect(result.outcome.entries.map((entry) => entry.result)).toEqual([
      { result: 'removed', bytes: movies.size },
      { result: 'skipped', reason: 'nested' },
    ]);
    expect(result.outcome.freedBytes).toBe(movies.size);
    expect(() => fixtureNode('Movies')).toThrow('unknown fixture path');
    expect(fixtureNodes[0].children).not.toContain(movies.id);
  });

  it('runs a permanent batch the same way, and says so', () => {
    const zshrc = fixtureNode('.zshrc');
    const result = run([zshrc.path], 'permanent');
    expect(result.outcome.mode).toBe('permanent');
    expect(mockActivityTail(10).entries[0].mode).toBe('permanent');
  });

  it('leaves the growers of the finished scan where they are', () => {
    const target = fixtureNode('src/storage-monitor/target');
    const before = fixtureGrowers();
    expect(before.map((grower) => grower.path)).toContain(target.path);
    run([target.path]);
    // A splice does not recompute them, and a deleted path must not throw on the way out.
    expect(fixtureGrowers()).toEqual(before);
  });

  it('does nothing at all for an empty batch', () => {
    const before = fixtureNodes[0].size;
    expect(run([])).toEqual({
      outcome: { entries: [], freedBytes: 0, at: expect.any(String), mode: 'trash' },
      recorded: true,
      treeStale: false,
    });
    expect(mockActionLog).toEqual([]);
    expect(fixtureNodes[0].size).toBe(before);
  });

  it('refuses everything and deletes nothing when nothing has been scanned', () => {
    const movies = fixtureNode('Movies');
    const before = fixtureNodes[0].size;
    const result = run([movies.path], 'trash', unscanned);
    expect(result.outcome.entries).toEqual([
      { path: movies.path, kind: 'other', result: { result: 'skipped', reason: 'outsideRoots' } },
    ]);
    expect(result.outcome.freedBytes).toBe(0);
    expect(fixtureNodes[0].size).toBe(before);
    expect(fixtureNode('Movies')).toBe(movies);
  });
});

describe('mockActivityTail', () => {
  it('is empty before anything is deleted', () => {
    expect(mockActivityTail(10)).toEqual({ entries: [], damaged: 0 });
  });

  it('records one line per entry of the batch, the last of them first', () => {
    const report = fixtureNode('Downloads/q3-report.pdf');
    const thesis = fixtureNode('Documents/thesis.docx');
    const [reportSize, thesisSize] = [report.size, thesis.size];
    const { outcome } = run([report.path, thesis.path, at('nope')]);
    const tail = mockActivityTail(10);
    expect(tail.damaged).toBe(0);
    expect(mockActionLog).toHaveLength(3);
    expect(tail.entries).toEqual([
      {
        at: expect.any(String),
        path: at('nope'),
        kind: 'other',
        mode: 'trash',
        result: 'skipped',
        detail: 'missing',
        bytes: 0,
      },
      {
        at: expect.any(String),
        path: thesis.path,
        kind: 'file',
        mode: 'trash',
        result: 'removed',
        detail: null,
        bytes: thesisSize,
      },
      {
        at: expect.any(String),
        path: report.path,
        kind: 'file',
        mode: 'trash',
        result: 'removed',
        detail: null,
        bytes: reportSize,
      },
    ]);
    // One instant for the whole batch — the outcome's own — in the form the backend writes.
    expect(tail.entries.map((entry) => entry.at)).toEqual([outcome.at, outcome.at, outcome.at]);
    expect(outcome.at).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/);
    expect(Number.isNaN(Date.parse(outcome.at))).toBe(false);
  });

  it('grows by one entry per entry of every batch, newest batch first', () => {
    run([fixtureNode('Downloads/q3-report.pdf').path]);
    run([fixtureNode('Documents/thesis.docx').path, at('nope')]);
    expect(mockActionLog).toHaveLength(3);
    expect(mockActivityTail(10).entries.map((entry) => entry.path)).toEqual([
      at('nope'),
      `${FIXTURE_ROOT}/Documents/thesis.docx`,
      `${FIXTURE_ROOT}/Downloads/q3-report.pdf`,
    ]);
  });

  it('honours the limit from the end of the log', () => {
    run([at('a'), at('b'), at('c')]);
    expect(mockActivityTail(2).entries.map((entry) => entry.path)).toEqual([at('c'), at('b')]);
    expect(mockActivityTail(0)).toEqual({ entries: [], damaged: 0 });
    expect(mockActivityTail(99).entries).toHaveLength(3);
  });

  it('counts a damaged line as far back as the read goes, without giving it a slot', () => {
    run([at('a'), at('b'), at('c')]);
    mockActionLog.splice(1, 0, '{ truncated');
    expect(mockActivityTail(10)).toEqual({
      entries: expect.arrayContaining([expect.objectContaining({ path: at('a') })]),
      damaged: 1,
    });
    expect(mockActivityTail(10).entries).toHaveLength(3);
    // The read stopped at the third entry, before it ever reached the torn line.
    expect(mockActivityTail(2)).toEqual({
      entries: [
        expect.objectContaining({ path: at('c') }),
        expect.objectContaining({ path: at('b') }),
      ],
      damaged: 0,
    });
  });

  it('counts a line that is JSON but not an entry, and skips a blank one', () => {
    const entry = {
      at: '2026-09-18T09:30:00Z',
      path: at('a'),
      kind: 'file',
      mode: 'trash',
      result: 'removed',
      detail: null,
      bytes: 12,
    };
    const withoutPath: Record<string, unknown> = { ...entry };
    delete withoutPath.path;
    // Every line below is that entry with a single field spoiled: a reader that stopped
    // checking any one of them would take that line for an entry of the user's history.
    // The three at the end are the ones a `u64` and a `DateTime<Utc>` refuse.
    const spoiled = [
      { ...entry, at: 12 },
      withoutPath,
      { ...entry, kind: 'folder' },
      { ...entry, mode: 'bin' },
      { ...entry, result: 'deleted' },
      { ...entry, detail: 7 },
      { ...entry, bytes: '12' },
      { ...entry, at: 'not a date' },
      { ...entry, bytes: -1 },
      { ...entry, bytes: 1.5 },
    ];
    mockActionLog.push(
      JSON.stringify(entry),
      '',
      '   ',
      '[]',
      '7',
      // Valid JSON with no fields to read at all: a reader that goes looking for them
      // without checking throws instead of counting the line.
      'null',
      '{ truncated',
      ...spoiled.map((line) => JSON.stringify(line)),
    );
    const tail = mockActivityTail(20);
    expect(tail.entries).toEqual([entry]);
    expect(tail.damaged).toBe(4 + spoiled.length);
  });

  it('reads the lines serde reads: no detail is null, and an unknown field is ignored', () => {
    // How a test of a later task writes a `failed` row by hand. `LogEntry::detail` is an
    // `Option<String>`, which serde fills in when the key is absent, so a line without it is
    // an ordinary entry in the app and must be one here.
    mockActionLog.push(
      JSON.stringify({
        at: '2026-09-18T09:30:00Z',
        path: at('a'),
        kind: 'dir',
        mode: 'permanent',
        result: 'removed',
        bytes: 12,
      }),
      JSON.stringify({
        at: '2026-09-18T09:31:00Z',
        path: at('b'),
        kind: 'file',
        mode: 'trash',
        result: 'failed',
        detail: 'cannot move to the Trash',
        bytes: 0,
        future: 'a field this version does not know',
      }),
    );
    const tail = mockActivityTail(10);
    expect(tail.damaged).toBe(0);
    expect(tail.entries).toEqual([
      {
        at: '2026-09-18T09:31:00Z',
        path: at('b'),
        kind: 'file',
        mode: 'trash',
        result: 'failed',
        detail: 'cannot move to the Trash',
        bytes: 0,
      },
      {
        at: '2026-09-18T09:30:00Z',
        path: at('a'),
        kind: 'dir',
        mode: 'permanent',
        result: 'removed',
        detail: null,
        bytes: 12,
      },
    ]);
  });
});

describe('resetMockActions', () => {
  const snapshot = () => fixtureNodes.map((node) => ({ ...node, children: [...node.children] }));

  it('restores every node of the tree and clears the log', () => {
    const before = snapshot();
    const status = fixtureStatusDone();
    run([fixtureNode('Movies').path, fixtureNode('Downloads').path, at('nope')]);
    expect(snapshot()).not.toEqual(before);
    expect(mockActionLog.length).toBeGreaterThan(0);

    resetMockActions();

    expect(snapshot()).toEqual(before);
    expect(mockActionLog).toEqual([]);
    expect(mockActivityTail(10)).toEqual({ entries: [], damaged: 0 });
    expect(fixtureStatusDone()).toEqual(status);
    expect(fixtureNodeView(fixtureNode('Movies').id).children).toHaveLength(2);
    expect(fixtureNode('Movies/family-2025.mov').path).toBe(
      `${FIXTURE_ROOT}/Movies/family-2025.mov`,
    );
  });

  it('is what the previous test left behind: the tree is whole again', () => {
    expect(fixtureNode('Movies').children).toHaveLength(2);
    expect(fixtureNodes[0].size).toBeGreaterThan(170 * GB);
    expect(mockActivityTail(10).entries).toEqual([]);
  });
});
