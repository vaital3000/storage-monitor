// The only module that talks to the backend. Types mirror `src-tauri/src/views.rs`,
// `crates/core/src/disk.rs`, `crates/core/src/snapshot/delta.rs`,
// `crates/core/src/action/`, `crates/core/src/module/` and `crates/core/src/cleanup/` field
// by field in camelCase; command and argument names match `src-tauri/src/commands.rs`.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { revealItemInDir } from '@tauri-apps/plugin-opener';

export type { UnlistenFn };

export interface AppInfo {
  name: string;
  version: string;
}

export type NodeId = number;

/**
 * Every kind the walker classifies, as the values first and the type from them: a variant
 * added to `NodeKind` in Rust and mirrored here then reaches the code that has to enumerate
 * them — the mock's log reader, an icon map — instead of leaving it silently behind.
 */
export const NODE_KINDS = ['dir', 'file', 'symlink', 'other'] as const;

export type NodeKind = (typeof NODE_KINDS)[number];

export type ScanState = 'idle' | 'running' | 'done' | 'cancelled' | 'failed';

export interface ScanStatus {
  state: ScanState;
  root: string | null;
  files: number;
  dirs: number;
  bytes: number;
  errors: number;
  /** Directory being read; empty unless the scan is running. */
  currentPath: string;
  durationMs: number;
  error: string | null;
  /** A previous snapshot exists, so deltas are available. */
  hasPrevious: boolean;
  /** RFC 3339 timestamp of the previous snapshot. */
  previousTakenAt: string | null;
}

export interface Crumb {
  id: NodeId;
  name: string;
}

export interface ChildView {
  id: NodeId;
  name: string;
  kind: NodeKind;
  size: number;
  logicalSize: number;
  fileCount: number;
  /** Seconds since the Unix epoch. */
  mtime: number;
  error: string | null;
  /** Growth since the previous snapshot; null when the snapshot has no entry. */
  delta: number | null;
  hasChildren: boolean;
}

export interface NodeView {
  id: NodeId;
  name: string;
  path: string;
  kind: NodeKind;
  size: number;
  logicalSize: number;
  fileCount: number;
  mtime: number;
  error: string | null;
  delta: number | null;
  /** From the root down to this node, inclusive. */
  breadcrumbs: Crumb[];
  /** Largest first, at most `limit` of them. */
  children: ChildView[];
  childrenTotal: number;
  /** True when `children` is shorter than `childrenTotal`. */
  truncated: boolean;
}

/** Space on the volume that contains `path`; `used = total - free`. */
export interface DiskUsage {
  path: string;
  total: number;
  /** Bytes free for this user. */
  available: number;
  /** Bytes free overall, including the superuser reserve. */
  free: number;
  used: number;
}

/** Size change of one path between two snapshots. */
export interface Delta {
  path: string;
  kind: NodeKind;
  before: number;
  after: number;
  delta: number;
}

/** How an entry leaves the disk. `Mode` in Rust, where it lives under `action::`. */
export const DELETION_MODES = ['trash', 'permanent'] as const;

export type DeletionMode = (typeof DELETION_MODES)[number];

/**
 * Why an entry will not be deleted, mirroring `BlockReason`. All ten of them: the guards
 * tell `missing` from `unreadable` (grant Full Disk Access, do not go hunting for a ghost)
 * and `malformed` from both, so a screen that folds them together says the wrong thing.
 * `shielded` is the one that is not a dead end — the folder is refused, what is inside it
 * is not — so folding it into `denylisted` would cost the user the way forward. `kept` is
 * a cleanup item's alone: a Keep verdict asked for without its force option, which the user
 * can lift from the same screen.
 */
export const BLOCK_REASONS = [
  'outsideRoots',
  'denylisted',
  'shielded',
  'malformed',
  'isRoot',
  'nested',
  'missing',
  'unreadable',
  'kindChanged',
  'kept',
] as const;

export type BlockReason = (typeof BLOCK_REASONS)[number];

