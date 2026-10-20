/**
 * One reading of a file, for its upload: the bytes that will be sent, and their SHA-256 as the
 * wire names content (64 lower-case hexadecimal digits, `Digest`).
 *
 * The bytes sent are the bytes hashed. The file is read once, into memory; the hash is computed
 * from that reading, and the bytes sent are that same reading, never the file read a second time
 * from the disk. So a file that changes while it is uploaded cannot be announced with one content
 * and sent with another. This is the one place where a file's content is read.
 *
 * Memory, for a file of N bytes. `crypto.subtle.digest` takes the whole content as one buffer, and
 * no library in this package hashes a stream, so:
 * - while it is hashed: about 2 N. The reading (N), and the copy the browser's hash works on (the
 *   Web Cryptography API has `digest` work on a copy of the bytes it is given);
 * - just after: up to 2 N for a moment. The Blob that is sent keeps its own copy of the reading
 *   (N); the first buffer is let go here and is the collector's to free;
 * - while it is sent: N, until its upload ends or fails.
 * So the largest file the page takes (`MAX_FILE_BYTES`, 512 MiB) needs about 1 GiB at its peak and
 * 512 MiB while it is sent. What bounds it: a file's length is checked against `MAX_FILE_BYTES`
 * when it is listed, before anything is read; one file is read and hashed at a time; and the
 * files under way together hold at most `MAX_FILE_BYTES` (`uploading.ts`). None of this is
 * measured in a browser: the numbers are what the two specifications say is copied.
 */

/** The file's content could not be read: it was moved, changed or deleted, or is too large. */
export class FileUnreadable extends Error {
  constructor(cause: unknown) {
    super('The file could not be read', { cause });
    this.name = 'FileUnreadable';
  }
}

/** The page has no `crypto.subtle`: browsers give it to secure pages only (HTTPS, or this computer). */
export class HashUnavailable extends Error {
  constructor() {
    super('This page cannot compute a SHA-256 hash');
    this.name = 'HashUnavailable';
  }
}

/** What one reading of a file gives. */
export interface FileReading {
  /** The bytes as they were read: what is sent. */
  bytes: Blob;
  /** Their SHA-256. */
  sha256: string;
}

export async function readForUpload(content: Blob): Promise<FileReading> {
  const subtle: SubtleCrypto | undefined = globalThis.crypto.subtle;
  if (subtle === undefined) throw new HashUnavailable();
  let read: ArrayBuffer;
  try {
    read = await content.arrayBuffer();
  } catch (cause) {
    throw new FileUnreadable(cause);
  }
  const digest = await subtle.digest('SHA-256', read);
  return {
    bytes: new Blob([read]),
    sha256: Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join(
      '',
    ),
  };
}
