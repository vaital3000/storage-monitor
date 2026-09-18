import { formatBytes } from '../lib/format';
import type { DiskUsage } from '../lib/ipc';

interface DiskUsageBarProps {
  usage: DiskUsage;
  /** Bytes of the used space that the scan accounts for; drawn as the darker segment. */
  scanned?: number;
}

function share(part: number, whole: number): number {
  return whole > 0 ? Math.min(100, Math.max(0, (part / whole) * 100)) : 0;
}

export default function DiskUsageBar({ usage, scanned }: DiskUsageBarProps) {
  const used = share(usage.used, usage.total);
  const covered = scanned === undefined ? 0 : share(Math.min(scanned, usage.used), usage.total);
  const summary = `${formatBytes(usage.used)} used of ${formatBytes(usage.total)}`;
  return (
    <div data-testid="disk-usage" className="text-xs text-neutral-500 tabular-nums">
      <div
        role="meter"
        aria-label="Disk usage"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(used)}
        aria-valuetext={summary}
        title={scanned === undefined ? summary : `${summary}; ${formatBytes(scanned)} scanned`}
        className="relative h-2 w-full overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-neutral-400 dark:bg-neutral-500"
          style={{ width: `${used}%` }}
        />
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-blue-500"
          style={{ width: `${covered}%` }}
        />
      </div>
      <p className="mt-1 flex justify-between gap-3 whitespace-nowrap">
        <span>{summary}</span>
        <span>{formatBytes(usage.available)} available</span>
      </p>
    </div>
  );
}
