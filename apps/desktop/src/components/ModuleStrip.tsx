import { RefreshCw } from 'lucide-react';
import { countLabel, formatBytes } from '../lib/format';
import type { ModuleView } from '../lib/ipc';
import Button from './Button';

interface ModuleStripProps {
  modules: readonly ModuleView[];
  onRefresh: (ids?: string[]) => void;
}

/** Where one module stands, in words. */
function standing(module: ModuleView): string {
  const holds = `${countLabel(module.itemCount, 'item')} · ${formatBytes(module.totalBytes)}`;
  switch (module.status) {
    case 'idle':
      return 'Not looked at yet';
    case 'discovering':
      // What it held before stays on screen while it looks again, and says so.
      return module.itemCount > 0 ? `Discovering… (${holds})` : 'Discovering…';
    case 'ready':
      return holds;
    case 'unavailable':
      return `Unavailable: ${module.reason ?? 'no reason given'}`;
    case 'failed':
      return `Failed: ${module.reason ?? 'no reason given'}`;
  }
}

/**
 * Every module of the build and where it stands, with one Refresh for all of them and a
 * Retry beside a module whose last discovery failed. A failure keeps its items on screen —
 * the strip is where the user learns that they are the items of an earlier look.
 */
export default function ModuleStrip({ modules, onRefresh }: ModuleStripProps) {
  const busy = modules.some((module) => module.status === 'discovering');
  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="module-strip">
      <ul className="flex flex-wrap gap-2" aria-label="Modules">
        {modules.map((module) => (
          <li
            key={module.id}
            data-testid={`module-${module.id}`}
            data-status={module.status}
            title={module.description}
            className="flex items-center gap-2 rounded-md border border-neutral-200 bg-white px-2 py-1 text-sm dark:border-neutral-800 dark:bg-neutral-900"
          >
            <span className="font-medium">{module.name}</span>
            <span
              className={
                module.status === 'failed'
                  ? 'text-red-700 dark:text-red-400'
                  : module.status === 'unavailable'
                    ? 'text-amber-700 dark:text-amber-400'
                    : 'text-muted'
              }
            >
              {standing(module)}
            </span>
            {module.status === 'failed' && (
              <Button onClick={() => onRefresh([module.id])}>Retry</Button>
            )}
          </li>
        ))}
      </ul>
      <Button className="ml-auto" disabled={busy} onClick={() => onRefresh()}>
        <RefreshCw className="size-3.5" />
        Refresh
      </Button>
    </div>
  );
}
