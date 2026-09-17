import { mockIPC } from '@tauri-apps/api/mocks';
import type { AppInfo } from '../lib/ipc';

export const MOCK_APP_INFO: AppInfo = { name: 'Storage Monitor', version: '0.0.0-mock' };

/** Installs fake handlers for every backend command. Used by unit tests, e2e and browser dev. */
export function installIpcMock(): void {
  mockIPC((cmd) => {
    switch (cmd) {
      case 'get_app_info':
        return MOCK_APP_INFO;
      default:
        throw new Error(`Unmocked IPC command: ${cmd}`);
    }
  });
}
