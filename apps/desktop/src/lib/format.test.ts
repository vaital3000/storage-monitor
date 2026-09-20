import { describe, expect, it } from 'vitest';
import {
  basename,
  countLabel,
  formatBytes,
  formatDate,
  formatDelta,
  formatDuration,
  formatPercent,
  formatStampDate,
  formatTimestamp,
  shortenPath,
} from './format';

describe('formatBytes', () => {
  it('shows bytes without a decimal', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1)).toBe('1 B');
    expect(formatBytes(999)).toBe('999 B');
  });

  it('uses 1000-based units with one decimal from KB up', () => {
    expect(formatBytes(1000)).toBe('1.0 KB');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(4_500_000_000)).toBe('4.5 GB');
    expect(formatBytes(1_200_000_000_000)).toBe('1.2 TB');
    expect(formatBytes(340_000_000)).toBe('340.0 MB');
  });

  it('steps up instead of showing 1000.0', () => {
    expect(formatBytes(999_950)).toBe('1.0 MB');
    expect(formatBytes(999_949)).toBe('999.9 KB');
  });

  it('never shows a negative or NaN size', () => {
    expect(formatBytes(-5)).toBe('0 B');
    expect(formatBytes(Number.NaN)).toBe('0 B');
  });
});

describe('formatDelta', () => {
  it('is empty for null, undefined and zero', () => {
    expect(formatDelta(null)).toBe('');
    expect(formatDelta(undefined)).toBe('');
    expect(formatDelta(0)).toBe('');
  });

  it('prefixes growth with a plus sign', () => {
    expect(formatDelta(1_200_000_000)).toBe('+1.2 GB');
    expect(formatDelta(512)).toBe('+512 B');
  });

  it('prefixes shrinkage with a real minus sign', () => {
    expect(formatDelta(-340_000_000)).toBe('−340.0 MB');
    expect(formatDelta(-1)).toBe('−1 B');
  });
});

describe('formatPercent', () => {
  it('shows one decimal of the share', () => {
    expect(formatPercent(123, 1000)).toBe('12.3%');
    expect(formatPercent(1, 1)).toBe('100.0%');
    expect(formatPercent(0, 10)).toBe('0.0%');
  });

  it('is 0% when the whole is empty or invalid', () => {
    expect(formatPercent(5, 0)).toBe('0%');
    expect(formatPercent(0, 0)).toBe('0%');
    expect(formatPercent(5, Number.NaN)).toBe('0%');
  });
});

describe('formatDate', () => {
  it('shows the local calendar date of a unix timestamp', () => {
    // Local noon, so the date is the same in every time zone.
    const noon = new Date(2026, 8, 18, 12, 0, 0);
    expect(formatDate(noon.getTime() / 1000)).toBe('2026-09-18');
    const january = new Date(2024, 0, 5, 12, 0, 0);
    expect(formatDate(january.getTime() / 1000)).toBe('2024-01-05');
  });
});

describe('shortenPath', () => {
  const path = '/Users/demo/Library/Developer/Xcode/DerivedData';

  it('returns a path that fits unchanged', () => {
    expect(shortenPath(path, path.length)).toBe(path);
    expect(shortenPath(path, 200)).toBe(path);
  });

  it('keeps whole trailing components behind an ellipsis', () => {
    expect(shortenPath(path, 24)).toBe('…/Xcode/DerivedData');
    expect(shortenPath(path, 30)).toBe('…/Developer/Xcode/DerivedData');
  });

  it('cuts the last component when even that does not fit', () => {
    expect(shortenPath(path, 8)).toBe('…vedData');
  });

  it('never returns more than max characters', () => {
    for (const max of [1, 2, 5, 10, 19, 20, 45]) {
      expect(shortenPath(path, max).length).toBeLessThanOrEqual(Math.max(max, 1));
    }
    expect(shortenPath(path, 1)).toBe('…');
  });
});

describe('basename', () => {
  it('is the last path component, with or without a trailing slash', () => {
    expect(basename('/Users/demo')).toBe('demo');
    expect(basename('/Users/demo/Library/')).toBe('Library');
    expect(basename('Application Support')).toBe('Application Support');
  });

  it('is the path itself when there is no component', () => {
    expect(basename('/')).toBe('/');
    expect(basename('')).toBe('');
  });
});

