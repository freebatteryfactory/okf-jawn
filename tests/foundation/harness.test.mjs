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
import { inventoryCheck, parseManifest, unverifiedEnvPaths, verifyAssets } from '../../qualification/docling/lib/assets.mjs';
import { buildFacts, classifyRefusal, declaredDoclingFeatures, resolvedFeatures } from '../../qualification/docling/lib/build.mjs';
import {
  bboxProblem,
  bodyFacts,
  converterOptions,
  describeDocument,
  judgePageRenders,
  judgeProvenance,
  pngSize,
} from '../../qualification/docling/lib/document.mjs';
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

const VERIFIED_MODEL = { file: 'layout.onnx', path: 'C:\\models\\layout.onnx', bytes: 10, sha256: 'd'.repeat(64) };

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
  assert.equal(verified.all_match, true);
  assert.equal(verified.hashed_at_run_time, true);
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

const DOCLING_ASSETS = { count: 1, all_match: true, manifest_sha256: 'e'.repeat(64), hashed_at_run_time: true, files: [VERIFIED_MODEL] };

const STUB_SOURCE = (paginated) => ({
  role: 'stub',
  expect: { confirmed_by: 'test stub', markdown_contains: ['Hello world'], tables: 1, ...(paginated ? { pages: 1 } : {}) },
});

const isPaginated = (only) => /\.(pdf|png)$/.test(only);

