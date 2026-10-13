/**
 * Read one item: its identity and properties from `get_item`, its text from `read_item` as
 * sanitized Markdown, at one resolved revision and the selection the URL names. A revision is
 * called "the latest" only when a request made for that statement resolved it.
 *
 * A partial read says so and offers its continuation, which resumes the same resolved read;
 * continuing moves keyboard focus to the part it loaded, and a live region says what happened.
 * Warnings, partial extraction and agent-supplied text are shown, never smoothed over. Each
 * operation's failure is shown where its content would have been when there is none to show, and
 * beside the content when there is: a read that fails when it is asked again never takes the
 * properties, the form, the editor or the text off the screen. The notice stays while the read is
 * tried again (`useRetry`), so a retry never drops keyboard focus.
 *
 * Editing happens in place of the reading: one button switches the pane between the committed
 * text (rendered, read through `read_item`) and the person's draft (`DraftEditor`, its text from
 * `get_item`). There is one editor per item on a page: while one pane edits an item, another pane
 * of the same item shows the committed text, says the item is being edited in the other pane, and
 * offers "Edit here", which moves the editor, its text and keyboard focus to this pane. So the
 * two can stand side by side, and two editors never write over one draft. An item that
 * has a draft opens on the draft, marked as one. Editing is for Notes: a Source or a View is shown
 * by the reader alone, with no "Edit"; the kind is the one the last read of the item answered.
 * A draft is started only where the pane reads the version
 * the workspace shows and the person may write; a pane of a Note that reads another revision says
 * so and offers that version. The switch is one element whatever it says, so it keeps keyboard
 * focus; "Edit" moves focus into the text box, and after a discard focus is on the switch.
 */
