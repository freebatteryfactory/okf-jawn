/**
 * The router owns the URL: a workspace list, a workspace, an item and its selection.
 *
 * The route tree is generator output (`bun scripts/dev.mjs routes`); the routes are authored.
 * Tests pass a memory history so a deep link can be opened without a browser.
 */
import type { QueryClient } from '@tanstack/react-query';
import { QueryClientProvider } from '@tanstack/react-query';
import { createRouter, type RouterHistory, RouterProvider } from '@tanstack/react-router';
import { routeTree } from '../routeTree.gen';

export function createAppRouter(history?: RouterHistory) {
  return createRouter({
    routeTree,
    ...(history === undefined ? {} : { history }),
    defaultPreload: false,
  });
}

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof createAppRouter>;
  }
}

export interface AppProps {
  router: ReturnType<typeof createAppRouter>;
  queryClient: QueryClient;
}

export function App({ router, queryClient }: AppProps) {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}
