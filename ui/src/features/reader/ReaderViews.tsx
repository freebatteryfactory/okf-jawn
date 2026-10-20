/**
 * The reader's views of one item (ruling A1): the choice of view (`ViewSwitch`) and the reading
 * of the view chosen, as `read_item` answers it at the pane's revision and selection, within the
 * reader's budgets (`request.ts`), continued through `next_cursor` (`continuation.ts`).
 *
 * - **Text**: the text as sanitized Markdown, in the parts the budget cut it into.
 * - **Outline**: the text's structure; an entry shows its part of the item in the Text view.
 * - **Text and pictures**: the pictures the conversion record keeps for the selection, each
 *   with its caption (or words that it has none) and its place in the original, then the text.
 * - **Pages**: the converter's page renders of the selection, at most `IMAGE_BUDGET` at once,
 *   with the pages before and after offered as selections of their own.
 * - **Original**: the source's own bytes in the viewer their media type goes to
 *   (`viewers/OriginalViewer`), after the read's citation of them.
 *
 * Every view shows what the read says beside its content: whose text it is, its warnings (an
 * image budget that left images out, a selection clamped to the text), whether it is partial and
 * how to continue it, and its exact citation, with each place in the original worded by how it
 * was found (`Locations`: an inferred place is never shown as a direct one).
 *
 * Keyboard focus. The choice of view keeps focus. An outline entry chosen from the keyboard
 * leaves with its view: focus goes to the Text choice at once, and to the first part of the text
 * once it is read, unless the person moved it on meanwhile. "Show pages …" keeps focus on itself
 * while the pages are read; when it leaves (the last pages are shown) focus goes to the other one,
 * or to the first page shown.
 */
import { type RefCallback, useId, useLayoutEffect, useRef } from 'react';
import type {
  ItemDocument,
  MediaReference,
  OutlineEntry,
  PageRegion,
  ReadItemRequest,
  ReadItemResponse,
  ReadView,
  Revision,
  Selection,
} from '../../api/generated/types.gen';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { keyed } from '../../lib/keys';
import { describeSelection, short, textLabels } from '../documents/describe';
import { SafeMarkdown } from '../documents/SafeMarkdown';
import { FailureNotice } from '../errors/FailureNotice';
import type { FailureRetry } from '../errors/retry';
import { MediaImage } from '../viewers/image/MediaImage';
import { OriginalViewer } from '../viewers/OriginalViewer';
import { type Continuation, partOf } from './continuation';
import { Locations } from './Locations';
import { IMAGE_BUDGET } from './request';
import { ViewSwitch } from './ViewSwitch';
import type { ItemViews } from './views';

/** What the reader needs of its read's state. */
export interface ReadState {
  pages: readonly ReadItemResponse[];
  isPending: boolean;
  isFetchingNextPage: boolean;
  isFetchNextPageError: boolean;
  hasNextPage: boolean;
}

export interface ReaderViewsProps {
  workspaceId: string;
  itemId: string;
  revision: Revision;
  /** The item as `get_item` answered it, once it has. */
  document: ItemDocument | undefined;
  views: ItemViews;
  /** The item's properties could not be read: what views it has beyond Text and Outline is not known. */
  viewsUnknown: boolean;
  view: ReadView;
  onView: (view: ReadView) => void;
  request: ReadItemRequest;
  read: ReadState;
  readFailure: FailureRetry;
  moreFailure: FailureRetry;
  continuation: Continuation;
  /** Show another selection of this item (or the whole item). */
  onSelect: (selection: Selection | undefined) => void;
  /** The heading level a document's own `#` is drawn at, when shown under a heading of the page. */
  textHeadingLevel?: 1 | 2 | 3 | 4 | 5 | 6;
  /** An id for the Original region's heading. */
  originalId: string;
}

