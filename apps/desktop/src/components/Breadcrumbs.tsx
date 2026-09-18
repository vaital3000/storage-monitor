import { ChevronRight } from 'lucide-react';
import type { Crumb, NodeId } from '../lib/ipc';

interface BreadcrumbsProps {
  /** From the root down to the current node, inclusive. */
  crumbs: Crumb[];
  onSelect: (id: NodeId) => void;
}

/** The path of the current node; every ancestor is a button, the last crumb is text. */
export default function Breadcrumbs({ crumbs, onSelect }: BreadcrumbsProps) {
  return (
    <nav aria-label="Breadcrumb">
      <ol className="flex flex-wrap items-center gap-0.5 text-sm">
        {crumbs.map((crumb, index) => {
          const last = index === crumbs.length - 1;
          return (
            <li key={crumb.id} className="flex items-center gap-0.5">
              {index > 0 && <ChevronRight className="size-3.5 shrink-0 text-neutral-400" />}
              {last ? (
                <span aria-current="page" className="px-1 font-semibold">
                  {crumb.name}
                </span>
              ) : (
                <button
                  type="button"
                  onClick={() => onSelect(crumb.id)}
                  className="rounded px-1 text-neutral-600 hover:bg-neutral-200/70 hover:text-neutral-900 focus-visible:outline-2 focus-visible:outline-blue-500 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
                >
                  {crumb.name}
                </button>
              )}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
