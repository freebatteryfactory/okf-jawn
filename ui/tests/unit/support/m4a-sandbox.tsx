/**
 * Ground for the HTML sandbox viewer's unit tests (UI M4a, ruling A3): an HTML source's bytes as
 * the service describes them, the capability `create_sandbox_capability` answers for them, and the
 * viewer rendered alone, inside the query cache and a minimal router (a failure's way home is a
 * router link).
 *
 * Each capability a test mints is a new address on the sandbox origin, numbered in the order the
 * service answered it, so a test can tell which one a frame shows.
 */
import { QueryClientProvider } from '@tanstack/react-query';
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import type { SandboxCapability, SourceAppearance } from '../../../src/api/generated/types.gen';
import { zSandboxCapability, zSourceAppearance } from '../../../src/api/generated/zod.gen';
import { createQueryClient } from '../../../src/app/query-client';
import type { OriginalProps } from '../../../src/features/viewers/types';
import { HEAD, NOW, SOURCE, WS } from './workspace';

/** The configured sandbox origin: never the page's own. */
export const SANDBOX_ORIGIN = 'http://sandbox.okf.test:7712';
/** The HTML file's bytes, as one digest of the item's revision. */
export const HTML_OBJECT = 'e'.repeat(64);
export const HTML_PATH = 'pages/welcome.html.md';

/** An HTML source's bytes, converted whole, as `get_item` describes them (held to its schema). */
export function htmlAppearance(mediaType = 'text/html'): SourceAppearance {
  const appearance: SourceAppearance = {
    extraction: {
      corrected: false,
      outcome: { status: 'completed' },
      text_origin: 'converter',
      warnings: [],
      digest: 'f'.repeat(64),
    },
    media_type: mediaType,
    metadata: {},
    names: [{ filename: 'welcome.html', folder: 'pages', observed_at: NOW, supplied_by: 'upload' }],
    object: HTML_OBJECT,
    size: '2048',
  };
  zSourceAppearance.parse(appearance);
  return appearance;
}

/** What the viewer is given for that source at the head. */
export function htmlProps(overrides: Partial<OriginalProps> = {}): OriginalProps {
  return {
    workspaceId: WS,
    itemId: SOURCE,
    revision: HEAD,
    path: HTML_PATH,
    appearance: htmlAppearance(),
    selection: { kind: 'all' },
    ...overrides,
  };
}

/** The `number`th capability the service mints, expiring `lifetimeMs` after now. */
export function capability(number: number, lifetimeMs = 300_000): SandboxCapability {
  return zSandboxCapability.parse({
    url: `${SANDBOX_ORIGIN}/sandbox/token-${number}-${'0'.repeat(48)}`,
    expires_at: new Date(Date.now() + lifetimeMs).toISOString(),
  });
}

/**
 * Render `element` with the app's query cache and a router, under a page heading as the reader
 * has one; returns the cache.
 */
export async function renderViewer(element: ReactNode) {
  const queryClient = createQueryClient();
  const router = createRouter({
    routeTree: createRootRoute({
      component: () => (
        <QueryClientProvider client={queryClient}>
          <main>
            <h1 tabIndex={-1}>Annual report</h1>
            {element}
          </main>
        </QueryClientProvider>
      ),
    }),
    history: createMemoryHistory({ initialEntries: ['/somewhere'] }),
  });
  const view = render(<RouterProvider router={router} />);
  await screen.findByRole('heading', { level: 1 });
  return { queryClient, router, ...view };
}

/**
 * Every value the `sandbox` attribute of any frame under `root` has had since now, the attribute's
 * absence included (as `null`): frames already there, frames added and attributes changed.
 */
export function watchSandboxAttributes(root: Node): {
  seen: Array<string | null>;
  stop: () => void;
} {
  const seen: Array<string | null> = [];
  const look = (node: Node) => {
    if (!(node instanceof Element)) return;
    const frames = node.matches('iframe') ? [node] : [...node.querySelectorAll('iframe')];
    for (const frame of frames) seen.push(frame.getAttribute('sandbox'));
  };
  look(root);
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      if (record.type === 'attributes') look(record.target);
      for (const added of record.addedNodes) look(added);
    }
  });
  observer.observe(root, { subtree: true, childList: true, attributes: true });
  return {
    seen,
    stop: () => {
      for (const record of observer.takeRecords()) {
        if (record.type === 'attributes') look(record.target);
        for (const added of record.addedNodes) look(added);
      }
      observer.disconnect();
    },
  };
}
