/**
 * What an open editor becomes when a read says its item is not a Note any more (a Source or a
 * View): the session is refused (`saving.ts`), so nothing of it is saved, the page says so in one
 * line, and what the person typed stays on screen, read-only and selectable, until they discard
 * the draft or leave the page.
 *
 * What is shown is everything the session holds of the person's: the text; the properties of the
 * draft, when they differ from the committed ones; and what was entered in the properties form
 * and held back (it did not fit the type, so it is in no save), each property as it was entered,
 * one that was emptied shown as empty. The text is in a read-only text area, which takes keyboard
 * focus so it can be scrolled and selected without a pointer; the properties are plain text.
 *
 * "Discard draft" is offered here, with the question the editor asks (`DiscardDraft`): it is the
 * way to be rid of the draft, and the only request this part of the page sends.
 */
import type { Ref } from 'react';
import { PropertyList } from '../documents/Properties';
import { DiscardDraft } from './DiscardDraft';
import { type DraftSession, sameText } from './sessions';

export interface KeptTextProps {
  session: DraftSession;
  title: string;
  /** What the item is now, as the person reads it: "Source" or "View". */
  kind: string;
  /** The box that holds the text, so focus that was in the editor can be given to it. */
  boxRef: Ref<HTMLTextAreaElement>;
}

const DRAFT_PROPERTIES = 'The properties of your draft, read only';
const HELD_BACK = 'What you entered in the properties and was held back, read only';

export function KeptText({ session, title, kind, boxRef }: KeptTextProps) {
  const committed = session.committed.properties;
  const changed = !sameText(
    { body: '', properties: session.properties },
    { body: '', properties: committed },
  );
  const held = session.heldBack;
  return (
    <section className="draft-editor" aria-label={`Draft of ${title}, not saved`}>
      <p role="note" className="notice">
        This item is now a {kind}, which is not edited as a draft, so your text is no longer saved.
        It is kept here: copy what you need, then discard the draft.
      </p>
      <textarea
        ref={boxRef}
        readOnly
        className="kept-text"
        aria-label="Your text, read only"
        rows={12}
        value={session.body}
      />
      {changed && (
        <fieldset className="kept-properties">
          <legend>{DRAFT_PROPERTIES}</legend>
          <PropertyList properties={session.properties} />
        </fieldset>
      )}
      {held !== null && (
        <fieldset className="kept-properties">
          <legend>{HELD_BACK}</legend>
          <PropertyList
            properties={Object.fromEntries(
              held.entered.map((name) => [name, held.form[name] ?? null]),
            )}
          />
        </fieldset>
      )}
      {/* With the draft gone the session is gone, and the pane shows the item as it is now. */}
      <DiscardDraft session={session} />
    </section>
  );
}
