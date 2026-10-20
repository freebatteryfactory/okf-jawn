/**
 * The HTML sandbox viewer (UI M4a, ruling A3): an HTML original, shown as written from the
 * sandbox origin, never from this page's own (SPEC sections 7 and 11: "Uploaded HTML is hostile
 * data until constrained by actual isolation").
 *
 * What is framed. The original's bytes (`SourceAppearance.object`) when its media type is
 * `text/html` (parameters and case aside, as `original-kind.ts` reads it), and nothing else: not
 * XHTML, not SVG, not text. A derived HTML object would be framed only where a conversion records
 * one, and none does on this wire: `Extraction.digest` names the conversion record (JSON), and a
 * read's `MediaReference` is a page render or a picture (`AssetRole`). So nothing derived is
 * framed, and the viewer asks a capability for the original's digest alone.
 *
 * How. `create_sandbox_capability` mints a short-lived address on the sandbox origin, bound to the
 * workspace, the item, the exact revision and that digest, just before it is framed. The frame is
 * an `<iframe sandbox="">` with no token at all (no script, form, popup, top navigation, same
 * origin or download) and `referrerpolicy="no-referrer"`, and its `src` is exactly the
 * capability's `url`. The sandbox origin adds its own isolation on top (`crates/server`
 * `sandbox.rs`: `Content-Security-Policy: sandbox`, `default-src 'none'`, `frame-ancestors` this
 * origin); the frame's own sandbox holds even without it.
 *
 * A capability is a credential. It lives in this component's state while it is framed, and
 * nowhere else: the mutation that mints it hands it to that state and answers nothing, so neither
 * the query cache nor the mutation cache holds it, and it is never written to the browser's
 * storage. It is never reused: a reload mints a new one, whether the person asks for it or the
 * capability is past its `expires_at` (the page says so when it is: the frame keeps what it
 * loaded, and only a reload needs a new address). Each new capability is a new frame element
 * (keyed by its address), so no entry of an older address is left in the frame's history. A late
 * answer for a revision the reader no longer shows, or for a reload since replaced, frames
 * nothing.
 *
 * The service's address is framed only when it is an `http` or `https` address on another origin
 * than this page's: the sandbox origin is never the application's (`SandboxOriginConfig`), and an
 * answer that breaks that is said in words, never framed.
 *
 * What the file's links and forms do. The frame's sandbox stops its forms, its windows and any
 * move of this page; the page policy's `frame-src` (`crates/server` `assets.rs`) stops a link to
 * another site, so no link reaches one. What a refused link leaves in the frame is the browser's:
 * WebKit keeps the file, Chromium shows its refusal page in the frame. The page cannot tell which
 * (a cross-origin frame fires `load` in both cases, the file still shown or not), so it says
 * nothing about it; "Reload the page" shows the file again. Nothing is minted until the person
 * reloads, so a file that keeps reloading itself cannot make the page ask for capabilities.
 *
 * Failures read through `FailureNotice` and `useRetry`; a frame already shown stays beside the
 * failure of its reload. The reload button keeps keyboard focus while it mints (`aria-disabled`).
 */
import { useMutation } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { createSandboxCapability } from '../../../api/generated/sdk.gen';
import type { SandboxCapability } from '../../../api/generated/types.gen';
import { Instant } from '../../../lib/instant';
import { FailureNotice } from '../../errors/FailureNotice';
import type { FailureMessage } from '../../errors/failure';
import { useRetry } from '../../errors/retry';
import { originalKind } from '../original-kind';
import type { OriginalProps } from '../types';

/** What one capability is minted for: the exact object of one item revision. */
interface Binding {
  workspaceId: string;
  itemId: string;
  revision: string;
  object: string;
}

/** The capability framed now, and the binding it was minted for. */
interface Framed {
  capability: SandboxCapability;
  binding: string;
}

/** The service answered an address the page does not frame (see the module header). */
class NotFramed extends Error {
  constructor() {
    super('The sandbox capability is not on a separate http or https origin');
    this.name = 'NotFramed';
  }
}

const NOT_FRAMED: FailureMessage = {
  title: 'The service answered an address this page does not frame.',
  advice:
    "A file's HTML is shown only from the separate sandbox origin, never from this page's own origin. Nothing was framed.",
  action: { kind: 'retry', label: 'Try again' },
};

/** The longest wait a timer takes; a capability that lives longer is looked at again then. */
const LONGEST_TIMER_MS = 2_147_483_647;

function keyOf(binding: Binding): string {
  return JSON.stringify([binding.workspaceId, binding.itemId, binding.revision, binding.object]);
}

/** Whether `url` may be framed: an absolute http or https address on another origin. */
function frameable(url: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  return (
    (parsed.protocol === 'http:' || parsed.protocol === 'https:') &&
    parsed.origin !== window.location.origin
  );
}

