import { describe, expect, it } from 'vitest';
import { describeFact } from './facts';

describe('describeFact', () => {
  it('says each type of value the way the rest of the app says it', () => {
    expect(describeFact({ type: 'text', value: 'Demo object' })).toBe('Demo object');
    expect(describeFact({ type: 'bytes', value: 1_000_000 })).toBe('1.0 MB');
    expect(describeFact({ type: 'count', value: 12_345 })).toBe('12,345');
    expect(describeFact({ type: 'flag', value: true })).toBe('Yes');
    expect(describeFact({ type: 'flag', value: false })).toBe('No');
    expect(describeFact({ type: 'path', value: '/Users/demo/x' })).toBe('/Users/demo/x');
    // In the pinned zone of the suite (America/New_York): the local calendar date.
    expect(describeFact({ type: 'date', value: '2026-08-04T09:30:00Z' })).toBe('2026-08-04');
    expect(describeFact({ type: 'date', value: 'not a date' })).toBe('not a date');
  });
});