/** The words of the reading's live region: what the last continuation did. */
function readingStatus(read: ReadState, parts: number, retrying: boolean): string {
  if (read.isFetchingNextPage && !retrying) return 'Reading more…';
  if (read.isFetchNextPageError) return 'The rest of this reading could not be read.';
  if (parts > 1) return `Part ${parts} read${read.hasNextPage ? '; more can be read' : ''}.`;
  return '';
}

const ENTRY_KIND: Record<OutlineEntry['kind'], string> = {
  heading: 'heading',
  table: 'table',
  figure: 'figure',
  page: 'page',
  sheet: 'sheet',
};

/** Keyboard focus is nowhere the person put it: on the page body, or on nothing. */
function focusOnBody(): boolean {
  return document.activeElement === null || document.activeElement === document.body;
}

/** The pages before and after those shown, as selections, when the item has `count` pages. */
export function pageWindows(
  selection: Selection,
  count: number | null,
): { before: Selection | null; after: Selection | null } {
  if (count === null || count < 1) return { before: null, after: null };
  const range =
    selection.kind === 'all'
      ? { start: 1, end: count }
      : selection.kind === 'pages'
        ? selection.range
        : null;
  if (range === null) return { before: null, after: null };
  const start = Math.max(1, range.start);
  const shownEnd = Math.min(range.end, start + IMAGE_BUDGET - 1, count);
  const before =
    start > 1
      ? {
          kind: 'pages' as const,
          range: { start: Math.max(1, start - IMAGE_BUDGET), end: start - 1 },
        }
      : null;
  const after =
    shownEnd < count
      ? {
          kind: 'pages' as const,
          range: { start: shownEnd + 1, end: Math.min(count, shownEnd + IMAGE_BUDGET) },
        }
      : null;
  return { before, after };
}

