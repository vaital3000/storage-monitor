import { QueryClientProvider } from '@tanstack/react-query';
import { render, type RenderResult } from '@testing-library/react';
import type { ReactElement } from 'react';
import { createQueryClient } from '../lib/queryClient';

/** Renders `ui` under a fresh QueryClientProvider, like `main.tsx` does for the app. */
export function renderWithClient(ui: ReactElement): RenderResult {
  return render(<QueryClientProvider client={createQueryClient()}>{ui}</QueryClientProvider>);
}
