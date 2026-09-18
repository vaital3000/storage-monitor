import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, type RenderResult } from '@testing-library/react';
import type { ReactElement } from 'react';

/** A client that fails fast: IPC errors are deterministic, retrying only slows tests down. */
export function createTestQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
  });
}

/** Renders `ui` under a fresh QueryClientProvider, like `main.tsx` does for the app. */
export function renderWithClient(ui: ReactElement): RenderResult {
  return render(<QueryClientProvider client={createTestQueryClient()}>{ui}</QueryClientProvider>);
}
