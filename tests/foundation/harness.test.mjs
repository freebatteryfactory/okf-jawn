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
import { cpSync } from 'node:fs';
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { reviewCoversRevision, readResolvedRevision } from '../support/assertions.mjs';
import {
  mapReceiptPath,
  pathContext,
  receiptHeader,
  scrubReceiptPaths,
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
  HARNESS_EXIT,
  MUST_FAIL,
  TIMEOUT_PROBE,
  buildDoclingReceipt,
} from '../../qualification/docling/lib/receipt.mjs';
import { assetsMatched, inventoryCheck, parseManifest, unverifiedEnvPaths, verifyAssets } from '../../qualification/docling/lib/assets.mjs';
import { buildFacts, classifyRefusal, declaredDoclingFeatures, doclingPackages, resolvedFeatures } from '../../qualification/docling/lib/build.mjs';
import * as doclingCriteria from '../../qualification/docling/lib/criteria.mjs';
import * as doclingEnvelope from '../../scripts/lib/receipt-envelope.mjs';
import * as sharedEnvelope from '../../scripts/lib/receipt-envelope.mjs';
import {
  DISTANCE_STATEMENT,
  PAGE_RENDER_RULE,
  PROVENANCE_RULE,
  TEXT_LAYER_LIMITS,
  bboxProblem,
  bodyFacts,
  converterOptions,
  deriveLookups,
  describeDocument,
  judgePageRenders,
  judgeProvenance,
  lookupDifferences,
  pngSize,
  textLayerLines,
} from '../../qualification/docling/lib/document.mjs';
import { loadEvidence } from '../../qualification/docling/lib/evidence.mjs';
import { qualify as qualifyDocling } from '../../qualification/docling/lib/orchestrate.mjs';
import { distRows, onnxRuntimeRecord, ortBuildMessage, readOnnxRuntime } from '../../qualification/docling/lib/native.mjs';
import { MATCH_RULES, closestLine, collapse, judgeContent, rowHasCells, textTokens } from '../../qualification/docling/lib/expect.mjs';
import { OCR_FIXTURES, decodeFixture, encodePng, fixtureWords, renderLines } from '../../qualification/docling/lib/ocr-fixture.mjs';
import { pngInk } from '../../qualification/docling/lib/png.mjs';
import { deflateSync } from 'node:zlib';
import { recordRefusal } from '../../qualification/record.mjs';
import { run as runCommand } from '../../scripts/lib/process.mjs';
import { checkReceipts, derivedRecord, staleReceiptLines, writeDerivedRecord } from '../../scripts/lib/receipts.mjs';
import { afterAll } from 'bun:test';
import concurrently from './concurrent-test.mjs';
import { commit as fixtureCommit, copyRepo, eachCase, git as fixtureGit, sharedRepos } from './fixture-repo.mjs';
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
import { qualify as qualifyMcpApps } from '../../qualification/mcp-apps/lib/qualify.mjs';
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

// The join tests start from copies of repositories prepared once for this file.
const shared = sharedRepos();
afterAll(shared.dispose);
const fixtureRepo = shared.fixtureRepo;

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

