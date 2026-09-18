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

  it('lists the five sections: Explorer selected, the others disabled and marked "soon"', async () => {
    installIpcMock();
    renderWithClient(<App />);
    const nav = screen.getByRole('navigation', { name: 'Sections' });
    expect(within(nav).getAllByRole('button')).toHaveLength(5);
    const explorer = within(nav).getByRole('button', { name: 'Explorer' });
    expect(explorer).toHaveAttribute('aria-current', 'page');
    expect(explorer).toBeEnabled();
    for (const label of ['Overview', 'Cleanup', 'Activity', 'Settings']) {
      const button = within(nav).getByRole('button', { name: `${label} soon` });
      expect(button).toBeDisabled();
      expect(button).not.toHaveAttribute('aria-disabled');
      expect(button).not.toHaveAttribute('aria-current');
    }
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeInTheDocument();
  });

  it('stays on the Explorer when a section of a later phase is clicked', async () => {
    installIpcMock();
    renderWithClient(<App />);
    expect(await screen.findByRole('button', { name: 'Scan' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Cleanup soon' }));
    expect(screen.getByRole('button', { name: 'Scan' })).toBeInTheDocument();
    expect(screen.queryByText('Coming in a later phase')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Explorer' })).toHaveAttribute(
      'aria-current',
      'page',
    );
  });
});
