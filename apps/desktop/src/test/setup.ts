import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach } from 'vitest';
import { cleanup } from '@testing-library/react';
import { clearMocks } from '@tauri-apps/api/mocks';
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
  clearMocks();
});
