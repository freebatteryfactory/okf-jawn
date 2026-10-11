/**
 * Guard: no external job engine comes back. The job design is Tokio over the RecordStore port.
 *
 * The rejected engine's three-letter name is built from parts so that this file does not
 * contain it and therefore cannot trip its own scan.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const word = 'i'.repeat(3);
const wholeWord = new RegExp(`(?<![A-Za-z0-9_])${word}(?![A-Za-z0-9_])`, 'i');
const prefixed = new RegExp(String.raw`^${word}([-_./]|$)`, 'i');
const advice = 'state the job design with the RecordStore port names; no external job engine';

const tracked = execFileSync('git', ['ls-files', '-z'], { cwd: root, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
  .split('\0').filter(path => path && existsSync(join(root, path)));
const read = path => readFileSync(join(root, path));
const isBinary = bytes => bytes.subarray(0, 8000).includes(0);

/** `file:line` of every line that matches, for failure messages. */
const lineHits = (path, text, test) => text.split(/\r?\n/).flatMap((line, index) => (test(line) ? [`${path}:${index + 1}: ${line.trim().slice(0, 120)}`] : []));

test('no workspace member, dependency key or locked package starts with the rejected engine name', () => {
  const hits = [];
  for (const path of tracked.filter(file => basename(file) === 'Cargo.toml')) {
    const text = read(path).toString('utf8');
    hits.push(...lineHits(path, text, line => {
      const key = /^\s*"?([A-Za-z0-9_.-]+)"?\s*=/.exec(line)?.[1];
      const quoted = [...line.matchAll(/"([^"]+)"/g)].map(match => match[1].replace(/^.*\//, ''));
      return (key !== undefined && prefixed.test(key)) || (/^\s*(members|\[|")/.test(line) && quoted.some(item => prefixed.test(item)));
    }));
  }
  for (const path of tracked.filter(file => basename(file) === 'package.json')) {
    const manifest = JSON.parse(read(path).toString('utf8'));
    const keys = ['dependencies', 'devDependencies', 'peerDependencies', 'optionalDependencies'].flatMap(field => Object.keys(manifest[field] ?? {}));
    for (const key of keys.filter(item => prefixed.test(item))) hits.push(`${path}: dependency ${key}`);
  }
  for (const path of tracked.filter(file => basename(file) === 'Cargo.lock')) {
    hits.push(...lineHits(path, read(path).toString('utf8'), line => {
      const name = /^name = "([^"]+)"/.exec(line)?.[1];
      return name !== undefined && prefixed.test(name);
    }));
  }
  for (const path of tracked.filter(file => basename(file) === 'bun.lock')) {
    hits.push(...lineHits(path, read(path).toString('utf8'), line => {
      const match = /^\s*"([^"]+)": \["([^"]+)@/.exec(line);
      return match !== null && prefixed.test(match[1]);
    }));
  }
  assert.deepEqual(hits, [], `${advice}\n${hits.join('\n')}`);
});

test('no tracked text file contains the rejected engine name as a whole word', () => {
  const hits = [];
  for (const path of tracked) {
    if (/^LICENSE/i.test(basename(path))) continue; // the Apache text has a roman-numeral list item that matches
    const bytes = read(path);
    if (isBinary(bytes)) continue;
    const text = bytes.toString('utf8');
    // A DoclingDocument export carries a third-party document's own text (a roman-numeral page number matches).
    if (/^\s*\{\s*"schema_name"\s*:\s*"DoclingDocument"/.test(text)) continue;
    hits.push(...lineHits(path, text, line => wholeWord.test(line)));
  }
  assert.deepEqual(hits, [], `${advice}\n${hits.join('\n')}`);
});
