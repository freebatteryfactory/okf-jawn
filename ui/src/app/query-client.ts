/** The one server cache: TanStack Query over the generated client, retrying only transient failures. */
import { QueryClient } from '@tanstack/react-query';
import { shouldRetry } from '../features/errors/failure';

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: shouldRetry, refetchOnWindowFocus: false, staleTime: 30_000 },
    },
  });
}
