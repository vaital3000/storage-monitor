import { describe, expect, it } from 'vitest';
import type { BlockReason, DeletionMode, PreviewEntry } from '../lib/ipc';
import {
  FIXTURE_ROOT,
  PERMISSION_DENIED,
  fixtureGrowers,
  fixtureNode,
  fixtureNodeView,
  fixtureNodes,
  fixtureStatusDone,
} from './fixtures';
import {
  checkPath,
  dropNested,
  limitsFor,
  type HeldScan,
  mockActionPreview,
  mockActionRun,
  resetMockActions,
} from './actions';
import { mockActionLog, mockActivityTail } from './actionLog';
import sharedCases from '../../../../crates/core/tests/fixtures/guard-cases.json';

const GB = 1e9;

/** An absolute path under the fixture root, for entries the fixture does not have. */
const under = (relative: string) => `${FIXTURE_ROOT}/${relative}`;

/** The window after a finished scan of the fixture: a root and the tree that came with it. */
const scanned = (root: string = FIXTURE_ROOT): HeldScan => ({ held: 'tree', root });
/** A scan of the fixture root in flight: the root is set and the tree is gone. */
const scanning = (root: string = FIXTURE_ROOT): HeldScan => ({ held: 'root', root });
const unscanned: HeldScan = { held: 'nothing' };

const preview = (paths: string[], scan: HeldScan = scanned()) =>
  mockActionPreview(paths, 'trash', scan);

const run = (paths: string[], mode: DeletionMode = 'trash', scan: HeldScan = scanned()) =>
  mockActionRun(paths, mode, scan);

const blocked = (reason: BlockReason) => ({ state: 'blocked', reason });

