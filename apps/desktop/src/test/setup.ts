import '@testing-library/jest-dom/vitest';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';
import { clearMocks } from '@tauri-apps/api/mocks';
import { resetIpcMock } from '../mocks/ipc';

afterEach(() => {
  cleanup();
  // Stop a simulated scan a test left running before its mock goes away.
  resetIpcMock();
  clearMocks();
});
