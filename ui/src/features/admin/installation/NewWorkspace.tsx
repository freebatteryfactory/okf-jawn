/**
 * New workspace (UI M4c, ruling C9). A placeholder of the Administration skeleton: the lane's builder of ruling C9 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function NewWorkspace() {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h2 id={headingId}>New workspace</h2>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
