import { describe, expect, it } from 'vitest';
import type { Item } from './ipc';
import { choiceFor, defaultChoice, prune, requestsOf, selectedBytes } from './selection';

function item(name: string, bytes: number, defaults: string[] = []): Item {
  return {
    id: `demo:${name}`,
    module: 'demo',
    kind: 'folder',
    title: name,
    subtitle: null,
    path: `/x/${name}`,
    size: { bytes, estimated: false },
    lastUsed: null,
    verdict: { level: 'safe', reasons: [] },
    facts: [],
    actions: [
      {
        id: 'delete',
        label: 'Delete folder',
        estimatedFree: bytes,
        options: [
          { id: 'force', label: 'Force', default: false, force: true },
          ...defaults.map((id) => ({ id, label: id, default: true, force: false })),
        ],
      },
      { id: 'archive', label: 'Archive', estimatedFree: bytes / 2, options: [] },
    ],
  };
}

describe('the cleanup selection', () => {
  const a = item('a', 10, ['branch']);
  const b = item('b', 4);

  it('asks for the first action with the options that start turned on', () => {
    expect(defaultChoice(a)).toEqual({ action: 'delete', options: ['branch'] });
    expect(defaultChoice(b)).toEqual({ action: 'delete', options: [] });
  });

  it('prefers what the detail panel chose', () => {
    const choices = new Map([['demo:b', { action: 'archive', options: [] }]]);
    expect(choiceFor(b, choices)).toEqual({ action: 'archive', options: [] });
    expect(choiceFor(a, choices)).toEqual(defaultChoice(a));
  });

  it('drops the ticks a filter hides', () => {
    expect([...prune(new Set(['demo:a', 'demo:b']), [b])]).toEqual(['demo:b']);
  });

  it('builds the requests in the order the rows are shown', () => {
    const choices = new Map([['demo:a', { action: 'delete', options: ['force'] }]]);
    expect(requestsOf([b, a], new Set(['demo:a', 'demo:b']), choices)).toEqual([
      { item: 'demo:b', action: 'delete', options: [] },
      { item: 'demo:a', action: 'delete', options: ['force'] },
    ]);
  });

  it('sums what the chosen actions promise, for the ticked rows only', () => {
    const choices = new Map([['demo:a', { action: 'archive', options: [] }]]);
    expect(selectedBytes([a, b], new Set(['demo:a']), choices)).toBe(5);
    expect(selectedBytes([a, b], new Set(['demo:a', 'demo:b']), new Map())).toBe(14);
  });
});
