import { mockIPC } from '@tauri-apps/api/mocks';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';
import { installIpcMock } from './mocks/ipc';

describe('App', () => {
  it('shows the product name and the version reported by the backend', async () => {
    installIpcMock();
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'Storage Monitor' })).toBeInTheDocument();
    expect(await screen.findByText('v0.0.0-mock')).toBeInTheDocument();
  });

  it('shows the backend error when the command fails', async () => {
    mockIPC(() => {
      throw new Error('boom');
    });
    render(<App />);
    expect(await screen.findByText('Error: boom')).toBeInTheDocument();
  });
});