test('doclingPackages names every docling crate of Cargo.lock with the source it is built from', async () => {
  const fork = 'git+https://github.com/Heyoub/docling.rs?rev=e500c2323bcf5c3c84d3731ed00de65b15d0709a#e500c2323bcf5c3c84d3731ed00de65b15d0709a';
  const lock = [
    '# This file is automatically @generated by Cargo.',
    'version = 4',
    '',
    '[[package]]',
    'name = "docling-pdf"',
    'version = "1.93.6"',
    `source = "${fork}"`,
    'dependencies = [',
    ' "docling-core",',
    ']',
    '',
    '[[package]]',
    'name = "docling"',
    'version = "1.93.5"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    'checksum = "abc123"',
    '',
    '[[package]]',
    'name = "doclingx"',
    'version = "0.1.0"',
    '',
    '[[package]]',
    'name = "okf-qualify-docling"',
    'version = "0.1.0"',
    '',
  ];
  for (const text of [lock.join('\n'), lock.join('\r\n')]) {
    assert.deepEqual(doclingPackages(text), [
      { name: 'docling', version: '1.93.5', source: 'registry+https://github.com/rust-lang/crates.io-index', checksum: 'abc123' },
      { name: 'docling-pdf', version: '1.93.6', source: fork, checksum: null },
    ], 'docling and docling-* only, sorted by name; a git source has no checksum');
  }
  // In this repository: docling from crates.io, its PDF crate and the two crates of the same tree from the fork at the commit Cargo.toml patches in.
  const rev = /^docling-pdf = \{ git = "https:\/\/github\.com\/Heyoub\/docling\.rs", rev = "([0-9a-f]{40})" \}$/m.exec(await readFile(join(root, 'Cargo.toml'), 'utf8'))?.[1];
  assert.ok(rev, 'Cargo.toml [patch.crates-io] does not take docling-pdf from the fork by rev');
  assert.deepEqual(doclingPackages(await readFile(join(root, 'Cargo.lock'), 'utf8')).map(({ name, version, source }) => [name, version, source]), [
    ['docling', '1.93.5', 'registry+https://github.com/rust-lang/crates.io-index'],
    ...['docling-core', 'docling-onnx', 'docling-pdf'].map((name) => [name, '1.93.6', `git+https://github.com/Heyoub/docling.rs?rev=${rev}#${rev}`]),
  ]);
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

  // A string that is not found is recorded beside what the output holds instead: its closest line, read as the matcher reads it.
  assert.deepEqual(failing({ markdown_contains: ['hello world'] }), [{ kind: 'markdown_contains', expected: 'hello world', case_insensitive: false, ok: false, closest: { line: 'Hello world', shared: 2, of: 2 } }]);
  assert.deepEqual(failing({ markdown_contains: ['Hello world', 'Goodbye'] }), [{ kind: 'markdown_contains', expected: 'Goodbye', case_insensitive: false, ok: false, closest: null }]);
  assert.ok(ok.checks.every((check) => !Object.hasOwn(check, 'closest')), 'a string that was found has nothing to show beside it');
  const page = '## Limits\n\n- /SM590000 Work  Function\tUsage ( WRKFCNUSG )\n- Work Function Usage is a view\n- unrelated\n';
  const phrase = 'Work Function Usage (WRKFCNUSG)';
  // The line that holds the most of the string's letter and digit words, with its whitespace collapsed as the matcher collapses it.
  assert.deepEqual(closestLine(page, phrase), { line: '- /SM590000 Work Function Usage ( WRKFCNUSG )', shared: 4, of: 4 });
  assert.deepEqual(closestLine('- unrelated\n- Work Function Usage is a view\n', phrase), { line: '- Work Function Usage is a view', shared: 3, of: 4 });
  // Half of the words is close; fewer is not, and then nothing is shown as if it were.
  assert.deepEqual(closestLine('a b\nWork Function\n', phrase), { line: 'Work Function', shared: 2, of: 4 });
  assert.equal(closestLine('Work on it\nUsage notes\n', phrase), null);
  assert.equal(closestLine('', phrase), null);
  assert.equal(closestLine('( )\n', '( )'), null, 'a string without a letter or digit has no word to look for');
  // Of several equally close lines the first is shown; a word counts once however often the line repeats it.
  assert.deepEqual(closestLine('Work Function x\nWork Function y\n', phrase), { line: 'Work Function x', shared: 2, of: 4 });
  assert.deepEqual(closestLine('work work work work\n', phrase), null);
  // A long line is cut, and the words are compared whatever their case.
  assert.equal(closestLine(`WORK FUNCTION USAGE WRKFCNUSG ${'x'.repeat(400)}\n`, phrase).line.length, 240);
  // It is an observation: the check it sits in is exactly as failed as before.
  assert.deepEqual(judgeContent({ markdown_contains: [phrase] }, { ...observed, markdown: page }).checks.map((check) => check.ok), [false]);
  assert.match(MATCH_RULES.markdown_contains, /a string that is not found is recorded with the closest line of the output/);
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
  const layerDocument = { texts: [{ label: 'text', text: 'Hello world', prov: [{ page_no: 1, bbox: { l: 72, t: 690, r: 300, b: 670, coord_origin: 'BOTTOMLEFT' } }] }] };
  const layer = Buffer.from(JSON.stringify(layerDocument));
  const written = {
    markdown_file: 'document.md',
    markdown_sha256: sha(markdown),
    json_file: 'document.json',
    json_sha256: sha(json),
    page_images: [{ ...PAGE_IMAGE, file: 'page-1.png' }],
    text_layer: { file: 'text_layer.json', sha256: sha(layer), bytes: layer.length },
  };
  const write = async (over = {}) => {
    const files = { 'document.md': markdown, 'document.json': json, 'page-1.png': PAGE_PNG, 'text_layer.json': layer, ...over };
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
  assert.deepEqual(good.textLayer, layerDocument, 'the text-layer document the lookups were made from is read from its file');
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

  // The text-layer document is held to its recorded hash like the others: changed, missing, or not JSON.
  await write({ 'text_layer.json': Buffer.from(JSON.stringify({ texts: [...layerDocument.texts, ...layerDocument.texts] })) });
  const relayered = await loadEvidence(dir, written);
  assert.deepEqual(relayered.problems, ['text_layer.json: hash on disk differs from the receipt']);
  await write({ 'text_layer.json': null });
  const unlayered = await loadEvidence(dir, written);
  assert.equal(unlayered.textLayer, null);
  assert.equal(unlayered.problems.length, 1);
  assert.match(unlayered.problems[0], /^text_layer\.json: .*ENOENT/);
  const notLayer = Buffer.from('[ not json');
  await write({ 'text_layer.json': notLayer });
  const brokenLayer = await loadEvidence(dir, { ...written, text_layer: { file: 'text_layer.json', sha256: sha(notLayer), bytes: notLayer.length } });
  assert.equal(brokenLayer.textLayer, null);
  assert.match(brokenLayer.problems.join(' | '), /^text_layer\.json: not JSON/);
  // A process that recorded no text-layer file (not a PDF, or a text layer the library could not read) has none to read.
  await write();
  const { text_layer: _none, ...withoutLayer } = written;
  for (const recorded of [withoutLayer, { ...written, text_layer: null }]) {
    const none = await loadEvidence(dir, recorded);
    assert.deepEqual([none.textLayer, none.problems], [null, []]);
  }

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
  assert.deepEqual(await loadEvidence(dir, null), { markdown: null, document: null, textLayer: null, pageFiles: {}, problems: [] });
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
  assert.deepEqual(good.text_items, { total: 2, with_provenance: 2, located: 2, located_by: { export: 2, text_layer: 0, none: 0 }, invalid_total: 0, invalid: [] });
  assert.deepEqual(good.tables, { total: 1, with_provenance: 1, located: 1, located_by: { export: 1, text_layer: 0, none: 0 }, invalid_total: 0, invalid: [] });
  assert.deepEqual(good.items, { total: 3, located: 3, located_by: { export: 3, text_layer: 0, none: 0 } });
  // Every item is listed once with the source that located it, its page and its box.
  assert.deepEqual(good.located_items.map((item) => [item.ref, item.kind, item.located_by, item.page_no]), [['#/texts/0', 'text', 'export', 1], ['#/texts/1', 'text', 'export', 1], ['#/tables/0', 'table', 'export', 1]]);
  assert.deepEqual(good.located_items[0].bbox, { coord_origin: 'BOTTOMLEFT', ...BOX });
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
  assert.deepEqual(lostTable.items, { total: 3, located: 2, located_by: { export: 2, text_layer: 0, none: 1 } });
  assert.deepEqual(lostTable.located_items.at(-1), { ref: '#/tables/0', kind: 'table', label: 'table', located_by: 'none', page_no: null, bbox: null, problem: 'no provenance' });
  const picture = (provValue) => ({ pictures: [{ self_ref: '#/pictures/0', label: 'picture', prov: provValue }] });
  assert.equal(judge(picture(prov(BOX))).status, 'PASS');
  assert.deepEqual(judge(picture(prov(BOX))).items, { total: 4, located: 4, located_by: { export: 4, text_layer: 0, none: 0 } });
  const lostPicture = judge(picture(prov({ l: 0, t: 0, r: 0, b: 0 })));
  assert.equal(lostPicture.status, 'FAIL');
  assert.deepEqual(lostPicture.pictures.invalid, [{ ref: '#/pictures/0', label: 'picture', problem: 'bbox has no area' }]);
  assert.match(good.rule, /every text item, table and picture is located: by its own provenance in the export or, for a PDF item the export gives none, by the one text-layer item/);
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
  assert.equal(office.located_items, undefined, 'a format that is not judged lists no located items');
});

/** What src/locate.rs records for an item the export left unlocated: found in the text layer, or not and why. */
const lookupFound = (item, bbox, page_no = 1, measured = {}) => ({ item, kind: 'text', label: 'caption', located_by: 'text_layer', page_no, bbox: { coord_origin: 'BOTTOMLEFT', ...bbox }, lookup: { basis: 'parent', occurrences: 1, pages: [page_no], reason: null, ...measured } });
const lookupMissed = (item, reason, occurrences = 0) => ({ item, kind: 'text', label: 'caption', located_by: 'none', page_no: null, bbox: null, lookup: { basis: 'parent', occurrences, pages: [1], reason } });

test('provenance: an item the export leaves unlocated is located by the text-layer lookup, held to the same rule for its page and box', () => {
  const caption = { self_ref: '#/texts/2', label: 'caption', text: 'Table 2-1   FUNCTION_USAGE view', prov: [] };
  const document = describeDocument(stubDocument({ texts: [...stubDocument().texts, caption] }));
  const judge = (records) => judgeProvenance(document, { paginated: true, lookups: records === null ? null : new Map(records.map((record) => [record.item, record])) });
  const captionBox = { l: 136.27, t: 512.02, r: 284.48, b: 504.28 };

  // Without a lookup the caption is what it was: not located.
  assert.equal(judge(null).status, 'FAIL');
  assert.deepEqual(judge(null).text_items.invalid, [{ ref: '#/texts/2', label: 'caption', problem: 'no provenance' }]);

  const found = judge([lookupFound('#/texts/2', captionBox)]);
  assert.equal(found.status, 'PASS');
  assert.deepEqual(found.items, { total: 4, located: 4, located_by: { export: 3, text_layer: 1, none: 0 } });
  assert.deepEqual(found.text_items.located_by, { export: 2, text_layer: 1, none: 0 });
  assert.equal(found.text_items.with_provenance, 2, 'the export itself still gives the caption no provenance');
  assert.deepEqual(found.located_items[2], { ref: '#/texts/2', kind: 'text', label: 'caption', located_by: 'text_layer', page_no: 1, bbox: { coord_origin: 'BOTTOMLEFT', ...captionBox }, problem: null, lookup: { basis: 'parent', occurrences: 1, pages: [1], reason: null } });
  assert.deepEqual(found.page_provenance, [{ page_no: 1, text_items: 3, tables: 1, pictures: 0 }], 'the page counts the item the text layer put on it');

  // The text occurs twice on the page, or not at all: the lookup found nothing, and the item says why.
  const twice = judge([lookupMissed('#/texts/2', '2 text-layer items on page [1] have exactly this text', 2)]);
  assert.equal(twice.status, 'FAIL');
  assert.deepEqual(twice.text_items.invalid, [{ ref: '#/texts/2', label: 'caption', problem: 'no provenance; text layer: 2 text-layer items on page [1] have exactly this text' }]);
  assert.deepEqual(twice.items.located_by, { export: 3, text_layer: 0, none: 1 });
  assert.equal(twice.located_items[2].lookup.occurrences, 2);
  const absent = judge([lookupMissed('#/texts/2', 'no text-layer item on page [1] has exactly this text')]);
  assert.equal(absent.text_items.invalid[0].problem, 'no provenance; text layer: no text-layer item on page [1] has exactly this text');

  // A page or a box the text layer gave is judged like an exported one: inside the page, with area, on a page of the document.
  const problem = (record) => {
    const judged = judge([record]);
    assert.equal(judged.status, 'FAIL');
    assert.deepEqual(judged.items.located_by, { export: 3, text_layer: 0, none: 1 });
    assert.deepEqual([judged.located_items[2].located_by, judged.located_items[2].page_no, judged.located_items[2].bbox], ['none', null, null]);
    return judged.text_items.invalid[0].problem;
  };
  assert.equal(problem(lookupFound('#/texts/2', { ...captionBox, r: 700 })), 'no provenance; text layer: bbox outside the 612 x 792 page');
  assert.equal(problem(lookupFound('#/texts/2', { ...captionBox, b: 512.02 })), 'no provenance; text layer: bbox has no area');
  assert.equal(problem(lookupFound('#/texts/2', { ...captionBox, r: 136.27 })), 'no provenance; text layer: bbox has no area');
  assert.equal(problem(lookupFound('#/texts/2', captionBox, 2)), 'no provenance; text layer: page_no 2 is not a page of the document (1..=1)');
  assert.equal(problem({ ...lookupFound('#/texts/2', captionBox), bbox: null }), 'no provenance; text layer: no bbox');

  // The export decides for an item that has provenance: a lookup record cannot rescue a bad exported box.
  const badExport = describeDocument(stubDocument({ texts: [{ self_ref: '#/texts/0', label: 'text', text: 'x', prov: prov({ l: 0, t: 0, r: 0, b: 0 }) }] }));
  const rescued = judgeProvenance(badExport, { paginated: true, lookups: new Map([['#/texts/0', lookupFound('#/texts/0', captionBox)]]) });
  assert.equal(rescued.status, 'FAIL');
  assert.deepEqual(rescued.text_items.invalid, [{ ref: '#/texts/0', label: 'text', problem: 'bbox has no area' }]);
});

test('the text-layer rule says what it does not guarantee, and each item it locates records its distance as a measurement with no threshold', async () => {
  // One sentence in two places: the binary states it with its locations, the receipt with its rule.
  const locate = await readFile(join(root, 'qualification/docling/src/locate.rs'), 'utf8');
  assert.ok(locate.includes(`pub(crate) const LOCATE_LIMITS: &str = "${TEXT_LAYER_LIMITS}";`), 'src/locate.rs LOCATE_LIMITS is not the sentence of lib/document.mjs TEXT_LAYER_LIMITS');
  assert.match(locate, /^ {8}limits: LOCATE_LIMITS,$/m, 'the locations the binary writes carry the limits');
  // The two cases the harness review placed wrongly, and that nothing holds the box to the parent's.
  for (const said of [
    "does not guarantee that the box is the item's own",
    'An item under the body that starts a page is searched on the page of its earlier sibling, so the same words standing once on that earlier page (a running footer) are taken for it',
    "when the page sets the item's text differently (a caption continued as 'Figure 1 (continued)') while another line of that page is exactly the item's text, that line is taken",
    'Nothing holds the box found to the box of the parent',
    'as a measurement, and no threshold is applied',
  ]) assert.ok(TEXT_LAYER_LIMITS.includes(said), said);
  assert.ok(PROVENANCE_RULE.endsWith(`. ${TEXT_LAYER_LIMITS}`));
  assert.equal(doclingReceipt().criterion_rules.provenance, PROVENANCE_RULE, 'the receipt carries the limits with the rule');
  assert.match(locate, /^ {4}pub\(crate\) distance: Option<f64>,$/m);
  assert.match(locate, /^ {4}pub\(crate\) reference: Option<String>,$/m);

  // Four captions the export left unlocated. The lookup measured three of them; the reference of the fourth has no box on the page.
  const captions = ['Figure 1', 'Table 2', 'Example 3', 'Figure 4'].map((text, index) => ({ self_ref: `#/texts/${index + 2}`, label: 'caption', text, prov: [] }));
  const document = stubDocument({ texts: [...stubDocument().texts, ...captions] });
  const box = (index) => ({ l: 136.27, t: 500 - 20 * index, r: 284.48, b: 492 - 20 * index });
  const found = [
    lookupFound('#/texts/2', box(0), 1, { distance: 4.6, reference: '#/pictures/0' }),
    lookupFound('#/texts/3', box(1), 1, { distance: 1.5, reference: '#/tables/0' }),
    lookupFound('#/texts/4', box(2), 1, { basis: 'earlier_sibling', distance: 9.28, reference: '#/texts/1' }),
    lookupFound('#/texts/5', box(3), 1, { distance: null, reference: '#/pictures/1' }),
  ];
  const judged = judgeProvenance(describeDocument(document), { paginated: true, lookups: new Map(found.map((record) => [record.item, record])) });
  assert.equal(judged.status, 'PASS');
  assert.deepEqual(judged.text_layer_distances, {
    statement: DISTANCE_STATEMENT,
    threshold_applied: false,
    measured: 3,
    unmeasured: 1,
    min: 1.5,
    max: 9.28,
    items: [
      { ref: '#/texts/2', basis: 'parent', reference: '#/pictures/0', distance: 4.6 },
      { ref: '#/texts/3', basis: 'parent', reference: '#/tables/0', distance: 1.5 },
      { ref: '#/texts/4', basis: 'earlier_sibling', reference: '#/texts/1', distance: 9.28 },
      { ref: '#/texts/5', basis: 'parent', reference: '#/pictures/1', distance: null },
    ],
  });
  assert.match(DISTANCE_STATEMENT, /A measurement: no threshold is applied and no item passes or fails on it$/);
  // Each located item keeps what the lookup recorded, the distance with it.
  assert.deepEqual(judged.located_items.find((item) => item.ref === '#/texts/4').lookup, { basis: 'earlier_sibling', occurrences: 1, pages: [1], reason: null, distance: 9.28, reference: '#/texts/1' });

  // The receipt states the range with the three counts, and says that it is not a rule.
  const withLocations = (records) => doclingReceipt({ 'born_digital_text.pdf': lookedUp(document, records) });
  const zero = '(0 text items, 0 tables, 0 pictures)';
  const receipt = withLocations(found);
  assert.equal(receipt.result, 'PASS');
  assert.equal(
    doclingCriterion(receipt, 'born_digital_text.pdf/provenance').detail,
    `7 items: 3 located by the export (2 text items, 1 tables, 0 pictures), 4 through the text layer (4 text items, 0 tables, 0 pictures), 0 not located ${zero}; the 3 found through the text layer lie 1.5 to 9.28 page units from the item whose page was searched (a measurement, no threshold, 1 more not measured)`,
  );
  assert.deepEqual(entryOf(receipt, 'born_digital_text.pdf').criteria.provenance.text_layer_distances, judged.text_layer_distances);

  // No threshold: the item the review placed wrongly lies 654.55 units from the sibling whose page was searched, and is located all the same.
  const far = withLocations([...found.slice(0, 3), lookupFound('#/texts/5', box(3), 1, { basis: 'earlier_sibling', distance: 654.55, reference: '#/texts/0' })]);
  assert.equal(far.result, 'PASS');
  assert.match(doclingCriterion(far, 'born_digital_text.pdf/provenance').detail, /; the 4 found through the text layer lie 1\.5 to 654\.55 page units from the item whose page was searched \(a measurement, no threshold\)$/);

  // Nothing found through the text layer: nothing was measured, and the detail says only the counts.
  const plain = entryOf(doclingReceipt(), 'born_digital_text.pdf').criteria.provenance;
  assert.deepEqual(plain.text_layer_distances, { statement: DISTANCE_STATEMENT, threshold_applied: false, measured: 0, unmeasured: 0, min: null, max: null, items: [] });
  assert.doesNotMatch(plain.detail, /page units/);
  // An image has no text layer to look anything up in.
  assert.equal(entryOf(doclingReceipt(), 'text_image.png').criteria.provenance.text_layer_distances, undefined);
});

/**
 * The converted document of the Rust tests of src/locate.rs: on page 2 a picture and a table,
 * each with a caption the export left unlocated; on page 3 a code block followed by a caption
 * that hangs under the body.
 */
function locateExport() {
  return {
    pages: { 2: { page_no: 2, size: { width: 612, height: 792 } }, 3: { page_no: 3, size: { width: 612, height: 792 } } },
    body: { self_ref: '#/body', children: ['#/texts/0', '#/pictures/0', '#/tables/0', '#/texts/3', '#/texts/4'].map(($ref) => ({ $ref })) },
    texts: [
      { self_ref: '#/texts/0', label: 'text', text: 'A paragraph.', parent: { $ref: '#/body' }, prov: prov({ l: 72, t: 700, r: 300, b: 690 }, 2) },
      { self_ref: '#/texts/1', label: 'caption', text: 'Figure 1-2   Existing controls', parent: { $ref: '#/pictures/0' }, prov: [] },
      { self_ref: '#/texts/2', label: 'caption', text: 'Table 2-1   FUNCTION_USAGE view', parent: { $ref: '#/tables/0' }, prov: [] },
      { self_ref: '#/texts/3', label: 'code', text: 'CREATE MASK', parent: { $ref: '#/body' }, prov: prov({ l: 72, t: 500, r: 400, b: 420 }, 3) },
      { self_ref: '#/texts/4', label: 'text', text: 'Example 3-9   Creating a mask', parent: { $ref: '#/body' }, prov: [] },
    ],
    tables: [{ self_ref: '#/tables/0', label: 'table', parent: { $ref: '#/body' }, prov: prov({ l: 72, t: 400, r: 500, b: 300 }, 2), data: { table_cells: [] } }],
    pictures: [{ self_ref: '#/pictures/0', label: 'picture', parent: { $ref: '#/body' }, prov: prov({ l: 72, t: 650, r: 500, b: 450 }, 2) }],
  };
}
const layerLine = (page_no, text, bbox) => ({ label: 'text', text, prov: [{ page_no, bbox: { coord_origin: 'BOTTOMLEFT', ...bbox } }] });
const FIGURE_BOX = { l: 136.27, t: 100.55, r: 316.76, b: 91.27 };
const TABLE_BOX = { l: 136.27, t: 512.02, r: 284.48, b: 504.28 };
const EXAMPLE_BOX = { l: 136.27, t: 385.17, r: 351.42, b: 377.44 };
/** The text layer of that document: each caption once, on its own page. */
function locateTextLayer() {
  return {
    texts: [
      layerLine(2, 'A paragraph.', FIGURE_BOX),
      layerLine(2, 'Figure 1-2   Existing controls', FIGURE_BOX),
      layerLine(2, 'Table 2-1   FUNCTION_USAGE view', TABLE_BOX),
      layerLine(3, 'Example 3-9   Creating a mask', EXAMPLE_BOX),
    ],
  };
}

test('the judge makes every text-layer lookup again from the two files the process wrote, by the rule of src/locate.rs', () => {
  const origin = (box) => ({ coord_origin: 'BOTTOMLEFT', ...box });
  const derived = deriveLookups(locateExport(), locateTextLayer());
  // Only the text items the export left unlocated are looked up; the result is what the Rust tests hold.
  assert.deepEqual([...derived.keys()], ['#/texts/1', '#/texts/2', '#/texts/4']);
  assert.deepEqual(derived.get('#/texts/1'), { basis: 'parent', pages: [2], occurrences: 1, page_no: 2, bbox: origin(FIGURE_BOX) });
  assert.deepEqual(derived.get('#/texts/2'), { basis: 'parent', pages: [2], occurrences: 1, page_no: 2, bbox: origin(TABLE_BOX) });
  assert.deepEqual(derived.get('#/texts/4'), { basis: 'earlier_sibling', pages: [3], occurrences: 1, page_no: 3, bbox: origin(EXAMPLE_BOX) });
  assert.equal(textLayerLines(locateTextLayer()).length, 4);

  const figure = (layer, document = locateExport()) => deriveLookups(document, layer).get('#/texts/1');
  const withLines = (...lines) => ({ texts: [...locateTextLayer().texts, ...lines] });
  const unlocated = (occurrences) => ({ basis: 'parent', pages: [2], occurrences, page_no: null, bbox: null });
  // Twice on the page: not located. The same text on another page is not a second occurrence.
  assert.deepEqual(figure(withLines(layerLine(2, 'Figure 1-2   Existing controls', { l: 72, t: 60, r: 200, b: 50 }))), unlocated(2));
  assert.equal(figure(withLines(layerLine(3, 'Figure 1-2   Existing controls', FIGURE_BOX))).page_no, 2);
  // Absent, only on another page, only as part of a longer line, or spaced differently: not located.
  for (const layer of [
    { texts: [] },
    { texts: [layerLine(3, 'Figure 1-2   Existing controls', FIGURE_BOX)] },
    { texts: [layerLine(2, 'Figure 1-2   Existing controls and more', FIGURE_BOX), layerLine(2, 'Figure 1-2', FIGURE_BOX)] },
    { texts: [layerLine(2, 'Figure 1-2 Existing controls', FIGURE_BOX)] },
    {},
    null,
  ]) assert.deepEqual(figure(layer), unlocated(0), JSON.stringify(layer));
  // The one match must be a region of the page: inside it, with area, with a known origin.
  for (const bbox of [{ l: 136, t: 900, r: 316, b: 880 }, { l: 136, t: 100, r: 136, b: 90 }, { l: 136, t: 90, r: 316, b: 100 }, { l: 136, t: 100, r: 316 }, { l: 136, t: 100, r: 316, b: 90, coord_origin: 'CENTRE' }]) {
    assert.deepEqual(figure({ texts: [layerLine(2, 'Figure 1-2   Existing controls', bbox)] }), unlocated(1), JSON.stringify(bbox));
  }
  // A line without a location, or without text, is no line of the text layer.
  assert.deepEqual(figure({ texts: [{ label: 'text', text: 'Figure 1-2   Existing controls', prov: [] }, { label: 'text', prov: [{ page_no: 2, bbox: origin(FIGURE_BOX) }] }] }), unlocated(0));
  // An item whose parent is not located and that has no located earlier sibling has no page to search.
  const first = locateExport();
  first.body.children.unshift(first.body.children.pop());
  assert.deepEqual(deriveLookups(first, locateTextLayer()).get('#/texts/4'), { basis: 'none', pages: [], occurrences: 0, page_no: null, bbox: null });
  const orphan = locateExport();
  orphan.pictures[0].prov = [];
  assert.deepEqual(figure(locateTextLayer(), orphan), { basis: 'none', pages: [], occurrences: 0, page_no: null, bbox: null });
  // An item without text has nothing to look up; the earlier sibling's LAST page is the one searched.
  const blank = locateExport();
  blank.texts[1].text = '  ';
  assert.deepEqual(figure(locateTextLayer(), blank), unlocated(0));
  const spanning = locateExport();
  spanning.texts[3].prov = [...prov({ l: 72, t: 100, r: 400, b: 50 }, 2), ...prov({ l: 72, t: 500, r: 400, b: 420 }, 3)];
  assert.deepEqual(deriveLookups(spanning, locateTextLayer()).get('#/texts/4').pages, [3]);

  // What the process recorded is held to that result, field by field.
  const record = (ref, lookup = derived.get(ref)) => ({
    item: ref,
    kind: 'text',
    label: 'caption',
    located_by: lookup.page_no === null ? 'none' : 'text_layer',
    page_no: lookup.page_no,
    bbox: lookup.bbox,
    lookup: { basis: lookup.basis, occurrences: lookup.occurrences, pages: lookup.pages, reason: null, distance: 1.5, reference: '#/pictures/0' },
  });
  const recorded = (change = {}) => new Map(['#/texts/1', '#/texts/2', '#/texts/4'].map((ref) => [ref, { ...record(ref), ...(ref === '#/texts/1' ? change : {}) }]));
  assert.deepEqual(lookupDifferences(recorded(), derived), [], 'the record that is the lookup made again differs nowhere; the distance and the reason are not compared');
  const differs = (change) => lookupDifferences(recorded(change), derived);
  const figureBox = JSON.stringify(origin(FIGURE_BOX));
  assert.deepEqual(differs({ page_no: 3 }), [`#/texts/1: page 3 and box ${figureBox}, the files give page 2 and box ${figureBox}`]);
  assert.deepEqual(differs({ bbox: origin({ ...FIGURE_BOX, t: 101 }) }), [`#/texts/1: page 2 and box ${JSON.stringify(origin({ ...FIGURE_BOX, t: 101 }))}, the files give page 2 and box ${figureBox}`]);
  assert.deepEqual(differs({ located_by: 'none', page_no: null, bbox: null }), ['#/texts/1: recorded as not located, and the files locate it']);
  assert.deepEqual(differs({ lookup: { ...record('#/texts/1').lookup, basis: 'earlier_sibling' } }), ['#/texts/1: basis "earlier_sibling", the files give "parent"']);
  assert.deepEqual(differs({ lookup: { ...record('#/texts/1').lookup, pages: [2, 3] } }), ['#/texts/1: pages [2,3], the files give [2]']);
  assert.deepEqual(differs({ lookup: { ...record('#/texts/1').lookup, occurrences: 2 } }), ['#/texts/1: 2 line(s) with its text, the files give 1']);
  assert.deepEqual(differs({ lookup: null }), ['#/texts/1: no lookup is recorded for an item the export left unlocated']);
  // Recorded as located where the files find the text twice.
  const twice = deriveLookups(locateExport(), withLines(layerLine(2, 'Figure 1-2   Existing controls', { l: 72, t: 60, r: 200, b: 50 })));
  assert.deepEqual(lookupDifferences(recorded(), twice), ['#/texts/1: 1 line(s) with its text, the files give 2; recorded as located, and the files do not locate it']);
  // An item the export located has no lookup, whatever a record claims for it.
  const claimed = recorded();
  claimed.set('#/texts/0', { ...record('#/texts/1'), item: '#/texts/0' });
  assert.deepEqual(lookupDifferences(claimed, derived), ['#/texts/0: recorded as located through the text layer, and the export locates it itself or it is no text item']);
});

test('a lookup record that is not what the files give is a harness error, never a location; the text-layer document is evidence like the rest', async () => {
  const captions = [
    { self_ref: '#/texts/2', label: 'caption', text: 'Table 2-1   FUNCTION_USAGE view', prov: [] },
    { self_ref: '#/texts/3', label: 'caption', text: 'Figure 1-2   Existing row and column controls', prov: [] },
  ];
  const document = stubDocument({ texts: [...stubDocument().texts, ...captions] });
  const found = [lookupFound('#/texts/2', TABLE_BOX), lookupFound('#/texts/3', FIGURE_BOX)];
  const only = 'born_digital_text.pdf';
  const run = (change = (parts) => parts) => doclingReceipt({ [only]: change(lookedUp(document, found)) });

  // Made again from document.json and text_layer.json, the two lookups are the records: located, and the receipt says what was checked.
  const good = run();
  assert.equal(good.result, 'PASS');
  const provenance = entryOf(good, only).criteria.provenance;
  assert.deepEqual(provenance.text_layer_lookups, { lookups_made_again: 2, text_layer_items: 2, text_layer_file: 'text_layer.json' });
  assert.deepEqual(provenance.items.located_by, { export: 3, text_layer: 2, none: 0 });
  assert.deepEqual(entryOf(good, only).document.text_layer, { file: 'text_layer.json', sha256: 'a'.repeat(64), bytes: 12 }, 'the entry keeps the file and the hash the process recorded');
  // A PDF with nothing to look up needs no lookup made again; an image has no text layer at all.
  assert.equal(entryOf(doclingReceipt(), only).criteria.provenance.text_layer_lookups, undefined);
  assert.equal(entryOf(good, 'text_image.png').criteria.provenance.text_layer_lookups, undefined);

  // Each way the record can differ from the files stops the judgement: a harness error, nothing located, nothing failed.
  const stoppedBy = (change, sentence) => {
    const receipt = run(change);
    assert.equal(receipt.harness_error, sentence);
    assert.equal(receipt.result, 'INCOMPLETE', sentence);
    assert.deepEqual(doclingCriterion(receipt, `${only}/provenance`), { id: `${only}/provenance`, required: true, result: 'not_judged', detail: sentence });
    assert.deepEqual(doclingFailedIds(receipt), [], 'the binary contradicting its own evidence says nothing about the library');
    assertDoclingEnvelope(receipt);
  };
  const differ = (count, list) => `${count} text-layer lookup(s) the converter process for ${only} recorded are not what its document.json and text_layer.json give; all: ${list}`;
  const withLayer = (texts) => (parts) => ({ evidence: { ...parts.evidence, textLayer: { texts: texts(parts.evidence.textLayer.texts) } }, fixture: { locations: { ...parts.fixture.locations, text_layer: { ...parts.fixture.locations.text_layer, items: texts(parts.evidence.textLayer.texts).length } } } });
  const withRecord = (ref, change) => (parts) => ({ ...parts, fixture: { locations: { ...parts.fixture.locations, items: parts.fixture.locations.items.map((entry) => (entry.item === ref ? { ...entry, ...change(entry) } : entry)) } } });
  // The text stands twice in the text layer the process wrote, and the record says it was found once.
  stoppedBy(withLayer((lines) => [...lines, layerLine(1, captions[0].text, { l: 72, t: 60, r: 200, b: 50 })]), differ(1, '#/texts/2: 1 line(s) with its text, the files give 2; recorded as located, and the files do not locate it'));
  // The text is not in the text layer at all.
  stoppedBy(withLayer((lines) => lines.slice(1)), differ(1, '#/texts/2: 1 line(s) with its text, the files give 0; recorded as located, and the files do not locate it'));
  // The record gives another box, or another page, than the line of the file.
  const box = (value) => JSON.stringify({ coord_origin: 'BOTTOMLEFT', ...value });
  stoppedBy(withRecord('#/texts/3', (entry) => ({ bbox: { ...entry.bbox, l: 72 } })), differ(1, `#/texts/3: page 1 and box ${box({ ...FIGURE_BOX, l: 72 })}, the files give page 1 and box ${box(FIGURE_BOX)}`));
  stoppedBy(withRecord('#/texts/3', () => ({ page_no: 2 })), differ(1, `#/texts/3: page 2 and box ${box(FIGURE_BOX)}, the files give page 1 and box ${box(FIGURE_BOX)}`));
  // The record says the item was not found, and the files find it.
  stoppedBy(withRecord('#/texts/3', (entry) => ({ located_by: 'none', page_no: null, bbox: null, lookup: { ...entry.lookup, occurrences: 0, reason: 'no text-layer item on page [1] has exactly this text' } })), differ(1, '#/texts/3: 0 line(s) with its text, the files give 1; recorded as not located, and the files locate it'));
  // Both records wrong: both are named.
  stoppedBy(withLayer(() => []), differ(2, '#/texts/2: 1 line(s) with its text, the files give 0; recorded as located, and the files do not locate it; #/texts/3: 1 line(s) with its text, the files give 0; recorded as located, and the files do not locate it'));
  // The count of text-layer items the process recorded is not the count in its file.
  stoppedBy((parts) => ({ ...parts, fixture: { locations: { ...parts.fixture.locations, text_layer: { ...parts.fixture.locations.text_layer, items: 549 } } } }), `the converter process for ${only} recorded 549 located text-layer item(s) and its text_layer.json holds 2`);
  // No text-layer document was written for lookups that were recorded.
  stoppedBy((parts) => ({ ...parts, evidence: { ...parts.evidence, textLayer: null } }), `the converter process for ${only} wrote no text-layer document (text_layer.json) for the 2 item(s) it looked up, so its lookups cannot be made again`);
  // The process says the library could not read the text layer, and records items as found in it.
  const unread = (parts) => ({ evidence: { ...parts.evidence, textLayer: null }, fixture: { locations: { ...parts.fixture.locations, text_layer: { error: 'pdf: unreadable', items: 0, source: 'docling::pdf_text_layer_pages' } } } });
  stoppedBy(unread, `the converter process for ${only} recorded 2 item(s) as located through a text layer it says it could not read (pdf: unreadable)`);

  // A text layer the library could not read, honestly recorded: nothing is found through it, and that is the library's result.
  const honest = doclingReceipt({ [only]: { evidence: { document, textLayer: null }, fixture: { locations: { ...stubLocations(document), text_layer: { error: 'pdf: unreadable', items: 0, source: 'docling::pdf_text_layer_pages' } } } } });
  assert.deepEqual([honest.result, honest.harness_error], ['FAIL', null]);
  assert.deepEqual(doclingFailedIds(honest), [`${only}/provenance`]);
  // A text-layer file whose bytes are not the recorded ones stops everything judged from the files, as any evidence file does.
  const changed = doclingReceipt({ [only]: { ...lookedUp(document, found), evidence: { ...lookedUp(document, found).evidence, problems: ['text_layer.json: hash on disk differs from the receipt'] } } });
  assert.equal(changed.result, 'INCOMPLETE');
  assert.match(changed.harness_error, /^1 file\(s\) the converter process for born_digital_text\.pdf wrote are not the ones it recorded; all: text_layer\.json: hash on disk differs from the receipt$/);
  assert.equal(doclingCriterion(changed, `${only}/provenance`).result, 'not_judged');

  // The binary writes the file it read the lookups from and records its hash; the orchestrator re-reads it with the rest.
  const main = await readFile(join(root, 'qualification/docling/src/main.rs'), 'utf8');
  assert.match(main, /^const TEXT_LAYER_JSON: &str = "text_layer\.json";$/m);
  assert.match(main, /Ok\(document\) => Some\(write_text_layer\(out_dir, document\)\?\),/);
  assert.match(main, /let locations = locate::locate_items\(export, text_layer\.as_ref\(\)\.map_err\(String::as_str\)\);/, 'the lookups are made from the document that was written');
  assert.match(main, /document\.text_layer = text_layer;/);
  const orchestrate = await readFile(join(root, 'qualification/docling/lib/orchestrate.mjs'), 'utf8');
  assert.match(orchestrate, /evidence: await io\.loadEvidence\(fixtureOut, written\),/);
  assert.match(good.criterion_rules.evidence, /for a PDF, the text-layer document the converter process wrote are on disk with the hashes it recorded$/);
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

/** A reason src/locate.rs gives for an item it could not look up. */
const NO_PAGE_TO_SEARCH = 'no page to search: neither its parent nor an earlier sibling is located';

/**
 * What src/locate.rs records for a converted PDF whose text layer locates nothing more: each
 * item is located by its own provenance or by nothing. `found` replaces the entries of items
 * the text layer did locate.
 */
function stubLocations(document, found = []) {
  const entries = [['texts', 'text'], ['tables', 'table'], ['pictures', 'picture']].flatMap(([list, kind]) =>
    (document?.[list] ?? []).map((item) => {
      const located = (item.prov ?? []).length > 0;
      return {
        bbox: located ? item.prov[0].bbox : null,
        item: item.self_ref,
        kind,
        label: item.label,
        located_by: located ? 'export' : 'none',
        lookup: located || kind !== 'text' ? null : { basis: 'none', occurrences: 0, pages: [], reason: NO_PAGE_TO_SEARCH },
        page_no: located ? item.prov[0].page_no : null,
      };
    }),
  );
  return {
    items: entries.map((entry) => found.find((record) => record.item === entry.item) ?? entry),
    rule: 'stub',
    text_layer: { error: null, items: found.length, source: 'docling::pdf_text_layer_pages' },
  };
}

/**
 * What a converter process would have written and recorded for these lookup records, as the
 * parts of a doclingRun: the export in which each looked-up item hangs under the table (or,
 * for an earlier-sibling record, under the body after the located texts), the text-layer
 * document that holds its text on the searched page once per recorded occurrence, and the
 * locations. Made again from the two files by lib/document.mjs, the lookups are the records.
 */
function lookedUp(document, records) {
  const recordOf = (item) => records.find((entry) => entry.item === item.self_ref);
  const texts = document.texts.map((item) => (recordOf(item) ? { ...item, parent: { $ref: recordOf(item).lookup.basis === 'earlier_sibling' ? '#/body' : '#/tables/0' } } : item));
  const written = { ...document, texts, body: { self_ref: '#/body', children: texts.map((item) => ({ $ref: item.self_ref })) } };
  const elsewhere = { coord_origin: 'BOTTOMLEFT', l: 72, t: 60, r: 200, b: 50 };
  const lines = records.flatMap((entry) =>
    Array.from({ length: entry.lookup.occurrences }, (_, index) => ({
      label: 'text',
      text: texts.find((item) => item.self_ref === entry.item).text,
      prov: [{ page_no: entry.lookup.pages[0], bbox: index === 0 && entry.bbox ? entry.bbox : elsewhere }],
    })),
  );
  const locations = stubLocations(written, records);
  return { evidence: { document: written, textLayer: { texts: lines } }, fixture: { locations: { ...locations, text_layer: { ...locations.text_layer, items: lines.length } } } };
}

/** What src/glyphs.rs records for a converted PDF: placeholders per page (`pages`), none by default. */
function stubGlyphs(pages = {}, unlocated_tokens = 0) {
  const tokens = Object.values(pages).reduce((sum, count) => sum + count, 0) + unlocated_tokens;
  return { items: [], pages, rule: 'stub', tokens, unlocated_tokens };
}

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
  // A converted PDF also carries where its items are, for the document the process wrote, and
  // the text-layer document it read: here one that holds no line, so nothing is found through it.
  if (fixture.input_format === 'pdf' && only !== MUST_FAIL) {
    fixture.locations = stubLocations(evidence.document);
    fixture.undecoded_glyphs = stubGlyphs();
    if (fixture.document) fixture.document = { ...fixture.document, text_layer: { file: 'text_layer.json', sha256: 'a'.repeat(64), bytes: 12 } };
    if (evidence.textLayer === undefined) evidence.textLayer = { texts: [] };
  }
  Object.assign(fixture, over.fixture);
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
  assert.deepEqual(Object.keys(receipt.criteria_summary), ['conversion', 'evidence', 'format_recognised', 'content', 'page_renders', 'provenance', 'memory_measured', 'undecoded_glyphs_reported', 'refused', 'refusal_attributed', 'timeout_reported', 'budget_had_effect']);
  const pdf = entryOf(receipt, 'born_digital_text.pdf');
  assert.deepEqual(Object.keys(pdf.criteria), ['conversion', 'evidence', 'format_recognised', 'content', 'page_renders', 'provenance', 'undecoded_glyphs_reported', 'memory_measured']);
  assert.deepEqual(Object.keys(entryOf(receipt, 'text_image.png').criteria), ['conversion', 'evidence', 'format_recognised', 'content', 'page_renders', 'provenance', 'memory_measured'], 'an image has no text layer and no glyph criterion');
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
    INCOMPLETE: doclingReceipt({ 'table_heavy.pdf': { report: null, run: { exitCode: HARNESS_EXIT } } }),
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

test('a converter process the environment stopped, or a fixture that never ran, is INCOMPLETE with a harness error and no library verdict', () => {
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
  // The harness binary refused the run itself: its own exit code, and what it said.
  const refusal = `the converter process for table_heavy.pdf could not make the run (exit ${HARNESS_EXIT}, the harness binary's own refusal)`;
  const refused = stoppedBy(
    { report: null, run: { exitCode: HARNESS_EXIT, stderr: 'okf-qualify-docling: missing fixture C:\\fixtures\\table_heavy.pdf\n' } },
    `${refusal}: okf-qualify-docling: missing fixture C:\\fixtures\\table_heavy.pdf`,
  );
  assert.equal(refused.harness_exit_code, HARNESS_EXIT);
  assert.match(refused.harness_stderr, /missing fixture/);
  assert.deepEqual([refused.process_failed, refused.process_crashed], [true, false]);
  // That exit is not trusted although a receipt is on disk (the review's surviving mutant).
  stoppedBy({ run: { exitCode: HARNESS_EXIT, stderr: 'okf-qualify-docling: wait for stdin EOF: broken pipe' } }, `${refusal}: okf-qualify-docling: wait for stdin EOF: broken pipe`);
  stoppedBy({ report: null }, 'the converter process for table_heavy.pdf exited 0 without a readable receipt');
  // The binary is missing.
  stoppedBy({ report: null, run: { exitCode: null, spawnError: 'ENOENT: no such file or directory, uv_spawn' } }, 'the converter process for table_heavy.pdf could not be started: ENOENT: no such file or directory, uv_spawn');
  // The harness's own timeout expired, whatever the kill then looked like to the process.
  stoppedBy({ report: null, run: { exitCode: null, signal: 'SIGTERM', timedOut: true } }, 'the converter process for table_heavy.pdf was killed at the harness timeout');
  stoppedBy({ report: null, run: { exitCode: 1, signal: null, timedOut: true } }, 'the converter process for table_heavy.pdf was killed at the harness timeout');
  // A signal sent from outside the process (the operating system, an operator).
  stoppedBy({ report: null, run: { exitCode: null, signal: 'SIGKILL' } }, 'the converter process for table_heavy.pdf ended on signal SIGKILL, sent from outside the process');
  stoppedBy({ report: null, run: { exitCode: null, signal: 'SIGTERM' } }, 'the converter process for table_heavy.pdf ended on signal SIGTERM, sent from outside the process');

  // A harness error does not hide a failure the library did produce, and several are counted.
  const both = doclingReceipt({
    'table_heavy.pdf': { report: null, run: { exitCode: HARNESS_EXIT } },
    'scanned_text.pdf': { report: null, run: { exitCode: HARNESS_EXIT } },
    'corpus/word_sample.docx': { evidence: { markdown: 'Goodbye' } },
  });
  assert.equal(both.result, 'FAIL');
  assert.equal(both.harness_error, `the converter process for scanned_text.pdf could not make the run (exit ${HARNESS_EXIT}, the harness binary's own refusal) (and 1 more harness error(s); each is the detail of the criteria it stopped)`);
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

test('a converter process that dies on a fixture fails that fixture, with the exit code and the last lines of its stderr, and is no harness error', async () => {
  const panic = [
    'warning: 3 fonts without a ToUnicode map',
    'loading layout model',
    "thread 'main' panicked at docling-pdf-1.93.6/src/assemble.rs:2084:17:",
    'index out of bounds: the len is 3 but the index is 7',
    'note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace',
    '',
  ].join('\n');
  const shown = "warning: 3 fonts without a ToUnicode map | loading layout model | thread 'main' panicked at docling-pdf-1.93.6/src/assemble.rs:2084:17: | index out of bounds: the len is 3 but the index is 7 | note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace";
  const others = (aspects) => aspects.filter((aspect) => !['conversion', 'refused', 'timeout_reported'].includes(aspect));

  // A Rust panic exits 101, whether or not the process had written a receipt before it died.
  for (const report of [undefined, null]) {
    const receipt = doclingReceipt({ 'table_heavy.pdf': { ...(report === null ? { report } : {}), run: { exitCode: 101, peakRssBytes: null, peakRssNote: 'converter exited before printing the done marker; peak RSS was not sampled', stderr: panic } } });
    const how = 'the converter process for table_heavy.pdf exited 101';
    assert.equal(receipt.result, 'FAIL');
    assert.equal(receipt.harness_error, null, 'the library dying on a fixture is not the environment');
    assert.deepEqual(doclingFailedIds(receipt), ['table_heavy.pdf/conversion']);
    assert.deepEqual(doclingCriterion(receipt, 'table_heavy.pdf/conversion'), {
      id: 'table_heavy.pdf/conversion',
      required: true,
      result: 'fail',
      detail: `${how} while converting the fixture; the last lines of its stderr: ${shown}`,
    });
    const entry = entryOf(receipt, 'table_heavy.pdf');
    assert.deepEqual([entry.outcome, receipt.summary['table_heavy.pdf'], entry.process_crashed, entry.process_failed, entry.harness_error, entry.harness_exit_code], ['FAIL', 'FAIL', true, false, null, 101]);
    assert.deepEqual(entry.failed_criteria, ['conversion']);
    assert.deepEqual([entry.criteria.conversion.exit_code, entry.criteria.conversion.signal], [101, null]);
    // Nothing else of the fixture is judged, and none of it is blamed on the environment.
    for (const aspect of others(Object.keys(entry.criteria))) {
      assert.deepEqual(entry.criteria[aspect], { result: 'not_judged', detail: `${how}, so there is nothing of the fixture to judge` }, aspect);
    }
    assert.deepEqual(Object.keys(entry.criteria), ['conversion', 'evidence', 'format_recognised', 'content', 'page_renders', 'provenance', 'undecoded_glyphs_reported', 'memory_measured']);
    assert.deepEqual(receipt.failures.filter((item) => item.fixture === 'table_heavy.pdf').map((item) => [item.criterion, item.result]), Object.keys(entry.criteria).map((aspect) => [aspect, aspect === 'conversion' ? 'fail' : 'not_judged']));
    assertDoclingEnvelope(receipt);
  }
  // The detail carries the last five lines, no more.
  assert.equal(doclingCriterion(doclingReceipt({ 'table_heavy.pdf': { run: { exitCode: 101, stderr: `an earlier line\n${panic}` } } }), 'table_heavy.pdf/conversion').detail, `the converter process for table_heavy.pdf exited 101 while converting the fixture; the last lines of its stderr: ${shown}`);
  assert.match(doclingCriterion(doclingReceipt({ 'table_heavy.pdf': { run: { exitCode: 101, stderr: '' } } }), 'table_heavy.pdf/conversion').detail, /the last lines of its stderr: \(it wrote nothing to stderr\)$/);
  // A peak that was read before the process died stays a measurement.
  assert.equal(entryOf(doclingReceipt({ 'table_heavy.pdf': { run: { exitCode: 101, stderr: panic } } }), 'table_heavy.pdf').criteria.memory_measured.result, 'pass');

  // Every way a process dies of itself: any other non-zero exit, and the signals a process raises against itself.
  for (const [run, how] of [
    [{ exitCode: 1 }, 'exited 1'],
    [{ exitCode: 3221225477 }, 'exited 3221225477'],
    [{ exitCode: 134 }, 'exited 134'],
    [{ exitCode: null, signal: 'SIGSEGV' }, 'died on signal SIGSEGV'],
    [{ exitCode: null, signal: 'SIGABRT' }, 'died on signal SIGABRT'],
    [{ exitCode: null, signal: 'SIGBUS' }, 'died on signal SIGBUS'],
    [{ exitCode: null, signal: 'SIGILL' }, 'died on signal SIGILL'],
  ]) {
    const receipt = doclingReceipt({ 'scanned_text.pdf': { report: null, run: { ...run, stderr: 'fatal runtime error' } } });
    assert.deepEqual([receipt.result, receipt.harness_error], ['FAIL', null], how);
    assert.deepEqual(doclingFailedIds(receipt), ['scanned_text.pdf/conversion'], how);
    assert.equal(doclingCriterion(receipt, 'scanned_text.pdf/conversion').detail, `the converter process for scanned_text.pdf ${how} while converting the fixture; the last lines of its stderr: fatal runtime error`);
  }

  // The must-fail fixture and the timeout probe: a process that dies has neither refused the input nor reported the budget.
  const truncated = doclingReceipt({ [MUST_FAIL]: { report: null, run: { exitCode: 101, stderr: panic } } });
  assert.deepEqual(doclingFailedIds(truncated), [`${MUST_FAIL}/refused`]);
  assert.match(doclingCriterion(truncated, `${MUST_FAIL}/refused`).detail, /^the converter process for must_fail_truncated\.pdf exited 101 instead of refusing the truncated input; the last lines of its stderr: /);
  assert.equal(doclingCriterion(truncated, `${MUST_FAIL}/refusal_attributed`).result, 'not_judged');
  const probe = doclingReceipt({ [TIMEOUT_PROBE]: { report: null, run: { exitCode: 101, stderr: panic } } });
  assert.deepEqual(doclingFailedIds(probe), [`${TIMEOUT_PROBE}/timeout_reported`]);
  assert.match(doclingCriterion(probe, `${TIMEOUT_PROBE}/timeout_reported`).detail, /^the converter process for timeout_probe exited 101 instead of reporting the spent budget; /);
  assert.deepEqual([truncated.harness_error, probe.harness_error, truncated.result, probe.result], [null, null, 'FAIL', 'FAIL']);
  // A PDF whose process died did not convert: it cannot vouch for the pipeline, nor serve as the probe's full conversion.
  const all = doclingReceipt(Object.fromEntries(FIXTURE_RUNS.filter((only) => only.endsWith('.pdf') && only !== MUST_FAIL).map((only) => [only, { run: { exitCode: 101 } }])));
  assert.equal(doclingCriterion(all, `${MUST_FAIL}/refusal_attributed`).result, 'not_judged');
  assert.match(doclingCriterion(all, `${TIMEOUT_PROBE}/budget_had_effect`).detail, /^the full conversion of scanned_image_only\.pdf did not succeed in this run/);

  // The library failing beside the environment failing: the failure stands, and the harness error is still said.
  const mixed = doclingReceipt({ 'table_heavy.pdf': { run: { exitCode: 101, stderr: panic } }, 'scanned_text.pdf': { report: null, run: { exitCode: null, signal: 'SIGKILL' } } });
  assert.deepEqual([mixed.result, mixed.harness_error], ['FAIL', 'the converter process for scanned_text.pdf ended on signal SIGKILL, sent from outside the process']);
  assert.deepEqual(doclingFailedIds(mixed), ['table_heavy.pdf/conversion']);

  // The one exit code that is the environment is the one the binary keeps for its own refusals, on both sides.
  const main = await readFile(join(root, 'qualification/docling/src/main.rs'), 'utf8');
  assert.equal(HARNESS_EXIT, 64);
  assert.match(main, new RegExp(`^const EXIT_HARNESS: u8 = ${HARNESS_EXIT};$`, 'm'));
  assert.match(main, /^fn main\(\) -> ExitCode \{$/m);
  assert.match(main, /Err\(error\) => \{\s+let _ = writeln!\(io::stderr\(\), "okf-qualify-docling: \{error\}"\);\s+ExitCode::from\(EXIT_HARNESS\)\s+\}/);
  assert.doesNotMatch(main, /process::exit|fn main\(\) -> Result/, 'the binary must end through main, with the code that says who refused');
  // The rules say so in the receipt.
  assert.match(truncated.criterion_rules.conversion, /a converter process that dies on the fixture \(a panic, an abort, a fault: any exit the harness binary did not make itself\) fails this criterion, with its exit code and the last lines of its stderr$/);
  assert.match(truncated.criterion_rules.refused, /a converter process that dies on the input has not refused it and fails this criterion$/);
  assert.match(truncated.criterion_rules.timeout_reported, /a converter process that dies under the budget has not reported it and fails this criterion$/);
  // No id was added: the criteria a dying process fails are ones criteria.json already pins.
  const pinned = JSON.parse(await readFile(join(root, 'qualification/docling/criteria.json'), 'utf8')).required;
  for (const id of ['table_heavy.pdf/conversion', `${MUST_FAIL}/refused`, `${TIMEOUT_PROBE}/timeout_reported`]) assert.ok(pinned.includes(id), id);
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

test('a PDF item the text layer locates passes provenance, and the detail always states how many items each source located', () => {
  const captions = [
    { self_ref: '#/texts/2', label: 'caption', text: 'Table 2-1   FUNCTION_USAGE view', prov: [] },
    { self_ref: '#/texts/3', label: 'caption', text: 'Figure 1-2   Existing row and column controls', prov: [] },
  ];
  const document = stubDocument({ texts: [...stubDocument().texts, ...captions] });
  const boxes = [{ l: 136.27, t: 512.02, r: 284.48, b: 504.28 }, { l: 136.27, t: 100.55, r: 316.76, b: 91.27 }];
  const withLocations = (found) => doclingReceipt({ 'born_digital_text.pdf': lookedUp(document, found) });
  const zero = '(0 text items, 0 tables, 0 pictures)';

  const receipt = withLocations(captions.map((caption, index) => lookupFound(caption.self_ref, boxes[index])));
  const entry = entryOf(receipt, 'born_digital_text.pdf');
  assert.equal(entry.criteria.provenance.result, 'pass');
  assert.equal(receipt.result, 'PASS');
  assert.equal(
    doclingCriterion(receipt, 'born_digital_text.pdf/provenance').detail,
    `5 items: 3 located by the export (2 text items, 1 tables, 0 pictures), 2 through the text layer (2 text items, 0 tables, 0 pictures), 0 not located ${zero}`,
  );
  // Each item records the source that located it, its page and its box; the receipt holds the list once.
  assert.deepEqual(
    entry.criteria.provenance.located_items.map((item) => [item.ref, item.located_by, item.page_no, item.bbox?.l ?? null]),
    [['#/texts/0', 'export', 1, 72], ['#/texts/1', 'export', 1, 72], ['#/texts/2', 'text_layer', 1, 136.27], ['#/texts/3', 'text_layer', 1, 136.27], ['#/tables/0', 'export', 1, 72]],
  );
  assert.deepEqual(entry.locations, { items: '5 recorded; judged under criteria.provenance.located_items', rule: 'stub', text_layer: { error: null, items: 2, source: 'docling::pdf_text_layer_pages' } });
  assert.deepEqual(entry.page_provenance, [{ page_no: 1, text_items: 4, tables: 1, pictures: 0, has_image: true }]);
  assertDoclingEnvelope(receipt);

  // The three counts are stated whether the criterion passes or fails, for every paginated fixture.
  const stated = /^\d+ items: \d+ located by the export \(\d+ text items, \d+ tables, \d+ pictures\), \d+ through the text layer \(\d+ text items, \d+ tables, \d+ pictures\), \d+ not located \(\d+ text items, \d+ tables, \d+ pictures\)/;
  const judged = receipt.criteria.filter((criterion) => criterion.id.endsWith('/provenance') && (criterion.result === 'pass' || criterion.result === 'fail'));
  assert.deepEqual(judged.map((criterion) => criterion.id), FIXTURE_RUNS.filter((only) => isPaginated(only) && only !== MUST_FAIL).map((only) => `${only}/provenance`));
  for (const criterion of judged) assert.match(criterion.detail, stated, criterion.id);
  // An image has no text layer: it is judged by its export alone and needs no record of the lookup.
  assert.equal(entryOf(receipt, 'text_image.png').locations, undefined);
  assert.equal(doclingCriterion(receipt, 'text_image.png/provenance').detail, `3 items: 3 located by the export (2 text items, 1 tables, 0 pictures), 0 through the text layer ${zero}, 0 not located ${zero}`);

  // One caption the lookup did not find: the fixture fails, with the counts and the reason.
  const partly = withLocations([lookupFound('#/texts/2', boxes[0]), lookupMissed('#/texts/3', '2 text-layer items on page [1] have exactly this text', 2)]);
  assert.equal(partly.result, 'FAIL');
  assert.deepEqual(doclingFailedIds(partly), ['born_digital_text.pdf/provenance']);
  assert.equal(
    doclingCriterion(partly, 'born_digital_text.pdf/provenance').detail,
    '5 items: 3 located by the export (2 text items, 1 tables, 0 pictures), 1 through the text layer (1 text items, 0 tables, 0 pictures), 1 not located (1 text items, 0 tables, 0 pictures); all: #/texts/3 caption: no provenance; text layer: 2 text-layer items on page [1] have exactly this text',
  );

  // A converter process that recorded no locations for a PDF, or locations of another document, is a harness error.
  for (const [locations, sentence] of [
    [undefined, 'the converter process for born_digital_text.pdf recorded no item locations for a PDF'],
    [stubLocations(stubDocument()), 'the item locations the converter process for born_digital_text.pdf recorded (3) are not those of the 5 items of the document it wrote'],
    [{ ...stubLocations(document), items: stubLocations(document).items.map((item) => ({ ...item, item: '#/texts/0' })) }, 'the item locations the converter process for born_digital_text.pdf recorded (5) are not those of the 5 items of the document it wrote'],
  ]) {
    const unrecorded = doclingReceipt({ 'born_digital_text.pdf': { evidence: { document }, fixture: { locations } } });
    assert.equal(unrecorded.result, 'INCOMPLETE', sentence);
    assert.equal(unrecorded.harness_error, sentence);
    assert.deepEqual(doclingCriterion(unrecorded, 'born_digital_text.pdf/provenance'), { id: 'born_digital_text.pdf/provenance', required: true, result: 'not_judged', detail: sentence });
    assert.deepEqual(doclingFailedIds(unrecorded), []);
  }
});

test('a PDF must show glyph-name placeholders on exactly the pages and in the numbers its fixture declares, and none when it declares none', async () => {
  const declared = structuredClone(DOCLING_SOURCES);
  declared.files['table_heavy.pdf'].expect.undecoded_glyphs = { pages: { 3: 268, 5: 3, 8: 7 }, confirmed_by: 'reading the fonts of the stub' };
  const reporting = (glyphs, sources = declared) => doclingReceipt({ 'table_heavy.pdf': { fixture: { undecoded_glyphs: glyphs } } }, { sources });
  const criterion = (receipt, only = 'table_heavy.pdf') => doclingCriterion(receipt, `${only}/undecoded_glyphs_reported`);

  // Reported as declared: a pass that says how many, where, and that the text is not recovered.
  const good = reporting(stubGlyphs({ 3: 268, 5: 3, 8: 7 }));
  assert.equal(good.result, 'PASS');
  assert.deepEqual(criterion(good), {
    id: 'table_heavy.pdf/undecoded_glyphs_reported',
    required: true,
    result: 'pass',
    detail: '278 glyph-name placeholder(s) reported, as SOURCES.json declares: page 3 (268), page 5 (3), page 8 (7); the text of those glyphs is not in the file and is not recovered',
  });
  // The receipt lists, for the fixture, the pages with undecodable text and the count.
  const judged = entryOf(good, 'table_heavy.pdf').criteria.undecoded_glyphs_reported;
  assert.deepEqual(judged.reported_pages, { 3: 268, 5: 3, 8: 7 });
  assert.deepEqual(judged.declared_pages, { 3: 268, 5: 3, 8: 7 });
  assert.deepEqual([judged.reported_total, judged.declared_total, judged.unlocated_tokens, judged.confirmed_by], [278, 278, 0, 'reading the fonts of the stub']);
  assert.deepEqual(entryOf(good, 'table_heavy.pdf').undecoded_glyphs.pages, { 3: 268, 5: 3, 8: 7 }, 'what the converter process recorded stays in the receipt');
  // A PDF that declares none and shows none passes, and says so.
  assert.equal(criterion(good, 'born_digital_text.pdf').detail, 'no glyph-name placeholder is reported and SOURCES.json declares none: every glyph of the text layer has Unicode');

  // A different number, a different page, a page too many or too few: each fails, naming the page.
  const failing = (glyphs, sources) => {
    const receipt = reporting(glyphs, sources);
    assert.equal(receipt.result, 'FAIL');
    assert.deepEqual(doclingFailedIds(receipt), ['table_heavy.pdf/undecoded_glyphs_reported']);
    return criterion(receipt).detail;
  };
  assert.equal(failing(stubGlyphs({ 3: 267, 5: 3, 8: 7 })), 'the glyph-name placeholders reported differ from the ones SOURCES.json declares in 1 place(s); all: page 3: 267 reported, 268 declared');
  assert.equal(failing(stubGlyphs({ 3: 268, 5: 3 })), 'the glyph-name placeholders reported differ from the ones SOURCES.json declares in 1 place(s); all: page 8: 0 reported, 7 declared');
  assert.equal(failing(stubGlyphs({ 3: 268, 5: 3, 9: 7 })), 'the glyph-name placeholders reported differ from the ones SOURCES.json declares in 2 place(s); all: page 8: 0 reported, 7 declared; page 9: 7 reported, 0 declared');
  assert.equal(failing(stubGlyphs()), 'the glyph-name placeholders reported differ from the ones SOURCES.json declares in 3 place(s); all: page 3: 0 reported, 268 declared; page 5: 0 reported, 3 declared; page 8: 0 reported, 7 declared');
  // Placeholders nobody declared: text that cannot be decoded and that the fixture did not know of.
  assert.equal(failing(stubGlyphs({ 2: 4 }), DOCLING_SOURCES), 'the glyph-name placeholders reported differ from the ones SOURCES.json declares in 1 place(s); all: page 2: 4 reported, 0 declared');
  // A placeholder in an item that is on no page is on none of the declared pages.
  assert.equal(failing(stubGlyphs({ 3: 268, 5: 3, 8: 6 }, 1)), 'the glyph-name placeholders reported differ from the ones SOURCES.json declares in 2 place(s); all: page 8: 6 reported, 7 declared; 1 reported in items that are on no page');
  // A page declared or reported with zero is no page with undecodable text.
  assert.equal(reporting(stubGlyphs({ 3: 268, 5: 3, 8: 7, 9: 0 })).result, 'PASS');

  // A PDF process that recorded no count is a harness error, not a pass and not a library failure.
  for (const glyphs of [undefined, { pages: null, unlocated_tokens: 0 }, { pages: { 3: 'many' }, unlocated_tokens: 0 }, { pages: {} }]) {
    const unrecorded = reporting(glyphs);
    assert.equal(unrecorded.result, 'INCOMPLETE');
    assert.equal(unrecorded.harness_error, 'the converter process for table_heavy.pdf recorded no count of glyph-name placeholders for a PDF');
    assert.equal(criterion(unrecorded).result, 'not_judged');
  }
  // A conversion that failed leaves nothing to count.
  const failed = doclingReceipt({ 'table_heavy.pdf': { fixture: { outcome: 'FAIL_converter_error', status: null } } }, { sources: declared });
  assert.equal(criterion(failed).result, 'not_judged');

  // Every PDF fixture has the criterion, required and pinned; no other fixture has it.
  const sources = JSON.parse(await readFile(join(root, 'tests/fixtures/documents/SOURCES.json'), 'utf8'));
  const pdfs = Object.entries(sources.files).filter(([, entry]) => entry.kind === 'pdf' && entry.role !== 'must_fail').map(([name]) => name);
  assert.deepEqual(pdfs.sort(), ['born_digital_text.pdf', 'corpus/redp5110_sampled.pdf', 'scanned_image_only.pdf', 'scanned_text.pdf', 'table_heavy.pdf']);
  assert.deepEqual(doclingCriteria.requiredIds(sources).filter((id) => id.endsWith('/undecoded_glyphs_reported')), pdfs.map((name) => `${name}/undecoded_glyphs_reported`));
  // The corpus PDF declares what was counted in the file; every other PDF declares nothing, which means none.
  assert.deepEqual(sources.files['corpus/redp5110_sampled.pdf'].expect.undecoded_glyphs.pages, { 3: 268, 5: 3, 6: 2, 8: 7, 11: 5 });
  assert.match(sources.files['corpus/redp5110_sampled.pdf'].expect.undecoded_glyphs.confirmed_by, /^reading the file without any converter/);
  // How the numbers were obtained, and that two readers written independently of each other agree; the receipt carries the sentence as confirmed_by.
  assert.match(sources.files['corpus/redp5110_sampled.pdf'].expect.undecoded_glyphs.confirmed_by, /The counts were obtained by that reader, written by whoever authored this expectation, and again by a second reader of the same PDF bytes that an independent reviewer of the harness wrote without sight of the first; the two agree on every page, and neither reader is a tool of this repository\.$/);
  for (const name of pdfs.filter((pdf) => pdf !== 'corpus/redp5110_sampled.pdf')) assert.equal(sources.files[name].expect.undecoded_glyphs, undefined, name);
  assert.match(sources.undecoded_glyphs_note, /never from converter output/);
  assert.match(good.criterion_rules.undecoded_glyphs_reported, /A pass says the text that cannot be decoded is detected and where; it does not say that text was recovered/);
  // The harness binary computes the count for a PDF with the detector module, and the module restates the library's rule.
  const main = await readFile(join(root, 'qualification/docling/src/main.rs'), 'utf8');
  assert.match(main, /let undecoded_glyphs = glyphs::undecoded_glyphs\(export, &locations\.items\);/);
  const detector = await readFile(join(root, 'qualification/docling/src/glyphs.rs'), 'utf8');
  assert.match(detector, /docling-pdf 1\.93\.6 `textparse\.rs`, `Font::decode_code`, lines 118-119/);
  assert.match(detector, /`is_gid_name` accepts it \(lines 354-376\)/);
});

test('an expectation that crosses a font change is judged by its own criterion, with the same comparison, and content judges the others', async () => {
  const phrase = 'Work Function Usage (WRKFCNUSG)';
  const marked = structuredClone(DOCLING_SOURCES);
  marked.files['table_heavy.pdf'].expect = {
    confirmed_by: 'test stub',
    crosses_font_runs_confirmed_by: 'three font selections in the content stream of the stub',
    markdown_contains: ['Hello world', { text: phrase, crosses_font_runs: true }],
    tables: 1,
    pages: 1,
  };
  const converted = (markdown) => doclingReceipt({ 'table_heavy.pdf': { evidence: { markdown } } }, { sources: marked });
  const of = (receipt, aspect) => doclingCriterion(receipt, `table_heavy.pdf/${aspect}`);

  // The fixture emits both criteria, both required; a fixture that marks nothing emits only content.
  assert.deepEqual(doclingCriteria.fontRunExpectations(marked.files['table_heavy.pdf']), [{ text: phrase, crosses_font_runs: true }]);
  assert.deepEqual(doclingCriteria.fixtureAspects('table_heavy.pdf', marked.files['table_heavy.pdf']).map(({ aspect, required }) => [aspect, required]).slice(3, 5), [['content', true], ['content_across_font_runs', true]]);
  assert.ok(!doclingCriteria.fixtureAspects('table_heavy.pdf', DOCLING_SOURCES.files['table_heavy.pdf']).some(({ aspect }) => aspect === 'content_across_font_runs'));

  // What the library gives today: the phrase with a space after "(" and before ")". Content passes without it.
  const today = converted('## Title\n\nHello   world\n\n- /SM590000 Work Function Usage ( WRKFCNUSG )\n');
  assert.equal(of(today, 'content').result, 'pass');
  assert.deepEqual(of(today, 'content_across_font_runs'), {
    id: 'table_heavy.pdf/content_across_font_runs',
    required: true,
    result: 'fail',
    detail: `1 of 1 expectations are not met; all: markdown_contains ${JSON.stringify(phrase)} (closest line of the output: "- /SM590000 Work Function Usage ( WRKFCNUSG )", which holds 4 of its 4 words)`,
  });
  assert.equal(today.result, 'FAIL');
  assert.deepEqual(doclingFailedIds(today), ['table_heavy.pdf/content_across_font_runs'], 'exactly the one criterion that names the limitation fails');
  const entry = entryOf(today, 'table_heavy.pdf');
  assert.deepEqual(entry.criteria.content.checks.map((check) => check.expected), ['Hello world', 1], 'content no longer holds the marked phrase');
  assert.deepEqual(entry.criteria.content_across_font_runs.checks, [{ kind: 'markdown_contains', expected: phrase, case_insensitive: false, ok: false, closest: { line: '- /SM590000 Work Function Usage ( WRKFCNUSG )', shared: 4, of: 4 } }]);
  // The observed text stands next to the expected one: "( WRKFCNUSG )" beside "(WRKFCNUSG)".
  assert.ok(of(today, 'content_across_font_runs').detail.includes('(WRKFCNUSG)') && of(today, 'content_across_font_runs').detail.includes('( WRKFCNUSG )'));
  assert.equal(entry.criteria.content_across_font_runs.confirmed_by, 'three font selections in the content stream of the stub');

  // The matcher is the one content uses and forgives nothing: only the phrase as the page shows it passes.
  assert.equal(of(converted(`Hello world\n\n${phrase}\n`), 'content_across_font_runs').result, 'pass');
  assert.equal(converted(`Hello world\n\n${phrase}\n`).result, 'PASS');
  for (const spaced of ['Work Function Usage ( WRKFCNUSG )', 'Work Function Usage (WRKFCNUSG )', 'Work Function Usage ( WRKFCNUSG)', 'work function usage (wrkfcnusg)', 'Work Function Usage(WRKFCNUSG)']) {
    assert.equal(of(converted(`Hello world\n\n${spaced}\n`), 'content_across_font_runs').result, 'fail', spaced);
  }
  // A failure of an ordinary expectation still fails content, and only content.
  const lost = converted(`${phrase}\n`);
  assert.deepEqual(doclingFailedIds(lost), ['table_heavy.pdf/content']);
  assert.equal(of(lost, 'content').detail, '1 of 2 expectations are not met; all: markdown_contains "Hello world" (no line of the output holds half of its words)');
  // A conversion that failed leaves both unjudged.
  const failed = doclingReceipt({ 'table_heavy.pdf': { fixture: { outcome: 'FAIL_converter_error', status: null } } }, { sources: marked });
  assert.deepEqual([of(failed, 'content').result, of(failed, 'content_across_font_runs').result], ['not_judged', 'not_judged']);

  // The real declarations: exactly one expectation is marked, in the corpus PDF, with how it was confirmed.
  const sources = JSON.parse(await readFile(join(root, 'tests/fixtures/documents/SOURCES.json'), 'utf8'));
  const withMarks = Object.entries(sources.files).filter(([, source]) => doclingCriteria.fontRunExpectations(source).length > 0);
  assert.deepEqual(withMarks.map(([name, source]) => [name, doclingCriteria.fontRunExpectations(source)]), [['corpus/redp5110_sampled.pdf', [{ text: phrase, crosses_font_runs: true }]]]);
  const corpus = sources.files['corpus/redp5110_sampled.pdf'];
  assert.match(corpus.expect.crosses_font_runs_confirmed_by, /^reading the content stream of page 8 .*\/F2 \(Helvetica\).*\/F14 \(BookMasterGothic-Bold\).*\/F2 again/);
  assert.equal(corpus.expect.markdown_contains.length, 7, 'no expectation was dropped: six are judged by content, one by the font-run criterion');
  assert.equal(doclingCriteria.splitExpect(corpus).plain.markdown_contains.length, 6);
  assert.deepEqual(doclingCriteria.splitExpect(corpus).font_runs, { confirmed_by: corpus.expect.crosses_font_runs_confirmed_by, markdown_contains: [{ text: phrase, crosses_font_runs: true }] });
  assert.deepEqual(doclingCriteria.requiredIds(sources).filter((id) => id.endsWith('/content_across_font_runs')), ['corpus/redp5110_sampled.pdf/content_across_font_runs']);
  assert.match(sources.font_runs_note, /the expectation stays what the page shows/);
  assert.match(today.criterion_rules.content_across_font_runs, /by the same comparison as content and with nothing forgiven/);
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
      { fixture: 'corpus/word_sample.docx', criterion: 'content', result: 'fail', detail: '1 of 2 expectations are not met; all: markdown_contains "Hello world" (no line of the output holds half of its words)' },
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
    const shown = captions.slice(0, 10).map((item) => `${item.self_ref} caption: no provenance; text layer: ${NO_PAGE_TO_SEARCH}`).join('; ');
    assert.deepEqual(receipt.failures, [
      {
        fixture: 'born_digital_text.pdf',
        criterion: 'provenance',
        result: 'fail',
        detail: `13 items: 1 located by the export (0 text items, 1 tables, 0 pictures), 0 through the text layer (0 text items, 0 tables, 0 pictures), 12 not located (12 text items, 0 tables, 0 pictures); first 10: ${shown}`,
      },
    ]);
    assert.equal(entry.criteria.provenance.text_items.invalid_total, 12);
    assert.equal(receipt.criteria_summary.provenance['born_digital_text.pdf'], 'fail');
    assert.equal(receipt.criteria_summary.provenance['sample_sheet.xlsx'], 'not_judged');
    assert.equal(receipt.result, 'FAIL');
    // A table without a location fails the fixture although every text item is located.
    const lostTable = doclingReceipt({ 'table_heavy.pdf': { evidence: { document: stubDocument({ tables: stubDocument().tables.map((table) => ({ ...table, prov: [] })) }) } } });
    assert.equal(
      doclingCriterion(lostTable, 'table_heavy.pdf/provenance').detail,
      '3 items: 2 located by the export (2 text items, 0 tables, 0 pictures), 0 through the text layer (0 text items, 0 tables, 0 pictures), 1 not located (0 text items, 1 tables, 0 pictures); all: #/tables/0 table: no provenance',
    );
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
  assert.deepEqual(Object.keys(entry.criteria), ['conversion', 'evidence', 'format_recognised', 'no_invented_text', 'page_renders', 'provenance', 'text_provenance', 'undecoded_glyphs_reported', 'memory_measured']);
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
  unjudged({ 'scanned_image_only.pdf': { run: { exitCode: HARNESS_EXIT } } }, 'the full conversion of scanned_image_only.pdf did not succeed in this run, so there is nothing to compare the probe with');
  // A process that left no receipt recorded no input hash: which fixture held the same bytes is not known.
  unjudged({ 'scanned_image_only.pdf': { report: null, run: { exitCode: HARNESS_EXIT } } }, 'no fixture of this run converted the same bytes without a budget, so there is nothing to compare the probe with');
  // The same when the process died on the fixture, which is then a failure of that fixture as well.
  unjudged({ 'scanned_image_only.pdf': { run: { exitCode: 101 } } }, 'the full conversion of scanned_image_only.pdf did not succeed in this run, so there is nothing to compare the probe with', 'FAIL');
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

const ORT_HASH = 'f7c654b3729cb9e5ad2a36a0c38e5b48e63bf4eed22968931aed33a0ad0b527d';

const ORT_DIRECTORY = `C:\\Users\\x\\AppData\\Local\\ort.pyke.io\\dfbin\\x86_64-pc-windows-msvc\\${ORT_HASH}`;

/** cargo's JSON message stream for a build: an artifact line, the ort-sys build script, another build script. */
const cargoMessages = (linkedPaths = [`native=${ORT_DIRECTORY}`]) =>
  [
    JSON.stringify({ reason: 'compiler-artifact', package_id: 'registry+https://github.com/rust-lang/crates.io-index#ort-sys@2.0.0-rc.13', fresh: true }),
    JSON.stringify({ reason: 'build-script-executed', package_id: 'registry+https://github.com/rust-lang/crates.io-index#zstd-sys@2.0.16', linked_libs: [], linked_paths: ['native=D:\\t\\out'] }),
    JSON.stringify({
      reason: 'build-script-executed',
      package_id: 'registry+https://github.com/rust-lang/crates.io-index#ort-sys@2.0.0-rc.13',
      linked_libs: ['dxguid', 'DirectML', 'static=onnxruntime'],
      linked_paths: linkedPaths,
      out_dir: 'D:\\t\\target\\release\\build\\ort-sys-bf63\\out',
    }),
    '{"reason":"build-finished","success":true}',
    'not json at all',
  ].join('\n');

const ORT_DIST = [
  'target\tfeature_set\turl\tsha256_hash',
  '# a comment line',
  `x86_64-pc-windows-msvc\tdirectml\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-pc-windows-msvc+directml.tar.lzma2\t${ORT_HASH}`,
  `x86_64-pc-windows-msvc\twebgpu\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-pc-windows-msvc+webgpu.tar.lzma2\t${'7'.repeat(64)}`,
  '',
].join('\n');

test('the ONNX Runtime library is recorded from what the build left, or as not_recorded with the reason', async () => {
  assert.equal(ortBuildMessage(cargoMessages()).linked_libs.at(-1), 'static=onnxruntime');
  assert.equal(ortBuildMessage('{"reason":"build-finished","success":true}'), null);
  assert.deepEqual(distRows(ORT_DIST).map((row) => row.feature_set), ['directml', 'webgpu']);

  const record = onnxRuntimeRecord({ messagesText: cargoMessages(), distTsv: ORT_DIST, distPath: 'C:\\registry\\ort-sys-2.0.0-rc.13\\build\\download\\dist.tsv' });
  assert.deepEqual(
    { name: record.name, status: record.status, version: record.version, sha256: record.sha256, url: record.url, target: record.target, feature_set: record.feature_set, crate: record.crate, crate_version: record.crate_version, directory: record.directory },
    {
      name: 'onnxruntime',
      status: 'recorded',
      version: '1.28.0',
      sha256: ORT_HASH,
      url: 'https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/x86_64-pc-windows-msvc+directml.tar.lzma2',
      target: 'x86_64-pc-windows-msvc',
      feature_set: 'directml',
      crate: 'ort-sys',
      crate_version: '2.0.0-rc.13',
      directory: ORT_DIRECTORY,
    },
  );
  assert.deepEqual(record.linked_libs, ['dxguid', 'DirectML', 'static=onnxruntime']);
  assert.match(record.sha256_is, /ort-sys compares the downloaded archive with/);
  assert.equal(record.read_from.length, 2);

  // Nothing is assumed: each thing that cannot be read is the stated reason.
  const reason = (input) => {
    const judged = onnxRuntimeRecord({ messagesText: cargoMessages(), distTsv: ORT_DIST, distPath: 'dist.tsv', ...input });
    assert.deepEqual(Object.keys(judged), ['name', 'status', 'reason']);
    assert.equal(judged.status, 'not_recorded');
    return judged.reason;
  };
  assert.equal(reason({ messagesText: '{"reason":"build-finished","success":true}' }), 'cargo printed no build-script-executed message for ort-sys in this build');
  assert.match(reason({ messagesText: cargoMessages(['native=D:\\onnxruntime\\lib']) }), /none of which is a download directory named by a SHA-256/);
  assert.equal(reason({ distTsv: ORT_DIST.replaceAll(ORT_HASH, '0'.repeat(64)) }), `dist.tsv has no row with the hash ${ORT_HASH} of the directory this build links from`);
  assert.match(reason({ distTsv: null }), /has no row with the hash/);
  assert.equal(onnxRuntimeRecord({ messagesText: cargoMessages(), distTsv: ORT_DIST.replace('ms@1.28.0', 'ms'), distPath: 'dist.tsv' }).version, null, 'a URL that names no version gives none');

  // With I/O: cargo is asked, the crate's table is read from where cargo metadata says the crate is, the files are hashed now.
  const asked = [];
  const io = (over = {}) => ({
    root: 'D:\\repo',
    pkg: 'okf-qualify-docling',
    exec: async (command, args) => {
      asked.push([command, ...args].join(' '));
      return args[0] === 'build'
        ? { code: 0, stdout: cargoMessages(), stderr: '' }
        : { code: 0, stdout: JSON.stringify({ packages: [{ name: 'ort-sys', manifest_path: join('C:', 'registry', 'ort-sys-2.0.0-rc.13', 'Cargo.toml') }] }), stderr: '' };
    },
    readFile: async (path) => {
      asked.push(`read ${path}`);
      return ORT_DIST;
    },
    readdir: async () => ['onnxruntime.lib', 'DirectML.dll'],
    hashFile: async (path) => ({ bytes: path.length, sha256: createHash('sha256').update(path).digest('hex') }),
    ...over,
  });
  const read = await readOnnxRuntime(io());
  assert.equal(read.status, 'recorded');
  assert.deepEqual(asked, [
    'cargo build --locked --release -p okf-qualify-docling --message-format=json',
    'cargo metadata --format-version 1 --locked',
    `read ${join('C:', 'registry', 'ort-sys-2.0.0-rc.13', 'build', 'download', 'dist.tsv')}`,
  ]);
  assert.deepEqual(read.files.map((file) => file.file), ['DirectML.dll', 'onnxruntime.lib']);
  assert.equal(read.files[1].sha256, createHash('sha256').update(join(ORT_DIRECTORY, 'onnxruntime.lib')).digest('hex'));
  assert.equal((await readOnnxRuntime(io({ exec: async () => ({ code: 101, stdout: '', stderr: 'error' }) }))).reason, 'cargo build --locked --release -p okf-qualify-docling --message-format=json exited 101');
  assert.equal((await readOnnxRuntime(io({ readdir: async () => { throw new Error('ENOENT: no such directory'); } }))).reason, 'reading the ONNX Runtime record failed: ENOENT: no such directory');
  assert.equal(
    (await readOnnxRuntime(io({ exec: async (command, args) => (args[0] === 'build' ? { code: 0, stdout: cargoMessages(), stderr: '' } : { code: 0, stdout: '{"packages":[]}', stderr: '' }) }))).reason,
    'cargo metadata lists no ort-sys package',
  );

  // The record sits in the receipt's asset section, beside the model assets.
  const receipt = doclingReceipt({}, { assets: { ...DOCLING_ASSETS, native_libraries: [read] } });
  assert.equal(receipt.assets_verified.native_libraries[0].version, '1.28.0');
  assert.equal(receipt.result, 'PASS');
});

/** Every repository file `entry` imports, directly or through its imports. */
async function doclingImportGraph(entry) {
  const seen = new Set();
  const visit = async (file) => {
    if (seen.has(file)) return;
    seen.add(file);
    const source = await readFile(join(root, file), 'utf8');
    for (const match of source.matchAll(/^import\s[^'"]*['"](\.{1,2}\/[^'"]+)['"];?\s*$/gm)) {
      await visit(join(file, '..', match[1]).replaceAll('\\', '/'));
    }
  };
  await visit(entry);
  return [...seen].sort();
}

test('the Docling receipt inputs cover what the run executes and what decides its result', async () => {
  for (const input of ['rust-toolchain.toml', '.cargo/config.toml', 'scripts/lib/provenance.mjs', 'scripts/lib/receipt-envelope.mjs', 'qualification/docling/criteria.json', 'Cargo.toml', 'Cargo.lock', 'tests/fixtures/documents']) {
    assert.ok(DOCLING_INPUTS.includes(input), `${input} is not an input of the Docling receipt`);
  }
  assert.equal(new Set(DOCLING_INPUTS).size, DOCLING_INPUTS.length);
  const tracked = execFileSync('git', ['ls-files', '--', ...DOCLING_INPUTS], { cwd: root }).toString().split('\n').filter(Boolean);
  for (const input of DOCLING_INPUTS) assert.ok(tracked.some((file) => file === input || file.startsWith(`${input}/`)), `${input} is not tracked, so check-receipts could never see it change`);
  // Every repository file the orchestrator loads lies under an input.
  const loaded = await doclingImportGraph('qualification/docling/run.mjs');
  assert.ok(loaded.includes('scripts/lib/receipt-envelope.mjs') && loaded.includes('qualification/docling/lib/png.mjs'), loaded.join(', '));
  for (const file of loaded) assert.ok(DOCLING_INPUTS.some((input) => file === input || file.startsWith(`${input}/`)), `${file} is loaded by run.mjs and is not covered by an input`);
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

/**
 * Run lib/orchestrate.mjs `qualify` with every external step stubbed: no cargo, no converter,
 * no model file. Files are really written, under a temporary output directory. `over` replaces
 * steps; `calls` records the order in which they ran.
 */
async function doclingQualify(t, over = {}) {
  const outDir = await mkdtemp(join(tmpdir(), 'okf-docling-run-'));
  t.after(() => rm(outDir, { recursive: true, force: true }));
  const manifestPath = join(outDir, 'assets.json');
  const calls = [];
  const sourcesPath = join(root, 'tests/fixtures/documents/SOURCES.json');
  const distPath = join('C:', 'registry', 'ort-sys-2.0.0-rc.13', 'build', 'download', 'dist.tsv');
  const nameOf = (dir) => dir.split(/[\\/]/).pop().replaceAll('__', '/');
  const io = {
    readFile: async (path, encoding) => {
      if (path === sourcesPath) return JSON.stringify(DOCLING_SOURCES);
      if (path === distPath) return ORT_DIST;
      if (path === manifestPath) {
        calls.push('read manifest');
        if (over.manifestMissing) throw Object.assign(new Error(`ENOENT: no such file or directory, open '${path}'`), { code: 'ENOENT' });
        return Buffer.from('{}');
      }
      return readFile(path, encoding);
    },
    writeFile,
    mkdir,
    rm,
    readdir: async () => ['onnxruntime.lib'],
    stat: async () => ({ size: 7 }),
    sha256File: async () => 'c'.repeat(64),
    verifyAssets: async () => {
      calls.push('verifyAssets');
      return { manifest: { DOCLING_RS_MODELS_DIR: 'C:\\models', recommended_env: { DOCLING_LAYOUT_ONNX: 'C:\\models\\layout.onnx' } }, verified: DOCLING_ASSETS };
    },
    buildRelease: async () => {
      calls.push('buildRelease');
      return 'D:\\target\\release\\okf-qualify-docling.exe';
    },
    exec: async (command, args) => {
      calls.push(`${command} ${args[0]}${args.includes('--message-format=json') ? ' --message-format=json' : ''}`);
      if (args[0] === 'tree') return { code: 0, stdout: DOCLING_TREE, stderr: '' };
      if (args[0] === 'metadata') return { code: 0, stdout: JSON.stringify({ packages: [{ name: 'ort-sys', manifest_path: join('C:', 'registry', 'ort-sys-2.0.0-rc.13', 'Cargo.toml') }] }), stderr: '' };
      return { code: 0, stdout: cargoMessages(), stderr: '' };
    },
    runFixtureProcess: async ({ command, env }) => {
      const only = env.OKF_DOCLING_ONLY;
      calls.push(`convert ${only}`);
      assert.equal(command, 'D:\\target\\release\\okf-qualify-docling.exe');
      assert.equal(env.DOCLING_LAYOUT_ONNX, 'C:\\models\\layout.onnx', 'the converter is given the manifest environment');
      const stub = doclingRun(only);
      await writeFile(join(env.OKF_DOCLING_OUT, 'receipt.json'), JSON.stringify(stub.report));
      return { ...stub.run, stdout: 'Wrote receipt\n' };
    },
    loadEvidence: async (dir) => doclingRun(nameOf(dir)).evidence,
    log: () => {},
    logError: () => {},
    now: () => '2026-10-05T18:10:00.000Z',
    ...over.io,
  };
  const outcome = await qualifyDocling({
    root,
    header: DOCLING_HEADER,
    outDir,
    manifestPath,
    selected: over.selected ?? FIXTURE_RUNS,
    scope: { mode: 'all', fixtures: FIXTURE_RUNS, qualification: true },
    env: { PATH: 'x' },
    platform: 'win32',
    io,
  });
  const written = JSON.parse(await readFile(join(outDir, 'receipt.json'), 'utf8'));
  return { ...outcome, outDir, calls, written };
}

test('the Docling run verifies assets before it builds, builds before it converts, and writes the receipt it returns', async (t) => {
  const { receipt, receiptPath, converted, calls, written, outDir } = await doclingQualify(t);
  assert.equal(receipt.result, 'PASS');
  assert.equal(converted, true);
  assert.equal(receiptPath, join(outDir, 'receipt.json'));
  assert.deepEqual(written, JSON.parse(JSON.stringify(receipt)), 'the file on disk is the receipt that was judged');
  assert.deepEqual(calls.slice(0, 7), ['read manifest', 'verifyAssets', 'buildRelease', 'cargo tree', 'cargo build --message-format=json', 'cargo metadata', `convert ${FIXTURE_RUNS[0]}`]);
  assert.deepEqual(calls.slice(6), FIXTURE_RUNS.map((only) => `convert ${only}`), 'one process per fixture, in order');
  assert.equal(receipt.git_sha, DOCLING_HEADER.git_sha);
  assert.equal(receipt.converter.version, lockedPackage(await readFile(join(root, 'Cargo.lock'), 'utf8'), 'docling').version, 'the converter version is read from Cargo.lock');
  const lockText = await readFile(join(root, 'Cargo.lock'), 'utf8');
  assert.deepEqual(receipt.converter.packages, doclingPackages(lockText), 'the receipt names where each docling crate is built from, read from Cargo.lock');
  assert.match(receipt.converter.packages.find((entry) => entry.name === 'docling-pdf').source, /^git\+https:\/\/github\.com\/Heyoub\/docling\.rs\?rev=[0-9a-f]{40}#[0-9a-f]{40}$/, 'the PDF crate is named as the fork at its commit, not as a crates.io release');
  assert.equal(receipt.build.command, 'cargo build --locked --release -p okf-qualify-docling');
  assert.match(receipt.assets_verified.manifest, /^[^\\]*assets\.json$/, 'the manifest is named relative to the repository, with forward slashes');
  assert.equal(receipt.assets_verified.native_libraries[0].version, '1.28.0');
  assert.equal(receipt.settings.environment.OKF_DOCLING_CRATE_VERSION, receipt.converter.version);
  assert.equal(receipt.per_fixture.length, FIXTURE_RUNS.length);
  assert.match(receipt.per_fixture[0].stdout_sha256, /^[0-9a-f]{64}$/);
  assertDoclingEnvelope(receipt);
  assert.equal(doclingCriteria.exitCodeFor(receipt.result), 0);

  const harness = await readFile(join(root, 'qualification/docling/src/main.rs'), 'utf8');
  assert.doesNotMatch(harness, /SuccessNonEmpty|must_contain|peak_rss_bytes/, 'no label for a rule that is not applied, no expectation outside SOURCES.json, no field that is always null');
  assert.match(harness, /\.generate_page_images\(applied\.generate_page_images\)/);
  assert.match(harness, /version_source: VERSION_SOURCE/);
  assert.match(harness, /Rule::TimeoutHonoured => "partial_success_with_pipeline_timeout_error"/);
  const run = await readFile(join(root, 'qualification/docling/run.mjs'), 'utf8');
  assert.ok(run.indexOf('await receiptHeader(') < run.indexOf('await qualify('), 'the clean-tree guard comes before the run');
  assert.match(run, /process\.exitCode = exitCodeFor\(receipt\.result\);/);
  assert.doesNotMatch(run, /process\.exit\(|throw new Error\(`Docling qualification/, 'run.mjs reports the folded result; it does not decide one');
});

test('a failure before conversion replaces the previous receipt with an INCOMPLETE one for this run', async (t) => {
  // What an earlier run left behind: a receipt that says PASS for another commit, and its evidence.
  const stale = async (outDir) => {
    await mkdir(join(outDir, 'partial', 'born_digital_text.pdf'), { recursive: true });
    await writeFile(join(outDir, 'partial', 'born_digital_text.pdf', 'document.md'), 'old run');
    await writeFile(join(outDir, 'receipt.json'), JSON.stringify({ git_sha: 'b'.repeat(40), result: 'PASS' }));
  };
  const stoppedAt = async (over, sentence, ranBefore) => {
    let outDir = null;
    const io = {
      ...over.io,
      mkdir: async (path, options) => {
        // The first thing the run touches is its output directory: plant the stale files just before.
        if (outDir === null) {
          outDir = path;
          await mkdir(path, options);
          await stale(path);
          return undefined;
        }
        return mkdir(path, options);
      },
    };
    const { receipt, converted, calls, written } = await doclingQualify(t, { ...over, io });
    assert.equal(receipt.result, 'INCOMPLETE', String(sentence));
    if (typeof sentence === 'string') assert.equal(receipt.harness_error, sentence);
    else assert.match(receipt.harness_error, sentence);
    assert.equal(converted, false);
    assert.deepEqual(calls, ranBefore, 'nothing runs after the step that failed');
    assert.deepEqual(doclingFailedIds(receipt), [], 'the environment is never a library verdict');
    assert.deepEqual([...new Set(receipt.criteria.filter((item) => !item.id.startsWith('assets/')).map((item) => item.result))], ['not_judged'], 'no fixture criterion is judged');
    // The receipt on disk is this run's, not the earlier PASS; the earlier evidence is gone.
    assert.equal(written.result, 'INCOMPLETE');
    assert.equal(written.git_sha, DOCLING_HEADER.git_sha);
    assert.equal(written.harness_error, receipt.harness_error);
    await assert.rejects(stat(join(outDir, 'partial', 'born_digital_text.pdf', 'document.md')), /ENOENT/);
    assertDoclingEnvelope(receipt);
    assert.equal(doclingCriteria.exitCodeFor(receipt.result), 3);
    return receipt;
  };

  const noManifest = await stoppedAt(
    { manifestMissing: true },
    /^the Docling model assets manifest is not readable at .*assets\.json \(qualification needs the verified model assets\): ENOENT: no such file or directory/,
    ['read manifest'],
  );
  assert.equal(noManifest.assets_verified, null);
  assert.equal(doclingCriterion(noManifest, 'assets/hashes_match').result, 'not_judged');
  await stoppedAt(
    { io: { verifyAssets: async () => { throw new Error('Docling model asset missing: C:\\models\\layout.onnx (ENOENT). A missing asset is never qualified.'); } } },
    'the Docling model assets could not be verified: Docling model asset missing: <abs>/models/layout.onnx (ENOENT). A missing asset is never qualified.',
    ['read manifest'],
  );
  const noBuild = await stoppedAt(
    { io: { buildRelease: async () => { throw new Error('cargo build --locked --release -p okf-qualify-docling exited 101'); } } },
    'the harness build failed: cargo build --locked --release -p okf-qualify-docling exited 101',
    ['read manifest', 'verifyAssets'],
  );
  assert.equal(doclingCriterion(noBuild, 'assets/hashes_match').result, 'pass', 'what was verified before the failure stays recorded');
  assert.equal(noBuild.build, null);
  const noTree = await stoppedAt(
    { io: { exec: async () => ({ code: 101, stdout: '', stderr: 'error: no such package' }) } },
    'the docling features of this build could not be read from cargo: cargo tree --locked -p okf-qualify-docling -e normal --prefix none -f {p}|{f} exited 101 error: no such package',
    ['read manifest', 'verifyAssets', 'buildRelease'],
  );
  assert.equal(noTree.converter.crate, 'docling');
});

test('a converter binary that is missing, or declarations that cannot be read, end INCOMPLETE and never as a library failure', async (t) => {
  const missing = { exitCode: null, signal: null, spawnError: 'ENOENT: no such file or directory, uv_spawn', timedOut: false, done: null, peakRssBytes: null, peakRssNote: 'not sampled', stdout: '', stderr: '' };
  const { receipt, converted, written } = await doclingQualify(t, { io: { runFixtureProcess: async () => missing, loadEvidence: async () => ({ markdown: null, document: null, pageFiles: {}, problems: [] }) } });
  assert.equal(converted, true, 'the run reached conversion; each process failed to start');
  assert.equal(receipt.result, 'INCOMPLETE');
  assert.equal(
    receipt.harness_error,
    `the converter process for ${FIXTURE_RUNS[0]} could not be started: ENOENT: no such file or directory, uv_spawn (and ${FIXTURE_RUNS.length - 1} more harness error(s); each is the detail of the criteria it stopped)`,
  );
  assert.deepEqual(doclingFailedIds(receipt), []);
  assert.equal(doclingCriterion(receipt, 'assets/model_inventory').detail, 'no converter process reported docling::model_inventory()');
  assert.deepEqual(receipt.failures.filter((item) => item.fixture !== null).map((item) => item.fixture), FIXTURE_RUNS, 'each fixture has an entry of its own');
  assert.equal(written.result, 'INCOMPLETE');
  assertDoclingEnvelope(receipt);

  // Without the declarations there is no list of fixtures: the receipt still says what stopped the run.
  const unread = await doclingQualify(t, { io: { readFile: async (path, encoding) => (path.endsWith('SOURCES.json') ? '{ not json' : readFile(path, encoding)) } });
  assert.equal(unread.receipt.result, 'INCOMPLETE');
  assert.match(unread.receipt.harness_error, /^the fixture declarations in SOURCES\.json could not be read: /);
  assert.deepEqual(unread.calls, []);
  assert.deepEqual(doclingFailedIds(unread.receipt), []);

  // A step that fails between two fixtures keeps what was judged and leaves the rest not judged.
  let seen = 0;
  const midway = await doclingQualify(t, {
    io: {
      loadEvidence: async (dir) => {
        seen += 1;
        if (seen === 3) throw new Error('EIO: read failed');
        return doclingRun(dir.split(/[\\/]/).pop().replaceAll('__', '/')).evidence;
      },
    },
  });
  assert.equal(midway.receipt.result, 'INCOMPLETE');
  assert.equal(midway.receipt.harness_error, 'EIO: read failed');
  assert.deepEqual(Object.values(midway.receipt.summary).slice(0, 3), ['PASS', 'PASS', 'INCOMPLETE']);
  assert.equal(doclingCriterion(midway.receipt, `${FIXTURE_RUNS[2]}/conversion`).detail, 'the fixture did not run: EIO: read failed');
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
  const run = (options = {}) => qualifyMcpApps(effects, { pinned, config, paths: pathContext({ root }), ...options });
  return { run, calls, written, pinned, observations };
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
  const { receipt, exitCode } = await double.run({ views: withoutDataset });
  assert.equal(receipt.result, 'INCOMPLETE');
  assert.match(receipt.harness_error, /not the ones criteria\.json pins: pinned criterion render_present\/chart_marks is missing/);
  assert.match(receipt.harness_error, /pinned criterion render_present\/dataset_exercised is missing/);
  assert.ok(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned).includes('pinned criterion render_present/dataset_exercised is missing'));
  assert.equal(receipt.basic_host.present_dataset.status, 'not_observed');
  assert.equal(exitCode, 3);
  // The one recording path refuses it, for the same reason.
  assert.match(await recordRefusal(root, 'mcp-apps', receipt), /^its envelope cannot be trusted \(.*pinned criterion render_present\/dataset_exercised is missing/);
});

test('the MCP Apps receipt carries the shared envelope, and its result is the fold of its criteria', async () => {
  const double = await qualifyDouble();
  const { receipt, exitCode } = await double.run();
  assert.deepEqual(Object.keys(receipt).slice(0, 9), ['git_sha', 'inputs', 'produced_at', 'component', 'gate', 'result', 'harness_error', 'criteria', 'not_judged']);
  assert.equal(receipt.gate, 'mcp-apps-protocol-qualification');
  assert.equal(receipt.result, 'PASS');
  assert.equal(receipt.harness_error, null);
  assert.deepEqual(receipt.harness_error_detail, []);
  assert.deepEqual(receipt.not_judged, []);
  assert.deepEqual(receipt.criteria.map((criterion) => criterion.id), criterionIds());
  assert.ok(receipt.criteria.every((criterion) => criterion.required === true && criterion.result === 'pass'));
  assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned), []);
  assert.equal(receipt.result, mcpAppsFold(receipt.criteria, receipt.harness_error));
  assert.equal(exitCode, 0);
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
  assert.deepEqual(EXIT_CODES, { PASS: 0, FAIL: 2, INCOMPLETE: 3 });
  assert.deepEqual(['PASS', 'FAIL', 'INCOMPLETE', 'anything else'].map(exitCodeFor), [0, 2, 3, 3]);
});

test('the receipt says in words what was the product and what was this harness', async () => {
  const { receipt } = await (await qualifyDouble()).run();
  assert.deepEqual(receipt.scope, SCOPE);
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
    assert.equal(exitCode, 2, label);
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
    ['startHarness', boom('okf-qualify-mcp-apps exited before listening (exit 1)\nstderr:\nbind 127.0.0.1:18765: access denied'), /^the harness server did not start: okf-qualify-mcp-apps exited before listening \(exit 1\)$/, 4],
    ['observeProtocol', boom('fetch failed'), /^the MCP client could not reach the harness server: fetch failed$/, 4],
    ['startHost', boom('fetch basic-host package.json: HTTP 503'), /^the reference host \(basic-host\) did not start: fetch basic-host package\.json: HTTP 503$/, 17],
    ['launchBrowser', boom(noBrowser), /^Chromium did not start: browserType\.launch: Executable doesn't exist at <abs>\/chrome-win\/chrome\.exe$/, 17],
  ];
  for (const [effect, behaviour, sentence, judgedCount] of stages) {
    const double = await qualifyDouble({ change: { [effect]: behaviour } });
    const { receipt, exitCode } = await double.run();
    const label = `${effect}: ${receipt.harness_error}`;
    assert.equal(receipt.result, 'INCOMPLETE', label);
    assert.match(receipt.harness_error, sentence, label);
    assert.deepEqual(failedIds(receipt), [], `${label}: an environment failure is never a product failure`);
    assert.equal(exitCode, 3, label);
    assert.equal(receipt.criteria.filter((criterion) => criterion.result === 'pass').length, judgedCount, label);
    assert.deepEqual(receipt.not_judged, receipt.criteria.filter((criterion) => criterion.result === 'not_judged').map((criterion) => criterion.id), label);
    assert.equal(receipt.not_judged.length, criterionIds().length - judgedCount, label);
    // The sentence is one line; the whole message is kept beside it; a criterion says only which step stopped.
    assert.doesNotMatch(receipt.harness_error, /\n/, label);
    assert.equal(receipt.harness_error_detail.length, 1, label);
    assert.ok(receipt.harness_error.startsWith(receipt.harness_error_detail[0].what), label);
    assert.ok(receipt.criteria.filter((criterion) => criterion.result === 'not_judged').every((criterion) => criterion.detail === `not reached: ${receipt.harness_error_detail[0].what}`), label);
    if (effect === 'startHarness') assert.match(receipt.harness_error_detail[0].detail, /stderr:\nbind 127\.0\.0\.1:18765: access denied$/);
    if (effect === 'launchBrowser') assert.match(receipt.harness_error_detail[0].detail, /Looks like Playwright was just installed or updated\.$/);
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
  assert.equal(exitCode, 3);
  assert.equal(receipt.protocol_only, true);
  assert.equal(receipt.protocol_check.status, 'passed');
  assert.deepEqual([receipt.basic_host.status, receipt.basic_host.reason], ['not_run', 'skipped: protocol-only run']);
  assert.deepEqual(receipt.host_render, { status: 'not_run', reason: 'skipped: protocol-only run' });
  for (const effect of ['startHost', 'launchBrowser', 'openTunnel', 'hostRenderEvidence']) assert.ok(!double.calls.includes(effect), effect);
  assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, double.pinned), []);
  // Its envelope is clean, so it can be recorded like any other receipt: the gate then says incomplete.
  assert.equal(await recordRefusal(root, 'mcp-apps', receipt), null);
});

test('a run writes its receipt once, when the result is known, and records nothing itself; record.mjs takes a receipt of any result', async () => {
  const passed = await qualifyDouble();
  const pass = await passed.run();
  assert.deepEqual(Object.keys(pass).sort(), ['exitCode', 'receipt'], 'a run returns its receipt and its exit code; recording is not its business');
  assert.deepEqual(passed.calls.slice(-5), ['closeBrowser', 'stopHost', 'stopHarness', 'hostRenderEvidence', 'writeReceipt']);
  assert.equal(passed.calls.filter((call) => call === 'writeReceipt').length, 1);

  // A failed run leaves a failed receipt: honest evidence, with the result its criteria fold to.
  const failed = await qualifyDouble({ observe: (view, good) => (view.dataset ? { ...good, frame: { ...good.frame, svg_marks: 1 } } : good) });
  const fail = await failed.run();
  assert.equal(fail.exitCode, 2);
  assert.equal(failed.written.length, 1);
  assert.equal(failed.written[0].result, 'FAIL');
  assert.equal(failed.calls.at(-1), 'writeReceipt');
  // So does one the machine could not finish; its basic-host section never ran.
  const stopped = await qualifyDouble({ change: { launchBrowser: async () => { throw new Error("Executable doesn't exist"); } } });
  await stopped.run();
  assert.equal(stopped.written[0].result, 'INCOMPLETE');
  assert.equal(stopped.written[0].basic_host.status, 'not_run');
  for (const receipt of [...passed.written, ...failed.written, ...stopped.written]) {
    assert.equal(receipt.result, mcpAppsFold(receipt.criteria, receipt.harness_error));
    assert.deepEqual(mcpAppsEnvelopeFailures(receipt, MCP_APPS_GATE, passed.pinned), []);
    // The single recording path takes each of them: PASS, FAIL and INCOMPLETE alike.
    assert.equal(await recordRefusal(root, 'mcp-apps', receipt), null, receipt.result);
  }
  // It refuses a receipt whose typed result its criteria do not support, whatever the result says.
  assert.match(await recordRefusal(root, 'mcp-apps', { ...failed.written[0], result: 'PASS' }), /^its envelope cannot be trusted \(result is PASS but its criteria fold to FAIL\)/);

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
  assert.equal(sectionStatus(stopped.written[0].criteria, (id) => id.startsWith('render_')), 'not_run');
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

test('both orchestrators take their header from receiptHeader, take no argument and record nothing; record.mjs is the one recording path', async () => {
  for (const name of ['docling', 'mcp-apps']) {
    const source = await readFile(join(root, 'qualification', name, 'run.mjs'), 'utf8');
    assert.match(source, / = await receiptHeader\(root, /, name);
    assert.doesNotMatch(source, /recordReceipt|--record|qualification\/receipts|requireCleanTree|commit_sha/, `${name}/run.mjs records or names the old flag`);
    // An argument is refused before anything is touched, naming the command that records.
    const refusal = source.indexOf(`if (process.argv.length > 2) throw new Error(\`run.mjs takes no argument (got \${process.argv.slice(2).join(' ')}). To record a finished receipt: bun qualification/record.mjs ${name}\`);`);
    assert.ok(refusal > 0, `${name}/run.mjs does not refuse an argument`);
    assert.ok(refusal < source.indexOf('receiptHeader(root, '), `${name}/run.mjs must refuse an argument before it reads the tree`);
    assert.ok(refusal < source.indexOf('await qualify('), `${name}/run.mjs must refuse an argument before the run`);
  }
  // No file of either harness, and no library, copies a receipt: only record.mjs calls recordReceipt.
  const callers = [];
  for (const directory of ['qualification', 'qualification/lib', 'qualification/docling', 'qualification/docling/lib', 'qualification/mcp-apps', 'qualification/mcp-apps/lib', 'scripts', 'scripts/lib']) {
    for (const file of (await readdir(join(root, directory))).filter((entry) => entry.endsWith('.mjs'))) {
      // A call, not the definition and not the name inside its error message.
      if (/(?<!function )\brecordReceipt\((?!\$\{)/.test(await readFile(join(root, directory, file), 'utf8'))) callers.push(`${directory}/${file}`);
    }
  }
  assert.deepEqual(callers, ['qualification/record.mjs']);
});

test('both harnesses end with the same exit code for the same outcome, taken from one place', async () => {
  // The numbers live in the shared envelope module; a harness that kept its own could drift.
  assert.deepEqual(sharedEnvelope.EXIT_CODES, { PASS: 0, FAIL: 2, INCOMPLETE: 3 });
  assert.equal(sharedEnvelope.EXIT_REFUSED, 1);
  assert.ok(Object.isFrozen(sharedEnvelope.EXIT_CODES));
  assert.ok(!Object.values(sharedEnvelope.EXIT_CODES).includes(sharedEnvelope.EXIT_REFUSED), 'a run that wrote no receipt must not look like one that did');
  assert.deepEqual(['PASS', 'FAIL', 'INCOMPLETE', 'anything else', 'constructor', undefined].map(sharedEnvelope.exitCodeFor), [0, 2, 3, 3, 3, 3]);
  for (const [name, harness] of [['docling', doclingCriteria], ['mcp-apps', { EXIT_CODES, exitCodeFor }]]) {
    assert.equal(harness.EXIT_CODES, sharedEnvelope.EXIT_CODES, `${name} has exit codes of its own`);
    assert.equal(harness.exitCodeFor, sharedEnvelope.exitCodeFor, `${name} maps a result to an exit code itself`);
  }
  // No harness file types an exit number: every one comes from the shared names.
  const files = [
    ...['run.mjs', 'lib/criteria.mjs', 'lib/orchestrate.mjs', 'lib/receipt.mjs'].map((file) => `qualification/docling/${file}`),
    ...['run.mjs', 'lib/criteria.mjs', 'lib/qualify.mjs'].map((file) => `qualification/mcp-apps/${file}`),
  ];
  const sources = Object.fromEntries(await Promise.all(files.map(async (file) => [file, await readFile(join(root, file), 'utf8')])));
  for (const [file, source] of Object.entries(sources)) {
    assert.doesNotMatch(source, /process\.exitCode\s*=\s*\d|process\.exit\(|EXIT_CODES\s*=\s*Object|\b(PASS|FAIL|INCOMPLETE):\s*\d/, file);
  }
  // A run that wrote a receipt ends with the code of its result ...
  assert.match(sources['qualification/docling/run.mjs'], /process\.exitCode = exitCodeFor\(receipt\.result\);/);
  assert.match(sources['qualification/mcp-apps/lib/qualify.mjs'], /exitCode: exitCodeFor\(receipt\.result\)/);
  assert.match(sources['qualification/mcp-apps/run.mjs'], /process\.exitCode = exitCode;/);
  // ... and one that was refused before it started, with no receipt, ends with EXIT_REFUSED:
  // Docling by an uncaught error (what Bun exits with), MCP Apps by naming it.
  assert.match(sources['qualification/mcp-apps/run.mjs'], /\n\} catch \(error\) \{\r?\n[^\n]*\n[^\n]*\n  process\.exitCode = EXIT_REFUSED;\r?\n\}\r?\n$/);
  assert.doesNotMatch(sources['qualification/docling/run.mjs'], /\bcatch\b/, 'a refused Docling run must stay an uncaught error');
});

test('no time-budgeted test uses a budget a loaded machine can miss', async () => {
  const source = await readFile(fileURLToPath(import.meta.url), 'utf8');
  assert.equal(SLOW_MACHINE_BUDGET_MS, 20_000);
  assert.doesNotMatch(source, new RegExp(String.raw`\b${2 * 2000}\b`), 'a 4 s budget failed once under a parallel cargo build');
});

test('record.mjs refuses a receipt by its envelope, never by its result or by which sections ran', async () => {
  // Built by the harness's own code: a full PASS and a protocol-only INCOMPLETE whose basic-host section did not run.
  const full = (await (await qualifyDouble()).run()).receipt;
  const protocolOnly = (await (await qualifyDouble()).run({ protocolOnly: true })).receipt;
  assert.deepEqual([full.result, protocolOnly.result, protocolOnly.protocol_only, protocolOnly.basic_host.status], ['PASS', 'INCOMPLETE', true, 'not_run']);
  assert.equal(await recordRefusal(root, 'mcp-apps', full), null);
  assert.equal(await recordRefusal(root, 'mcp-apps', protocolOnly), null, 'an INCOMPLETE receipt is evidence and is recorded; its gate then says incomplete');
  // What is refused is an envelope that cannot be trusted.
  const reasons = async (receipt, name = 'mcp-apps') => recordRefusal(root, name, receipt);
  assert.match(await reasons({ ...protocolOnly, result: 'PASS' }), /result is PASS but its criteria fold to INCOMPLETE/);
  assert.match(await reasons({ ...full, criteria: full.criteria.slice(1) }), /pinned criterion bundle\/single_app_resource is missing/);
  assert.match(await reasons({ ...protocolOnly, not_judged: [] }), /not_judged is \[\] but its criteria derive \["render_source\/text"/);
  assert.match(await reasons(full, 'docling'), /gate is "mcp-apps-protocol-qualification", expected "docling-library-qualification"/);
  assert.match(await reasons(full, 'no-such-harness'), /no Phase 0 gate of kind receipt names the harness no-such-harness/);
  assert.match(await reasons({ git_sha: full.git_sha, inputs: full.inputs, produced_at: full.produced_at }), /criteria must be an array/);
  const source = await readFile(join(root, 'qualification/record.mjs'), 'utf8');
  assert.doesNotMatch(source, /protocol_only|basic_host/, 'record.mjs must not look inside a receipt for a reason to refuse it');
});

/**
 * The join: what each harness really emits, held to the checker's content rules.
 *
 * The harness packages and the checker were built apart. These doubles go through each
 * harness's own receipt-building code with the real declarations (SOURCES.json, VIEWS), so the
 * receipts carry the real criterion ids, and the real `checkReceipts` reads them against the
 * real tracked criteria.json files and the real verification.json in a repository shaped like
 * this one.
 */

/** A phrase as the published docling-pdf 1.93.6 extracts it when its parentheses and their content are separate font runs. */
const spacedAtFontRuns = (text) => text.replace(/\(([^()\s]+)\)/g, '( $1 )');

/**
 * A converter process double for a real fixture of SOURCES.json: a document that holds exactly
 * what the fixture's `expect` block asks for. `over` replaces parts, as for doclingRun; with
 * `over.font_runs_spaced` an expectation that crosses a font change is written the way the
 * published docling-pdf 1.93.6 extracts it, with a space at each font change.
 */
function doclingRunFor(only, source, over = {}) {
  if (only === MUST_FAIL || only === TIMEOUT_PROBE) return doclingRun(only, over);
  const expect = source.expect ?? {};
  const kind = doclingCriteria.KINDS[source.kind];
  const located = kind.provenance !== 'none';
  const pageCount = expect.pages ?? (kind.page_renders ? 1 : 0);
  const pageNumbers = Array.from({ length: pageCount }, (_, index) => index + 1);
  const where = (index) => (located && pageCount > 0 ? prov({ l: 72, t: 720 - 10 * (index % 60), r: 300, b: 712 - 10 * (index % 60) }) : []);
  const phrase = (entry) => (typeof entry === 'string' ? entry : over.font_runs_spaced && entry.crosses_font_runs === true ? spacedAtFontRuns(entry.text) : entry.text);
  const strings = [...(expect.markdown_contains ?? []).map(phrase), ...(expect.ocr_tokens ?? [])];
  const texts = expect.no_text === true
    ? []
    : [
        ...Array.from({ length: expect.headings_at_least ?? 0 }, (_, index) => ({ label: 'section_header', text: `Heading ${index + 1}` })),
        ...strings.map((text) => ({ label: 'text', text })),
      ].map((item, index) => ({ self_ref: `#/texts/${index}`, ...item, prov: where(index) }));
  const tableCount = expect.tables ?? expect.tables_at_least ?? ((expect.table_rows ?? []).length ? 1 : 0);
  const tables = Array.from({ length: tableCount }, (_, index) => ({
    self_ref: `#/tables/${index}`,
    label: 'table',
    prov: where(index),
    data: { grid: (index === 0 ? (expect.table_rows ?? [['a']]) : [['a']]).map((row) => row.map((text) => ({ text }))), table_cells: [] },
  }));
  // A fixture without glyphs still shows something: its picture is what provenance judges.
  const pictures = Array.from({ length: Math.max(expect.pictures_at_least ?? 0, expect.no_text === true ? 1 : 0) }, (_, index) => ({
    self_ref: `#/pictures/${index}`,
    label: 'picture',
    prov: where(index),
    captions: [],
  }));
  const pageBytes = (page_no) => ((expect.blank_pages ?? []).includes(page_no) ? BLANK_PNG : PAGE_PNG);
  const images = kind.page_renders
    ? pageNumbers.map((page_no) => ({ ...PAGE_IMAGE, page_no, bytes: pageBytes(page_no).length, sha256: createHash('sha256').update(pageBytes(page_no)).digest('hex'), file: `page-${page_no}.png` }))
    : [];
  return doclingRun(only, {
    ...over,
    fixture: {
      library_page_count: source.kind === 'pdf' ? { value: pageCount, error: null } : null,
      document: { markdown_file: 'document.md', json_file: 'document.json', page_images: images },
      ...(kind.text_layer ? { undecoded_glyphs: stubGlyphs(expect.undecoded_glyphs?.pages ?? {}) } : {}),
      ...over.fixture,
    },
    evidence: {
      markdown: expect.no_text === true ? '<!-- image -->\n' : `${strings.join('\n\n')}\n`,
      document: {
        pages: Object.fromEntries(pageNumbers.map((page_no) => [page_no, { size: { width: 612, height: 792 }, page_no }])),
        texts,
        tables,
        pictures,
        groups: (expect.sheet_names ?? []).map((name) => ({ label: 'sheet', name })),
      },
      pageFiles: Object.fromEntries(images.map((image) => [image.file, pageFile(pageBytes(image.page_no))])),
      problems: [],
      ...over.evidence,
    },
  });
}

/**
 * A Docling receipt for the real SOURCES.json, built by the harness's own code. `overrides` as
 * for doclingReceipt. `spaced`: the library joins font runs with a space, as the published
 * docling-pdf 1.93.6 does; without it the double is the library this build uses, whose
 * docling-pdf is the fork that keeps such a phrase in one cell (verification.json gate
 * converter-docling-pdf-font-run-patch).
 */
async function realDoclingReceipt(header, overrides = {}, parts = {}, { spaced = false, only = null } = {}) {
  const sources = JSON.parse(await readFile(join(root, 'tests/fixtures/documents/SOURCES.json'), 'utf8'));
  // `only`: the single-fixture mode of run.mjs, which runs the named fixtures and says so in `scope`.
  const selected = only ? FIXTURE_RUNS.filter((name) => only.includes(name)) : FIXTURE_RUNS;
  // As the harness does (qualification/docling/lib/orchestrate.mjs): every path goes through the one rule before the receipt exists.
  return scrubReceiptPaths(buildDoclingReceipt({
    header,
    converter: DOCLING_CONVERTER,
    platform: 'win32',
    build: DOCLING_BUILD,
    assets: DOCLING_ASSETS,
    environment: { DOCLING_RS_MODELS_DIR: 'C:\\models' },
    sources,
    runs: selected.map((name) => doclingRunFor(name, sources.files[name], { ...overrides[name], font_runs_spaced: spaced })),
    scope: only ? { mode: 'only', fixtures: selected, qualification: false } : { mode: 'all', fixtures: FIXTURE_RUNS, qualification: true },
    paths: { fixtures_dir: 'tests/fixtures/documents' },
    finishedAt: '2026-10-06T10:00:00.000Z',
    ...parts,
  }), pathContext({ root }));
}

/**
 * The real record with the owner's acceptance of the font-run criterion added to the Docling gate,
 * tracked by a construction gate, as the record carried one before the docling-pdf patch. The real
 * record accepts no failure; the tests of what an acceptance does under the real checker use this.
 */
function withFontRunAcceptance(recordText) {
  const typed = JSON.parse(recordText);
  const gates = typed.current.gates;
  const acceptance = { criterion: FONT_RUN_CRITERION, decision: 'Accepted in this test repository only.', decided_on: '2026-10-06', decided_by: 'owner', tracked_by: FONT_RUN_TRACKER };
  gates.phase_0 = gates.phase_0.map((gate) => (gate.id !== 'docling-library-qualification' ? gate
    : Object.fromEntries(Object.entries(gate).flatMap((field) => (field[0] === 'status' ? [field, ['accepted_failures', [acceptance]]] : [field])))));
  gates.construction = [...gates.construction, { id: FONT_RUN_TRACKER, status: 'blocked_upstream', owner: 'integration-owner', receipt: 'none', meaning: 'test repository only' }];
  return `${JSON.stringify(typed, null, 2)}
`;
}

/**
 * A repository shaped like this one: the real record and the real criteria files, committed.
 * With `tools` it also holds the real scripts/ and qualification/record.mjs, committed before
 * any receipt cites a commit, so the recording command itself runs in it. With `acceptance` the
 * record carries the owner's acceptance of the font-run criterion (withFontRunAcceptance).
 */
async function joinRepository(t, { tools = false, acceptance = false } = {}) {
  const tracked = ['verification.json', 'qualification/docling/criteria.json', 'qualification/mcp-apps/criteria.json'];
  const files = Object.fromEntries(await Promise.all(tracked.map(async (path) => [path, await readFile(join(root, path), 'utf8')])));
  if (acceptance) files['verification.json'] = withFontRunAcceptance(files['verification.json']);
  const { root: repo } = await fixtureRepo(t, { ...files, 'qualification/receipts/.gitkeep': '', ...(tools ? { '.gitignore': '/.artifacts/\n' } : {}) });
  if (tools) {
    cpSync(join(root, 'scripts'), join(repo, 'scripts'), { recursive: true });
    cpSync(join(root, 'qualification', 'record.mjs'), join(repo, 'qualification', 'record.mjs'));
    await fixtureCommit(repo, {}, 'the recording tools');
  }
  const sha = await fixtureGit(repo, 'rev-parse', 'HEAD');
  /** Commit these receipts as recorded, with the statuses record.mjs would write. */
  const record = async (receipts) => {
    for (const [harness, receipt] of Object.entries(receipts)) await writeFile(join(repo, 'qualification/receipts', `${harness}.json`), `${JSON.stringify(receipt, null, 2)}\n`);
    await writeDerivedRecord(repo);
    await fixtureCommit(repo, {}, 'record');
    return derivedRecord(repo);
  };
  return { repo, sha, record };
}

const derivedStatuses = (derived) => Object.fromEntries(derived.gates.map((gate) => [gate.harness, gate.status]));
const derivedFailures = (derived) => derived.failures.map((failure) => `${failure.name}: ${failure.message}`);
/** A receipt as a harness that dropped one check would write it: the criterion is gone and nothing else says so. */
const withoutCriterion = (receipt, id) => ({ ...receipt, criteria: receipt.criteria.filter((criterion) => criterion.id !== id), not_judged: receipt.not_judged.filter((other) => other !== id) });

const FONT_RUN_CRITERION = 'corpus/redp5110_sampled.pdf/content_across_font_runs';
/** The construction gate the test acceptance of withFontRunAcceptance is tracked by. */
const FONT_RUN_TRACKER = 'font-run-acceptance-of-this-test';

concurrently('join: the receipt each harness builds has no content failure under the real checker, criteria files and record; the Docling result of this build passes, and the font-run failure of the published docling-pdf is failed unless accepted', async (t) => {
  const { repo, sha, record } = await joinRepository(t);
  const doclingHeader = { git_sha: sha, inputs: DOCLING_INPUTS, produced_at: '2026-10-06T09:00:00.000Z' };
  const spaced = await realDoclingReceipt(doclingHeader, {}, {}, { spaced: true });
  const fixed = await realDoclingReceipt(doclingHeader);
  const apps = (await (await qualifyDouble({ change: { header: async () => ({ git_sha: sha, inputs: MCP_APPS_INPUTS, produced_at: '2026-10-06T09:00:00.000Z' }) } })).run()).receipt;
  assert.deepEqual([spaced.result, fixed.result, apps.result], ['FAIL', 'PASS', 'PASS'], JSON.stringify(fixed.failures));
  // The published docling-pdf fails exactly one required criterion: the one that names the font-run limitation.
  assert.deepEqual(doclingFailedIds(spaced), [FONT_RUN_CRITERION]);
  // The ids each harness emitted as required are exactly the ones its tracked file pins.
  const pinned = async (harness) => JSON.parse(await readFile(join(root, 'qualification', harness, 'criteria.json'), 'utf8')).required;
  const requiredOf = (receipt) => receipt.criteria.filter((criterion) => criterion.required).map((criterion) => criterion.id).sort();
  assert.deepEqual(requiredOf(fixed), await pinned('docling'));
  assert.deepEqual(requiredOf(spaced), await pinned('docling'));
  assert.deepEqual(requiredOf(apps), await pinned('mcp-apps'));
  // The Docling receipt lists criteria that are not judged by design; the checker derives the same list.
  assert.ok(fixed.not_judged.length > 0 && apps.not_judged.length === 0);

  // Recorded as they are under the real record, which accepts no failure: this build's library passes.
  const derived = await record({ docling: fixed, 'mcp-apps': apps });
  assert.deepEqual(derivedFailures(derived), []);
  assert.deepEqual(derivedStatuses(derived), { docling: 'passed', 'mcp-apps': 'passed' });
  assert.equal(derived.gates[0].basis, 'qualification/receipts/docling.json result PASS');
  assert.equal(derived.phase_0_qualified, true);
  assert.match(await checkReceipts(repo), /^check-receipts: 2 receipt\(s\) valid against HEAD; docling-library-qualification: passed \(qualification\/receipts\/docling\.json result PASS\); mcp-apps-protocol-qualification: passed \(qualification\/receipts\/mcp-apps\.json result PASS\); phase_0_qualified: true; verification\.json agrees\.$/);
  const written = JSON.parse(await readFile(join(repo, 'verification.json'), 'utf8'));
  assert.equal(written.current.gates.phase_0.find((gate) => gate.harness === 'docling').status, 'passed');
  assert.equal(written.phase_0_qualified, true);
  assert.equal(await recordRefusal(repo, 'docling', fixed), null);
  assert.equal(await recordRefusal(repo, 'mcp-apps', apps), null);

  // A harness that dropped a pinned check is named, for each harness, although its result folds as before; a receipt that fails a check is trusted for nothing, so its gate is incomplete.
  for (const [harness, receipt, id] of [['docling', fixed, 'corpus/redp5110_sampled.pdf/provenance'], ['mcp-apps', apps, 'render_present/dataset_exercised']]) {
    const dropped = withoutCriterion(receipt, id);
    assert.equal(sharedEnvelope.foldCriteria(dropped.criteria, dropped.harness_error), receipt.result);
    const after = await record({ [harness]: dropped });
    assert.deepEqual(derivedFailures(after), [`${harness}.json: pinned criterion ${id} is missing`]);
    assert.equal(derivedStatuses(after)[harness], 'incomplete');
    assert.equal(after.gates.find((gate) => gate.harness === harness).basis, `qualification/receipts/${harness}.json cannot be trusted: pinned criterion ${id} is missing`);
    assert.equal(after.phase_0_qualified, false);
    await assert.rejects(checkReceipts(repo), (error) => error.message.includes(`\n${harness}.json: pinned criterion ${id} is missing`));
    assert.match(await recordRefusal(repo, harness, dropped), new RegExp(`pinned criterion ${id.replaceAll('/', '\\/')} is missing`));
    assert.deepEqual(derivedFailures(await record({ [harness]: receipt })), [], `${harness} restored`);
  }

  // The published docling-pdf under the real record: its one failure is nobody's acceptance, so the gate is failed.
  const unpatched = await record({ docling: spaced });
  assert.deepEqual(derivedFailures(unpatched), []);
  assert.deepEqual(derivedStatuses(unpatched), { docling: 'failed', 'mcp-apps': 'passed' });
  assert.equal(unpatched.phase_0_qualified, false);

  // Where the owner accepted that failure (the record before the patch), the same receipt is accepted with limitations...
  const accepting = await joinRepository(t, { acceptance: true });
  const acceptingHeader = { ...doclingHeader, git_sha: accepting.sha };
  const acceptedSpaced = await realDoclingReceipt(acceptingHeader, {}, {}, { spaced: true });
  const acceptingApps = (await (await qualifyDouble({ change: { header: async () => ({ git_sha: accepting.sha, inputs: MCP_APPS_INPUTS, produced_at: '2026-10-06T09:00:00.000Z' }) } })).run()).receipt;
  const accepted = await accepting.record({ docling: acceptedSpaced, 'mcp-apps': acceptingApps });
  assert.deepEqual(derivedFailures(accepted), []);
  assert.deepEqual(derivedStatuses(accepted), { docling: 'accepted_with_limitations', 'mcp-apps': 'passed' });
  assert.equal(accepted.gates[0].basis, `qualification/receipts/docling.json result FAIL; the owner accepted every failing criterion: ${FONT_RUN_CRITERION}`);
  assert.equal(accepted.phase_0_qualified, true);
  assert.match(await checkReceipts(accepting.repo), /^check-receipts: 2 receipt\(s\) valid against HEAD; docling-library-qualification: accepted_with_limitations \(.*\); mcp-apps-protocol-qualification: passed \(qualification\/receipts\/mcp-apps\.json result PASS\); phase_0_qualified: true \(with accepted limitations: docling-library-qualification\); verification\.json agrees\.$/);
  assert.equal(JSON.parse(await readFile(join(accepting.repo, 'verification.json'), 'utf8')).current.gates.phase_0.find((gate) => gate.harness === 'docling').status, 'accepted_with_limitations');
  assert.equal(await recordRefusal(accepting.repo, 'docling', acceptedSpaced), null);

  // ...and the patched library's receipt makes that acceptance stale: the record says to remove it, as the patch commit did.
  const stale = `docling-library-qualification: accepted failure ${FONT_RUN_CRITERION} is not failing in qualification/receipts/docling.json (its result there is pass); the acceptance is stale: remove the entry from accepted_failures`;
  const later = await accepting.record({ docling: await realDoclingReceipt(acceptingHeader) });
  assert.deepEqual(derivedFailures(later), [stale]);
  assert.deepEqual(derivedStatuses(later), { docling: 'passed', 'mcp-apps': 'passed' });
  assert.equal(later.phase_0_qualified, false);
  await assert.rejects(checkReceipts(accepting.repo), (error) => error.message.split('\n').includes(stale));
});

concurrently('join: a failed and an unfinished receipt from each harness are clean evidence, and the gate says failed or incomplete', async (t) => {
  const { repo, sha, record } = await joinRepository(t);
  const header = (inputs) => ({ git_sha: sha, inputs, produced_at: '2026-10-06T09:00:00.000Z' });
  const appsDouble = (options) => qualifyDouble({ ...options, change: { header: async () => header(MCP_APPS_INPUTS), ...options?.change } });

  // FAIL: the library, or the App, did what a rule forbids.
  // The Docling double is this build's library with one failure, which nobody accepted.
  const doclingFail = await realDoclingReceipt(header(DOCLING_INPUTS), { 'born_digital_text.pdf': { evidence: { markdown: 'something else\n' } } });
  assert.deepEqual(doclingFailedIds(doclingFail), ['born_digital_text.pdf/content']);
  const appsFail = (await (await appsDouble({ observe: (view, good) => (view.dataset ? { ...good, frame: { ...good.frame, svg_marks: 1 } } : good) })).run()).receipt;
  assert.deepEqual([doclingFail.result, appsFail.result], ['FAIL', 'FAIL']);
  let derived = await record({ docling: doclingFail, 'mcp-apps': appsFail });
  assert.deepEqual(derivedFailures(derived), []);
  assert.deepEqual(derivedStatuses(derived), { docling: 'failed', 'mcp-apps': 'failed' });
  assert.match(await checkReceipts(repo), /^check-receipts: 2 receipt\(s\) valid against HEAD; /);

  // INCOMPLETE: the machine stopped the run. Every Docling criterion is then unjudged, 19 of the MCP Apps ones.
  const doclingStopped = await realDoclingReceipt(header(DOCLING_INPUTS), {}, { runs: [], assets: null, build: null, harnessError: 'the Docling model assets could not be verified: missing' });
  const appsStopped = (await (await appsDouble({ change: { launchBrowser: async () => { throw new Error("Executable doesn't exist"); } } })).run()).receipt;
  assert.deepEqual([doclingStopped.result, appsStopped.result], ['INCOMPLETE', 'INCOMPLETE']);
  assert.equal(doclingStopped.not_judged.length, doclingStopped.criteria.length);
  assert.equal(appsStopped.not_judged.length, 19);
  derived = await record({ docling: doclingStopped, 'mcp-apps': appsStopped });
  assert.deepEqual(derivedFailures(derived), []);
  assert.deepEqual(derivedStatuses(derived), { docling: 'incomplete', 'mcp-apps': 'incomplete' });
  assert.match(await checkReceipts(repo), /^check-receipts: 2 receipt\(s\) valid against HEAD; /);

  // The checker's own rule about not_judged holds a harness to its list: one id hidden is named.
  const hidden = { ...doclingStopped, not_judged: doclingStopped.not_judged.slice(1) };
  assert.match(derivedFailures(await record({ docling: hidden })).join('\n'), /^docling\.json: not_judged is \[.*\] but its criteria derive \["assets\/hashes_match",/);
});

concurrently('join: a Docling run that did not judge everything never qualifies, although its one failure is the accepted one', async (t) => {
  // The record of the test repository accepts the font-run failure of the published docling-pdf (withFontRunAcceptance).
  const { repo: first } = await joinRepository(t, { tools: true, acceptance: true });
  const produced_at = '2026-10-06T09:00:00.000Z';
  /** The recording command, the artifacts it reads and the record it writes, in the repository `repo`. */
  const toolsIn = (repo) => ({
    recordCommand: (...names) => runCommand(process.execPath, ['qualification/record.mjs', ...names], { cwd: repo, capture: true, allowFailure: true }),
    artifact: async (harness, receipt) => {
      await mkdir(join(repo, '.artifacts', 'qualification', harness), { recursive: true });
      await writeFile(join(repo, '.artifacts', 'qualification', harness, 'receipt.json'), `${JSON.stringify(receipt, null, 2)}\n`);
    },
    typedRecord: async () => JSON.parse(await readFile(join(repo, 'verification.json'), 'utf8')),
  });
  const doclingGate = (record) => record.current.gates.phase_0.find((gate) => gate.harness === 'docling');
  const unjudgedRequired = (receipt) => receipt.criteria.filter((criterion) => criterion.required && criterion.result === 'not_judged').length;
  // The MCP Apps gate passes throughout, so the Docling gate alone decides.
  const firstHead = await fixtureGit(first, 'rev-parse', 'HEAD');
  const appsReceipt = (await (await qualifyDouble({ change: { header: async () => ({ git_sha: firstHead, inputs: MCP_APPS_INPUTS, produced_at }) } })).run()).receipt;
  await toolsIn(first).artifact('mcp-apps', appsReceipt);

  // Each is the Docling run of the published docling-pdf (the accepted criterion fails) in which something else was not judged.
  const cutShort = [
    ['a converter process that could not be started', { overrides: { 'table_heavy.pdf': { report: null, run: { exitCode: null, spawnError: 'ENOENT: no such file or directory, uv_spawn' } } } }, /^the converter process for table_heavy\.pdf could not be started: /, 8],
    ['a process killed at the harness timeout', { overrides: { 'scanned_text.pdf': { report: null, run: { exitCode: null, signal: 'SIGTERM', timedOut: true } } } }, /^the converter process for scanned_text\.pdf was killed at the harness timeout$/, 8],
    ['a memory peak that could not be read', { overrides: { 'sample_sheet.xlsx': { run: { peakRssBytes: null, peakRssNote: 'Get-Process reported no PeakWorkingSet64' } } } }, /^the peak memory of the converter process for sample_sheet\.xlsx was not measured: /, 1],
    ['a single-fixture run', { only: ['corpus/redp5110_sampled.pdf'] }, null, 87],
  ];

  // The first case records both gates with one command. The others, and the two runs after them, start from the
  // repository that command leaves (the MCP Apps gate recorded and committed), each in its own copy: they are
  // independent of each other, so they run at once.
  const oneCase = async (repo, [what, { overrides = {}, only = null }, harnessError, unjudged], names) => {
    const { recordCommand, artifact, typedRecord } = toolsIn(repo);
    const head = await fixtureGit(repo, 'rev-parse', 'HEAD');
    const receipt = await realDoclingReceipt({ git_sha: head, inputs: DOCLING_INPUTS, produced_at }, overrides, {}, { spaced: true, only });
    // The shape both reviews showed: FAIL, only the accepted criterion failing, required criteria nobody judged.
    assert.equal(receipt.result, 'FAIL', what);
    assert.deepEqual(doclingFailedIds(receipt), [FONT_RUN_CRITERION], what);
    assert.equal(unjudgedRequired(receipt), unjudged, what);
    if (harnessError) assert.match(receipt.harness_error, harnessError, what);
    else assert.equal(receipt.harness_error, null, what);
    assert.equal(await recordRefusal(repo, 'docling', receipt), null, `${what}: clean evidence, so it is recorded`);

    // Recorded by the real command: the gate is incomplete and Phase 0 is not qualified.
    await artifact('docling', receipt);
    const said = await recordCommand('docling', ...names);
    assert.equal(said.code, 0, `${what}: ${said.stderr}`);
    assert.match(said.stdout, /^docling-library-qualification: incomplete \(qualification\/receipts\/docling\.json result FAIL with every failing criterion accepted, but the run did not judge everything: /m, what);
    assert.match(said.stdout, /^phase_0_qualified: false \(docling-library-qualification is incomplete\)$/m, what);
    const derived = await derivedRecord(repo);
    assert.deepEqual(derivedFailures(derived), [], what);
    assert.deepEqual(derivedStatuses(derived), { docling: 'incomplete', 'mcp-apps': 'passed' }, what);
    assert.equal(derived.phase_0_qualified, false, what);
    assert.deepEqual(derived.limitations, [], what);
    const agreeing = await readFile(join(repo, 'verification.json'), 'utf8');
    const written = JSON.parse(agreeing);
    assert.deepEqual([doclingGate(written).status, written.phase_0_qualified], ['incomplete', false], what);
    const recorded = await fixtureCommit(repo, {}, `record: ${what}`);
    assert.match(await checkReceipts(repo), /^check-receipts: 2 receipt\(s\) valid against HEAD; docling-library-qualification: incomplete \(.*\); mcp-apps-protocol-qualification: passed \(.*\); phase_0_qualified: false \(docling-library-qualification is incomplete\); verification\.json agrees\.$/, what);
    assert.deepEqual(await staleReceiptLines(repo, recorded), [], what);

    // What such a run derived before, typed by hand, is refused by the working-tree check and at the pushed commit.
    const forged = await typedRecord();
    doclingGate(forged).status = 'accepted_with_limitations';
    forged.phase_0_qualified = true;
    const typed = await fixtureCommit(repo, { 'verification.json': `${JSON.stringify(forged, null, 2)}\n` }, `type a qualification over: ${what}`);
    await assert.rejects(checkReceipts(repo), (error) => /^docling-library-qualification: verification\.json types status "accepted_with_limitations" but the derived status is "incomplete" /m.test(error.message)
      && /^phase_0_qualified: verification\.json types true but the derived value is false \(docling-library-qualification is incomplete\)/m.test(error.message), what);
    const lines = await staleReceiptLines(repo, typed);
    assert.equal(lines.length, 2, `${what}: ${lines.join(' | ')}`);
    assert.match(lines[0], /^typed record docling-library-qualification: verification\.json at [0-9a-f]{40} types status "accepted_with_limitations" but the derived status is "incomplete" /, what);
    assert.match(lines[1], /^typed record phase_0_qualified: verification\.json at [0-9a-f]{40} types true but the derived value is false /, what);
    // The recording command writes the derived values back.
    const rewritten = await recordCommand();
    assert.equal(rewritten.code, 0, `${what}: ${rewritten.stderr}`);
    assert.match(rewritten.stdout, /^verification\.json rewritten from the receipts\.$/m, what);
    assert.equal(await readFile(join(repo, 'verification.json'), 'utf8'), agreeing, what);
    await fixtureCommit(repo, {}, `the derived record again: ${what}`);
  };

  // A converter process that dies on a valid fixture is the library failing, and nobody accepted that:
  // the gate is failed. It is not incomplete (nothing in the environment stopped it) and not accepted.
  const crashedCase = async (repo) => {
    const { recordCommand, artifact } = toolsIn(repo);
    const crashedAt = await fixtureGit(repo, 'rev-parse', 'HEAD');
    const crashed = await realDoclingReceipt({ git_sha: crashedAt, inputs: DOCLING_INPUTS, produced_at }, { 'table_heavy.pdf': { run: { exitCode: 101, stderr: 'thread main panicked at docling-pdf' } } }, {}, { spaced: true });
    assert.deepEqual([crashed.result, crashed.harness_error], ['FAIL', null]);
    assert.deepEqual(doclingFailedIds(crashed), ['table_heavy.pdf/conversion', FONT_RUN_CRITERION]);
    await artifact('docling', crashed);
    const afterCrash = await recordCommand('docling');
    assert.equal(afterCrash.code, 0, afterCrash.stderr);
    assert.match(afterCrash.stdout, /^docling-library-qualification: failed \(qualification\/receipts\/docling\.json result FAIL\)$/m);
    assert.match(afterCrash.stdout, /^phase_0_qualified: false \(docling-library-qualification is failed\)$/m);
    await fixtureCommit(repo, {}, 'record: a converter process that crashed');
    assert.match(await checkReceipts(repo), /; docling-library-qualification: failed \(.*\); .*phase_0_qualified: false \(docling-library-qualification is failed\); verification\.json agrees\.$/);
  };

  // The same run with every fixture judged is still what the owner accepted.
  const wholeCase = async (repo) => {
    const { recordCommand, artifact } = toolsIn(repo);
    const head = await fixtureGit(repo, 'rev-parse', 'HEAD');
    const whole = await realDoclingReceipt({ git_sha: head, inputs: DOCLING_INPUTS, produced_at }, {}, {}, { spaced: true });
    assert.deepEqual([whole.result, whole.harness_error, unjudgedRequired(whole)], ['FAIL', null, 0]);
    await artifact('docling', whole);
    const said = await recordCommand('docling');
    assert.equal(said.code, 0, said.stderr);
    assert.ok(said.stdout.split(/\r?\n/).includes(`docling-library-qualification: accepted_with_limitations (qualification/receipts/docling.json result FAIL; the owner accepted every failing criterion: ${FONT_RUN_CRITERION})`), said.stdout);
    assert.match(said.stdout, /^phase_0_qualified: true \(with accepted limitations: docling-library-qualification\)$/m);
    const recorded = await fixtureCommit(repo, {}, 'record the whole run');
    assert.match(await checkReceipts(repo), /phase_0_qualified: true \(with accepted limitations: docling-library-qualification\); verification\.json agrees\.$/);
    assert.deepEqual(await staleReceiptLines(repo, recorded), []);
  };

  // The first case leaves the MCP Apps gate recorded and committed: that repository is where the others start.
  const [firstCase, ...laterCases] = cutShort;
  const { root: firstCopy } = await copyRepo(t, first);
  const firstRun = oneCase(firstCopy, firstCase, ['mcp-apps']);
  firstRun.catch(() => {}); // its failure is reported where it is awaited, below
  // The state that case leaves for the MCP Apps gate, made once by the same command and committed.
  let recordedApps;
  try {
    ({ root: recordedApps } = await copyRepo(t, first));
    assert.equal((await toolsIn(recordedApps).recordCommand('mcp-apps')).code, 0);
    await fixtureCommit(recordedApps, {}, 'record the MCP Apps receipt');
  } catch (error) {
    await firstRun.catch(() => {}); // the first case must stop writing before the cleanup removes its copy
    throw error;
  }
  const fromRecorded = async (judge) => judge((await copyRepo(t, recordedApps)).root);
  // Every case settles before the cleanup runs, and every failing case is named.
  await eachCase([
    [`${firstCase[0]} (both gates recorded by one command)`, firstRun],
    ...laterCases.map((entry) => [entry[0], fromRecorded((repo) => oneCase(repo, entry, []))]),
    ['a converter process that crashed', fromRecorded(crashedCase)],
    ['the whole run', fromRecorded(wholeCase)],
  ], ([, running]) => running);
});

concurrently('join: a receipt recorded over an earlier committed receipt of the same gate replaces it, and the gate follows the second one', async (t) => {
  // The record of the test repository accepts the font-run failure of the published docling-pdf (withFontRunAcceptance).
  const { repo } = await joinRepository(t, { tools: true, acceptance: true });
  const produced_at = '2026-10-06T09:00:00.000Z';
  const recordCommand = (...names) => runCommand(process.execPath, ['qualification/record.mjs', ...names], { cwd: repo, capture: true, allowFailure: true });
  const artifact = async (receipt) => {
    await mkdir(join(repo, '.artifacts', 'qualification', 'docling'), { recursive: true });
    await writeFile(join(repo, '.artifacts', 'qualification', 'docling', 'receipt.json'), `${JSON.stringify(receipt, null, 2)}\n`);
  };
  const recordedFile = join(repo, 'qualification', 'receipts', 'docling.json');
  const typedStatus = async () => JSON.parse(await readFile(join(repo, 'verification.json'), 'utf8')).current.gates.phase_0.find((gate) => gate.harness === 'docling').status;

  // First run: every fixture judged, the owner's one accepted failure: recorded and committed.
  const firstHead = await fixtureGit(repo, 'rev-parse', 'HEAD');
  const first = await realDoclingReceipt({ git_sha: firstHead, inputs: DOCLING_INPUTS, produced_at }, {}, {}, { spaced: true });
  await artifact(first);
  const recordedFirst = await recordCommand('docling');
  assert.equal(recordedFirst.code, 0, recordedFirst.stderr);
  assert.deepEqual(JSON.parse(await readFile(recordedFile, 'utf8')), first);
  assert.equal(await typedStatus(), 'accepted_with_limitations');
  const afterFirst = await fixtureCommit(repo, {}, 'record the first run');
  assert.match(await checkReceipts(repo), /docling-library-qualification: accepted_with_limitations/);

  // Second run, a later commit: a single-fixture run that did not judge everything derives another status.
  const second = await realDoclingReceipt({ git_sha: afterFirst, inputs: DOCLING_INPUTS, produced_at: '2026-10-06T10:00:00.000Z' }, {}, {}, { spaced: true, only: ['corpus/redp5110_sampled.pdf'] });
  assert.notDeepEqual(second, first);
  await artifact(second);
  const recordedSecond = await recordCommand('docling');
  assert.equal(recordedSecond.code, 0, recordedSecond.stderr);
  assert.match(recordedSecond.stdout, /^recorded .*receipt\.json -> .*docling\.json$/m);
  assert.deepEqual(JSON.parse(await readFile(recordedFile, 'utf8')), second, 'the receipt file is the second receipt, not the first');
  assert.equal(await typedStatus(), 'incomplete', 'the typed status follows the second receipt');
  assert.deepEqual(derivedStatuses(await derivedRecord(repo)).docling, 'incomplete');
  await fixtureCommit(repo, {}, 'record the second run over the first');
  assert.match(await checkReceipts(repo), /docling-library-qualification: incomplete \(.*did not judge everything/);
  assert.deepEqual(await staleReceiptLines(repo, await fixtureGit(repo, 'rev-parse', 'HEAD')), []);
});

test('the Docling receipt a run writes holds no path of this machine: repository, temp and unmapped forms only', async (t) => {
  const { receipt, written, outDir } = await doclingQualify(t);
  const text = JSON.stringify(written);
  assert.deepEqual(written, JSON.parse(JSON.stringify(receipt)), 'the file is the receipt that was judged');
  for (const [name, absolute] of [['the repository', root], ['the temp directory', tmpdir()], ['the output directory', outDir]]) {
    assert.ok(!text.includes(absolute.replaceAll('\\', '\\\\')) && !text.includes(absolute.replaceAll('\\', '/')), `${name} ${absolute} is not in the receipt`);
  }
  assert.doesNotMatch(text, /(?<![A-Za-z0-9])[A-Za-z]:(?:\\|\/(?!\/))/, 'no drive path');
  assert.doesNotMatch(text, /"\.\.[\\/]/, 'no path climbs out of the repository');
  assert.match(written.assets_verified.manifest, /^<tmp>\/okf-docling-run-[^/]+\/assets\.json$/, 'the manifest outside the repository is mapped like any other path');
  assert.equal(written.settings.environment.OKF_DOCLING_FIXTURES, 'tests/fixtures/documents');
  assert.match(written.paths.per_fixture_evidence, /^<tmp>\/okf-docling-run-[^/]+\/partial$/);
  assert.deepEqual(written.paths.unmapped.filter((path) => path.endsWith('layout.onnx')), ['<abs>/models/layout.onnx']);
  assert.equal(written.settings.environment.DOCLING_RS_MODELS_DIR, '<abs>/models', 'a directory outside every root keeps its last segments');
  assert.match(written.assets_verified.native_libraries[0].read_from[1], /^<abs>\/[^/]+\/dist\.tsv$/);
});

test('the MCP Apps receipt a run writes holds repository-relative paths and lists what fits no root', async () => {
  const { digest } = await goodObservations();
  const double = await qualifyDouble({
    change: {
      buildHarness: async () => ({ path: join(root, 'target', 'release', 'okf-qualify-mcp-apps.exe'), bytes: 1, sha256: 'b'.repeat(64), args: [] }),
      checkHarness: async () => ({
        code: 0,
        report: { resources: [{ name: 'app', readable: true, path: join(root, 'ui', 'dist-apps', 'app.html') }, { name: 'x', readable: true, path: join('E:', 'elsewhere', 'a', 'b.html') }], dataset: { sha256: digest } },
        stderr: '',
      }),
    },
  });
  const { receipt } = await double.run();
  assert.equal(receipt.harness_binary.path, 'target/release/okf-qualify-mcp-apps.exe');
  assert.deepEqual(receipt.check.resources.map((resource) => resource.path), ['ui/dist-apps/app.html', '<abs>/a/b.html']);
  assert.deepEqual(receipt.paths, { unmapped: ['<abs>/a/b.html'] });
  assert.equal(receipt.harness_binary.sha256, 'b'.repeat(64));
  assert.deepEqual(double.written[0], JSON.parse(JSON.stringify(receipt)), 'the file is the receipt that was judged');
  assert.ok(!JSON.stringify(receipt).includes(root.replaceAll('\\', '\\\\')), 'the repository path is not in the receipt');
});

// The path rule of every receipt (scripts/lib/provenance.mjs scrubReceiptPaths); each test fails when its rule is removed.
const pathsWindows = pathContext({ root: 'D:\\work\\okf-jawn', home: 'C:\\Users\\eayou', tmp: 'C:\\Users\\eayou\\AppData\\Local\\Temp' });
const pathsPosix = pathContext({ root: '/srv/okf-jawn', home: '/home/dev', tmp: '/tmp' });
const sha256Of = text => createHash('sha256').update(text).digest('hex');

test('a path inside the repository becomes repo-relative with forward slashes, in either separator style', () => {
  assert.deepEqual(mapReceiptPath('D:\\work\\okf-jawn\\tests\\fixtures\\documents\\x.pdf', pathsWindows), { path: 'tests/fixtures/documents/x.pdf', mapped: true });
  assert.deepEqual(mapReceiptPath('D:/work/okf-jawn/target/release/okf.exe', pathsWindows), { path: 'target/release/okf.exe', mapped: true });
  assert.deepEqual(mapReceiptPath('d:\\WORK\\okf-jawn\\ui\\dist-apps\\app.html', pathsWindows), { path: 'ui/dist-apps/app.html', mapped: true }, 'a Windows drive path is compared without case');
  assert.deepEqual(mapReceiptPath('D:\\work\\okf-jawn', pathsWindows), { path: '.', mapped: true });
  assert.deepEqual(mapReceiptPath('/srv/okf-jawn/api/mcp-tools.json', pathsPosix), { path: 'api/mcp-tools.json', mapped: true });
  assert.deepEqual(mapReceiptPath('/srv/Okf-Jawn/api', pathsPosix), { path: '<abs>/Okf-Jawn/api', mapped: false }, 'a POSIX path is compared with case');
});

test('a path inside the home directory becomes ~/..., and the temp directory is judged before the home it sits in', () => {
  assert.deepEqual(mapReceiptPath('C:\\Users\\eayou\\.cache\\okf-jawn\\docling\\models\\layout.onnx', pathsWindows), { path: '~/.cache/okf-jawn/docling/models/layout.onnx', mapped: true });
  assert.deepEqual(mapReceiptPath('C:\\Users\\eayou', pathsWindows), { path: '~', mapped: true });
  assert.deepEqual(mapReceiptPath('/home/dev/.cargo/registry', pathsPosix), { path: '~/.cargo/registry', mapped: true });
  assert.deepEqual(mapReceiptPath('C:\\Users\\eayou\\AppData\\Local\\Temp\\.tmpAB12', pathsWindows), { path: '<tmp>/.tmpAB12', mapped: true });
  assert.deepEqual(mapReceiptPath('/tmp/.tmpAB12/partial', pathsPosix), { path: '<tmp>/.tmpAB12/partial', mapped: true });
});

test('a repository inside the home directory is repo-relative, not ~', () => {
  const nested = pathContext({ root: 'C:\\Users\\eayou\\code\\okf-jawn', home: 'C:\\Users\\eayou', tmp: 'C:\\Windows\\Temp' });
  assert.deepEqual(mapReceiptPath('C:\\Users\\eayou\\code\\okf-jawn\\api\\x.json', nested), { path: 'api/x.json', mapped: true });
  assert.deepEqual(mapReceiptPath('C:\\Users\\eayou\\code\\other', nested), { path: '~/code/other', mapped: true });
});

test('a path that only shares a prefix with a root is not inside it', () => {
  assert.deepEqual(mapReceiptPath('C:\\Users\\eayou2\\secret\\file.txt', pathsWindows), { path: '<abs>/secret/file.txt', mapped: false });
  assert.deepEqual(mapReceiptPath('D:\\work\\okf-jawn-old\\a\\b.txt', pathsWindows), { path: '<abs>/a/b.txt', mapped: false });
  assert.deepEqual(mapReceiptPath('/home/dev2/x/y', pathsPosix), { path: '<abs>/x/y', mapped: false });
  assert.deepEqual(mapReceiptPath('/srv/okf-jawn2/x', pathsPosix), { path: '<abs>/okf-jawn2/x', mapped: false });
});

test('any other absolute path keeps its last two segments and is listed in paths.unmapped', () => {
  assert.deepEqual(mapReceiptPath('E:\\tools\\chromium-1243\\chrome-win\\chrome.exe', pathsWindows), { path: '<abs>/chrome-win/chrome.exe', mapped: false });
  assert.deepEqual(mapReceiptPath('/opt/onnx/lib/libonnxruntime.so', pathsPosix), { path: '<abs>/lib/libonnxruntime.so', mapped: false });
  const out = scrubReceiptPaths({ a: 'E:\\tools\\x\\y.exe', b: ['/opt/onnx/lib/z.so', 'E:/tools/x/y.exe'] }, pathsWindows);
  assert.deepEqual(out.paths.unmapped, ['<abs>/lib/z.so', '<abs>/x/y.exe']);
  assert.deepEqual(scrubReceiptPaths({ ok: 'plain' }, pathsWindows).paths, { unmapped: [] }, 'the list is always present, empty when everything mapped');
});

test('every string in a receipt is rewritten: values, keys, nesting, Debug text with escaped separators and verbatim prefixes', () => {
  const receipt = {
    git_sha: 'a'.repeat(40),
    settings: {
      environment: { DOCLING_RS_MODELS_DIR: 'C:\\Users\\eayou\\.cache\\okf-jawn\\docling\\models', OKF_DOCLING_FIXTURES: 'D:\\work\\okf-jawn\\tests\\fixtures\\documents' },
      converter_debug: 'ConverterOptions { artifacts_dir: Some("C:\\\\Users\\\\eayou\\\\AppData\\\\Local\\\\Temp\\\\.tmpQ1"), models: "C:\\\\Users\\\\eayou\\\\.cache" }',
    },
    receipts: [{ path: 'D:\\work\\okf-jawn\\tests\\fixtures\\documents\\a.pdf', note: 'wrote \\\\?\\D:\\work\\okf-jawn\\out.json.' }],
    by_name: { 'D:\\work\\okf-jawn\\x.png': 1 },
    pathsPosix: 'read /home/dev/.cargo/x and (/tmp/.tmpZ).',
  };
  const out = scrubReceiptPaths(receipt, pathsWindows);
  assert.equal(out.settings.environment.DOCLING_RS_MODELS_DIR, '~/.cache/okf-jawn/docling/models');
  assert.equal(out.settings.environment.OKF_DOCLING_FIXTURES, 'tests/fixtures/documents');
  assert.equal(out.settings.converter_debug, 'ConverterOptions { artifacts_dir: Some("<tmp>/.tmpQ1"), models: "~/.cache" }');
  assert.deepEqual(out.receipts, [{ path: 'tests/fixtures/documents/a.pdf', note: 'wrote out.json.' }]);
  assert.deepEqual(Object.keys(out.by_name), ['x.png']);
  assert.equal(out.pathsPosix, 'read <abs>/.cargo/x and (<abs>/tmp/.tmpZ).', 'a POSIX path outside the roots of this machine is rewritten too');
  assert.equal(out.git_sha, receipt.git_sha);
  assert.doesNotMatch(JSON.stringify(out), /eayou|[A-Za-z]:\\\\|[A-Za-z]:\//);
});

test('POSIX paths under the roots of this machine are rewritten inside a sentence', () => {
  const out = scrubReceiptPaths({ note: 'read /home/dev/.cargo/x and (/tmp/.tmpZ). Also /srv/okf-jawn/api/a.json, /opt/x/y/z.' }, pathsPosix);
  assert.equal(out.note, 'read ~/.cargo/x and (<tmp>/.tmpZ). Also api/a.json, <abs>/y/z.');
  assert.deepEqual(out.paths.unmapped, ['<abs>/y/z']);
});

test('hashes, lengths, URLs, schemes and relative paths are not touched', () => {
  const receipt = {
    sha256: sha256Of('x'), bytes: 4096, ratio: 0.5, flag: true, none: null,
    url: 'http://127.0.0.1:18765/mcp', cdn: 'https://cdn.pyke.io/a/b.tgz', resource: 'ui://okf-jawn/app.html',
    relative: 'tests/fixtures/documents/x.pdf', route: '/mcp', time: '2026-10-06T10:00:00.000Z', path_like_text: 'api/mcp-tools.json',
  };
  const { paths, ...rest } = scrubReceiptPaths(receipt, pathsWindows);
  assert.deepEqual(rest, receipt);
  assert.deepEqual(paths, { unmapped: [] });
});

test('scrubbing twice changes nothing more', () => {
  const once = scrubReceiptPaths({ a: 'C:\\Users\\eayou\\x', b: 'E:\\t\\u\\v' }, pathsWindows);
  assert.deepEqual(scrubReceiptPaths(once, pathsWindows), once);
});
