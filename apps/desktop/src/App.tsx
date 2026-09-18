import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import AppShell from './components/AppShell';
import { getAppInfo } from './lib/ipc';
import { DEFAULT_PAGE, pageEntry, type PageId } from './lib/pages';
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
      {page === 'explorer' ? <ExplorerPage /> : <PlaceholderPage title={pageEntry(page).label} />}
    </AppShell>
  );
}
