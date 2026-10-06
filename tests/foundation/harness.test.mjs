/**
 * Harness controls. They are not application acceptance.
 *
 * HTTP controls prove the assertion helpers can fail. Qualification controls prove the
 * orchestrators' own rules (provenance, process handling, judgement) without cargo or network.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { reviewCoversRevision, readResolvedRevision } from '../support/assertions.mjs';
import {
  receiptHeader,
  receiptHeaderProblems,
  recordReceipt,
  requireCleanTree,
} from '../../scripts/lib/provenance.mjs';

import { lockedPackage, lockedPackages, releaseBinary } from '../../qualification/lib/cargo.mjs';
import {
  mergePeak,
  parseDoneMarker,
  parseVmHwm,
  runFixtureProcess,
  samplePeakRss,
} from '../../qualification/docling/lib/runner.mjs';
import {
  DOCLING_INPUTS,
  FIXTURE_RUNS,
  MUST_FAIL,
  TIMEOUT_PROBE,
  buildDoclingReceipt,
} from '../../qualification/docling/lib/receipt.mjs';
import { assetsMatched, inventoryCheck, parseManifest, unverifiedEnvPaths, verifyAssets } from '../../qualification/docling/lib/assets.mjs';
import { buildFacts, classifyRefusal, declaredDoclingFeatures, resolvedFeatures } from '../../qualification/docling/lib/build.mjs';
import * as doclingCriteria from '../../qualification/docling/lib/criteria.mjs';
import * as doclingEnvelope from '../../scripts/lib/receipt-envelope.mjs';
import {
  PAGE_RENDER_RULE,
  bboxProblem,
  bodyFacts,
  converterOptions,
  describeDocument,
  judgePageRenders,
  judgeProvenance,
  pngSize,
} from '../../qualification/docling/lib/document.mjs';
import { loadEvidence } from '../../qualification/docling/lib/evidence.mjs';
import { MATCH_RULES, collapse, judgeContent, rowHasCells, textTokens } from '../../qualification/docling/lib/expect.mjs';
import { OCR_FIXTURES, decodeFixture, encodePng, fixtureWords, renderLines } from '../../qualification/docling/lib/ocr-fixture.mjs';
import { pngInk } from '../../qualification/docling/lib/png.mjs';
import { deflateSync } from 'node:zlib';
import { killProcessTree, spawnGroup, waitForListening } from '../../qualification/mcp-apps/lib/process.mjs';
import {
  APP_ONLY_TOOLS,
  MCP_APPS_INPUTS,
  PRESENT_DATASET,
  REACT_DEVELOPMENT_MARKER,
  TOOL_CALL_LOG_PREFIX,
  UPSTREAM_HOST_RULES,
  VIEWS,
  appBundleBuild,
  basicHostUrl,
  basicHostVerdict,
  datasetExpectation,
  judgePresentDataset,
  judgeView,
  ngrokRecord,
  partitionAxe,
  runProblems,
  toolCallsFrom,
  transportRecord,
} from '../../qualification/mcp-apps/lib/views.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));

/** Budget for a promise that settles at once; generous because a parallel cargo build has starved these. */
const SLOW_MACHINE_BUDGET_MS = 20_000;

/** Reject when `promise` has not settled after `ms`; a hang must read as a named failure. */
function within(promise, ms, label) {
  let timer;
  const expired = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} did not settle within ${ms} ms`)), ms);
  });
  return Promise.race([promise, expired]).finally(() => clearTimeout(timer));
}

/** A throwaway git repository outside this checkout, isolated from user hooks and signing. */
async function tempRepo({ commit = true } = {}) {
  const base = await mkdtemp(join(tmpdir(), 'okf-provenance-'));
  const repo = join(base, 'repo');
  const hooks = join(base, 'hooks');
  await mkdir(join(repo, 'dir'), { recursive: true });
  await mkdir(hooks);
  const git = (...args) =>
    execFileSync(
      'git',
      [
        '-c', `core.hooksPath=${hooks}`,
        '-c', 'commit.gpgsign=false',
        '-c', 'user.name=okf-test',
        '-c', 'user.email=okf-test@example.invalid',
        ...args,
      ],
      { cwd: repo, stdio: 'pipe' },
    ).toString();
  git('init');
  await writeFile(join(repo, 'a.txt'), 'a\n');
  await writeFile(join(repo, 'dir', 'b.txt'), 'b\n');
  if (commit) {
    git('add', '.');
    git('commit', '-m', 'init');
  }
  return { base, repo, git, dispose: () => rm(base, { recursive: true, force: true }) };
}

async function fixture(t, body) {
 const server=createServer((request,response)=>{response.setHeader('Content-Type','application/json');response.end(JSON.stringify(body));});
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
 t.after(()=>new Promise(resolve=>server.close(resolve)));
 return `http://127.0.0.1:${server.address().port}`;
}
test('positive HTTP control reaches and validates a correct reviewed revision',async t=>{
 const revision='a'.repeat(40);const url=await fixture(t,{source:{revision},coverage:'current'});
 const body=await (await fetch(url)).json();
 assert.deepEqual(body,{source:{revision},coverage:'current'},'the control must have received the value the server sent');
 reviewCoversRevision(body,revision);
});
test('negative HTTP control detects a response that advanced the reviewed revision',async t=>{
 const url=await fixture(t,{source:{revision:'b'.repeat(40)},coverage:'current'});
 const value=await (await fetch(url)).json();assert.throws(()=>reviewCoversRevision(value,'a'.repeat(40)),/review must name/);
});
test('pinned read and truncation controls reject wrong but well-shaped responses',()=>{
 assert.throws(()=>readResolvedRevision({source:{revision:'b'.repeat(40)},markdown:'x',truncated:false},'a'.repeat(40)),/pinned revision/);
 assert.throws(()=>readResolvedRevision({source:{revision:'a'.repeat(40)},markdown:'x',truncated:true},'a'.repeat(40)),/continuation/);
});

test('requireCleanTree returns HEAD when clean and rejects untracked, modified and staged paths', async () => {
  const fixture = await tempRepo();
  try {
    assert.match(await requireCleanTree(fixture.repo), /^[0-9a-f]{40}$/);
    await writeFile(join(fixture.repo, 'new.txt'), 'x\n');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*new\.txt/);
    await rm(join(fixture.repo, 'new.txt'));
    await writeFile(join(fixture.repo, 'a.txt'), 'changed\n');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*a\.txt/);
    fixture.git('add', 'a.txt');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*a\.txt/);
  } finally {
    await fixture.dispose();
  }
});

test('requireCleanTree sees untracked files even when user configuration hides them', async () => {
  const fixture = await tempRepo();
  try {
    fixture.git('config', 'status.showUntrackedFiles', 'no');
    await writeFile(join(fixture.repo, 'new.txt'), 'x\n');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*new\.txt/);
  } finally {
    await fixture.dispose();
  }
});

test('requireCleanTree rejects when git exits non-zero', async () => {
  const fixture = await tempRepo({ commit: false });
  try {
    await rm(join(fixture.repo, 'a.txt'));
    await rm(join(fixture.repo, 'dir'), { recursive: true });
    // Empty work tree, no commit: status prints nothing and exits 0, rev-parse HEAD exits 128.
    await assert.rejects(requireCleanTree(fixture.repo), /git rev-parse HEAD exited \d+/);
  } finally {
    await fixture.dispose();
  }
});

test('receiptHeader output is the top-level shape check-receipts validates', async () => {
  const fixture = await tempRepo();
  try {
    const before = Date.now();
    const header = await receiptHeader(fixture.repo, ['a.txt', 'dir']);
    assert.deepEqual(Object.keys(header), ['git_sha', 'inputs', 'produced_at']);
    // The two predicates of scripts/dev.mjs check-receipts (lines 202-210).
    assert.match(header.git_sha, /^[0-9a-f]{40}$/i);
    assert.ok(Array.isArray(header.inputs) && header.inputs.every((item) => typeof item === 'string'));
    assert.deepEqual(header.inputs, ['a.txt', 'dir']);
    assert.ok(Date.parse(header.produced_at) >= before - 1000);
    assert.deepEqual(receiptHeaderProblems({ ...header, anything: 1 }), []);
    // The old Docling shape nested the header and failed check-receipts.
    assert.equal(receiptHeaderProblems({ orchestrator: header }).length, 3);
  } finally {
    await fixture.dispose();
  }
});

test('receiptHeader refuses a dirty tree and inputs check-receipts could never see change', async () => {
  const fixture = await tempRepo();
  try {
    await assert.rejects(receiptHeader(fixture.repo, ['missing.txt']), /does not exist at [0-9a-f]{40}/);
    await assert.rejects(receiptHeader(fixture.repo, ['../a.txt']), /segments/);
    await assert.rejects(receiptHeader(fixture.repo, ['dir/']), /segments/);
    await assert.rejects(receiptHeader(fixture.repo, ['C:/a.txt']), /relative to the repository root/);
    await assert.rejects(receiptHeader(fixture.repo, ['/a.txt']), /relative to the repository root/);
    await assert.rejects(receiptHeader(fixture.repo, ['dir\\b.txt']), /forward slashes/);
    await assert.rejects(receiptHeader(fixture.repo, []), /non-empty array/);
    await writeFile(join(fixture.repo, 'new.txt'), 'x\n');
    await assert.rejects(receiptHeader(fixture.repo, ['a.txt']), /clean git tree/);
  } finally {
    await fixture.dispose();
  }
});

test('recordReceipt writes qualification/receipts/<name>.json only for a headed receipt at HEAD', async () => {
  const fixture = await tempRepo();
  try {
    const header = await receiptHeader(fixture.repo, ['a.txt']);
    const target = await recordReceipt(fixture.repo, 'docling', { ...header, result: 'PASS' });
    assert.equal(target, join(fixture.repo, 'qualification', 'receipts', 'docling.json'));
    assert.deepEqual(JSON.parse(await readFile(target, 'utf8')), { ...header, result: 'PASS' });
    await assert.rejects(recordReceipt(fixture.repo, '../escape', header), /lower-case words/);
    await assert.rejects(recordReceipt(fixture.repo, 'nested', { orchestrator: header }), /git_sha/);
    await assert.rejects(
      recordReceipt(fixture.repo, 'stale', { ...header, git_sha: 'a'.repeat(40) }),
      /HEAD is [0-9a-f]{40}/,
    );
  } finally {
    await fixture.dispose();
  }
});

test('lockedPackage reads exactly one pinned package from Cargo.lock text', () => {
  const lock = [
    '# This file is automatically @generated by Cargo.',
    'version = 4',
    '',
    '[[package]]',
    'name = "docling"',
    'version = "1.93.5"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    'checksum = "abc123"',
    '',
    '[[package]]',
    'name = "docling-core"',
    'version = "1.93.6"',
    '',
    '[[package]]',
    'name = "sha2"',
    'version = "0.10.9"',
    '',
    '[[package]]',
    'name = "sha2"',
    'version = "0.11.0"',
    '',
  ];
  for (const text of [lock.join('\n'), lock.join('\r\n')]) {
    assert.deepEqual(lockedPackage(text, 'docling'), { name: 'docling', version: '1.93.5', checksum: 'abc123' });
    assert.deepEqual(lockedPackages(text, 'sha2').map((entry) => entry.version), ['0.10.9', '0.11.0']);
    assert.throws(() => lockedPackage(text, 'sha2'), /exactly one sha2 package.*found 2/);
    assert.throws(() => lockedPackage(text, 'absent'), /exactly one absent package.*found 0/);
  }
});

test('the locked docling version is the one the workspace manifest pins', async () => {
  const manifest = await readFile(join(root, 'Cargo.toml'), 'utf8');
  const pinned = /^docling = \{ version = "=([^"]+)"/m.exec(manifest)?.[1];
  assert.ok(pinned, 'workspace Cargo.toml must pin docling with an exact version');
  assert.equal(lockedPackage(await readFile(join(root, 'Cargo.lock'), 'utf8'), 'docling').version, pinned);
});

test('releaseBinary names the platform executable under the cargo target directory', () => {
  assert.equal(releaseBinary('t', 'okf-qualify-docling', 'win32'), join('t', 'release', 'okf-qualify-docling.exe'));
  assert.equal(releaseBinary('t', 'okf-qualify-docling', 'linux'), join('t', 'release', 'okf-qualify-docling'));
});

const MARKER_LINE = "JSON.stringify({okf_docling:'done',only:'x.pdf',receipt:'r.json'})+'\\n'";
const HOLD_CHILD = `process.stdout.write(${MARKER_LINE});process.stdin.on('data',()=>{});process.stdin.on('end',()=>process.exit(0));`;

test('an instant-exit converter process resolves promptly and is recorded as unmeasured', async () => {
  const result = await within(runFixtureProcess({ command: process.execPath, args: ['-e', ''] }), SLOW_MACHINE_BUDGET_MS, 'instant exit');
  assert.equal(result.exitCode, 0);
  assert.equal(result.done, null);
  assert.equal(result.peakRssBytes, null);
  assert.match(result.peakRssNote, /exited before printing the done marker/);
});

test('the peak is sampled exactly once, while the process still waits on stdin', async () => {
  let calls = 0;
  let aliveAtSample = null;
  const sample = (pid) => {
    calls += 1;
    try {
      process.kill(pid, 0);
      aliveAtSample = true;
    } catch {
      aliveAtSample = false;
    }
    return { bytes: 4096, note: 'stub' };
  };
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', HOLD_CHILD], sample }),
    SLOW_MACHINE_BUDGET_MS,
    'held child',
  );
  assert.equal(result.exitCode, 0);
  assert.deepEqual(result.done, { okf_docling: 'done', only: 'x.pdf', receipt: 'r.json' });
  assert.equal(calls, 1);
  assert.equal(aliveAtSample, true);
  assert.equal(result.peakRssBytes, 4096);
  assert.equal(result.peakRssNote, 'stub');
});

test('a null sample never overwrites a number', async () => {
  assert.equal(mergePeak(5, null), 5);
  assert.equal(mergePeak(null, 5), 5);
  assert.equal(mergePeak(5, 3), 5);
  assert.equal(mergePeak(3, 5), 5);
  assert.equal(mergePeak(null, null), null);
  assert.equal(mergePeak(5, Number.NaN), 5);
  let calls = 0;
  const sample = () => {
    calls += 1;
    return calls === 1 ? { bytes: 100, note: 'first' } : { bytes: null, note: 'late null' };
  };
  const twice = `const l=${MARKER_LINE};process.stdout.write(l+l);process.stdin.on('data',()=>{});process.stdin.on('end',()=>process.exit(0));`;
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', twice], sample }),
    SLOW_MACHINE_BUDGET_MS,
    'double marker',
  );
  assert.equal(calls, 1);
  assert.equal(result.peakRssBytes, 100);
  assert.equal(result.peakRssNote, 'first');
});

test('a process that exits while the sample is in flight still resolves, with the reason recorded', async () => {
  const sample = () => new Promise((done) => setTimeout(() => done({ bytes: null, note: 'process gone' }), 300));
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', `process.stdout.write(${MARKER_LINE});`], sample }),
    SLOW_MACHINE_BUDGET_MS,
    'exit during sample',
  );
  assert.equal(result.exitCode, 0);
  assert.equal(result.peakRssBytes, null);
  assert.equal(result.peakRssNote, 'process gone');
});

test('a converter that never finishes is killed at the fixture timeout', async () => {
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', 'setInterval(()=>{},1000)'], timeoutMs: 300 }),
    SLOW_MACHINE_BUDGET_MS,
    'timeout',
  );
  assert.equal(result.timedOut, true);
  assert.notEqual(result.exitCode, 0);
  assert.equal(result.peakRssBytes, null);
  assert.match(result.peakRssNote, /killed after 300 ms/);
});

test('a missing converter binary is a result, not a hang or a throw', async () => {
  const result = await within(
    runFixtureProcess({ command: join(tmpdir(), 'okf-no-such-binary') }),
    SLOW_MACHINE_BUDGET_MS,
    'missing binary',
  );
  assert.notEqual(result.exitCode, 0);
  assert.equal(result.peakRssBytes, null);
});

test('the platform sampler reads a real high-water mark and reports why when it cannot', async () => {
  assert.equal(parseVmHwm('Name:\tx\nVmHWM:\t    1234 kB\nVmRSS:\t     900 kB\n'), 1234 * 1024);
  assert.equal(parseVmHwm('Name:\tx\nVmRSS:\t 900 kB\n'), null);
  assert.equal(parseDoneMarker('Wrote C:\\out\\receipt.json'), null);
  assert.equal(parseDoneMarker('{"okf_docling":"other"}'), null);
  assert.deepEqual(parseDoneMarker(' {"okf_docling":"done","only":"a"} \r'), { okf_docling: 'done', only: 'a' });
  const own = await samplePeakRss(process.pid);
  if (process.platform === 'win32' || process.platform === 'linux') {
    assert.ok(own.bytes > 0, own.note);
  } else {
    assert.equal(own.bytes, null);
    assert.match(own.note, /no peak RSS source on platform/);
  }
  assert.deepEqual(await samplePeakRss(process.pid, 'sunos'), {
    bytes: null,
    note: 'no peak RSS source on platform sunos',
  });
});

test('every fixture run is a recorded source whose bytes are unchanged, and must_fail states its stage rule', async () => {
  const dir = join(root, 'tests/fixtures/documents');
  const sources = JSON.parse(await readFile(join(dir, 'SOURCES.json'), 'utf8'));
  assert.deepEqual(
    FIXTURE_RUNS.filter((only) => only !== TIMEOUT_PROBE).sort(),
    Object.keys(sources.files).sort(),
  );
  for (const [name, entry] of Object.entries(sources.files)) {
    const bytes = await readFile(join(dir, name));
    assert.equal(bytes.length, entry.bytes, name);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), entry.sha256, name);
  }
  assert.deepEqual(sources.files[MUST_FAIL].pass_when, ['converter_error', 'converter_status:Failure']);
});

