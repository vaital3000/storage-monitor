const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;

/** `0 B`, `999 B`, `1.0 KB`, `4.5 GB`: 1000-based, one decimal from KB up. */
export function formatBytes(bytes: number): string {
  if (!(bytes >= 1000)) {
    return `${bytes > 0 ? Math.round(bytes) : 0} B`;
  }
  let value = bytes;
  let unit = 0;
  // Step up at 999.95 so that rounding never shows "1000.0".
  while (value >= 999.95 && unit < UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return `${value.toFixed(1)} ${UNITS[unit]}`;
}

/** `+1.2 GB`, `−340.0 MB` (with a real minus sign); empty for null, undefined or 0. */
export function formatDelta(delta: number | null | undefined): string {
  if (delta === null || delta === undefined || delta === 0 || Number.isNaN(delta)) {
    return '';
  }
  return `${delta > 0 ? '+' : '−'}${formatBytes(Math.abs(delta))}`;
}

/** `12.3%`; `0%` when the whole is empty. */
export function formatPercent(part: number, whole: number): string {
  if (!(whole > 0) || !Number.isFinite(part)) {
    return '0%';
  }
  return `${((part / whole) * 100).toFixed(1)}%`;
}

/** `2026-09-18`: the local calendar date of a Unix timestamp in seconds. */
export function formatDate(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/**
 * Fits a path into `max` characters by dropping leading components behind an ellipsis
 * (`…/Xcode/DerivedData`); cuts the last component when even that is too long.
 */
export function shortenPath(path: string, max: number): string {
  if (path.length <= max) {
    return path;
  }
  if (max <= 1) {
    return '…';
  }
  const parts = path.split('/');
  for (let i = 1; i < parts.length; i += 1) {
    const tail = parts.slice(i).join('/');
    if (tail.length + 2 <= max) {
      return `…/${tail}`;
    }
  }
  return `…${path.slice(-(max - 1))}`;
}
