/**
 * The page tools of an open workspace, offered to the browser while the WebMCP setting is on
 * (ruling B5). The Explorer shell mounts it; nothing else registers a tool.
 *
 * Registration follows the setting (`features/settings/webmcp-setting.ts`) and the workspace:
 * it starts when the setting is on and the browser offers WebMCP; turning the setting off, opening
 * another workspace, leaving for the workspace list or the shell unmounting aborts every
 * registration (the draft's only way to unregister); turning the setting on again registers anew.
 * The tools read the shell's state as it was last committed (`latest`), at the moment each runs.
 *
 * What the browser answered is kept for the Settings page in a small store of page state: the
 * tools it took, and those it refused with its words. It is this page's knowledge of its own
 * registrations, not something the service holds.
 */
import { useEffect, useLayoutEffect, useRef } from 'react';
import { create } from 'zustand';
import { useWebMcpSetting } from '../features/settings/webmcp-setting';
import { modelContext, registerPageTools } from './register';
import { type PageState, pageTools } from './tools';

interface Registrations {
  /** The tools the browser took in the registration now standing. */
  taken: string[];
  /** The tools it refused, with its words. */
  refused: { name: string; reason: string }[];
  begin: () => void;
  took: (name: string) => void;
  refuse: (name: string, reason: string) => void;
}

/** What the browser answered the standing registration; empty while there is none. */
export const useRegistrations = create<Registrations>()((set) => ({
  taken: [],
  refused: [],
  begin: () => set({ taken: [], refused: [] }),
  took: (name) => set((state) => ({ taken: [...state.taken, name] })),
  refuse: (name, reason) => set((state) => ({ refused: [...state.refused, { name, reason }] })),
}));

/** Offer the page tools over `state` while the setting is on. */
export function usePageTools(state: PageState) {
  const on = useWebMcpSetting();
  const latest = useRef(state);
  useLayoutEffect(() => {
    latest.current = state;
  });
  const { workspaceId } = state;
  useEffect(() => {
    if (!on || modelContext() === null) return;
    const controller = new AbortController();
    const { begin, took, refuse } = useRegistrations.getState();
    begin();
    registerPageTools(
      pageTools(() => latest.current, workspaceId, controller.signal),
      {
        signal: controller.signal,
        onRegistered: took,
        onRefused: refuse,
      },
    );
    return () => {
      controller.abort();
      useRegistrations.getState().begin();
    };
  }, [on, workspaceId]);
}
