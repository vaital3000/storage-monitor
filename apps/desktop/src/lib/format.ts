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

const pad = (n: number) => String(n).padStart(2, '0');

/** `2026-09-18`: the local calendar date of a Unix timestamp in seconds. */
export function formatDate(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/**
 * `2026-09-18 14:32:05`: the local date and time of day of an RFC 3339 stamp — and the
 * stamp itself when this platform cannot read it.
 *
 * Seconds, because the caller is the action log, where every line of one batch carries the
 * instant the batch began: without them two batches a few seconds apart read as one.
 *
 * The fallback is not defensive decoration. `at` is written by `chrono`, whose grammar is
 * wider than the one `Date.parse` is required to accept — a leap second (`23:59:60Z`) is a
 * stamp `chrono` writes, the mock's reader takes and this engine answers `NaN` for. A
 * record of deletions may not print `NaN-NaN-NaN` over a line it is holding, so an
 * unreadable stamp is shown as it was written.
 */
export function formatTimestamp(rfc3339: string): string {
  const ms = Date.parse(rfc3339);
  if (Number.isNaN(ms)) {
    return rfc3339;
  }
  const date = new Date(ms);
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
  return `${formatDate(ms / 1000)} ${time}`;
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

/** The last component of a path (`demo` for `/Users/demo/`); the path itself when it has none. */
export function basename(path: string): string {
  const parts = path.split('/').filter((part) => part !== '');
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/** `312 ms`, `4.8 s`, `12 min 35 s`: a duration in milliseconds. */
export function formatDuration(ms: number): string {
  if (ms < 1000) {
    return `${Math.max(0, Math.round(ms))} ms`;
  }
  // Rounded to tenths first, so that 59.96 s becomes "1 min 0 s" and never "60.0 s".
  const tenths = Math.round(ms / 100);
  if (tenths < 600) {
    return `${(tenths / 10).toFixed(1)} s`;
  }
  const seconds = Math.round(ms / 1000);
  return `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/**
 * `1 item`, `12,345 read errors`, `2 damaged entries`: a count with its noun.
 *
 * The plural defaults to the singular with an `s`, which is every noun this app counts but
 * one — so a caller whose noun ends in a `y` hands over its own rather than settling for
 * "entrys".
 */
export function countLabel(count: number, singular: string, plural = `${singular}s`): string {
  return `${count.toLocaleString('en-US')} ${count === 1 ? singular : plural}`;
}
