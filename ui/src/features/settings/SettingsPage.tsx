/**
 * Settings: what this browser does for the person, whichever workspace it shows (ruling B6). It
 * holds one setting, WebMCP (`webmcp-setting.ts`): off by default, kept in this browser alone, and
 * it says in plain words what turning it on allows, so nobody turns it on without knowing.
 *
 * It is a page of the workspace's bar like the others (`explorer/WorkspacePages.tsx`), with a
 * level-two heading, but nothing on it is the workspace's: the page says so. The setting is a
 * native checkbox, so Space turns it on and off and keyboard focus stays on it. Its state is the
 * one storage holds: where the browser refuses to keep it, the box stays clear and the page says
 * that it could not be kept. What a change did is said in a polite status, present from the start
 * so that it is announced. Whether the browser offers WebMCP at all is said apart, in a note: the
 * choice is kept either way, and does nothing in a browser that does not offer it.
 */
import { useId, useState } from 'react';
import { keepWebMcpSetting, useWebMcpSetting, webMcpOffered } from './webmcp-setting';

/** The five page tools and what each lets an assistant do, in the person's words. */
const TOOLS = [
  ['get_selection', 'tell it which item you have open, at which exact revision, and which part.'],
  ['open_source', 'open an item for you, at an exact revision.'],
  ['open_comparison', 'open the changes between two revisions for you.'],
  ['preview_names', 'show it what naming rules would rename. It renames nothing.'],
  [
    'prepare_proposal',
    'fill in the Propose change form of the note you have open. It never sends it: only you can press Propose.',
  ],
] as const;

export function SettingsPage() {
  const heading = useId();
  const webmcpHeading = useId();
  const on = useWebMcpSetting();
  const offered = webMcpOffered();
  const [said, setSaid] = useState('');
  const change = (next: boolean) => {
    const kept = keepWebMcpSetting(next);
    if (!kept) {
      setSaid(
        'The setting could not be kept in this browser: it does not let this page keep settings, so the page tools stay off.',
      );
    } else if (!next) {
      setSaid('Turned off: no assistant can use the page tools.');
    } else if (offered) {
      setSaid('Turned on: an assistant in this browser can use the five page tools.');
    } else {
      setSaid('Turned on. This browser does not offer WebMCP, so no tool is offered here.');
    }
  };
  return (
    <section className="workspace-page" aria-labelledby={heading}>
      <h2 id={heading} tabIndex={-1} className="page-heading">
        Settings
      </h2>
      <p className="meta">
        These settings are kept in this browser, not in the workspace: they apply to every workspace
        you open in this browser, and to no other browser.
      </p>
      <section aria-labelledby={webmcpHeading} className="setting">
        <h3 id={webmcpHeading}>Page tools for an assistant (WebMCP)</h3>
        <p>
          <label>
            <input
              type="checkbox"
              checked={on}
              onChange={(event) => change(event.target.checked)}
            />{' '}
            Let an assistant in this browser use the page tools (WebMCP)
          </label>
        </p>
        <p>
          Off unless you turn it on. While it is on and a workspace is open here, an assistant that
          this browser connects to the page through WebMCP can use five tools:
        </p>
        <ul className="setting-tools">
          {TOOLS.map(([name, words]) => (
            <li key={name}>
              <code>{name}</code>: {words}
            </li>
          ))}
        </ul>
        <p>
          They act with your own session, as you: an assistant using them can see and open what you
          can, and nothing more. None of them can verify, approve or review anything. Each works on
          what is on screen when the assistant uses it, at that exact revision, never on “the
          latest”.
        </p>
        <p>It does nothing unless this browser offers WebMCP.</p>
        {!offered && (
          <p role="note" className="notice">
            This browser does not offer WebMCP: no assistant can use these tools here, whatever this
            setting says.
          </p>
        )}
        <p role="status">{said}</p>
      </section>
    </section>
  );
}
