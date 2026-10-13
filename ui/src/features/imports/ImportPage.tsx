/**
 * Bringing files in: choose them, see them listed with their paths and sizes, upload them, and
 * start the import of the uploaded ones.
 *
 * Choosing. Two buttons open the browser's own pickers (several files; a whole folder), and the
 * same area takes a drop of files and folders, and of nothing else: a drag that carries no file (a
 * row of the folder tree) is not marked or taken. Every drag of files is cancelled once, by the
 * application's root (`drop-guard.ts`): over this box that is what lets a browser deliver the
 * drop, and anywhere else it keeps the file from opening in place of the app. The buttons are the
 * keyboard's way; the drop is the pointer's. Each file is listed with the path it came in and its size before anything is sent,
 * and can be removed. A file larger than this page uploads is listed as too large and never sent.
 * A file of a dropped folder that the browser could not hand over is listed by its path as not
 * readable, and never sent; the other files of the folder are listed as usual. A polite status
 * says how many were listed, and what of a drop was not: a file that could not be read, a folder
 * that is empty (nothing is kept of it), a folder that could not be listed in full. While a dropped
 * folder is being read the same status says so and how many files were found so far (every
 * `PROGRESS_EVERY` files, not each one); a drop of more than `MAX_DROP_FILES` files, or of folders
 * nested deeper than `MAX_DROP_DEPTH`, lists what was read and says plainly that there was more.
 *
 * Paths the person did not mean (`paths.ts`). A file whose path the page can see is wrong is
 * listed as one that cannot be imported, with the reason, and is never sent. Two files whose paths
 * are the same place (the same, or different only in capital and small letters) are both marked,
 * and the Upload button sends neither until one is removed. A name is drawn with every character
 * that is not drawn itself shown as a mark of its code point (`shownName`), so a right-to-left
 * override cannot make a name read as another; what is sent is the name itself.
 *
 * What stays with the service. The client cannot remove an upload from the service. So a file
 * whose upload was left for a new one says, on its row, that the earlier one stays with the
 * service until it expires; and removing a file some of which was sent says the same in the
 * list's status.
 *
 * Uploading starts when the person asks. Each file shows how it stands, in words: its own percent
 * while it is sent, and "uploaded" only once the service has verified its length and hash. A file
 * that failed keeps its place, with its failure in plain words and its retry (`useRetry`); the
 * others go on. One polite status says how the whole upload stands. Its words change when a file
 * ends and at each tenth of the bytes, not at every report of progress.
 *
 * Starting the import of the uploaded files, and the jobs started here, are `ImportStart.tsx`.
 *
 * Keyboard focus. The pickers' buttons, "Upload" and each "Remove" are always there or are
 * followed: a row that leaves with focus in it (its file was removed) hands focus to the row now
 * in its place, else the last row, else "Choose files…". "Upload" never leaves; with nothing to
 * upload it is unavailable (`aria-disabled`), not gone.
 */
