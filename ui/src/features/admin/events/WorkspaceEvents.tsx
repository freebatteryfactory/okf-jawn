/**
 * Security events (UI M4c, ruling C8). A placeholder of the Administration skeleton: the lane's builder of ruling C8 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function WorkspaceEvents(_props: { workspaceId: string }) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h3 id={headingId}>Security events</h3>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
