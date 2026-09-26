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
import { basename, countLabel, formatBytes, formatDuration, formatStampDate } from '../lib/format';
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
import { questionsFromPreview, reportFromBatch } from '../lib/batchQuestion';
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
 * All three carry the scope they were asked in, and a question that is out of it is
 * dropped: a "Checking what would be deleted…" over another directory's rows narrates a
 * batch nobody asked for, a banner about rows that are gone outlives the ticks it names,
 * and a dialog does both at once — over a tree whose sizes and verdicts were read before
 * the rescan that replaced it. Nothing here is modal until the dialog opens, and Rescan sits
 * in the header the whole time.
 *
 * An **answer** is never dropped, which is why the rule reads `status.phase === 'asking'`
 * rather than "not confirming": once the batch is running the report is the only thing that
 * will ever say `recorded` or `treeStale`, and this page's own re-anchor moves the scope
 * out from under it every time.
 */
type Deletion = Scope &
  (
    | { phase: 'previewing' }
    | { phase: 'previewFailed'; message: string }
    | {
        phase: 'confirming';
        /** The mode the entry point asked for; the dialog's toggle may move away from it. */
        mode: DeletionMode;
        /** Captured when the dialog opened: what the user is being asked to confirm. */
        paths: string[];
        preview: Preview;
        status: BatchStatus;
      }
  );

