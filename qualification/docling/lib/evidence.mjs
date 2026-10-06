/**
 * Re-read the files one converter process wrote beside its receipt: the Markdown, the
 * document export and each page image. Every file is hashed again here, so what the
 * orchestrator judges is what is on disk, not what the process said it wrote.
 */

import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pngSize } from './document.mjs';

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

/**
 * @param {string} dir the fixture's output directory
 * @param {object|null|undefined} written the `document` block of the fixture's Rust receipt
 * @returns {Promise<{ markdown: string|null, document: object|null, pageFiles: object, problems: string[] }>}
 */
export async function loadEvidence(dir, written) {
  const evidence = { markdown: null, document: null, pageFiles: {}, problems: [] };
  if (!written || typeof written !== 'object') return evidence;
  const read = async (file, expectedSha) => {
    try {
      const bytes = await readFile(join(dir, file));
      if (sha256(bytes) !== expectedSha) evidence.problems.push(`${file}: hash on disk differs from the receipt`);
      return bytes;
    } catch (error) {
      evidence.problems.push(`${file}: ${error.message}`);
      return null;
    }
  };
  const markdown = await read(written.markdown_file, written.markdown_sha256);
  if (markdown) evidence.markdown = markdown.toString('utf8');
  const json = await read(written.json_file, written.json_sha256);
  if (json) {
    try {
      evidence.document = JSON.parse(json.toString('utf8'));
    } catch (error) {
      evidence.problems.push(`${written.json_file}: not JSON (${error.message})`);
    }
  }
  for (const image of written.page_images ?? []) {
    try {
      const bytes = await readFile(join(dir, image.file));
      evidence.pageFiles[image.file] = { sha256: sha256(bytes), bytes: bytes.length, png: pngSize(bytes) };
    } catch {
      evidence.pageFiles[image.file] = null; // judged by judgePageRenders as "could not be re-read"
    }
  }
  return evidence;
}
