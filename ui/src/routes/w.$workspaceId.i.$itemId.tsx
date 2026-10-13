/**
 * An open item: its identity is in the path, its selection and cited revision in the search.
 * The workspace route's Explorer shell renders it; this route only makes the URL addressable.
 */
import { createFileRoute } from '@tanstack/react-router';
import { validateItemSearch } from '../features/explorer/search-params';

export const Route = createFileRoute('/w/$workspaceId/i/$itemId')({
  validateSearch: validateItemSearch,
  component: () => null,
});
