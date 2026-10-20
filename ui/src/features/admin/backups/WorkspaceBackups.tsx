/**
 * Backups (UI M4c, ruling C6). A placeholder of the Administration skeleton: the lane's builder of ruling C6 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function WorkspaceBackups(_props: { workspaceId: string }) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h3 id={headingId}>Backups</h3>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