import { type DragEvent, type RefObject, useId, useLayoutEffect, useRef, useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { type HeldFocus, takeHeld, useWatchedPart } from '../../lib/focus';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import {
  type ChosenFile,
  carriesFiles,
  chosenFromDrop,
  chosenFromPicker,
  type Dropped,
  describeSize,
  MAX_DROP_DEPTH,
  MAX_DROP_FILES,
  pathOf,
} from './files';
import { ImportStart, StartedJobs } from './ImportStart';
import { describeProblem, shownName } from './paths';
import {
  collisions,
  MAX_FILE_BYTES,
  neverSent,
  toBeSent,
  type UploadEntry,
  underWay,
  useUploads,
  useWorkspaceUploads,
} from './upload-list';
import { uploadWords } from './upload-words';
import { removeUpload, retryUpload, uploadChosen } from './uploading';

export interface ImportPageProps {
  workspaceId: string;
  /** The revision the workspace shows: an import is based on it. */
  head: Revision;
  /** The person may write in this workspace: only then can files be brought in. */
  canWrite: boolean;
}

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** Folders named in a status: the first few by name, the rest counted. */
const NAMED_FOLDERS = 3;

function folders(paths: readonly string[]): string {
  const named = paths.slice(0, NAMED_FOLDERS).map((path) => `“${shownName(path)}”`);
  const rest = paths.length - named.length;
  return rest > 0 ? `${named.join(', ')} and ${rest} more` : named.join(', ');
}

/** What of a drop is not a file to send. */
type DropRemainder = Pick<
  Dropped,
  'unreadable' | 'emptyFolders' | 'unlistedFolders' | 'tooMany' | 'tooDeep'
>;

const NOTHING_LEFT: DropRemainder = {
  unreadable: [],
  emptyFolders: [],
  unlistedFolders: [],
  tooMany: false,
  tooDeep: false,
};

/** The status while a dropped folder is being read. */
export function describeReading(filesFound: number): string {
  return filesFound === 0
    ? 'Reading the dropped folder…'
    : `Reading the dropped folder… ${count(filesFound, 'file', 'files')} found so far.`;
}

/**
 * What choosing did, for the polite status. `unreadable`: how many files that could not be read
 * were newly listed as such.
 */
function describeChoice(
  { added, repeated, unreadable }: { added: number; repeated: number; unreadable: number },
  rest: Pick<Dropped, 'emptyFolders' | 'unlistedFolders' | 'tooMany' | 'tooDeep'>,
): string {
  const parts = [
    added === 0 ? 'No file was added to the list.' : `${count(added, 'file', 'files')} added.`,
  ];
  if (repeated > 0) {
    parts.push(`${count(repeated, 'was', 'were')} already in the list.`);
  }
  if (unreadable > 0) {
    const n = unreadable;
    parts.push(
      `${count(n, 'file', 'files')} could not be read: ${n === 1 ? 'it is' : 'they are'} listed as not readable and will not be sent.`,
    );
  }
  if (rest.emptyFolders.length > 0) {
    const n = rest.emptyFolders.length;
    parts.push(
      `The ${n === 1 ? 'folder' : 'folders'} ${folders(rest.emptyFolders)} ${n === 1 ? 'is' : 'are'} empty: an empty folder is not kept.`,
    );
  }
  if (rest.unlistedFolders.length > 0) {
    const n = rest.unlistedFolders.length;
    parts.push(
      `The ${n === 1 ? 'folder' : 'folders'} ${folders(rest.unlistedFolders)} could not be read in full: only what was read of ${n === 1 ? 'it' : 'them'} is listed.`,
    );
  }
  if (rest.tooMany) {
    parts.push(
      `The drop holds more than this page reads at once: only the first ${MAX_DROP_FILES.toLocaleString('en-US')} files are listed, and the rest were not read. Drop the rest in smaller parts.`,
    );
  }
  if (rest.tooDeep) {
    parts.push(`Folders nested more than ${MAX_DROP_DEPTH} levels deep were not read.`);
  }
  return parts.join(' ');
}

/** How the whole upload stands, in words that change rarely. */
export function describeUploads(entries: readonly UploadEntry[]): string {
  const asked = entries.filter((entry) => entry.state !== 'chosen' && !neverSent(entry));
  if (asked.length === 0) return '';
  const done = asked.filter((entry) => entry.state === 'uploaded').length;
  const failed = asked.filter((entry) => entry.state === 'failed').length;
  if (asked.some(underWay)) {
    const bytes = asked.reduce((sum, entry) => sum + entry.size, 0);
    const sent = asked.reduce(
      (sum, entry) => sum + (entry.state === 'uploaded' ? entry.size : entry.sent),
      0,
    );
    const tenths = bytes === 0 ? 0 : Math.floor((10 * sent) / bytes) * 10;
    return `Uploading: ${done} of ${count(asked.length, 'file', 'files')} done${failed > 0 ? `, ${failed} failed` : ''}; about ${tenths}% of the bytes sent.`;
  }
  if (failed === 0) {
    return asked.length === 1
      ? 'The file is uploaded and checked.'
      : `All ${asked.length} files are uploaded and checked.`;
  }
  return `${done} of ${count(asked.length, 'file', 'files')} uploaded; ${failed} not uploaded. Each of those has its own notice and retry.`;
}

export function ImportPage({ workspaceId, head, canWrite }: ImportPageProps) {
  const heading = useId();
  const listHeading = useId();
  const entries = useWorkspaceUploads(workspaceId);
  const add = useUploads((state) => state.add);
  const [choiceNote, setChoiceNote] = useState('');
  const [over, setOver] = useState(false);
  const filesInput = useRef<HTMLInputElement | null>(null);
  const folderInput = useRef<HTMLInputElement | null>(null);
  const chooseRef = useRef<HTMLButtonElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);

  const list = (chosen: readonly ChosenFile[], rest: DropRemainder = NOTHING_LEFT) => {
    setChoiceNote(describeChoice(add(workspaceId, chosen, rest.unreadable), rest));
  };
  const picked = (input: HTMLInputElement) => {
    list(chosenFromPicker(input.files ?? []));
    // The same file can be chosen again later: the picker forgets what it held.
    input.value = '';
  };
  const dropped = (event: DragEvent<HTMLFieldSetElement>) => {
    // A drag that carries no file (a row of the folder tree) is not this box's: nothing is taken.
    // The application's root cancels the drop itself (`drop-guard.ts`); this box reads it.
    if (!carriesFiles(event.dataTransfer)) return;
    setOver(false);
    void chosenFromDrop(event.dataTransfer, (found) => setChoiceNote(describeReading(found))).then(
      (found) => list(found.files, found),
    );
  };

  // A row that left with keyboard focus in it: focus goes to the row now in its place.
  const held: HeldFocus = useRef<HTMLElement | null>(null);
  const removedAt = useRef(0);
  useLayoutEffect(() => {
    if (!takeHeld(held)) return;
    const buttons = listRef.current?.querySelectorAll<HTMLElement>('[data-remove]');
    const next = buttons?.item(Math.min(removedAt.current, (buttons?.length ?? 0) - 1));
    (next ?? chooseRef.current)?.focus();
  });

  const colliding = collisions(entries);
  const toUpload = toBeSent(entries).length;

  if (!canWrite) {
    return (
      <section className="workspace-page" aria-labelledby={heading}>
        <h2 id={heading} tabIndex={-1} className="page-heading">
          Import files
        </h2>
        <p role="note" className="notice">
          You may read this workspace, but not add to it. Ask its owner for permission to write.
        </p>
      </section>
    );
  }

  return (
    <section className="workspace-page" aria-labelledby={heading}>
      <h2 id={heading} tabIndex={-1} className="page-heading">
        Import files
      </h2>
      <p>
        Choose files, or a whole folder: each file keeps the path it comes in. Nothing is sent until
        you press Upload.
      </p>
      {/* The drop is the pointer's way in; the two buttons inside are the keyboard's. */}
      <fieldset
        className={over ? 'drop-target drop-target-over' : 'drop-target'}
        onDragOver={(event) => {
          if (!carriesFiles(event.dataTransfer)) return;
          // The application's root cancels the move (`drop-guard.ts`), which is what makes a
          // browser deliver the drop; here the pointer is made to say that files are taken.
          event.dataTransfer.dropEffect = 'copy';
          setOver(true);
        }}
        onDragLeave={() => setOver(false)}
        onDrop={dropped}
      >
        <legend className="sr-only">Choose what to import</legend>
        <button type="button" ref={chooseRef} onClick={() => filesInput.current?.click()}>
          Choose files…
        </button>{' '}
        <button type="button" onClick={() => folderInput.current?.click()}>
          Choose a folder…
        </button>{' '}
        <span className="meta">or drop files and folders here.</span>
        <input
          ref={filesInput}
          type="file"
          multiple
          hidden
          aria-label="Files to import"
          onChange={(event) => picked(event.target)}
        />
        <input
          ref={(input) => {
            folderInput.current = input;
            // React has no property for a folder picker; the attribute is what browsers read.
            input?.setAttribute('webkitdirectory', '');
          }}
          type="file"
          multiple
          hidden
          aria-label="Folder to import"
          onChange={(event) => picked(event.target)}
        />
      </fieldset>
      {/* Present from the start, so what choosing did is announced. */}
      <p role="status" className="meta">
        {choiceNote}
      </p>

      <h3 id={listHeading} className="panel-heading">
        Files to import ({entries.length})
      </h3>
      {entries.length === 0 ? (
        <p className="meta">No files are listed yet.</p>
      ) : (
        <ul ref={listRef} className="upload-list" aria-labelledby={listHeading}>
          {entries.map((entry, index) => (
            <UploadRow
              key={entry.id}
              entry={entry}
              collidesWith={colliding.get(entry.id) ?? NONE}
              held={held}
              onRemove={() => {
                removedAt.current = index;
                const path = shownName(pathOf(entry));
                if (removeUpload(entry.id).leftWithService) {
                  setChoiceNote(
                    `“${path}” was removed from the list. What was already sent of it stays with the service until it expires: this page has no way to remove it.`,
                  );
                }
              }}
            />
          ))}
        </ul>
      )}
      <p className="page-actions">
        {/* aria-disabled rather than disabled: the button keeps focus once it has been pressed. */}
        <button
          type="button"
          aria-disabled={toUpload === 0}
          onClick={() => {
            if (toUpload > 0) uploadChosen(workspaceId);
          }}
        >
          Upload {count(toUpload, 'file', 'files')}
        </button>
      </p>
      <p role="status" className="meta">
        {describeUploads(entries)}
      </p>
      <ImportStart workspaceId={workspaceId} head={head} entries={entries} />
      <StartedJobs workspaceId={workspaceId} canWrite={canWrite} />
    </section>
  );
}

