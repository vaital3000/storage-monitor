// The sections of the app shell (design section 10). The Explorer and Activity exist —
// phase 2a ships the deletion and the record of it together, because a screen that deletes
// with nothing that says what it deleted is half a feature. Cleanup exists from phase 2b,
// and opens only in a build that ships a module (`App` decides, from `modules_list`): a
// release build of 2b has none. The rest open a placeholder.

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
  /**
   * False for sections that arrive in a later phase. Cleanup's is the default the shell
   * starts from, and replaces once it knows whether the build ships a module.
   */
  available: boolean;
}

export const PAGES: readonly PageEntry[] = [
  { id: 'overview', label: 'Overview', icon: LayoutDashboard, available: false },
  { id: 'explorer', label: 'Explorer', icon: FolderTree, available: true },
  { id: 'cleanup', label: 'Cleanup', icon: Sparkles, available: false },
  { id: 'activity', label: 'Activity', icon: Activity, available: true },
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
