import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config.ts';

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: 'jsdom',
      setupFiles: ['./src/test/setup.ts'],
      include: ['src/**/*.test.{ts,tsx}'],
      // A zone that is not UTC, and one with a summer time, because this app formats
      // instants into local time and a suite that runs in UTC cannot see the difference:
      // `getUTCHours` for `getHours` is then the same function, and the test that would
      // have caught it passes on the runner while the app shows the wrong hour to
      // everyone else. CI runners are UTC, so leaving this to the machine means the check
      // exists only on the maintainer's laptop. `the_suite_runs_outside_utc` in
      // `src/lib/format.test.ts` fails if this line is removed.
      env: { TZ: 'America/New_York' },
    },
  }),
);