const statuses = (entries: readonly PreviewEntry[]) => entries.map((entry) => entry.status);

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
    // The witness is `ready`, not the zero: a directory the walker could not list has no
    // size in the tree either, so that number says nothing about where the plan read it.
    expect(preview([trash.path]).entries).toEqual([
      { path: trash.path, kind: 'dir', size: trash.size, status: { state: 'ready' } },
    ]);
    expect(trash.size).toBe(0);
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
    const result = preview(['/', '', under('Movies/..'), under('..')]);
    expect(statuses(result.entries)).toEqual([
      blocked('malformed'),
      blocked('malformed'),
      blocked('malformed'),
      blocked('malformed'),
    ]);
  });

  it('shields the Library folder of the scanned home and lets what is inside it through', () => {
    const library = fixtureNode('Library');
    const derived = fixtureNode('Library/Developer/Xcode/DerivedData');
    const keychains = fixtureNode('Library/Keychains');
    const result = preview([library.path, derived.path, keychains.path]);
    // The three verdicts of ADR 0007 in one batch: the shield, what it lets past, and the
    // one name under it that is denied outright. The middle one is the positive control —
    // a shield that took its contents with it would still pass the other two.
    expect(statuses(result.entries)).toEqual([
      blocked('shielded'),
      { state: 'ready' },
      blocked('denylisted'),
    ]);
    // Ready entries only, so the shielded folder does not promise the bytes of a deletion
    // that will not happen — and the one entry that will is counted in full.
    expect(result.totalBytes).toBe(derived.size);
  });

  it('blocks a path the tree does not have as missing, with the plan empty', () => {
    const result = preview([under('nope'), 'Movies', 'Users/demo/Movies']);
    expect(result.entries).toEqual([
      { path: under('nope'), kind: 'other', size: 0, status: blocked('missing') },
      // No working directory in the mock, so the parent of a relative path never resolves.
      { path: 'Movies', kind: 'other', size: 0, status: blocked('missing') },
      // And the tree behind the plan cannot be asked about it either: `Tree::find` strips
      // the root from an absolute base, so a relative path never gets as far as a lookup —
      // however much this one looks like a row of the fixture with the slash taken off.
      { path: 'Users/demo/Movies', kind: 'other', size: 0, status: blocked('missing') },
    ]);
  });

  it('normalizes the path of an entry the guards let through, and only that one', () => {
    const result = preview([under('Downloads/../nope'), under('Library/../Library')]);
    expect(result.entries[0]).toEqual({
      path: under('nope'),
      kind: 'other',
      size: 0,
      status: blocked('missing'),
    });
    // Refused by the guards, so there is no normalized form to show: the path as asked for.
    expect(result.entries[1]).toEqual({
      path: under('Library/../Library'),
      kind: 'other',
      size: 0,
      status: blocked('shielded'),
    });
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
    expect(statuses(preview([movies.path, under('Documents/../Movies')]).entries)).toEqual([
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
    const result = preview([movies.path, under('nope')], unscanned);
    // No tree, so no kind and no size for any of them: `refused_preview` copies both from a
    // plan that `with_result` never filled in.
    expect(result).toEqual({
      entries: [
        { path: movies.path, kind: 'other', size: 0, status: blocked('outsideRoots') },
        { path: under('nope'), kind: 'other', size: 0, status: blocked('outsideRoots') },
      ],
      totalBytes: 0,
      mode: 'trash',
    });
  });

  it('guards a batch that arrives while a scan runs, and promises it no bytes', () => {
    const movies = fixtureNode('Movies');
    const result = preview([movies.path, fixtureNode('Library').path, under('nope')], scanning());
    // The root outlives the tree, so the rules are the same ones; the sizes are not there to
    // be had, and a dialog in this state names no bytes it cannot account for.
    expect(result.entries).toEqual([
      { path: movies.path, kind: 'dir', size: 0, status: { state: 'ready' } },
      { path: fixtureNode('Library').path, kind: 'other', size: 0, status: blocked('shielded') },
      { path: under('nope'), kind: 'other', size: 0, status: blocked('missing') },
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
    expect(preview([under('./Movies')]).entries[0]).toEqual({
      path: movies.path,
      kind: 'dir',
      size: movies.size,
      status: { state: 'ready' },
    });
    // `..` is a component of its own and matches no child: the tree has no entry so spelled,
    // and the guards still resolve it for the deletion itself.
    expect(preview([under('Downloads/../Movies')]).entries[0]).toEqual({
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
    // Through the path, because `movies.id` is no longer the id of this directory: the
    // batch renumbered the arena. The two assertions below are about that.
    const moved = fixtureNode('Movies');
    expect(fixtureNodeView(moved.id).children.map((child) => child.name)).toEqual([
      'screen-recording.mp4',
    ]);
    expect(fixtureNodeView(moved.id).childrenTotal).toBe(1);
    // Movies lost 18.9 GB of 22.2 and sank past four siblings, so its own id moved too.
    expect(moved.id).not.toBe(movies.id);
    // And the ids the caller was holding do not throw, which is the hazard and not an
    // oversight: the slot a deleted node had is somebody else's now, and asking again with
    // it answers about a directory nobody asked about.
    expect(fixtureNodeView(movies.id).path).not.toBe(movies.path);
    expect(fixtureNodeView(film.id).path).not.toBe(film.path);
  });

  it('takes the whole subtree with a directory', () => {
    const documents = fixtureNode('Documents');
    const design = fixtureNode('Documents/Design');
    const psd = fixtureNode('Documents/Design/hero-assets.psd');
    const thesis = fixtureNode('Documents/thesis.docx');
    expect(run([design.path]).outcome.freedBytes).toBe(design.size);
    expect(() => fixtureNode('Documents/Design/hero-assets.psd')).toThrow('unknown fixture path');
    expect(fixtureNodeView(psd.id).path).not.toBe(psd.path);
    // The captured nodes are the old arena's: the splice shrank them before the
    // renumbering copied what was left, so their sizes are right and their ids are not.
    expect(documents.children).toEqual([thesis.id]);
    expect(documents.size).toBe(thesis.size);
    expect(documents.logicalSize).toBe(thesis.logicalSize);
    expect(documents.fileCount).toBe(1);
    // Gone from the arena, not left in it as a hole: the ids that follow close up.
    expect(fixtureNodes.map((node) => node.path)).not.toContain(psd.path);
    expect(fixtureNodes.map((node) => node.id)).toEqual(fixtureNodes.map((_, id) => id));
  });

  it('skips every blocked entry with the reason the preview gave, and keeps it in place', () => {
    const library = fixtureNode('Library');
    const librarySize = library.size;
    const report = fixtureNode('Downloads/q3-report.pdf');
    const result = run([library.path, under('nope'), report.path, FIXTURE_ROOT]);
    expect(result.outcome.entries).toEqual([
      { path: library.path, kind: 'dir', result: { result: 'skipped', reason: 'shielded' } },
      { path: under('nope'), kind: 'other', result: { result: 'skipped', reason: 'missing' } },
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
    // By path: every id in the root's list is a new one, and one of them is the number
    // Movies used to have.
    expect(fixtureNodes[0].children.map((id) => fixtureNodes[id].name)).not.toContain('Movies');
  });

  it('runs a permanent batch the same way, and says so', () => {
    const zshrc = fixtureNode('.zshrc');
    const result = run([zshrc.path], 'permanent');
    expect(result.outcome.mode).toBe('permanent');
    expect(mockActivityTail(10).entries[0].mode).toBe('permanent');
  });

  it('leaves the scan its own read errors, whether or not a batch has patched the tree', () => {
    const trash = fixtureNode('.Trash');
    expect(trash.error).toBe(PERMISSION_DENIED);
    const before = fixtureStatusDone();
    run([trash.path]);
    const after = fixtureStatusDone();
    // `patched_stats` carries `stats.errors` through untouched: deleting a folder the walker
    // could not read does not unmake the moment it could not read it.
    expect(after.errors).toBe(before.errors);
    expect(after.dirs).toBe(before.dirs - 1);
    expect(after.files).toBe(before.files);
    expect(after.bytes).toBe(before.bytes);
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

describe('resetMockActions', () => {
  const snapshot = () => fixtureNodes.map((node) => ({ ...node, children: [...node.children] }));

  it('restores every node of the tree and clears the log', () => {
    const before = snapshot();
    const status = fixtureStatusDone();
    run([fixtureNode('Movies').path, fixtureNode('Downloads').path, under('nope')]);
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

  it("leaves this test's own deletions behind, deliberately, for the next one", () => {
    run([fixtureNode('Movies').path, under('nope')]);
    expect(() => fixtureNode('Movies')).toThrow('unknown fixture path');
    expect(mockActionLog).toHaveLength(2);
    // No reset here: the test below is the assertion, and what it asserts is that the
    // `afterEach` of `src/test/setup.ts` really does run this between every pair of tests.
  });

  it('runs between tests: the tree is whole again and the log is empty', () => {
    expect(fixtureNode('Movies').children).toHaveLength(2);
    expect(fixtureNodes[0].size).toBeGreaterThan(170 * GB);
    expect(mockActivityTail(10).entries).toEqual([]);
    expect(mockActionLog).toEqual([]);
  });
});

interface GuardCase {
  base: string;
  path: string;
  expect: string;
  why: string;
}

interface GuardScenario {
  name: string;
  root: string;
  home: string;
  tree: { dirs: string[]; files: string[] };
  cases: GuardCase[];
}

interface NestingCase {
  paths: string[];
  keep: boolean[];
  why: string;
}

/**
 * The cases `crates/core/src/action/guards.rs` answers as well, read from the crate rather
 * than copied: a rule that changes on one side alone reddens whichever test was not updated.
 * The file says what it covers and what `ready` means in it.
 */
const SHARED = sharedCases as unknown as {
  scenarios: GuardScenario[];
  nesting: NestingCase[];
};

const join = (base: string, relative: string) => (relative === '' ? base : `${base}/${relative}`);

describe('the guard cases shared with the Rust', () => {
  it('has scenarios to run', () => {
    expect(SHARED.scenarios.length).toBeGreaterThan(1);
  });

  for (const scenario of SHARED.scenarios) {
    it(scenario.name, () => {
      // The cases are written against a tree both sides have. If the fixture loses one of
      // these entries the cases stop meaning the same thing on the two sides, so say so
      // here rather than passing over a path that is no longer there.
      for (const dir of scenario.tree.dirs) {
        expect(fixtureNode(dir).kind, dir).toBe('dir');
      }
      for (const file of scenario.tree.files) {
        expect(fixtureNode(file).kind, file).toBe('file');
      }

      const root = join(FIXTURE_ROOT, scenario.root);
      const home = join(FIXTURE_ROOT, scenario.home);
      const parent = root.slice(0, root.lastIndexOf('/'));
      const limits = limitsFor(root, home);
      for (const shared of scenario.cases) {
        const path =
          shared.base === 'root'
            ? join(root, shared.path)
            : shared.base === 'home'
              ? join(home, shared.path)
              : shared.base === 'parent'
                ? join(parent, shared.path)
                : // The root's own path with the case appended to its last component.
                  shared.base === 'sibling'
                  ? `${root}${shared.path}`
                  : shared.path;
        const checked = checkPath(limits, path);
        const verdict = 'judged' in checked ? 'ready' : checked.reason;
        expect(verdict, `${shared.base} ${shared.path} — ${shared.why}`).toBe(shared.expect);
      }
    });
  }

  it('answers the nesting cases the same way', () => {
    expect(SHARED.nesting.length).toBeGreaterThan(0);
    for (const nesting of SHARED.nesting) {
      const paths = nesting.paths.map((path) => join(FIXTURE_ROOT, path));
      expect(dropNested(paths), `${nesting.paths.join(', ')} — ${nesting.why}`).toEqual(
        nesting.keep,
      );
    }
  });
});
