import { useQueryClient } from '@tanstack/react-query';
import { useMemo, useRef, useState } from 'react';
import Button from '../components/Button';
import ConfirmDeleteDialog, { type BatchStatus } from '../components/ConfirmDeleteDialog';
import ItemDetail from '../components/ItemDetail';
import ItemTable from '../components/ItemTable';
import ModuleStrip from '../components/ModuleStrip';
import { useModules } from '../hooks/useModules';
import { questionsFromCleanup, reportFromCleanup, type BatchQuestions } from '../lib/batchQuestion';
import { countLabel, formatBytes } from '../lib/format';
import { VERDICT_LABELS } from '../lib/items';
import {
  VERDICT_LEVELS,
  cleanupPreview,
  cleanupRun,
  onCleanupProgress,
  revealInFinder,
  type CleanupRequest,
  type DeletionMode,
  type Item,
  type VerdictLevel,
} from '../lib/ipc';
import { choiceFor, prune, requestsOf, selectedBytes, type Choice } from '../lib/selection';

/** Safe and Review; Keep is what the design means by "do not offer by default". */
const DEFAULT_LEVELS: ReadonlySet<VerdictLevel> = new Set(['safe', 'review']);

const ALL_MODULES = 'all';

/**
 * How far this page has got with a batch, from the click to the report — the Explorer's
 * `Deletion` without its scope: an item id does not move when the items are found again,
 * so a question asked before a rediscovery is still a question about the same items.
 */
type Cleaning =
  | { phase: 'previewing' }
  | { phase: 'previewFailed'; message: string }
  | {
      phase: 'confirming';
      /** Captured when the dialog opened: exactly what the user is being asked about. */
      requests: CleanupRequest[];
      questions: BatchQuestions;
      status: BatchStatus;
    };

/** The items the filters let through, in the order they came: largest first. */
function filtered(
  items: readonly Item[],
  levels: ReadonlySet<VerdictLevel>,
  module: string,
): Item[] {
  return items.filter(
    (item) => levels.has(item.verdict.level) && (module === ALL_MODULES || item.module === module),
  );
}

/**
 * What the ticked rows are, and the one thing to do with them. It keeps its place in the
 * layout when it has nothing to say, for the reason the Explorer's bar does: a first tick
 * must not slide the next row up under the pointer.
 */
function CleanupBar({
  summary,
  ticked,
  busy,
  error,
  onClean,
  onDismiss,
}: {
  summary: string;
  ticked: number;
  busy: boolean;
  error: string | null;
  onClean: () => void;
  onDismiss: () => void;
}) {
  const shown = ticked > 0 || error !== null;
  return (
    <div
      data-testid="cleanup-bar"
      role="group"
      aria-label="Selection"
      aria-hidden={shown ? undefined : true}
      className={`flex items-center gap-3 rounded-lg border border-neutral-200 bg-white px-3 py-2 dark:border-neutral-800 dark:bg-neutral-900 ${
        shown ? '' : 'invisible'
      }`}
    >
      {error === null ? (
        <p aria-hidden="true" className="truncate text-sm font-medium tabular-nums">
          {summary}
        </p>
      ) : (
        <p role="alert" data-testid="preview-error" className="min-w-0 text-sm">
          <span className="font-medium text-red-700 dark:text-red-400">
            Could not check what would be cleaned
          </span>{' '}
          <span className="font-mono text-xs break-words text-red-700 dark:text-red-300">
            {error}
          </span>{' '}
          <span className="text-muted">Nothing was cleaned.</span>
        </p>
      )}
      <div className="ml-auto flex shrink-0 gap-2">
        {error !== null && <Button onClick={onDismiss}>Dismiss</Button>}
        <Button variant="primary" disabled={busy || ticked === 0} onClick={onClean}>
          Clean…
        </Button>
      </div>
    </div>
  );
}

/**
 * What the modules found, across all of them: filters by verdict and by module, a table to
 * tick from, a detail panel for the row in focus, and batches through the one confirmation
 * dialog of the app (phase 2b design, section 11).
 */
