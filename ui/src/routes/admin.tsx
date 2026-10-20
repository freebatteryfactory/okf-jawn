/** The installation's Administration (ruling C1), reached from the workspace list. */
import { createFileRoute } from '@tanstack/react-router';
import { InstallationAdminPage } from '../features/admin/InstallationAdminPage';

export const Route = createFileRoute('/admin')({ component: InstallationAdminPage });
