import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import AppShell from './components/AppShell';
import { getAppInfo, modulesList } from './lib/ipc';
import { DEFAULT_PAGE, PAGES, pageEntry, type PageId } from './lib/pages';
import ActivityPage from './pages/ActivityPage';
import CleanupPage from './pages/CleanupPage';
import ExplorerPage from './pages/ExplorerPage';
import PlaceholderPage from './pages/PlaceholderPage';

export default function App() {
  const [page, setPage] = useState<PageId>(DEFAULT_PAGE);
  const info = useQuery({ queryKey: ['appInfo'], queryFn: getAppInfo, staleTime: Infinity });
  // The Cleanup page's own query: one cache for the sidebar and the page. Cleanup opens once
  // the build is known to ship a module, which a release build of phase 2b does not.
  const modules = useQuery({ queryKey: ['modules'], queryFn: modulesList, staleTime: 0 });
  const hasModules = (modules.data?.length ?? 0) > 0;
  const pages = useMemo(
    () =>
      PAGES.map((entry) => (entry.id === 'cleanup' ? { ...entry, available: hasModules } : entry)),
    [hasModules],
  );
  const versionLabel =
    info.data !== undefined
      ? `v${info.data.version}`
      : info.error !== null
        ? String(info.error)
        : '…';

  return (
    <AppShell page={page} pages={pages} onNavigate={setPage} versionLabel={versionLabel}>
      {/* One page at a time, which is what lets the Activity screen re-read the record on
          every open rather than being told to: leaving the Explorer unmounts it. */}
      {page === 'explorer' ? (
        <ExplorerPage />
      ) : page === 'activity' ? (
        <ActivityPage />
      ) : page === 'cleanup' ? (
        <CleanupPage />
      ) : (
        <PlaceholderPage title={pageEntry(page).label} />
      )}
    </AppShell>
  );
}