const DOCLING_SOURCES = {
  files: Object.fromEntries(
    FIXTURE_RUNS.filter((only) => only !== TIMEOUT_PROBE && only !== MUST_FAIL).map((only) => [only, STUB_SOURCE(isPaginated(only))]),
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
    document: supported
      ? { markdown_file: 'document.md', json_file: 'document.json', page_images: paginated ? [PAGE_IMAGE] : [] }
      : null,
    ...over.fixture,
  };
  const evidence = {
    markdown: '## Title\n\nHello   world\n',
    document: stubDocument(paginated ? {} : { pages: {}, texts: stubDocument().texts.map((item) => ({ ...item, prov: [] })), tables: stubDocument().tables.map((item) => ({ ...item, prov: [] })) }),
    pageFiles: { 'page-1.png': pageFile(PAGE_PNG) },
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
    evidence: supported ? evidence : undefined,
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

test('a Docling receipt carries the shared header at its top level and passes when every rule holds', () => {
  const receipt = doclingReceipt();
  assert.deepEqual(receiptHeaderProblems(receipt), []);
  assert.deepEqual(Object.keys(receipt).slice(0, 3), ['git_sha', 'inputs', 'produced_at']);
  assert.equal(receipt.orchestrator, undefined);
  assert.deepEqual(receipt.failures, []);
  assert.equal(receipt.result, 'PASS');
  assert.equal(receipt.receipts.length, FIXTURE_RUNS.length - 1);
  assert.equal(receipt.timeout_case.outcome, 'PASS');
  assert.deepEqual(Object.keys(receipt.summary), FIXTURE_RUNS);
  assert.deepEqual(Object.keys(receipt.criteria_summary), ['conversion', 'content', 'page_renders', 'provenance', 'memory_measured', 'refusal']);
  const pdf = entryOf(receipt, 'born_digital_text.pdf');
  assert.deepEqual(Object.keys(pdf.criteria), ['conversion', 'content', 'page_renders', 'provenance', 'memory_measured']);
  assert.equal(pdf.page_image_count, 1);
  assert.deepEqual(pdf.page_provenance, [{ page_no: 1, text_items: 2, tables: 1, pictures: 0, has_image: true }]);
  assert.equal(pdf.role, 'stub');
});

test('memory is a measurement with no limit applied, and an unmeasured fixture fails memory_measured', () => {
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
  assert.equal(entry.criteria.memory_measured.status, 'FAIL');
  assert.equal(entry.conversion_outcome, 'PASS');
  assert.equal(entry.outcome, 'FAIL_memory_measured');
  assert.equal(receipt.criteria_summary.memory_measured['sample_sheet.xlsx'], 'FAIL');
  assert.equal(receipt.result, 'FAIL');
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
  assert.equal(receipt.summary[MUST_FAIL], 'FAIL_expected_failure');
  assert.equal(entryOf(receipt, MUST_FAIL).refusal, undefined, 'an accepted input has no refusal to classify');
  assert.equal(receipt.result, 'FAIL');
});

test('a refusal counts only when another PDF converted in the same run', () => {
  const pdfs = FIXTURE_RUNS.filter((only) => only.endsWith('.pdf') && only !== MUST_FAIL);
  const overrides = Object.fromEntries(
    pdfs.map((only) => [only, { fixture: { outcome: 'FAIL_converter_error', stage: 'converter_error', status: null } }]),
  );
  const receipt = doclingReceipt(overrides);
  assert.equal(receipt.summary[MUST_FAIL], 'FAIL_refusal_unproven');
  assert.match(entryOf(receipt, MUST_FAIL).refusal_note, /no other PDF fixture converted/);
  assert.equal(receipt.criteria_summary.refusal[MUST_FAIL], 'FAIL');
  assert.equal(receipt.result, 'FAIL');

  const counted = entryOf(doclingReceipt(), MUST_FAIL);
  assert.equal(counted.criteria.refusal.status, 'PASS');
  assert.deepEqual(counted.refusal.other_pdfs_converted, pdfs);
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
  assert.equal(contradicted.summary[MUST_FAIL], 'FAIL_refusal');
  assert.equal(contradicted.result, 'FAIL');
});

test('a fixture process that wrote no receipt, or never ran, is a harness FAIL', () => {
  const crashed = doclingReceipt({
    'table_heavy.pdf': { report: null, run: { exitCode: 1, stderr: 'okf-qualify-docling: missing fixture' } },
  });
  const entry = entryOf(crashed, 'table_heavy.pdf');
  assert.equal(entry.outcome, 'FAIL_harness');
  assert.equal(entry.harness_exit_code, 1);
  assert.match(entry.harness_error, /missing fixture/);
  assert.equal(crashed.result, 'FAIL');

  const partial = doclingReceipt({}, { runs: [doclingRun('sample_sheet.xlsx')], scope: { mode: 'only', fixtures: ['sample_sheet.xlsx'], qualification: false } });
  assert.equal(partial.summary['sample_sheet.xlsx'], 'PASS');
  assert.equal(partial.summary[TIMEOUT_PROBE], 'FAIL_not_run');
  assert.equal(partial.result, 'FAIL', 'a single-fixture run is never the qualification');
});

test('each content, page-render and provenance failure fails its fixture and the receipt, with the evidence named', () => {
  {
    const receipt = doclingReceipt({ 'corpus/word_sample.docx': { evidence: { markdown: '## Title\n\nGoodbye\n' } } });
    const entry = entryOf(receipt, 'corpus/word_sample.docx');
    assert.equal(entry.fixture, 'corpus/word_sample.docx');
    assert.equal(entry.outcome, 'FAIL_content');
    assert.equal(entry.conversion_outcome, 'PASS');
    assert.deepEqual(entry.failed_criteria, ['content']);
    assert.equal(receipt.result, 'FAIL');
    const failure = receipt.failures.find((item) => item.criterion === 'content');
    assert.equal(failure.fixture, 'corpus/word_sample.docx');
    assert.deepEqual(failure.detail.map((check) => check.expected), ['Hello world']);
  }
  {
    const receipt = doclingReceipt({ 'scanned_text.pdf': { fixture: { document: { markdown_file: 'document.md', json_file: 'document.json', page_images: [] } } } });
    const entry = entryOf(receipt, 'scanned_text.pdf');
    assert.equal(entry.criteria.page_renders.status, 'unavailable');
    assert.equal(entry.outcome, 'FAIL_page_renders');
    assert.equal(entry.page_image_count, 0);
    assert.equal(entry.page_provenance[0].has_image, false);
    assert.equal(receipt.result, 'FAIL');
    assert.equal(entryOf(receipt, 'sample_sheet.xlsx').criteria.page_renders.status, 'not_applicable');
  }
  {
    const receipt = doclingReceipt({
      'born_digital_text.pdf': { evidence: { document: stubDocument({ texts: [{ self_ref: '#/texts/0', label: 'text', text: 'x', prov: [] }] }) } },
    });
    const entry = entryOf(receipt, 'born_digital_text.pdf');
    assert.equal(entry.criteria.provenance.status, 'FAIL');
    assert.deepEqual(entry.failed_criteria, ['provenance']);
    assert.equal(entry.outcome, 'FAIL_provenance');
    assert.deepEqual(receipt.failures, [
      { fixture: 'born_digital_text.pdf', criterion: 'provenance', status: 'FAIL', detail: [{ ref: '#/texts/0', label: 'text', problem: 'no provenance' }] },
    ]);
    assert.equal(receipt.criteria_summary.provenance['born_digital_text.pdf'], 'FAIL');
    assert.equal(receipt.criteria_summary.provenance['sample_sheet.xlsx'], 'recorded_not_judged');
    assert.equal(receipt.result, 'FAIL');
  }
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
  assert.equal(receipt.assets_verified.all_match, true);
});

test('the receipt fails when a model the library resolves is not a verified file', () => {
  const receipt = doclingReceipt({}, { assets: { ...DOCLING_ASSETS, files: [] } });
  assert.equal(receipt.model_inventory.status, 'FAIL');
  assert.deepEqual(Object.values(receipt.summary).filter((outcome) => !outcome.startsWith('PASS')), []);
  assert.equal(receipt.result, 'FAIL');
  assert.equal(receipt.failures[0].criterion, 'model_inventory');
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
