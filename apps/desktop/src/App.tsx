import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import AppShell from './components/AppShell';
import { getAppInfo } from './lib/ipc';
import { DEFAULT_PAGE, pageEntry, type PageId } from './lib/pages';
import ActivityPage from './pages/ActivityPage';
import ExplorerPage from './pages/ExplorerPage';
import PlaceholderPage from './pages/PlaceholderPage';

export default function App() {
  const [page, setPage] = useState<PageId>(DEFAULT_PAGE);
  const info = useQuery({ queryKey: ['appInfo'], queryFn: getAppInfo, staleTime: Infinity });
  const versionLabel =
    info.data !== undefined
      ? `v${info.data.version}`
      : info.error !== null
        ? String(info.error)
        : '…';

  return (
    <AppShell page={page} onNavigate={setPage} versionLabel={versionLabel}>
      {/* One page at a time, which is what lets the Activity screen re-read the record on
          every open rather than being told to: leaving the Explorer unmounts it. */}
      {page === 'explorer' ? (
        <ExplorerPage />
      ) : page === 'activity' ? (
        <ActivityPage />
      ) : (
        <PlaceholderPage title={pageEntry(page).label} />
      )}
    </AppShell>
  );
}
