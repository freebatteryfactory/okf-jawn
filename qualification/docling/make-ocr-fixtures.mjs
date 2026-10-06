/**
 * Write the OCR fixtures drawn by lib/ocr-fixture.mjs into tests/fixtures/documents/.
 *
 * Usage: bun qualification/docling/make-ocr-fixtures.mjs
 * A fixture that already exists is never rewritten: its bytes are compared, pixel for pixel,
 * with what this script would draw, and a difference is an error. After writing a new file,
 * record its sha256 and byte length in tests/fixtures/documents/SOURCES.json by hand; the
 * foundation tests refuse a fixture whose recorded hash, length or pixels disagree.
 */

import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { OCR_FIXTURES, decodeFixture, encodeFixture, renderLines } from './lib/ocr-fixture.mjs';

const dir = resolve(fileURLToPath(new URL('../../tests/fixtures/documents', import.meta.url)));

for (const [name, spec] of Object.entries(OCR_FIXTURES)) {
  const path = join(dir, name);
  let existing = null;
  try {
    existing = await readFile(path);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  if (existing) {
    const same = Buffer.compare(Buffer.from(decodeFixture(spec, existing)), Buffer.from(renderLines(spec))) === 0;
    if (!same) throw new Error(`${name} exists and does not show what lib/ocr-fixture.mjs draws; fixtures are never rewritten`);
    process.stdout.write(`${name}: present, pixels match the generator\n`);
    continue;
  }
  const bytes = encodeFixture(spec);
  await writeFile(path, bytes, { flag: 'wx' });
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  process.stdout.write(`${name}: wrote ${bytes.length} bytes, sha256 ${sha256}\n`);
}
