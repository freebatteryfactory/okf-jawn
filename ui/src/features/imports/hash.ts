/**
 * A file's SHA-256, as the wire names content: 64 lower-case hexadecimal digits (`Digest`).
 *
 * `crypto.subtle.digest` takes the whole content as one buffer, and no library in this package
 * hashes a stream, so a file is read into memory once to be hashed. The buffer is let go as soon
 * as the hash is known; the bytes that are sent are read from the file again by the browser.
 * This is the one place, with the sending, where a file's content is read.
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

export async function sha256Hex(content: Blob): Promise<string> {
  const subtle: SubtleCrypto | undefined = globalThis.crypto.subtle;
  if (subtle === undefined) throw new HashUnavailable();
  let bytes: ArrayBuffer;
  try {
    bytes = await content.arrayBuffer();
  } catch (cause) {
    throw new FileUnreadable(cause);
  }
  const digest = await subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}
