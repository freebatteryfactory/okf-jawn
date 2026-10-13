/**
 * What an open editor becomes when a later read says its item is not a Note any more (a Source or
 * a View): the service would refuse a draft of it, so nothing is saved (`refuseSaving`), the page
 * says so in one line, and the person's text stays on screen, read-only and selectable, so that
 * nothing typed is lost. The text is in the session until the page is left; the box takes
 * keyboard focus (a read-only text area) so it can be scrolled and selected without a pointer.
 */
import type { Ref } from 'react';
import type { DraftSession } from './sessions';

export interface KeptTextProps {
  session: DraftSession;
  title: string;
  /** What the item is now, as the person reads it: "Source" or "View". */
  kind: string;
  /** The box that holds the text, so focus that was in the editor can be given to it. */
  boxRef: Ref<HTMLTextAreaElement>;
}

export function KeptText({ session, title, kind, boxRef }: KeptTextProps) {
  return (
    <section className="draft-editor" aria-label={`Draft of ${title}, not saved`}>
      <p role="note" className="notice">
        This item is now a {kind}, which is not edited as a draft, so your text is no longer saved.
        It is kept here: copy what you need.
      </p>
      <textarea
        ref={boxRef}
        readOnly
        className="kept-text"
        aria-label="Your text, read only"
        rows={12}
        value={session.body}
      />
    </section>
  );
}
