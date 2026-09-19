import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState } from 'react';
import Breadcrumbs from '../components/Breadcrumbs';
import Button from '../components/Button';
import ConfirmDeleteDialog, { type BatchStatus } from '../components/ConfirmDeleteDialog';
import DiskUsageBar from '../components/DiskUsageBar';
import EmptyState from '../components/EmptyState';
import NodeTable from '../components/NodeTable';
import ScanProgress from '../components/ScanProgress';
import Treemap from '../components/Treemap';
import { useScan } from '../hooks/useScan';
import { basename, countLabel, formatBytes, formatDate, formatDuration } from '../lib/format';
import {
  actionPreview,
  actionRun,
  defaultRoot,
  diskUsage,
  revealInFinder,
  treeNode,
  type BatchResult,
  type DeletionMode,
  type DiskUsage,
  type NodeId,
  type Preview,
  type ScanStatus,
} from '../lib/ipc';
import { describeNodeError } from '../lib/nodeErrors';

const ROOT_ID: NodeId = 0;

/** Nothing ticked; one value, so that every render with an empty selection shares it. */
const NO_SELECTION: ReadonlySet<NodeId> = new Set();

/**
 * The ticked rows, and the tree and directory they were ticked in.
 *
 * A `NodeId` only means something inside the arena that handed it out, and both a rescan and
 * the splice after a batch replace every id in it. Carrying the scope with the ids, and
 * comparing it while rendering, is what makes the selection die with them: there is no
 * effect to forget, and no order of updates in which a stale id reaches a batch.
 */
interface Selection {
  generation: number;
  /** The directory the rows belong to; ids of another directory are not these rows. */
  node: NodeId;
  ids: ReadonlySet<NodeId>;
}

/** The tree and the directory a piece of this page's state belongs to. */
interface Scope {
  generation: number;
  node: NodeId;
}

/**
 * How far this page has got with a deletion: one at a time, from the click to the report.
 *
 * `previewing` and `previewFailed` are this page's alone. `ConfirmDeleteDialog` takes a
 * resolved preview and renders no pending state, so the round trip before it appears — and
 * a preview that never arrives — have to be said here; a rejected `action_preview` is not a
 * `{ phase: 'failed' }` batch, which is the far more alarming sentence "the deletion ran and
 * could not finish".
 *
 * Both carry the scope they were started in, and die with it like the ticks do: a "Checking
 * what would be deleted…" over another directory's rows narrates a batch nobody asked for,
 * and a banner about rows that are gone can outlive a navigation, a rescan and the ticks it
 * names — taking Backspace with it, since a deletion on screen holds the key.
 *
 * `confirming` carries none, deliberately: it is modal, so nothing can move the scope under
 * it, and the report it ends on has to survive the one thing that does — this page sending
 * itself back to the root when the batch patched the tree.
 */
type Deletion =
  | ({ phase: 'previewing' } & Scope)
  | ({ phase: 'previewFailed'; message: string } & Scope)
  | {
      phase: 'confirming';
      /** The mode the entry point asked for; the dialog's toggle may move away from it. */
      mode: DeletionMode;
      /** Captured when the dialog opened: what the user is being asked to confirm. */
      paths: string[];
      preview: Preview;
      status: BatchStatus;
    };

