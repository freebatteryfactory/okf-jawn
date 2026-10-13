/**
 * What happens to uploads when the page goes away.
 *
 * - Looking at something else in the workspace (an item, the jobs, another workspace): nothing is
 *   asked and nothing stops. The uploads run outside the import page (`uploading.ts`) and their
 *   list is there on return.
 * - Reloading, or closing the browser tab, while a file is under way, or uploaded and not yet
 *   imported: the browser asks before leaving (`beforeunload`). Leaving ends the requests in
 *   flight, and the page alone knows which slots it filled, so an import could no longer be
 *   started for them: the files would have to be sent again. With only files that were listed and
 *   never sent, or nothing listed, the page leaves without a question.
 * - A hidden page goes on uploading: nothing depends on it being looked at.
 */
import { useEffect } from 'react';
import { uploadsAtRisk } from './uploading';

export function useUploadLeaveGuard() {
  useEffect(() => {
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (uploadsAtRisk()) event.preventDefault();
    };
    window.addEventListener('beforeunload', beforeUnload);
    return () => window.removeEventListener('beforeunload', beforeUnload);
  }, []);
}