/** A question nobody can still be asking, because the rows it is about are gone. */
function outOfScope(deletion: Deletion, scope: Scope): boolean {
  const unanswered = deletion.phase !== 'confirming' || deletion.status.phase === 'asking';
  return unanswered && (deletion.generation !== scope.generation || deletion.node !== scope.node);
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
  // Through `formatStampDate`, not `formatDate(Date.parse(at) / 1000)`: the same stamp
  // this app cannot always read, and the same reason the Activity screen gives for
  // refusing to print `NaN-NaN-NaN` over one. A snapshot is stamped by the app itself, so
  // the unreadable case is remote here — which is why it read like that for a phase.
  const previous =
    status.hasPrevious && status.previousTakenAt !== null
      ? formatStampDate(status.previousTakenAt)
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
    <p role="alert" data-testid="preview-error" className="min-w-0 text-sm">
      <span className="font-medium text-red-700 dark:text-red-400">
        Could not check what would be deleted
      </span>{' '}
      {/* Wrapped, not truncated, and not left to a `title`: at the window's own minimum
          width a truncated line showed an ellipsis and nothing else, and a tooltip is not
          reachable by keyboard or touch, not announced and not selectable for a bug
          report. What it costs is a second shift, on the path that already went wrong. */}
      <span className="font-mono text-xs break-words text-red-700 dark:text-red-300">
        {message}
      </span>{' '}
      <span className="text-muted">Nothing was deleted.</span>
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
 * out of the accessibility tree, so a bar nobody can see is not a trap either. The cost is
 * a band of that height on every Explorer screen, ticked or not; the plan records the trade.
 *
 * "Nothing to say" is not "nothing ticked": the table stays live through the round trip, so
 * the ticks can go before the refusal they caused has been read. An alert that hides itself,
 * with its own Dismiss disabled inside it, is worse than no alert at all.
 */
function SelectionBar({
  summary,
  ticked,
  busy,
  error,
  onDelete,
  onDismiss,
}: {
  summary: string;
  /** How many rows are ticked; none is what disables the two entry points. */
  ticked: number;
  busy: boolean;
  error: string | null;
  onDelete: (mode: DeletionMode) => void;
  onDismiss: () => void;
}) {
  const shown = ticked > 0 || error !== null;
  return (
    <div
      data-testid="selection-bar"
      // `group` and not `toolbar`: the APG pattern is one tab stop with arrow keys between
      // the controls, and these are three ordinary tab stops. The same argument that left
      // the table a `table` rather than a `grid` applies here, to this task's own markup.
      role="group"
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
        {error !== null && <Button onClick={onDismiss}>Dismiss</Button>}
        <Button disabled={busy || ticked === 0} onClick={() => onDelete('trash')}>
          Move to Trash
        </Button>
        <Button
          variant="danger"
          disabled={busy || ticked === 0}
          onClick={() => onDelete('permanent')}
        >
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
  // The preview in flight, by number, or null. It is the guard — one question at a time —
  // and the stamp a reply checks itself against: a reply whose number is no longer here was
  // abandoned by a scope that moved, and may not open a dialog over rows nobody is looking
  // at. A ref, because both have to be true before React re-renders: two clicks in one task
  // read the same `deletion` from the same render.
  const previews = useRef(0);
  const pendingPreview = useRef<number | null>(null);
  // The batch in flight. Kept apart from the preview on purpose: one flag for both would let
  // an abandoned preview's `finally` — which can land long after the user moved on and
  // confirmed something else — free the guard of a running batch.
  const running = useRef(false);
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
    // Both halves of abandoning it: the reply will find its number gone and drop itself,
    // and the next directory's buttons work again rather than looking idle and ignoring
    // the click. A running batch is untouched — it has a flag of its own.
    pendingPreview.current = null;
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
    if (
      view === undefined ||
      ticked.length === 0 ||
      pendingPreview.current !== null ||
      running.current
    )
      return;
    const call = (previews.current += 1);
    pendingPreview.current = call;
    const asked = scope;
    setDeletion({ phase: 'previewing', ...asked });
    // `ChildView` carries no path of its own: a row is its parent's path and its name.
    const paths = ticked.map((child) => `${view.path}/${child.name}`);
    void actionPreview(paths, mode)
      .then(
        (preview) => {
          // The scope moved while this was on the wire — a navigation, or a rescan started
          // from the header, which is reachable the whole time this is pending. Both make
          // the reply about rows nobody is looking at.
          if (pendingPreview.current !== call) return;
          setDeletion({
            phase: 'confirming',
            mode,
            paths,
            preview,
            status: { phase: 'asking' },
            ...asked,
          });
        },
        (e: unknown) => {
          if (pendingPreview.current !== call) return;
          setDeletion({ phase: 'previewFailed', message: String(e), ...asked });
        },
      )
      .finally(() => {
        // Only if it is still this call's to give back: an abandoned preview must not free
        // the question someone asked after it.
        if (pendingPreview.current === call) pendingPreview.current = null;
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
    // And deliberately not `['activity']`, which this batch has just added lines to. The
    // shell renders one page at a time, so the Activity screen is unmounted whenever this
    // code runs: the invalidation would be aimed at a query nobody is observing, and that
    // screen re-reads the log on every mount anyway (`staleTime: 0`, and the comment on
    // its `useQuery` says so from the other end). A screen that deletes *while* Activity
    // is mounted — a split view, a batch started from Cleanup in 2b — changes that, and
    // is the case to add it for.
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
    // Two guards for one rule, because they fail in different directions: the flag is the
    // only thing two clicks in one task can see, and the status is what answers a click that
    // arrives after the state moved — a stale render, a handler kept by something else.
    if (deletion?.phase !== 'confirming' || deletion.status.phase !== 'asking') return;
    if (running.current) return;
    running.current = true;
    const asked = deletion;
    setDeletion({ ...asked, status: { phase: 'running' } });
    void actionRun(asked.paths, mode)
      .then(
        (result) => {
          setDeletion({ ...asked, status: { phase: 'done', report: reportFromBatch(result) } });
          afterBatch(result);
        },
        // A rejection means the batch did not run: nothing to invalidate, and the ticks are
        // still about rows that are still there.
        (e: unknown) => setDeletion({ ...asked, status: { phase: 'failed', message: String(e) } }),
      )
      .finally(() => {
        running.current = false;
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
            ticked={ticked.length}
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
        </>
      ) : node.isError ? (
        <div role="alert" className="flex items-center gap-4 text-sm">
          <p className="font-mono text-red-700 dark:text-red-300">{String(node.error)}</p>
          <Button onClick={() => open(ROOT_ID)}>Back to the top</Button>
        </div>
      ) : (
        <p className="text-sm text-muted">Loading…</p>
      )}
      {/* Outside the branch above, because a modal is not part of the tree it was opened
          over: a rescan blanks `view` while the new root loads, and a dialog mounted in
          there would vanish mid-batch and come back a stranger — new focus, the mode reset
          to what it opened with. What ends a dialog is the scope rule and `onClose`. */}
      {pending?.phase === 'confirming' && (
        <ConfirmDeleteDialog
          // A whole `Preview` where `Omit<Preview, 'mode'>` is asked for, and no cast: the
          // question the dialog reads has the same rows in both modes, and no way to learn
          // the mode the guards happened to be called with.
          questions={questionsFromPreview(pending.preview)}
          status={pending.status}
          initialMode={pending.mode}
          onConfirm={runDeletion}
          // "Unmount me", in every phase, and no claim about whether a batch ran — this
          // page knows that from `deletion.status`, and has already acted on it.
          onClose={() => setDeletion(null)}
        />
      )}
    </div>
  );
}
