/**
 * Backup of the installation (UI M4c, ruling C6). A placeholder of the Administration skeleton: the lane's builder of ruling C6 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function InstallationBackups() {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h2 id={headingId}>Backup of the installation</h2>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
