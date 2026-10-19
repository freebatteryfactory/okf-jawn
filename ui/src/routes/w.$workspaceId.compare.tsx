/**
 * A comparison of two exact versions of the workspace, or of one item: the revisions and the item
 * are in the search. The workspace route's Explorer shell renders the page in place of the
 * documents; this route only makes the URL addressable.
 */
import { createFileRoute } from '@tanstack/react-router';
import { validateComparisonSearch } from '../features/history/ComparisonPage';

export const Route = createFileRoute('/w/$workspaceId/compare')({
  validateSearch: validateComparisonSearch,
  component: () => null,
});
