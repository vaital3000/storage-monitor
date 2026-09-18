// How the Explorer explains a node's `error`, the message the walker attached to a
// directory it did not (fully) read. Two of them are not failures at all.

/** The walker stayed on the root's volume; a mount point is reported, not entered. */
export const DIFFERENT_VOLUME_ERROR = 'skipped: different volume';
/** The scan was cancelled before this directory was read. */
export const SCAN_CANCELLED_ERROR = 'scan cancelled';

export interface NodeErrorMark {
  /** `info`: nothing is wrong, the scanner chose not to read it. `lock`: it could not. */
  kind: 'info' | 'lock';
  /** Shown as the marker's tooltip and accessible name. */
  title: string;
}

export function describeNodeError(error: string): NodeErrorMark {
  switch (error) {
    case DIFFERENT_VOLUME_ERROR:
      return { kind: 'info', title: 'Not scanned: different volume' };
    case SCAN_CANCELLED_ERROR:
      return { kind: 'info', title: 'Not scanned: cancelled' };
    default:
      return { kind: 'lock', title: error };
  }
}