export function ReaderViews({
  workspaceId,
  itemId,
  revision,
  document: item,
  views,
  viewsUnknown,
  view,
  onView,
  request,
  read,
  readFailure,
  moreFailure,
  continuation,
  onSelect,
  textHeadingLevel,
  originalId,
}: ReaderViewsProps) {
  const pages = read.pages;
  const first = pages.at(0);
  const last = pages.at(-1);
  const switchRef = useRef<HTMLDivElement | null>(null);
  const checkedChoice = () =>
    switchRef.current?.querySelector<HTMLInputElement>('input[type="radio"]:checked') ?? null;

  // An outline entry chosen: its view leaves, and the text of the entry comes.
  const outlineHeld = useRef<HTMLElement | null>(null);
  const [, outlineRef] = useWatchedPart<HTMLElement>(outlineHeld);
  const awaitingText = useRef(false);
  // "Show pages …": the button pressed stays while the pages are read; one that leaves is followed.
  const pagerHeld = useRef<HTMLElement | null>(null);
  const [, beforeRef] = useWatchedPart<HTMLButtonElement>(pagerHeld);
  const [, afterRef] = useWatchedPart<HTMLButtonElement>(pagerHeld);
  const pagerRef = useRef<HTMLParagraphElement | null>(null);
  const body = continuation.bodyRef;
  useLayoutEffect(() => {
    if (takeHeld(outlineHeld)) checkedChoice()?.focus();
    if (awaitingText.current) {
      if (view !== 'text') awaitingText.current = false;
      else if (!read.isPending && pages.length > 0) {
        awaitingText.current = false;
        if (document.activeElement === checkedChoice() || focusOnBody()) {
          partOf(body.current, 'first')?.focus();
        }
      }
    }
    if (takeHeld(pagerHeld)) {
      (
        pagerRef.current?.querySelector<HTMLElement>('button') ??
        partOf(body.current, 'first')?.querySelector<HTMLElement>('figure') ??
        checkedChoice() ??
        pageHeading()
      )?.focus();
    }
  });

  const showInText = (selection: Selection) => {
    awaitingText.current = true;
    onSelect(selection);
    onView('text');
  };

  const shownText = view === 'text' || view === 'multimodal' || view === 'outline';
  const region: PageRegion | null =
    request.selection.kind === 'region' ? request.selection.region : null;
  const pageCount = item?.source?.extraction.page_count ?? null;
  const windows = view === 'pages' ? pageWindows(request.selection, pageCount) : null;
  const pictures = view === 'multimodal' ? pages.flatMap((page) => page.media) : [];
  const warnings = pages.flatMap((page) => page.warnings);
  // Each place the parts cite, once.
  const placed = new Set<string>();
  const locations = pages
    .flatMap((page) => page.source.locations ?? [])
    .filter((location) => {
      const key = JSON.stringify(location);
      if (placed.has(key)) return false;
      placed.add(key);
      return true;
    });
  const picturesId = useId();

  const partContent = (page: ReadItemResponse) => {
    switch (view) {
      case 'text':
      case 'multimodal':
        return <SafeMarkdown markdown={page.markdown} headingLevel={textHeadingLevel ?? 1} />;
      case 'outline':
        return (
          <OutlineList
            entries={page.outline}
            selection={request.selection}
            onChoose={showInText}
            listRef={outlineRef}
          />
        );
      case 'pages':
        return (
          <Gallery
            media={page.media}
            region={region}
            empty="No page render lies in this selection."
          />
        );
      case 'original':
        return <OriginalCitation page={page} />;
    }
  };

  const reading = (
    <>
      {shownText &&
        textLabels(first?.extraction).map((label) => (
          <p key={label} role="note" className="notice">
            {label}
          </p>
        ))}
      {read.isPending && !readFailure.retrying && <p role="status">Reading…</p>}
      {readFailure.shown && (
        <FailureNotice
          context="Reading this item"
          error={readFailure.error}
          retry={readFailure}
          workspaceId={workspaceId}
        />
      )}
      {view === 'multimodal' && first !== undefined && (
        <section aria-labelledby={picturesId} className="reader-pictures grid gap-2">
          <h3 id={picturesId} className="m-0">
            Pictures
          </h3>
          <Gallery media={pictures} region={region} empty="No picture lies in this selection." />
        </section>
      )}
      {windows !== null && (windows.before !== null || windows.after !== null) && (
        <p ref={pagerRef} className="reader-pager flex flex-wrap gap-2">
          {windows.before !== null && (
            <button
              type="button"
              ref={beforeRef}
              onClick={() => windows.before && onSelect(windows.before)}
            >
              Show {describeSelection(windows.before)}
            </button>
          )}
          {windows.after !== null && (
            <button
              type="button"
              ref={afterRef}
              onClick={() => windows.after && onSelect(windows.after)}
            >
              Show {describeSelection(windows.after)}
            </button>
          )}
        </p>
      )}
      {pages.length > 0 && (
        <div className="document-body" ref={body}>
          {keyed(pages, (page) => page.receipt_id).map(({ key, value: page }, index) => (
            <div
              key={key}
              data-part
              tabIndex={-1}
              {...(pages.length > 1
                ? { role: 'group', 'aria-label': `Part ${index + 1} of this reading` }
                : {})}
            >
              {partContent(page)}
            </div>
          ))}
        </div>
      )}
      <p role="status" className="sr-only">
        {readingStatus(read, pages.length, moreFailure.retrying)}
      </p>
      {keyed(warnings, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
        <p key={key} role="note" className="notice">
          {warning.message}
          {warning.location ? ` (${warning.location})` : ''}
        </p>
      ))}
      {last?.truncated && (
        <div role="note" className="notice">
          <p>
            Partial reading: this is not all of {describeSelection(request.selection)}.
            {last.next_cursor ? '' : ' The rest cannot be continued from here.'}
          </p>
          {read.hasNextPage && (
            // aria-disabled rather than disabled while reading: the button stays focusable and
            // announced. A disabled one would lose focus: Chromium and WebKit move focus to the
            // body within a frame of a focused button becoming disabled (HTML's focus fixup).
            <button
              type="button"
              ref={continuation.buttonRef}
              aria-disabled={read.isFetchingNextPage}
              onClick={continuation.continueReading}
            >
              {read.isFetchingNextPage ? 'Reading more…' : 'Continue reading'}
            </button>
          )}
        </div>
      )}
      {moreFailure.shown && (
        <FailureNotice
          context="Continuing this reading"
          error={moreFailure.error}
          retry={moreFailure}
          workspaceId={workspaceId}
        />
      )}
      {first && (
        <div className="citation">
          <p className="m-0">
            Read from {first.source.path} at{' '}
            <code title={first.source.revision}>{short(first.source.revision)}</code>,{' '}
            {pages.map((page) => describeSelection(page.source.selection)).join('; ')}.
          </p>
          <Locations locations={locations} label="Where this reading lies in the original" />
        </div>
      )}
    </>
  );

  return (
    <>
      <div ref={switchRef}>
        <ViewSwitch
          views={views}
          view={view}
          onView={onView}
          unknown={
            viewsUnknown ? (
              <p className="meta m-0">
                Whether this item has pictures, pages or an original is not known until its
                properties are read.
              </p>
            ) : null
          }
        />
      </div>
      {view === 'original' ? (
        <section aria-labelledby={originalId} className="original grid gap-2">
          <h3 id={originalId}>Original</h3>
          {reading}
          {item?.source && (
            <OriginalViewer
              workspaceId={workspaceId}
              itemId={itemId}
              revision={revision}
              path={item.summary.path}
              appearance={item.source}
              selection={request.selection}
            />
          )}
        </section>
      ) : (
        <div className={`reading reading-${view}`}>{reading}</div>
      )}
    </>
  );
}

