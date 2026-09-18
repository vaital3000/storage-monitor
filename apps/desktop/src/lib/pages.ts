// The sections of the app shell (design section 10). Only the Explorer exists in phase 1;
// the others open a placeholder.

import {
  Activity,
  FolderTree,
  LayoutDashboard,
  Settings,
  Sparkles,
  type LucideIcon,
} from 'lucide-react';

export type PageId = 'overview' | 'explorer' | 'cleanup' | 'activity' | 'settings';

export interface PageEntry {
  id: PageId;
  label: string;
  icon: LucideIcon;
  /** False for sections that arrive in a later phase. */
  available: boolean;
}

export const PAGES: readonly PageEntry[] = [
  { id: 'overview', label: 'Overview', icon: LayoutDashboard, available: false },
  { id: 'explorer', label: 'Explorer', icon: FolderTree, available: true },
  { id: 'cleanup', label: 'Cleanup', icon: Sparkles, available: false },
  { id: 'activity', label: 'Activity', icon: Activity, available: false },
  { id: 'settings', label: 'Settings', icon: Settings, available: false },
];

export const DEFAULT_PAGE: PageId = 'explorer';

export function pageEntry(id: PageId): PageEntry {
  const entry = PAGES.find((page) => page.id === id);
  if (entry === undefined) {
    throw new Error(`unknown page ${id}`);
  }
  return entry;
}
