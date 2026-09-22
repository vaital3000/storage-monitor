import { SquareArrowOutUpRight } from 'lucide-react';
import { describeFact } from '../lib/facts';
import type { Item } from '../lib/ipc';
import type { Choice } from '../lib/selection';
import { itemSize } from '../lib/items';
import Button from './Button';
import VerdictBadge from './VerdictBadge';

interface ItemDetailProps {
  item: Item;
  moduleName: string;
  choice: Choice;
  onChoice: (choice: Choice) => void;
  onReveal: (path: string) => void;
  /** "Clean…" for this item alone, with the choice made here. */
  onClean: () => void;
  /** A batch is being checked or run: nothing here may start another. */
  busy: boolean;
}

const SECTION = 'flex flex-col gap-1';
const HEADING = 'text-xs font-medium tracking-wide text-muted uppercase';

/**
 * Everything the module said about one item — where it is, why the verdict is what it is,
 * the facts it found — and what to do with it: the action, its options, and a button that
 * cleans this item alone. A force option is drawn in the danger style and says what it
 * overrides, because turning it on is the one way to clean an item the module wants kept.
 */
export default function ItemDetail({
  item,
  moduleName,
  choice,
  onChoice,
  onReveal,
  onClean,
  busy,
}: ItemDetailProps) {
  const action = item.actions.find((spec) => spec.id === choice.action);
  const toggleOption = (id: string, on: boolean) => {
    const options = on ? [...choice.options, id] : choice.options.filter((option) => option !== id);
    onChoice({ ...choice, options });
  };
  return (
    <section
      aria-label="Details"
      data-testid="item-detail"
      className="flex flex-col gap-4 rounded-lg border border-neutral-200 bg-white p-4 dark:border-neutral-800 dark:bg-neutral-900"
    >
      <header className="flex flex-col gap-1">
        <h3 className="text-base font-semibold break-words">{item.title}</h3>
        <p className="text-sm text-muted">
          {[moduleName, item.subtitle].filter((part) => part !== null && part !== '').join(' · ')}
        </p>
        <p className="text-sm tabular-nums">{itemSize(item)}</p>
        {item.path !== null && (
          <p className="flex min-w-0 items-center gap-1">
            <span className="truncate font-mono text-xs" title={item.path}>
              {item.path}
            </span>
            <button
              type="button"
              aria-label="Reveal in Finder"
              title="Reveal in Finder"
              onClick={() => item.path !== null && onReveal(item.path)}
              className="shrink-0 rounded p-0.5 text-muted hover:text-neutral-700 focus-visible:outline-2 focus-visible:outline-blue-500 dark:hover:text-neutral-200"
            >
              <SquareArrowOutUpRight className="size-3.5" />
            </button>
          </p>
        )}
      </header>

      <div className={SECTION}>
        <h4 className={HEADING}>Verdict</h4>
        <div>
          <VerdictBadge level={item.verdict.level} />
        </div>
        <ul data-testid="reasons" className="list-disc pl-4 text-sm">
          {item.verdict.reasons.map((reason) => (
            <li key={reason.code}>{reason.text}</li>
          ))}
        </ul>
      </div>

      {item.facts.length > 0 && (
        <div className={SECTION}>
          <h4 className={HEADING}>Facts</h4>
          <dl data-testid="facts" className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-sm">
            {item.facts.map((fact) => (
              <div key={fact.key} className="contents">
                <dt className="text-muted">{fact.label}</dt>
                <dd className="min-w-0 truncate" title={describeFact(fact.value)}>
                  {describeFact(fact.value)}
                </dd>
              </div>
            ))}
          </dl>
        </div>
      )}

      <div className={SECTION}>
        <h4 className={HEADING}>Action</h4>
        {item.actions.length > 1 ? (
          <fieldset className="flex flex-col gap-1">
            <legend className="sr-only">What to do</legend>
            {item.actions.map((spec) => (
              <label key={spec.id} className="flex items-center gap-1.5 text-sm">
                <input
                  type="radio"
                  name={`action-${item.id}`}
                  checked={choice.action === spec.id}
                  disabled={busy}
                  // A different action has different options: start from its defaults.
                  onChange={() =>
                    onChoice({
                      action: spec.id,
                      options: spec.options.filter((o) => o.default).map((o) => o.id),
                    })
                  }
                  className="size-4 accent-blue-600"
                />
                {spec.label}
              </label>
            ))}
          </fieldset>
        ) : (
          <p className="text-sm">{action?.label ?? 'Nothing can be done with it'}</p>
        )}
        {action !== undefined &&
          action.options.map((option) => (
            <label
              key={option.id}
              className={`flex items-start gap-1.5 text-sm ${
                option.force ? 'text-red-700 dark:text-red-400' : ''
              }`}
            >
              <input
                type="checkbox"
                checked={choice.options.includes(option.id)}
                disabled={busy}
                onChange={(event) => toggleOption(option.id, event.target.checked)}
                className="mt-0.5 size-4 accent-red-600"
              />
              <span className="flex flex-col">
                {option.label}
                {option.force && (
                  <span className="text-xs">Overrides the Keep verdict of this item.</span>
                )}
              </span>
            </label>
          ))}
      </div>

      <Button disabled={busy || action === undefined} onClick={onClean}>
        Clean…
      </Button>
    </section>
  );
}