import {
  type UseQueryResult,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import {
  getItemOptions,
  openWorkspaceQueryKey,
  readItemInfiniteOptions,
} from '../../api/generated/@tanstack/react-query.gen';
import { openWorkspace, readItem } from '../../api/generated/sdk.gen';
import type {
  ItemSummary,
  ReadItemRequest,
  ReadItemResponse,
  Revision,
  Workspace,
} from '../../api/generated/types.gen';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { ConflictNotice } from '../drafts/ConflictNotice';
import { DraftEditor } from '../drafts/DraftEditor';
import { beginSession, claimEditor, releaseEditor } from '../drafts/saving';
import { saveStateOf, useConflict, useDraftSession } from '../drafts/sessions';
import type { Snapshotted } from '../drafts/snapshot';
import { FailureNotice } from '../errors/FailureNotice';
import { type FailureRetry, useRetry } from '../errors/retry';
import type { ItemSelection } from '../explorer/search-params';
import { describeSelection, extractionLabels, short } from './describe';
import { Properties } from './Properties';
import { PropertiesForm } from './PropertiesForm';
import { SafeMarkdown } from './SafeMarkdown';

/** Text budget per read; a longer item is read in parts through the continuation. */
export const READ_BUDGET_BYTES = 65_536;

export interface DocumentViewProps {
  workspaceId: string;
  itemId: string;
  /** The revision to read at: the shell's resolved head, or the exact revision a link names. */
  revision: Revision;
  /** A link names this revision, so the reader says whether it is still the latest. */
  pinned: boolean;
  selection: ItemSelection | undefined;
  /** Show another selection of this item (or the whole item). */
  onSelect: (selection: ItemSelection | undefined) => void;
  /** The latest version was read; `resolved` is the revision the server resolved for it. */
  onReadLatest: (resolved: Revision) => void;
  /** The revision the workspace shows: a draft is started only from a pane that reads it. */
  head: Revision;
  /** The person may write in this workspace. */
  canWrite: boolean;
  /** Open this item as the workspace shows it, without a cited revision. */
  onOpenCurrent: () => void;
  /** A Snapshot of this item was taken here ("keep mine"). */
  onSnapshotted: (snapshotted: Snapshotted) => void;
}

/** The words for a draft that is not on screen: the reader shows the committed text. */
const DRAFT_STANDS = {
  unchanged: 'You started editing this item.',
  unsaved: 'You have unsaved changes to this item.',
  saving: 'Your changes to this item are being saved.',
  saved: 'You have a draft of this item.',
  failed: 'You have changes to this item that are not saved.',
  'too-long': 'You have changes to this item that are not saved.',
  'held-back': 'You changed this item’s properties, and that is held back and not saved.',
} as const;

const ROLE = { note: 'Note', source: 'Source', view: 'View' } as const;

/**
 * The words of the reading's live region: what the last continuation did. While the failure
 * notice is trying the continuation again, the notice says so and this line stays as it was.
 */
function readingStatus(
  read: { isFetchingNextPage: boolean; isFetchNextPageError: boolean; hasNextPage: boolean },
  parts: number,
  retrying: boolean,
): string {
  if (read.isFetchingNextPage && !retrying) return 'Reading more…';
  if (read.isFetchNextPageError) return 'The rest of this reading could not be read.';
  if (parts > 1) return `Part ${parts} read${read.hasNextPage ? '; more can be read' : ''}.`;
  return '';
}

/** A part of the reading on screen, as an element keyboard focus can go to. */
function partOf(body: HTMLDivElement | null, which: 'first' | 'last'): HTMLElement | null {
  const part = which === 'first' ? body?.firstElementChild : body?.lastElementChild;
  return part instanceof HTMLElement ? part : null;
}

export function itemOptions(workspaceId: string, itemId: string, revision: Revision) {
  return getItemOptions({
    body: { workspace_id: workspaceId, item_id: itemId, at: { kind: 'revision', revision } },
  });
}

/**
 * A pinned revision, compared with a head fetched for this comparison (never a cached one), and
 * the way to the latest version: a read with `at: latest`, which the server resolves once. Its
 * answer is kept as the first part of a read pinned to the revision it resolved, so the pane,
 * the URL and the continuation all follow that one read.
 *
 * The answer moves the pane only while this reader is the one on screen. A reader is keyed by its
 * tab, so when the person goes to another tab before the answer, this one is gone: its request is
 * aborted, and an answer that arrives all the same moves nothing and takes no focus.
 */
function PinnedRevision({
  workspaceId,
  revision,
  request,
  onReadLatest,
}: {
  workspaceId: string;
  revision: Revision;
  request: ReadItemRequest;
  onReadLatest: (resolved: Revision) => void;
}) {
  const client = useQueryClient();
  const noticeRef = useRef<HTMLDivElement | null>(null);
  const body = { workspace_id: workspaceId };
  const latestHead = useQuery({
    queryKey: [{ ...openWorkspaceQueryKey({ body })[0], _comparedWith: revision }],
    queryFn: async ({ signal }) => {
      const { data } = await openWorkspace({ body, signal, throwOnError: true });
      return data;
    },
    staleTime: 0,
    refetchOnWindowFocus: true,
  });
  // Whether this reader is on screen, and the request it has in flight.
  const onScreen = useRef(false);
  const asked = useRef<AbortController | null>(null);
  useEffect(() => {
    onScreen.current = true;
    return () => {
      onScreen.current = false;
      asked.current?.abort();
    };
  }, []);
  const readLatest = useMutation({
    mutationFn: async () => {
      const controller = new AbortController();
      asked.current = controller;
      const { data } = await readItem({
        body: { ...request, at: { kind: 'latest' }, cursor: null },
        signal: controller.signal,
        throwOnError: true,
      });
      return data;
    },
    // Whatever the page shows by now, the answer is the first part of that read: only the cache
    // is told here. (The library runs this after the component is gone, too.)
    onSuccess: (response) => {
      const resolved = response.source.revision;
      const pinned: ReadItemRequest = { ...request, at: { kind: 'revision', revision: resolved } };
      client.setQueryData(readItemInfiniteOptions({ body: pinned }).queryKey, {
        pages: [response],
        pageParams: [{ body: pinned }],
      });
    },
  });
  /** The answer is here: the pane goes to the revision it resolved, if this reader is still shown. */
  const arrive = (response: ReadItemResponse) => {
    if (!onScreen.current) return;
    // The button goes once the pane reads that revision; keyboard focus stays at this notice,
    // which then says which revision is read, instead of dropping to the page.
    const notice = noticeRef.current;
    if (notice?.contains(document.activeElement)) notice.focus();
    onReadLatest(response.source.revision);
  };
  // Either failure keeps its notice through a retry, which ends with keyboard focus at this
  // notice: it then says what the check found, or which revision was read.
  const checkFailure = useRetry({
    request: revision,
    failed: latestHead.isError,
    error: latestHead.error,
    run: () => latestHead.refetch(),
    loaded: () => noticeRef.current,
  });
  const latestFailure = useRetry({
    request: revision,
    failed: readLatest.isError,
    error: readLatest.error,
    run: () => readLatest.mutateAsync().then(arrive),
    loaded: () => noticeRef.current,
  });

  return (
    <div ref={noticeRef} tabIndex={-1} className="pinned-revision">
      <PinnedRevisionState
        workspaceId={workspaceId}
        revision={revision}
        latestHead={latestHead}
        reading={readLatest.isPending}
        onRead={() => readLatest.mutate(undefined, { onSuccess: arrive })}
        checkFailure={checkFailure}
        latestFailure={latestFailure}
      />
    </div>
  );
}

function PinnedRevisionState({
  workspaceId,
  revision,
  latestHead,
  reading: readingLatest,
  onRead,
  checkFailure,
  latestFailure,
}: {
  workspaceId: string;
  revision: Revision;
  latestHead: UseQueryResult<Workspace>;
  /** The latest version is being read. */
  reading: boolean;
  /** Read the latest version. */
  onRead: () => void;
  checkFailure: FailureRetry;
  latestFailure: FailureRetry;
}) {
  const reading = <code title={revision}>{short(revision)}</code>;
  if (checkFailure.shown) {
    return (
      <FailureNotice
        context="Checking for the latest version"
        error={checkFailure.error}
        retry={checkFailure}
        workspaceId={workspaceId}
      />
    );
  }
  const latest = latestHead.data;
  if (latest === undefined) {
    return (
      <p role="note" className="notice">
        You are reading revision {reading}. Checking whether it is the latest version…
      </p>
    );
  }
  const checked = <Instant value={new Date(latestHead.dataUpdatedAt).toISOString()} />;
  if (latest.head === revision) {
    return (
      <p role="note" className="notice">
        You are reading revision {reading}, the latest version when checked at {checked}.
      </p>
    );
  }
  return (
    <div role="note" className="notice">
      <p>
        You are reading revision {reading}; it is not the latest version (checked at {checked}).{' '}
        <button
          type="button"
          aria-disabled={readingLatest}
          onClick={() => {
            if (!readingLatest) onRead();
          }}
        >
          {readingLatest ? 'Reading the latest version…' : 'Read the latest version'}
        </button>
      </p>
      {latestFailure.shown && (
        <FailureNotice
          context="Reading the latest version"
          error={latestFailure.error}
          retry={latestFailure}
          workspaceId={workspaceId}
        />
      )}
    </div>
  );
}

export function DocumentView({
  workspaceId,
  itemId,
  revision,
  pinned,
  selection,
  onSelect,
  onReadLatest,
  head,
  canWrite,
  onOpenCurrent,
  onSnapshotted,
}: DocumentViewProps) {
  const titleId = useId();
  const item = useQuery(itemOptions(workspaceId, itemId, revision));
  const document = item.data;
  const conflict = useConflict(workspaceId, itemId);

  // Reading or editing. Until the person chooses, an item with a draft opens on the draft.
  const session = useDraftSession(workspaceId, itemId);
  // Editing is for Notes: a Source's text is changed through corrections and a View has its own
  // editor, so neither is offered "Edit". What kind the item is, is what the last `get_item` that
  // answered said. It is read with every read, not kept from the first: no sentence of SPEC or of
  // the contract says an item's kind stays what it was (`ItemKind` is its "built-in rendering
  // role", a field of each answer). While the item is read again at another revision the last
  // answer stands, so the switch does not leave under keyboard focus for the time of a read. A
  // session is begun only for a Note, and while there is one its editor stays, with its text.
  const atShown = canWrite && revision === head;
  const [kind, setKind] = useState<ItemSummary['kind'] | null>(null);
  if (document !== undefined && kind !== document.summary.kind) setKind(document.summary.kind);
  // The item's type, kept the same way: the form is drawn from it while the item is read again.
  const [typeName, setTypeName] = useState<string | null>(null);
  if (document !== undefined && typeName !== document.summary.type_name) {
    setTypeName(document.summary.type_name);
  }
  const isNote = session !== undefined || kind === 'note';
  const editable = atShown && isNote;
  const hasDraft = session !== undefined || (document?.draft ?? null) !== null;
  const [chosen, setChosen] = useState<'read' | 'edit' | null>(null);
  const wantsEdit = editable && (chosen ?? (hasDraft ? 'edit' : 'read')) === 'edit';
  // A draft the service holds is opened as it arrives. Without one, a session starts only when
  // the person presses "Edit": a discarded draft is not started again behind their back.
  const heldDraft = document?.draft ?? null;
  // One editor per item on a page: this pane's own name, and whether another pane has the editor.
  const pane = useId();
  const editor = session?.editor ?? null;
  const elsewhere = session !== undefined && editor !== null && editor !== pane;
  useEffect(() => {
    if (editable && session === undefined && document && heldDraft !== null) {
      beginSession(workspaceId, revision, document, pane);
    }
  }, [editable, session, document, heldDraft, workspaceId, revision, pane]);
  // A pane that wants the editor takes it when no pane has it (the one that had it was closed or
  // went back to reading), and gives it up when it stops wanting it or goes.
  const sessionOpen = session !== undefined;
  useEffect(() => {
    if (!sessionOpen) return;
    if (wantsEdit && editor === null) claimEditor(workspaceId, itemId, pane);
    else if (!wantsEdit && editor === pane) releaseEditor(workspaceId, itemId, pane);
  }, [sessionOpen, wantsEdit, editor, workspaceId, itemId, pane]);
  useEffect(() => () => releaseEditor(workspaceId, itemId, pane), [workspaceId, itemId, pane]);
  const editing = wantsEdit && session !== undefined && editor === pane;
  // Whether this pane opens on a draft is known once `get_item` has answered. Until then the
  // committed text is neither read nor shown, so it never stands where the draft will.
  const undecided =
    atShown && chosen === null && session === undefined && document === undefined && !item.isError;
  const request: ReadItemRequest = {
    workspace_id: workspaceId,
    item_id: itemId,
    at: { kind: 'revision', revision },
    view: 'text',
    selection: selection ?? { kind: 'all' },
    cursor: null,
    max_bytes: READ_BUDGET_BYTES,
    max_images: 0,
  };
  const read = useInfiniteQuery({
    ...readItemInfiniteOptions({ body: request }),
    initialPageParam: { body: request },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // The editor shows the draft's own text; the committed text is read when it is shown.
    enabled: !(wantsEdit && !elsewhere) && !undecided,
  });

  const pages = read.data?.pages ?? [];
  const first = pages[0];
  const last = pages.at(-1);
  const summary = document?.summary;
  const path = summary?.path ?? first?.source.path;
  const title = summary?.title || path || 'Item';
  const warnings = pages.flatMap((page) => page.warnings);
  const extraction = first?.extraction;

  // "Continue reading" moves keyboard focus to the part it loaded. If none arrived, focus stays
  // on the button, which is still there, and the status says what happened.
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const continueRef = useRef<HTMLButtonElement | null>(null);
  const focusPart = useRef<number | null>(null);
  const continuing = read.isFetchingNextPage;
  const continuationFailed = read.isFetchNextPageError;
  const partCount = pages.length;
  useEffect(() => {
    const part = focusPart.current;
    if (part === null) return;
    // Settled once the asked-for part is here, or the continuation failed.
    if (partCount > part) {
      focusPart.current = null;
      const loaded = bodyRef.current?.children.item(part);
      if (loaded instanceof HTMLElement) loaded.focus();
    } else if (continuationFailed && !continuing) {
      focusPart.current = null;
      continueRef.current?.focus();
    }
  }, [partCount, continuing, continuationFailed]);
  const continueReading = () => {
    if (continuing) return;
    focusPart.current = pages.length;
    void read.fetchNextPage();
  };

  // Each operation's failure keeps its notice through a retry, which ends with keyboard focus on
  // what it loaded: the properties' disclosure, the first part of the text, or the part continued.
  // A retry belongs to what this view reads: once the revision or the selection changes, a retry
  // still in flight is no longer shown.
  const articleRef = useRef<HTMLElement | null>(null);
  // Where keyboard focus goes once the pane has switched: into the text box after "Edit", onto
  // the switch after a discard (the editor and its buttons are gone then).
  const switchRef = useRef<HTMLButtonElement | null>(null);
  const focusNext = useRef<'text' | 'switch' | null>(null);
  useEffect(() => {
    const wanted = focusNext.current;
    if (wanted === null) return;
    const target =
      wanted === 'switch'
        ? switchRef.current
        : articleRef.current?.querySelector<HTMLElement>('.draft-editor [role="textbox"]');
    if (target instanceof HTMLElement) {
      focusNext.current = null;
      target.focus();
    }
  });
  // The editor and the form leave when the session ends by another hand than this pane's own
  // buttons (a Snapshot taken in the Drafts panel). Keyboard focus that was in them goes to the
  // pane's switch, as after the other ways out of the editor; with no switch, to the page's heading.
  const heldFocus = useRef<HTMLElement | null>(null);
  const [, editingRef] = useWatchedPart<HTMLDivElement>(heldFocus);
  // The switch itself leaves when the pane stops being editable (a read says the item is not a
  // Note any more, or the pane no longer reads the version shown). Focus on it goes to the page's
  // heading.
  const heldSwitch = useRef<HTMLElement | null>(null);
  const [, actionsRef] = useWatchedPart<HTMLParagraphElement>(heldSwitch);
  useLayoutEffect(() => {
    if (takeHeld(heldFocus)) (switchRef.current ?? pageHeading())?.focus();
    if (takeHeld(heldSwitch)) pageHeading()?.focus();
  });
  const reading = JSON.stringify(request);
  const itemFailure = useRetry({
    request: reading,
    failed: item.isError,
    error: item.error,
    run: () => item.refetch(),
    loaded: () => articleRef.current?.querySelector<HTMLElement>('.properties > summary'),
  });
  const readFailure = useRetry({
    request: reading,
    failed: read.isError && !read.isFetchNextPageError,
    error: read.error,
    run: () => read.refetch(),
    loaded: () => partOf(bodyRef.current, 'first'),
  });
  const moreFailure = useRetry({
    request: reading,
    failed: read.isFetchNextPageError,
    error: read.error,
    run: () => read.fetchNextPage(),
    loaded: () => partOf(bodyRef.current, 'last'),
  });

  return (
    <article ref={articleRef} className="document" aria-labelledby={titleId}>
      <header className="document-header">
        <h2 id={titleId}>{title}</h2>
        <p className="meta">
          {path ? <span>{path}</span> : null}
          {summary ? (
            <>
              {' · '}
              {ROLE[summary.kind]}
              {' · type '}
              <span className="type-name">{summary.type_name}</span>
              {' · status '}
              {summary.status}
              {summary.archived ? ' · archived' : ''}
            </>
          ) : null}
          {' · revision '}
          <code title={revision}>{short(revision)}</code>
        </p>
        {summary?.description ? <p>{summary.description}</p> : null}
      </header>

      {pinned && (
        <PinnedRevision
          workspaceId={workspaceId}
          revision={revision}
          request={request}
          onReadLatest={onReadLatest}
        />
      )}
      {canWrite && !atShown && isNote && (
        <p role="note" className="notice">
          {hasDraft ? 'You have a draft of this item. ' : ''}
          Editing starts from the version this workspace shows, and this tab reads another revision.{' '}
          <button type="button" onClick={onOpenCurrent}>
            Open the version this workspace shows
          </button>
        </p>
      )}
      {editable && (
        <p ref={actionsRef} className="document-actions">
          {/* aria-disabled, not disabled, while the item is read again: it may hold focus. */}
          <button
            type="button"
            ref={switchRef}
            aria-disabled={session === undefined && document === undefined}
            onClick={() => {
              if (editing) setChosen('read');
              else if (session !== undefined || document !== undefined) {
                if (session === undefined && document !== undefined) {
                  beginSession(workspaceId, revision, document, pane);
                }
                // "Edit here" moves the editor from the pane that has it to this one.
                claimEditor(workspaceId, itemId, pane, true);
                focusNext.current = 'text';
                setChosen('edit');
              }
            }}
          >
            {editing
              ? 'Read the committed text'
              : elsewhere
                ? 'Edit here'
                : hasDraft
                  ? 'Edit your draft'
                  : 'Edit'}
          </button>
        </p>
      )}
      {editable && conflict !== undefined && (
        <ConflictNotice
          workspaceId={workspaceId}
          itemId={itemId}
          title={title}
          conflict={conflict}
          onSnapshotted={onSnapshotted}
          onResolved={() => {
            focusNext.current = 'switch';
            setChosen('read');
          }}
        />
      )}
      {editable && !editing && session !== undefined && (
        <p role="note" className="notice">
          {elsewhere
            ? 'This item is being edited in another pane.'
            : DRAFT_STANDS[saveStateOf(session)]}{' '}
          Below is the committed text.
        </p>
      )}
      {selection && !editing && (
        <p role="note" className="notice">
          Showing {describeSelection(selection)}.{' '}
          <button type="button" onClick={() => onSelect(undefined)}>
            Show the whole item
          </button>
        </p>
      )}

      {/* A read that fails when asked again never takes away what is on screen: the notice
          stands beside the properties, the form and the text, and replaces them only when there
          is nothing to show. */}
      {itemFailure.shown && (
        <FailureNotice
          context="Reading this item’s properties"
          error={itemFailure.error}
          retry={itemFailure}
          workspaceId={workspaceId}
        />
      )}
      {editing ? (
        <div ref={editingRef} className="editing">
          {typeName === null ? null : <PropertiesForm session={session} typeName={typeName} />}
          <DraftEditor
            session={session}
            title={title}
            onDiscarded={() => {
              focusNext.current = 'switch';
              setChosen('read');
            }}
          />
        </div>
      ) : (
        <>
          {document ? (
            <Properties properties={document.properties} />
          ) : itemFailure.shown ? null : (
            <p role="status">Loading properties…</p>
          )}
          {first && first.outline.length > 0 && (
            <details className="outline">
              <summary>Outline ({first.outline.length})</summary>
              <nav aria-label="Outline">
                <ol>
                  {keyed(first.outline, (entry) => JSON.stringify(entry)).map(
                    ({ key, value: entry }) => (
                      <li key={key} style={{ marginInlineStart: `${entry.level}rem` }}>
                        <button
                          type="button"
                          className="link-button"
                          onClick={() => onSelect(entry.selection)}
                        >
                          {entry.label}
                        </button>
                      </li>
                    ),
                  )}
                </ol>
              </nav>
            </details>
          )}

          {extractionLabels(extraction).map((label) => (
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
          {pages.length > 0 && (
            <div className="document-body" ref={bodyRef}>
              {keyed(pages, (page) => page.receipt_id).map(({ key, value: page }, index) => (
                <div
                  key={key}
                  data-part
                  tabIndex={-1}
                  {...(pages.length > 1
                    ? { role: 'group', 'aria-label': `Part ${index + 1} of this reading` }
                    : {})}
                >
                  <SafeMarkdown markdown={page.markdown} />
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
                  ref={continueRef}
                  aria-disabled={continuing}
                  onClick={continueReading}
                >
                  {continuing ? 'Reading more…' : 'Continue reading'}
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
            <p className="citation">
              Read from {first.source.path} at{' '}
              <code title={first.source.revision}>{short(first.source.revision)}</code>,{' '}
              {describeSelection(first.source.selection)}.
            </p>
          )}
        </>
      )}
    </article>
  );
}
