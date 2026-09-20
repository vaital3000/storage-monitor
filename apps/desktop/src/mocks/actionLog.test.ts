import { describe, expect, it } from 'vitest';
import type { DeletionMode } from '../lib/ipc';
import { FIXTURE_ROOT, fixtureNode } from './fixtures';
import { type HeldScan, mockActionRun } from './actions';
import { appendToLog, mockActionLog, mockActivityTail } from './actionLog';

/** An absolute path under the fixture root, for entries the fixture does not have. */
const under = (relative: string) => `${FIXTURE_ROOT}/${relative}`;

const scanned = (root: string = FIXTURE_ROOT): HeldScan => ({ held: 'tree', root });

const run = (paths: string[], mode: DeletionMode = 'trash', scan: HeldScan = scanned()) =>
  mockActionRun(paths, mode, scan);

describe('mockActivityTail', () => {
  it('is empty before anything is deleted', () => {
    expect(mockActivityTail(10)).toEqual({ entries: [], damaged: 0 });
  });

  it('records one line per entry of the batch, the last of them first', () => {
    const report = fixtureNode('Downloads/q3-report.pdf');
    const thesis = fixtureNode('Documents/thesis.docx');
    const [reportSize, thesisSize] = [report.size, thesis.size];
    const { outcome } = run([report.path, thesis.path, under('nope')]);
    const tail = mockActivityTail(10);
    expect(tail.damaged).toBe(0);
    expect(mockActionLog).toHaveLength(3);
    expect(tail.entries).toEqual([
      {
        at: expect.any(String),
        path: under('nope'),
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

  it('splits a failure into its message and no bytes, which no batch of this mock can', () => {
    // `mockActionRun` cannot produce `failed` — the fixture has no disk to refuse — so
    // this arm of `logEntry` is reached by nothing else in the suite, and the Activity
    // screen's failure rows are all hand-written lines. The mapping is still the mock's
    // half of `LogEntry::of`: the message goes to `detail`, and `bytes` is 0 because
    // nothing was freed.
    appendToLog({
      entries: [
        {
          path: under('locked'),
          kind: 'dir',
          result: { result: 'failed', message: 'cannot delete /Users/demo/locked: denied' },
        },
      ],
      freedBytes: 0,
      at: '2026-09-18T09:15:00Z',
      mode: 'permanent',
    });
    expect(mockActivityTail(10)).toEqual({
      entries: [
        {
          at: '2026-09-18T09:15:00Z',
          path: under('locked'),
          kind: 'dir',
          mode: 'permanent',
          result: 'failed',
          detail: 'cannot delete /Users/demo/locked: denied',
          bytes: 0,
        },
      ],
      damaged: 0,
    });
  });

  it('grows by one entry per entry of every batch, newest batch first', () => {
    run([fixtureNode('Downloads/q3-report.pdf').path]);
    run([fixtureNode('Documents/thesis.docx').path, under('nope')]);
    expect(mockActionLog).toHaveLength(3);
    expect(mockActivityTail(10).entries.map((entry) => entry.path)).toEqual([
      under('nope'),
      `${FIXTURE_ROOT}/Documents/thesis.docx`,
      `${FIXTURE_ROOT}/Downloads/q3-report.pdf`,
    ]);
  });

  it('honours the limit from the end of the log', () => {
    run([under('a'), under('b'), under('c')]);
    expect(mockActivityTail(2).entries.map((entry) => entry.path)).toEqual([
      under('c'),
      under('b'),
    ]);
    expect(mockActivityTail(0)).toEqual({ entries: [], damaged: 0 });
    expect(mockActivityTail(99).entries).toHaveLength(3);
  });

  it('counts a damaged line as far back as the read goes, without giving it a slot', () => {
    run([under('a'), under('b'), under('c')]);
    mockActionLog.splice(1, 0, '{ truncated');
    expect(mockActivityTail(10)).toEqual({
      entries: expect.arrayContaining([expect.objectContaining({ path: under('a') })]),
      damaged: 1,
    });
    expect(mockActivityTail(10).entries).toHaveLength(3);
    // The read stopped at the third entry, before it ever reached the torn line.
    expect(mockActivityTail(2)).toEqual({
      entries: [
        expect.objectContaining({ path: under('c') }),
        expect.objectContaining({ path: under('b') }),
      ],
      damaged: 0,
    });
  });

  it('counts a line that is JSON but not an entry, and skips a blank one', () => {
    const entry = {
      at: '2026-09-18T09:30:00Z',
      path: under('a'),
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
      // `chrono` needs an offset, and a time with its seconds; `Date.parse` takes all three.
      { ...entry, at: '2026-09-18T09:30:00' },
      { ...entry, at: '2026-09-18 09:30:00' },
      { ...entry, at: '2026-09-18' },
      { ...entry, at: '2026-09-18T09:30Z' },
      { ...entry, at: '2026-09-31T09:30:00Z' },
      { ...entry, at: '2026-13-18T09:30:00Z' },
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

  it('reads every timestamp chrono reads, and only those', () => {
    // Measured against the real `LogEntry`: a space for the `T`, a lower-case `z`, an offset
    // with or without its colon, any number of fractional digits, a leap second and a
    // trailing space are all lines the app reads — two of which `Date.parse` refuses.
    const stamps = [
      '2026-09-18T09:30:00Z',
      '2026-09-18 09:30:00Z',
      '2026-09-18t09:30:00z',
      '2026-09-18T09:30:00+02:00',
      '2026-09-18T09:30:00-07:00',
      '2026-09-18T09:30:00+0200',
      '2026-09-18T09:30:00.123Z',
      '2026-09-18T09:30:00.123456789Z',
      '2026-09-18T09:30:00Z ',
      '2026-09-18T09:30:60Z',
      '2028-02-29T00:00:00Z',
    ];
    for (const stamp of stamps) {
      mockActionLog.push(
        JSON.stringify({
          at: stamp,
          path: under('a'),
          kind: 'file',
          mode: 'trash',
          result: 'removed',
          detail: null,
          bytes: 1,
        }),
      );
    }
    const tail = mockActivityTail(stamps.length);
    expect(tail.damaged).toBe(0);
    expect(tail.entries.map((line) => line.at)).toEqual([...stamps].reverse());
  });

  it('reads the lines serde reads: no detail is null, and an unknown field is ignored', () => {
    // How a test of a later task writes a `failed` row by hand. `LogEntry::detail` is an
    // `Option<String>`, which serde fills in when the key is absent, so a line without it is
    // an ordinary entry in the app and must be one here.
    mockActionLog.push(
      JSON.stringify({
        at: '2026-09-18T09:30:00Z',
        path: under('a'),
        kind: 'dir',
        mode: 'permanent',
        result: 'removed',
        bytes: 12,
      }),
      JSON.stringify({
        at: '2026-09-18T09:31:00Z',
        path: under('b'),
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
        path: under('b'),
        kind: 'file',
        mode: 'trash',
        result: 'failed',
        detail: 'cannot move to the Trash',
        bytes: 0,
      },
      {
        at: '2026-09-18T09:30:00Z',
        path: under('a'),
        kind: 'dir',
        mode: 'permanent',
        result: 'removed',
        detail: null,
        bytes: 12,
      },
    ]);
  });
});
