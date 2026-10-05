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

const root = fileURLToPath(new URL('../../', import.meta.url));

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
 reviewCoversRevision(await (await fetch(url)).json(),revision);
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
  const result = await within(runFixtureProcess({ command: process.execPath, args: ['-e', ''] }), 4000, 'instant exit');
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
    4000,
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
    4000,
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
    4000,
    'exit during sample',
  );
  assert.equal(result.exitCode, 0);
  assert.equal(result.peakRssBytes, null);
  assert.equal(result.peakRssNote, 'process gone');
});

test('a converter that never finishes is killed at the fixture timeout', async () => {
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', 'setInterval(()=>{},1000)'], timeoutMs: 300 }),
    4000,
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
    4000,
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
