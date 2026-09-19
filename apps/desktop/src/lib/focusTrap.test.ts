import { afterEach, describe, expect, it } from 'vitest';
import { tabStops } from './focusTrap';

/** Builds a container from HTML and puts it in the document, where focus rules apply. */
function container(html: string): HTMLElement {
  const host = document.createElement('div');
  host.innerHTML = html;
  document.body.append(host);
  return host;
}

function names(elements: HTMLElement[]): string[] {
  return elements.map((element) => element.getAttribute('data-name') ?? element.tagName);
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('tabStops', () => {
  it('takes the buttons and inputs in document order', () => {
    const host = container(`
      <button data-name="first"></button>
      <input data-name="text" type="text" />
      <p data-name="prose">not a stop</p>
      <button data-name="last"></button>
    `);
    expect(names(tabStops(host))).toEqual(['first', 'text', 'last']);
  });

  it('leaves out what the browser skips: disabled controls and tabindex -1', () => {
    const host = container(`
      <button data-name="enabled"></button>
      <button data-name="disabled" disabled></button>
      <input data-name="off" disabled />
      <div data-name="held" tabindex="-1"></div>
      <div data-name="reachable" tabindex="0"></div>
    `);
    expect(names(tabStops(host))).toEqual(['enabled', 'reachable']);
  });

  it('counts a radio group as one stop, the checked radio', () => {
    // The reason this is a function rather than a selector. An end of the ring taken from
    // an unchecked radio sits where the browser never stops, and the Tab that should have
    // wrapped steps out of the dialog instead.
    const host = container(`
      <input data-name="trash" type="radio" name="mode" />
      <input data-name="permanent" type="radio" name="mode" checked />
      <button data-name="confirm"></button>
    `);
    expect(names(tabStops(host))).toEqual(['permanent', 'confirm']);
  });

  it('follows the checked radio when the selection moves', () => {
    const host = container(`
      <input data-name="trash" type="radio" name="mode" checked />
      <input data-name="permanent" type="radio" name="mode" />
    `);
    expect(names(tabStops(host))).toEqual(['trash']);

    host.querySelector<HTMLInputElement>('[data-name="permanent"]')!.checked = true;
    expect(names(tabStops(host))).toEqual(['permanent']);
  });

  it('is empty when nothing inside can be reached', () => {
    const host = container(`<p>text</p><button disabled></button>`);
    expect(tabStops(host)).toEqual([]);
  });
});
