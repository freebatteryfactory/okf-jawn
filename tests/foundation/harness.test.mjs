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
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
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
import { killProcessTree, spawnGroup, waitForListening } from '../../qualification/mcp-apps/lib/process.mjs';
import {
  APP_ONLY_TOOLS,
  PRESENT_DATASET,
  TOOL_CALL_LOG_PREFIX,
  UPSTREAM_HOST_RULES,
  VIEWS,
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

/** A fixture process that behaved: exit 0, measured, receipt on disk. `over` replaces parts. */
function doclingRun(only, over = {}) {
  const fixture = {
    fixture: only,
    outcome: only === MUST_FAIL ? 'PASS_explicit_failure' : 'PASS',
    stage: only === MUST_FAIL ? 'converter_error' : 'converter_status',
    status: only === MUST_FAIL ? null : only === TIMEOUT_PROBE ? 'PartialSuccess' : 'Success',
    finding: null,
    peak_rss_bytes: null,
    ...over.fixture,
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
          ? { receipts: [], timeout_case: fixture }
          : { receipts: [fixture], timeout_case: null },
  };
}

function doclingReceipt(overrides = {}) {
  const runs = FIXTURE_RUNS.map((only) => doclingRun(only, overrides[only]));
  return buildDoclingReceipt({
    header: DOCLING_HEADER,
    converter: DOCLING_CONVERTER,
    platform: 'win32',
    modelsDir: 'models',
    runs,
    finishedAt: '2026-10-05T18:10:00.000Z',
  });
}

test('a Docling receipt carries the shared header at its top level and passes when every rule holds', () => {
  const receipt = doclingReceipt();
  assert.deepEqual(receiptHeaderProblems(receipt), []);
  assert.deepEqual(Object.keys(receipt).slice(0, 3), ['git_sha', 'inputs', 'produced_at']);
  assert.equal(receipt.orchestrator, undefined);
  assert.equal(receipt.result, 'PASS');
  assert.equal(receipt.receipts.length, FIXTURE_RUNS.length - 1);
  assert.equal(receipt.timeout_case.outcome, 'PASS');
  assert.deepEqual(Object.keys(receipt.summary), FIXTURE_RUNS);
});

test('a fixture whose peak was not measured fails the memory criterion without throwing', () => {
  const receipt = doclingReceipt({
    'sample_sheet.xlsx': { run: { peakRssBytes: null, peakRssNote: 'Get-Process reported no PeakWorkingSet64' } },
  });
  const entry = receipt.receipts.find((item) => item.fixture === 'sample_sheet.xlsx');
  assert.equal(entry.peak_rss_bytes, null);
  assert.equal(entry.peak_rss_note, 'Get-Process reported no PeakWorkingSet64');
  assert.equal(entry.memory, 'FAIL_not_measured');
  assert.equal(entry.outcome, 'PASS');
  assert.equal(receipt.memory_summary['sample_sheet.xlsx'], 'FAIL_not_measured');
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
      },
    },
  });
  assert.equal(receipt.finding, 'converter accepts truncated PDF');
  assert.equal(receipt.summary[MUST_FAIL], 'FAIL_expected_failure');
  assert.equal(receipt.result, 'FAIL');
});

test('a refusal counts only when another PDF converted in the same run', () => {
  const pdfs = FIXTURE_RUNS.filter((only) => only.endsWith('.pdf') && only !== MUST_FAIL);
  const overrides = Object.fromEntries(
    pdfs.map((only) => [only, { fixture: { outcome: 'FAIL_converter_error', stage: 'converter_error', status: null } }]),
  );
  const receipt = doclingReceipt(overrides);
  assert.equal(receipt.summary[MUST_FAIL], 'FAIL_refusal_unproven');
  assert.match(receipt.receipts.find((item) => item.fixture === MUST_FAIL).refusal_note, /no other PDF fixture converted/);
  assert.equal(receipt.result, 'FAIL');
});

test('a fixture process that wrote no receipt, or never ran, is a harness FAIL', () => {
  const crashed = doclingReceipt({
    'table_heavy.pdf': { report: null, run: { exitCode: 1, stderr: 'okf-qualify-docling: missing fixture' } },
  });
  const entry = crashed.receipts.find((item) => item.fixture === 'table_heavy.pdf');
  assert.equal(entry.outcome, 'FAIL_harness');
  assert.equal(entry.harness_exit_code, 1);
  assert.match(entry.harness_error, /missing fixture/);
  assert.equal(crashed.result, 'FAIL');

  const partial = buildDoclingReceipt({
    header: DOCLING_HEADER,
    converter: DOCLING_CONVERTER,
    platform: 'win32',
    modelsDir: 'models',
    runs: [doclingRun('sample_sheet.xlsx')],
    finishedAt: '2026-10-05T18:10:00.000Z',
  });
  assert.equal(partial.summary[TIMEOUT_PROBE], 'FAIL_not_run');
  assert.equal(partial.result, 'FAIL');
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

test('all three orchestrators take their header from receiptHeader and record only through recordReceipt', async () => {
  for (const name of ['docling', 'mcp-apps', 'iii']) {
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
