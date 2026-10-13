/** A workspace with no item open: the Explorer shell shows its folders and an empty document area. */
import { createFileRoute } from '@tanstack/react-router';

export const Route = createFileRoute('/w/$workspaceId/')({ component: () => null });
