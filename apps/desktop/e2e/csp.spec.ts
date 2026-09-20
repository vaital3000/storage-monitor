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
    // None of these is a bare `eval('…')`, and that is measured rather than stylistic: an
    // eval written straight into this function reports that the policy allows eval — it
    // ran, and raised nothing — because `page.evaluate` compiles its argument through the
    // debugger, which the page's policy does not cover. What goes through the page's own
    // loader is checked: a `<script>` element it inserts, a `Worker`, a `FontFace`, and a
    // `setTimeout` handed a **string**, which the page compiles in a task of its own.
    const script = document.createElement('script');
    script.textContent = 'window.__planted = "inline script ran";';
    document.head.append(script);

    const worker = URL.createObjectURL(new Blob(['self.close();'], { type: 'text/javascript' }));
    new Worker(worker);

    const face = new FontFace('Planted', 'url(data:font/woff2;base64,AA==)');
    document.fonts.add(face);
    // The rejection is the point; `NetworkError` is what a blocked fetch raises.
    face.load().catch(() => undefined);

    // The eval class the ADR names, and the one that would let a dependency compile a
    // string at runtime. `setTimeout` takes the string without complaint; the compilation
    // is what `script-src` refuses.
    setTimeout('window.__timed = "the timer string ran";', 0);

    return (window as unknown as { __planted?: string }).__planted;
  });

  // Not only the event: the inline script never ran. An instrument that reported a
  // violation for code that executed anyway would be worse than none.
  expect(planted).toBeUndefined();
  await expect
    .poll(() => directives(csp.seen))
    .toEqual([
      'font-src <- data',
      'script-src <- eval',
      'script-src-elem <- inline',
      'worker-src <- blob',
    ]);
  // Read after the four violations are in, by which time the timer's own task is long
  // past: the string was never compiled, so it can never run.
  expect(await page.evaluate(() => (window as unknown as { __timed?: string }).__timed)).toBe(
    undefined,
  );

  // Mine, all four — without this the watcher fails this test on the way out, which is
  // exactly what it does to a spec that trips the policy by accident.
  csp.planted();
});

/**
 * The watcher's own alarm, which `csp.spec.ts` above does not test: that test proves the
 * listener **collects** violations, and would go on passing if the teardown assertion in
 * `fixtures.ts` were deleted, or if the fixture stopped being `auto`. Both were tried as
 * one-token mutants and left all sixteen specs green.
 *
 * `test.fail()` inverts the verdict, so this passes only if something fails — and the body
 * asserts nothing and never asks for `csp`, so the only thing that can fail is the watcher
 * reaching a test that did not want it. Delete either half of the watcher and this test
 * goes red with "expected to fail, but passed".
 */
test.fail('a violation nobody planted fails the test that met it', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => {
    const script = document.createElement('script');
    script.textContent = 'window.__stray = 1;';
    document.head.append(script);
  });
  // One more round trip, so that the violation is reported before the fixture reads it:
  // the listener's call and this command's reply travel the same ordered channel.
  await page.evaluate(() => 0);
});
