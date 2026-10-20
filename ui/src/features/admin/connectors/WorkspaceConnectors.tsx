/**
 * Connectors (UI M4c, ruling C5). A placeholder of the Administration skeleton: the lane's builder of ruling C5 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function WorkspaceConnectors(_props: { workspaceId: string }) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h3 id={headingId}>Connectors</h3>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