function outOfScope(deletion: Deletion, scope: Scope): boolean {
  return (
    deletion.phase !== 'confirming' &&
    (deletion.generation !== scope.generation || deletion.node !== scope.node)
  );
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

/**
 * A refused `action_preview`: the guards never ran, so nothing was even proposed — which is
 * a different and much smaller thing than a batch that ran and failed, and has to read like
 * one. The ticks stay, so the two buttons beside this are a retry.
 *
 * One line, inside the bar rather than under it, because a block of its own would push the
 * table down a second time — asynchronously, while the pointer is over a row.
 */
function PreviewError({ message }: { message: string }) {
  return (
    <p
      role="alert"
      data-testid="preview-error"
      className="flex min-w-0 items-baseline gap-2 text-sm"
    >
      <span className="shrink-0 font-medium text-red-700 dark:text-red-400">
        Could not check what would be deleted
      </span>
      <span className="truncate font-mono text-xs text-red-700 dark:text-red-300" title={message}>
        {message}
      </span>
      <span className="shrink-0 text-muted">Nothing was deleted.</span>
    </p>
  );
}

/**
 * What the ticked rows are and what may be done to them. Two entry points, because a button
 * that opened a dialog headed "Move 2 items to the Trash?" would be asking about something
 * else; the safety gate does not move with them — a permanent deletion still waits for the
 * acknowledgement inside the dialog, so the danger button opens a dialog armed for nothing.
 *
 * It keeps its place in the layout when it has nothing to say. Measured at 1280 px, showing
 * it moves everything below by 61.4 px against a row of 34.5 px: the first tick would slide
 * the next row the user is aiming at almost two rows up, on the one screen in this app that
 * deletes things. `invisible` also takes the buttons out of the tab order, and `aria-hidden`
 * out of the accessibility tree, so a bar nobody can see is not a trap either.
 */
function SelectionBar({
  summary,
  shown,
  busy,
  error,
  onDelete,
  onDismiss,
}: {
  summary: string;
  shown: boolean;
  busy: boolean;
  error: string | null;
  onDelete: (mode: DeletionMode) => void;
  onDismiss: () => void;
}) {
  return (
    <div
      data-testid="selection-bar"
      role="toolbar"
      aria-label="Selection"
      aria-hidden={shown ? undefined : true}
      className={`flex items-center gap-3 rounded-lg border border-neutral-200 bg-white px-3 py-2 dark:border-neutral-800 dark:bg-neutral-900 ${
        shown ? '' : 'invisible'
      }`}
    >
      {error === null ? (
        // The same words the live region above announces, so they are not read twice.
        <p aria-hidden="true" className="truncate text-sm font-medium tabular-nums">
          {summary}
        </p>
      ) : (
        <PreviewError message={error} />
      )}
      <div className="ml-auto flex shrink-0 gap-2">
        {error !== null && (
          <Button disabled={!shown} onClick={onDismiss}>
            Dismiss
          </Button>
        )}
        <Button disabled={busy || !shown} onClick={() => onDelete('trash')}>
          Move to Trash
        </Button>
        <Button variant="danger" disabled={busy || !shown} onClick={() => onDelete('permanent')}>
          Delete permanently
        </Button>
      </div>
    </div>
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
  const queries = useQueryClient();

  // The node on screen, valid for one generation: ids change with every scan, so a new
  // tree opens at its root. `focus` remembers whether the keyboard brought us there.
  const [nav, setNav] = useState({ generation, id: ROOT_ID, focus: false });
  const current = nav.generation === generation ? nav : { id: ROOT_ID, focus: false };
  const open = useCallback(
    (id: NodeId) => setNav({ generation, id, focus: isKeyboard() }),
    [generation, isKeyboard],
  );

  // What everything below is scoped to: one tree, one directory.
  const scope: Scope = { generation, node: current.id };

  const [picked, setPicked] = useState<Selection>({
    generation,
    node: ROOT_ID,
    ids: NO_SELECTION,
  });
  // Ticks belong to the rows they were made on, and are dropped rather than merely hidden:
  // a directory the user comes back to shows the table as they left it everywhere else, and
  // ticks that reappear are ticks nobody is looking at while a batch is being confirmed.
  //
  // Adjusted while rendering rather than in an effect (`react-hooks/set-state-in-effect`):
  // React re-runs the component with the new state before committing, so nothing with ticks
  // from elsewhere ever reaches the screen. Reading it through `stale` below is belt and
  // braces on top of that, and invisible to any test — the render it corrects is the one
  // React throws away. It is here for the day someone moves the adjustment into an effect,
  // which is where this started and where a committed stale selection would come back.
  const stale = picked.generation !== generation || picked.node !== current.id;
  if (stale && picked.ids.size > 0) {
    setPicked({ generation, node: current.id, ids: NO_SELECTION });
  }
  const selection = stale ? NO_SELECTION : picked.ids;
  // Not memoized: `NodeTable` reads this through a ref of its own and hands its rows one
  // stable handler, so a new identity per render costs nothing and a `useCallback` here
  // only gives the React compiler a dependency list to disagree with.
  const select = (ids: ReadonlySet<NodeId>) => setPicked({ ...scope, ids });

  const [deletion, setDeletion] = useState<Deletion | null>(null);
  // One call of a deletion in flight, and a ref because that has to be true before React
  // re-renders: two clicks in one task both read the same `deletion` from their render, and
  // two overlapping `action_run` calls resolve their ids against one generation — whichever
  // splices second is dropped, and its rows stay in the tree until the next scan.
  const calling = useRef(false);
  // Which preview a reply may still open a dialog for. Bumped whenever the scope moves, so
  // a reply that was in flight across a navigation finds itself stale and is dropped
  // instead of opening a modal over rows the user is no longer looking at.
  const request = useRef(0);
  // The other half of the same rule, in state: the "Checking…" and the banner it may end
  // with are dropped when the scope they were started in is gone. Same shape as the ticks
  // above — adjusted while rendering, and read through `pending` in this render too.
  const orphaned = deletion !== null && outOfScope(deletion, scope);
  if (orphaned) {
    setDeletion(null);
  }
  const pending = orphaned ? null : deletion;

  // A deletion that has not become a dialog belongs where it was started. Its state is
  // dropped while rendering, below; the two refs cannot be touched there — rendering has to
  // stay pure — and they belong together anyway.
  useEffect(() => {
    request.current += 1;
    // Freeing the guard here is what keeps the next directory's buttons from doing nothing
    // at all while an abandoned preview is still on the wire. It cannot free a running
    // batch by accident: the dialog is modal, so no scope moves under one — and the only
    // move that happens during a batch is this page's own, after the outcome is in hand.
    calling.current = false;
  }, [generation, current.id]);

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

  // The dialog is modal, and this listener is on the window: without the guard, Backspace
  // would walk the Explorer up a directory behind an open dialog — taking the selection the
  // dialog is asking about with it. The preview round trip counts, and is the only part of
  // this with no backdrop of its own to stop the key. A banner does not: it is a message,
  // not a dialog, and a page that holds the keyboard until someone finds Dismiss is stuck.
  const asking = pending !== null && pending.phase !== 'previewFailed';
  useEffect(() => {
    if (parentId === null || asking) return;
    const onKeyDown = (event: KeyboardEvent) => {
      // With a modifier, Backspace belongs to the system (⌘⌫ moves to the Trash).
      if (event.key !== 'Backspace' || event.metaKey || event.ctrlKey || event.altKey) return;
      if (isEditable(event.target)) return;
      event.preventDefault();
      open(parentId);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [parentId, open, asking]);

  const reveal = useCallback((path: string) => {
    revealInFinder(path).catch((e: unknown) => {
      console.error(`Reveal in Finder failed for ${path}: ${String(e)}`);
    });
  }, []);

  // The ticked rows as the table shows them. Taken from the children on screen rather than
  // from the ids alone, so that a row this directory does not have cannot reach a batch.
  const ticked = view === undefined ? [] : view.children.filter((child) => selection.has(child.id));
  const tickedBytes = ticked.reduce((sum, child) => sum + child.size, 0);
  const checking = pending?.phase === 'previewing';
  const summary = checking
    ? 'Checking what would be deleted…'
    : `${countLabel(ticked.length, 'item')} selected · ${formatBytes(tickedBytes)}`;

  /** Asks the guards about the ticked rows, and opens the dialog on their answer. */
  const askToDelete = (mode: DeletionMode) => {
    if (view === undefined || ticked.length === 0 || calling.current) return;
    calling.current = true;
    const asked = { ...scope, token: request.current };
    setDeletion({ phase: 'previewing', ...scope });
    // `ChildView` carries no path of its own: a row is its parent's path and its name.
    const paths = ticked.map((child) => `${view.path}/${child.name}`);
    void actionPreview(paths, mode)
      .then(
        (preview) => {
          // The user left while this was on the wire. Opening the dialog now would put a
          // modal about another directory's rows in front of them, and a `previewFailed`
          // would name rows that are no longer on screen; both are the reply's to drop.
          if (request.current !== asked.token) return;
          setDeletion({ phase: 'confirming', mode, paths, preview, status: { phase: 'asking' } });
        },
        (e: unknown) => {
          if (request.current !== asked.token) return;
          setDeletion({ phase: 'previewFailed', message: String(e), ...asked });
        },
      )
      .finally(() => {
        calling.current = false;
      });
  };

  /** What a batch that ran leaves for this page to put right. */
  const afterBatch = (result: BatchResult) => {
    // Whatever it managed to delete, the question the ticks were made for has been answered.
    setPicked({ generation, node: current.id, ids: NO_SELECTION });
    // Exactly what `touched` hands to the splice in `actions.rs`: with nothing removed and
    // nothing failed there is no patch, so the arena — and every id in it — is untouched.
    const patched = result.outcome.entries.some(
      (entry) => entry.result.result === 'removed' || entry.result.result === 'failed',
    );
    if (!patched) return;
    // Nothing upstream does this. The generation the backend bumps for a splice never
    // crosses the wire — `ScanStatus` has no field for it — and `useScan().generation` is a
    // front-end counter that only `scan:done` moves. So every `['treeNode', generation, id]`
    // in the cache still holds ids from the arena the splice threw away, and the Explorer
    // would go on drawing a tree that no longer exists.
    void queries.invalidateQueries({ queryKey: ['treeNode', generation] });
    void queries.invalidateQueries({ queryKey: ['diskUsage', generation] });
    // A batch emits no event, so the header's bytes and counters have to be asked for.
    void scan.refresh();
    // And the id this page navigates by is one of the ids that moved: `install_patches`
    // rebuilds the arena, and `replace_subtrees` re-sorts every sibling group holding a node
    // whose size changed — which is every ancestor of the deletion. Refetching the old id
    // would silently open another directory. The root is the one id a splice cannot move.
    //
    // `treeStale: true` is the case this is deliberately too careful for: the splice was
    // dropped, so the old ids still mean what they did and the Explorer could have stayed
    // where it was. It is not worth telling the two apart — `treeStale` also covers a patch
    // that installed after an incomplete rescan, where the ids did move.
    setNav({ generation, id: ROOT_ID, focus: false });
  };

  /** Runs the batch the dialog is showing, in the mode its toggle now stands at. */
  const runDeletion = (mode: DeletionMode) => {
    // Two guards for one rule, because they fail in different directions: `calling` is the
    // only thing two clicks in one task can see, and the status is the only thing left if
    // the guard was freed by a scope that moved under a batch — which nothing can do today.
    if (deletion?.phase !== 'confirming' || deletion.status.phase !== 'asking') return;
    if (calling.current) return;
    calling.current = true;
    const asked = deletion;
    setDeletion({ ...asked, status: { phase: 'running' } });
    void actionRun(asked.paths, mode)
      .then(
        (result) => {
          setDeletion({ ...asked, status: { phase: 'done', result } });
          afterBatch(result);
        },
        // A rejection means the batch did not run: nothing to invalidate, and the ticks are
        // still about rows that are still there.
        (e: unknown) => setDeletion({ ...asked, status: { phase: 'failed', message: String(e) } }),
      )
      .finally(() => {
        calling.current = false;
      });
  };

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
          {/* Mounted whether or not anything is ticked, because a live region added to the
              page together with its first words is announced by no screen reader reliably.
              It is where a ticked row is heard at all: the table is not a `grid`, so a row
              cannot carry `aria-selected`, and Space toggles a box inside the focused row
              without moving the focus to it. */}
          <p role="status" data-testid="selection-status" className="sr-only">
            {ticked.length > 0 ? summary : ''}
          </p>
          <SelectionBar
            summary={summary}
            shown={ticked.length > 0}
            busy={checking}
            error={pending?.phase === 'previewFailed' ? pending.message : null}
            onDelete={askToDelete}
            onDismiss={() => setDeletion(null)}
          />
          <NodeTable
            node={view}
            focusFirstRow={current.focus}
            onOpen={open}
            onReveal={reveal}
            selection={selection}
            onSelectionChange={select}
          />
          {pending?.phase === 'confirming' && (
            <ConfirmDeleteDialog
              // A whole `Preview` where `Omit<Preview, 'mode'>` is asked for, and no cast:
              // the dialog simply cannot read the mode the guards were called with.
              preview={pending.preview}
              status={pending.status}
              initialMode={pending.mode}
              onConfirm={runDeletion}
              // "Unmount me", in every phase, and no claim about whether a batch ran — this
              // page knows that from `deletion.status`, and has already acted on it.
              onClose={() => setDeletion(null)}
            />
          )}
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
