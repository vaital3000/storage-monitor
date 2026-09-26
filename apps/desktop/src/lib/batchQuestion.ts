// What the confirmation dialog asks, and then reports, whichever screen started the batch:
// the Explorer's paths and the Cleanup screen's items, each adapted into one shape, so that
// `ConfirmDeleteDialog` stays the one door to a deletion (phase 2b design, section 11).

import type { GuardScope } from './blockReasons';
import type {
  BatchResult,
  CleanupPreview,
  CleanupPreviews,
  CleanupResult,
  DeletionMode,
  EntryResult,
  EntryStatus,
  Preview,
  StepView,
} from './ipc';

/** One row of the question, in one mode. */
export interface BatchRow {
  /** The path, for the Explorer; the item's title, for Cleanup. */
  title: string;
  /** "Demo · Delete folder", for Cleanup; absent for the Explorer, whose title says it all. */
  context?: string;
  /** What will run, in order; absent for the Explorer, where the title is the whole step. */
  steps?: readonly StepView[];
  size: number;
  status: EntryStatus;
  /** True when the Trash cannot undo this row in the question's mode. */
  irreversible: boolean;
}

export interface BatchQuestion {
  /** One per entry of the batch, in the order the backend answered them. */
  rows: readonly BatchRow[];
  /** Over the ready rows: the backend's own number, never a sum taken again here. */
  totalBytes: number;
}

/** Which screen's words the dialog speaks: deleting paths, or cleaning items. */
export type BatchWording = 'delete' | 'clean';

/**
 * The batch in both modes, the rows of the two aligned by index. The dialog switches between
 * them without a round trip, so it can never show one mode's rows above a button armed for
 * the other: which of the two it reads is decided by its own toggle alone.
 */
export interface BatchQuestions {
  trash: BatchQuestion;
  permanent: BatchQuestion;
  wording: BatchWording;
  /** Where the guards were built from, which two block reasons name. */
  scope: GuardScope;
}

/** One entry of a batch that ran. */
export interface ReportRow {
  title: string;
  /** How this entry left: `permanent` for anything the Trash cannot undo. */
  mode: DeletionMode;
  result: EntryResult;
}

/** What a batch that ran did, and the two things the window has to admit about it. */
export interface BatchReport {
  rows: readonly ReportRow[];
  freedBytes: number;
  /** The mode the batch was asked to run in. */
  mode: DeletionMode;
  /** False when the entries are gone and the action log does not have them. */
  recorded: boolean;
  /** True when the Explorer still shows a row for something this batch deleted. */
  treeStale: boolean;
  wording: BatchWording;
  scope: GuardScope;
}

/**
 * The Explorer's preview as a question. The guards never read the mode, so the rows are the
 * same in both, and a deletion is irreversible exactly when it is permanent — which makes the
 * dialog ask for its acknowledgement exactly where it always did.
 *
 * Takes a preview without its mode, as the dialog used to: nothing downstream can read the
 * mode the guards happened to be called with.
 */
export function questionsFromPreview(preview: Omit<Preview, 'mode'>): BatchQuestions {
  const rows = (irreversible: boolean): BatchRow[] =>
    preview.entries.map((entry) => ({
      title: entry.path,
      size: entry.size,
      status: entry.status,
      irreversible,
    }));
  return {
    trash: { rows: rows(false), totalBytes: preview.totalBytes },
    permanent: { rows: rows(true), totalBytes: preview.totalBytes },
    wording: 'delete',
    scope: 'scan',
  };
}

/** What an Explorer batch did: every row in the mode of the batch, titled by its path. */
export function reportFromBatch(result: BatchResult): BatchReport {
  const { outcome, recorded, treeStale } = result;
  return {
    rows: outcome.entries.map((entry) => ({
      title: entry.path,
      mode: outcome.mode,
      result: entry.result,
    })),
    freedBytes: outcome.freedBytes,
    mode: outcome.mode,
    recorded,
    treeStale,
    wording: 'delete',
    scope: 'scan',
  };
}

/**
 * The Cleanup screen's pair of previews as a question: each row titled by its item, with the
 * module and the action as its context and the steps it would run.
 */
export function questionsFromCleanup(
  previews: CleanupPreviews,
  moduleNames: ReadonlyMap<string, string>,
): BatchQuestions {
  const question = (preview: CleanupPreview): BatchQuestion => ({
    rows: preview.entries.map((entry) => ({
      title: entry.title,
      context: [moduleNames.get(entry.module) ?? entry.module, entry.action]
        .filter((part) => part !== '')
        .join(' · '),
      steps: entry.steps,
      size: entry.size,
      status: entry.status,
      irreversible: !entry.reversible,
    })),
    totalBytes: preview.totalBytes,
  });
  return {
    trash: question(previews.trash),
    permanent: question(previews.permanent),
    wording: 'clean',
    scope: 'home',
  };
}

/** What a cleanup batch did: every row in the mode its entry really left in. */
export function reportFromCleanup(result: CleanupResult): BatchReport {
  const { outcome, recorded, treeStale } = result;
  return {
    rows: outcome.entries.map((entry) => ({
      title: entry.title,
      mode: entry.mode,
      result: entry.result,
    })),
    freedBytes: outcome.freedBytes,
    mode: outcome.mode,
    recorded,
    treeStale,
    wording: 'clean',
    scope: 'home',
  };
}
