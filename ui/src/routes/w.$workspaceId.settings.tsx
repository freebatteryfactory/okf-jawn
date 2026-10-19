/**
 * The settings of this browser, reached from a workspace's bar. The workspace route's Explorer
 * shell renders the page in place of the documents; this route only makes the URL addressable.
 */
import { createFileRoute } from '@tanstack/react-router';

export const Route = createFileRoute('/w/$workspaceId/settings')({ component: () => null });
