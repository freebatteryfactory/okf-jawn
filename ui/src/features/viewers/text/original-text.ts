/**
 * The text of a text-like original (ruling A2): its bytes read with the transport's byte read
 * (`app/byte-read.ts`, `download_object` with a range: never `get_object`), a part at a time, and
 * decoded as UTF-8.
 *
 * Bounded. The first part (`ORIGINAL_PART_BYTES`) is read when the text is shown; each further
 * part only when the person asks (`fetchNextPage`), so a large file is never read whole behind
 * their back. TanStack Query holds the parts, keyed by workspace, item, revision, object and
 * part size: an object is named by its SHA-256, so its bytes never change and are never asked
 * again while they are held.
 *
 * Decoding. The parts are joined and decoded together, so a character whose bytes lie across the
 * end of a part is whole once the next part is read. While more is to be read, an incomplete
 * sequence at the very end is held back, not shown as a replacement: it is not known yet to be
 * wrong. What is not UTF-8 is shown as U+FFFD, as the WHATWG decoder does, and counted; whether
 * the file holds bytes that are not UTF-8 at all is found by a strict decoding of the same bytes,
 * so a replacement character the file itself holds is told apart.
 *
 * A service that answers a ranged read with the whole object (a 200 without Content-Range) is
 * read as the part asked of it: its bytes from the offset asked.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import type { Revision, SourceAppearance } from '../../../api/generated/types.gen';
import { readObjectBytes } from '../../../app/byte-read';

/** How many bytes of the original one read asks: the reader's own text budget. */
export const ORIGINAL_PART_BYTES = 65_536;

/** One part of the original as read: its bytes from `offset`. */
export interface OriginalPart {
  offset: number;
  bytes: Uint8Array<ArrayBuffer>;
}

/** The original's text as far as it is read. */
export interface DecodedText {
  text: string;
  /** Replacement characters in `text`. */
  replacements: number;
  /** Some bytes read are not UTF-8 (else every replacement character is the file's own). */
  notUtf8: boolean;
}

/** The byte count off the wire (a decimal string), or null when it is not a safe number. */
export function sizeOf(size: string): number | null {
  if (!/^\d+$/.test(size)) return null;
  const bytes = Number(size);
  return Number.isSafeInteger(bytes) ? bytes : null;
}

/** The bytes the parts hold, in order. */
function joined(parts: readonly OriginalPart[]): Uint8Array<ArrayBuffer> {
  const length = parts.reduce((total, part) => total + part.bytes.length, 0);
  const all = new Uint8Array(length);
  let at = 0;
  for (const part of parts) {
    all.set(part.bytes, at);
    at += part.bytes.length;
  }
  return all;
}

const REPLACEMENT = /�/g;

/** Decode the parts read as UTF-8; `complete` when nothing more is to be read. */
export function decodeOriginal(parts: readonly OriginalPart[], complete: boolean): DecodedText {
  const bytes = joined(parts);
  const text = new TextDecoder('utf-8').decode(bytes, { stream: !complete });
  let notUtf8 = false;
  try {
    new TextDecoder('utf-8', { fatal: true }).decode(bytes, { stream: !complete });
  } catch {
    notUtf8 = true;
  }
  return { text, replacements: text.match(REPLACEMENT)?.length ?? 0, notUtf8 };
}

/** The lines of a text as a reader counts them: a final line end does not start another line. */
export function lineCount(text: string): number {
  if (text === '') return 0;
  const ends = text.match(/\n/g)?.length ?? 0;
  return text.endsWith('\n') ? ends : ends + 1;
}

export interface OriginalOf {
  workspaceId: string;
  itemId: string;
  revision: Revision;
  appearance: SourceAppearance;
}

/** The original's text, read a part at a time. */
export function useOriginalText({ workspaceId, itemId, revision, appearance }: OriginalOf) {
  const size = sizeOf(appearance.size);
  const digest = appearance.object;
  const read = useInfiniteQuery({
    queryKey: [
      {
        _id: 'originalBytes',
        workspaceId,
        itemId,
        revision,
        digest,
        part: ORIGINAL_PART_BYTES,
      },
    ],
    queryFn: async ({ pageParam, signal }): Promise<OriginalPart> => {
      const length = Math.min(ORIGINAL_PART_BYTES, (size ?? 0) - pageParam);
      const blob = await readObjectBytes({
        workspaceId,
        itemId,
        revision,
        digest,
        range: { offset: pageParam, length },
        signal,
      });
      const answered = new Uint8Array(await blob.arrayBuffer());
      // The whole object answered for a part of it: the part asked is taken from it.
      const bytes =
        size !== null && answered.length === size && length < size
          ? answered.slice(pageParam, pageParam + length)
          : answered;
      return { offset: pageParam, bytes };
    },
    initialPageParam: 0,
    getNextPageParam: (last) => {
      const next = last.offset + last.bytes.length;
      return last.bytes.length > 0 && size !== null && next < size ? next : undefined;
    },
    enabled: size !== null && size > 0,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const parts = read.data?.pages ?? [];
  const shown = parts.reduce((total, part) => total + part.bytes.length, 0);
  const complete = parts.length > 0 && !read.hasNextPage;
  return { read, size, shown, complete, parts };
}