/** The verdict of the guards on one entry. */
export type EntryStatus = { state: 'ready' } | { state: 'blocked'; reason: BlockReason };

export interface PreviewEntry {
  /** Normalized for an entry the guards let through; as it was asked for when they did not. */
  path: string;
  /**
   * What the entry is: read from the disk for a ready entry, and otherwise the claim of the
   * tree, unverified — `status` is what says which of the two this is.
   */
  kind: NodeKind;
  /** Allocated bytes as the scan recorded them; 0 for a path the tree does not know. */
  size: number;
  status: EntryStatus;
}

/** A checked batch: nothing was touched, so it is safe to show and to throw away. */
export interface Preview {
  /**
   * Every path that was asked about, in the order they were asked about, blocked ones
   * included. Readonly because that order is the backend's answer: a screen that sorted it
   * in place would renumber rows another screen is still holding.
   */
  entries: readonly PreviewEntry[];
  /** Sum of `size` over the ready entries only. */
  totalBytes: number;
  mode: DeletionMode;
}

/** What became of one entry. */
export type EntryResult =
  | { result: 'removed'; bytes: number }
  | { result: 'failed'; message: string }
  | { result: 'skipped'; reason: BlockReason };

export interface EntryOutcome {
  /** The path of the matching `PreviewEntry`, with the same two spellings. */
  path: string;
  kind: NodeKind;
  result: EntryResult;
}

/** What a whole batch did. */
export interface Outcome {
  entries: EntryOutcome[];
  /** Bytes of the removed entries; under `trash`, what emptying the Trash will free. */
  freedBytes: number;
  /** RFC 3339 timestamp of the batch, shared by every entry of it. */
  at: string;
  mode: DeletionMode;
}

/**
 * A batch that ran, with the two things the window has to admit about it.
 *
 * Both are failures of a batch that *did* delete, so neither is an error: a rejected
 * `actionRun` means the batch did not run at all.
 */
export interface BatchResult {
  outcome: Outcome;
  /** False when the entries are deleted and the action log does not have them. */
  recorded: boolean;
  /** True when the Explorer still shows a row for something this batch deleted. */
  treeStale: boolean;
}

/** What became of one entry, as the action log says it: the verdict alone. */
export const LOG_RESULTS = ['removed', 'failed', 'skipped'] as const;

export type LogResult = (typeof LOG_RESULTS)[number];

/** Where a cleanup line came from, as the module, the item and the action were named then. */
export interface LogSource {
  /** The module's id. */
  module: string;
  /** The item's id. */
  item: string;
  title: string;
  /** The action's label. */
  action: string;
}

/** One line of the action log, readable on its own long after the dialog that wrote it. */
export interface ActivityEntry {
  /** When the batch began; every line of one batch carries the same instant. */
  at: string;
  /**
   * What was deleted, as the guards normalized it — which under a symlinked scan root is
   * not the spelling the Explorer showed, so do not match an entry back to a row by string.
   * A line of the Explorer always has one; a cleanup line has the item's path, and none at
   * all when the item named no path or was already gone. Absent then, not null.
   */
  path?: string;
  /** Absent exactly when `path` is, and for a cleanup item whose plan had no target there. */
  kind?: NodeKind;
  /**
   * How the entry left. A cleanup line says the mode the entry really left in, which is
   * `permanent` for anything the Trash cannot undo, whatever the batch was asked.
   */
  mode: DeletionMode;
  result: LogResult;
  /**
   * Meaningless without `result`: the failure message under `failed`, the wire name of a
   * `BlockReason` under `skipped`, null under `removed`. The two kinds of string cannot be
   * told apart — a failure whose message reads `denylisted` is not a blocked entry — so
   * nothing may read this without reading `result` first.
   */
  detail: string | null;
  /** Bytes this entry freed; 0 for anything that was not removed. */
  bytes: number;
  /** Present on a cleanup line and absent on a line of the Explorer: how to tell them apart. */
  source?: LogSource;
  /** The argv of every command a cleanup entry started, in order; absent when none did. */
  commands?: string[][];
}

