/**
 * What happens to typed text when the page goes away.
 *
 * - Reloading, or closing the browser tab, while some text is not confirmed by the service: the
 *   browser asks before leaving (`beforeunload`), and the save is sent at once in case the person
 *   stays. With nothing at risk the page leaves without a question.
 * - The page being hidden (another browser tab, a minimised window): every session is saved at
 *   once, without waiting for the autosave delay.
 *
 * Inside the workspace nothing asks: leaving an editor saves at once and its session keeps the
 * text (`DraftEditor`, `sessions.ts`).
 */
import { useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { anySessionAtRisk, flushAllSessions } from './saving';

export function useLeaveGuard() {
  const client = useQueryClient();
  useEffect(() => {
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (!anySessionAtRisk()) return;
      flushAllSessions(client);
      event.preventDefault();
    };
    const visibility = () => {
      if (document.visibilityState === 'hidden') flushAllSessions(client);
    };
    window.addEventListener('beforeunload', beforeUnload);
    document.addEventListener('visibilitychange', visibility);
    return () => {
      window.removeEventListener('beforeunload', beforeUnload);
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [client]);
}
