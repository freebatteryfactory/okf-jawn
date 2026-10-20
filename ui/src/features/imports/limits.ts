/**
 * The limits this deployment applies (`get_session`, ruling 4), read once while the page is open:
 * the most bytes of one upload, so that a file over it is refused before anything of it is read or
 * sent. The value is a decimal string on the wire (a 64-bit count); one that is not a count is no
 * limit the page can apply, and the service then refuses a file over its limit itself.
 */
import { getSessionOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { SessionResponse } from '../../api/generated/types.gen';

/** The session, as long as the page is open: its limits do not change while it runs. */
export function sessionOptions() {
  return { ...getSessionOptions({ body: {} }), staleTime: Number.POSITIVE_INFINITY };
}

/** The most bytes of one upload the service takes; null when it is not known as a count. */
export function uploadLimitOf(session: SessionResponse | undefined): number | null {
  const stated = session?.limits.upload_bytes;
  if (stated === undefined || !/^\d+$/.test(stated)) return null;
  const bytes = Number(stated);
  return Number.isSafeInteger(bytes) ? bytes : null;
}
