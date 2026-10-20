/**
 * This workspace (UI M4c, rulings C3, C4, C7 and C9). A placeholder of the Administration skeleton: the lane's builder of rulings C3, C4, C7 and C9 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function WorkspaceLifecycle(_props: { workspaceId: string }) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h3 id={headingId}>This workspace</h3>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