test('the OCR fixtures show exactly the words their expectation names, pixel for pixel', async () => {
  const dir = join(root, 'tests/fixtures/documents');
  const sources = JSON.parse(await readFile(join(dir, 'SOURCES.json'), 'utf8'));
  assert.deepEqual(Object.keys(OCR_FIXTURES).sort(), ['scanned_text.pdf', 'text_image.png']);
  for (const [name, spec] of Object.entries(OCR_FIXTURES)) {
    const drawn = Buffer.from(renderLines(spec));
    assert.equal(Buffer.compare(Buffer.from(decodeFixture(spec, await readFile(join(dir, name)))), drawn), 0, `${name} is not what the generator draws`);
    assert.deepEqual(sources.files[name].expect.ocr_tokens, fixtureWords(spec), name);
    assert.ok(drawn.includes(0) && drawn.includes(255), `${name}: black strokes on white`);
    // A different word draws different pixels, so the comparison above can fail.
    const other = { ...spec, lines: spec.lines.map((line, index) => (index === 0 ? { ...line, text: 'TEAM' } : line)) };
    assert.notEqual(Buffer.compare(Buffer.from(renderLines(other)), drawn), 0);
  }
  assert.throws(() => renderLines({ width: 10, height: 10, unit: 1, lines: [{ text: 'Q', left: 0, top: 0 }] }), /no glyph for "Q"/);
  assert.match((await readFile(join(dir, 'scanned_text.pdf'))).toString('latin1'), /\/Subtype \/Image \/Width 612 \/Height 792 \/ColorSpace \/DeviceGray/);
  assert.doesNotMatch((await readFile(join(dir, 'scanned_text.pdf'))).toString('latin1'), /\bBT\b|\bTj\b/, 'no text operators: the words exist only as pixels');
});

test('content is matched against the declared expectation, and a missing string, row or count fails', () => {
  const observed = {
    markdown: '## Title\n\nHello   world\n\n| Name | Qty |\n| - | - |\n| Widget | 3 |\n',
    tables: [[['Name', 'Qty'], [' Widget ', '3']]],
    headings: 1,
    pictures: 0,
    sheet_names: ['Sheet1'],
    page_count: 3,
  };
  const ok = judgeContent(
    {
      confirmed_by: 'test',
      markdown_contains: ['Hello world', { text: 'TITLE', case_insensitive: true }],
      table_rows: [['Widget', '3']],
      tables: 1,
      headings_at_least: 1,
      sheet_names: ['Sheet1'],
      pages: 3,
    },
    observed,
  );
  assert.equal(ok.status, 'PASS');
  assert.equal(ok.found, 7);
  assert.equal(ok.total, 7);
  assert.equal(ok.confirmed_by, 'test');

  const failing = (expect) => {
    const judged = judgeContent(expect, observed);
    assert.equal(judged.status, 'FAIL', JSON.stringify(expect));
    return judged.checks.filter((check) => !check.ok);
  };
  assert.deepEqual(failing({ markdown_contains: ['hello world'] }).map((check) => check.expected), ['hello world'], 'case-sensitive by default');
  assert.equal(failing({ markdown_contains: ['Hello world', 'Goodbye'] }).length, 1);
  assert.equal(failing({ table_rows: [['Widget', '4']] }).length, 1);
  assert.equal(failing({ table_rows: [['3', 'Widget']] }).length, 1, 'cells must come in row order');
  assert.deepEqual(failing({ tables: 2 })[0], { kind: 'tables', comparison: 'exact', expected: 2, observed: 1, ok: false });
  assert.equal(failing({ tables_at_least: 2 }).length, 1);
  assert.equal(failing({ headings_at_least: 2 })[0].observed, 1);
  assert.equal(failing({ pictures_at_least: 1 }).length, 1);
  assert.equal(failing({ sheet_names: ['Sheet1', 'Sheet2'] }).length, 1);
  assert.equal(failing({ pages: 2 })[0].observed, 3);
  assert.match(judgeContent(undefined, observed).reason, /no expect block/);
  assert.match(judgeContent({ confirmed_by: 'x' }, observed).reason, /checks nothing/);
  assert.equal(judgeContent({ confirmed_by: 'x' }, observed).status, 'FAIL');
  assert.ok(rowHasCells(['R1', 'True', '', 'False'], ['R1', 'False']));
  assert.ok(!rowHasCells(['R1', 'True'], ['R1', 'True', 'True']));
});

test('OCR text must hold every expected token exactly; only case and whitespace are forgiven', () => {
  const judge = (markdown, expect = { ocr_tokens: ['WATER', 'METER', '47'] }) =>
    judgeContent(expect, { markdown, tables: [], headings: 0, pictures: 1 });
  assert.match(MATCH_RULES.ocr_tokens, /No character confusion/);
  assert.deepEqual(textTokens('<!-- image -->\n\nWater-Meter  47.'), ['water', 'meter', '47']);

  const read = judge('<!-- image -->\n\nwater   METER\n\ntax year 47\n');
  assert.equal(read.status, 'PASS');
  assert.equal(read.ocr_exercised, true);
  assert.equal(read.observed_text, 'water METER tax year 47');

  for (const [markdown, missing] of [
    ['W4TER METER 47', ['WATER']], // a digit for a letter is not forgiven
    ['WATERMETER 47', ['WATER', 'METER']], // a lost space is a different token
    ['WATER METER 4 7', ['47']],
    ['WATER METEP 47', ['METER']],
    ['<!-- image -->', ['WATER', 'METER', '47']], // a picture placeholder is not text
    ['<!-- WATER METER 47 -->', ['WATER', 'METER', '47']],
  ]) {
    const judged = judge(markdown);
    assert.equal(judged.status, 'FAIL', markdown);
    assert.deepEqual(judged.checks.filter((check) => !check.ok).map((check) => check.expected), missing, markdown);
    assert.equal(judged.observed_text, collapse(markdown.replace(/<!--[\s\S]*?-->/g, ' ')), 'the text read is recorded on a FAIL');
  }

  // A fixture that shows no glyphs: placeholders are fine, any letter or digit is invented text.
  const blank = judge('<!-- image -->\n\n<!-- image -->\n', { no_text: true });
  assert.equal(blank.status, 'PASS');
  assert.equal(blank.ocr_exercised, false);
  const invented = judge('<!-- image -->\n\nIll\n', { no_text: true });
  assert.equal(invented.status, 'FAIL');
  assert.deepEqual(invented.checks[0].observed, ['ill']);
});

test('every supported fixture declares what it contains and how that was confirmed', async () => {
  const sources = JSON.parse(await readFile(join(root, 'tests/fixtures/documents/SOURCES.json'), 'utf8'));
  const nothing = { markdown: '', tables: [], headings: 0, pictures: 0, sheet_names: [], page_count: null };
  for (const [name, entry] of Object.entries(sources.files)) {
    if (name === MUST_FAIL) {
      assert.equal(entry.expect, undefined, 'the must-fail fixture has no content to expect');
      continue;
    }
    assert.ok(typeof entry.expect?.confirmed_by === 'string' && entry.expect.confirmed_by.length > 40, `${name}: confirmed_by must say how the expectation was read from the fixture`);
    const judged = judgeContent(entry.expect, nothing);
    assert.ok(judged.total > 0, `${name}: expect must check something`);
    if (/\.(pdf|png)$/.test(name)) assert.ok(Number.isInteger(entry.expect.pages), `${name}: a paginated fixture declares its page count`);
    // An empty document satisfies only the two fixtures that show no glyphs.
    assert.equal(judged.status, entry.expect.no_text === true ? 'PASS' : 'FAIL', name);
  }
  assert.deepEqual(
    Object.entries(sources.files).filter(([, entry]) => entry.expect?.no_text === true).map(([name]) => name).sort(),
    ['sample_image.png', 'scanned_image_only.pdf'],
  );
  // A blank page is a declared fact with its confirmation, and only one page of one fixture is blank.
  const blank = Object.entries(sources.files).filter(([, entry]) => entry.expect?.blank_pages !== undefined);
  assert.deepEqual(blank.map(([name, entry]) => [name, entry.expect.blank_pages]), [['corpus/redp5110_sampled.pdf', [17]]]);
  assert.match(blank[0][1].expect.blank_pages_confirmed_by, /inflates to zero bytes.*pdftotext 4\.06 -f 17 -l 17 prints no character/);
});

const VERIFIED_MODEL = { file: 'layout.onnx', path: 'C:\\models\\layout.onnx', bytes: 10, sha256: 'd'.repeat(64), manifest_bytes: 10, manifest_sha256: 'd'.repeat(64) };

const DOCLING_INVENTORY = [
  { stage: 'layout', path: 'C:/models/layout.onnx', found: true, bytes: 10 },
  { stage: 'pdfium', path: '.pdfium/lib', found: false, bytes: 0 },
];

async function assetFixture(t) {
  const dir = await mkdtemp(join(tmpdir(), 'okf-docling-assets-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const files = { 'layout.onnx': Buffer.from('layout model bytes'), 'tableformer/encoder.onnx': Buffer.from('encoder bytes') };
  await mkdir(join(dir, 'tableformer'));
  const assets = [];
  for (const [name, bytes] of Object.entries(files)) {
    const path = join(dir, ...name.split('/'));
    await writeFile(path, bytes);
    assets.push({ path, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
  }
  const manifest = { DOCLING_RS_MODELS_DIR: dir, recommended_env: { DOCLING_RS_MODELS_DIR: dir, DOCLING_LAYOUT_ONNX: assets[0].path }, assets };
  return { dir, assets, manifest, text: (value = manifest) => `\uFEFF${JSON.stringify(value)}` };
}

test('model assets are re-hashed at run time and any missing or changed file stops the run by name', async (t) => {
  const fixture = await assetFixture(t);
  const text = fixture.text();
  const { manifest, verified } = await verifyAssets(Buffer.from(text));
  assert.equal(manifest.DOCLING_RS_MODELS_DIR, fixture.dir);
  assert.equal(verified.count, 2);
  // No literal says "all match": the counts come from each file's own hash beside its manifest entry.
  assert.equal(verified.all_match, undefined);
  assert.equal(verified.hashed_at_run_time, undefined);
  assert.equal(verified.matched, 2);
  assert.deepEqual(verified.unmatched, []);
  for (const [index, file] of verified.files.entries()) {
    assert.equal(file.manifest_sha256, fixture.assets[index].sha256);
    assert.equal(file.manifest_bytes, fixture.assets[index].bytes);
    assert.equal(file.sha256, createHash('sha256').update(await readFile(file.path)).digest('hex'), 'the recorded hash is the hash of the bytes on disk');
  }
  assert.deepEqual(assetsMatched(verified.files), { count: 2, matched: 2, unmatched: [] });
  const [first, second] = verified.files;
  assert.deepEqual(assetsMatched([first, { ...second, sha256: '0'.repeat(64) }]), { count: 2, matched: 1, unmatched: ['tableformer/encoder.onnx'] });
  assert.deepEqual(assetsMatched([{ ...first, bytes: first.bytes + 1 }, second]).unmatched, ['layout.onnx']);
  assert.deepEqual(assetsMatched([{ file: 'x.onnx', bytes: 1, sha256: 'a'.repeat(64) }]).unmatched, ['x.onnx'], 'a record with no manifest entry beside it matches nothing');
  assert.deepEqual(assetsMatched(undefined), { count: 0, matched: 0, unmatched: [] });
  assert.equal(verified.manifest_sha256, createHash('sha256').update(Buffer.from(text)).digest('hex'), 'the manifest is hashed as read, BOM included');
  assert.deepEqual(verified.files.map((file) => file.file), ['layout.onnx', 'tableformer/encoder.onnx']);
  assert.equal(verified.files[0].sha256, fixture.assets[0].sha256);
  assert.equal(verified.bytes_total, fixture.assets[0].bytes + fixture.assets[1].bytes);

  const withAsset = (index, change) => fixture.text({ ...fixture.manifest, assets: fixture.manifest.assets.map((asset, at) => (at === index ? { ...asset, ...change } : asset)) });
  const encoder = fixture.assets[1].path;
  await assert.rejects(verifyAssets(withAsset(1, { sha256: '0'.repeat(64) })), (error) => error.message.includes(encoder) && /has sha256 [0-9a-f]{64}, manifest says 0{64}/.test(error.message));
  await assert.rejects(verifyAssets(withAsset(1, { bytes: 5 })), (error) => error.message.includes(encoder) && /is 13 bytes, manifest says 5/.test(error.message));
  await assert.rejects(verifyAssets(withAsset(0, { path: join(fixture.dir, 'gone.onnx') })), /recommended_env points at files with no verified hash: DOCLING_LAYOUT_ONNX=/);
  await assert.rejects(
    verifyAssets(fixture.text({ ...fixture.manifest, recommended_env: {}, assets: [{ ...fixture.assets[0], path: join(fixture.dir, 'gone.onnx') }] })),
    (error) => /Docling model asset missing: .*gone\.onnx/.test(error.message) && /never qualified/.test(error.message),
  );

  // The file itself changes after the manifest was written: same length, other bytes.
  await writeFile(encoder, Buffer.from('ENCODER BYTES'));
  await assert.rejects(verifyAssets(text), (error) => error.message.includes(encoder) && /Docling model asset changed/.test(error.message));

  await assert.rejects(verifyAssets(fixture.text({ ...fixture.manifest, assets: [] })), /non-empty array/);
  await assert.rejects(verifyAssets(fixture.text({ ...fixture.manifest, assets: [{ path: encoder, bytes: 13 }] })), /no 64-hex sha256/);
  await assert.rejects(verifyAssets(fixture.text({ assets: fixture.assets })), /DOCLING_RS_MODELS_DIR/);
  assert.throws(() => parseManifest('{'), SyntaxError);
  assert.deepEqual(unverifiedEnvPaths({ ...fixture.manifest, recommended_env: { DOCLING_OCR_DICT: join(fixture.dir, 'en_dict.txt'), DOCLING_RS_EP: 'cpu' } }), [
    `DOCLING_OCR_DICT=${join(fixture.dir, 'en_dict.txt')}`,
  ]);
});

test('the models the library resolves must be verified files', () => {
  const checked = inventoryCheck(DOCLING_INVENTORY, [VERIFIED_MODEL]);
  assert.equal(checked.status, 'PASS');
  assert.equal(checked.pdfium_library_found, false);
  assert.equal(checked.entries[0].verified_sha256, VERIFIED_MODEL.sha256);
  assert.equal(checked.entries[1].verified_sha256, null, 'pdfium is the optional native renderer, not a verified model');

  assert.deepEqual(inventoryCheck([{ stage: 'layout', path: 'C:/models/other.onnx', found: true, bytes: 10 }], [VERIFIED_MODEL]).unverified, ['layout']);
  assert.deepEqual(inventoryCheck([{ stage: 'layout', path: 'C:/models/layout.onnx', found: true, bytes: 11 }], [VERIFIED_MODEL]).unverified, ['layout'], 'same path, other length');
  assert.equal(inventoryCheck([{ stage: 'ocr.rec', path: 'C:/models/layout.onnx', found: false, bytes: 0 }], [VERIFIED_MODEL]).status, 'FAIL');
  assert.equal(inventoryCheck(null, [VERIFIED_MODEL]).status, 'FAIL');
});

const CONVERTER_DEBUG =
  'DocumentConverter { allowed_formats: None, strict: false, compact_tables: false, no_table_former: false, no_ocr: false, skip_ocr: false, force_full_page_ocr: false, ocr_mode: None, ocr_engine: None, ocr_scale: None, images_scale: None, generate_page_images: true, heading_hierarchy: false, enrich: EnrichmentOptions { picture_classification: false, code: false, formula: false }, page_range: None, ocr_lang: Some("en"), artifacts_dir: "C:\\\\t", document_timeout: None }';

/** A greyscale PNG of this size: `paper` everywhere, with one dark bar when `ink` is set. */
function pagePng(width, height, { paper = 255, ink = true } = {}) {
  const gray = new Uint8Array(width * height).fill(paper);
  if (ink) gray.fill(0, width * 10, width * 12);
  return encodePng({ width, height }, gray);
}

/** What lib/evidence.mjs records for a page image file it re-read. */
const pageFile = (bytes) => ({ sha256: createHash('sha256').update(bytes).digest('hex'), bytes: bytes.length, png: pngSize(bytes), ink: pngInk(bytes) });

const PAGE_PNG = pagePng(1224, 1584);

const PAGE_PNG_SHA = createHash('sha256').update(PAGE_PNG).digest('hex');

/** PAGE_PNG as re-read from disk, decoded once: the receipt stubs share it. */
const PAGE_FILE = pageFile(PAGE_PNG);

/** An all-white render of the right size: the image the review showed passing. */
const BLANK_PNG = pagePng(1224, 1584, { ink: false });

const PAGE_IMAGE = { page_no: 1, mimetype: 'image/png', width: 1224, height: 1584, dpi: 144, bytes: PAGE_PNG.length, sha256: PAGE_PNG_SHA, file: 'page-1.png' };

const prov = (bbox, page_no = 1) => [{ page_no, bbox: { coord_origin: 'BOTTOMLEFT', ...bbox }, charspan: [0, 5] }];

const BOX = { l: 72, t: 720, r: 300, b: 700 };

/** A docling JSON export: one page, a heading and a paragraph with boxes on it, one 2x2 table. */
function stubDocument(over = {}) {
  return {
    pages: { 1: { size: { width: 612, height: 792 }, page_no: 1 } },
    texts: [
      { self_ref: '#/texts/0', label: 'section_header', text: 'Title', prov: prov(BOX) },
      { self_ref: '#/texts/1', label: 'text', text: 'Hello world', prov: prov({ l: 72, t: 690, r: 300, b: 670 }) },
    ],
    tables: [
      {
        self_ref: '#/tables/0',
        label: 'table',
        prov: prov({ l: 72, t: 600, r: 400, b: 500 }),
        data: {
          grid: [[{ text: 'Name' }, { text: 'Qty' }], [{ text: 'Widget ' }, { text: '3' }]],
          table_cells: [{ text: 'Name', bbox: { l: 72, t: 192, r: 200, b: 212, coord_origin: 'TOPLEFT' } }],
        },
      },
    ],
    pictures: [],
    groups: [],
    ...over,
  };
}

test('page renders: one image per page with matching bytes and size, or the criterion fails', () => {
  const pages = [{ page_no: 1, width: 612, height: 792 }];
  const image = { ...PAGE_IMAGE, file: pageFile(PAGE_PNG) };
  const judge = (over = {}) =>
    judgePageRenders({ applicable: true, expectedPages: 1, libraryPageCount: { value: 1, error: null }, pages, images: [image], ...over });

  const good = judge();
  assert.equal(good.status, 'PASS');
  assert.deepEqual(good.renders, [
    { page_no: 1, width_px: 1224, height_px: 1584, mimetype: 'image/png', dpi: 144, bytes: PAGE_PNG.length, sha256: PAGE_PNG_SHA, px_per_page_unit: 2, declared_blank: false, uniform: false, differing_pixels: 2448 },
  ]);
  assert.deepEqual(pngSize(PAGE_PNG), { width: 1224, height: 1584 });
  assert.equal(pngSize(Buffer.from('not a png, but long enough to read')), null);

  const none = judge({ images: [] });
  assert.equal(none.status, 'unavailable');
  assert.match(none.reason, /generate_page_images\(true\) was applied and the returned document\.page_images is empty for 1 page/);

  const problems = (over) => {
    const judged = judge(over);
    assert.equal(judged.status, 'FAIL', JSON.stringify(over));
    return judged.problems.join('; ');
  };
  assert.match(problems({ pages: [...pages, { page_no: 2, width: 612, height: 792 }], expectedPages: 2, libraryPageCount: null }), /1 page image\(s\) for 2 page\(s\).*page 2 has no image/);
  assert.match(problems({ expectedPages: 3 }), /the fixture has 3/);
  assert.match(problems({ libraryPageCount: { value: 4, error: null } }), /pdf_page_count says 4/);
  assert.match(problems({ images: [{ ...image, file: { ...image.file, sha256: '0'.repeat(64) } }] }), /file hash differs/);
  assert.match(problems({ images: [{ ...image, file: { ...image.file, png: { width: 10, height: 10 } } }] }), /PNG is 10x10, the library reports 1224x1584/);
  assert.match(problems({ images: [{ ...image, file: { ...image.file, png: null } }] }), /not a PNG/);
  assert.match(problems({ images: [{ ...image, file: null }] }), /could not be re-read/);
  assert.match(problems({ images: [{ ...image, width: 0 }] }), /no pixels/);
  assert.match(problems({ pages: [] }), /no pages map/);

  assert.equal(judge({ applicable: false, images: [] }).status, 'not_applicable');
});

test('page renders: a render that is one flat colour fails unless the fixture declares that page blank', () => {
  const pages = [{ page_no: 1, width: 612, height: 792 }, { page_no: 2, width: 612, height: 792 }];
  const shown = (page_no, bytes) => ({ ...PAGE_IMAGE, page_no, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), file: pageFile(bytes) });
  const judge = (images, blankPages = []) =>
    judgePageRenders({ applicable: true, expectedPages: 2, libraryPageCount: { value: 2, error: null }, pages, images, blankPages });

  // The right size, the right hash, a valid PNG, and nothing on it.
  assert.deepEqual(pngSize(BLANK_PNG), { width: 1224, height: 1584 });
  const flat = judge([shown(1, PAGE_PNG), shown(2, BLANK_PNG)]);
  assert.equal(flat.status, 'FAIL');
  assert.deepEqual(flat.problems, ['page 2: the render is one flat colour (sample bytes 255) and SOURCES.json does not declare this page blank']);
  assert.equal(flat.renders[1].uniform, true);
  assert.equal(flat.renders[1].differing_pixels, 0);

  // The same render passes when the fixture says the page is blank, and the rule says so.
  const declared = judge([shown(1, PAGE_PNG), shown(2, BLANK_PNG)], [2]);
  assert.equal(declared.status, 'PASS');
  assert.equal(declared.renders[1].declared_blank, true);
  assert.deepEqual(declared.blank_pages, [2]);
  assert.match(declared.rule, /single flat colour fails unless SOURCES\.json declares that page blank/);
  assert.match(declared.rule, /declared blank must render flat/);

  // A declaration is an expectation, not a waiver.
  assert.match(judge([shown(1, PAGE_PNG), shown(2, PAGE_PNG)], [2]).problems.join('; '), /page 2: declared blank in SOURCES\.json but 2448 of 1938816 pixel\(s\) differ from the first/);
  assert.match(judge([shown(1, PAGE_PNG), shown(2, PAGE_PNG)], [3]).problems.join('; '), /blank page 3 is declared in SOURCES\.json and is not a page of the document/);

  // Pixels that cannot be read are not judged; they are never taken for ink.
  const unread = judge([shown(1, PAGE_PNG), { ...shown(2, PAGE_PNG), file: { ...pageFile(PAGE_PNG), ink: { decoded: false, reason: 'bit depth 1' } } }]);
  assert.equal(unread.status, 'not_judged');
  assert.deepEqual(unread.undecoded, ['page 2: the pixels could not be read (bit depth 1)']);
  assert.equal(judge([shown(1, PAGE_PNG), { ...shown(2, PAGE_PNG), mimetype: 'image/jpeg' }]).status, 'not_judged');
  assert.equal(judge([shown(1, BLANK_PNG), { ...shown(2, PAGE_PNG), mimetype: 'image/jpeg' }]).status, 'FAIL', 'a flat page fails whatever else could not be read');
});

test('evidence is what is on disk: every file a converter process wrote is re-read and re-hashed', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'okf-docling-evidence-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
  const markdown = Buffer.from('## Title\n\nHello world\n');
  const json = Buffer.from(JSON.stringify(stubDocument()));
  const written = {
    markdown_file: 'document.md',
    markdown_sha256: sha(markdown),
    json_file: 'document.json',
    json_sha256: sha(json),
    page_images: [{ ...PAGE_IMAGE, file: 'page-1.png' }],
  };
  const write = async (over = {}) => {
    const files = { 'document.md': markdown, 'document.json': json, 'page-1.png': PAGE_PNG, ...over };
    for (const [name, bytes] of Object.entries(files)) {
      if (bytes === null) await rm(join(dir, name), { force: true });
      else await writeFile(join(dir, name), bytes);
    }
  };

  await write();
  const good = await loadEvidence(dir, written);
  assert.deepEqual(good.problems, []);
  assert.equal(good.markdown, markdown.toString('utf8'));
  assert.deepEqual(good.document, stubDocument());
  assert.deepEqual(good.pageFiles, { 'page-1.png': pageFile(PAGE_PNG) });
  assert.equal(good.pageFiles['page-1.png'].ink.uniform, false, 'the pixels of the file on disk are read');

  // The bytes on disk changed after the process recorded their hash.
  await write({ 'document.md': Buffer.from('## Title\n\nGoodbye\n') });
  assert.deepEqual((await loadEvidence(dir, written)).problems, ['document.md: hash on disk differs from the receipt']);
  await write({ 'document.json': Buffer.from(JSON.stringify(stubDocument({ texts: [] }))) });
  assert.deepEqual((await loadEvidence(dir, written)).problems, ['document.json: hash on disk differs from the receipt']);
  await write({ 'page-1.png': BLANK_PNG });
  const swapped = await loadEvidence(dir, written);
  assert.deepEqual(swapped.problems, ['page-1.png: hash on disk differs from the receipt']);
  assert.equal(swapped.pageFiles['page-1.png'].sha256, sha(BLANK_PNG), 'what is recorded is the file as found');

  // A file that is not there, or not what its name says.
  await write({ 'document.md': null, 'page-1.png': null });
  const missing = await loadEvidence(dir, written);
  assert.equal(missing.markdown, null);
  assert.equal(missing.pageFiles['page-1.png'], null);
  assert.equal(missing.problems.length, 2);
  assert.match(missing.problems[0], /^document\.md: .*ENOENT/);
  assert.match(missing.problems[1], /^page-1\.png: .*ENOENT/);
  const notJson = Buffer.from('{ not json');
  await write({ 'document.json': notJson });
  const broken = await loadEvidence(dir, { ...written, json_sha256: sha(notJson) });
  assert.equal(broken.document, null);
  assert.match(broken.problems.join(' | '), /document\.json: not JSON/);

  // A process that returned no document wrote nothing to read.
  assert.deepEqual(await loadEvidence(dir, null), { markdown: null, document: null, pageFiles: {}, problems: [] });
});

/** One PNG row filtered the way an encoder would write it (PNG specification, filter types 0 to 4). */
function filterRow(type, row, previous, step) {
  const paeth = (a, b, c) => {
    const p = a + b - c;
    const [pa, pb, pc] = [Math.abs(p - a), Math.abs(p - b), Math.abs(p - c)];
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
  };
  return Uint8Array.from(row, (value, i) => {
    const left = i >= step ? row[i - step] : 0;
    const up = previous ? previous[i] : 0;
    const upLeft = previous && i >= step ? previous[i - step] : 0;
    return [value, value - left, value - up, value - ((left + up) >> 1), value - paeth(left, up, upLeft)][type] & 0xff;
  });
}

/** A PNG whose rows all use one filter type. Chunk checksums are zero: the reader under test does not check them. */
function filteredPng({ width, height, colourType, depth = 8, interlace = 0 }, pixels, type) {
  const step = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 }[colourType] * (depth / 8);
  const stride = width * step;
  const rows = [];
  for (let y = 0; y < height; y += 1) {
    const row = pixels.subarray(y * stride, (y + 1) * stride);
    rows.push(Buffer.from([type]), Buffer.from(filterRow(type, row, y > 0 ? pixels.subarray((y - 1) * stride, y * stride) : null, step)));
  }
  const chunk = (tag, data) => {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(data.length);
    head.write(tag, 4, 'latin1');
    return Buffer.concat([head, data, Buffer.alloc(4)]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([depth, colourType, 0, 0, interlace], 8);
  const data = deflateSync(Buffer.concat(rows));
  // Two IDAT chunks: the image data of a real file is often split.
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header), chunk('IDAT', data.subarray(0, 5)), chunk('IDAT', data.subarray(5)), chunk('IEND', Buffer.alloc(0))]);
}

