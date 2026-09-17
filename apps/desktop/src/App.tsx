import { useEffect, useState } from 'react';
import { getAppInfo, type AppInfo } from './lib/ipc';

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch((e: unknown) => setError(String(e)));
  }, []);

  return (
    <main className="flex h-full flex-col items-center justify-center gap-2 bg-neutral-50 text-neutral-900 dark:bg-neutral-950 dark:text-neutral-50">
      <h1 className="text-2xl font-semibold">{info?.name ?? 'Storage Monitor'}</h1>
      <p className="text-sm text-neutral-500" data-testid="version">
        {error ?? (info ? `v${info.version}` : 'Loading…')}
      </p>
    </main>
  );
}
