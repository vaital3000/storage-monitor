import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const host = process.env.TAURI_DEV_HOST;

/**
 * The server Playwright drives: the mock one (`--mode mock`), sealed. It is the only place
 * in the project where a browser runs this app under the policy it ships with — Tauri
 * attaches the header in the `tauri://localhost` handler, which a `devUrl` document never
 * reaches, so neither `just dev` nor `just dev-web` can apply it.
 *
 * Set by `playwright.config.ts`, and an environment variable rather than a mode of its own
 * so that `.env.mock` stays the single file that says what the mock server is: a second
 * mode would need a copy of it, and the copies would drift.
 */
const e2e = process.env.STORAGE_MONITOR_E2E === '1';

/**
 * The window's Content Security Policy, read from the config that ships it rather than
 * copied into this file, so that the two cannot drift.
 */
function appCsp(): string {
  const file = fileURLToPath(new URL('./src-tauri/tauri.conf.json', import.meta.url));
  const config: unknown = JSON.parse(readFileSync(file, 'utf8'));
  const csp = (config as { app?: { security?: { csp?: unknown } } }).app?.security?.csp;
  if (typeof csp !== 'string') {
    throw new Error('tauri.conf.json has no app.security.csp to serve');
  }
  return csp;
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Tauri prints its own errors; keep them visible.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    // Off for the e2e server, and not as a speed-up: fast refresh injects its preamble as
    // an **inline** script, which `script-src 'self'` blocks — measured, and the page then
    // renders nothing at all. Keeping it would mean serving a policy with a nonce in it,
    // which is no longer the policy the app ships with. The packaged app has no preamble,
    // so turning fast refresh off is what makes the served page the one under test.
    hmr: e2e ? false : host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
    // Byte for byte what the window gets, so that the classes which break under it — an
    // `'unsafe-eval'`, a `blob:` worker, a `data:` font — break here too. `'self'` does
    // **not** mean the same thing across the two origins; `docs/adr/0006` says what this
    // substitute is worth and what it is not.
    headers: e2e ? { 'Content-Security-Policy': appCsp() } : undefined,
  },
  envPrefix: ['VITE_'],
  build: {
    // macOS 13.3+ = Safari 16.4, required by Tailwind v4.
    target: 'safari16',
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