/**
 * What an entry's `detail` says, read the only way it may be read: with `result` first.
 *
 * The rule lives here, once and executably, rather than in every screen that draws a row —
 * `detail` holds two different things and one absence, and the strings cannot be told apart
 * by looking at them. A failure whose message happens to read `denylisted` is a failure.
 */
export function logDetail(
  entry: ActivityEntry,
): { kind: 'message'; message: string } | { kind: 'reason'; reason: BlockReason } | null {
  if (entry.detail === null) {
    return null;
  }
  if (entry.result === 'failed') {
    return { kind: 'message', message: entry.detail };
  }
  if (entry.result === 'skipped' && isBlockReason(entry.detail)) {
    return { kind: 'reason', reason: entry.detail };
  }
  // A `removed` line carries no detail, and a `skipped` one carries a reason this version
  // does not know — a variant added to `BlockReason` after this build. Neither is a string
  // to put on screen as if it explained something.
  return null;
}

function isBlockReason(value: string): value is BlockReason {
  return (BLOCK_REASONS as readonly string[]).includes(value);
}

/** The end of the action log. */
export interface LogTail {
  /** Newest first. */
  entries: ActivityEntry[];
  /**
   * Lines of the stretch that was read which could not be parsed, and are therefore missing
   * from `entries`. A screen that shows the entries says this number too.
   */
  damaged: number;
}

/** A module's confidence that an item can go, mirroring `Level`. */
export const VERDICT_LEVELS = ['safe', 'review', 'keep'] as const;

export type VerdictLevel = (typeof VERDICT_LEVELS)[number];

/** Why a verdict is what it is: a code for tests, a sentence for the screen. */
export interface Reason {
  code: string;
  text: string;
}

export interface Verdict {
  level: VerdictLevel;
  reasons: Reason[];
}

/** A value the detail panel draws by its type; dates are RFC 3339. */
export type FactValue =
  | { type: 'text'; value: string }
  | { type: 'bytes'; value: number }
  | { type: 'count'; value: number }
  | { type: 'date'; value: string }
  | { type: 'flag'; value: boolean }
  | { type: 'path'; value: string };

export interface Fact {
  key: string;
  label: string;
  value: FactValue;
}

/** A switch on an action; one marked `force` lets the action remove an item marked Keep. */
export interface ActionOption {
  id: string;
  label: string;
  /** Whether it starts turned on. */
  default: boolean;
  force: boolean;
}

/** Something that can be done with an item. What it does is planned per mode, in the preview. */
export interface ActionSpec {
  id: string;
  label: string;
  /** What the dialog promises and the record says was freed. */
  estimatedFree: number;
  options: ActionOption[];
}

/** One thing a module found. */
export interface Item {
  /** `<module>:<native id>`, stable across discoveries: what a selection is keyed by. */
  id: string;
  module: string;
  /** The module's own word for it: `folder`, `object`, `worktree`, `image`. */
  kind: string;
  title: string;
  subtitle: string | null;
  path: string | null;
  size: { bytes: number; estimated: boolean };
  /** RFC 3339. */
  lastUsed: string | null;
  verdict: Verdict;
  facts: Fact[];
  /** The first is what a batch does unless the detail panel says otherwise. */
  actions: ActionSpec[];
}

/** At most the limit of what the modules hold, largest first, and how many there are. */
export interface ItemsPage {
  items: Item[];
  total: number;
}

/** Where a module stands, mirroring `ModuleStatus`. */
export const MODULE_STATUSES = ['idle', 'discovering', 'ready', 'unavailable', 'failed'] as const;

export type ModuleStatus = (typeof MODULE_STATUSES)[number];

export interface ModuleView {
  id: string;
  name: string;
  description: string;
  status: ModuleStatus;
  /** Why it is unavailable, or what made its last discovery fail. */
  reason: string | null;
  itemCount: number;
  /** Over the items it holds — after a failed refresh, those of the last one that worked. */
  totalBytes: number;
  safeBytes: number;
  /** RFC 3339: when the held items were found. */
  discoveredAt: string | null;
}

