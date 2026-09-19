import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach } from 'vitest';
import { cleanup } from '@testing-library/react';
import { clearMocks } from '@tauri-apps/api/mocks';
import { resetMockActions } from '../mocks/actions';
import { resetIpcMock, setMockScanDelay } from '../mocks/ipc';

beforeEach(() => {
  // A simulated scan at the browser pace (150 ms per tick) outlasts Testing Library's
  // 1 s `findBy*` timeout; tests that want to watch the progress set their own delay.
  setMockScanDelay(0);
});

afterEach(() => {
  cleanup();
  // Stop a simulated scan a test left running before its mock goes away.
  resetIpcMock();
  // The fixture tree and the action log are module state a batch changes, and no test may
  // see what the one before it deleted. `resetIpcMock` does this too, for the callers that
  // never come through here: `just dev-web` and every e2e page load.
  resetMockActions();
  clearMocks();
});
