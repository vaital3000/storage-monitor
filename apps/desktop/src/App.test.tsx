import { mockIPC } from '@tauri-apps/api/mocks';
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';
import { installIpcMock } from './mocks/ipc';
import { renderWithClient } from './test/render';

describe('App', () => {
  it('shows the product name and the version reported by the backend', async () => {
    installIpcMock();
    renderWithClient(<App />);
    expect(await screen.findByRole('heading', { name: 'Storage Monitor' })).toBeInTheDocument();
    const version = await screen.findByText('v0.0.0-mock');
    expect(version).toBeInTheDocument();
    expect(version.parentElement).toHaveTextContent('Storage Monitor v0.0.0-mock');
  });

  it('shows the backend error in place of the version when the command fails', async () => {
    mockIPC(() => {
      throw new Error('boom');
    });
    renderWithClient(<App />);
    await waitFor(() => expect(screen.getByTestId('version')).toHaveTextContent('Error: boom'));
  });

  it('lists the five sections, Explorer selected, the others focusable but marked', async () => {
    installIpcMock();
    renderWithClient(<App />);
    const nav = screen.getByRole('navigation', { name: 'Sections' });
    expect(within(nav).getAllByRole('button')).toHaveLength(5);
    expect(within(nav).getByRole('button', { name: 'Explorer' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    expect(within(nav).getByRole('button', { name: 'Explorer' })).not.toHaveAttribute(
      'aria-disabled',
    );
    for (const label of ['Overview', 'Cleanup', 'Activity', 'Settings']) {
      const button = within(nav).getByRole('button', { name: label });
      expect(button).toHaveAttribute('aria-disabled', 'true');
      expect(button).not.toHaveAttribute('aria-current');
      expect(button).not.toBeDisabled();
      button.focus();
      expect(button).toHaveFocus();
    }
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeInTheDocument();
  });

  it('opens a placeholder for a section that is not built yet, and comes back', async () => {
    installIpcMock();
    renderWithClient(<App />);
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Cleanup' }));
    expect(screen.getByRole('heading', { name: 'Cleanup' })).toBeInTheDocument();
    expect(screen.getByText('Coming in a later phase')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cleanup' })).toHaveAttribute('aria-current', 'page');
    expect(screen.queryByRole('button', { name: 'Scan' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Explorer' }));
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeInTheDocument();
  });
});