/** One item to clean, with the action and the options chosen for it. */
export interface CleanupRequest {
  item: string;
  action: string;
  options: string[];
}

/** What a `run` step changes, as its module declared it. */
export type StepEffect = 'housekeeping' | 'destroys' | 'removes';

/** One step as the dialog shows it. A command line is for reading; nothing parses it back. */
export type StepView =
  | { step: 'trash'; path: string }
  | { step: 'delete'; path: string }
  | { step: 'run'; command: string; effect: StepEffect; path: string | null };

/** One request, checked in one mode. */
export interface CleanupEntry {
  item: string;
  /** Empty for an item that is not held any more. */
  module: string;
  /** The item's title, or its id when it is not held any more. */
  title: string;
  /** The action's label, or its id when the item does not offer it. */
  action: string;
  /** What would run, in order; empty when the entry was refused before it was planned. */
  steps: readonly StepView[];
  size: number;
  status: EntryStatus;
  /** Whether the Trash can undo it in this preview's mode. */
  reversible: boolean;
}

/** A checked cleanup batch in one mode: nothing was touched. */
export interface CleanupPreview {
  /** One per request, in the order they came, refused ones included. */
  entries: readonly CleanupEntry[];
  totalBytes: number;
  mode: DeletionMode;
}

/** A cleanup batch checked in both modes, entries aligned by index. */
export interface CleanupPreviews {
  trash: CleanupPreview;
  permanent: CleanupPreview;
}

export interface CleanupEntryOutcome {
  item: string;
  module: string;
  title: string;
  action: string;
  path: string | null;
  kind: NodeKind | null;
  /** Every target a step tried to delete, normalized. */
  targets: string[];
  /** How this entry left: `permanent` for anything the Trash cannot undo. */
  mode: DeletionMode;
  /** The argv of every command that started, in order. */
  commands: string[][];
  result: EntryResult;
}

export interface CleanupOutcome {
  entries: CleanupEntryOutcome[];
  freedBytes: number;
  /** RFC 3339 timestamp of the batch, shared by every entry of it. */
  at: string;
  /** The mode the batch was asked to run in; each entry says the mode it really left in. */
  mode: DeletionMode;
}

/** A cleanup batch that ran, with the two things the window has to admit about it. */
export interface CleanupResult {
  outcome: CleanupOutcome;
  /** False when the entries are cleaned and the action log does not have them. */
  recorded: boolean;
  /** True when the Explorer still shows a row for something this batch deleted. */
  treeStale: boolean;
}

/** How far a cleanup batch has got: sent before every entry and once at the end. */
export interface CleanupProgress {
  done: number;
  total: number;
  /** The title of the entry starting now; null once the batch is over. */
  current: string | null;
}

export const MODULES_STATE_EVENT = 'modules:state';
export const CLEANUP_PROGRESS_EVENT = 'cleanup:progress';

export const SCAN_PROGRESS_EVENT = 'scan:progress';
export const SCAN_DONE_EVENT = 'scan:done';

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>('get_app_info');
}

/** The folder scanned when the UI does not pick one. */
export function defaultRoot(): Promise<string> {
  return invoke<string>('default_root');
}

/** Starts a scan of `root` (default: the home folder); progress arrives as events. */
export function scanStart(root?: string): Promise<ScanStatus> {
  return invoke<ScanStatus>('scan_start', { root });
}

export function scanStatus(): Promise<ScanStatus> {
  return invoke<ScanStatus>('scan_status');
}

/** Asks the running scan to stop; `scan:done` follows with the partial result. */
export function scanCancel(): Promise<ScanStatus> {
  return invoke<ScanStatus>('scan_cancel');
}

/** One page of the last scan's tree: node `id` (default: the root) with up to `limit` children (default: 500). */
export function treeNode(id?: NodeId, limit?: number): Promise<NodeView> {
  return invoke<NodeView>('tree_node', { id, limit });
}

