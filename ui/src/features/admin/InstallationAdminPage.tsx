/**
 * The installation's Administration (ruling C1): create a workspace, back up the installation,
 * its jobs and its security events. It asks with the installation's (tenant) operations only; a
 * workspace's own administration is on that workspace's page.
 *
 * Reached from the workspace list (`InstallationAdminLink`). The contract names no tenant grant a
 * client can read, so the page does not decide who may act here: each action is offered, the
 * service decides, and its refusal is said in words (ruling C2). Like every page outside a
 * workspace it has a level-one heading that takes focus from script (`lib/focus.ts`); each section's
 * heading is a level-two heading.
 */
import { Link } from '@tanstack/react-router';
import { InstallationEvents } from './events/InstallationEvents';
import { InstallationBackups } from './installation/InstallationBackups';
import { InstallationJobs } from './installation/InstallationJobs';
import { NewWorkspace } from './installation/NewWorkspace';

export function InstallationAdminPage() {
  return (
    <section className="home admin-page" aria-labelledby="admin-heading">
      <h1 id="admin-heading" tabIndex={-1}>
        Administration of this installation
      </h1>
      <p>
        <Link to="/">Your workspaces</Link>
      </p>
      <NewWorkspace />
      <InstallationBackups />
      <InstallationJobs />
      <InstallationEvents />
    </section>
  );
}
