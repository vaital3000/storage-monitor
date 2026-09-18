import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { useCallback, useEffect, useState } from 'react';
import Breadcrumbs from '../components/Breadcrumbs';
import Button from '../components/Button';
import DiskUsageBar from '../components/DiskUsageBar';
import EmptyState from '../components/EmptyState';
import NodeTable from '../components/NodeTable';
import ScanProgress from '../components/ScanProgress';
import { useScan } from '../hooks/useScan';
import { formatBytes, formatDate } from '../lib/format';
import {
  defaultRoot,
  diskUsage,
  revealInFinder,
  treeNode,
  type DiskUsage,
  type NodeId,
  type ScanStatus,
} from '../lib/ipc';
import { describeNodeError } from '../lib/nodeErrors';

const ROOT_ID: NodeId = 0;

function basename(path: string): string {
  const parts = path.split('/').filter((part) => part !== '');
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes} min ${seconds} s`;
}

/** Backspace in a text field edits the text; anywhere else it goes up one directory. */
function isEditable(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable ||
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement)
  );
}

interface ResultHeaderProps {
  status: ScanStatus;
  disk: DiskUsage | undefined;
  onRescan: () => void;
}

function ResultHeader({ status, disk, onRescan }: ResultHeaderProps) {
  const root = status.root ?? '';
  const previous =
    status.hasPrevious && status.previousTakenAt !== null
      ? formatDate(Date.parse(status.previousTakenAt) / 1000)
      : null;
  return (
    <header className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
      <div className="min-w-0">
        <h2 className="truncate text-xl font-semibold" title={root}>
          {basename(root)}
        </h2>
        <p data-testid="scan-summary" className="text-sm text-neutral-500 tabular-nums">
          {formatBytes(status.bytes)} · {status.files.toLocaleString('en-US')} files ·{' '}
          {status.dirs.toLocaleString('en-US')} folders · scanned in{' '}
          {formatDuration(status.durationMs)}
          {status.errors > 0 && ` · ${status.errors.toLocaleString('en-US')} read errors`}
        </p>
        {status.state === 'cancelled' && (
          <p className="mt-1 text-sm text-amber-700 dark:text-amber-400">
            Scan cancelled: partial results.
          </p>
        )}
        {previous !== null && (
          <p className="mt-1 text-xs text-neutral-500">
            Δ compares with the snapshot of {previous}.
          </p>
        )}
      </div>
      <div className="flex items-center gap-4">
        {disk !== undefined && (
          <div className="w-80">
            <DiskUsageBar usage={disk} scanned={status.bytes} />
          </div>
        )}
        <Button onClick={onRescan}>Rescan</Button>
      </div>
    </header>
  );
}

function FailedState({ error, onRetry }: { error: string | null; onRetry: () => void }) {
  return (
    <section
      role="alert"
      className="mx-auto mt-16 w-full max-w-xl rounded-xl border border-red-200 bg-red-50 p-5 dark:border-red-900 dark:bg-red-950/40"
    >
      <h2 className="text-lg font-semibold text-red-800 dark:text-red-300">Scan failed</h2>
      <p className="mt-2 font-mono text-sm break-words text-red-700 dark:text-red-300">
        {error ?? 'Unknown error'}
      </p>
      <Button className="mt-4" onClick={onRetry}>
        Retry
      </Button>
    </section>
  );
}

export default function ExplorerPage() {
  const scan = useScan();
  const { status, generation, hasResult } = scan;

  // The node on screen, valid for one generation: ids change with every scan, so a new
  // tree opens at its root.
  const [nav, setNav] = useState({ generation, id: ROOT_ID });
  const currentId = nav.generation === generation ? nav.id : ROOT_ID;
  const open = useCallback((id: NodeId) => setNav({ generation, id }), [generation]);

  const root = useQuery({ queryKey: ['defaultRoot'], queryFn: defaultRoot, staleTime: Infinity });
  // `tree_node` rejects with "no scan result" until a scan is done or cancelled.
  const node = useQuery({
    queryKey: ['treeNode', generation, currentId],
    queryFn: () => treeNode(currentId),
    enabled: hasResult,
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });
  const disk = useQuery({
    queryKey: ['diskUsage', generation, status.root],
    queryFn: () => diskUsage(status.root ?? undefined),
    enabled: hasResult,
    staleTime: Infinity,
  });

  const view = node.data;
  const parentId =
    view !== undefined && view.breadcrumbs.length > 1
      ? view.breadcrumbs[view.breadcrumbs.length - 2].id
      : null;

  useEffect(() => {
    if (parentId === null) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Backspace' || isEditable(event.target)) return;
      event.preventDefault();
      open(parentId);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [parentId, open]);

  const reveal = useCallback((path: string) => {
    revealInFinder(path).catch((e: unknown) => {
      console.error(`Reveal in Finder failed for ${path}: ${String(e)}`);
    });
  }, []);

  if (!scan.ready) {
    return <p className="p-6 text-sm text-neutral-500">Loading…</p>;
  }
  if (status.state === 'idle') {
    return <EmptyState root={root.data} onScan={() => void scan.start()} />;
  }
  if (status.state === 'running') {
    return (
      <ScanProgress
        status={status}
        cancelling={scan.cancelling}
        onCancel={() => void scan.cancel()}
      />
    );
  }
  if (status.state === 'failed') {
    return (
      <FailedState error={status.error} onRetry={() => void scan.start(status.root ?? undefined)} />
    );
  }

  return (
    <div className="flex flex-col gap-4 p-5">
      <ResultHeader
        status={status}
        disk={disk.data}
        onRescan={() => void scan.start(status.root ?? undefined)}
      />
      {view !== undefined ? (
        <>
          <Breadcrumbs crumbs={view.breadcrumbs} onSelect={open} />
          {view.error !== null && view.children.length > 0 && (
            <p
              role="note"
              className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-300"
            >
              {describeNodeError(view.error).title}
            </p>
          )}
          <NodeTable node={view} onOpen={open} onReveal={reveal} />
        </>
      ) : node.isError ? (
        <div role="alert" className="flex items-center gap-4 text-sm">
          <p className="font-mono text-red-700 dark:text-red-300">{String(node.error)}</p>
          <Button onClick={() => open(ROOT_ID)}>Back to the top</Button>
        </div>
      ) : (
        <p className="text-sm text-neutral-500">Loading…</p>
      )}
    </div>
  );
}
