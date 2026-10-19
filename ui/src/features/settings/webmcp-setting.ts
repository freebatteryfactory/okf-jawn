/**
 * The WebMCP setting: whether this browser offers the page tools to an assistant (SPEC section 9:
 * "WebMCP is optional by capability detection and an explicit setting"; ruling B6).
 *
 * It belongs to the browser, not to a workspace or a person: it is kept in this origin's
 * `localStorage`, which only this browser reads. It is off unless the stored value says "on". The
 * stored value is the one source of truth: nothing holds a copy, so the Settings page and the
 * Explorer shell read it from storage whenever it may have changed (`useWebMcpSetting`, through
 * `useSyncExternalStore`), and both follow a change without a reload, whether it was made on this
 * page or in another tab of the browser (a `storage` event).
 *
 * Storage is read and written in try/catch. A browser can refuse a page its storage (blocked site
 * data: reading `window.localStorage` then throws) or refuse a write (a full quota): a page that
 * cannot read the setting acts as off, and a write that did not stick reports that it was not
 * kept, so the setting stays as it was there. Whether the browser offers WebMCP at all is another
 * question (`webMcpOffered`): the setting may be kept in a browser that does not offer it, and
 * does nothing there.
 */
import { useSyncExternalStore } from 'react';

/** The key the setting is kept under in this origin's `localStorage`. */
export const WEBMCP_SETTING_KEY = 'okf-jawn.webmcp';
const ON = 'on';

/** Whether the setting is on in this browser; off where it cannot be read. */
export function readWebMcpSetting(): boolean {
  try {
    return window.localStorage.getItem(WEBMCP_SETTING_KEY) === ON;
  } catch {
    return false;
  }
}

/** Parts of this page that follow the setting: a write made here sends them no `storage` event. */
const followers = new Set<() => void>();

/**
 * Keep the setting in this browser. Returns whether storage now holds what was asked: false when
 * the browser refused the write, and the setting is then as it was (off, where nothing is kept).
 */
export function keepWebMcpSetting(on: boolean): boolean {
  try {
    if (on) window.localStorage.setItem(WEBMCP_SETTING_KEY, ON);
    else window.localStorage.removeItem(WEBMCP_SETTING_KEY);
  } catch {
    // Refused: what storage holds is read back below.
  }
  for (const changed of followers) changed();
  return readWebMcpSetting() === on;
}

function follow(changed: () => void) {
  const fromAnotherTab = (event: StorageEvent) => {
    // A null key: the other tab cleared this origin's storage.
    if (event.key === null || event.key === WEBMCP_SETTING_KEY) changed();
  };
  followers.add(changed);
  window.addEventListener('storage', fromAnotherTab);
  return () => {
    followers.delete(changed);
    window.removeEventListener('storage', fromAnotherTab);
  };
}

/** The setting as this browser keeps it now, following every change. */
export function useWebMcpSetting(): boolean {
  return useSyncExternalStore(follow, readWebMcpSetting, () => false);
}

/** Whether this browser offers WebMCP: a model context on the document (the W3C draft). */
export function webMcpOffered(): boolean {
  const context: unknown = Reflect.get(document, 'modelContext');
  return context !== undefined && context !== null;
}
