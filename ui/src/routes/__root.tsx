/**
 * One root layout for the browser workspace; no independent chat state exists.
 *
 * What must hold on every route is mounted here: a file dropped anywhere but on the import page's
 * drop box never opens in place of the app (`useFileDropGuard`), and the browser asks before the
 * tab is closed or reloaded while an upload would be lost (`useUploadLeaveGuard`).
 */
import { createRootRoute, Link, Outlet } from '@tanstack/react-router';
import { useFileDropGuard } from '../features/imports/drop-guard';
import { useUploadLeaveGuard } from '../features/imports/leave-guard';

export const Route = createRootRoute({ component: Root });

function Root() {
  useFileDropGuard();
  useUploadLeaveGuard();
  return (
    <>
      <a href="#main" className="skip-link">
        Skip to content
      </a>
      <header className="app-header">
        <Link to="/" className="wordmark">
          okf-jawn
        </Link>
        <span>A document workspace for people and their AI tools.</span>
      </header>
      <main id="main" tabIndex={-1} className="app-main">
        <Outlet />
      </main>
    </>
  );
}
