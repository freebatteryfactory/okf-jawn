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
import { OCR_FIXTURES, decodeFixture, fixtureWords, renderLines } from '../../qualification/docling/lib/ocr-fixture.mjs';
import { killProcessTree, spawnGroup, waitForListening } from '../../qualification/mcp-apps/lib/process.mjs';
import { BASIC_HOST, PATCHED_SERVE, SOURCE_RECORD, lsRemoteArgs, patchServe, rawUrl, sourceProblems, sourceRecord, tagCommit } from '../../qualification/mcp-apps/lib/basic-host.mjs';
import {
  EXIT_CODES,
  GATE as MCP_APPS_GATE,
  SCOPE,
  criterionIds,
  criterionRules,
  exitCodeFor,
  requiredCriterionIds,
  sealEnvelope,
  sectionStatus,
  viewCriteria,
} from '../../qualification/mcp-apps/lib/criteria.mjs';
import { DOM_SELECTORS, HarnessError, appFrameOf, observeView, readAppDom, viewSettled } from '../../qualification/mcp-apps/lib/observe.mjs';
import { qualify } from '../../qualification/mcp-apps/lib/qualify.mjs';
import { envelopeFailures as mcpAppsEnvelopeFailures, foldCriteria as mcpAppsFold } from '../../scripts/lib/receipt-envelope.mjs';
import { PROTOCOL_CRITERIA, PROTOCOL_RULES, canonicalJson, judgeProtocol, observeProtocol } from '../../qualification/mcp-apps/lib/protocol.mjs';
import {
  APP_ONLY_TOOLS,
  APP_RESOURCE_URI,
  MCP_APPS_INPUTS,
  PRESENT_DATASET,
  REACT_DEVELOPMENT_MARKER,
  TOOL_CALL_LOG_PREFIX,
  UPSTREAM_HOST_RULES,
  VIEWS,
  appBundleBuild,
  basicHostUrl,
  chartForBinding,
  datasetExpectation,
  expectedChartMarks,
  judgePresentDataset,
  judgeView,
  ngrokRecord,
  partitionAxe,
  readResolutions,
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

/** The first 24 bytes of a PNG of this size: all pngSize reads. */
function pngHead(width, height) {
  const head = Buffer.alloc(24);
  head.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
  head.writeUInt32BE(width, 16);
  head.writeUInt32BE(height, 20);
  return head;
}

const PAGE_PNG = pngHead(1224, 1584);

const PAGE_PNG_SHA = createHash('sha256').update(PAGE_PNG).digest('hex');

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
  const image = { ...PAGE_IMAGE, file: { sha256: PAGE_PNG_SHA, bytes: PAGE_PNG.length, png: pngSize(PAGE_PNG) } };
  const judge = (over = {}) =>
    judgePageRenders({ applicable: true, expectedPages: 1, libraryPageCount: { value: 1, error: null }, pages, images: [image], ...over });

  const good = judge();
  assert.equal(good.status, 'PASS');
  assert.deepEqual(good.renders, [
    { page_no: 1, width_px: 1224, height_px: 1584, mimetype: 'image/png', dpi: 144, bytes: 24, sha256: PAGE_PNG_SHA, px_per_page_unit: 2 },
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

test('provenance: every text item of a paginated fixture needs a page of the document and a box inside it', () => {
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

  assert.equal(judge({ texts: [] }).status, 'not_exercised');
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
    pageFiles: { 'page-1.png': { sha256: PAGE_PNG_SHA, bytes: PAGE_PNG.length, png: pngSize(PAGE_PNG) } },
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
  const present = JSON.parse(await readFile(join(root, PRESENT_DATASET.present), 'utf8'));
  const chart = chartForBinding(present, PRESENT_DATASET.binding);
  const rows = JSON.parse(bytes.toString('utf8'));
  return { bytes, digest, present, chart, rows, expected: datasetExpectation({ binding: PRESENT_DATASET.binding, digest, rows, bytes: bytes.length, chart }) };
}

test('the present fixture retains exactly the committed dataset, and the expectation is the table DataTable draws', async () => {
  const { bytes, digest, present, chart, expected } = await committedDataset();
  assert.equal(PRESENT_DATASET.present, 'tests/fixtures/views/present-response.json');
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
    bytes: 223,
    rows: 5,
    columns: ['category', 'value'],
    cells: [['Ingested', '412'], ['Converted', '397'], ['Indexed', '389'], ['Reviewed', '127'], ['Published', '61']],
    chart_marks: 5,
  });
  assert.equal(expected.bytes, bytes.length);
  // The chart specification reads the dataset under the binding's name, as Chart.tsx injects it.
  assert.equal(chart, present.view.charts.metrics_chart);
  assert.equal(chart.data.name, PRESENT_DATASET.binding);
  assert.deepEqual([chart.encoding.x.field, chart.encoding.y.field], expected.columns);
  assert.deepEqual(present.view.spec.elements.chart.props, { binding: 'metrics', chart: 'metrics_chart', title: 'Metrics chart' });
  assert.deepEqual(present.view.spec.elements.table.props, { binding: 'metrics' });
  assert.throws(() => chartForBinding(present, 'venue'), /exactly one Chart element bound to venue; found 0/);
  assert.throws(() => chartForBinding({ view: { spec: { elements: { c: { type: 'Chart', props: { binding: 'b', chart: 'gone' } } } }, charts: {} } }, 'b'), /no chart named gone/);

  // Columns come in first-seen order and a missing or null cell is the empty string.
  const bar = { mark: 'bar', encoding: { x: { field: 'a', type: 'nominal' }, y: { field: 'n', type: 'quantitative' } } };
  assert.deepEqual(
    datasetExpectation({ binding: 'b', digest: 'd', bytes: 9, chart: bar, rows: [{ a: 1, n: 1 }, { b: null, a: false, n: 2 }] }),
    { binding: 'b', digest: 'd', bytes: 9, rows: 2, columns: ['a', 'n', 'b'], cells: [['1', '1', ''], ['false', '2', '']], chart_marks: 2 },
  );
  assert.throws(() => datasetExpectation({ binding: 'b', digest: 'd', bytes: 9, chart: bar, rows: [] }), /non-empty JSON array/);
  assert.throws(() => datasetExpectation({ binding: 'b', digest: 'd', bytes: 9, chart: bar, rows: { a: 1 } }), /non-empty JSON array/);
  assert.throws(() => datasetExpectation({ binding: 'b', digest: 'd', chart: bar, rows: [{ a: 1, n: 1 }] }), /byte length must be a positive integer/);
});

test('the chart draws one mark per dataset row only for the shape the fixture has, and any other shape is refused', async () => {
  const { chart, rows } = await committedDataset();
  // The fixture: one unit bar, a nominal field against a quantitative one, nothing that merges or drops rows.
  assert.equal(chart.mark, 'bar');
  assert.deepEqual(chart.encoding, { x: { field: 'category', type: 'nominal' }, y: { field: 'value', type: 'quantitative' } });
  assert.equal(expectedChartMarks(chart, rows), rows.length);
  assert.equal(expectedChartMarks(chart, rows.slice(0, 1)), 1);
  const refused = [
    [{ ...chart, mark: 'line' }, /its mark is "line", not "bar"/],
    [{ ...chart, mark: { type: 'bar' } }, /not "bar"/],
    [{ ...chart, transform: [{ filter: 'datum.value > 100' }] }, /it has transform/],
    [{ ...chart, layer: [] }, /it has layer/],
    [{ ...chart, encoding: { ...chart.encoding, y: { field: 'value', type: 'quantitative', aggregate: 'sum' } } }, /channel y is .*aggregate.*not a plain \{ field, type \}/],
    [{ ...chart, encoding: { ...chart.encoding, x: { field: 'category', type: 'nominal', bin: true } } }, /channel x/],
    [{ ...chart, encoding: { ...chart.encoding, color: { field: 'category', type: 'nominal' } } }, /not exactly x and y/],
    [{ ...chart, encoding: { x: chart.encoding.x, y: { field: 'value', type: 'nominal' } } }, /not one category and one quantity/],
    [null, /not an object/],
  ];
  for (const [spec, why] of refused) assert.throws(() => expectedChartMarks(spec, rows), why, JSON.stringify(spec));
  // Vega-Lite drops a row whose quantity is not a number: then marks would be fewer than rows.
  assert.throws(() => expectedChartMarks(chart, [...rows, { category: 'Void', value: null }]), /row 5 has no finite value/);
  assert.throws(() => expectedChartMarks(chart, [{ value: 1 }]), /row 0 has no category/);
});

test('reads of one binding are contiguous from offset 0 to the size of the dataset, each resolution on its own', () => {
  const read = (offset, bytes, has_more) => ({ tool: 'read_object', ok: true, object: 'ab', offset, bytes, total_size: '223', has_more });
  const whole = [read('0', 64, true), read('64', 64, true), read('128', 64, true), read('192', 31, false)];
  assert.deepEqual(readResolutions(whole, 223), { complete: 1, problems: [] });
  // A development build of React resolves the binding twice; the blocks interleave.
  const doubled = whole.flatMap((block) => [block, block]);
  assert.deepEqual(readResolutions(doubled, 223), { complete: 2, problems: [] });
  assert.deepEqual(readResolutions([...whole, ...whole], 223), { complete: 2, problems: [] });
  assert.deepEqual(readResolutions([], 223), { complete: 0, problems: [] });

  const broken = [
    ['the middle skipped', [read('0', 64, true), read('192', 31, false)], /block starts at 192, where no read from offset 0 had arrived/],
    ['an overlap', [read('0', 64, true), read('32', 64, true), read('96', 127, false)], /block starts at 32/],
    ['a read that does not start at 0', [read('64', 64, true), read('128', 95, false)], /block starts at 64/],
    ['a read that stops early', [read('0', 64, true), read('64', 64, false)], /a read ended at byte 128 of 223/],
    ['a read that runs past the end', [read('0', 64, true), read('64', 200, false)], /a read ended at byte 264 of 223/],
    ['a read that never finishes', [read('0', 64, true), read('64', 64, true)], /stopped at byte 128 of 223 without a final block/],
    ['the dataset in one block', [read('0', 223, false)], /in one block; the ranged loop was not exercised/],
    ['a block without progress', [read('0', 0, true), read('0', 64, true)], /made no progress/],
    ['another total size', [read('0', 64, true), { ...read('64', 159, false), total_size: '999' }], /reported total_size "999", the dataset has 223 bytes/],
    ['a report without a block length', [{ ...read('0', 64, true), bytes: undefined }], /carries no usable range/],
    ['a report without a decimal offset', [read('0x0', 64, true)], /carries no usable range/],
  ];
  for (const [label, reads, problem] of broken) {
    const judged = readResolutions(reads, 223);
    assert.ok(judged.problems.some((item) => problem.test(item)), `${label}: ${JSON.stringify(judged)}`);
    assert.equal(judged.complete, 0, label);
  }
});

test('the present dataset counts as exercised only when the chart, the table, the reads and the alerts all agree', async () => {
  const { digest, expected } = await committedDataset();
  const table = (extra) => ({ caption: 'metrics', visible: true, in_details: false, columns: expected.columns, rows: expected.cells, ...extra });
  const chartTable = (extra) => table({ visible: false, in_details: true, ...extra });
  const read = (offset, has_more, bytes = 64) => ({ tool: 'read_object', ok: true, object: digest, offset, bytes, total_size: '223', has_more });
  const good = {
    text: VIEW_TEXT.render_present,
    alerts: [],
    svgs: 1,
    svg_marks: 5,
    tables: [table(), chartTable()],
    tool_calls: [
      { tool: 'render_present', ok: true },
      { tool: 'show', ok: true },
      { tool: 'show', ok: true },
      read('0', true), read('64', true), read('128', true), read('192', false, 31),
    ],
  };
  const verdict = judgePresentDataset(expected, good);
  assert.deepEqual(verdict.problems, []);
  assert.equal(verdict.ok, true);
  assert.deepEqual(verdict.checks, {
    observed: [], no_alert: [], chart_marks: [], table_rows: [], chart_source_table: [], show_calls: [], read_object_calls: [], no_refused_call: [],
  });
  assert.deepEqual(verdict.record, {
    status: 'exercised',
    expected_binding: 'metrics',
    expected_digest: digest,
    expected_bytes: 223,
    expected_rows: 5,
    expected_chart_marks: 5,
    table_captions: ['metrics', 'metrics'],
    read_object_digests: [digest],
    read_object_calls: 4,
    read_object_offsets: ['0', '64', '128', '192'],
    read_object_bytes: [64, 64, 64, 31],
    read_object_total_sizes: ['223'],
    read_object_resolutions: 1,
    show_calls: 2,
    chart_svgs: 1,
    chart_marks: 5,
    table_rows: 5,
    chart_table_rows: 5,
    alerts: [],
    problems: [],
  });
  // Nothing in the record is the expectation unless its name says so.
  for (const [field, value] of Object.entries(verdict.record)) {
    if (!field.startsWith('expected_')) continue;
    assert.equal(value, expected[field.slice('expected_'.length)], field);
  }
  assert.deepEqual(Object.keys(verdict.record).filter((field) => ['binding', 'digest', 'rows', 'bytes'].includes(field)), []);

  const without = (tool) => good.tool_calls.filter((call) => call.tool !== tool);
  const fourRows = expected.cells.slice(0, 4);
  const wrongCell = expected.cells.map((row) => row.map((cell) => (cell === 'Published' ? 'Unpublished' : cell)));
  const foreignRows = [['a', '1'], ['b', '2'], ['c', '3'], ['d', '4'], ['e', '5']];
  const failures = [
    ['the frame was never observed', null, 'observed', /App frame was not observed/],
    ['a dataset-unavailable alert', { alerts: [DATASET_UNAVAILABLE[0]] }, 'no_alert', /shows alerts: \["Dataset unavailable: metrics"\]/],
    ['a chart-unavailable alert', { alerts: [DATASET_UNAVAILABLE[1]] }, 'no_alert', /shows alerts/],
    ['any other alert', { alerts: ['Dataset digest verification failed'] }, 'no_alert', /shows alerts/],
    ['no svg', { svgs: 0, svg_marks: 0 }, 'chart_marks', /no chart svg/],
    ['an svg without marks', { svg_marks: 0 }, 'chart_marks', /chart svg has 0 mark element\(s\); the dataset's 5 rows draw 5/],
    ['one mark for five rows', { svg_marks: 1 }, 'chart_marks', /chart svg has 1 mark element\(s\)/],
    ['axes counted as marks', { svg_marks: 40 }, 'chart_marks', /chart svg has 40 mark element\(s\)/],
    ['no table', { tables: [chartTable()] }, 'table_rows', /no visible data table captioned metrics/],
    ['a table for another binding', { tables: [table({ caption: 'venue' }), chartTable()] }, 'table_rows', /no visible data table/],
    ['one row too few', { tables: [table({ rows: fourRows }), chartTable()] }, 'table_rows', /data table has 4 body rows, the dataset has 5/],
    ['one row too many', { tables: [table({ rows: [...expected.cells, ['Extra', '1']] }), chartTable()] }, 'table_rows', /data table has 6 body rows/],
    ['a missing cell value', { tables: [table({ rows: wrongCell }), chartTable()] }, 'table_rows', /does not show the dataset values \["Published"\]/],
    ['rows out of order', { tables: [table({ rows: [...expected.cells].reverse() }), chartTable()] }, 'table_rows', /not the dataset rows in order/],
    ['other columns', { tables: [table({ columns: ['value', 'category'] }), chartTable()] }, 'table_rows', /data table columns are/],
    ['a value that is in the DOM but not in the visible text', { text: VIEW_TEXT.render_present.replaceAll('Published', '') }, 'table_rows', /"Published"\] are not in the visible text/],
    ['a chart without its own table', { tables: [table()] }, 'chart_source_table', /chart has no data table of its own/],
    ['a chart table with too few rows', { tables: [table(), chartTable({ rows: fourRows })] }, 'chart_source_table', /chart's own data table has 4 body rows/],
    ['a chart table with five foreign rows', { tables: [table(), chartTable({ rows: foreignRows })] }, 'chart_source_table', /chart's own data table does not show the dataset values/],
    ['a chart table with foreign columns', { tables: [table(), chartTable({ columns: ['k', 'v'] })] }, 'chart_source_table', /chart's own data table columns are \["k","v"\]/],
    ['a chart table with one changed cell', { tables: [table(), chartTable({ rows: wrongCell })] }, 'chart_source_table', /chart's own data table does not show the dataset values \["Published"\]/],
    ['a chart table out of order', { tables: [table(), chartTable({ rows: [...expected.cells].reverse() })] }, 'chart_source_table', /chart's own data table cells are not the dataset rows in order/],
    ['read_object never called', { tool_calls: without('read_object') }, 'read_object_calls', /read_object for [0-9a-f]{64} 0 time\(s\)/],
    ['read_object called once for everything', { tool_calls: [...without('read_object'), read('0', false, 223)] }, 'read_object_calls', /in one block; the ranged loop was not exercised/],
    ['reads of another object', { tool_calls: good.tool_calls.map((call) => (call.tool === 'read_object' ? { ...call, object: '0'.repeat(64) } : call)) }, 'read_object_calls', /0 time\(s\)/],
    ['reads that skip the middle', { tool_calls: [...without('read_object'), read('0', true), read('192', false, 31)] }, 'read_object_calls', /block starts at 192/],
    ['a last block that still has more', { tool_calls: [...without('read_object'), read('0', true), read('64', true)] }, 'read_object_calls', /without a final block/],
    ['a read that does not start at 0', { tool_calls: [...without('read_object'), read('64', true), read('128', false, 95)] }, 'read_object_calls', /block starts at 64/],
    ['a read that ends before the last byte', { tool_calls: [...without('read_object'), read('0', true), read('64', false)] }, 'read_object_calls', /ended at byte 128 of 223/],
    ['show never called', { tool_calls: without('show') }, 'show_calls', /did not call show/],
    ['a refused tool call', { tool_calls: [...good.tool_calls, { tool: 'read_object', ok: false }] }, 'no_refused_call', /harness refused tool calls/],
  ];
  for (const [label, change, check, problem] of failures) {
    const judged = judgePresentDataset(expected, change === null ? null : { ...good, ...change });
    assert.equal(judged.ok, false, `${label}: judged exercised`);
    assert.equal(judged.record.status, 'failed', label);
    assert.ok(judged.checks[check].some((item) => problem.test(item)), `${label}: ${check} is ${JSON.stringify(judged.checks)}`);
    assert.deepEqual(judged.record.problems, judged.problems, label);
    assert.deepEqual(judged.problems, Object.values(judged.checks).flat(), label);
  }
  // What the record states is what was seen, not what was hoped for.
  assert.equal(judgePresentDataset(expected, { ...good, svg_marks: 0 }).record.chart_marks, 0);
  assert.equal(judgePresentDataset(expected, { ...good, svgs: 0 }).record.chart_svgs, 0);
  assert.equal(judgePresentDataset(expected, { ...good, tables: [] }).record.table_rows, null);
  assert.deepEqual(judgePresentDataset(expected, { ...good, tables: [table({ caption: 'venue' })] }).record.table_captions, ['venue']);
  assert.equal(judgePresentDataset(expected, { ...good, tool_calls: without('read_object') }).record.read_object_calls, 0);
  assert.deepEqual(judgePresentDataset(expected, { ...good, tool_calls: without('read_object') }).record.read_object_digests, []);
  assert.equal(judgePresentDataset(expected, null).record.show_calls, 0);
  // A development build resolves the binding twice: accepted, and the record says two.
  const doubled = judgePresentDataset(expected, { ...good, tool_calls: good.tool_calls.flatMap((call) => [call, call]) });
  assert.equal(doubled.ok, true, JSON.stringify(doubled.problems));
  assert.equal(doubled.record.read_object_resolutions, 2);
  assert.equal(doubled.record.read_object_calls, 8);
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

/** The orchestrator and its rule modules, as text. */
async function mcpAppsSources() {
  const names = ['run.mjs', 'lib/qualify.mjs', 'lib/criteria.mjs', 'lib/protocol.mjs', 'lib/observe.mjs', 'lib/views.mjs', 'lib/basic-host.mjs'];
  const texts = await Promise.all(names.map((name) => readFile(join(root, 'qualification/mcp-apps', name), 'utf8')));
  return Object.fromEntries(names.map((name, index) => [name, texts[index]]));
}

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

  const sources = await mcpAppsSources();
  // Values the receipt once stated without having seen them, and instructions it carried.
  for (const [name, source] of Object.entries(sources)) {
    assert.doesNotMatch(source, /stdio: 'default'|cspObject: true|has_structured_content: true|build_command|serve_command|how_to_http_ngrok/, name);
  }
  assert.match(sources['lib/qualify.mjs'], /transport: transportRecord\(\{ requested: config\.mcp_url, reported: server\?\.endpoint \?\? null \}\)/);
  assert.match(sources['run.mjs'], /endpoint: listening\[1\]/);
  assert.match(sources['lib/qualify.mjs'], /static_bundle_smoke: \{\s*status: 'not_run'/);
  assert.match(sources['run.mjs'], /version: browser\.version\(\),/);

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
  const sources = await mcpAppsSources();
  assert.match(sources['lib/qualify.mjs'], /appBundleBuild\(\{ html, nodeEnv: bundle\.node_env \}\)/);
  assert.match(sources['lib/qualify.mjs'], /app_bundle: built\?\.app_bundle \?\? null,/);
  assert.match(sources['run.mjs'], /app_html: typeof first === 'string' \? await readFile\(join\(distApps,/);
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

test('the MCP Apps orchestrator only performs effects; no place but the fold decides a result', async () => {
  const sources = await mcpAppsSources();
  const run = sources['run.mjs'];
  assert.match(run, /await qualify\(effects, \{/);
  assert.match(run, /observeView\(playwrightDriver\(/);
  assert.match(run, /frame\.evaluate\(readAppDom, DOM_SELECTORS\)/);
  assert.match(run, /observeProtocol\(client, \{ views: VIEWS \}\)/);
  assert.match(run, /waitForListening\(proc,/);
  assert.match(run, /receiptHeader\(root, MCP_APPS_INPUTS\)/);
  assert.match(run, /recordReceipt\(root, 'mcp-apps', receipt\)/);
  assert.match(run, /process\.exitCode = exitCode;/);
  assert.doesNotMatch(run, /disableRules\(|session_open|spawnDetached|waitForTcp|requireCleanTree|commit_sha/);
  assert.doesNotMatch(run, /'cargo',\s*\[\s*'run'/);
  // The orchestrator judges nothing itself and reads no DOM itself.
  assert.doesNotMatch(run, /judgeView|judgePresentDataset|judgeProtocol|partitionAxe|foldCriteria|sealEnvelope|querySelector|allInnerTexts|throw new Error\(`MCP Apps qualification/);
  // A result is never typed: the three words appear in no string literal of the harness.
  for (const [name, source] of Object.entries(sources)) {
    assert.doesNotMatch(source, /['"`](PASS|FAIL|INCOMPLETE)['"`]/, name);
    assert.doesNotMatch(source, /not_exercised|status: 'exercised'|runProblems|basicHostVerdict/, name);
  }
  assert.deepEqual([...sources['lib/criteria.mjs'].matchAll(/result: foldCriteria\(/g)].length, 2, 'the draft and the sealed envelope');
  assert.doesNotMatch(sources['lib/qualify.mjs'], /foldCriteria|result:/);
  assert.match(sources['lib/qualify.mjs'], /\.\.\.envelope,/);
});

const APP_MIME = 'text/html;profile=mcp-app';
const DOUBLE_HTML = '<!doctype html><html><body><div id="root"></div></body></html>';
const AXE_CLEAN = { passes: [{ id: 'document-title', impact: null, nodes: [{ target: ['iframe', 'iframe', 'html'] }] }], violations: [], incomplete: [] };

/** The frame and the tool calls a correct render of each view leaves, for the committed fixtures. */
async function goodObservations() {
  const { digest, expected } = await committedDataset();
  const table = (extra) => ({ caption: 'metrics', visible: true, in_details: false, columns: expected.columns, rows: expected.cells, ...extra });
  const read = (offset, bytes, has_more) => ({ tool: 'read_object', ok: true, object: digest, offset, bytes, total_size: '223', has_more });
  const presentCalls = [
    { tool: 'render_present', ok: true }, { tool: 'show', ok: true }, { tool: 'show', ok: true },
    read('0', 64, true), read('64', 64, true), read('128', 64, true), read('192', 31, false),
  ];
  const of = (view) => ({
    url: basicHostUrl(view.tool),
    app_frame_depth: 2,
    frame: {
      text: VIEW_TEXT[view.tool],
      alerts: [],
      svgs: view.dataset ? 1 : 0,
      svg_marks: view.dataset ? 5 : 0,
      tables: view.dataset ? [table(), table({ visible: false, in_details: true })] : [],
    },
    tool_calls: view.dataset ? presentCalls : [{ tool: view.tool, ok: true }],
    settled: true,
    screenshot: `basic-host-${view.tool}.png`,
    axe: AXE_CLEAN,
  });
  return { of, expected, digest, table, read, presentCalls };
}

/**
 * Effects for lib/qualify.mjs that succeed as a correct run does, and a log of the order in
 * which they were called. `change` replaces single effects; `observe(view, good)` replaces
 * what a view render shows.
 */
async function qualifyDouble({ change = {}, observe = (_view, good) => good } = {}) {
  const observations = await goodObservations();
  const double = await harnessDouble();
  const calls = [];
  const written = [];
  const recorded = [];
  const sha = (text) => createHash('sha256').update(text).digest('hex');
  const plain = {
    removeReceipt: async () => {},
    header: async () => ({ git_sha: 'a'.repeat(40), inputs: ['qualification/mcp-apps'], produced_at: '2026-10-06T00:00:00.000Z' }),
    buildBundle: async () => ({
      manifest_resources: [{ uri: APP_RESOURCE_URI, name: 'app', mimeType: APP_MIME, byteLength: DOUBLE_HTML.length, sha256: sha(DOUBLE_HTML) }],
      sdk_mime_type: APP_MIME,
      app_html: DOUBLE_HTML,
      node_env: 'development',
    }),
    buildHarness: async () => ({ path: 'okf-qualify-mcp-apps.exe', bytes: 1, sha256: 'b'.repeat(64), args: ['--http', '127.0.0.1:18765'] }),
    expectations: async () => ({ dataset: observations.expected, product_read_object: double.product }),
    checkHarness: async () => ({ code: 0, report: { resources: [{ name: 'app', readable: true }], dataset: { sha256: observations.digest } }, stderr: '' }),
    startHarness: async () => ({
      endpoint: 'http://127.0.0.1:18765/mcp',
      toolLog: () => '',
      stop: async () => {
        calls.push('stopHarness');
        return { exited_before_teardown: false, exit: { code: 1, signal: null, error: null }, stderr_tail: '' };
      },
    }),
    observeProtocol: async () => observeProtocol(double.client, { views: VIEWS }),
    openTunnel: async () => ({ opened_at: 't1', public_url: 'https://x.ngrok.app/mcp', note: 'n', close: async () => 't2' }),
    startHost: async () => ({ source: { status: 'cached' }, stop: async () => calls.push('stopHost') }),
    launchBrowser: async () => ({
      version: '140.0.7339.16',
      render: async (view) => {
        calls.push(`render ${view.tool}`);
        return observe(view, observations.of(view));
      },
      close: async () => calls.push('closeBrowser'),
    }),
    hostRenderEvidence: async () => ({ status: 'not_run', gate: 'mcp-apps-web-hosts' }),
    writeReceipt: async (receipt) => {
      written.push(JSON.parse(JSON.stringify(receipt)));
      return 'receipt.json';
    },
    recordReceipt: async (receipt) => {
      recorded.push(JSON.parse(JSON.stringify(receipt)));
      return 'qualification/receipts/mcp-apps.json';
    },
    ...change,
  };
  const effects = Object.fromEntries(
    Object.entries(plain).map(([name, effect]) => [name, async (...args) => {
      calls.push(name);
      return effect(...args);
    }]),
  );
  const pinned = JSON.parse(await readFile(join(root, 'qualification/mcp-apps/criteria.json'), 'utf8')).required;
  const config = { harness: 'okf-qualify-mcp-apps', mcp_url: 'http://127.0.0.1:18765/mcp', http_port: 18765, ngrok: false };
  const run = (options = {}) => qualify(effects, { pinned, config, ...options });
  return { run, calls, written, recorded, pinned, observations };
}

const byId = (receipt, id) => receipt.criteria.find((criterion) => criterion.id === id);
const failedIds = (receipt) => receipt.criteria.filter((criterion) => criterion.result === 'fail').map((criterion) => criterion.id);

test('criteria.json pins every criterion the MCP Apps harness can emit as required, the present dataset among them', async () => {
  const pinnedFile = JSON.parse(await readFile(join(root, 'qualification/mcp-apps/criteria.json'), 'utf8'));
  assert.deepEqual(pinnedFile, { gate: MCP_APPS_GATE, required: requiredCriterionIds() }, 'adding or dropping a check must change the tracked criteria.json');
  assert.equal(MCP_APPS_GATE, 'mcp-apps-protocol-qualification');
  assert.deepEqual(pinnedFile.required, [...pinnedFile.required].sort());
  assert.equal(new Set(pinnedFile.required).size, pinnedFile.required.length);
  for (const id of ['chart_marks', 'table_rows', 'chart_source_table', 'dataset_exercised', 'read_object_calls', 'text', 'no_alert', 'accessibility']) {
    assert.ok(pinnedFile.required.includes(`render_present/${id}`), id);
  }
  for (const view of VIEWS) for (const rule of ['text', 'no_alert', 'accessibility']) assert.ok(pinnedFile.required.includes(`${view.tool}/${rule}`));
  const rules = criterionRules();
  for (const id of pinnedFile.required) {
    assert.match(id, /^[a-z_]+\/[a-z_]+$/, `${id} is not <scope>/<what>`);
    assert.ok(typeof rules[id] === 'string' && rules[id].length > 40, `${id} has no rule text`);
  }
  assert.deepEqual(Object.keys(rules).sort(), pinnedFile.required);
  assert.deepEqual(criterionIds().slice().sort(), requiredCriterionIds());

  // A view list that no longer exercises the dataset emits fewer ids: the pin catches it at run time too.
  const withoutDataset = VIEWS.map(({ dataset, ...view }) => view);
  assert.ok(!requiredCriterionIds(withoutDataset).includes('render_present/dataset_exercised'));
  const double = await qualifyDouble();
  const { receipt, exitCode, recorded } = await double.run({ views: withoutDataset, record: true });
  assert.equal(receipt.result, 'INCOMPLETE');
  assert.match(receipt.harness_error, /not the ones criteria\.json pins: pinned criterion render_present\/chart_marks is missing/);
  assert.match(receipt.harness_error, /pinned criterion render_present\/dataset_exercised is missing/);
  assert.ok(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned).includes('pinned criterion render_present/dataset_exercised is missing'));
  assert.equal(receipt.basic_host.present_dataset.status, 'not_observed');
  assert.equal(exitCode, 2);
  assert.match(recorded.refused, /not recorded: the envelope cannot be trusted/);
  assert.deepEqual(double.recorded, []);
});

test('the MCP Apps receipt carries the shared envelope, and its result is the fold of its criteria', async () => {
  const double = await qualifyDouble();
  const { receipt, exitCode, recorded } = await double.run();
  assert.deepEqual(Object.keys(receipt).slice(0, 9), ['git_sha', 'inputs', 'produced_at', 'component', 'gate', 'result', 'harness_error', 'criteria', 'not_judged']);
  assert.equal(receipt.gate, 'mcp-apps-protocol-qualification');
  assert.equal(receipt.result, 'PASS');
  assert.equal(receipt.harness_error, null);
  assert.deepEqual(receipt.not_judged, []);
  assert.deepEqual(receipt.criteria.map((criterion) => criterion.id), criterionIds());
  assert.ok(receipt.criteria.every((criterion) => criterion.required === true && criterion.result === 'pass'));
  assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned), []);
  assert.equal(receipt.result, mcpAppsFold(receipt.criteria, receipt.harness_error));
  assert.equal(exitCode, 0);
  assert.equal(recorded, null);
  assert.deepEqual(receiptHeaderProblems(receipt), []);
  assert.deepEqual(double.written, [JSON.parse(JSON.stringify(receipt))]);
  // The detailed sections say what was seen; their status words are derived from the criteria.
  assert.equal(receipt.protocol_check.status, 'passed');
  assert.equal(receipt.basic_host.status, 'passed');
  assert.deepEqual(receipt.basic_host.browser, { name: 'chromium', version: '140.0.7339.16' });
  assert.deepEqual(receipt.basic_host.views.map((view) => [view.tool, view.status]), VIEWS.map((view) => [view.tool, 'passed']));
  assert.equal(receipt.basic_host.present_dataset.status, 'exercised');
  assert.deepEqual(receipt.app_bundle, { node_env: 'development', react_development_build: false, marker: 'Download the React DevTools' });
  assert.deepEqual(receipt.rules, criterionRules());
  assert.equal(receipt.ngrok.status, 'not_run');
  // Exit codes: 0 only for PASS, and the other two differ.
  assert.deepEqual(EXIT_CODES, { PASS: 0, FAIL: 1, INCOMPLETE: 2 });
  assert.deepEqual(['PASS', 'FAIL', 'INCOMPLETE', 'anything else'].map(exitCodeFor), [0, 1, 2, 2]);
});

test('the receipt says in words what was the product and what was this harness', async () => {
  const { receipt } = await (await qualifyDouble()).run();
  assert.equal(receipt.scope, SCOPE);
  assert.deepEqual(Object.keys(SCOPE), ['summary', 'real', 'fixture', 'not_covered', 'reading_note']);
  assert.match(SCOPE.summary, /real MCP App bundle .* real MCP Apps host.* fixture MCP server.*does not test the product server/);
  assert.match(SCOPE.real.join('\n'), /App bundle[\s\S]*View code[\s\S]*host implementation/);
  assert.match(SCOPE.fixture.join('\n'), /MCP server is okf-qualify-mcp-apps, this harness; it is not the product server/);
  assert.match(SCOPE.fixture.join('\n'), /show and read_object answer from committed files under tests\/fixtures\/views/);
  assert.match(SCOPE.not_covered.join('\n'), /product server's show and read_object handlers[\s\S]*product blob store/);
  assert.match(SCOPE.reading_note, /"exercised" mean the App fetched the fixture dataset through the harness read_object.*do not mean the product read_object tool was called/);
  // N6: the alert rule says how far it sees.
  assert.match(receipt.rules['render_present/no_alert'], /Only the App frame is read: an alert in the host page or in the sandbox proxy frame is not seen/);
  assert.match(receipt.rules['render_present/chart_marks'], /Equality is right for this fixture because its chart is one unit bar/);
});

test('a criterion about the App or the protocol that fails makes the run FAIL, whatever else passed', async () => {
  const { table, expected } = await goodObservations();
  const foreign = [['a', '1'], ['b', '2'], ['c', '3'], ['d', '4'], ['e', '5']];
  const present = (changeFrame, changeObservation = {}) => (view, good) =>
    view.tool === 'render_present' ? { ...good, ...changeObservation, frame: { ...good.frame, ...changeFrame } } : good;
  const cases = [
    ['the dataset is not drawn although the view text is there', present({ tables: [table({ rows: foreign }), table({ visible: false, in_details: true })] }), ['render_present/table_rows', 'render_present/dataset_exercised']],
    ['the chart has its axes but one mark', present({ svg_marks: 1 }), ['render_present/chart_marks', 'render_present/dataset_exercised']],
    ["the chart's own table holds other rows", present({ tables: [table(), table({ visible: false, in_details: true, rows: foreign })] }), ['render_present/chart_source_table', 'render_present/dataset_exercised']],
    ['the App never read the dataset', present({}, { tool_calls: [{ tool: 'show', ok: true }] }), ['render_present/read_object_calls', 'render_present/dataset_exercised']],
    ['the App never called show', (view, good) => (view.dataset ? { ...good, tool_calls: good.tool_calls.filter((call) => call.tool !== 'show') } : good), ['render_present/show_calls', 'render_present/dataset_exercised']],
    ['the harness refused a call', (view, good) => (view.dataset ? { ...good, tool_calls: [...good.tool_calls, { tool: 'read_object', ok: false }] } : good), ['render_present/no_refused_call', 'render_present/dataset_exercised']],
    ['the present view shows an alert', present({ alerts: ['Dataset digest verification failed'] }), ['render_present/no_alert', 'render_present/dataset_exercised']],
    ['a view shows another view', (view, good) => (view.tool === 'render_source' ? { ...good, frame: { ...good.frame, text: VIEW_TEXT.render_timeline } } : good), ['render_source/text']],
    ['a view shows an alert', (view, good) => (view.tool === 'render_changes' ? { ...good, frame: { ...good.frame, alerts: ['boom'] } } : good), ['render_changes/no_alert']],
    ['axe finds a serious violation in the App frame', (view, good) => (view.tool === 'render_timeline' ? { ...good, axe: { ...AXE_CLEAN, violations: [{ id: 'color-contrast', impact: 'serious', nodes: [{ target: ['iframe', 'iframe', 'code'] }] }] } } : good), ['render_timeline/accessibility']],
  ];
  for (const [label, observe, failing] of cases) {
    const double = await qualifyDouble({ observe });
    const { receipt, exitCode } = await double.run();
    assert.deepEqual(failedIds(receipt), failing, label);
    assert.equal(receipt.result, 'FAIL', label);
    assert.equal(receipt.harness_error, null, label);
    assert.equal(exitCode, 1, label);
    assert.equal(receipt.basic_host.status, 'failed', label);
    assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned), [], label);
    assert.deepEqual(receipt.criteria.map((criterion) => criterion.id), criterionIds(), `${label}: every criterion is still listed`);
  }
  assert.equal(expected.rows, 5);

  // A view that never appeared: its text fails, what could not be seen is listed as not judged.
  const unseen = await qualifyDouble({ observe: (view, good) => (view.dataset ? { ...good, frame: null, app_frame_depth: null, axe: null, settled: false } : good) });
  const blank = (await unseen.run()).receipt;
  assert.equal(blank.result, 'FAIL');
  assert.deepEqual(failedIds(blank), ['render_present/text', 'render_present/dataset_exercised']);
  assert.deepEqual(blank.not_judged, ['render_present/no_alert', 'render_present/accessibility', 'render_present/chart_marks', 'render_present/table_rows', 'render_present/chart_source_table']);
  assert.equal(blank.basic_host.present_dataset.status, 'failed');

  // The bundle and the protocol are judged the same way.
  const development = await qualifyDouble({ change: { buildBundle: async () => ({
    manifest_resources: [{ uri: APP_RESOURCE_URI, name: 'app', mimeType: 'text/html', byteLength: 1, sha256: 'c'.repeat(64) }, { uri: 'ui://x', name: 'other', mimeType: APP_MIME, byteLength: 1, sha256: 'd'.repeat(64) }],
    sdk_mime_type: APP_MIME,
    app_html: `${DOUBLE_HTML}<script>console.info("${REACT_DEVELOPMENT_MARKER}")</script>`,
    node_env: 'development',
  }) } });
  const dev = (await development.run()).receipt;
  assert.deepEqual(failedIds(dev), ['bundle/single_app_resource', 'bundle/mime_type', 'bundle/react_production_build', 'protocol/resource_is_built_bundle']);
  assert.equal(dev.result, 'FAIL');
  const unreadable = await qualifyDouble({ change: { checkHarness: async () => ({ code: 1, report: { resources: [{ name: 'app', readable: false }], dataset: { sha256: (await goodObservations()).digest } }, stderr: 'one or more MCP App resources were not readable' }) } });
  assert.deepEqual(failedIds((await unreadable.run()).receipt), ['bundle/served_as_html']);
  const wrongProtocol = await harnessDouble({ blockBytes: 1000 });
  const protocol = await qualifyDouble({ change: { observeProtocol: async () => observeProtocol(wrongProtocol.client, { views: VIEWS }) } });
  const judged = (await protocol.run()).receipt;
  assert.deepEqual(failedIds(judged), ['protocol/read_object_ranged_loop']);
  assert.equal(judged.protocol_check.status, 'failed');
  assert.equal(judged.result, 'FAIL');
});

test('what the machine could not do is a harness error: INCOMPLETE, a receipt for this run, and no criterion marked failed', async () => {
  const boom = (message) => async () => {
    throw new Error(message);
  };
  const noBrowser = "browserType.launch: Executable doesn't exist at D:\\empty\\chromium-1243\\chrome-win\\chrome.exe\nLooks like Playwright was just installed or updated.";
  const stages = [
    ['buildBundle', boom('the UI build (bun --bun run build in ui/) exited 1'), /^the App bundle was not built: the UI build .* exited 1$/, 0],
    ['buildHarness', boom('cargo build --locked --release -p okf-qualify-mcp-apps exited 101'), /^the harness binary was not built: cargo build/, 3],
    ['expectations', boom('ENOENT'), /^the committed fixtures could not be read as an expectation: ENOENT$/, 3],
    ['checkHarness', boom('spawn EACCES'), /^the harness --check did not run: spawn EACCES$/, 3],
    ['checkHarness', async () => ({ code: 1, report: null, stderr: 'okf-qualify-mcp-apps: manifest must list exactly 1 shared App resource' }), /^the harness --check printed no report \(exit 1\): okf-qualify-mcp-apps: manifest must list/, 3],
    ['checkHarness', async () => ({ code: 0, report: { resources: [{ name: 'app', readable: true }], dataset: { sha256: 'e'.repeat(64) } }, stderr: '' }), /^the harness serves dataset e{64}; the committed fixture is [0-9a-f]{64}$/, 4],
    ['startHarness', boom('okf-qualify-mcp-apps exited before listening (exit 1)\nstderr:\nbind 127.0.0.1:18765: access denied'), /^the harness server did not start: okf-qualify-mcp-apps exited before listening \(exit 1\) \| stderr: \| bind/, 4],
    ['observeProtocol', boom('fetch failed'), /^the MCP client could not reach the harness server: fetch failed$/, 4],
    ['startHost', boom('fetch basic-host package.json: HTTP 503'), /^the reference host \(basic-host\) did not start: fetch basic-host package\.json: HTTP 503$/, 17],
    ['launchBrowser', boom(noBrowser), /^Chromium did not start: browserType\.launch: Executable doesn't exist at .* \| Looks like Playwright was just installed or updated\.$/, 17],
  ];
  for (const [effect, behaviour, sentence, judgedCount] of stages) {
    const double = await qualifyDouble({ change: { [effect]: behaviour } });
    const { receipt, exitCode } = await double.run();
    const label = `${effect}: ${receipt.harness_error}`;
    assert.equal(receipt.result, 'INCOMPLETE', label);
    assert.match(receipt.harness_error, sentence, label);
    assert.deepEqual(failedIds(receipt), [], `${label}: an environment failure is never a product failure`);
    assert.equal(exitCode, 2, label);
    assert.equal(receipt.criteria.filter((criterion) => criterion.result === 'pass').length, judgedCount, label);
    assert.deepEqual(receipt.not_judged, receipt.criteria.filter((criterion) => criterion.result === 'not_judged').map((criterion) => criterion.id), label);
    assert.equal(receipt.not_judged.length, criterionIds().length - judgedCount, label);
    assert.ok(receipt.criteria.filter((criterion) => criterion.result === 'not_judged').every((criterion) => criterion.detail.startsWith('not reached: ')), label);
    assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned), [], label);
    // The old receipt goes before anything else, and this run writes its own, once, after everything was stopped.
    assert.equal(double.calls[0], 'removeReceipt', label);
    assert.equal(double.calls.filter((call) => call === 'writeReceipt').length, 1, label);
    assert.equal(double.calls.at(-1), 'writeReceipt', label);
    assert.equal(double.calls.includes('startHarness') && effect !== 'startHarness', double.calls.includes('stopHarness'), label);
  }

  // One view the host could not show: the others are still judged, nothing is failed.
  const oneView = await qualifyDouble({ observe: (view, good) => {
    if (view.tool === 'render_changes') throw new HarnessError('basic-host could not reach the harness server: Failed to connect to any servers');
    return good;
  } });
  const partial = (await oneView.run()).receipt;
  assert.equal(partial.result, 'INCOMPLETE');
  assert.match(partial.harness_error, /^render_changes could not be shown in the reference host: basic-host could not reach the harness server/);
  assert.deepEqual(partial.not_judged, ['render_changes/text', 'render_changes/no_alert', 'render_changes/accessibility']);
  assert.deepEqual(failedIds(partial), []);
  assert.equal(partial.basic_host.status, 'incomplete');

  // axe that did not reach the App frame judged nothing; a violation in the host's own page is not the App's.
  const shallow = await qualifyDouble({ observe: (view, good) => (view.tool === 'render_source' ? { ...good, axe: { passes: [{ id: 'x', impact: null, nodes: [{ target: ['body'] }] }], violations: [], incomplete: [] } } : good) });
  const unreached = (await shallow.run()).receipt;
  assert.equal(unreached.result, 'INCOMPLETE');
  assert.deepEqual(unreached.not_judged, ['render_source/accessibility']);
  assert.match(unreached.harness_error, /^axe did not analyse the App frame of render_source$/);
  const chrome = await qualifyDouble({ observe: (view, good) => (view.tool === 'render_source' ? { ...good, axe: { ...AXE_CLEAN, violations: [{ id: 'button-name', impact: 'critical', nodes: [{ target: ['.closeButton'] }] }, { id: 'frame-title', impact: 'serious', nodes: [{ target: ['iframe'] }] }] } } : good) });
  const hostChrome = (await chrome.run()).receipt;
  assert.equal(hostChrome.result, 'INCOMPLETE');
  assert.deepEqual(failedIds(hostChrome), []);
  assert.deepEqual(hostChrome.not_judged, []);
  assert.match(hostChrome.harness_error, /reference host's own chrome has serious or critical axe violations outside the tolerated upstream rules \["color-contrast","frame-title"\] while showing render_source: \["button-name"\]/);

  // A failure of the App found before the machine gave up is still a failure.
  const both = await qualifyDouble({ change: { launchBrowser: boom(noBrowser), observeProtocol: async () => observeProtocol((await harnessDouble({ blockBytes: 1000 })).client, { views: VIEWS }) } });
  const mixed = (await both.run()).receipt;
  assert.equal(mixed.result, 'FAIL');
  assert.match(mixed.harness_error, /^Chromium did not start/);

  // A dirty tree has no commit to cite: the old receipt is gone, none is written, and the run rejects.
  const dirty = await qualifyDouble({ change: { header: boom('Qualification requires a clean git tree before writing a receipt. Dirty paths:\n M x') } });
  await assert.rejects(dirty.run(), /requires a clean git tree/);
  assert.deepEqual(dirty.calls, ['removeReceipt', 'header']);
  assert.deepEqual(dirty.written, []);
});

test('a protocol-only run leaves every render criterion not judged and so folds to INCOMPLETE', async () => {
  const double = await qualifyDouble();
  const { receipt, exitCode } = await double.run({ protocolOnly: true });
  assert.equal(receipt.result, 'INCOMPLETE');
  assert.equal(receipt.harness_error, null, 'nothing went wrong; the render half was not asked for');
  assert.deepEqual(receipt.not_judged, criterionIds().filter((id) => id.startsWith('render_')));
  assert.equal(receipt.not_judged.length, 19);
  assert.ok(receipt.criteria.filter((criterion) => criterion.result === 'not_judged').every((criterion) => criterion.detail === 'skipped: protocol-only run'));
  assert.ok(receipt.criteria.filter((criterion) => !criterion.id.startsWith('render_')).every((criterion) => criterion.result === 'pass'));
  assert.equal(exitCode, 2);
  assert.equal(receipt.protocol_only, true);
  assert.equal(receipt.protocol_check.status, 'passed');
  assert.deepEqual([receipt.basic_host.status, receipt.basic_host.reason], ['not_run', 'skipped: protocol-only run']);
  assert.deepEqual(receipt.host_render, { status: 'not_run', reason: 'skipped: protocol-only run' });
  for (const effect of ['startHost', 'launchBrowser', 'openTunnel', 'hostRenderEvidence']) assert.ok(!double.calls.includes(effect), effect);
  assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned), []);
  // It is refused as the gate receipt before anything is touched.
  const refused = await qualifyDouble();
  await assert.rejects(refused.run({ protocolOnly: true, record: true }), /--record refused: a protocol-only run is not the gate receipt/);
  assert.deepEqual(refused.calls, []);
});

test('--record writes nothing before the result is known, records any result, and never an envelope that cannot be trusted', async () => {
  const passed = await qualifyDouble();
  const pass = await passed.run({ record: true });
  assert.deepEqual(pass.recorded, { path: 'qualification/receipts/mcp-apps.json', refused: null });
  assert.deepEqual(passed.recorded, passed.written);
  assert.deepEqual(passed.calls.slice(-6), ['closeBrowser', 'stopHost', 'stopHarness', 'hostRenderEvidence', 'writeReceipt', 'recordReceipt']);
  assert.equal(passed.calls.filter((call) => call === 'writeReceipt' || call === 'recordReceipt').length, 2);

  // A failed run is recorded as a failed run: honest evidence, with the result its criteria fold to.
  const failed = await qualifyDouble({ observe: (view, good) => (view.dataset ? { ...good, frame: { ...good.frame, svg_marks: 1 } } : good) });
  const fail = await failed.run({ record: true });
  assert.equal(fail.exitCode, 1);
  assert.equal(failed.recorded.length, 1);
  assert.equal(failed.recorded[0].result, 'FAIL');
  assert.deepEqual(failed.calls.slice(-2), ['writeReceipt', 'recordReceipt']);
  // So is one the machine could not finish.
  const stopped = await qualifyDouble({ change: { launchBrowser: async () => { throw new Error("Executable doesn't exist"); } } });
  await stopped.run({ record: true });
  assert.equal(stopped.recorded[0].result, 'INCOMPLETE');
  for (const receipt of [...passed.recorded, ...failed.recorded, ...stopped.recorded]) {
    assert.equal(receipt.result, mcpAppsFold(receipt.criteria, receipt.harness_error));
    assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, passed.pinned), []);
  }

  // The seal: the result is only ever the fold, and criteria that are not the pinned ones are a harness error.
  const criteria = passed.written[0].criteria;
  assert.equal(sealEnvelope({ criteria, pinned: passed.pinned }).result, 'PASS');
  const extra = sealEnvelope({ criteria: [...criteria, { id: 'render_present/new_check', required: true, result: 'pass' }], pinned: passed.pinned });
  assert.equal(extra.result, 'INCOMPLETE');
  assert.match(extra.harness_error, /required criterion render_present\/new_check is not pinned/);
  const optional = sealEnvelope({ criteria: criteria.map((criterion) => (criterion.id === 'render_present/dataset_exercised' ? { ...criterion, required: false } : criterion)), pinned: passed.pinned });
  assert.equal(optional.result, 'INCOMPLETE');
  assert.match(optional.harness_error, /pinned criterion render_present\/dataset_exercised is not marked required/);
  const dropped = sealEnvelope({ criteria: criteria.filter((criterion) => criterion.id !== 'protocol/read_object_digest'), pinned: passed.pinned });
  assert.equal(dropped.result, 'INCOMPLETE');
  assert.deepEqual(sealEnvelope({ criteria, harnessErrors: ['a', 'b'], pinned: passed.pinned }).harness_error, 'a; b');
  assert.equal(sealEnvelope({ criteria: 'nonsense', pinned: passed.pinned }).result, 'INCOMPLETE');
  assert.deepEqual(sealEnvelope({ criteria: criteria.map((criterion, index) => (index === 0 ? { ...criterion, result: 'not_judged', detail: 'x' } : criterion)), pinned: passed.pinned }).not_judged, [criteria[0].id]);
  // A section's word is derived, and says not_run only when nothing in it was judged.
  assert.equal(sectionStatus(criteria, (id) => id.startsWith('protocol/')), 'passed');
  assert.equal(sectionStatus(criteria.map((criterion) => ({ ...criterion, result: 'not_judged' })), () => true), 'not_run');
  assert.equal(sectionStatus(stopped.recorded[0].criteria, (id) => id.startsWith('render_')), 'not_run');
});

/** A document double that answers only the selectors it is given, so the selector text itself is pinned. */
function documentDouble(bySelector, bodyText) {
  return {
    body: { innerText: bodyText },
    querySelectorAll(selector) {
      if (!(selector in bySelector)) throw new Error(`unexpected selector ${selector}`);
      return bySelector[selector];
    },
  };
}

test('the App document is read with the selectors that separate marks from axes and alerts from text', () => {
  assert.deepEqual(DOM_SELECTORS, {
    alert: '[role="alert"]',
    svg: 'svg',
    chart_mark: 'svg g[class~="role-mark"] > *',
    table: 'table',
    header_cell: 'thead th',
  });
  const cell = (textContent) => ({ textContent });
  const tableNode = ({ caption, rects, details, columns, rows }) => ({
    caption: caption === null ? null : { textContent: caption },
    getClientRects: () => ({ length: rects }),
    closest: (selector) => (selector === 'details' && details ? {} : null),
    querySelectorAll: (selector) => (selector === 'thead th' ? columns.map(cell) : (() => { throw new Error(`unexpected selector ${selector}`); })()),
    tBodies: [{ rows: rows.map((row) => ({ cells: row.map(cell) })) }],
  });
  const marks = Array.from({ length: 5 }, () => ({}));
  const everything = Array.from({ length: 40 }, () => ({}));
  const doc = documentDouble({
    '[role="alert"]': [{ innerText: 'Dataset digest verification failed' }],
    svg: [{}],
    'svg g[class~="role-mark"] > *': marks,
    // What a loosened selector would count: the axes, their ticks and labels as well.
    'svg *': everything,
    table: [
      tableNode({ caption: 'metrics', rects: 1, details: false, columns: ['category', 'value'], rows: [['Ingested', '412']] }),
      tableNode({ caption: 'metrics', rects: 0, details: true, columns: ['category', 'value'], rows: [['Ingested', '412']] }),
      tableNode({ caption: null, rects: 1, details: false, columns: [], rows: [] }),
    ],
  }, 'Six-component catalog');
  assert.deepEqual(readAppDom(DOM_SELECTORS, doc), {
    text: 'Six-component catalog',
    alerts: ['Dataset digest verification failed'],
    svgs: 1,
    svg_marks: 5,
    tables: [
      { caption: 'metrics', visible: true, in_details: false, columns: ['category', 'value'], rows: [['Ingested', '412']] },
      { caption: 'metrics', visible: false, in_details: true, columns: ['category', 'value'], rows: [['Ingested', '412']] },
      { caption: '', visible: true, in_details: false, columns: [], rows: [] },
    ],
  });
  // It is sent to the page as source text, so it may name nothing outside itself.
  assert.doesNotMatch(readAppDom.toString(), /DOM_SELECTORS|import|require\(/);
  const empty = readAppDom(DOM_SELECTORS, { body: null, querySelectorAll: () => [] });
  assert.deepEqual(empty, { text: '', alerts: [], svgs: 0, svg_marks: 0, tables: [] });

  // The App document is the deepest frame at least two levels below the host page.
  const frame = (parent) => ({ parentFrame: () => parent });
  const hostPage = frame(null);
  const sandbox = frame(hostPage);
  const app = frame(sandbox);
  const nested = frame(app);
  assert.equal(appFrameOf([hostPage, sandbox]), null);
  assert.deepEqual(appFrameOf([hostPage, sandbox, app]), { frame: app, depth: 2 });
  assert.equal(appFrameOf([hostPage, nested, sandbox, app]).frame, nested);
});

test('a view is watched from the moment it is opened: only its own tool calls count, and waiting stops when it has settled', async () => {
  const { of, expected, presentCalls } = await goodObservations();
  const present = VIEWS[3];
  const line = (call) => `${TOOL_CALL_LOG_PREFIX}${JSON.stringify(call)}\n`;
  const frame = (parent) => ({ parentFrame: () => parent });
  const appFrame = frame(frame(frame(null)));
  /** A driver whose App frame shows `frames[n]` on the n-th read and whose server log grows by `appends[n]`. */
  const driver = ({ frames, appends = [], hostText = 'basic-host', before = '' }) => {
    const state = { log: before, reads: 0, clock: 0, opened: [], waits: 0, axe: 0, screenshots: [] };
    return {
      state,
      now: () => state.clock,
      toolLog: () => state.log,
      open: async (url) => {
        state.opened.push(url);
      },
      hostText: async () => hostText,
      frames: () => [appFrame.parentFrame().parentFrame(), appFrame.parentFrame(), appFrame],
      readFrame: async (given) => {
        assert.equal(given, appFrame);
        state.log += appends[state.reads] ?? '';
        const shown = frames[Math.min(state.reads, frames.length - 1)];
        state.reads += 1;
        return shown;
      },
      wait: async (ms) => {
        state.waits += 1;
        state.clock += ms;
      },
      screenshot: async (name) => {
        state.screenshots.push(name);
        return `basic-host-${name}.png`;
      },
      axe: async () => {
        state.axe += 1;
        return AXE_CLEAN;
      },
    };
  };
  const good = of(present);
  const appLog = presentCalls.map(line).join('');
  // The protocol check read the dataset through the same server before this view was opened.
  const earlier = presentCalls.filter((call) => call.tool === 'read_object').map(line).join('');

  const waiting = { ...good.frame, text: 'Waiting for a tool result from the connected host.', svgs: 0, svg_marks: 0, tables: [] };
  const watched = driver({ frames: [waiting, waiting, good.frame], appends: ['', appLog.slice(0, 200), appLog.slice(200)], before: earlier });
  const observation = await observeView(watched, present, expected, { timeoutMs: 30_000, pollMs: 500 });
  assert.deepEqual(watched.state.opened, [basicHostUrl('render_present')]);
  assert.equal(observation.settled, true);
  assert.equal(watched.state.reads, 3, 'it reads until the view has settled, then stops');
  assert.equal(watched.state.waits, 2);
  assert.deepEqual(observation.tool_calls, presentCalls, 'only calls reported after the view was opened are this view\'s');
  assert.equal(observation.tool_calls.filter((call) => call.tool === 'read_object').length, 4);
  assert.deepEqual(observation.frame, good.frame);
  assert.equal(observation.app_frame_depth, 2);
  assert.equal(observation.screenshot, 'basic-host-render_present.png');
  assert.equal(observation.axe, AXE_CLEAN);
  const judged = viewCriteria(present, expected, observation);
  assert.deepEqual(judged.criteria.filter((criterion) => criterion.result !== 'pass'), []);
  assert.equal(judged.record.present_dataset.read_object_calls, 4);
  assert.equal(judged.record.present_dataset.read_object_resolutions, 1);

  // Settled means the dataset too: the text alone does not end the wait or pass the view.
  const noData = { ...good.frame, svgs: 0, svg_marks: 0, tables: [] };
  assert.equal(viewSettled(present, expected, { frame: good.frame, tool_calls: presentCalls }), true);
  assert.equal(viewSettled(present, expected, { frame: noData, tool_calls: presentCalls }), false);
  assert.equal(viewSettled(present, expected, { frame: good.frame, tool_calls: [] }), false);
  assert.equal(viewSettled(present, expected, { frame: null, tool_calls: presentCalls }), false);
  assert.equal(viewSettled(VIEWS[0], null, { frame: of(VIEWS[0]).frame, tool_calls: [] }), true);
  const never = driver({ frames: [noData], appends: [appLog] });
  const timedOut = await observeView(never, present, expected, { timeoutMs: 2_000, pollMs: 500 });
  assert.equal(timedOut.settled, false);
  assert.equal(never.state.waits, 4, 'it waits until the deadline, not longer');
  assert.equal(timedOut.axe, null);
  assert.equal(never.state.axe, 0, 'axe runs only on a rendered view');
  assert.deepEqual(never.state.screenshots, ['render_present']);
  const unsettled = viewCriteria(present, expected, timedOut);
  assert.deepEqual(unsettled.criteria.filter((criterion) => criterion.result === 'fail').map((criterion) => criterion.id), [
    'render_present/chart_marks', 'render_present/table_rows', 'render_present/chart_source_table', 'render_present/dataset_exercised',
  ]);
  assert.deepEqual(unsettled.criteria.filter((criterion) => criterion.result === 'not_judged').map((criterion) => criterion.id), ['render_present/accessibility']);
  assert.equal(unsettled.record.status, 'failed');
  assert.match(unsettled.record.error, /render_present\/dataset_exercised: no chart svg in the App frame/);

  // A host that cannot reach the harness is the environment, and says so at once.
  const unreachable = driver({ frames: [good.frame], hostText: 'Failed to connect to any servers: TypeError' });
  await assert.rejects(observeView(unreachable, present, expected), (error) => error instanceof HarnessError && /basic-host could not reach the harness server/.test(error.message));
  assert.equal(unreachable.state.reads, 0);
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

  // What no import statement names: fixtures, lockfiles, manifests, toolchains, tool
  // configuration, and what `bun --bun run build` reads in ui/.
  for (const path of [
    'tests/fixtures/views/present-response.json',
    'tests/fixtures/views/present-metrics-dataset.json',
    'qualification/mcp-apps/src/main.rs',
    'qualification/mcp-apps/Cargo.toml',
    'qualification/mcp-apps/criteria.json',
    'api/mcp-tools.json',
    'scripts/lib/receipt-envelope.mjs',
    'Cargo.toml',
    'Cargo.lock',
    'rust-toolchain.toml',
    '.cargo/config.toml',
    'bun.lock',
    'bunfig.toml',
    'package.json',
    '.bun-version',
    'ui/package.json',
    'ui/vite.config.ts',
    'ui/index.html',
    'ui/tsconfig.json',
    'ui/scripts/bundle-docs.mjs',
    'ui/src/styles.css',
    'ui/src/lib/wire.ts',
  ]) {
    await stat(join(root, path));
    assert.ok(covered(path), `${path} is not covered by the receipt inputs`);
  }
  // Narrowed: a change to a test, to lint or test configuration or to prose under ui/ does
  // not make the receipt stale, because nothing the run executes reads it.
  assert.ok(!MCP_APPS_INPUTS.includes('ui'), 'ui/ as a whole is no longer an input');
  for (const path of [
    'ui/tests/e2e/mcp-apps-static-bundle-smoke.spec.ts',
    'ui/tests/unit/layout.test.tsx',
    'ui/AGENTS.md',
    'ui/biome.json',
    'ui/playwright.config.ts',
    'ui/vitest.config.ts',
    'ui/tsconfig.tests.json',
  ]) {
    await stat(join(root, path));
    assert.ok(!covered(path), `${path} is an input again`);
  }
  // The narrowing holds only while these stay true: the stylesheet scans ui/src alone, the App
  // build takes no configuration file, and the build script runs these three steps.
  assert.match(await readFile(join(root, 'ui/src/styles.css'), 'utf8'), /^@import "tailwindcss" source\("\.\/"\);/);
  assert.match(await readFile(join(root, 'ui/scripts/bundle-app.mjs'), 'utf8'), /configFile: false,/);
  assert.equal(JSON.parse(await readFile(join(root, 'ui/package.json'), 'utf8')).scripts.build, 'vite build && bun scripts/bundle-app.mjs && bun scripts/bundle-docs.mjs');

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
  assert.match(source, /run\(harness\.path, \['--check'\], \{ env: harnessEnv,/);
  assert.match(source, /spawnGroup\(harness\.path, harness\.args, \{\s*env: \{\s*\.\.\.harnessEnv,/);
  assert.equal(source.match(/OKF_MCP_APPS_(DIST|FIXTURES):/g).length, 2, 'no second place builds the harness environment');
  // And what the harness says it serves is compared with the committed dataset before anything
  // renders: a harness serving another dataset stops the run as a harness error.
  const other = await qualifyDouble({ change: { checkHarness: async () => ({ code: 0, report: { resources: [{ name: 'app', readable: true }], dataset: { sha256: 'e'.repeat(64) } }, stderr: '' }) } });
  const { receipt } = await other.run();
  assert.match(receipt.harness_error, /the harness serves dataset e{64}; the committed fixture is [0-9a-f]{64}/);
  assert.ok(!other.calls.includes('startHarness'));
});

/**
 * An MCP client double that answers as the harness must for the committed fixtures, and what
 * the protocol rules then expect. `change` replaces single answers; an answer may throw.
 */
async function harnessDouble(change = {}) {
  const { bytes, digest, present } = await committedDataset();
  const product = JSON.parse(await readFile(join(root, 'api/mcp-tools.json'), 'utf8')).tools.find((tool) => tool.name === 'read_object');
  const tool = (name, ui, extra = {}) => ({ name, _meta: { ui }, inputSchema: { type: 'object' }, ...extra });
  const text = (value) => [{ type: 'text', text: value }];
  const answers = {
    blockBytes: 64,
    tools: () => [
      ...VIEWS.map((view) => tool(view.tool, { resourceUri: APP_RESOURCE_URI })),
      tool('show', { visibility: ['app'] }),
      tool('read_object', { visibility: ['app'] }, { inputSchema: product.inputSchema, outputSchema: product.outputSchema }),
    ],
    resources: () => [{ uri: APP_RESOURCE_URI, name: 'app', mimeType: APP_MIME }],
    resource: (uri) => ({ contents: [{ uri, mimeType: APP_MIME, text: DOUBLE_HTML, _meta: { ui: { csp: { connectDomains: [], resourceDomains: [] } } } }] }),
    render: (name) => ({ structuredContent: name === 'render_present' ? present : { fixture: name }, content: text(`${name} (text fallback).`) }),
    show: (args) => ({ structuredContent: { source: { item_id: args.item_id, revision: args.at.revision } }, content: text('show') }),
    block: (block) => block,
    readObject(args) {
      const known = ['source', 'object', 'offset', 'length'];
      if (Object.keys(args).some((key) => !known.includes(key)) || args.object !== digest) return { isError: true, content: text('read_object: refused') };
      const offset = Number(args.offset ?? '0');
      const part = bytes.subarray(offset, offset + answers.blockBytes);
      return {
        structuredContent: answers.block({
          sha256: digest,
          offset: String(offset),
          total_size: String(bytes.length),
          media_type: 'application/json',
          data_base64: part.toString('base64'),
          has_more: offset + part.length < bytes.length,
        }),
        content: text('block'),
      };
    },
    ...change,
  };
  const client = {
    getServerVersion: () => ({ name: 'okf-qualify-mcp-apps', version: '0.1.0' }),
    listTools: async () => ({ tools: answers.tools() }),
    listResources: async () => ({ resources: answers.resources() }),
    readResource: async ({ uri }) => answers.resource(uri),
    callTool: async ({ name, arguments: args }) => (name === 'show' ? answers.show(args) : name === 'read_object' ? answers.readObject(args) : answers.render(name)),
  };
  const expected = {
    render_tools: VIEWS.map((view) => view.tool),
    app_only_tools: APP_ONLY_TOOLS,
    resource_uri: APP_RESOURCE_URI,
    mime_type: APP_MIME,
    bundle_sha256: createHash('sha256').update(DOUBLE_HTML).digest('hex'),
    product_read_object: product,
  };
  return { client, expected, digest, bytes, product, present };
}

test('the protocol check judges every rule as a criterion and passes a server that answers as the harness must', async () => {
  const { client, expected, digest } = await harnessDouble();
  const observed = await observeProtocol(client, { views: VIEWS });
  const judged = judgeProtocol(expected, observed);
  assert.deepEqual(judged.criteria.map((criterion) => criterion.id), PROTOCOL_CRITERIA);
  assert.deepEqual(judged.criteria.filter((criterion) => criterion.result !== 'pass'), []);
  assert.ok(judged.criteria.every((criterion) => criterion.required === true));
  assert.equal(PROTOCOL_CRITERIA.length, 13);
  for (const id of PROTOCOL_CRITERIA) {
    assert.match(id, /^protocol\/[a-z_]+$/);
    assert.ok(PROTOCOL_RULES[id].length > 40, `${id} has no rule text`);
  }
  // What the receipt keeps is what was answered.
  assert.deepEqual(judged.record.server, { name: 'okf-qualify-mcp-apps', version: '0.1.0' });
  assert.deepEqual(judged.record.object_reads, [{
    binding: 'metrics', object: digest, blocks: 4, block_offsets: ['0', '64', '128', '192'], stopped: 'finished', assembled_bytes: 223, assembled_sha256: digest,
  }]);
  assert.deepEqual(judged.record.show_calls.map((call) => call.binding), ['venue', 'metrics']);
  assert.deepEqual(judged.record.unknown_digest, { is_error: true, structured: false, text: 'read_object: refused' });
  assert.deepEqual(judged.record.unknown_argument, { is_error: true, structured: false, text: 'read_object: refused' });
  const listed = judged.record.tools.find((tool) => tool.name === 'read_object');
  assert.equal(listed.input_schema_sha256, judged.record.product_read_object.input_schema_sha256);
  assert.equal(listed.output_schema_sha256, judged.record.product_read_object.output_schema_sha256);
  assert.match(listed.output_schema_sha256, /^[0-9a-f]{64}$/);

  // Key order is not a difference; a value is.
  assert.equal(canonicalJson({ b: [1, { d: 1, c: 2 }], a: null }), canonicalJson({ a: null, b: [1, { c: 2, d: 1 }] }));
  assert.notEqual(canonicalJson({ a: [1, 2] }), canonicalJson({ a: [2, 1] }));
});

test('each protocol rule fails on its own wrong answer, and every other rule is still judged', async () => {
  const base = await harnessDouble();
  const tools = () => base.client.listTools().then((listed) => listed.tools);
  const baseTools = await tools();
  const good = (await base.client.readResource({ uri: APP_RESOURCE_URI })).contents[0];
  const refuse = () => {
    throw new Error("Structured content does not match the tool's output schema");
  };
  const dependent = ['show_bound_sources', 'read_object_ranged_loop', 'read_object_digest', 'read_object_unknown_digest_refused', 'read_object_unknown_argument_refused'];
  const cases = [
    ['read_object is not listed', { tools: () => baseTools.filter((tool) => tool.name !== 'read_object') }, ['app_only_tools', 'read_object_declaration'], /lists no read_object/],
    ['a render tool names another resource', { tools: () => baseTools.map((tool) => (tool.name === 'render_source' ? { ...tool, _meta: { ui: { resourceUri: 'ui://other/app.html' } } } : tool)) }, ['render_tools'], /render_source resourceUri is ui:\/\/other\/app\.html/],
    ['a render tool is missing', { tools: () => baseTools.filter((tool) => tool.name !== 'render_timeline') }, ['render_tools'], /render tools are \["render_changes","render_present","render_source"\]/],
    ['a tool is neither a render tool nor app-only', { tools: () => [...baseTools, { name: 'extra', inputSchema: {} }] }, ['app_only_tools'], /not exactly one of render tool or app-only/],
    ['show is visible to the model', { tools: () => baseTools.map((tool) => (tool.name === 'show' ? { ...tool, _meta: { ui: { visibility: ['model', 'app'] } } } : tool)) }, ['app_only_tools'], /app-only tools are \["read_object"\]/],
    ['read_object has a hand-written input schema', { tools: () => baseTools.map((tool) => (tool.name === 'read_object' ? { ...tool, inputSchema: { type: 'object', additionalProperties: false } } : tool)) }, ['read_object_declaration'], /input_schema is not the product's/],
    ['read_object declares no output schema', { tools: () => baseTools.map((tool) => (tool.name === 'read_object' ? { ...tool, outputSchema: undefined } : tool)) }, ['read_object_declaration'], /output_schema is not the product's/],
    ['tools/list fails', { tools: () => { throw new Error('boom'); } }, ['render_tools', 'app_only_tools', 'read_object_declaration'], /tools\/list failed: boom/],
    ['two resources are listed', { resources: () => [{ uri: APP_RESOURCE_URI, name: 'app' }, { uri: APP_RESOURCE_URI, name: 'again' }] }, ['resource_listed'], /lists 2 resources/],
    ['another resource is listed', { resources: () => [{ uri: 'ui://other/app.html', name: 'app' }] }, ['resource_listed'], /uri is ui:\/\/other\/app\.html/],
    ['no resource is listed', { resources: () => [] }, ['resource_listed', 'resource_mime_type', 'resource_meta_ui', 'resource_is_built_bundle'], /lists 0 resources/],
    ['the resource has another mime type', { resource: () => ({ contents: [{ ...good, mimeType: 'text/html' }] }) }, ['resource_mime_type'], /mimeType is text\/html, not text\/html;profile=mcp-app/],
    ['the resource has no csp', { resource: () => ({ contents: [{ ...good, _meta: { ui: {} } }] }) }, ['resource_meta_ui'], /_meta\.ui\.csp is not an object/],
    ['the csp lacks a domain list', { resource: () => ({ contents: [{ ...good, _meta: { ui: { csp: { connectDomains: [] } } } }] }) }, ['resource_meta_ui'], /lacks a connectDomains or resourceDomains array/],
    ['the resource is not the built bundle', { resource: () => ({ contents: [{ ...good, text: `${DOUBLE_HTML} ` }] }) }, ['resource_is_built_bundle'], /the bundle manifest says/],
    ['a render tool has no text fallback', { render: (name) => ({ structuredContent: name === 'render_present' ? base.present : { fixture: name }, content: [] }) }, ['render_tool_results'], /render_source returned no text fallback/],
    ['a render tool is rejected by the client', { render: refuse }, ['render_tool_results', ...dependent], /render_source failed: Structured content does not match/],
    ['read_object returns everything in one block', { blockBytes: 1000 }, ['read_object_ranged_loop'], /in one block; the ranged loop was not exercised/],
    ['read_object labels a block with another offset', { block: (block) => ({ ...block, offset: '0' }) }, ['read_object_ranged_loop'], /changed identity or range at 64/],
    ['read_object labels a block with another object', { block: (block) => ({ ...block, sha256: 'f'.repeat(64) }) }, ['read_object_ranged_loop'], /changed identity or range at 0/],
    ['read_object makes no progress', { block: (block) => ({ ...block, data_base64: '', has_more: true }) }, ['read_object_ranged_loop', 'read_object_digest'], /made no progress/],
    ['read_object serves other bytes', { block: (block) => ({ ...block, data_base64: Buffer.from(Buffer.from(block.data_base64, 'base64').map((byte) => (byte === 0x34 ? 0x35 : byte))).toString('base64') }) }, ['read_object_digest'], /bytes read hash to [0-9a-f]{64}, not/],
    ['read_object states another total size', { block: (block) => ({ ...block, total_size: '999' }) }, ['read_object_digest'], /total_size is "999", 223 bytes were read/],
    ['read_object is rejected by the client', { readObject: refuse }, ['read_object_ranged_loop', 'read_object_digest', 'read_object_unknown_digest_refused', 'read_object_unknown_argument_refused'], /read_object for binding metrics failed: Structured content does not match/],
  ];
  for (const [label, change, failing, detail] of cases) {
    const { client, expected } = await harnessDouble(change);
    const judged = judgeProtocol(expected, await observeProtocol(client, { views: VIEWS }));
    assert.deepEqual(judged.criteria.map((criterion) => criterion.id), PROTOCOL_CRITERIA, label);
    const failed = judged.criteria.filter((criterion) => criterion.result === 'fail');
    assert.deepEqual(failed.map((criterion) => criterion.id).sort(), failing.map((id) => `protocol/${id}`).sort(), `${label}: ${JSON.stringify(failed)}`);
    assert.ok(judged.criteria.every((criterion) => ['pass', 'fail'].includes(criterion.result)), label);
    assert.ok(failed.some((criterion) => detail.test(criterion.detail)), `${label}: ${JSON.stringify(failed)}`);
  }

  // The two refusals are judged from what came back, not assumed.
  const { digest, bytes } = base;
  const served = (args) => ({
    structuredContent: { sha256: args.object, offset: '0', total_size: String(bytes.length), media_type: 'application/json', data_base64: bytes.subarray(0, 64).toString('base64'), has_more: true },
    content: [{ type: 'text', text: 'block' }],
  });
  for (const [label, accepts, failing] of [
    ['an unknown digest is served', (args) => args.object !== digest, 'read_object_unknown_digest_refused'],
    ['an unknown argument is served', (args) => 'not_a_product_argument' in args, 'read_object_unknown_argument_refused'],
  ]) {
    const honest = await harnessDouble();
    const { client, expected } = await harnessDouble({ readObject: (args) => (accepts(args) ? served(args) : honest.client.callTool({ name: 'read_object', arguments: args })) });
    const judged = judgeProtocol(expected, await observeProtocol(client, { views: VIEWS }));
    const failed = judged.criteria.filter((criterion) => criterion.result === 'fail');
    assert.deepEqual(failed.map((criterion) => criterion.id), [`protocol/${failing}`], label);
    assert.match(failed[0].detail, /was not answered with isError; .* was answered with structuredContent/, label);
  }

  // A loop that never ends is stopped and reported, not waited for.
  const endless = await harnessDouble({ block: (block) => ({ ...block, has_more: true, data_base64: 'QQ==' }) });
  const stopped = judgeProtocol(endless.expected, await observeProtocol(endless.client, { views: VIEWS, maxBlocks: 5 }));
  assert.match(stopped.criteria.find((criterion) => criterion.id === 'protocol/read_object_ranged_loop').detail, /did not finish in 5 calls/);
});

test('the reference host is named by the commit its tag resolved to, and a cache that is not what was fetched is refused', async () => {
  const tagObject = '1'.repeat(40);
  const commit = '2'.repeat(40);
  // An annotated tag lists the tag object and what it points at; the commit is the second.
  assert.equal(tagCommit(`${tagObject}\trefs/tags/v2.0.3\n${commit}\trefs/tags/v2.0.3^{}\n`, 'v2.0.3'), commit);
  assert.equal(tagCommit(`${commit}\trefs/tags/v2.0.3\r\n`, 'v2.0.3'), commit, 'a lightweight tag is the commit');
  assert.throws(() => tagCommit(`${commit}\trefs/tags/v2.0.4\n`, 'v2.0.3'), /did not list the tag v2\.0\.3/);
  assert.throws(() => tagCommit('', 'v2.0.3'), /did not list the tag/);
  assert.deepEqual(lsRemoteArgs(), ['ls-remote', 'https://github.com/modelcontextprotocol/ext-apps', 'refs/tags/v2.0.3', 'refs/tags/v2.0.3^{}']);
  assert.equal(rawUrl(commit, 'src/index.tsx'), `https://raw.githubusercontent.com/modelcontextprotocol/ext-apps/${commit}/examples/basic-host/src/index.tsx`);
  assert.ok(BASIC_HOST.files.includes('serve.ts') && BASIC_HOST.files.includes('package.json'));

  const files = Object.fromEntries(BASIC_HOST.files.map((file, index) => [file, index.toString(16).padStart(64, '0')]));
  const record = { repository: BASIC_HOST.repository, tag: BASIC_HOST.tag, commit, fetched_at: '2026-10-06T00:00:00.000Z', files };
  assert.deepEqual(sourceProblems(record, files), []);
  assert.deepEqual(sourceProblems(record, { ...files, 'serve.ts': 'f'.repeat(64) }), [`serve.ts is ${'f'.repeat(64)}, fetched as ${files['serve.ts']}`]);
  const { 'src/sandbox.ts': removed, ...withoutOne } = files;
  assert.deepEqual(sourceProblems(record, withoutOne), [`src/sandbox.ts is missing, fetched as ${removed}`]);
  assert.deepEqual(sourceProblems({ ...record, files: withoutOne }, files), ['src/sandbox.ts has no recorded sha256']);
  assert.deepEqual(sourceProblems({ ...record, tag: 'v2.0.2' }, files), ['fetched at tag v2.0.2, not v2.0.3']);
  assert.deepEqual(sourceProblems({ ...record, repository: 'https://example.invalid/fork' }, files), ['fetched from https://example.invalid/fork, not https://github.com/modelcontextprotocol/ext-apps']);
  assert.deepEqual(sourceProblems({ ...record, commit: 'v2.0.3' }, files), ['no commit recorded']);
  assert.deepEqual(sourceProblems(null, files), ['no record of what was fetched']);

  const serve = 'const DIRECTORY = join(__dirname, "dist");\n  res.sendFile(join(DIRECTORY, "sandbox.html"));\n';
  assert.equal(patchServe(serve), 'const DIRECTORY = join(__dirname, "dist");\n  res.sendFile("sandbox.html", { root: DIRECTORY });\n');
  assert.throws(() => patchServe('res.sendFile("sandbox.html", { root: DIRECTORY });'), /serve\.ts no longer contains/);
  assert.notEqual(PATCHED_SERVE, 'serve.ts', 'the fetched serve.ts is never overwritten, so its hash stays checkable');
  assert.ok(!BASIC_HOST.files.includes(PATCHED_SERVE) && !BASIC_HOST.files.includes(SOURCE_RECORD));

  assert.deepEqual(
    sourceRecord({ status: 'cached', record, hashes: files, packageJson: { name: '@modelcontextprotocol/ext-apps-basic-host', version: '2.0.3', private: true }, lockfileSha256: 'a'.repeat(64) }),
    {
      status: 'cached',
      repository: 'https://github.com/modelcontextprotocol/ext-apps',
      requested_tag: 'v2.0.3',
      commit,
      commit_resolved_at: '2026-10-06T00:00:00.000Z',
      files_verified: 14,
      files_expected: 14,
      package: { name: '@modelcontextprotocol/ext-apps-basic-host', version: '2.0.3' },
      lockfile_sha256: 'a'.repeat(64),
    },
  );

  // The orchestrator fetches by commit, checks the cache on every run and serves the patched copy.
  const run = (await mcpAppsSources())['run.mjs'];
  assert.match(run, /run\('git', lsRemoteArgs\(\),/);
  assert.match(run, /const commit = tagCommit\(listed\.stdout, BASIC_HOST\.tag\);/);
  assert.match(run, /fetch\(rawUrl\(commit, file\)\)/);
  assert.match(run, /const problems = sourceProblems\(record, hashes\);\s*if \(problems\.length\) \{\s*throw new Error\(/);
  assert.match(run, /spawnGroup\('bun', \[PATCHED_SERVE\], \{/);
  assert.doesNotMatch(run, /BASIC_HOST_TAG|raw\.githubusercontent\.com|'serve\.ts'\], \{/);
  // And the browser's own version is asked of the browser.
  assert.match(run, /version: browser\.version\(\),/);
  assert.match((await mcpAppsSources())['lib/qualify.mjs'], /browser: browser \? \{ name: 'chromium', version: browser\.version \} : null,/);
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
