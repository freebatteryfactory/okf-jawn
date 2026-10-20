/**
 * A purge's record (ruling C4): its state and what it removed, readable after the workspace it
 * removed is gone, so it is the installation's page and not a workspace's.
 */
import { createFileRoute } from '@tanstack/react-router';
import { PurgeRecordPage } from '../features/admin/purges/PurgeRecordPage';

export const Route = createFileRoute('/purges/$purgeId')({ component: PurgeRoute });

function PurgeRoute() {
  const { purgeId } = Route.useParams();
  return <PurgeRecordPage purgeId={purgeId} />;
}
