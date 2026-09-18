import { describe, expect, it } from 'vitest';
import { describeNodeError } from './nodeErrors';

describe('describeNodeError', () => {
  it('explains a skipped volume and a cancelled scan as information, not failures', () => {
    expect(describeNodeError('skipped: different volume')).toEqual({
      kind: 'info',
      title: 'Not scanned: different volume',
    });
    expect(describeNodeError('scan cancelled')).toEqual({
      kind: 'info',
      title: 'Not scanned: cancelled',
    });
  });

  it('gives a partially read directory a kind of its own, keeping the message', () => {
    expect(describeNodeError('3 entries could not be read')).toEqual({
      kind: 'partial',
      title: '3 entries could not be read',
    });
    expect(describeNodeError('1 entry could not be read')).toEqual({
      kind: 'partial',
      title: '1 entry could not be read',
    });
  });

  it('treats any other message as a directory that could not be read', () => {
    expect(describeNodeError('Operation not permitted (os error 1)')).toEqual({
      kind: 'lock',
      title: 'Operation not permitted (os error 1)',
    });
    expect(describeNodeError('entries could not be read').kind).toBe('lock');
    expect(describeNodeError('3 entries could not be read, sadly').kind).toBe('lock');
  });
});
