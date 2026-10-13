/**
 * The files chosen for import in this page, and how the upload of each stands.
 *
 * This is ephemeral UI state and lasts as long as the page: the File objects the browser handed
 * over, each one's progress, and what its last try threw. It lives outside the import page's
 * components so that an upload goes on, and its list is still there, when the person looks at
 * something else in the workspace and comes back. The slot the service holds for a file is the
 * service's; what is kept of it here is the identity needed to finish it and to start the import
 * (`uploadId`), and the keys under which its requests are repeated.
 *
 * A file larger than `MAX_FILE_BYTES` is listed as too large and is never read or sent. A file of
 * a dropped folder that the browser named and could not hand over is listed by its path as not
 * readable: there is no File for it, so nothing of it can be read or sent. A file whose path the
 * page can see is wrong (`paths.ts`: a `..` step, a root, a drive, a control character, a length
 * over the contract's) is listed as one that cannot be imported, and is never read or sent.
 *
 * Two listed files whose paths are the same place to a person (`collisionKey`: the same path, or
 * one that differs only in capital and small letters) collide: neither is sent while both are
 * listed. The one that is still only listed is held back; one whose upload had already begun
 * goes on, and is marked too. Removing either frees the other.
 *
 * Beside the files: the jobs started from this page, by identity only; their state is read from
 * the service (`features/jobs`).
 */
import { create } from 'zustand';
import type { ChosenFile, NamedPath } from './files';
import { collisionKey, type PathProblem, pathProblem } from './paths';

/**
 * The largest file this page uploads: 512 MiB.
 *
 * Finishing an upload needs the file's SHA-256 (`complete_upload`), and the browser's hash takes
 * the whole content as one buffer (`hash.ts`), so a file is held in memory while it is uploaded:
 * its length, and about twice that while it is hashed. For this bound that is about 1 GiB at the
 * peak and 512 MiB while the file is sent (`hash.ts` and `uploading.ts` say what is held and what
 * bounds it across a list). The generated client states no bound (`CreateUploadRequest.size` is
 * any decimal count) and no operation reports the service's, so this bound is the page's own,
 * chosen and not measured: it is larger than any document the workspace is designed around, and
 * its peak is what a browser tab can be expected to hold on an ordinary computer, not what every
 * tab can. A file the service itself finds too large is refused there and shown as such.
 */
export const MAX_FILE_BYTES = 512 * 1024 * 1024;

/** Where a try of an upload stood when it failed. */
export type UploadStep = 'read' | 'slot' | 'bytes' | 'finish';

export type UploadState =
  /** Listed; nothing has been read or sent. */
  | 'chosen'
  /** Asked to upload; its turn has not come. */
  | 'waiting'
  /** Being read for its hash, or waiting to be: one file is read at a time. */
  | 'reading'
  | 'sending'
  /** Every byte is with the service; `complete_upload` is asked. */
  | 'finishing'
  /** The service verified its length and hash. */
  | 'uploaded'
  | 'failed'
  /** Larger than `MAX_FILE_BYTES`: never read, never sent. */
  | 'too-large'
  /** Named by a drop, and the browser could not hand it over: no File, so never read or sent. */
  | 'unreadable'
  /** Its path is one the page does not send (`problem` says why): never read, never sent. */
  | 'unacceptable';

export interface UploadEntry extends NamedPath {
  /** This page's own identity for the entry. */
  id: string;
  workspaceId: string;
  /** The file as the browser handed it over; null only for an `unreadable` entry. */
  file: File | null;
  /** The file's length in bytes, as the browser reported it when it was chosen; 0 when unknown. */
  size: number;
  state: UploadState;
  /** What is wrong with its path, for an `unacceptable` entry; null for every other. */
  problem: PathProblem | null;
  /** Bytes the service holds or the browser reports gone, for the progress shown. */
  sent: number;
  /**
   * What the last try threw, and where it stood. `resumed`: bytes had been sent to its slot by an
   * earlier try, so the slot may hold more than it says.
   */
  failure: { error: unknown; step: UploadStep; resumed: boolean } | null;
  /**
   * Bytes have been sent to the slot of `slotKey` and the service did not refuse them: it holds,
   * or may hold, some of this file.
   */
  slotBegan: boolean;
  /** Slots this file was sent to and that were then left for a new one: they stay with the service. */
  leftBehind: number;
  /** The key `create_upload` is repeated under: the slot's identity while the slot can be used. */
  slotKey: string;
  /** The key `complete_upload` is repeated under, for that slot. */
  finishKey: string;
  /** The file's hash, once read; announced with the slot and sent again to finish it. */
  sha256: string | null;
  /** The slot, once the service has verified the whole file. */
  uploadId: string | null;
}

/** States in which requests of an entry are, or are about to be, in flight. */
const UNDER_WAY: ReadonlySet<UploadState> = new Set(['waiting', 'reading', 'sending', 'finishing']);

export const underWay = (entry: UploadEntry) => UNDER_WAY.has(entry.state);

/** States of an entry that is listed and will never be sent as it is. */
const NEVER_SENT: ReadonlySet<UploadState> = new Set(['too-large', 'unreadable', 'unacceptable']);

