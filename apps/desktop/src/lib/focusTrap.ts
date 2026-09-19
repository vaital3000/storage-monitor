// What a modal has to know to keep the keyboard inside it: the ring of places a Tab would
// stop. A dialog reads it to find its two ends; its test reads the same function, because
// jsdom performs no Tab of its own and the ring cannot be observed from the outside.

const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * The tab stops inside `container`, in document order.
 *
 * A radio group is one stop — the checked radio — while `querySelectorAll` hands back every
 * radio in it. That difference is the whole reason this is a function and not a selector:
 * an end of the ring taken from an unchecked radio sits on an element the browser never
 * stops at, and Shift+Tab then steps backwards past the trap and out of the dialog.
 *
 * A group with nothing checked has no stop under this rule, where a browser would stop at
 * its first radio. Nothing here renders one — a mode is always selected — and a trap that
 * skips a group it cannot name is safe in the direction that matters: it keeps the ends on
 * elements that are really reachable.
 */
export function tabStops(container: HTMLElement): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (element) =>
      !(element instanceof HTMLInputElement && element.type === 'radio' && !element.checked),
  );
}