/** The file's name as it was supplied, else the last part of its path. */
function fileName({ appearance, path }: OriginalProps): string {
  const supplied = appearance.names[0]?.filename;
  if (supplied !== undefined && supplied.length > 0) return supplied;
  return path.split('/').pop() ?? path;
}

export function HtmlSandbox(props: OriginalProps) {
  if (originalKind(props.appearance.media_type) !== 'html') {
    return (
      <p role="note">
        Only an HTML file (<code>text/html</code>) is shown in the sandbox. This file is{' '}
        <code>{props.appearance.media_type}</code>, so nothing is framed here.
      </p>
    );
  }
  return (
    <SandboxFrame
      // Returning to an earlier binding must mint again, never frame its old state first.
      key={keyOf({
        workspaceId: props.workspaceId,
        itemId: props.itemId,
        revision: props.revision,
        object: props.appearance.object,
      })}
      workspaceId={props.workspaceId}
      itemId={props.itemId}
      revision={props.revision}
      object={props.appearance.object}
      name={fileName(props)}
    />
  );
}

function SandboxFrame({ name, ...binding }: Binding & { name: string }) {
  const { workspaceId, itemId, revision, object } = binding;
  const key = keyOf(binding);
  const [framed, setFramed] = useState<Framed | null>(null);
  // The address whose `expires_at` has passed while it was framed.
  const [expired, setExpired] = useState<string | null>(null);
  const reloadRef = useRef<HTMLButtonElement | null>(null);
  // The last minting asked for: only its answer is framed.
  const asked = useRef(0);

  const mint = useMutation({
    mutationFn: async (wanted: Binding): Promise<void> => {
      asked.current += 1;
      const number = asked.current;
      const { data } = await createSandboxCapability({
        body: {
          workspace_id: wanted.workspaceId,
          item_id: wanted.itemId,
          revision: wanted.revision,
          object: wanted.object,
        },
        throwOnError: true,
      });
      if (!frameable(data.url)) throw new NotFramed();
      // Into this component's state, and nowhere else: the mutation itself answers nothing.
      if (number === asked.current) setFramed({ capability: data, binding: keyOf(wanted) });
    },
  });
  const { mutate } = mint;

  // A capability for each object shown, minted when it is to be framed.
  useEffect(() => {
    mutate({ workspaceId, itemId, revision, object });
  }, [workspaceId, itemId, revision, object, mutate]);

  const shown = framed !== null && framed.binding === key ? framed.capability : null;

  // Past `expires_at`, the page says so; nothing is minted until a reload.
  useEffect(() => {
    if (shown === null) return;
    const until = Date.parse(shown.expires_at);
    if (Number.isNaN(until)) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const look = () => {
      const left = until - Date.now();
      if (left <= 0) setExpired(shown.url);
      else timer = setTimeout(look, Math.min(left, LONGEST_TIMER_MS));
    };
    look();
    return () => clearTimeout(timer);
  }, [shown]);

  const mine = mint.variables !== undefined && keyOf(mint.variables) === key;
  const failure = useRetry({
    request: key,
    failed: mine && mint.isError,
    error: mine ? mint.error : null,
    run: () => mint.mutateAsync({ workspaceId, itemId, revision, object }),
    loaded: () => reloadRef.current,
  });
  const minting = mine && mint.isPending;
  const isExpired = shown !== null && expired === shown.url;

  return (
    <section aria-label={`${name} in the sandbox`} className="grid gap-2">
      <p role="note">
        This is the file as written, shown in a sandbox on a separate origin. Scripts in it do not
        run, and its links and forms do nothing: none of them reaches another site or changes this
        page. Pictures, styles and fonts from elsewhere are not loaded.
      </p>
      {failure.shown && (
        <FailureNotice
          context="Opening the page in the sandbox"
          error={failure.error}
          retry={failure}
          words={failure.error instanceof NotFramed ? NOT_FRAMED : undefined}
        />
      )}
      {shown === null && !failure.shown && <p role="status">Opening the page in the sandbox…</p>}
      {shown !== null && (
        <>
          <iframe
            key={shown.url}
            src={shown.url}
            sandbox=""
            referrerPolicy="no-referrer"
            title={`${name}, as written, in the sandbox`}
            className="h-[32rem] w-full rounded border border-[color:GrayText] bg-white"
          />
          <p>
            <button
              type="button"
              ref={reloadRef}
              aria-disabled={minting}
              onClick={() => {
                if (!minting) mutate({ workspaceId, itemId, revision, object });
              }}
            >
              {minting ? 'Reloading the page…' : 'Reload the page'}
            </button>{' '}
            <span role="status" className="meta">
              {isExpired ? (
                <>
                  Its address on the sandbox origin expired at <Instant value={shown.expires_at} />,
                  and reloading asks for a new one.
                </>
              ) : (
                <>
                  Its address on the sandbox origin is good until{' '}
                  <Instant value={shown.expires_at} />.
                </>
              )}
            </span>
          </p>
        </>
      )}
    </section>
  );
}
