/**
 * Re-read the files one converter process wrote beside its receipt: the Markdown, the
 * document export, each page image and, for a PDF, the library's text-layer document. Every
 * file is hashed again here, so what the orchestrator judges is what is on disk, not what the
 * process said it wrote. Each page image is also decoded (lib/png.mjs), so a render can be
 * judged by what it shows; the text-layer document is what the orchestrator makes each
 * text-layer lookup from again (lib/document.mjs deriveLookups).
 */

import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pngSize } from './document.mjs';
import { pngInk } from './png.mjs';

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

/**
 * @param {string} dir the fixture's output directory
 * @param {object|null|undefined} written the `document` block of the fixture's Rust receipt
 * @returns {Promise<{ markdown: string|null, document: object|null, textLayer: object|null, pageFiles: object, problems: string[] }>}
 *   `problems` names every file that is unreadable or whose hash differs from the one the
 *   process recorded; a fixture with a problem is not judged from its files. `textLayer` is
 *   null when the process recorded no text-layer file (not a PDF, or the library could not
 *   read the text layer)
 */
export async function loadEvidence(dir, written) {
  const evidence = { markdown: null, document: null, textLayer: null, pageFiles: {}, problems: [] };
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
  if (written.text_layer !== null && typeof written.text_layer === 'object') {
    const layer = await read(written.text_layer.file, written.text_layer.sha256);
    if (layer) {
      try {
        evidence.textLayer = JSON.parse(layer.toString('utf8'));
      } catch (error) {
        evidence.problems.push(`${written.text_layer.file}: not JSON (${error.message})`);
      }
    }
  }
  for (const image of written.page_images ?? []) {
    const bytes = await read(image.file, image.sha256);
    // Recorded as found, changed or not; a file that could not be read is null.
    evidence.pageFiles[image.file] = bytes ? { sha256: sha256(bytes), bytes: bytes.length, png: pngSize(bytes), ink: pngInk(bytes) } : null;
  }
  return evidence;
}
