/**
 * Installation jobs (UI M4c, ruling C1). A placeholder of the Administration skeleton: the lane's builder of ruling C1 replaces
 * it with the section the rulings describe.
 */
import { useId } from 'react';

export function InstallationJobs() {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId}>
      <h2 id={headingId}>Installation jobs</h2>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