test('the PNG reader undoes every row filter before it calls an image flat or inked', async () => {
  const shape = { width: 5, height: 4, colourType: 2 };
  const flat = new Uint8Array(5 * 4 * 3);
  for (let i = 0; i < flat.length; i += 3) flat.set([200, 120, 40], i);
  const dotted = Uint8Array.from(flat);
  dotted.set([200, 121, 40], (2 * 5 + 3) * 3);
  for (const type of [0, 1, 2, 3, 4]) {
    // A flat image written with a filter has rows of unequal bytes: only the unfiltered pixels are flat.
    assert.deepEqual(pngInk(filteredPng(shape, flat, type)), { decoded: true, width: 5, height: 4, pixels: 20, differing_pixels: 0, uniform: true, first_pixel: [200, 120, 40] }, `filter ${type}`);
    const inked = pngInk(filteredPng(shape, dotted, type));
    assert.equal(inked.uniform, false, `filter ${type}`);
    assert.equal(inked.differing_pixels, 1, `filter ${type}: one pixel differs by one sample`);
  }
  assert.equal(pngInk(filteredPng({ width: 2, height: 2, colourType: 6 }, new Uint8Array(16).fill(9), 4)).uniform, true);
  assert.equal(pngInk(filteredPng({ width: 2, height: 2, colourType: 0, depth: 16 }, Uint8Array.from([1, 2, 1, 2, 1, 2, 1, 3]), 1)).differing_pixels, 1, '16-bit samples are compared whole');

  assert.deepEqual(pngInk(Buffer.from('not a png at all')), { decoded: false, reason: 'the bytes do not start with the PNG signature' });
  assert.match(pngInk(filteredPng({ width: 2, height: 2, colourType: 3 }, new Uint8Array(4), 0)).reason, /colour type 3.*not a format this reader decodes/);
  assert.match(pngInk(filteredPng({ width: 2, height: 2, colourType: 2, interlace: 1 }, new Uint8Array(12), 0)).reason, /interlace 1/);
  assert.equal(pngInk(filteredPng({ width: 3, height: 2, colourType: 2 }, new Uint8Array(18), 0).subarray(0, 50)).reason, 'image data does not inflate: unexpected end of file');
  assert.equal(pngInk(PAGE_PNG.subarray(0, 60)).reason, 'chunk IDAT is cut short');
  assert.match(pngInk(filteredPng({ width: 3, height: 2, colourType: 2 }, new Uint8Array(12), 0)).reason, /image data is 14 bytes; 3x2 at 3 byte\(s\) per pixel needs 20/);
  assert.equal(pngInk(null).decoded, false);

  // The committed OCR fixture is a real file with strokes on it; an all-white page is not.
  const fixture = pngInk(await readFile(join(root, 'tests/fixtures/documents/text_image.png')));
  assert.equal(fixture.uniform, false);
  assert.ok(fixture.differing_pixels > 1000);
  assert.equal(pngInk(BLANK_PNG).uniform, true);
});

test('provenance: every text item, table and picture of a paginated fixture needs a page of the document and a box inside it', () => {
  const judge = (over, paginated = true) => judgeProvenance(describeDocument(stubDocument(over)), { paginated });
  const good = judge();
  assert.equal(good.status, 'PASS');
  assert.deepEqual(good.text_items, { total: 2, with_provenance: 2, located: 2, invalid_total: 0, invalid: [] });
  assert.deepEqual(good.tables, { total: 1, with_provenance: 1, located: 1, invalid_total: 0, invalid: [] });
  assert.deepEqual(good.page_provenance, [{ page_no: 1, text_items: 2, tables: 1, pictures: 0 }]);
  assert.deepEqual(good.sample.first, { ref: '#/texts/0', label: 'section_header', text: 'Title', page_no: 1, bbox: { coord_origin: 'BOTTOMLEFT', ...BOX } });
  assert.equal(good.sample.last.text, 'Hello world');
  assert.equal(good.sample.heading.ref, '#/texts/0');
  assert.equal(good.sample.table_cell.text, 'Name');
  assert.equal(good.sample.table_cell.bbox.coord_origin, 'TOPLEFT');

  const text = (provValue) => ({ texts: [{ self_ref: '#/texts/0', label: 'text', text: 'x', prov: provValue }] });
  const problem = (provValue) => {
    const judged = judge(text(provValue));
    assert.equal(judged.status, 'FAIL', JSON.stringify(provValue));
    assert.equal(judged.text_items.located, 0);
    return judged.text_items.invalid[0].problem;
  };
  assert.equal(problem([]), 'no provenance');
  assert.match(problem(prov(BOX, 2)), /page_no 2 is not a page of the document \(1\.\.=1\)/);
  assert.match(problem(prov(BOX, 0)), /page_no 0 is not a page/);
  assert.match(problem(prov({ ...BOX, r: 700 })), /outside the 612 x 792 page/);
  assert.match(problem(prov({ ...BOX, t: 800 })), /outside/);
  assert.match(problem(prov({ ...BOX, l: -5 })), /outside/);
  assert.equal(problem(prov({ l: 0, t: 0, r: 0, b: 0 })), 'bbox has no area', 'the library writes a zero box for an item without geometry');
  assert.equal(problem(prov({ ...BOX, t: 700, b: 720 })), 'bbox has no area', 'a bottom-left box has t above b');
  assert.equal(problem([{ page_no: 1, bbox: null }]), 'no bbox');
  assert.match(problem([{ page_no: 1, bbox: { ...BOX, coord_origin: 'SIDEWAYS' } }]), /unknown coord_origin/);
  assert.equal(bboxProblem({ l: 10, t: 20, r: 30, b: 40, coord_origin: 'TOPLEFT' }, { width: 612, height: 792 }), null);
  assert.equal(bboxProblem({ l: 10, t: 40, r: 30, b: 20, coord_origin: 'TOPLEFT' }, { width: 612, height: 792 }), 'bbox has no area');

  // One good item does not hide a bad one.
  const mixed = judge({ texts: [...stubDocument().texts, { self_ref: '#/texts/2', label: 'text', text: 'lost', prov: [] }] });
  assert.equal(mixed.status, 'FAIL');
  assert.deepEqual(mixed.text_items.invalid, [{ ref: '#/texts/2', label: 'text', problem: 'no provenance' }]);

  // Tables and pictures decide the status exactly as text items do (review note N3).
  const lostTable = judge({ tables: stubDocument().tables.map((table) => ({ ...table, prov: [] })) });
  assert.equal(lostTable.status, 'FAIL', 'a table with empty prov fails although every text item is located');
  assert.deepEqual(lostTable.text_items.invalid, []);
  assert.deepEqual(lostTable.tables.invalid, [{ ref: '#/tables/0', label: 'table', problem: 'no provenance' }]);
  assert.deepEqual(lostTable.items, { total: 3, located: 2 });
  const picture = (provValue) => ({ pictures: [{ self_ref: '#/pictures/0', label: 'picture', prov: provValue }] });
  assert.equal(judge(picture(prov(BOX))).status, 'PASS');
  assert.deepEqual(judge(picture(prov(BOX))).items, { total: 4, located: 4 });
  const lostPicture = judge(picture(prov({ l: 0, t: 0, r: 0, b: 0 })));
  assert.equal(lostPicture.status, 'FAIL');
  assert.deepEqual(lostPicture.pictures.invalid, [{ ref: '#/pictures/0', label: 'picture', problem: 'bbox has no area' }]);
  assert.match(good.rule, /every text item, table and picture has provenance/);
  assert.doesNotMatch(good.rule, /do not decide/);

  // A document without text is still judged by what it does hold; only an empty one is not.
  assert.equal(judge({ texts: [] }).status, 'PASS');
  assert.equal(judge({ texts: [], tables: [], ...picture(prov(BOX)) }).status, 'PASS');
  assert.equal(judge({ texts: [], tables: [], ...picture([]) }).status, 'FAIL');
  const empty = judge({ texts: [], tables: [] });
  assert.equal(empty.status, 'not_exercised');
  assert.equal(empty.reason, 'the converted document holds no text item, table or picture');
  const office = judge({ pages: {}, texts: [{ self_ref: '#/texts/0', label: 'text', text: 'x', prov: [] }], tables: [], groups: [{ label: 'sheet', name: 'Sheet1' }] }, false);
  assert.equal(office.status, 'recorded_not_judged');
  assert.equal(office.locator, 'none');
  assert.deepEqual(office.groups, ['sheet:Sheet1']);
  assert.equal(judge({}, false).locator, 'page_and_bbox_on_every_item');
  // An item that carries a prov entry with a zero box (slide notes) is not located, and the locator word says so.
  const notes = judge({ texts: [...stubDocument().texts, { self_ref: '#/texts/2', label: 'text', text: 'notes', prov: prov({ l: 0, t: 0, r: 0, b: 0 }) }] }, false);
  assert.equal(notes.locator, 'page_and_bbox_on_some_items');
  assert.equal(notes.text_items.with_provenance, 3);
  assert.equal(notes.text_items.located, 2);
  // The list of invalid items is capped; the count is not.
  const many = judge({ texts: Array.from({ length: 12 }, (_, index) => ({ self_ref: `#/texts/${index}`, label: 'caption', text: 'c', prov: [] })) });
  assert.equal(many.text_items.invalid.length, 10);
  assert.equal(many.text_items.invalid_total, 12);
});

