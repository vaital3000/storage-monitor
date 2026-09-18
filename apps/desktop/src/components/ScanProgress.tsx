import { formatBytes, shortenPath } from '../lib/format';
import type { ScanStatus } from '../lib/ipc';
import Button from './Button';

interface ScanProgressProps {
  status: ScanStatus;
  /** Cancel was requested; the walker stops at the next directory. */
  cancelling: boolean;
  onCancel: () => void;
}

const CURRENT_PATH_WIDTH = 72;

function Stat({ label, value, testId }: { label: string; value: string; testId?: string }) {
  return (
    <div>
      <dt className="text-xs text-neutral-500">{label}</dt>
      <dd data-testid={testId} className="text-lg font-semibold">
        {value}
      </dd>
    </div>
  );
}

/** The card shown while a scan runs: counters, the directory being read, and Cancel. */
export default function ScanProgress({ status, cancelling, onCancel }: ScanProgressProps) {
  return (
    <section className="mx-auto mt-16 w-full max-w-xl rounded-xl border border-neutral-200 bg-white p-5 shadow-sm dark:border-neutral-800 dark:bg-neutral-800/60">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          {/* The only live region: the counters and the path change too often to be read out. */}
          <p role="status" className="text-lg font-semibold">
            Scanning…
          </p>
          <p
            className="truncate font-mono text-xs text-neutral-500"
            title={status.root ?? undefined}
          >
            {status.root}
          </p>
        </div>
        <Button onClick={onCancel} disabled={cancelling}>
          {cancelling ? 'Cancelling…' : 'Cancel'}
        </Button>
      </div>
      <div
        role="progressbar"
        aria-label="Scanning"
        className="relative mt-4 h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        <div className="animate-indeterminate absolute inset-y-0 left-0 w-1/3 rounded-full bg-blue-500" />
      </div>
      <dl className="mt-4 grid grid-cols-3 gap-4 tabular-nums">
        <Stat label="Files" value={status.files.toLocaleString('en-US')} testId="scan-files" />
        <Stat label="Folders" value={status.dirs.toLocaleString('en-US')} />
        <Stat label="Scanned" value={formatBytes(status.bytes)} />
      </dl>
      <p
        data-testid="scan-current-path"
        className="mt-4 truncate font-mono text-xs text-neutral-500"
        title={status.currentPath}
      >
        {shortenPath(status.currentPath, CURRENT_PATH_WIDTH)}
      </p>
    </section>
  );
}
