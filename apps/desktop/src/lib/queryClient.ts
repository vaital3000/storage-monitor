import { QueryClient } from '@tanstack/react-query';

/** Commands are local and deterministic: a failure is worth showing, not retrying. */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
  });
}