test('the document facts come from the body layer of the export', () => {
  const doc = describeDocument(
    stubDocument({
      texts: [
        { self_ref: '#/texts/0', label: 'title', text: 'T', prov: [] },
        { self_ref: '#/texts/1', label: 'section_header', text: 'S', prov: [] },
        { self_ref: '#/texts/2', label: 'page_header', text: 'H', content_layer: 'furniture', prov: [] },
        { self_ref: '#/texts/3', label: 'section_header', text: 'hidden', content_layer: 'invisible', prov: [] },
      ],
      tables: [...stubDocument().tables, { self_ref: '#/tables/1', content_layer: 'invisible', prov: [], data: { grid: [[{ text: 'header' }]] } }],
      pictures: [{ self_ref: '#/pictures/0', prov: [], captions: [{ $ref: '#/texts/9' }], image: { uri: 'data:image/png;base64,AA' } }],
      groups: [{ label: 'sheet', name: 'Sheet1', content_layer: 'body' }, { label: 'sheet', name: 'Sheet4', content_layer: 'invisible' }],
    }),
  );
  const facts = bodyFacts(doc);
  assert.equal(facts.headings, 2);
  assert.deepEqual(facts.tables, [[['Name', 'Qty'], ['Widget ', '3']]]);
  assert.equal(facts.pictures, 1);
  assert.equal(facts.pictures_with_image, 1);
  assert.equal(facts.pictures_with_caption, 1);
  assert.deepEqual(facts.sheet_names, ['Sheet1']);
  assert.equal(facts.items_on_other_layers, 3);
  assert.deepEqual(describeDocument(null), { pages: [], texts: [], tables: [], pictures: [], groups: [] });
});

test('converter options are lifted from the Debug text of the converter as raw Rust literals', () => {
  assert.deepEqual(converterOptions(CONVERTER_DEBUG, ['generate_page_images', 'images_scale', 'ocr_lang', 'document_timeout', 'nope']), {
    generate_page_images: 'true',
    images_scale: 'None',
    ocr_lang: 'Some("en")',
    document_timeout: 'None',
    nope: null,
  });
  assert.equal(converterOptions(CONVERTER_DEBUG.replace('document_timeout: None', 'document_timeout: Some(1ms)'), ['document_timeout']).document_timeout, 'Some(1ms)');
});

const NO_FALLBACK_ERROR = 'parse error: pdf: pdf: pdfium support is not compiled in (docling-pdf feature `pdfium`)';

const DOCLING_TREE = [
  'okf-qualify-docling v0.1.0 (D:\\okf\\qualification\\docling)|',
  'docling v1.93.5|pdf',
  'docling-core v1.93.6|',
  'docling-pdf v1.93.6|ml,ocr-prep',
  'docling-core v1.93.6| (*)',
  'lopdf v0.44.0|chrono,default,jiff,rayon,time',
].join('\n');

test('a refusal is classified from its error text alone', () => {
  assert.equal(classifyRefusal(NO_FALLBACK_ERROR), 'primary_parser_failed_no_fallback_in_build');
  assert.equal(classifyRefusal('parse error: pdf: unexpected end of stream at byte 330'), 'converter_rejected');
  assert.equal(classifyRefusal(''), 'converter_rejected');
});

test('build features are read from the manifests and from cargo, never assumed', async () => {
  const declared = declaredDoclingFeatures(
    await readFile(join(root, 'qualification/docling/Cargo.toml'), 'utf8'),
    await readFile(join(root, 'Cargo.toml'), 'utf8'),
  );
  assert.deepEqual(declared.features, ['pdf'], 'the pinned build enables pdf only; pdfium is not to be switched on here');
  assert.equal(declared.default_features, false);
  assert.match(declared.declared_in, /workspace\.dependencies/);
  assert.deepEqual(declaredDoclingFeatures('docling = { version = "1", features = ["pdf", "pdfium"] }', '').features, ['pdf', 'pdfium']);
  assert.deepEqual(
    declaredDoclingFeatures('docling = { workspace = true, features = ["pdfium"] }', 'docling = { version = "1", features = ["pdf"] }').features,
    ['pdf', 'pdfium'],
  );
  assert.throws(() => declaredDoclingFeatures('serde = "1"', ''), /no inline docling dependency/);

  assert.deepEqual(resolvedFeatures(DOCLING_TREE)['docling-pdf'], ['ml', 'ocr-prep']);
  assert.deepEqual(resolvedFeatures(DOCLING_TREE).docling, ['pdf']);
  assert.deepEqual(resolvedFeatures(DOCLING_TREE)['docling-core'], []);
  const withPdfium = buildFacts({
    harnessToml: 'docling = { workspace = true }',
    workspaceToml: 'docling = { version = "1", features = ["pdfium"] }',
    treeText: 'docling v1.93.5|pdf,pdfium\ndocling-pdf v1.93.6|ml,ocr-prep,pdfium\npdfium-render v0.8.0|default',
    command: 'c',
  });
  assert.equal(withPdfium.pdfium_compiled_in, true);
  assert.throws(
    () => buildFacts({ harnessToml: 'docling = { workspace = true }', workspaceToml: 'docling = { features = ["pdf"] }', treeText: 'serde v1.0.0|std', command: 'c' }),
    /did not list docling/,
  );
});

const DOCLING_HEADER = {
  git_sha: 'a'.repeat(40),
  inputs: DOCLING_INPUTS,
  produced_at: '2026-10-05T18:00:00.000Z',
};

const DOCLING_CONVERTER = {
  crate: 'docling',
  version: '1.93.5',
  checksum: 'abc',
  docling_core_versions: ['1.93.6'],
  source: 'Cargo.lock',
};

const DOCLING_BUILD = buildFacts({
  harnessToml: 'docling = { workspace = true }\n',
  workspaceToml: 'docling = { version = "=1.93.5", default-features = false, features = ["pdf"] }\n',
  treeText: DOCLING_TREE,
  command: 'cargo build --locked --release -p okf-qualify-docling',
});

const DOCLING_ASSETS = { ...assetsMatched([VERIFIED_MODEL]), manifest_sha256: 'e'.repeat(64), bytes_total: 10, files: [VERIFIED_MODEL] };

const isPaginated = (only) => /\.(pdf|png)$/.test(only);

const STUB_KIND = (only) => ({ pdf: 'pdf', png: 'image', docx: 'docx', xlsx: 'xlsx', pptx: 'pptx' })[only.split('.').pop()];

const STUB_SOURCE = (only) => ({
  role: 'stub',
  kind: STUB_KIND(only),
  expect: { confirmed_by: 'test stub', markdown_contains: ['Hello world'], tables: 1, ...(isPaginated(only) ? { pages: 1 } : {}) },
});

const DOCLING_SOURCES = {
  files: Object.fromEntries(
    FIXTURE_RUNS.filter((only) => only !== TIMEOUT_PROBE).map((only) => [only, only === MUST_FAIL ? { role: 'must_fail', kind: 'pdf' } : STUB_SOURCE(only)]),
  ),
};

/** A fixture process that behaved: exit 0, measured, receipt and evidence on disk. `over` replaces parts. */
function doclingRun(only, over = {}) {
  const supported = only !== MUST_FAIL && only !== TIMEOUT_PROBE;
  const paginated = isPaginated(only);
  const fixture = {
    fixture: only,
    outcome: only === MUST_FAIL ? 'PASS_explicit_failure' : 'PASS',
    conversion_rule: only === MUST_FAIL ? 'converter_refuses_the_input' : 'success_status_and_nonblank_markdown',
    stage: only === MUST_FAIL ? 'converter_error' : 'converter_status',
    status: only === MUST_FAIL ? null : only === TIMEOUT_PROBE ? 'PartialSuccess' : 'Success',
    finding: null,
    input_format: only === TIMEOUT_PROBE || only.endsWith('.pdf') ? 'pdf' : only.endsWith('.png') ? 'image' : only.split('.').pop(),
    library_page_count: only.endsWith('.pdf') && supported ? { value: 1, error: null } : null,
    errors:
      only === MUST_FAIL
        ? [{ component_type: 'converter', module_name: 'DocumentConverter::convert', error_message: NO_FALLBACK_ERROR }]
        : [],
    settings: { converter_debug: CONVERTER_DEBUG },
    // The probe converts the bytes of scanned_image_only.pdf, as the Rust harness does.
    sha256_before: createHash('sha256').update(only === TIMEOUT_PROBE ? 'scanned_image_only.pdf' : only).digest('hex'),
    document:
      only === MUST_FAIL
        ? null
        : { markdown_file: 'document.md', json_file: 'document.json', page_images: supported && paginated ? [PAGE_IMAGE] : [] },
    ...over.fixture,
  };
  // The spent budget of the real run returned an empty document: no page, no item.
  const evidence =
    only === TIMEOUT_PROBE
      ? { markdown: '', document: { pages: {}, texts: [], tables: [], pictures: [], groups: [] }, pageFiles: {}, problems: [], ...over.evidence }
      : {
          markdown: '## Title\n\nHello   world\n',
          document: stubDocument(paginated ? {} : { pages: {}, texts: stubDocument().texts.map((item) => ({ ...item, prov: [] })), tables: stubDocument().tables.map((item) => ({ ...item, prov: [] })) }),
          pageFiles: { 'page-1.png': PAGE_FILE },
          problems: [],
          ...over.evidence,
        };
  return {
    only,
    run: {
      exitCode: 0,
      signal: null,
      spawnError: null,
      timedOut: false,
      done: { okf_docling: 'done', only },
      peakRssBytes: 1_048_576,
      peakRssNote: 'stub source',
      stderr: '',
      stdoutSha256: 'f'.repeat(64),
      ...over.run,
    },
    report:
      'report' in over
        ? over.report
        : only === TIMEOUT_PROBE
          ? { model_inventory: DOCLING_INVENTORY, receipts: [], timeout_case: fixture }
          : { model_inventory: DOCLING_INVENTORY, receipts: [fixture], timeout_case: null },
    evidence: 'evidence' in over && over.evidence === undefined ? undefined : only === MUST_FAIL ? undefined : evidence,
  };
}

function doclingReceipt(overrides = {}, parts = {}) {
  const runs = FIXTURE_RUNS.map((only) => doclingRun(only, overrides[only]));
  return buildDoclingReceipt({
    header: DOCLING_HEADER,
    converter: DOCLING_CONVERTER,
    platform: 'win32',
    build: DOCLING_BUILD,
    assets: DOCLING_ASSETS,
    environment: { DOCLING_RS_MODELS_DIR: 'C:\\models' },
    sources: DOCLING_SOURCES,
    runs,
    scope: { mode: 'all', fixtures: FIXTURE_RUNS, qualification: true },
    paths: { fixtures_dir: 'tests/fixtures/documents' },
    finishedAt: '2026-10-05T18:10:00.000Z',
    ...parts,
  });
}

const entryOf = (receipt, only) => receipt.receipts.find((item) => item.only === only);

const doclingCriterion = (receipt, id) => receipt.criteria.find((item) => item.id === id);

const doclingFailedIds = (receipt) => receipt.criteria.filter((item) => item.result === 'fail').map((item) => item.id);

/** The envelope can be trusted: the result is the shared fold and every pinned criterion is there. */
function assertDoclingEnvelope(receipt, sources = DOCLING_SOURCES) {
  assert.deepEqual(doclingEnvelope.envelopeFailures(receipt, doclingCriteria.GATE, doclingCriteria.requiredIds(sources)), []);
  assert.equal(receipt.result, doclingEnvelope.foldCriteria(receipt.criteria, receipt.harness_error));
  assert.deepEqual(receipt.not_judged, receipt.criteria.filter((item) => item.result === 'not_judged' || item.result === 'not_applicable').map((item) => item.id));
  assert.deepEqual(Object.keys(receipt.not_judged_reasons), receipt.not_judged);
  for (const item of receipt.criteria) {
    if (item.result !== 'pass') assert.ok(typeof item.detail === 'string' && item.detail.length > 10, `${item.id} is ${item.result} without a reason`);
  }
}

test('a Docling receipt carries the shared header and envelope, and passes when every rule holds', () => {
  const receipt = doclingReceipt();
  assert.deepEqual(receiptHeaderProblems(receipt), []);
  assert.deepEqual(Object.keys(receipt).slice(0, 9), ['git_sha', 'inputs', 'produced_at', 'gate', 'result', 'harness_error', 'not_judged', 'not_judged_reasons', 'criteria']);
  assert.equal(receipt.gate, 'docling-library-qualification');
  assert.equal(receipt.gate, doclingCriteria.GATE);
  assert.equal(receipt.result, 'PASS');
  assert.equal(receipt.harness_error, null);
  assertDoclingEnvelope(receipt);
  assert.deepEqual(receipt.criteria.map(({ id, required }) => ({ id, required })), doclingCriteria.expectedCriteria(DOCLING_SOURCES));
  assert.equal(receipt.orchestrator, undefined);
  assert.deepEqual(receipt.failures, []);
  assert.equal(receipt.receipts.length, FIXTURE_RUNS.length - 1);
  assert.equal(receipt.timeout_case.outcome, 'PASS');
  assert.deepEqual(Object.keys(receipt.summary), FIXTURE_RUNS);
  assert.deepEqual([...new Set(Object.values(receipt.summary))], ['PASS']);
  assert.deepEqual(Object.keys(receipt.criteria_summary), ['conversion', 'evidence', 'format_recognised', 'content', 'page_renders', 'provenance', 'memory_measured', 'refused', 'refusal_attributed', 'timeout_reported', 'budget_had_effect']);
  const pdf = entryOf(receipt, 'born_digital_text.pdf');
  assert.deepEqual(Object.keys(pdf.criteria), ['conversion', 'evidence', 'format_recognised', 'content', 'page_renders', 'provenance', 'memory_measured']);
  assert.deepEqual([...new Set(Object.values(pdf.criteria).map((item) => item.result))], ['pass']);
  assert.equal(pdf.criteria.page_renders.status, undefined, 'a criterion carries one verdict word: its result');
  assert.equal(pdf.outcome, 'PASS');
  assert.equal(pdf.page_image_count, 1);
  assert.deepEqual(pdf.page_provenance, [{ page_no: 1, text_items: 2, tables: 1, pictures: 0, has_image: true }]);
  assert.equal(pdf.role, 'stub');
  // Every criterion states what it asserts, in the receipt.
  for (const { id } of doclingCriteria.expectedCriteria(DOCLING_SOURCES)) {
    const rule = receipt.criterion_rules[id] ?? receipt.criterion_rules[id.split('/').pop()];
    assert.ok(typeof rule === 'string' && rule.length > 40, `${id} has no rule sentence`);
  }
  assert.match(receipt.match_rules.markdown_contains, /every run of whitespace is collapsed to one space/);
});

