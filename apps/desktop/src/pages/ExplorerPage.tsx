import { useQuery } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState } from 'react';
import Breadcrumbs from '../components/Breadcrumbs';
import Button from '../components/Button';
import DiskUsageBar from '../components/DiskUsageBar';
import EmptyState from '../components/EmptyState';
import NodeTable from '../components/NodeTable';
import ScanProgress from '../components/ScanProgress';
import Treemap from '../components/Treemap';
import { useScan } from '../hooks/useScan';
import { basename, countLabel, formatBytes, formatDate, formatDuration } from '../lib/format';
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

/**
 * Tells whether the last input came from the keyboard. A navigation made by key moves the
 * focus to the first row of the new directory; a click leaves it where the pointer put it.
 */
function useKeyboardInput(): () => boolean {
  const keyboard = useRef(false);
  useEffect(() => {
    const onKeyDown = () => {
      keyboard.current = true;
    };
    const onPointerDown = () => {
      keyboard.current = false;
    };
    // Captured, so that a handler which stops propagation cannot hide the input.
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('pointerdown', onPointerDown, true);
    return () => {
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('pointerdown', onPointerDown, true);
    };
  }, []);
  return useCallback(() => keyboard.current, []);
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
        <p data-testid="scan-summary" className="text-sm text-muted tabular-nums">
          {formatBytes(status.bytes)} · {countLabel(status.files, 'file')} ·{' '}
          {countLabel(status.dirs, 'folder')} · scanned in {formatDuration(status.durationMs)}
          {status.errors > 0 && ` · ${countLabel(status.errors, 'read error')}`}
        </p>
        {status.state === 'cancelled' && (
          <p className="mt-1 text-sm text-amber-700 dark:text-amber-400">
            Scan cancelled: partial results.
          </p>
        )}
        {previous !== null && (
          <p className="mt-1 text-xs text-muted">Δ compares with the snapshot of {previous}.</p>
        )}
      </div>
      <div className="flex items-center gap-4">
        {disk !== undefined && (
          <div className="w-80 max-w-full">
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
  const isKeyboard = useKeyboardInput();

  // The node on screen, valid for one generation: ids change with every scan, so a new
  // tree opens at its root. `focus` remembers whether the keyboard brought us there.
  const [nav, setNav] = useState({ generation, id: ROOT_ID, focus: false });
  const current = nav.generation === generation ? nav : { id: ROOT_ID, focus: false };
  const open = useCallback(
    (id: NodeId) => setNav({ generation, id, focus: isKeyboard() }),
    [generation, isKeyboard],
  );

  const root = useQuery({ queryKey: ['defaultRoot'], queryFn: defaultRoot, staleTime: Infinity });
  // `tree_node` rejects with "no scan result" until a scan is done or cancelled.
  const node = useQuery({
    queryKey: ['treeNode', generation, current.id],
    queryFn: () => treeNode(current.id),
    enabled: hasResult,
    staleTime: Infinity,
    // The node on screen stays while the next one loads, but only within one scan: a
    // rescan opens on a blank root rather than on the tree it replaces.
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey[1] === generation ? previous : undefined,
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
      // With a modifier, Backspace belongs to the system (⌘⌫ moves to the Trash).
      if (event.key !== 'Backspace' || event.metaKey || event.ctrlKey || event.altKey) return;
      if (isEditable(event.target)) return;
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
    return <p className="p-6 text-sm text-muted">Loading…</p>;
  }
  if (status.state === 'idle') {
    return (
      <EmptyState
        root={root.data}
        error={root.error === null ? undefined : String(root.error)}
        onScan={() => void scan.start()}
      />
    );
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
          <Treemap items={view.children} parentSize={view.size} onSelect={open} />
          <NodeTable node={view} focusFirstRow={current.focus} onOpen={open} onReveal={reveal} />
        </>
      ) : node.isError ? (
        <div role="alert" className="flex items-center gap-4 text-sm">
          <p className="font-mono text-red-700 dark:text-red-300">{String(node.error)}</p>
          <Button onClick={() => open(ROOT_ID)}>Back to the top</Button>
        </div>
      ) : (
        <p className="text-sm text-muted">Loading…</p>
      )}
    </div>
  );
}
