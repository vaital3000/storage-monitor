// What a modal has to know to keep the keyboard inside it: the ring of places a Tab would
// stop. A dialog reads it to find its two ends; its test reads the same function, because
// jsdom performs no Tab of its own and the ring cannot be observed from the outside.

/**
 * `:disabled` and not `[disabled]`: a control inside a `<fieldset disabled>` is disabled
 * with no attribute of its own — its `.disabled` property is even `false` — and an
 * attribute selector goes on matching it. One idiomatic simplification of a form, from
 * `disabled` on every input to `disabled` on the fieldset, would otherwise put dead
 * elements at the ends of the ring and let Tab walk out of the dialog. jsdom implements
 * the pseudo-class for that case, so the rule is tested rather than trusted.
 */
const FOCUSABLE = 'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])';

/**
 * The tab stops inside `container`, in document order — only inside it, which is the whole
 * point: the ends of the ring must be elements of the dialog and never of the page it
 * covers.
 *
 * A radio group is one stop — the checked radio — while `querySelectorAll` returns every
 * radio in it. An end taken from an unchecked one sits where the browser never stops, so
 * Shift+Tab steps backwards past the trap and out. The rule reads `checked` and never
 * groups by `name`, deliberately: a group can be split across containers, and half a group
 * seen through this container is still answered correctly — the checked radio is a stop
 * wherever its siblings are. A group with nothing checked has no stop here where a browser
 * would stop at its first radio; nothing in this app renders one, and erring towards fewer
 * stops keeps the ends on elements that are really reachable.
 *
 * ## What this does not model
 *
 * It is the whole trap of a dialog that stands in front of an irreversible action, so what
 * it leaves out is a limit on the markup that dialog may use, not a rough edge to live
 * with. A caller with richer markup has to extend it first:
 *
 * - Matched but never focusable: `[hidden]`, `display: none`, `visibility: hidden`, a
 *   descendant of `[inert]`, and `input[type="hidden"]`.
 * - Focusable but missed: `<a href>`, `<select>`, `<textarea>`, `<summary>`, and anything
 *   `contenteditable`.
 * - A positive `tabindex` comes first in the browser's order; this returns document order.
 *
 * The first group cannot be tested in this project at all: jsdom has no layout, so nothing
 * here can tell a hidden element from a shown one. That is a reason to keep such elements
 * out of a trapped dialog, not a reason to write an untestable check for them.
 */
export function tabStops(container: HTMLElement): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (element) =>
      !(element instanceof HTMLInputElement && element.type === 'radio' && !element.checked),
  );
}
