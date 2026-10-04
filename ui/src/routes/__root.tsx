/** One root layout for the browser workspace; no independent chat state exists. */
import { createRootRoute, Link, Outlet } from '@tanstack/react-router';

export const Route = createRootRoute({ component: Root });

function Root() {
  return (
    <>
      <header className="app-header">
        <Link to="/" className="wordmark">
          okf-jawn
        </Link>
        <span>A document workspace for people and their AI tools.</span>
      </header>
      <main>
        <Outlet />
      </main>
    </>
  );
}
