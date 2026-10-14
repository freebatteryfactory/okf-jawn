/**
 * The one server cache: TanStack Query over the generated client, retrying only transient failures.
 *
 * Making the cache also puts the workspace's transport on the generated client (the CSRF header
 * and the bounded wait, `api-client.ts`), so no request of the app goes out without them.
 *
 * Requests are sent whatever the browser says about being online (`networkMode: 'always'`): the
 * service is on this computer by default, and TanStack Query's default would hold a request back
 * while the browser reports no network, showing "Saving…" with nothing in flight and no bound.
 * That mode also turns off the library's asking again when the network returns, so it is turned
 * back on (`refetchOnReconnect`): a read that failed while the network was away is asked again
 * when it is back, as it was before that mode was set.
 */
import { QueryClient } from '@tanstack/react-query';
import { shouldRetry } from '../features/errors/failure';
import { configureApiClient } from './api-client';

export function createQueryClient(): QueryClient {
  configureApiClient();
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: shouldRetry,
        refetchOnWindowFocus: false,
        staleTime: 30_000,
        networkMode: 'always',
        refetchOnReconnect: true,
      },
      mutations: { networkMode: 'always' },
    },
  });
}