const NONE: readonly UploadEntry[] = [];

/** How one file stands, in words; never by color. */
function Standing({ entry }: { entry: UploadEntry }) {
  switch (entry.state) {
    case 'chosen':
      return <>Not sent yet.</>;
    case 'waiting':
      return <>Waiting for its turn.</>;
    case 'reading':
      return <>Reading the file…</>;
    case 'sending':
      return <>Sending: {Math.floor((100 * entry.sent) / entry.size)}%.</>;
    case 'finishing':
      return <>Sent. The service is checking it…</>;
    case 'uploaded':
      return <>Uploaded: the service checked its length and hash.</>;
    case 'failed':
      return <>Not uploaded.</>;
    case 'too-large':
      return (
        <>
          Too large: this page uploads files of up to {describeSize(MAX_FILE_BYTES)}. It will not be
          sent.
        </>
      );
    case 'unreadable':
      return <>Not readable: the browser could not open this file. It will not be sent.</>;
    case 'unacceptable':
      return (
        <>
          Cannot be imported:{' '}
          {entry.problem === null ? 'its path is not one to send.' : describeProblem(entry.problem)}{' '}
          It will not be sent.
        </>
      );
  }
}

/** That a file's path is the same place as another listed file's, and what follows from it. */
function Collision({ entry, others }: { entry: UploadEntry; others: readonly UploadEntry[] }) {
  if (others.length === 0) return null;
  const named = others.map((other) => `“${shownName(pathOf(other))}”`).join(', ');
  return (
    <span className="upload-state">
      {' '}
      Its path is the same as that of {named} in this list, or differs only in capital and small
      letters: both cannot be imported to one place.{' '}
      {entry.state === 'chosen'
        ? 'It is not sent while both are listed: remove one of them.'
        : 'Remove one of them before you import.'}
    </span>
  );
}