function OutlineList({
  entries,
  selection,
  onChoose,
  listRef,
}: {
  entries: readonly OutlineEntry[];
  selection: Selection;
  onChoose: (selection: Selection) => void;
  listRef: RefCallback<HTMLElement>;
}) {
  if (entries.length === 0) {
    return (
      <p>
        {selection.kind === 'all'
          ? 'This text has no headings, tables, figures, pages or sheets to outline.'
          : `Nothing of the outline lies in ${describeSelection(selection)}.`}
      </p>
    );
  }
  return (
    <nav ref={listRef} aria-label="Outline of this item">
      <ol className="reader-outline">
        {keyed(entries, (entry) => JSON.stringify(entry)).map(({ key, value: entry }) => (
          <li key={key} style={{ marginInlineStart: `${entry.level}rem` }}>
            <button type="button" className="link-button" onClick={() => onChoose(entry.selection)}>
              {entry.label}
            </button>{' '}
            <span className="meta">
              ({ENTRY_KIND[entry.kind]}, {describeSelection(entry.selection)})
            </span>
          </li>
        ))}
      </ol>
    </nav>
  );
}

function Gallery({
  media,
  region,
  empty,
}: {
  media: readonly MediaReference[];
  region: PageRegion | null;
  empty: string;
}) {
  if (media.length === 0) return <p>{empty}</p>;
  return (
    <ul className="reader-gallery m-0 grid list-none gap-4 p-0 [grid-template-columns:repeat(auto-fill,minmax(min(100%,16rem),1fr))]">
      {keyed(media, (image) => `${image.object}/${JSON.stringify(image.source.selection)}`).map(
        ({ key, value: image }) => (
          <li key={key}>
            <MediaImage media={image} region={region} />
          </li>
        ),
      )}
    </ul>
  );
}

function OriginalCitation({ page }: { page: ReadItemResponse }) {
  const digest = page.source.digest;
  return (
    <p>
      The original bytes of {page.source.path}
      {digest ? (
        <>
          {' '}
          (<code title={digest}>{short(digest)}</code>)
        </>
      ) : null}
      , {describeSelection(page.source.selection)}, as they were stored.
    </p>
  );
}
