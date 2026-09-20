// The `test` every spec in this folder imports, instead of Playwright's own: it watches the
// page for Content Security Policy violations and fails whatever was running when one
// arrives.
//
// The server on port 1430 serves the header from `src-tauri/tauri.conf.json` verbatim
// (`vite.config.ts`, `STORAGE_MONITOR_E2E`), which makes this the only automated place where
// a browser runs this app under the policy it ships with. What the substitute is worth — and
// what it is not, the origin being a different one — is in `docs/adr/0006`.

import { expect, test as base } from '@playwright/test';

/** What a test can ask about the policy while it runs. */
export interface CspWatch {
  /** Every violation this page has reported, oldest first. */
  readonly seen: readonly string[];
  /**
   * "Everything reported so far was planted by me." The positive control's escape hatch,
   * and the only way a violation does not fail the test that saw it.
   */
  planted: () => void;
}

interface Reporter {
  __cspViolation: (violation: string) => void;
}

export const test = base.extend<{ csp: CspWatch }>({
  csp: [
    async ({ page }, use) => {
      const seen: string[] = [];
      let planted = 0;
      // Reported to node as it happens rather than collected on `window`, so that a
      // violation survives the navigation that follows it.
      await page.exposeFunction('__cspViolation', (violation: string) => {
        seen.push(violation);
      });
      await page.addInitScript(() => {
        document.addEventListener('securitypolicyviolation', (event) => {
          // The source location is what makes a real failure actionable; the directive and
          // the blocked URI are what a test asserts on.
          const at = event.sourceFile === '' ? '' : ` at ${event.sourceFile}:${event.lineNumber}`;
          (window as unknown as Reporter).__cspViolation(
            `${event.effectiveDirective} <- ${event.blockedURI}${at}`,
          );
        });
      });
      await use({
        seen,
        planted: () => {
          planted = seen.length;
        },
      });
      expect(seen.slice(planted), 'Content Security Policy violations').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
export type { Locator, Page } from '@playwright/test';