/** Usage of the volume holding `path` (default: the scan root, else the home folder). */
export function diskUsage(path?: string): Promise<DiskUsage> {
  return invoke<DiskUsage>('disk_usage', { path });
}

/** The folders that grew the most since the previous snapshot, at most `limit` (default: 10). */
export function topGrowers(limit?: number): Promise<Delta[]> {
  return invoke<Delta[]>('top_growers', { limit });
}

/** What deleting `paths` would do, with nothing touched: one row per path, with its verdict. */
export function actionPreview(paths: string[], mode: DeletionMode): Promise<Preview> {
  return invoke<Preview>('action_preview', { paths, mode });
}

/**
 * Deletes `paths`, records the batch and patches the tree.
 *
 * A rejection means the batch did **not** run. A batch that ran always resolves, and says
 * through `recorded` and `treeStale` what it could not finish afterwards.
 */
export function actionRun(paths: string[], mode: DeletionMode): Promise<BatchResult> {
  return invoke<BatchResult>('action_run', { paths, mode });
}

/**
 * The last `limit` entries of the action log (default: 100), newest first.
 *
 * A rejection means the log could not be read — never "there is nothing to show": an empty
 * log and a line that could not be parsed both resolve, the second one counted in `damaged`.
 */
export function activityLog(limit?: number): Promise<LogTail> {
  return invoke<LogTail>('activity_log', { limit });
}

/** The cleanup modules of this build, where each stands and what it holds. */
export function modulesList(): Promise<ModuleView[]> {
  return invoke<ModuleView[]>('modules_list');
}

/**
 * Starts a discovery of the modules named in `ids` (every module when there are none).
 * Answers once all of them are `discovering`; each change of state arrives as `modules:state`.
 */
export function modulesRefresh(ids?: string[]): Promise<ModuleView[]> {
  return invoke<ModuleView[]>('modules_refresh', { ids });
}

/** What the modules hold, largest first: at most `limit` items (default: 2000). */
export function cleanupItems(limit?: number): Promise<ItemsPage> {
  return invoke<ItemsPage>('cleanup_items', { limit });
}

/** What cleaning `requests` would do in each mode, with nothing touched. */
export function cleanupPreview(requests: CleanupRequest[]): Promise<CleanupPreviews> {
  return invoke<CleanupPreviews>('cleanup_preview', { requests });
}

/**
 * Cleans `requests` in `mode`, with `cleanup:progress` events on the way.
 *
 * A rejection means the batch did **not** run. A batch that ran always resolves, and says
 * through `recorded` and `treeStale` what it could not finish afterwards.
 */
export function cleanupRun(requests: CleanupRequest[], mode: DeletionMode): Promise<CleanupResult> {
  return invoke<CleanupResult>('cleanup_run', { requests, mode });
}

/** A module whose state changed: discovering, ready, unavailable or failed. */
export function onModulesState(callback: (view: ModuleView) => void): Promise<UnlistenFn> {
  return listen<ModuleView>(MODULES_STATE_EVENT, (event) => callback(event.payload));
}

/** Before every entry of a cleanup batch, and once at its end. */
export function onCleanupProgress(
  callback: (progress: CleanupProgress) => void,
): Promise<UnlistenFn> {
  return listen<CleanupProgress>(CLEANUP_PROGRESS_EVENT, (event) => callback(event.payload));
}

/** Status updates every 250 ms while a scan runs. */
export function onScanProgress(callback: (status: ScanStatus) => void): Promise<UnlistenFn> {
  return listen<ScanStatus>(SCAN_PROGRESS_EVENT, (event) => callback(event.payload));
}

/** The final status of a scan, whatever its outcome. */
export function onScanDone(callback: (status: ScanStatus) => void): Promise<UnlistenFn> {
  return listen<ScanStatus>(SCAN_DONE_EVENT, (event) => callback(event.payload));
}

/** Selects `path` in a Finder window. */
export function revealInFinder(path: string): Promise<void> {
  return revealItemInDir(path);
}
