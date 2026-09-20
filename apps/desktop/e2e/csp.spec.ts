// The Content Security Policy, exercised by a browser.
//
// Nothing else in this project does. `just dev` cannot — Tauri attaches the header in the
// `tauri://localhost` handler, and a `devUrl` document never passes through it — and CI's
// `tauri build` compiles the binary without launching it. So the header is served from the
// mock server instead (`vite.config.ts`), every spec in this folder runs under it, and the
// watcher in `fixtures.ts` fails any test that trips it.
//
// It is weaker than it looks: the origin is `http://localhost:1430`, so `'self'` here is not
// `'self'` in the packaged app, and the module graph is Vite's rather than the bundle's.
// What transfers is the class that actually breaks — a `blob:` worker, a `data:` font, an
// `'unsafe-eval'` — which is why the two tests below plant exactly those.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test } from './fixtures';

/** The policy the window ships with, from the file that ships it. */
function shippedCsp(): string {
  const file = fileURLToPath(new URL('../src-tauri/tauri.conf.json', import.meta.url));
  const config = JSON.parse(readFileSync(file, 'utf8')) as {
    app: { security: { csp: string } };
  };
  return config.app.security.csp;
}

/** `directive <- blockedURI`, without the source location, which carries the port. */
function directives(seen: readonly string[]): string[] {
  return seen.map((violation) => violation.split(' at ')[0]).sort();
}

test('serves the policy the window ships with, not a copy of it', async ({ page }) => {
  const response = await page.goto('/');

  // The header has to be *there*: `reuseExistingServer` will happily hand these tests a
  // plain `just dev-web` server left running on this port, under which every assertion
  // about a violation would pass by never firing.
  expect(response?.headers()['content-security-policy']).toBe(shippedCsp());
});

test('blocks what it is there to block, and the watcher sees it', async ({ page, csp }) => {
  await page.goto('/');

  const planted = await page.evaluate(() => {
    // Through the DOM, every one of them, and deliberately: a `eval('…')` written here
    // would report that the policy allows eval, because `page.evaluate` runs its argument
    // through the debugger, which is not subject to the page's policy. Measured — the eval
    // ran and raised nothing. A `<script>` the page inserts, a `Worker`, and a `FontFace`
    // all go through the loader instead, which is where the policy is enforced.
    const script = document.createElement('script');
    script.textContent = 'window.__planted = "inline script ran";';
    document.head.append(script);

    const worker = URL.createObjectURL(new Blob(['self.close();'], { type: 'text/javascript' }));
    new Worker(worker);

    const face = new FontFace('Planted', 'url(data:font/woff2;base64,AA==)');
    document.fonts.add(face);
    // The rejection is the point; `NetworkError` is what a blocked fetch raises.
    face.load().catch(() => undefined);

    return (window as unknown as { __planted?: string }).__planted;
  });

  // Not only the event: the inline script never ran. An instrument that reported a
  // violation for code that executed anyway would be worse than none.
  expect(planted).toBeUndefined();
  await expect
    .poll(() => directives(csp.seen))
    .toEqual(['font-src <- data', 'script-src-elem <- inline', 'worker-src <- blob']);

  // Mine, all three — without this the watcher fails this test on the way out, which is
  // exactly what it does to a spec that trips the policy by accident.
  csp.planted();
});