export const neverSent = (entry: UploadEntry) => NEVER_SENT.has(entry.state);

/**
 * The listed files that collide, each with the others of its place: files of one workspace that
 * could be sent and whose paths have one `collisionKey`. A file that is never sent collides with
 * nothing.
 */
export function collisions(entries: readonly UploadEntry[]): Map<string, UploadEntry[]> {
  const places = new Map<string, UploadEntry[]>();
  for (const entry of entries) {
    if (neverSent(entry)) continue;
    const place = `${entry.workspaceId}\n${collisionKey(entry)}`;
    places.set(place, [...(places.get(place) ?? []), entry]);
  }
  const colliding = new Map<string, UploadEntry[]>();
  for (const sharing of places.values()) {
    if (sharing.length < 2) continue;
    for (const entry of sharing) {
      colliding.set(
        entry.id,
        sharing.filter((other) => other.id !== entry.id),
      );
    }
  }
  return colliding;
}

/** A listed file that the Upload button would send: not yet asked for, and not held back. */
export function toBeSent(entries: readonly UploadEntry[]): UploadEntry[] {
  const colliding = collisions(entries);
  return entries.filter((entry) => entry.state === 'chosen' && !colliding.has(entry.id));
}

interface UploadsState {
  entries: UploadEntry[];
  /** By workspace: the jobs started from this page, newest first. */
  started: Record<string, string[]>;
  /** By job started from this page: the revision the workspace showed when it was started. */
  startedUnder: Record<string, string>;
  /**
   * List these files for a workspace, and the files of a drop that could not be read, each as not
   * readable. A file already listed there (the same path, length and modification time; for one
   * that could not be read, the same path) is not listed twice. Returns how many files were
   * added, how many entries were already listed, and how many were newly listed as not readable.
   */
  add: (
    workspaceId: string,
    chosen: readonly ChosenFile[],
    unreadable?: readonly NamedPath[],
  ) => { added: number; repeated: number; unreadable: number };
  change: (id: string, change: Partial<UploadEntry>) => void;
  remove: (ids: readonly string[]) => void;
  jobStarted: (workspaceId: string, jobId: string, shown: string) => void;
}

const samePath = (entry: UploadEntry, named: NamedPath) =>
  entry.folder === named.folder && entry.filename === named.filename;

const sameFile = (entry: UploadEntry, chosen: ChosenFile) =>
  samePath(entry, chosen) &&
  entry.file !== null &&
  entry.size === chosen.file.size &&
  entry.file.lastModified === chosen.file.lastModified;

/** What every new entry starts with. */
const fresh = () => ({
  id: crypto.randomUUID(),
  sent: 0,
  failure: null,
  slotBegan: false,
  leftBehind: 0,
  problem: null,
  slotKey: crypto.randomUUID(),
  finishKey: crypto.randomUUID(),
  sha256: null,
  uploadId: null,
});

export const useUploads = create<UploadsState>()((set, get) => ({
  entries: [],
  started: {},
  startedUnder: {},
  add: (workspaceId, chosen, unreadable = []) => {
    const entries = [...get().entries];
    let added = 0;
    for (const file of chosen) {
      const listed = entries.some(
        (entry) => entry.workspaceId === workspaceId && sameFile(entry, file),
      );
      if (listed) continue;
      added += 1;
      const problem = pathProblem(file);
      entries.push({
        ...fresh(),
        ...file,
        workspaceId,
        size: file.file.size,
        problem,
        state:
          problem !== null
            ? 'unacceptable'
            : file.file.size > MAX_FILE_BYTES
              ? 'too-large'
              : 'chosen',
      });
    }
    let repeated = chosen.length - added;
    let notReadable = 0;
    for (const named of unreadable) {
      const listed = entries.some(
        (entry) =>
          entry.workspaceId === workspaceId &&
          entry.state === 'unreadable' &&
          samePath(entry, named),
      );
      if (listed) {
        repeated += 1;
        continue;
      }
      notReadable += 1;
      entries.push({ ...fresh(), ...named, workspaceId, file: null, size: 0, state: 'unreadable' });
    }
    set({ entries });
    return { added, repeated, unreadable: notReadable };
  },
  change: (id, change) =>
    set((state) => ({
      entries: state.entries.map((entry) => (entry.id === id ? { ...entry, ...change } : entry)),
    })),
  remove: (ids) =>
    set((state) => ({ entries: state.entries.filter((entry) => !ids.includes(entry.id)) })),
  jobStarted: (workspaceId, jobId, shown) =>
    set((state) => ({
      startedUnder: { ...state.startedUnder, [jobId]: shown },
      started: {
        ...state.started,
        [workspaceId]: [
          jobId,
          ...(state.started[workspaceId] ?? []).filter((started) => started !== jobId),
        ],
      },
    })),
}));

/** The entries of one workspace, in the order they were chosen. */
export function useWorkspaceUploads(workspaceId: string): UploadEntry[] {
  const entries = useUploads((state) => state.entries);
  return entries.filter((entry) => entry.workspaceId === workspaceId);
}
