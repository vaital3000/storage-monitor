import { PAGES, type PageId } from '../lib/pages';

interface SidebarProps {
  page: PageId;
  onNavigate: (page: PageId) => void;
  /** `v1.2.3`, or the error that stood in its way. */
  versionLabel: string;
}

/**
 * The five sections of the design. Sections of later phases stay in the tab order and
 * open their placeholder, but are marked `aria-disabled` and dimmed.
 */
export default function Sidebar({ page, onNavigate, versionLabel }: SidebarProps) {
  return (
    <aside className="flex w-[220px] shrink-0 flex-col border-r border-neutral-200 bg-neutral-100 select-none dark:border-neutral-800 dark:bg-neutral-900">
      <nav aria-label="Sections" className="flex-1 overflow-y-auto p-2">
        <ul className="flex flex-col gap-0.5">
          {PAGES.map(({ id, label, icon: Icon, available }) => {
            const selected = id === page;
            return (
              <li key={id}>
                <button
                  type="button"
                  onClick={() => onNavigate(id)}
                  aria-current={selected ? 'page' : undefined}
                  aria-disabled={available ? undefined : true}
                  title={available ? undefined : 'Coming in a later phase'}
                  className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm focus-visible:outline-2 focus-visible:outline-blue-500 ${
                    selected
                      ? 'bg-neutral-200/80 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-50'
                      : 'text-neutral-700 hover:bg-neutral-200/60 dark:text-neutral-300 dark:hover:bg-neutral-800/70'
                  } ${available ? '' : 'opacity-50'}`}
                >
                  <Icon className="size-4 shrink-0" />
                  <span className="flex-1 truncate">{label}</span>
                  {!available && (
                    <span aria-hidden className="text-xs text-neutral-500">
                      soon
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      </nav>
      <footer className="border-t border-neutral-200 px-3 py-2 text-xs text-neutral-500 dark:border-neutral-800">
        Storage Monitor <span data-testid="version">{versionLabel}</span>
      </footer>
    </aside>
  );
}
