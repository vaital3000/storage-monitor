import { describe, expect, it } from 'vitest';
import {
  FIXTURE_ROOT,
  PARTIAL_READ,
  fixtureDisk,
  fixtureGrowers,
  fixtureNode,
  fixtureNodeView,
  fixtureNodes,
  fixtureStatusDone,
  previousSizes,
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
