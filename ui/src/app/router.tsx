/**
 * The router owns the URL: a workspace list, a workspace, an item and its selection.
 *
 * The route tree is generator output (`bun scripts/dev.mjs routes`); the routes are authored.
 * Tests pass a memory history so a deep link can be opened without a browser.
 *
 * A navigation can take the element that has keyboard focus off the page: Back from an item while
 * focus is in it, or the way out of a failure, which is a link of the page it leaves. Focus would
 * then be on the page body. So when a navigation has rendered, focus that was on an element of the
 * page when the navigation began, and is on the body now, goes to the new page's level-one
 * heading. Focus that was on the body when the navigation began is left there: that covers the
 * first load, and a pointer press that focused nothing. Focus that survived, or that the page has
 * already placed in the same change (a layout effect: `useRetry`), is left where it is. The router
 * says a navigation has rendered once React has committed it (`onRendered`), which is after the
 * page's layout effects and before its passive ones.
 *
 * Limits. Navigate from event handlers. A navigation begun in a layout effect or a ref callback of
 * the page another navigation has just rendered is noted with focus still on the body, and the
 * first navigation's focus then stays there. A page that places focus itself after a navigation
 * does it in a layout effect; in a passive one, focus passes through the heading first. A page
 * with no level-one heading that takes focus (the router's own pages for a path that matches no
 * route, or for a route that throws) leaves focus on the body.
 */
import type { QueryClient } from '@tanstack/react-query';
import { QueryClientProvider } from '@tanstack/react-query';
import { createRouter, type RouterHistory, RouterProvider } from '@tanstack/react-router';
import { pageHeading } from '../lib/focus';
import { routeTree } from '../routeTree.gen';

/** Keyboard focus is on an element of the page, not on the page body. */
const focusIsInPage = () => document.activeElement?.matches('body *') === true;

export function createAppRouter(history?: RouterHistory) {
  const router = createRouter({
    routeTree,
    ...(history === undefined ? {} : { history }),
    defaultPreload: false,
  });
  // The one focus rule of navigation, for every route: see the header. Every navigation says it
  // begins before it says it has rendered, so what was seen at its beginning is its own.
  let focusWasInPage = false;
  router.subscribe('onBeforeNavigate', () => {
    focusWasInPage = focusIsInPage();
  });
  router.subscribe('onRendered', () => {
    if (focusWasInPage && !focusIsInPage()) pageHeading()?.focus();
  });
  return router;
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