test('the result of a Docling receipt is the shared fold of its criteria, and nothing else decides it', async () => {
  const scenarios = {
    PASS: doclingReceipt(),
    FAIL: doclingReceipt({ 'corpus/word_sample.docx': { evidence: { markdown: '## Title\n\nGoodbye\n' } } }),
    INCOMPLETE: doclingReceipt({ 'table_heavy.pdf': { report: null, run: { exitCode: 1 } } }),
  };
  for (const [expected, receipt] of Object.entries(scenarios)) {
    assert.equal(receipt.result, expected);
    assertDoclingEnvelope(receipt);
    // A fixture's label and the summary are the same fold over that fixture's own criteria.
    for (const only of FIXTURE_RUNS) {
      const own = receipt.criteria.filter((item) => item.id.startsWith(`${only}/`));
      const entry = only === TIMEOUT_PROBE ? receipt.timeout_case : entryOf(receipt, only);
      const stopped = Object.values(entry.criteria).find((item) => item.harness_error)?.harness_error ?? null;
      assert.equal(receipt.summary[only], doclingEnvelope.foldCriteria(own, stopped), only);
      assert.equal(entry.outcome, receipt.summary[only], only);
      for (const item of own) assert.equal(receipt.criteria_summary[item.id.slice(only.length + 1)][only], item.result, item.id);
    }
  }
  // A result typed over the criteria is caught by the shared check.
  assert.deepEqual(doclingEnvelope.envelopeFailures({ ...scenarios.FAIL, result: 'PASS' }, doclingCriteria.GATE, doclingCriteria.requiredIds(DOCLING_SOURCES)), ['result is PASS but its criteria fold to FAIL']);
  // No harness file writes a result word of its own; only criteria.mjs calls the fold.
  for (const file of ['run.mjs', 'lib/receipt.mjs', 'lib/criteria.mjs', 'lib/document.mjs', 'lib/expect.mjs', 'lib/assets.mjs']) {
    const source = await readFile(join(root, 'qualification/docling', file), 'utf8');
    assert.doesNotMatch(source, /\bresult\s*[:=]\s*[^;\n]*['"`](PASS|FAIL|INCOMPLETE)['"`]/, `${file} types a result`);
    assert.equal(/foldCriteria\(/.test(source), file === 'lib/criteria.mjs', `${file}: only criteria.mjs folds`);
  }
  assert.deepEqual(doclingCriteria.EXIT_CODES, { PASS: 0, FAIL: 2, INCOMPLETE: 3 });
  assert.deepEqual(['PASS', 'FAIL', 'INCOMPLETE', 'anything else'].map(doclingCriteria.exitCodeFor), [0, 2, 3, 3]);
});

test('criteria.json lists exactly the criteria the Docling harness can emit as required', async () => {
  const sources = JSON.parse(await readFile(join(root, 'tests/fixtures/documents/SOURCES.json'), 'utf8'));
  const tracked = JSON.parse(await readFile(join(root, 'qualification/docling/criteria.json'), 'utf8'));
  assert.deepEqual(Object.keys(tracked), ['gate', 'required']);
  assert.equal(tracked.gate, doclingCriteria.GATE);
  // Derived from SOURCES.json and the run-level list, without converting anything.
  assert.deepEqual(tracked.required, doclingCriteria.requiredIds(sources));
  assert.deepEqual(tracked.required, [...tracked.required].sort());
  const emitted = doclingCriteria.expectedCriteria(sources);
  assert.equal(new Set(emitted.map((item) => item.id)).size, emitted.length, 'every id is emitted once');
  for (const { id } of emitted) assert.match(id, /^\S+\/[a-z_]+$/, id);
  assert.deepEqual(emitted.slice(0, 2), [{ id: 'assets/hashes_match', required: true }, { id: 'assets/model_inventory', required: true }]);
  // The only criteria not required are measurements recorded without a verdict.
  assert.deepEqual(emitted.filter((item) => !item.required).map((item) => item.id), ['sample_sheet.xlsx/provenance', 'corpus/xlsx_01.xlsx/provenance', 'corpus/powerpoint_sample.pptx/provenance']);

  // The receipt emits every one of them even when nothing ran, so none can go missing unseen.
  const nothing = buildDoclingReceipt({ header: DOCLING_HEADER, platform: 'win32', sources, runs: [], scope: { mode: 'all' }, paths: {}, finishedAt: 'x' });
  assert.deepEqual(nothing.criteria.map(({ id, required }) => ({ id, required })), emitted);
  assert.deepEqual(doclingEnvelope.envelopeFailures(nothing, tracked.gate, tracked.required), []);
  assert.equal(nothing.result, 'INCOMPLETE');
  const dropped = { ...nothing, criteria: nothing.criteria.filter((item) => item.id !== 'corpus/redp5110_sampled.pdf/provenance') };
  assert.deepEqual(doclingEnvelope.envelopeFailures(dropped, tracked.gate, tracked.required), ['pinned criterion corpus/redp5110_sampled.pdf/provenance is missing']);

  // What is judged follows the declared kind, and the declared kind is what the bytes are.
  const magic = { pdf: /^%PDF-1\.\d/, image: /^\x89PNG\r\n\x1a\n/, docx: /^PK[\s\S]*word\/document\.xml/, xlsx: /^PK[\s\S]*xl\/workbook\.xml/, pptx: /^PK[\s\S]*ppt\/presentation\.xml/ };
  for (const [name, entry] of Object.entries(sources.files)) {
    assert.ok(Object.hasOwn(doclingCriteria.KINDS, entry.kind), `${name} declares kind ${entry.kind}`);
    assert.match((await readFile(join(root, 'tests/fixtures/documents', name))).toString('latin1'), magic[entry.kind], `${name} is not a ${entry.kind} file`);
  }
  assert.throws(() => doclingCriteria.fixtureAspects('x.pdf', { role: 'stub' }), /x\.pdf must declare kind as one of pdf, image, docx, xlsx, pptx; found null/);
  assert.deepEqual(doclingCriteria.fixtureAspects('a.docx', { kind: 'docx' }).find((item) => item.aspect === 'provenance'), { aspect: 'provenance', required: true });
});

test('memory is a measurement with no limit applied, and a peak that could not be read is a harness error', () => {
  const measured = doclingReceipt();
  assert.deepEqual(measured.memory.limit_bytes, null);
  assert.equal(measured.memory.limit_applied, false);
  assert.match(measured.memory.statement, /applies no memory limit/);
  assert.equal(measured.memory.peak_rss_bytes['sample_sheet.xlsx'], 1_048_576);
  assert.deepEqual(entryOf(measured, 'sample_sheet.xlsx').memory, {
    peak_rss_bytes: 1_048_576,
    peak_rss_note: 'stub source',
    limit_bytes: null,
    limit_applied: false,
  });
  assert.equal(measured.memory_summary, undefined, 'a bare measurement is never worded as a pass');
  assert.doesNotMatch(JSON.stringify(measured), /"memory":\s*"PASS"/);

  const receipt = doclingReceipt({
    'sample_sheet.xlsx': { run: { peakRssBytes: null, peakRssNote: 'Get-Process reported no PeakWorkingSet64' } },
  });
  const entry = entryOf(receipt, 'sample_sheet.xlsx');
  assert.equal(entry.memory.peak_rss_bytes, null);
  assert.equal(entry.memory.peak_rss_note, 'Get-Process reported no PeakWorkingSet64');
  assert.equal(entry.criteria.memory_measured.result, 'not_judged');
  assert.equal(entry.conversion_outcome, 'PASS');
  assert.equal(entry.outcome, 'INCOMPLETE');
  assert.equal(receipt.criteria_summary.memory_measured['sample_sheet.xlsx'], 'not_judged');
  assert.equal(receipt.harness_error, 'the peak memory of the converter process for sample_sheet.xlsx was not measured: Get-Process reported no PeakWorkingSet64');
  assert.deepEqual(doclingFailedIds(receipt), [], 'a sampler that did not answer says nothing about the library');
  assert.equal(receipt.result, 'INCOMPLETE');
  assertDoclingEnvelope(receipt);
});

test('a truncated PDF the converter accepts is a finding and a FAIL, recorded as observed', () => {
  const receipt = doclingReceipt({
    [MUST_FAIL]: {
      fixture: {
        outcome: 'FAIL_expected_failure',
        stage: 'converter_status',
        status: 'Success',
        finding: 'converter accepts truncated PDF',
        errors: [],
      },
    },
  });
  assert.equal(receipt.finding, 'converter accepts truncated PDF');
  assert.equal(receipt.summary[MUST_FAIL], 'FAIL');
  assert.deepEqual(doclingFailedIds(receipt), [`${MUST_FAIL}/refused`]);
  assert.equal(doclingCriterion(receipt, `${MUST_FAIL}/refused`).detail, 'the converter did not refuse the truncated input: FAIL_expected_failure (converter accepts truncated PDF)');
  assert.equal(doclingCriterion(receipt, `${MUST_FAIL}/refusal_attributed`).result, 'not_judged');
  assert.equal(entryOf(receipt, MUST_FAIL).refusal, undefined, 'an accepted input has no refusal to classify');
  assert.equal(receipt.result, 'FAIL');
  assertDoclingEnvelope(receipt);
});

test('a refusal counts only when another PDF converted in the same run', () => {
  const pdfs = FIXTURE_RUNS.filter((only) => only.endsWith('.pdf') && only !== MUST_FAIL);
  const overrides = Object.fromEntries(
    pdfs.map((only) => [only, { fixture: { outcome: 'FAIL_converter_error', stage: 'converter_error', status: null, document: null } }]),
  );
  const receipt = doclingReceipt(overrides);
  const attributed = doclingCriterion(receipt, `${MUST_FAIL}/refusal_attributed`);
  assert.equal(attributed.result, 'not_judged');
  assert.match(attributed.detail, /no other PDF fixture converted successfully in this run/);
  assert.equal(doclingCriterion(receipt, `${MUST_FAIL}/refused`).result, 'pass');
  assert.equal(receipt.summary[MUST_FAIL], 'INCOMPLETE', 'an unattributed refusal is not a pass');
  assert.deepEqual(entryOf(receipt, MUST_FAIL).refusal.other_pdfs_converted, []);
  assert.deepEqual(doclingFailedIds(receipt), pdfs.map((only) => `${only}/conversion`));
  assert.equal(receipt.result, 'FAIL');

  const counted = entryOf(doclingReceipt(), MUST_FAIL);
  assert.equal(counted.criteria.refusal_attributed.result, 'pass');
  assert.deepEqual(counted.refusal.other_pdfs_converted, pdfs);
  // A PDF whose process failed did not convert either.
  const crashed = doclingReceipt(Object.fromEntries(pdfs.map((only) => [only, { run: { exitCode: 1 } }])));
  assert.equal(doclingCriterion(crashed, `${MUST_FAIL}/refusal_attributed`).result, 'not_judged');
});

test('the must-fail receipt says what kind of refusal it was, from the error text and the build', () => {
  assert.equal(DOCLING_BUILD.pdfium_compiled_in, false);
  assert.deepEqual(DOCLING_BUILD.build_features, ['pdf']);
  const refusal = entryOf(doclingReceipt(), MUST_FAIL).refusal;
  assert.equal(refusal.stage, 'converter_error');
  assert.equal(refusal.error_text, NO_FALLBACK_ERROR);
  assert.equal(refusal.refusal_kind, 'primary_parser_failed_no_fallback_in_build');
  assert.deepEqual(refusal.build_features, ['pdf']);
  assert.equal(refusal.pdfium_compiled_in, false);
  assert.match(refusal.meaning, /no pdfium fallback/);

  const rejected = doclingReceipt({
    [MUST_FAIL]: { fixture: { errors: [{ component_type: 'converter', module_name: 'm', error_message: 'parse error: pdf: bad xref' }] } },
  });
  assert.equal(entryOf(rejected, MUST_FAIL).refusal.refusal_kind, 'converter_rejected');
  assert.equal(entryOf(rejected, MUST_FAIL).refusal.meaning, undefined);
  assert.equal(rejected.result, 'PASS');

  // The text says "no pdfium" while cargo resolved pdfium: the classification cannot stand.
  const contradicted = doclingReceipt({}, { build: { ...DOCLING_BUILD, pdfium_compiled_in: true } });
  assert.deepEqual(doclingFailedIds(contradicted), [`${MUST_FAIL}/refusal_attributed`]);
  assert.match(doclingCriterion(contradicted, `${MUST_FAIL}/refusal_attributed`).detail, /cargo did not resolve this build without pdfium/);
  assert.equal(contradicted.summary[MUST_FAIL], 'FAIL');
  assert.equal(contradicted.result, 'FAIL');
});

test('a converter process that failed, or a fixture that never ran, is INCOMPLETE with a harness error and no library verdict', () => {
  const stoppedBy = (over, sentence) => {
    const receipt = doclingReceipt({ 'table_heavy.pdf': over });
    const entry = entryOf(receipt, 'table_heavy.pdf');
    assert.equal(entry.outcome, 'INCOMPLETE', sentence);
    assert.equal(receipt.summary['table_heavy.pdf'], 'INCOMPLETE');
    assert.deepEqual([...new Set(Object.values(entry.criteria).map((item) => item.result))], ['not_judged'], 'nothing is judged from a process that failed');
    assert.equal(receipt.harness_error, sentence);
    assert.deepEqual(doclingFailedIds(receipt), [], 'an environment failure is never a fail');
    assert.equal(receipt.result, 'INCOMPLETE');
    assert.deepEqual(receipt.failures, [{ fixture: 'table_heavy.pdf', criterion: null, result: 'not_judged', detail: sentence }], 'a fixture whose process failed has one entry of its own');
    assertDoclingEnvelope(receipt);
    return entry;
  };
  const crashed = stoppedBy(
    { report: null, run: { exitCode: 1, stderr: 'okf-qualify-docling: missing fixture\n' } },
    'the converter process for table_heavy.pdf exited 1: okf-qualify-docling: missing fixture',
  );
  assert.equal(crashed.harness_exit_code, 1);
  assert.match(crashed.harness_stderr, /missing fixture/);
  // A non-zero exit is not trusted although a receipt is on disk (the review's surviving mutant).
  stoppedBy({ run: { exitCode: 101, stderr: 'thread main panicked' } }, 'the converter process for table_heavy.pdf exited 101: thread main panicked');
  stoppedBy({ report: null }, 'the converter process for table_heavy.pdf exited 0 without a readable receipt');
  stoppedBy({ report: null, run: { exitCode: null, spawnError: 'ENOENT: no such file or directory, uv_spawn' } }, 'the converter process for table_heavy.pdf could not be started: ENOENT: no such file or directory, uv_spawn');
  stoppedBy({ report: null, run: { exitCode: null, signal: 'SIGTERM', timedOut: true } }, 'the converter process for table_heavy.pdf was killed at the harness timeout');
  stoppedBy({ report: null, run: { exitCode: null, signal: 'SIGKILL' } }, 'the converter process for table_heavy.pdf ended on signal SIGKILL');

  // A harness error does not hide a failure the library did produce, and several are counted.
  const both = doclingReceipt({
    'table_heavy.pdf': { report: null, run: { exitCode: 1 } },
    'scanned_text.pdf': { report: null, run: { exitCode: 1 } },
    'corpus/word_sample.docx': { evidence: { markdown: 'Goodbye' } },
  });
  assert.equal(both.result, 'FAIL');
  assert.equal(both.harness_error, 'the converter process for scanned_text.pdf exited 1 (and 1 more harness error(s); each is the detail of the criteria it stopped)');
  assert.deepEqual(doclingFailedIds(both), ['corpus/word_sample.docx/content']);

  const partial = doclingReceipt({}, { runs: [doclingRun('sample_sheet.xlsx')], scope: { mode: 'only', fixtures: ['sample_sheet.xlsx'], qualification: false } });
  assert.equal(partial.summary['sample_sheet.xlsx'], 'PASS');
  assert.equal(partial.summary[TIMEOUT_PROBE], 'INCOMPLETE');
  assert.equal(partial.result, 'INCOMPLETE', 'a single-fixture run is never the qualification');
  assert.equal(partial.harness_error, null, 'nothing in the environment failed: the fixtures were not asked for');
  const notRun = FIXTURE_RUNS.filter((only) => only !== 'sample_sheet.xlsx');
  assert.deepEqual(partial.failures.filter((item) => item.criterion === null), notRun.map((fixture) => ({ fixture, criterion: null, result: 'not_judged', detail: 'the fixture did not run' })));
  assert.equal(doclingCriterion(partial, 'born_digital_text.pdf/content').detail, 'the fixture did not run');
  assertDoclingEnvelope(partial);
});

test('a run that stopped before any conversion writes an INCOMPLETE envelope with every criterion not judged', () => {
  const sentence = 'the Docling model assets could not be verified: Docling model asset missing: C:\\models\\layout.onnx (ENOENT). A missing asset is never qualified.';
  const receipt = buildDoclingReceipt({
    header: DOCLING_HEADER,
    platform: 'win32',
    sources: DOCLING_SOURCES,
    runs: [],
    scope: { mode: 'all', fixtures: FIXTURE_RUNS, qualification: true },
    paths: {},
    finishedAt: '2026-10-05T18:10:00.000Z',
    harnessError: sentence,
  });
  assert.equal(receipt.result, 'INCOMPLETE');
  assert.equal(receipt.harness_error, sentence);
  assert.deepEqual([...new Set(receipt.criteria.map((item) => item.result))], ['not_judged']);
  assert.equal(receipt.criteria.length, doclingCriteria.expectedCriteria(DOCLING_SOURCES).length);
  assert.equal(doclingCriterion(receipt, 'assets/hashes_match').detail, `not reached: ${sentence}`);
  assert.equal(doclingCriterion(receipt, 'scanned_text.pdf/conversion').detail, `the fixture did not run: ${sentence}`);
  assert.deepEqual([...new Set(Object.values(receipt.summary))], ['INCOMPLETE']);
  assert.equal(receipt.assets_verified, null);
  assert.equal(receipt.build, null);
  assertDoclingEnvelope(receipt);
});

test('each content, page-render and provenance failure fails its fixture and the receipt, and its detail gives the total and the examples', () => {
  {
    const receipt = doclingReceipt({ 'corpus/word_sample.docx': { evidence: { markdown: '## Title\n\nGoodbye\n' } } });
    const entry = entryOf(receipt, 'corpus/word_sample.docx');
    assert.equal(entry.fixture, 'corpus/word_sample.docx');
    assert.equal(entry.outcome, 'FAIL');
    assert.equal(entry.conversion_outcome, 'PASS');
    assert.deepEqual(entry.failed_criteria, ['content']);
    assert.equal(receipt.result, 'FAIL');
    assert.deepEqual(receipt.failures, [
      { fixture: 'corpus/word_sample.docx', criterion: 'content', result: 'fail', detail: '1 of 2 expectations are not met; all: markdown_contains "Hello world"' },
    ]);
    assert.deepEqual(entry.criteria.content.checks.filter((check) => !check.ok).map((check) => check.expected), ['Hello world']);
    const counted = doclingReceipt({ 'table_heavy.pdf': { evidence: { document: stubDocument({ tables: [] }) } } });
    assert.equal(doclingCriterion(counted, 'table_heavy.pdf/content').detail, '1 of 2 expectations are not met; all: tables 1 (observed 0)');
  }
  {
    const receipt = doclingReceipt({ 'scanned_text.pdf': { fixture: { document: { markdown_file: 'document.md', json_file: 'document.json', page_images: [] } } } });
    const entry = entryOf(receipt, 'scanned_text.pdf');
    assert.equal(entry.criteria.page_renders.result, 'fail');
    assert.match(entry.criteria.page_renders.detail, /generate_page_images\(true\) was applied and the returned document\.page_images is empty for 1 page/);
    assert.equal(entry.outcome, 'FAIL');
    assert.equal(entry.page_image_count, 0);
    assert.equal(entry.page_provenance[0].has_image, false);
    assert.equal(receipt.result, 'FAIL');
    assert.equal(entryOf(receipt, 'sample_sheet.xlsx').criteria.page_renders.result, 'not_applicable');
  }
  {
    // The image the review showed passing: the right size and hash, and all white.
    const blank = { ...PAGE_IMAGE, bytes: BLANK_PNG.length, sha256: createHash('sha256').update(BLANK_PNG).digest('hex') };
    const receipt = doclingReceipt({
      'born_digital_text.pdf': {
        fixture: { document: { markdown_file: 'document.md', json_file: 'document.json', page_images: [blank] } },
        evidence: { pageFiles: { 'page-1.png': pageFile(BLANK_PNG) } },
      },
    });
    assert.deepEqual(doclingFailedIds(receipt), ['born_digital_text.pdf/page_renders']);
    assert.equal(
      doclingCriterion(receipt, 'born_digital_text.pdf/page_renders').detail,
      '1 problem(s) over 1 page image(s) for 1 page(s); all: page 1: the render is one flat colour (sample bytes 255) and SOURCES.json does not declare this page blank',
    );
    assert.equal(receipt.criterion_rules.page_renders, PAGE_RENDER_RULE);
    assert.equal(receipt.result, 'FAIL');
    // The same render passes for a fixture that declares the page blank.
    const sources = structuredClone(DOCLING_SOURCES);
    sources.files['born_digital_text.pdf'].expect.blank_pages = [1];
    const declared = doclingReceipt({ 'born_digital_text.pdf': { fixture: { document: { markdown_file: 'document.md', json_file: 'document.json', page_images: [blank] } }, evidence: { pageFiles: { 'page-1.png': pageFile(BLANK_PNG) } } } }, { sources });
    assert.equal(declared.result, 'PASS');
    // Pixels that could not be read leave the criterion unjudged.
    const unread = doclingReceipt({ 'born_digital_text.pdf': { evidence: { pageFiles: { 'page-1.png': { ...pageFile(PAGE_PNG), ink: { decoded: false, reason: 'bit depth 1' } } } } } });
    assert.equal(doclingCriterion(unread, 'born_digital_text.pdf/page_renders').detail, 'the pixels of 1 of 1 page image(s) for 1 page(s) could not be read; all: page 1: the pixels could not be read (bit depth 1)');
    assert.equal(unread.result, 'INCOMPLETE');
  }
  {
    const captions = Array.from({ length: 12 }, (_, index) => ({ self_ref: `#/texts/${index}`, label: 'caption', text: 'c', prov: [] }));
    const receipt = doclingReceipt({ 'born_digital_text.pdf': { evidence: { document: stubDocument({ texts: captions }) } } });
    const entry = entryOf(receipt, 'born_digital_text.pdf');
    assert.equal(entry.criteria.provenance.result, 'fail');
    assert.deepEqual(entry.failed_criteria, ['provenance']);
    assert.equal(entry.outcome, 'FAIL');
    const shown = captions.slice(0, 10).map((item) => `${item.self_ref} caption: no provenance`).join('; ');
    assert.deepEqual(receipt.failures, [
      {
        fixture: 'born_digital_text.pdf',
        criterion: 'provenance',
        result: 'fail',
        detail: `12 of 13 items are not located (12 of 12 text items, 0 of 1 tables, 0 of 0 pictures); first 10: ${shown}`,
      },
    ]);
    assert.equal(entry.criteria.provenance.text_items.invalid_total, 12);
    assert.equal(receipt.criteria_summary.provenance['born_digital_text.pdf'], 'fail');
    assert.equal(receipt.criteria_summary.provenance['sample_sheet.xlsx'], 'not_judged');
    assert.equal(receipt.result, 'FAIL');
    // A table without a location fails the fixture although every text item is located.
    const lostTable = doclingReceipt({ 'table_heavy.pdf': { evidence: { document: stubDocument({ tables: stubDocument().tables.map((table) => ({ ...table, prov: [] })) }) } } });
    assert.equal(doclingCriterion(lostTable, 'table_heavy.pdf/provenance').detail, '1 of 3 items are not located (0 of 2 text items, 1 of 1 tables, 0 of 0 pictures); all: #/tables/0 table: no provenance');
    assert.deepEqual(doclingFailedIds(lostTable), ['table_heavy.pdf/provenance']);
  }
});

test('what is judged for a fixture follows its declared kind, and a format the library reports otherwise fails', () => {
  // The review's case: a PDF the library reports as Markdown, with no page image and no locator.
  const unlocated = stubDocument({ pages: {}, texts: stubDocument().texts.map((item) => ({ ...item, prov: [] })), tables: stubDocument().tables.map((item) => ({ ...item, prov: [] })) });
  const receipt = doclingReceipt({
    'born_digital_text.pdf': {
      fixture: { input_format: 'md', library_page_count: null, document: { markdown_file: 'document.md', json_file: 'document.json', page_images: [] } },
      evidence: { document: unlocated },
    },
  });
  const entry = entryOf(receipt, 'born_digital_text.pdf');
  assert.equal(entry.criteria.format_recognised.result, 'fail');
  assert.equal(entry.criteria.format_recognised.detail, 'the library reports format "md" for a fixture SOURCES.json declares pdf (format "pdf")');
  assert.equal(entry.criteria.page_renders.result, 'fail', 'page renders are still judged: the fixture is a PDF');
  assert.equal(entry.criteria.provenance.result, 'fail', 'provenance is still judged: the fixture is a PDF');
  assert.deepEqual(entry.failed_criteria, ['format_recognised', 'page_renders', 'provenance']);
  assert.equal(entry.outcome, 'FAIL');
  assert.equal(receipt.result, 'FAIL');
  assert.match(receipt.criterion_rules.format_recognised, /follows the declared kind, never the reported format/);

  // The other direction: an Office file reported as a PDF does not gain page-render criteria, and fails on the format.
  const office = doclingReceipt({ 'sample_with_image.docx': { fixture: { input_format: 'pdf' } } });
  assert.deepEqual(doclingFailedIds(office), ['sample_with_image.docx/format_recognised']);
  assert.equal(doclingCriterion(office, 'sample_with_image.docx/page_renders').result, 'not_applicable');
  assert.equal(doclingCriterion(doclingReceipt({ 'sample_image.png': { fixture: { input_format: null } } }), 'sample_image.png/format_recognised').detail, 'the library reports format null for a fixture SOURCES.json declares image (format "image")');
});

test('the receipt lists at its top every criterion that was not judged or not applicable, with the reason', () => {
  const receipt = doclingReceipt();
  const office = FIXTURE_RUNS.filter((only) => /\.(docx|xlsx|pptx)$/.test(only));
  assert.deepEqual(
    receipt.not_judged,
    office.flatMap((only) => [`${only}/page_renders`, `${only}/provenance`]),
    'ten judgements were not made in a passing run, and the receipt says which',
  );
  assert.equal(receipt.not_judged_reasons['corpus/word_sample.docx/provenance'], 'not_applicable: the library gives DOCX items no page or box');
  assert.equal(receipt.not_judged_reasons['sample_sheet.xlsx/page_renders'], 'not_applicable: the library keeps page images for the PDF/image pipeline only (docling converter.rs:756 generate_page_images)');
  assert.equal(
    receipt.not_judged_reasons['corpus/powerpoint_sample.pptx/provenance'],
    'not_judged: not a PDF or image fixture: the locator the library gives is recorded as observed and no rule is applied (0 of 3 items located, locator none)',
  );
  // A DOCX locator is not applicable, never a pass; a spreadsheet's is a measurement and is not required.
  const docx = doclingCriterion(receipt, 'sample_with_image.docx/provenance');
  assert.deepEqual(docx, { id: 'sample_with_image.docx/provenance', required: true, result: 'not_applicable', detail: 'the library gives DOCX items no page or box' });
  assert.equal(entryOf(receipt, 'sample_with_image.docx').criteria.provenance.locator, 'none');
  assert.deepEqual(entryOf(receipt, 'sample_with_image.docx').not_judged, ['page_renders', 'provenance']);
  assert.equal(doclingCriterion(receipt, 'corpus/xlsx_01.xlsx/provenance').required, false);
  assert.equal(entryOf(receipt, 'corpus/xlsx_01.xlsx').criteria.provenance.items.total, 3);

  // If the library did locate DOCX items, "no page or box" would be untrue: the run stops short of PASS for a decision.
  const located = doclingReceipt({ 'sample_with_image.docx': { evidence: { document: stubDocument() } } });
  assert.equal(doclingCriterion(located, 'sample_with_image.docx/provenance').detail, 'the library located 3 of 3 DOCX items; this harness has no rule for a DOCX locator');
  assert.equal(located.result, 'INCOMPLETE');
});

test('a fixture that shows no glyphs is judged for invented text, by a criterion named for it', async () => {
  const sources = JSON.parse(await readFile(join(root, 'tests/fixtures/documents/SOURCES.json'), 'utf8'));
  const ids = doclingCriteria.expectedCriteria(sources).map((item) => item.id);
  for (const name of ['scanned_image_only.pdf', 'sample_image.png']) {
    assert.ok(ids.includes(`${name}/no_invented_text`), name);
    assert.ok(!ids.includes(`${name}/content`), `${name}: "content" would claim more than is asserted`);
    assert.ok(ids.includes(`${name}/text_provenance`), name);
    assert.ok(ids.includes(`${name}/provenance`), `${name}: its pictures are still located`);
  }
  assert.equal(ids.filter((id) => id.endsWith('/no_invented_text')).length, 2);
  assert.equal(ids.filter((id) => id.endsWith('/text_provenance')).length, 2);

  // One picture, no text: the stub of a page without glyphs.
  const stub = structuredClone(DOCLING_SOURCES);
  stub.files['scanned_image_only.pdf'].expect = { confirmed_by: 'test stub', no_text: true, pages: 1 };
  const picture = stubDocument({ texts: [], tables: [], pictures: [{ self_ref: '#/pictures/0', label: 'picture', prov: prov(BOX) }] });
  const receipt = (markdown, document = picture) => doclingReceipt({ 'scanned_image_only.pdf': { evidence: { markdown, document } } }, { sources: stub });

  const silent = receipt('<!-- image -->\n\n<!-- image -->\n');
  assert.equal(silent.result, 'PASS');
  const entry = entryOf(silent, 'scanned_image_only.pdf');
  assert.deepEqual(Object.keys(entry.criteria), ['conversion', 'evidence', 'format_recognised', 'no_invented_text', 'page_renders', 'provenance', 'text_provenance', 'memory_measured']);
  assert.equal(entry.criteria.no_invented_text.result, 'pass');
  assert.equal(entry.criteria.provenance.result, 'pass', 'the picture is located');
  assert.deepEqual(doclingCriterion(silent, 'scanned_image_only.pdf/text_provenance'), {
    id: 'scanned_image_only.pdf/text_provenance',
    required: true,
    result: 'not_applicable',
    detail: 'the fixture shows no glyphs, so the converter is to produce no text item and there is none whose location could be judged; its pictures and tables are located by provenance',
  });
  assert.ok(silent.not_judged.includes('scanned_image_only.pdf/text_provenance'));
  assert.match(silent.criterion_rules.no_invented_text, /the converter invents no text for a page without glyphs/);
  assert.match(silent.criterion_rules.no_invented_text, /holds no letter and no digit/);
  assertDoclingEnvelope(silent, stub);

  // Any letter or digit outside the picture placeholders is invented text.
  for (const [markdown, tokens, shown] of [
    ['<!-- image -->\n\nIll\n', 1, 'ill'],
    ['<!-- image -->\n\n7\n', 1, '7'],
    ['<!-- image -->\n\nl I 1 | O 0\n', 5, 'l i 1 o 0'],
  ]) {
    const invented = receipt(markdown);
    assert.deepEqual(doclingFailedIds(invented), ['scanned_image_only.pdf/no_invented_text'], markdown);
    assert.equal(
      doclingCriterion(invented, 'scanned_image_only.pdf/no_invented_text').detail,
      `1 of 1 expectations are not met; all: no_text: ${tokens} letter or digit token(s) outside the picture placeholders (${shown})`,
    );
    assert.equal(invented.result, 'FAIL');
  }
  assert.equal(receipt('<!-- WATER 47 -->\n').result, 'PASS', 'a placeholder comment is not text');
});

test('the timeout probe passes only when the budget changed the result, compared with the same bytes converted in full', () => {
  const budget = (receipt) => doclingCriterion(receipt, `${TIMEOUT_PROBE}/budget_had_effect`);
  const good = doclingReceipt();
  assert.deepEqual(Object.keys(good.timeout_case.criteria), ['timeout_reported', 'evidence', 'budget_had_effect', 'memory_measured']);
  assert.equal(good.timeout_case.sha256_before, entryOf(good, 'scanned_image_only.pdf').sha256_before, 'the probe converts the bytes of a fixture that is also converted in full');
  assert.deepEqual(budget(good), {
    id: `${TIMEOUT_PROBE}/budget_had_effect`,
    required: true,
    result: 'pass',
    detail: 'the probe returned 0 of 1 page(s) and 0 of 3 item(s) of the full conversion of scanned_image_only.pdf',
  });
  assert.deepEqual(good.timeout_case.criteria.budget_had_effect.full, { fixture: 'scanned_image_only.pdf', pages: 1, items: 3 });
  assert.match(good.criterion_rules.budget_had_effect, /fewer pages or fewer items than the conversion of the same bytes without a budget/);
  assert.match(good.criterion_rules.budget_had_effect, /elapsed time is recorded and not judged/);

  // A timeout that did nothing: the status and the error item are there, and so is the whole document.
  const noop = doclingReceipt({ [TIMEOUT_PROBE]: { evidence: { document: stubDocument() } } });
  assert.equal(doclingCriterion(noop, `${TIMEOUT_PROBE}/timeout_reported`).result, 'pass');
  assert.equal(budget(noop).result, 'fail');
  assert.equal(
    budget(noop).detail,
    'the probe reported a spent budget and still returned 1 page(s) and 3 item(s), no fewer than the full conversion of scanned_image_only.pdf (1 page(s), 3 item(s)): the budget had no visible effect',
  );
  assert.equal(noop.summary[TIMEOUT_PROBE], 'FAIL');
  assert.equal(noop.result, 'FAIL');
  // Fewer items on the same page count is an effect.
  assert.equal(budget(doclingReceipt({ [TIMEOUT_PROBE]: { evidence: { document: stubDocument({ tables: [] }) } } })).detail, 'the probe returned 1 of 1 page(s) and 2 of 3 item(s) of the full conversion of scanned_image_only.pdf');

  // Nothing to compare with is not judged, with the reason; it is never a pass.
  const unjudged = (overrides, reason, result = 'INCOMPLETE') => {
    const receipt = doclingReceipt(overrides);
    assert.deepEqual([budget(receipt).result, budget(receipt).detail], ['not_judged', reason]);
    assert.equal(receipt.result, result, reason);
  };
  unjudged(
    { 'scanned_image_only.pdf': { fixture: { outcome: 'FAIL_converter_error', stage: 'converter_error', status: null, document: null } } },
    'the full conversion of scanned_image_only.pdf did not succeed in this run, so there is nothing to compare the probe with',
    'FAIL',
  );
  unjudged({ 'scanned_image_only.pdf': { run: { exitCode: 1 } } }, 'the full conversion of scanned_image_only.pdf did not succeed in this run, so there is nothing to compare the probe with');
  // A process that left no receipt recorded no input hash: which fixture held the same bytes is not known.
  unjudged({ 'scanned_image_only.pdf': { report: null, run: { exitCode: 1 } } }, 'no fixture of this run converted the same bytes without a budget, so there is nothing to compare the probe with');
  unjudged({ 'scanned_image_only.pdf': { fixture: { sha256_before: '0'.repeat(64) } } }, 'no fixture of this run converted the same bytes without a budget, so there is nothing to compare the probe with');
  unjudged({ [TIMEOUT_PROBE]: { fixture: { document: null } } }, 'the library returned no document for the probe, so nothing distinguishes it from the full conversion');
  unjudged({ [TIMEOUT_PROBE]: { evidence: { problems: ['document.json: hash on disk differs from the receipt'] } } }, 'the files the probe process wrote are not the ones it recorded (see evidence), so nothing is judged from them');
  unjudged(
    { [TIMEOUT_PROBE]: { fixture: { outcome: 'FAIL_timeout_not_honoured', status: 'Success' }, evidence: { document: stubDocument() } } },
    'the converter did not report a spent budget, so there is no effect to compare',
    'FAIL',
  );
  const silent = doclingReceipt({ [TIMEOUT_PROBE]: { fixture: { outcome: 'FAIL_timeout_not_honoured', status: 'Success' }, evidence: { document: stubDocument() } } });
  assert.equal(doclingCriterion(silent, `${TIMEOUT_PROBE}/timeout_reported`).detail, 'the converter did not report the spent budget as documented: FAIL_timeout_not_honoured, status Success, no error item');
  assert.deepEqual(doclingFailedIds(silent), [`${TIMEOUT_PROBE}/timeout_reported`]);
  assert.deepEqual(good.timeout_case.criteria.budget_had_effect.probe, { pages: 0, items: 0 });
});

test('evidence that is not what the process recorded stops the judgements made from it', () => {
  const receipt = doclingReceipt({ 'table_heavy.pdf': { evidence: { problems: ['document.md: hash on disk differs from the receipt', 'page-1.png: hash on disk differs from the receipt'] } } });
  const entry = entryOf(receipt, 'table_heavy.pdf');
  const sentence = '2 file(s) the converter process for table_heavy.pdf wrote are not the ones it recorded; all: document.md: hash on disk differs from the receipt; page-1.png: hash on disk differs from the receipt';
  assert.equal(entry.criteria.evidence.result, 'not_judged');
  assert.equal(entry.criteria.evidence.detail, sentence);
  assert.equal(receipt.harness_error, sentence);
  assert.deepEqual(['conversion', 'format_recognised', 'content', 'page_renders', 'provenance'].map((aspect) => entry.criteria[aspect].result), ['pass', 'pass', 'not_judged', 'not_judged', 'not_judged']);
  assert.match(entry.criteria.content.detail, /are not the ones it recorded \(see evidence\)/);
  assert.equal(entry.outcome, 'INCOMPLETE');
  assert.deepEqual(doclingFailedIds(receipt), []);
  assert.equal(receipt.result, 'INCOMPLETE');
  assert.equal(entryOf(doclingReceipt(), 'table_heavy.pdf').criteria.evidence.result, 'pass');
  // Files that were never re-read are not trusted either.
  const unread = doclingReceipt({ 'table_heavy.pdf': { evidence: undefined } });
  assert.equal(doclingCriterion(unread, 'table_heavy.pdf/evidence').detail, 'the files the converter process for table_heavy.pdf wrote were not re-read');
  assert.equal(unread.result, 'INCOMPLETE');

  // A conversion the library reported as failed is a fail; what would have been read from its document is not judged.
  const failed = doclingReceipt({
    'table_heavy.pdf': { fixture: { outcome: 'FAIL_converter_error', stage: 'converter_error', status: null, document: null, errors: [{ component_type: 'converter', module_name: 'm', error_message: 'layout model failed' }] } },
  });
  assert.deepEqual(doclingFailedIds(failed), ['table_heavy.pdf/conversion']);
  assert.equal(doclingCriterion(failed, 'table_heavy.pdf/conversion').detail, 'the converter did not convert the fixture: FAIL_converter_error: layout model failed');
  assert.equal(doclingCriterion(failed, 'table_heavy.pdf/content').detail, 'the conversion did not succeed (FAIL_converter_error), so there is no converted document to judge');
  assert.equal(doclingCriterion(failed, 'table_heavy.pdf/evidence').result, 'not_judged');
  assert.equal(failed.summary['table_heavy.pdf'], 'FAIL');
});

test('the receipt lists the converter settings the processes held and the assets it verified', () => {
  const probeDebug = CONVERTER_DEBUG.replace('document_timeout: None', 'document_timeout: Some(1ms)');
  // Each process has its own temporary artifacts_dir; that alone must not make a second converter.
  const otherDir = CONVERTER_DEBUG.replace('artifacts_dir: "C:', 'artifacts_dir: "D:');
  assert.notEqual(otherDir, CONVERTER_DEBUG);
  const receipt = doclingReceipt({
    [TIMEOUT_PROBE]: { fixture: { settings: { converter_debug: probeDebug } } },
    'sample_sheet.xlsx': { fixture: { settings: { converter_debug: otherDir } } },
  });
  assert.equal(receipt.settings.converters.length, 2);
  const [main, probe] = receipt.settings.converters;
  assert.equal(main.options.generate_page_images, 'true');
  assert.equal(main.options.images_scale, 'None');
  assert.equal(main.options.ocr_mode, 'None');
  assert.equal(main.options.no_table_former, 'false');
  assert.equal(main.options.document_timeout, 'None');
  assert.equal(main.used_by.length, FIXTURE_RUNS.length - 1);
  assert.equal(probe.options.document_timeout, 'Some(1ms)');
  assert.deepEqual(probe.used_by, [TIMEOUT_PROBE]);
  assert.deepEqual(receipt.settings.environment, { DOCLING_RS_MODELS_DIR: 'C:\\models' });
  assert.deepEqual([receipt.assets_verified.count, receipt.assets_verified.matched], [1, 1]);
  assert.equal(doclingCriterion(receipt, 'assets/hashes_match').result, 'pass');
});

test('assets or models that are not the verified ones make the run INCOMPLETE, never a library verdict', () => {
  // The record says a file's hash is not the manifest's: the criterion is counted from the record, not assumed.
  const changed = { ...VERIFIED_MODEL, sha256: '0'.repeat(64) };
  const receipt = doclingReceipt({}, { assets: { ...DOCLING_ASSETS, files: [VERIFIED_MODEL, { ...changed, file: 'ocr_det.onnx' }] } });
  const sentence = '1 of 2 model assets do not have the length and hash the manifest gives; all: ocr_det.onnx';
  assert.deepEqual(doclingCriterion(receipt, 'assets/hashes_match'), { id: 'assets/hashes_match', required: true, result: 'not_judged', detail: sentence });
  assert.equal(receipt.harness_error, sentence);
  assert.equal(receipt.result, 'INCOMPLETE');
  assert.deepEqual(receipt.failures, [{ fixture: null, criterion: 'assets/hashes_match', result: 'not_judged', detail: sentence }]);
  assert.equal(doclingCriterion(doclingReceipt({}, { assets: null }), 'assets/hashes_match').detail, 'no asset was hashed in this run');
  assert.equal(doclingCriterion(doclingReceipt({}, { assets: { ...DOCLING_ASSETS, files: [] } }), 'assets/hashes_match').detail, 'the asset manifest lists no file that was hashed');

  // A model the library resolves is not a verified file.
  const unverified = doclingReceipt({}, { assets: { ...DOCLING_ASSETS, files: [{ ...VERIFIED_MODEL, path: 'C:\\models\\other.onnx' }] } });
  assert.equal(unverified.model_inventory.result, 'not_judged');
  assert.equal(unverified.model_inventory.status, undefined);
  assert.deepEqual(unverified.model_inventory.unverified, ['layout']);
  assert.equal(doclingCriterion(unverified, 'assets/model_inventory').detail, '1 of 1 models the library resolves are not verified files; all: layout');
  assert.deepEqual(Object.values(unverified.summary).filter((label) => label !== 'PASS'), []);
  assert.deepEqual(doclingFailedIds(unverified), []);
  assert.equal(unverified.result, 'INCOMPLETE');
  assert.equal(unverified.failures[0].criterion, 'assets/model_inventory');
  assert.equal(doclingReceipt().model_inventory.result, 'pass');
});

test('the Docling orchestrator verifies assets before it builds, and the harness labels what it writes', async () => {
  const run = await readFile(join(root, 'qualification/docling/run.mjs'), 'utf8');
  assert.ok(run.indexOf('await verifyAssets(manifestBytes)') > 0, 'run.mjs must re-hash the assets');
  assert.ok(run.indexOf('await verifyAssets(manifestBytes)') < run.indexOf('await buildRelease('), 'assets are verified before anything is built or converted');
  assert.ok(run.indexOf('await receiptHeader(') < run.indexOf('await verifyAssets('), 'the clean-tree guard still comes first');
  assert.match(run, /if \(only\.length && record\) throw new Error/);
  assert.match(run, /exec\('cargo', TREE_ARGS/);

  const harness = await readFile(join(root, 'qualification/docling/src/main.rs'), 'utf8');
  assert.doesNotMatch(harness, /SuccessNonEmpty|must_contain|peak_rss_bytes/, 'no label for a rule that is not applied, no expectation outside SOURCES.json, no field that is always null');
  assert.match(harness, /\.generate_page_images\(applied\.generate_page_images\)/);
  assert.match(harness, /version_source: VERSION_SOURCE/);
  assert.match(harness, /Rule::TimeoutHonoured => "partial_success_with_pipeline_timeout_error"/);
});

const pidAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

test('waitForListening resolves on the listening line and the process can then be stopped', async () => {
  const proc = spawnGroup(process.execPath, [
    '-e',
    "process.stderr.write('okf-qualify-mcp-apps listening on http://127.0.0.1:1/mcp\\n');setInterval(()=>{},1000)",
  ]);
  try {
    const match = await within(
      waitForListening(proc, { pattern: /okf-qualify-mcp-apps listening on (\S+)/, label: 'harness' }),
      SLOW_MACHINE_BUDGET_MS,
      'listening',
    );
    assert.equal(match[1], 'http://127.0.0.1:1/mcp');
  } finally {
    await killProcessTree(proc);
  }
  assert.equal(pidAlive(proc.child.pid), false);
});

test('a harness that fails to bind is reported at once with its exit code and stderr', async () => {
  const proc = spawnGroup(process.execPath, [
    '-e',
    "process.stderr.write('okf-qualify-mcp-apps: bind 127.0.0.1:1: access denied\\n');process.exit(1)",
  ]);
  const started = Date.now();
  await assert.rejects(
    within(waitForListening(proc, { pattern: /listening on (\S+)/, timeoutMs: 60_000, label: 'harness' }), SLOW_MACHINE_BUDGET_MS, 'bind failure'),
    (error) => {
      assert.match(error.message, /harness exited before listening \(exit 1\)/);
      assert.match(error.message, /bind 127\.0\.0\.1:1: access denied/);
      return true;
    },
  );
  assert.ok(Date.now() - started < SLOW_MACHINE_BUDGET_MS);
});

test('killProcessTree stops grandchildren, not only the process it was given', async () => {
  const parent =
    "const {spawn}=require('node:child_process');const g=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});process.stdout.write(String(g.pid)+'\\n');setInterval(()=>{},1000)";
  const proc = spawnGroup(process.execPath, ['-e', parent]);
  let grandchild = null;
  try {
    for (let attempt = 0; attempt < 80 && grandchild === null; attempt += 1) {
      const match = /^(\d+)\r?\n/.exec(proc.stdout());
      if (match) grandchild = Number(match[1]);
      else await pause(50);
    }
    assert.ok(grandchild !== null, 'parent did not report its child pid');
    assert.equal(pidAlive(grandchild), true);
    await killProcessTree(proc);
    let gone = false;
    for (let attempt = 0; attempt < 60 && !gone; attempt += 1) {
      gone = !pidAlive(grandchild);
      if (!gone) await pause(50);
    }
    assert.equal(gone, true, `grandchild ${grandchild} survived killProcessTree`);
  } finally {
    if (grandchild !== null && pidAlive(grandchild)) process.kill(grandchild);
    await killProcessTree(proc);
  }
});

const REVISION = '0123456789abcdef0123456789abcdef01234567';
/** What each view's App frame shows for the committed fixtures (innerText, abbreviated). */
const VIEW_TEXT = {
  render_source: `fixtures/qualification-source.md ${REVISION}\nQualification source\nContract-valid ReadItemResponse fixture for the MCP Apps harness.`,
  render_changes: `Changes\n${REVISION} → 89abcdef0123456789abcdef0123456789abcdef\nfixtures/qualification-source.md\n@@ -1,3 +1,4 @@\n # Qualification source\n+\n+Harness fixture line.`,
  render_timeline: `Timeline\nQualification timeline fixture\nokf-qualify-mcp-apps · 2026-10-05T12:00:00Z\n89abcdef0123456789abcdef0123456789abcdef`,
  render_present: `Six-component catalog\nfixtures/qualification-source.md ${REVISION}\nQualification source\nContract-valid ReadItemResponse fixture for the MCP Apps harness.\nmetrics\ncategory\tvalue\nIngested\t412\nConverted\t397\nIndexed\t389\nReviewed\t127\nPublished\t61\nMetrics chart\nConverted\nIndexed\nIngested\nPublished\nReviewed\ncategory\nvalue\nSource data\nfixtures/qualification-source.md @ ${REVISION}\nfixtures/qualification-metrics.json @ ${REVISION}`,
};
/** The two alerts the present view showed while the fixture retained no dataset; never accepted again. */
const DATASET_UNAVAILABLE = ['Dataset unavailable: metrics', 'Resolved chart data or specification unavailable.'];
/** The present frame as it was then: the table and the chart replaced by those alerts. */
const PRESENT_TEXT_WITHOUT_DATASET = `Six-component catalog\nfixtures/qualification-source.md ${REVISION}\nQualification source\nContract-valid ReadItemResponse fixture for the MCP Apps harness.\nDataset unavailable: metrics\nMetrics chart\nResolved chart data or specification unavailable.\nfixtures/qualification-source.md @ ${REVISION}\nfixtures/qualification-metrics.json @ ${REVISION}`;

test('each of the four views is recognised by its own text and by no other view', () => {
  assert.deepEqual(VIEWS.map((view) => view.tool), ['render_source', 'render_changes', 'render_timeline', 'render_present']);
  for (const view of VIEWS) {
    assert.equal(
      basicHostUrl(view.tool),
      `http://127.0.0.1:8080/?server=okf-qualify-mcp-apps&tool=${view.tool}&call=true&theme=hide`,
    );
    for (const other of VIEWS) {
      const verdict = judgeView(view, { text: VIEW_TEXT[other.tool], alerts: other.alerts });
      assert.equal(verdict.ok, view === other, `${view.tool} judged against ${other.tool}: ${JSON.stringify(verdict)}`);
    }
  }
});

test('a view that is waiting, disconnected, unparsed or showing an unexpected alert is not rendered', () => {
  const source = VIEWS[0];
  assert.equal(judgeView(source, null).ok, false);
  assert.equal(judgeView(source, { text: 'Waiting for a tool result from the connected host.', alerts: [] }).ok, false);
  assert.equal(
    judgeView(source, { text: `${VIEW_TEXT.render_source}\nHost connection failed: x`, alerts: ['Host connection failed: x'] }).ok,
    false,
  );
  const present = VIEWS[3];
  const broken = judgeView(present, { text: VIEW_TEXT.render_present, alerts: ['MCP error -32602: unknown tool: show'] });
  assert.equal(broken.ok, false);
  assert.deepEqual(broken.alerts, ['MCP error -32602: unknown tool: show']);
});

test('the present view is rendered only with its chart and table, and never again with a dataset-unavailable alert', () => {
  const present = VIEWS[3];
  assert.equal(present.tool, 'render_present');
  assert.deepEqual(present.alerts, [], 'the present view tolerates no alert at all');
  const rendered = judgeView(present, { text: VIEW_TEXT.render_present, alerts: [] });
  assert.equal(rendered.ok, true, JSON.stringify(rendered));
  assert.deepEqual(rendered.expected_alerts, []);

  // Either alert, alone or together, on an otherwise complete frame: not rendered.
  for (const alerts of [[DATASET_UNAVAILABLE[0]], [DATASET_UNAVAILABLE[1]], DATASET_UNAVAILABLE]) {
    const verdict = judgeView(present, { text: VIEW_TEXT.render_present, alerts });
    assert.equal(verdict.ok, false, `accepted ${JSON.stringify(alerts)}`);
    assert.deepEqual(verdict.missing, []);
  }
  // The frame this harness used to accept, with its two alerts and without them.
  for (const alerts of [DATASET_UNAVAILABLE, []]) {
    const verdict = judgeView(present, { text: PRESENT_TEXT_WITHOUT_DATASET, alerts });
    assert.equal(verdict.ok, false, `accepted the dataset-less frame with alerts ${JSON.stringify(alerts)}`);
    assert.deepEqual(verdict.foreign, ['Dataset unavailable', 'chart data or specification unavailable']);
    assert.deepEqual(verdict.missing, ['Source data']);
  }
});

test('harness tool-call reports are read back in order and nothing else on stderr is', () => {
  assert.deepEqual([...APP_ONLY_TOOLS].sort(), ['read_object', 'show']);
  const stderr = [
    'okf-qualify-mcp-apps listening on http://127.0.0.1:18765/mcp',
    `${TOOL_CALL_LOG_PREFIX}{"ok":true,"tool":"render_present"}`,
    `${TOOL_CALL_LOG_PREFIX}{"ok":true,"tool":"show"}\r`,
    `${TOOL_CALL_LOG_PREFIX}{"has_more":true,"object":"ab","offset":"0","ok":true,"tool":"read_object"}`,
    `  ${TOOL_CALL_LOG_PREFIX}{"ok":true,"tool":"indented"}`,
    `${TOOL_CALL_LOG_PREFIX}["not","a","record"]`,
    `${TOOL_CALL_LOG_PREFIX}{"ok":true}`,
    `${TOOL_CALL_LOG_PREFIX}{"has_more":false,"object":"ab","off`,
  ].join('\n');
  assert.deepEqual(toolCallsFrom(stderr), [
    { ok: true, tool: 'render_present' },
    { ok: true, tool: 'show' },
    { has_more: true, object: 'ab', offset: '0', ok: true, tool: 'read_object' },
  ]);
  assert.deepEqual(toolCallsFrom(''), []);
  assert.deepEqual(toolCallsFrom(undefined), []);
});

/** The committed dataset as the App must show it, with the digest of its exact bytes. */
async function committedDataset() {
  const bytes = await readFile(join(root, PRESENT_DATASET.fixture));
  const digest = createHash('sha256').update(bytes).digest('hex');
  return { bytes, digest, expected: datasetExpectation({ binding: PRESENT_DATASET.binding, digest, rows: JSON.parse(bytes.toString('utf8')) }) };
}

test('the present fixture retains exactly the committed dataset, and the expectation is the table DataTable draws', async () => {
  const { bytes, digest, expected } = await committedDataset();
  const present = JSON.parse(await readFile(join(root, 'tests/fixtures/views/present-response.json'), 'utf8'));
  const retained = (bindings) => bindings.filter((binding) => binding.materialized !== undefined).map((binding) => [binding.name, binding.materialized]);
  assert.deepEqual(retained(present.resolved_bindings), [[PRESENT_DATASET.binding, digest]]);
  assert.deepEqual(retained(present.view.bindings), [[PRESENT_DATASET.binding, digest]]);
  // One flipped byte is another object: the fixture would no longer retain it.
  const corrupted = Buffer.from(bytes);
  corrupted[corrupted.length - 2] ^= 1;
  assert.notEqual(createHash('sha256').update(corrupted).digest('hex'), digest);

  assert.deepEqual(expected, {
    binding: 'metrics',
    digest,
    rows: 5,
    columns: ['category', 'value'],
    cells: [['Ingested', '412'], ['Converted', '397'], ['Indexed', '389'], ['Reviewed', '127'], ['Published', '61']],
  });
  // The chart specification reads the dataset under the binding's name, as Chart.tsx injects it.
  const chart = present.view.charts.metrics_chart;
  assert.equal(chart.data.name, PRESENT_DATASET.binding);
  assert.deepEqual([chart.encoding.x.field, chart.encoding.y.field], expected.columns);
  assert.deepEqual(present.view.spec.elements.chart.props, { binding: 'metrics', chart: 'metrics_chart', title: 'Metrics chart' });
  assert.deepEqual(present.view.spec.elements.table.props, { binding: 'metrics' });

  // Columns come in first-seen order and a missing or null cell is the empty string.
  assert.deepEqual(
    datasetExpectation({ binding: 'b', digest: 'd', rows: [{ a: 1 }, { b: null, a: false }] }),
    { binding: 'b', digest: 'd', rows: 2, columns: ['a', 'b'], cells: [['1', ''], ['false', '']] },
  );
  assert.throws(() => datasetExpectation({ binding: 'b', digest: 'd', rows: [] }), /non-empty JSON array/);
  assert.throws(() => datasetExpectation({ binding: 'b', digest: 'd', rows: { a: 1 } }), /non-empty JSON array/);
});

test('the present dataset counts as exercised only when the chart, the table, the reads and the alerts all agree', async () => {
  const { digest, expected } = await committedDataset();
  const table = (extra) => ({ caption: 'metrics', visible: true, in_details: false, columns: expected.columns, rows: expected.cells, ...extra });
  const read = (offset, has_more) => ({ tool: 'read_object', ok: true, object: digest, offset, has_more });
  const good = {
    text: VIEW_TEXT.render_present,
    alerts: [],
    svgs: 1,
    svg_marks: 5,
    tables: [table(), table({ visible: false, in_details: true })],
    tool_calls: [
      { tool: 'render_present', ok: true },
      { tool: 'show', ok: true },
      { tool: 'show', ok: true },
      read('0', true), read('64', true), read('128', true), read('192', false),
    ],
  };
  const verdict = judgePresentDataset(expected, good);
  assert.deepEqual(verdict.problems, []);
  assert.equal(verdict.ok, true);
  assert.deepEqual(verdict.record, {
    status: 'exercised',
    binding: 'metrics',
    digest,
    rows: 5,
    read_object_calls: 4,
    read_object_offsets: ['0', '64', '128', '192'],
    show_calls: 2,
    chart_svg: true,
    chart_marks: 5,
    table_rows: 5,
    chart_table_rows: 5,
    alerts: [],
    problems: [],
  });

  const without = (tool) => good.tool_calls.filter((call) => call.tool !== tool);
  const fourRows = expected.cells.slice(0, 4);
  const wrongCell = expected.cells.map((row) => row.map((cell) => (cell === 'Published' ? 'Unpublished' : cell)));
  const failures = [
    ['the frame was never observed', null, /App frame was not observed/],
    ['a dataset-unavailable alert', { alerts: [DATASET_UNAVAILABLE[0]] }, /shows alerts: \["Dataset unavailable: metrics"\]/],
    ['a chart-unavailable alert', { alerts: [DATASET_UNAVAILABLE[1]] }, /shows alerts/],
    ['any other alert', { alerts: ['Dataset digest verification failed'] }, /shows alerts/],
    ['no svg', { svgs: 0, svg_marks: 0 }, /no chart svg/],
    ['an svg without marks', { svg_marks: 0 }, /chart svg has no mark element/],
    ['no table', { tables: [table({ visible: false, in_details: true })] }, /no visible data table captioned metrics/],
    ['a table for another binding', { tables: [table({ caption: 'venue' }), table({ visible: false, in_details: true })] }, /no visible data table/],
    ['one row too few', { tables: [table({ rows: fourRows }), table({ visible: false, in_details: true })] }, /data table has 4 body rows, the dataset has 5/],
    ['one row too many', { tables: [table({ rows: [...expected.cells, ['Extra', '1']] }), table({ visible: false, in_details: true })] }, /data table has 6 body rows/],
    ['a missing cell value', { tables: [table({ rows: wrongCell }), table({ visible: false, in_details: true })] }, /does not show the dataset values \["Published"\]/],
    ['rows out of order', { tables: [table({ rows: [...expected.cells].reverse() }), table({ visible: false, in_details: true })] }, /not the dataset rows in order/],
    ['other columns', { tables: [table({ columns: ['value', 'category'] }), table({ visible: false, in_details: true })] }, /data table columns are/],
    ['a value that is in the DOM but not in the visible text', { text: VIEW_TEXT.render_present.replaceAll('Published', '') }, /"Published"\] are not in the visible text/],
    ['a chart without its own table', { tables: [table()] }, /chart has no data table of its own/],
    ['a chart table with the wrong rows', { tables: [table(), table({ visible: false, in_details: true, rows: fourRows })] }, /chart's own data table has 4 body rows/],
    ['read_object never called', { tool_calls: without('read_object') }, /read_object for [0-9a-f]{64} 0 time\(s\)/],
    ['read_object called once', { tool_calls: [...without('read_object'), read('0', false)] }, /1 time\(s\); a ranged read needs at least 2/],
    ['reads of another object', { tool_calls: good.tool_calls.map((call) => (call.tool === 'read_object' ? { ...call, object: '0'.repeat(64) } : call)) }, /0 time\(s\)/],
    ['no block reporting has_more', { tool_calls: [...without('read_object'), read('0', false), read('0', false)] }, /no read_object block reported has_more/],
    ['a last block that still has more', { tool_calls: [...without('read_object'), read('0', true), read('64', true)] }, /last read_object block still reported has_more/],
    ['a read that does not start at 0', { tool_calls: [...without('read_object'), read('64', true), read('128', false)] }, /first read_object block starts at 64/],
    ['show never called', { tool_calls: without('show') }, /did not call show/],
    ['a refused tool call', { tool_calls: [...good.tool_calls, { tool: 'read_object', ok: false }] }, /harness refused tool calls/],
  ];
  for (const [label, change, problem] of failures) {
    const judged = judgePresentDataset(expected, change === null ? null : { ...good, ...change });
    assert.equal(judged.ok, false, `${label}: judged exercised`);
    assert.equal(judged.record.status, 'failed', label);
    assert.ok(judged.problems.some((item) => problem.test(item)), `${label}: ${JSON.stringify(judged.problems)}`);
    assert.deepEqual(judged.record.problems, judged.problems, label);
  }
  // What the record states is what was seen, not what was hoped for.
  assert.equal(judgePresentDataset(expected, { ...good, svg_marks: 0 }).record.chart_svg, false);
  assert.equal(judgePresentDataset(expected, { ...good, tables: [] }).record.table_rows, null);
  assert.equal(judgePresentDataset(expected, { ...good, tool_calls: without('read_object') }).record.read_object_calls, 0);
  assert.equal(judgePresentDataset(expected, null).record.show_calls, 0);
});

test('axe exclusions cover host chrome only; the App frame is judged by every rule', () => {
  const rule = (id, impact, ...targets) => ({ id, impact, nodes: targets.map((target) => ({ target })) });
  const results = {
    passes: [rule('document-title', null, ['iframe', 'iframe', 'html'])],
    incomplete: [],
    violations: [
      rule('color-contrast', 'serious', ['.collapsibleSize'], ['iframe', 'iframe', 'code']),
      rule('frame-title', 'serious', ['iframe'], ['iframe', 'iframe']),
      rule('button-name', 'critical', ['.closeButton']),
      rule('region', 'moderate', ['iframe', 'iframe', 'p']),
    ],
  };
  const axe = partitionAxe(results, 2);
  assert.equal(axe.app_frame_analysed, true);
  assert.deepEqual(axe.app_frame.map((item) => item.id), ['color-contrast']);
  assert.deepEqual(axe.app_frame[0].nodes, [['iframe', 'iframe', 'code']]);
  assert.deepEqual(axe.host_tolerated.map((item) => item.id), UPSTREAM_HOST_RULES);
  assert.deepEqual(axe.host_blocking.map((item) => item.id), ['button-name']);
  // Zero App-frame violations is a claim only when axe actually reached that frame.
  assert.equal(partitionAxe({ passes: [rule('x', null, ['body'])], violations: [], incomplete: [] }, 2).app_frame_analysed, false);
});

test('the run fails when one view fails, when a view is missing, or when the protocol check fails', () => {
  const passedViews = VIEWS.map((view) => ({ tool: view.tool, status: 'passed' }));
  assert.deepEqual(basicHostVerdict(passedViews), { status: 'passed', failed: [] });
  const oneFailed = passedViews.map((item) =>
    item.tool === 'render_timeline' ? { tool: item.tool, status: 'failed', error: 'missing=["Timeline"]' } : item,
  );
  const verdict = basicHostVerdict(oneFailed);
  assert.equal(verdict.status, 'failed');
  assert.deepEqual(verdict.failed, [{ tool: 'render_timeline', error: 'missing=["Timeline"]' }]);
  assert.equal(basicHostVerdict(passedViews.slice(0, 3)).failed[0].tool, 'render_present');

  const protocol = { status: 'passed' };
  assert.deepEqual(runProblems({ protocol, basicHost: { ...basicHostVerdict(passedViews) }, protocolOnly: false }), []);
  assert.match(runProblems({ protocol, basicHost: verdict, protocolOnly: false })[0], /basic_host failed: render_timeline/);
  assert.match(
    runProblems({ protocol, basicHost: { status: 'failed', error: 'chromium missing' }, protocolOnly: false })[0],
    /basic_host failed: chromium missing/,
  );
  assert.deepEqual(runProblems({ protocol, basicHost: { status: 'not_run' }, protocolOnly: true }), []);
  assert.match(runProblems({ protocol: { status: 'failed', error: 'x' }, basicHost: verdict, protocolOnly: true })[0], /protocol_check failed: x/);
});

test('the MCP Apps receipt states what ran: stdio not_run, HTTP as the harness reported it, and no literal posing as an observation', async () => {
  const url = 'http://127.0.0.1:18765/mcp';
  assert.deepEqual(transportRecord({ requested: url, reported: url }), {
    stdio: { status: 'not_run', reason: 'this orchestrator drives the harness over Streamable HTTP only' },
    http: { status: 'listening', endpoint: url, requested: url },
  });
  for (const reported of [null, undefined, '']) {
    const record = transportRecord({ requested: url, reported });
    assert.deepEqual(record.http, { status: 'not_listening', endpoint: null, requested: url });
    assert.equal(record.stdio.status, 'not_run');
  }

  const source = await readFile(join(root, 'qualification/mcp-apps/run.mjs'), 'utf8');
  // Values the receipt once stated without having seen them, and instructions it carried.
  assert.doesNotMatch(source, /stdio: 'default'|cspObject: true|has_structured_content: true|build_command|serve_command|how_to_http_ngrok/);
  assert.match(source, /transport: transportRecord\(\{ requested: MCP_URL, reported: listeningOn \}\)/);
  assert.match(source, /listeningOn = listening\[1\]/);
  assert.match(source, /static_bundle_smoke: \{\s*status: 'not_run'/);
  assert.match(source, /host_package: ensured\.package/);

  // The harness's check report reads resource metadata and the tool list back from its handlers.
  const harness = await readFile(join(root, 'qualification/mcp-apps/src/main.rs'), 'utf8');
  const report = harness.slice(harness.indexOf('fn check_report'), harness.indexOf('impl ServerHandler'));
  assert.match(report, /read_catalog\(&app\.uri\)/);
  assert.match(report, /self\.tool_definitions\(\)/);
  assert.match(report, /self\s*\.call_render_tool\(name\)/);
  assert.doesNotMatch(report, /prefersBorder|connectDomains|SHOW_TOOL|READ_OBJECT_TOOL|tool\.fixture/);
});

test('the receipt says which React build the rendered App bundle carries', async () => {
  const production = '<!doctype html><html><body><div id="root"></div><script>var a=1</script></body></html>';
  const development = production.replace('var a=1', `console.info("${REACT_DEVELOPMENT_MARKER} for a better development experience")`);
  assert.deepEqual(appBundleBuild({ html: production, nodeEnv: undefined }), {
    node_env: null,
    react_development_build: false,
    marker: 'Download the React DevTools',
  });
  assert.deepEqual(appBundleBuild({ html: development, nodeEnv: 'development' }), {
    node_env: 'development',
    react_development_build: true,
    marker: 'Download the React DevTools',
  });
  // The bundle decides, not the environment variable.
  assert.equal(appBundleBuild({ html: development, nodeEnv: 'production' }).react_development_build, true);
  assert.equal(appBundleBuild({ html: production, nodeEnv: 'development' }).react_development_build, false);
  const source = await readFile(join(root, 'qualification/mcp-apps/run.mjs'), 'utf8');
  assert.match(source, /app_bundle: appBundle,/);
  assert.match(source, /appBundleBuild\(\{\s*html: await readFile\(join\(distApps,/);
});

test('a receipt never records an open tunnel', () => {
  const base = { public_url: 'https://x.ngrok.app/mcp', local_port: 18765, note: 'n' };
  assert.deepEqual(ngrokRecord({ ...base, enabled: false, opened_at: 't1', closed_at: 't2' }), {
    status: 'not_run', opened_at: null, closed_at: null, public_url: null, local_port: 18765, note: 'n',
  });
  assert.deepEqual(ngrokRecord({ ...base, enabled: true, opened_at: 't1', closed_at: 't2' }), {
    status: 'closed', opened_at: 't1', closed_at: 't2', public_url: 'https://x.ngrok.app/mcp', local_port: 18765, note: 'n',
  });
  assert.equal(ngrokRecord({ ...base, enabled: true, opened_at: null, closed_at: null }).status, 'not_opened');
  assert.throws(() => ngrokRecord({ ...base, enabled: true, opened_at: 't1', closed_at: null }), /opened but not closed/);
});

test('the MCP Apps orchestrator renders every view, fails on any failure and never tolerates App-frame rules', async () => {
  const source = await readFile(join(root, 'qualification/mcp-apps/run.mjs'), 'utf8');
  assert.match(source, /for \(const view of VIEWS\)/);
  assert.match(source, /runProblems\(\{ protocol, basicHost, protocolOnly: PROTOCOL_ONLY \}\)/);
  assert.match(source, /waitForListening\(harness,/);
  assert.match(source, /partitionAxe\(/);
  assert.match(source, /ngrokRecord\(/);
  assert.match(source, /receiptHeader\(root, MCP_APPS_INPUTS\)/);
  assert.match(source, /recordReceipt\(root, 'mcp-apps', receipt\)/);
  assert.doesNotMatch(source, /disableRules\(|session_open|spawnDetached|waitForTcp|requireCleanTree|commit_sha/);
  assert.doesNotMatch(source, /'cargo',\s*\[\s*'run'/);
  // present_dataset is the judged record of what the App frame showed, never a sentence or a literal status.
  assert.match(source, /judgePresentDataset\(\s*expected,/);
  assert.match(source, /toolCallsFrom\(toolLog\(\)\.slice\(logStart\)\)/);
  assert.match(source, /present_dataset: dataset\.record/);
  assert.doesNotMatch(source, /not_exercised|status: 'exercised'|unavailable/);
});

/** Repo-relative files reachable from `entry` through relative or `@/` imports and `new URL(..., import.meta.url)`. */
async function repoImportGraph(entry) {
  const seen = new Set();
  const queue = [entry];
  const candidates = (path) => ['', '.ts', '.tsx', '.mjs', '/index.ts', '/index.tsx'].map((suffix) => `${path}${suffix}`);
  while (queue.length) {
    const file = queue.pop();
    if (seen.has(file)) continue;
    seen.add(file);
    if (!/\.(mjs|ts|tsx)$/.test(file)) continue;
    const text = await readFile(join(root, file), 'utf8');
    const specifiers = [
      ...text.matchAll(/\bfrom\s+'([^']+)'/g),
      ...text.matchAll(/\bimport\s+'([^']+)'/g),
      ...text.matchAll(/new URL\('([^']+)',\s*import\.meta\.url\)/g),
    ].map((match) => match[1]);
    for (const specifier of specifiers) {
      const base = specifier.startsWith('@/')
        ? `ui/src/${specifier.slice(2)}`
        : specifier.startsWith('.')
          ? join(file, '..', specifier).replaceAll('\\', '/')
          : null;
      if (base === null) continue; // a package, pinned by bun.lock
      for (const candidate of candidates(base)) {
        const isFile = await stat(join(root, candidate)).then((entryStat) => entryStat.isFile(), () => false);
        if (isFile) {
          queue.push(candidate);
          break;
        }
      }
    }
  }
  return [...seen].sort();
}

test('the MCP Apps receipt inputs cover everything the run renders and executes', async () => {
  assert.deepEqual(MCP_APPS_INPUTS, [...MCP_APPS_INPUTS].sort(), 'inputs are sorted');
  assert.equal(new Set(MCP_APPS_INPUTS).size, MCP_APPS_INPUTS.length, 'inputs are unique');
  for (const input of MCP_APPS_INPUTS) {
    assert.doesNotMatch(input, /\\|^\/|^[A-Za-z]:|(^|\/)\.\.?(\/|$)|\/$/, `${input} is not a plain repo-relative path`);
    await stat(join(root, input)); // receiptHeader refuses an input the commit does not contain
  }
  const covered = (path) => MCP_APPS_INPUTS.some((input) => path === input || path.startsWith(`${input}/`));

  // The App bundle: its import graph from the entry, the bundler script and what that reads.
  const app = await repoImportGraph('ui/src/mcp-apps/main.tsx');
  for (const module of ['ui/src/features/views/PresentView.tsx', 'ui/src/features/views/Chart.tsx', 'ui/src/features/views/DataTable.tsx', 'ui/src/lib/wire.ts', 'ui/src/api/generated/zod.gen.ts', 'ui/src/styles.css']) {
    assert.ok(app.includes(module), `the import walk did not reach ${module}: ${app.join(', ')}`);
  }
  const bundler = await repoImportGraph('ui/scripts/bundle-app.mjs');
  for (const file of ['api/mcp-apps.json', 'ui/scripts/app-declaration.ts', 'ui/src/mcp-apps/main.tsx']) {
    assert.ok(bundler.includes(file), `the bundler walk did not reach ${file}: ${bundler.join(', ')}`);
  }
  // The orchestrator itself and the repository modules it imports.
  const orchestrator = await repoImportGraph('qualification/mcp-apps/run.mjs');
  for (const file of ['qualification/mcp-apps/lib/views.mjs', 'qualification/lib/cargo.mjs', 'scripts/lib/provenance.mjs']) {
    assert.ok(orchestrator.includes(file), `the orchestrator walk did not reach ${file}`);
  }
  const uncovered = [...app, ...bundler, ...orchestrator].filter((file) => !covered(file));
  assert.deepEqual(uncovered, [], 'files the run executes or renders that are not receipt inputs');

  // What no import statement names: fixtures, lockfiles, manifests, toolchains, and the rest
  // of ui/ (Tailwind turns words in any tracked ui file into rules of the App's stylesheet).
  for (const path of [
    'tests/fixtures/views/present-response.json',
    'tests/fixtures/views/present-metrics-dataset.json',
    'qualification/mcp-apps/src/main.rs',
    'qualification/mcp-apps/Cargo.toml',
    'Cargo.toml',
    'Cargo.lock',
    'rust-toolchain.toml',
    'bun.lock',
    'package.json',
    '.bun-version',
    'ui/package.json',
    'ui/vite.config.ts',
    'ui/tsconfig.json',
    'ui/scripts/bundle-docs.mjs',
    'ui/tests/e2e/mcp-apps-static-bundle-smoke.spec.ts',
  ]) {
    await stat(join(root, path));
    assert.ok(covered(path), `${path} is not covered by the receipt inputs`);
  }

  const source = await readFile(join(root, 'qualification/mcp-apps/run.mjs'), 'utf8');
  assert.doesNotMatch(source, /const MCP_APPS_INPUTS/, 'the orchestrator must use the tested list');
});

test('the MCP Apps harness serves the committed fixtures and bundle whatever environment the caller inherited', async () => {
  const source = await readFile(join(root, 'qualification/mcp-apps/run.mjs'), 'utf8');
  // tests/fixtures/views is a receipt input; an inherited OKF_MCP_APPS_FIXTURES must not swap it.
  assert.ok(MCP_APPS_INPUTS.includes('tests/fixtures/views'));
  assert.ok(PRESENT_DATASET.fixture.startsWith('tests/fixtures/views/'));
  assert.match(source, /const fixturesDir = join\(root, 'tests\/fixtures\/views'\);/);
  assert.match(source, /const harnessEnv = \{ \.\.\.process\.env, OKF_MCP_APPS_DIST: distApps, OKF_MCP_APPS_FIXTURES: fixturesDir \};/);
  assert.match(source, /run\(harnessBin, \['--check'\], \{\s*env: harnessEnv,/);
  assert.match(source, /spawnGroup\(harnessBin, harnessArgs, \{\s*env: \{\s*\.\.\.harnessEnv,/);
  assert.equal(source.match(/OKF_MCP_APPS_(DIST|FIXTURES):/g).length, 2, 'no second place builds the harness environment');
  // And what the harness says it serves is compared with the committed dataset before anything renders.
  assert.match(source, /check\.dataset\?\.sha256 !== committedDataset\.digest/);
});

test('both orchestrators take their header from receiptHeader and record only through recordReceipt', async () => {
  for (const name of ['docling', 'mcp-apps']) {
    const source = await readFile(join(root, 'qualification', name, 'run.mjs'), 'utf8');
    assert.match(source, /= await receiptHeader\(root, /, name);
    assert.match(source, new RegExp(`recordReceipt\\(root, '${name}', receipt\\)`), name);
    assert.match(source, /process\.argv\.includes\('--record'\)/, name);
    assert.doesNotMatch(source, /requireCleanTree|commit_sha|writeFile\([^)]*qualification\/receipts/, name);
  }
});

test('no time-budgeted test uses a budget a loaded machine can miss', async () => {
  const source = await readFile(fileURLToPath(import.meta.url), 'utf8');
  assert.equal(SLOW_MACHINE_BUDGET_MS, 20_000);
  assert.doesNotMatch(source, new RegExp(String.raw`\b${2 * 2000}\b`), 'a 4 s budget failed once under a parallel cargo build');
});

test('record.mjs refuses a protocol-only MCP Apps receipt and any whose basic-host section did not run', async () => {
  const { recordRefusal } = await import('../../qualification/record.mjs');
  const ran = { protocol_only: false, basic_host: { status: 'passed', failed: [] } };
  assert.equal(recordRefusal('mcp-apps', ran), null);
  assert.match(recordRefusal('mcp-apps', { ...ran, protocol_only: true }), /protocol-only.*not the gate receipt/);
  assert.match(recordRefusal('mcp-apps', { ...ran, basic_host: { status: 'not_run', reason: 'skipped: protocol-only' } }), /basic-host section was skipped/);
  assert.match(recordRefusal('mcp-apps', { protocol_only: false }), /basic-host section was skipped/);
  assert.equal(recordRefusal('docling', { protocol_only: true }), null, 'only the MCP Apps receipt has a basic-host section');
  const source = await readFile(join(root, 'qualification/record.mjs'), 'utf8');
  assert.ok(source.indexOf('recordRefusal(name, receipt)') < source.indexOf('await recordReceipt('), 'the refusal must come before the receipt is recorded');
});