export default function CleanupPage() {
  const client = useQueryClient();
  const { modules, items, refresh } = useModules();
  const [levels, setLevels] = useState<ReadonlySet<VerdictLevel>>(DEFAULT_LEVELS);
  const [module, setModule] = useState<string>(ALL_MODULES);
  const [selection, setSelection] = useState<ReadonlySet<string>>(new Set());
  const [choices, setChoices] = useState<ReadonlyMap<string, Choice>>(new Map());
  const [focused, setFocused] = useState<string | null>(null);
  const [cleaning, setCleaning] = useState<Cleaning | null>(null);
  // The one thing two clicks in one task can both see, as in the Explorer.
  const running = useRef(false);

  const views = useMemo(() => modules.data ?? [], [modules.data]);
  const names = useMemo(() => new Map(views.map((view) => [view.id, view.name])), [views]);
  const all = useMemo(() => items.data?.items ?? [], [items.data]);
  const visible = useMemo(() => filtered(all, levels, module), [all, levels, module]);
  // Ticks of items that are gone — a batch, a rediscovery — tick nothing on screen, and a
  // batch is only ever the ticked rows the user can see.
  const ticked = useMemo(() => prune(selection, visible), [selection, visible]);
  const focusedItem = visible.find((item) => item.id === focused) ?? null;
  const busy = cleaning !== null && cleaning.phase !== 'previewFailed';

  const changeFilters = (nextLevels: ReadonlySet<VerdictLevel>, nextModule: string) => {
    setLevels(nextLevels);
    setModule(nextModule);
    // Pruned for good, not only hidden: a filter turned back on does not bring back ticks
    // the user stopped seeing.
    setSelection((current) => prune(current, filtered(all, nextLevels, nextModule)));
  };

  const toggleLevel = (level: VerdictLevel) => {
    const next = new Set(levels);
    if (next.has(level)) next.delete(level);
    else next.add(level);
    changeFilters(next, module);
  };

  const askAbout = (requests: CleanupRequest[]) => {
    if (requests.length === 0 || busy) return;
    setCleaning({ phase: 'previewing' });
    cleanupPreview(requests).then(
      (previews) =>
        setCleaning({
          phase: 'confirming',
          requests,
          questions: questionsFromCleanup(previews, names),
          status: { phase: 'asking' },
        }),
      (error: unknown) => setCleaning({ phase: 'previewFailed', message: String(error) }),
    );
  };

  /** What a batch that ran leaves for this page and the others to do. */
  const afterBatch = () => {
    setSelection(new Set());
    // The Explorer is unmounted while this page shows, and its tree queries never go stale
    // by themselves: the splice after this batch moved every id in them, so they are marked
    // for a refetch the next time the Explorer opens. The modules' own two queries hear the
    // rediscovery through `modules:state`, and are read again here for the part it cannot
    // say: what the batch forgot at once.
    void client.invalidateQueries({ queryKey: ['treeNode'] });
    void client.invalidateQueries({ queryKey: ['diskUsage'] });
    void client.invalidateQueries({ queryKey: ['modules'] });
    void client.invalidateQueries({ queryKey: ['cleanupItems'] });
  };

  const runCleanup = (mode: DeletionMode) => {
    if (cleaning?.phase !== 'confirming' || cleaning.status.phase !== 'asking') return;
    if (running.current) return;
    running.current = true;
    const asked = cleaning;
    setCleaning({ ...asked, status: { phase: 'running' } });
    let stop: (() => void) | undefined;
    // The listener first, then the batch: an event sent before anyone listens is lost, and
    // the first one says which entry the batch is on.
    void onCleanupProgress((progress) =>
      setCleaning((current) =>
        current?.phase === 'confirming' && current.status.phase === 'running'
          ? { ...current, status: { phase: 'running', progress } }
          : current,
      ),
    )
      .then((unlisten) => {
        stop = unlisten;
        return cleanupRun(asked.requests, mode);
      })
      .then(
        (result) => {
          setCleaning({ ...asked, status: { phase: 'done', report: reportFromCleanup(result) } });
          afterBatch();
        },
        // A rejection means the batch did not run: nothing to invalidate, and the ticks stay.
        (error: unknown) =>
          setCleaning({ ...asked, status: { phase: 'failed', message: String(error) } }),
      )
      .finally(() => {
        stop?.();
        running.current = false;
      });
  };

  const requests = requestsOf(visible, ticked, choices);
  const bytes = selectedBytes(visible, ticked, choices);
  const summary =
    ticked.size === 0 ? '' : `${countLabel(ticked.size, 'item')} selected · ${formatBytes(bytes)}`;
  const counts = new Map(
    VERDICT_LEVELS.map((level) => [
      level,
      all.filter(
        (item) =>
          item.verdict.level === level && (module === ALL_MODULES || item.module === module),
      ).length,
    ]),
  );

  const pending = cleaning?.phase === 'confirming' ? cleaning : null;

  if (modules.isError) {
    return (
      <section className="flex h-full flex-col items-center justify-center gap-3 p-8">
        <p role="alert" className="font-mono text-sm text-red-700 dark:text-red-300">
          {String(modules.error)}
        </p>
        <Button onClick={() => void modules.refetch()}>Try again</Button>
      </section>
    );
  }

  if (modules.data !== undefined && views.length === 0) {
    return (
      // The sidebar keeps this page out of reach in such a build; this is for the one that
      // reaches it anyway, before the list that says so has arrived.
      <section className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
        <h2 className="text-xl font-semibold">No cleanup modules in this build</h2>
        <p className="text-sm text-muted">Modules arrive with the next releases.</p>
      </section>
    );
  }

  return (
    <section className="flex h-full min-h-0 flex-col gap-3 p-4" aria-label="Cleanup">
      <header className="flex flex-col gap-2">
        <h2 className="text-lg font-semibold">Cleanup</h2>
        <ModuleStrip modules={views} onRefresh={(ids) => void refresh(ids)} />
      </header>

      <div className="flex flex-wrap items-center gap-2" data-testid="filters">
        <div role="group" aria-label="Verdicts" className="flex gap-1">
          {VERDICT_LEVELS.map((level) => (
            <button
              key={level}
              type="button"
              aria-pressed={levels.has(level)}
              onClick={() => toggleLevel(level)}
              className={`rounded-md border px-2 py-0.5 text-sm focus-visible:outline-2 focus-visible:outline-blue-500 ${
                levels.has(level)
                  ? 'border-blue-600 bg-blue-50 text-blue-800 dark:border-blue-500 dark:bg-blue-950 dark:text-blue-200'
                  : 'border-neutral-300 text-muted dark:border-neutral-700'
              }`}
            >
              {VERDICT_LABELS[level]} <span className="tabular-nums">{counts.get(level)}</span>
            </button>
          ))}
        </div>
        {views.length > 1 && (
          <label className="flex items-center gap-1.5 text-sm">
            Module
            <select
              value={module}
              onChange={(event) => changeFilters(levels, event.target.value)}
              className="rounded-md border border-neutral-300 bg-white px-1.5 py-0.5 dark:border-neutral-700 dark:bg-neutral-900"
            >
              <option value={ALL_MODULES}>All modules</option>
              {views.map((view) => (
                <option key={view.id} value={view.id}>
                  {view.name}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>

      {/* The count of the selection, announced once: the bar's own copy is `aria-hidden`. */}
      <p role="status" className="sr-only">
        {summary}
      </p>
      <CleanupBar
        summary={summary}
        ticked={ticked.size}
        busy={busy}
        error={cleaning?.phase === 'previewFailed' ? cleaning.message : null}
        onClean={() => askAbout(requests)}
        onDismiss={() => setCleaning(null)}
      />

      {items.data === undefined ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : all.length === 0 && views.some((view) => view.status === 'discovering') ? (
        <p className="text-sm text-muted">Looking for things to clean…</p>
      ) : (
        <div className="flex min-h-0 flex-1 gap-3">
          <div className="min-w-0 flex-1 overflow-auto">
            <ItemTable
              items={visible}
              total={items.data.total}
              moduleNames={names}
              selection={ticked}
              onSelectionChange={setSelection}
              focused={focusedItem?.id ?? null}
              onFocus={setFocused}
            />
          </div>
          {focusedItem !== null && (
            <aside className="w-72 shrink-0 overflow-auto">
              <ItemDetail
                item={focusedItem}
                moduleName={names.get(focusedItem.module) ?? focusedItem.module}
                choice={choiceFor(focusedItem, choices)}
                onChoice={(choice) =>
                  setChoices((current) => new Map(current).set(focusedItem.id, choice))
                }
                onReveal={(path) => void revealInFinder(path)}
                onClean={() =>
                  askAbout(requestsOf([focusedItem], new Set([focusedItem.id]), choices))
                }
                busy={busy}
              />
            </aside>
          )}
        </div>
      )}

      {pending !== null && (
        <ConfirmDeleteDialog
          questions={pending.questions}
          status={pending.status}
          onConfirm={runCleanup}
          onClose={() => setCleaning(null)}
        />
      )}
    </section>
  );
}
