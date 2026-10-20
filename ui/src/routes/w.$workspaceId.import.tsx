/**
 * Bringing files into a workspace. The workspace route's Explorer shell renders the page in place
 * of the documents; this route only makes the URL addressable.
 */
import { createFileRoute } from '@tanstack/react-router';

export const Route = createFileRoute('/w/$workspaceId/import')({ component: () => null });