function UploadRow({
  entry,
  collidesWith,
  held,
  onRemove,
}: {
  entry: UploadEntry;
  /** The other listed files whose path is the same place as this one's. */
  collidesWith: readonly UploadEntry[];
  held: RefObject<HTMLElement | null>;
  onRemove: () => void;
}) {
  const [, rowRef] = useWatchedPart<HTMLLIElement>(held);
  const removeRef = useRef<HTMLButtonElement | null>(null);
  // The path as it is drawn and named to a screen reader; what is sent is the entry's own.
  const path = shownName(pathOf(entry));
  const failure = useRetry({
    request: entry.id,
    failed: entry.state === 'failed',
    error: entry.failure?.error,
    run: () => retryUpload(entry.id),
    loaded: () => removeRef.current,
  });
  return (
    <li ref={rowRef} className="upload-row">
      {/* Isolated, so a name written right to left does not reorder the words beside it. */}
      <bdi className="upload-path">{path}</bdi>{' '}
      {/* A file that could not be read has no known size. */}
      {entry.file !== null && (
        <>
          <span className="meta" title={`${entry.size} bytes`}>
            {describeSize(entry.size)}
          </span>{' '}
        </>
      )}
      <span className="upload-state">
        <Standing entry={entry} />
      </span>
      <Collision entry={entry} others={collidesWith} />
      {entry.leftBehind > 0 && (
        <span className="upload-state">
          {' '}
          What was sent of it before stays with the service until it expires: this page has no way
          to remove it.
        </span>
      )}{' '}
      {entry.state === 'sending' && (
        <progress max={entry.size} value={entry.sent} aria-label={`${path}: bytes sent`} />
      )}{' '}
      <button
        type="button"
        ref={removeRef}
        data-remove
        aria-label={`Remove ${path}`}
        onClick={onRemove}
      >
        Remove
      </button>
      {failure.shown && (
        <FailureNotice
          context={`Uploading ${path}`}
          error={failure.error}
          retry={failure}
          words={entry.failure === null ? undefined : uploadWords(entry.failure)}
        />
      )}
    </li>
  );
}
