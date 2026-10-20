/**
 * Security events of the installation (UI M4c, ruling C8). A placeholder of the Administration skeleton: the lane's builder of ruling C8 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function InstallationEvents() {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h2 id={headingId}>Security events of the installation</h2>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