describe('formatDuration', () => {
  it('shows milliseconds under a second', () => {
    expect(formatDuration(0)).toBe('0 ms');
    expect(formatDuration(312)).toBe('312 ms');
    expect(formatDuration(999)).toBe('999 ms');
  });

  it('shows seconds with one decimal under a minute', () => {
    expect(formatDuration(1000)).toBe('1.0 s');
    expect(formatDuration(4812)).toBe('4.8 s');
    expect(formatDuration(59_949)).toBe('59.9 s');
  });

  it('shows minutes and whole seconds from a minute up, never "60.0 s"', () => {
    expect(formatDuration(59_950)).toBe('1 min 0 s');
    expect(formatDuration(60_000)).toBe('1 min 0 s');
    expect(formatDuration(754_500)).toBe('12 min 35 s');
  });
});

describe('countLabel', () => {
  it('pluralises with an s except for exactly one', () => {
    expect(countLabel(1, 'item')).toBe('1 item');
    expect(countLabel(0, 'file')).toBe('0 files');
    expect(countLabel(2, 'folder')).toBe('2 folders');
  });

  it('groups thousands', () => {
    expect(countLabel(12_345, 'read error')).toBe('12,345 read errors');
  });

  it('takes a plural for a noun that does not end in an s', () => {
    expect(countLabel(2, 'damaged entry', 'damaged entries')).toBe('2 damaged entries');
    expect(countLabel(1, 'damaged entry', 'damaged entries')).toBe('1 damaged entry');
    expect(countLabel(0, 'entry', 'entries')).toBe('0 entries');
  });
});

describe('formatTimestamp', () => {
  it('runs outside UTC, or half of what these tests check is invisible', () => {
    // Every test below is written to hold in any zone, which is exactly why none of them
    // can see a local/UTC mix-up when local *is* UTC — and a CI runner's clock is UTC.
    // `vitest.config.ts` pins a zone with an offset and a summer time; this is the
    // assertion that fails if that line goes away.
    expect(new Date(2026, 8, 18).getTimezoneOffset()).not.toBe(0);
    expect(new Date(2026, 0, 18).getTimezoneOffset()).not.toBe(0);
  });

  it('shows the local date and time of day of an RFC 3339 stamp', () => {
    // Built locally and sent as the instant it is, so the expectation holds in every
    // time zone — the same trick `formatDate`'s test uses one describe above.
    const at = new Date(2026, 8, 18, 14, 32, 5);
    expect(formatTimestamp(at.toISOString())).toBe('2026-09-18 14:32:05');
  });

  it('reads the offset the stamp carries rather than the digits in it', () => {
    // Two spellings of one instant. A formatter that took the digits as they stand would
    // answer 12:00 for the first and 09:00 for the second.
    expect(formatTimestamp('2026-09-18T12:00:00+03:00')).toBe(
      formatTimestamp('2026-09-18T09:00:00Z'),
    );
  });

  it('gives back a stamp this platform cannot read, instead of NaN', () => {
    // Two stamps the log's readers accept and this engine does not: a leap second, which
    // is a second to `chrono` while the ECMAScript grammar stops at 59, and a stamp with
    // space around it, which `chrono` and the mock's own grammar both allow. Neither is
    // written by this app — `Utc::now` cannot produce one — but a hand-edited or
    // foreign line can. The `Date.parse` assertions are what say which side moved, on
    // the day one of these fails.
    const leap = '2026-06-30T23:59:60Z';
    const spaced = ' 2026-09-18T09:15:00Z ';
    expect(Date.parse(leap)).toBeNaN();
    expect(Date.parse(spaced)).toBeNaN();
    expect(formatTimestamp(leap)).toBe(leap);
    expect(formatTimestamp(spaced)).toBe(spaced);
    expect(formatTimestamp('whenever')).toBe('whenever');
    expect(formatTimestamp('')).toBe('');
  });
});

describe('formatStampDate', () => {
  it('shows the local calendar date of a stamp, and the stamp when it cannot', () => {
    // Half an hour before local midnight, so the instant has already crossed into the
    // next day in the zone this suite is pinned to: a formatter reading the UTC clock
    // answers the 19th here, and only a local one answers the 18th.
    const at = new Date(2026, 8, 18, 23, 30, 0);
    expect(formatStampDate(at.toISOString())).toBe('2026-09-18');
    expect(formatStampDate('2026-06-30T23:59:60Z')).toBe('2026-06-30T23:59:60Z');
    expect(formatStampDate('whenever')).toBe('whenever');
  });
});
