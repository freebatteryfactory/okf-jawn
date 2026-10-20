/**
 * A purge's record (UI M4c, ruling C4). A placeholder of the Administration skeleton: the lane's
 * builder of C4 replaces it with the page the ruling describes.
 */
import { useRef } from 'react';

export function PurgeRecordPage({ purgeId }: { purgeId: string }) {
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  return (
    <section className="home" aria-labelledby="purge-heading">
      <h1 id="purge-heading" ref={headingRef} tabIndex={-1}>
        Purge {purgeId}
      </h1>
      <p className="meta">Not built yet.</p>
    </section>
  );
}
