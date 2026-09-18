// The only module that talks to the backend. Types mirror `src-tauri/src/views.rs`,
// `crates/core/src/disk.rs` and `crates/core/src/snapshot/delta.rs` field by field in
// camelCase; command and argument names match `src-tauri/src/commands.rs`.

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
