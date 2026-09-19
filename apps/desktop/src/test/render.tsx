import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import { render, type RenderResult } from '@testing-library/react';
import type { ReactElement } from 'react';
import { createQueryClient } from '../lib/queryClient';

/**
 * Renders `ui` under a QueryClientProvider, like `main.tsx` does for the app: a fresh
 * client unless the test passes one of its own.
 *
 * Passing one is how a test renders the same screen twice the way the app does — the shell
 * unmounts a page when another is opened and the cache outlives it — which a second fresh
 * client would quietly turn into a first load.
 */
export function renderWithClient(
  ui: ReactElement,
  client: QueryClient = createQueryClient(),
): RenderResult {
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}
