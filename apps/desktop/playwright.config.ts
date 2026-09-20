import { defineConfig, devices } from '@playwright/test';

const PORT = 1430;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `pnpm exec vite --mode mock --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    // The mock server, sealed: it serves the window's own Content Security Policy and turns
    // fast refresh off, because the preamble that injects is an inline script the policy
    // blocks. `vite.config.ts` says the rest; `csp.spec.ts` checks that the header really
    // arrived, which is what a reused server from an earlier `just dev-web` would fail.
    env: { STORAGE_MONITOR_E2E: '1' },
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
