/** Display actual server state and honest unavailable states, never fixture workspaces. */
import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { listWorkspacesOptions } from '../api/generated/@tanstack/react-query.gen';

export const Route = createFileRoute('/')({ component: Home });

function Home() {
  const result = useQuery(listWorkspacesOptions({ body: { page: { limit: 100 } } }));
  return <section className="home">
    <p className="eyebrow">YOUR WORKSPACES</p><h1>Make the context visible.</h1>
    <p>This foundation contains the real contracts and build machinery. The persistent application implementation is not included yet.</p>
    {result.isPending && <p role="status">Contacting the workspace service…</p>}
    {result.isError && <p role="alert">The workspace service is unavailable. Start the implemented server before using document operations. No sample data has been substituted.</p>}
    {result.data && <ul>{result.data.items.map(workspace => <li key={workspace.id}><strong>{workspace.name}</strong><p>{workspace.description}</p></li>)}</ul>}
  </section>;
}
