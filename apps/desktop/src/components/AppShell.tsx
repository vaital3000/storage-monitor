import type { ReactNode } from 'react';
import type { PageEntry, PageId } from '../lib/pages';
import Sidebar from './Sidebar';
import TitleBar from './TitleBar';

interface AppShellProps {
  page: PageId;
  pages?: readonly PageEntry[];
  onNavigate: (page: PageId) => void;
  versionLabel: string;
  children: ReactNode;
}

/** Title bar on top, sidebar on the left, and a content area that scrolls on its own. */
export default function AppShell({
  page,
  pages,
  onNavigate,
  versionLabel,
  children,
}: AppShellProps) {
  return (
    <div className="flex h-full flex-col overflow-hidden">
      <TitleBar title="Storage Monitor" />
      <div className="flex min-h-0 flex-1">
        <Sidebar page={page} pages={pages} onNavigate={onNavigate} versionLabel={versionLabel} />
        <main className="min-w-0 flex-1 overflow-y-auto">{children}</main>
      </div>
    </div>
  );
}
