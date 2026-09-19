// The only module that talks to the backend. Types mirror `src-tauri/src/views.rs`,
// `crates/core/src/disk.rs`, `crates/core/src/snapshot/delta.rs` and
// `crates/core/src/action/` field by field in camelCase; command and argument names match
// `src-tauri/src/commands.rs`.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { revealItemInDir } from '@tauri-apps/plugin-opener';

export type { UnlistenFn };

export interface AppInfo {
  name: string;
  version: string;
}

export type NodeId = number;

export type NodeKind = 'dir' | 'file' | 'symlink' | 'other';

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

/** How an entry leaves the disk. */
export type Mode = 'trash' | 'permanent';

/**
 * Why an entry will not be deleted, mirroring `BlockReason`. All eight of them: the guards
 * tell `missing` from `unreadable` (grant Full Disk Access, do not go hunting for a ghost)
 * and `malformed` from both, so a screen that folds them together says the wrong thing.
 */
export type BlockReason =
  | 'outsideRoots'
  | 'denylisted'
  | 'malformed'
  | 'isRoot'
  | 'nested'
  | 'missing'
  | 'unreadable'
  | 'kindChanged';

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
  /** Every path that was asked about, in order, blocked ones included. */
  entries: PreviewEntry[];
  /** Sum of `size` over the ready entries only. */
  totalBytes: number;
  mode: Mode;
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
  mode: Mode;
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
export type LogResult = 'removed' | 'failed' | 'skipped';

/** One line of the action log, readable on its own long after the dialog that wrote it. */
export interface ActivityEntry {
  /** When the batch began; every line of one batch carries the same instant. */
  at: string;
  /**
   * What was deleted, as the guards normalized it — which under a symlinked scan root is
   * not the spelling the Explorer showed, so do not match an entry back to a row by string.
   */
  path: string;
  kind: NodeKind;
  mode: Mode;
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
export function actionPreview(paths: string[], mode: Mode): Promise<Preview> {
  return invoke<Preview>('action_preview', { paths, mode });
}

/**
 * Deletes `paths`, records the batch and patches the tree.
 *
 * A rejection means the batch did **not** run. A batch that ran always resolves, and says
 * through `recorded` and `treeStale` what it could not finish afterwards.
 */
export function actionRun(paths: string[], mode: Mode): Promise<BatchResult> {
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
