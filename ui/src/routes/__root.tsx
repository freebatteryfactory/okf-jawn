/** One root layout for the browser workspace; no independent chat state exists. */
import { createRootRoute, Link, Outlet } from '@tanstack/react-router';

export const Route = createRootRoute({ component: Root });

function Root() {
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
