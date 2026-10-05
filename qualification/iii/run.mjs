/**
 * Orchestrate Phase 0 iii qualification.
 * Downloads pinned engine (+ iii-worker), starts compose + queue (file_based),
 * runs crash Test 1 (redelivery) and Test 2 (idempotency) by killing before Ok,
 * exercises DLQ and channel blob refs when APIs allow, writes receipt.json.
 */

import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { access, mkdir, readFile, rm, writeFile, unlink } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { homedir, platform } from 'node:os';
import { pipeline } from 'node:stream/promises';
import { Readable } from 'node:stream';
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const outDir = join(root, '.artifacts/qualification/iii');
const receiptPath = join(outDir, 'receipt.json');
const ENGINE_TAG = 'iii/v0.24.4';
const ENGINE_VERSION = '0.24.4';
const QUEUE_VERSION = '0.21.17';
const RELEASE_BASE = `https://github.com/iii-hq/iii/releases/download/${ENGINE_TAG}`;
const CACHE_ROOT = join(homedir(), '.cache', 'okf-jawn', 'iii', `v${ENGINE_VERSION}`);
const FALLBACK = {
  executor: 'Tokio+RecordStore',
  meaning: 'In-process Tokio worker over durable RecordStore; iii not adopted',
};

const GATE_BEFORE_WORK = 'GATE_BEFORE_WORK';
const GATE_AFTER_EFFECT = 'GATE_AFTER_EFFECT';
const FAIL_GATE = 'FAIL_GATE';

const ELV2_NOTES = {
  license: 'Elastic License 2.0',
  source: `https://github.com/iii-hq/iii/blob/${ENGINE_TAG}/engine/LICENSE`,
  reviewed_text_sha256: null,
  source_ref: null,
  deployment_implications: [
    'ELv2 forbids providing the software to third parties as a hosted/managed service that exposes a substantial set of iii engine features.',
    'Self-hosted / single-tenant use of the engine as an internal dependency is the intended qualification posture for okf-jawn.',
    'License key circumvention and notice removal are forbidden.',
    'SDK packages are Apache-2.0; the engine binary is ELv2.',
  ],
};

async function exists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

function sha256Buffer(buf) {
  return createHash('sha256').update(buf).digest('hex');
}

async function sha256File(path) {
  return sha256Buffer(await readFile(path));
}

function run(command, args, options = {}) {
  const { cwd = root, env = process.env, timeout = 120_000, input } = options;
  return new Promise((resolveRun, reject) => {
    const child = spawn(command, args, {
      cwd,
      env,
      stdio: [input === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'],
      windowsHide: true,
    });
    let stdout = '';
    let stderr = '';
    let settled = false;
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      if (!settled) {
        settled = true;
        reject(new Error(`${command} timed out after ${timeout}ms`));
      }
    }, timeout);
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', (error) => {
      clearTimeout(timer);
      if (!settled) {
        settled = true;
        reject(error);
      }
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      if (!settled) {
        settled = true;
        resolveRun({ code: code ?? 1, stdout, stderr });
      }
    });
    if (input !== undefined) {
      child.stdin.end(input);
    }
  });
}

async function download(url, dest) {
  if (await exists(dest)) return;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`download ${url} failed: ${response.status}`);
  await mkdir(resolve(dest, '..'), { recursive: true });
  await pipeline(Readable.fromWeb(response.body), createWriteStream(dest));
}

async function ensureCacheLinux() {
  const assets = {
    engine: 'iii-x86_64-unknown-linux-gnu.tar.gz',
    engineSha: 'iii-x86_64-unknown-linux-gnu.sha256',
    worker: 'iii-worker-x86_64-unknown-linux-gnu.tar.gz',
    workerSha: 'iii-worker-x86_64-unknown-linux-gnu.sha256',
  };
  const measured = {};
  await mkdir(CACHE_ROOT, { recursive: true });
  for (const [key, name] of Object.entries(assets)) {
    const dest = join(CACHE_ROOT, name);
    await download(`${RELEASE_BASE}/${name}`, dest);
    measured[key] = { file: name, sha256: await sha256File(dest), path: dest };
  }
  const engineExpected = (await readFile(join(CACHE_ROOT, assets.engineSha), 'utf8')).trim().split(/\s+/)[0];
  const workerExpected = (await readFile(join(CACHE_ROOT, assets.workerSha), 'utf8')).trim().split(/\s+/)[0];
  if (measured.engine.sha256 !== engineExpected) {
    throw new Error(`engine sha mismatch: got ${measured.engine.sha256} expected ${engineExpected}`);
  }
  if (measured.worker.sha256 !== workerExpected) {
    throw new Error(`worker sha mismatch: got ${measured.worker.sha256} expected ${workerExpected}`);
  }
  // Extract onto the WSL native filesystem. Compose fails on /mnt/c Drvfs paths
  // when creating engine-config.yaml (IO_ERROR / missing file).
  const extract = await run('wsl', [
    '-e',
    'bash',
    '-lc',
    [
      'set -euo pipefail',
      `WIN_CACHE=$(wslpath '${CACHE_ROOT.replace(/'/g, `'\\''`)}')`,
      'NATIVE="$HOME/.cache/okf-jawn/iii/v0.24.4"',
      'mkdir -p "$NATIVE/bin"',
      'cp -f "$WIN_CACHE"/iii-x86_64-unknown-linux-gnu.tar.gz "$NATIVE/"',
      'cp -f "$WIN_CACHE"/iii-worker-x86_64-unknown-linux-gnu.tar.gz "$NATIVE/"',
      'tar -xzf "$NATIVE/iii-x86_64-unknown-linux-gnu.tar.gz" -C "$NATIVE/bin"',
      'tar -xzf "$NATIVE/iii-worker-x86_64-unknown-linux-gnu.tar.gz" -C "$NATIVE/bin"',
      'test -x "$NATIVE/bin/iii" && test -x "$NATIVE/bin/iii-worker"',
      '"$NATIVE/bin/iii" --version',
      'echo NATIVE_CACHE=$NATIVE',
    ].join('\n'),
  ], { timeout: 180_000 });
  if (extract.code !== 0) {
    throw new Error(`extract failed: ${extract.stderr || extract.stdout}`);
  }
  const nativeMatch = (extract.stdout || '').match(/NATIVE_CACHE=(.+)/);
  const nativeCache = nativeMatch ? nativeMatch[1].trim() : '$HOME/.cache/okf-jawn/iii/v0.24.4';
  return {
    os: 'linux-wsl',
    cache_windows: CACHE_ROOT,
    cache_wsl_native: nativeCache,
    measured,
    iii_version: ENGINE_VERSION,
  };
}

async function fetchElv2() {
  const response = await fetch(
    `https://raw.githubusercontent.com/iii-hq/iii/refs/tags/${ENGINE_TAG}/engine/LICENSE`,
  );
  if (!response.ok) throw new Error(`ELv2 fetch failed: ${response.status}`);
  const text = await response.text();
  return { text, sha256: sha256Buffer(Buffer.from(text, 'utf8')), source_ref: `refs/tags/${ENGINE_TAG}` };
}

async function writeComposeWsl(nativeWorkdir) {
  const compose = [
    'namespace: okf-qualify',
    '',
    'engine:',
    '  url: ws://127.0.0.1:49134',
    '',
    'containers:',
    '  queue:',
    '    worker: package://api.workers.iii.dev/queue',
    `    version: "${QUEUE_VERSION}"`,
    '    config_override:',
    '      adapter:',
    '        name: builtin',
    '        config:',
    '          store_method: file_based',
    '          file_path: data/queue',
    '          save_interval_ms: 5000',
    '',
  ].join('\n');
  const result = await wslBash(
    [
      `mkdir -p "${nativeWorkdir}"`,
      `cat > "${nativeWorkdir}/worker-compose.yaml" <<'EOF'`,
      compose,
      'EOF',
    ].join('\n'),
  );
  if (result.code !== 0) {
    throw new Error(`write compose failed: ${result.stderr || result.stdout}`);
  }
}

async function wslBash(script, options = {}) {
  const wrapped = `set -euo pipefail\n${script}`;
  return run('wsl', ['-e', 'bash', '-lc', wrapped], { timeout: options.timeout ?? 180_000 });
}

async function startCompose(nativeWorkdir, nativeCache) {
  // Kill leftovers in a separate WSL invocation. Do not pkill from inside the
  // start script: the pattern matches this script's own argv and self-kills.
  await stopCompose(nativeWorkdir);
  // Detach compose from this short-lived WSL shell so exiting the waiter does
  // not SIGHUP the engine (seen as "startup interrupted").
  const launch = await run(
    'wsl',
    [
      '-e',
      'bash',
      '-lc',
      [
        'set -eu',
        `BIN="${nativeCache}/bin/iii"`,
        'test -x "$BIN"',
        `mkdir -p "${nativeWorkdir}"`,
        `cd "${nativeWorkdir}"`,
        'rm -f compose.pid compose.log',
        // nohup+setsid: survive shell exit. stdbuf -oL: file redirects otherwise
        // fully-buffer compose status lines so readiness greps hang forever.
        `nohup setsid stdbuf -oL -eL "$BIN" --no-update-check compose --up --namespace okf-qualify >compose.log 2>&1 < /dev/null &`,
        'echo $! >compose.pid',
        'sleep 2',
        'if ! kill -0 "$(cat compose.pid)" 2>/dev/null; then',
        '  echo LAUNCH_DEAD',
        '  cat compose.log || true',
        '  exit 1',
        'fi',
        'echo LAUNCHED=$(cat compose.pid)',
      ].join('\n'),
    ],
    { timeout: 30_000 },
  );
  if (launch.code !== 0) {
    throw new Error(`compose launch failed: ${launch.stderr || launch.stdout}`);
  }
  let queueReady = false;
  let log = '';
  for (let i = 0; i < 90; i += 1) {
    const logResult = await wslBash(`cat "${nativeWorkdir}/compose.log" 2>/dev/null || true`, {
      timeout: 10_000,
    });
    log = logResult.stdout || '';
    if (log.includes('queue ready') || /up: 1 of 1 changed/.test(log)) {
      queueReady = true;
      break;
    }
    // API probe: do not depend solely on log flushing.
    const workers = await iiiTrigger(
      nativeCache,
      'engine::workers::list',
      {},
      { namespace: 'default', allowFailure: true, timeout: 10_000 },
    );
    const names = Array.isArray(workers.json?.workers)
      ? workers.json.workers.map((worker) => worker.name)
      : [];
    if (names.includes('queue')) {
      queueReady = true;
      break;
    }
    if (log.includes('startup interrupted') || log.includes('Engine Failed')) {
      throw new Error(`compose failed early; compose.log:\n${log}`);
    }
    await sleep(1000);
  }
  if (!queueReady) {
    throw new Error(`queue worker not ready; compose.log:\n${log}`);
  }
  let probe = null;
  for (let i = 0; i < 30; i += 1) {
    probe = await iiiTrigger(
      nativeCache,
      'iii::durable::publish',
      { topic: 'okf.qualify.warmup', data: { ping: true } },
      { allowFailure: true },
    );
    if (probe.code === 0) break;
    await sleep(1000);
  }
  if (!probe || probe.code !== 0) {
    throw new Error(`iii::durable::publish unavailable: ${probe?.stderr || probe?.stdout || 'no probe'}`);
  }
  return { probe };
}

async function stopCompose(nativeWorkdir) {
  await run(
    'wsl',
    [
      '-e',
      'bash',
      '-lc',
      [
        'set +e',
        `if [ -d "${nativeWorkdir}" ]; then cd "${nativeWorkdir}"; fi`,
        'if [ -f compose.pid ]; then',
        '  pid=$(cat compose.pid)',
        '  kill "$pid" 2>/dev/null',
        '  sleep 1',
        '  kill -9 "$pid" 2>/dev/null',
        '  # also kill engine child recorded in logs if still around',
        '  pkill -P "$pid" 2>/dev/null',
        'fi',
        // Character-class trick avoids matching this pkill command line itself.
        'pkill -f "[i]ii --no-update-check compose" 2>/dev/null',
        'pkill -f "[.]iii/compose/okf-qualify" 2>/dev/null',
        'sleep 1',
        'true',
      ].join('\n'),
    ],
    { timeout: 30_000 },
  );
}

async function iiiTrigger(nativeCache, functionId, payload, options = {}) {
  const ns = options.namespace ?? 'okf-qualify';
  const args = [
    '-e',
    'bash',
    '-lc',
    [
      `export PATH="${nativeCache}/bin:$PATH"`,
      `iii --no-update-check trigger -n ${ns} --engine ws://127.0.0.1:49134 --json '${JSON.stringify(payload).replace(/'/g, `'\\''`)}' ${functionId}`,
    ].join('\n'),
  ];
  const result = await run('wsl', args, { timeout: options.timeout ?? 60_000 });
  if (result.code !== 0 && !options.allowFailure) {
    throw new Error(`trigger ${functionId} failed: ${result.stderr || result.stdout}`);
  }
  let json = null;
  const text = (result.stdout || '').trim();
  if (text) {
    try {
      json = JSON.parse(text);
    } catch {
      json = text;
    }
  }
  return { ...result, json };
}

async function waitForFile(path, timeoutMs = 30_000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (await exists(path)) return true;
    await new Promise((r) => setTimeout(r, 50));
  }
  return false;
}

async function sleep(ms) {
  await new Promise((r) => setTimeout(r, ms));
}

async function buildWorker() {
  const result = await run('cargo', ['build', '--locked', '-p', 'okf-qualify-iii', '--release'], {
    timeout: 600_000,
  });
  if (result.code !== 0) {
    throw new Error(`cargo build failed:\n${result.stderr || result.stdout}`);
  }
  const bin = join(root, 'target', 'release', platform() === 'win32' ? 'okf-qualify-iii.exe' : 'okf-qualify-iii');
  if (!(await exists(bin))) throw new Error(`missing worker binary ${bin}`);
  return bin;
}

function startQualifyWorker(bin, controlDir, markerDir) {
  return spawn(bin, [], {
    cwd: root,
    env: {
      ...process.env,
      III_URL: 'ws://127.0.0.1:49134',
      III_NAMESPACE: 'okf-qualify',
      OKF_III_CONTROL_DIR: controlDir,
      OKF_III_MARKER_DIR: markerDir,
    },
    stdio: 'ignore',
    windowsHide: true,
  });
}

async function killWorker(child) {
  if (!child || child.exitCode !== null) return;
  try {
    child.kill('SIGKILL');
  } catch {
    // ignore
  }
  await sleep(300);
}

/** Kill the qualify worker and the WSL engine+queue compose, then restart compose. */
async function killEngineStack(worker, nativeWorkdir, nativeCache) {
  await killWorker(worker);
  await stopCompose(nativeWorkdir);
  await sleep(500);
  await startCompose(nativeWorkdir, nativeCache);
}

async function readEffectLedger(markerDir, jobId) {
  const path = join(markerDir, `effect_ledger_${jobId}`);
  if (!(await exists(path))) return [];
  const text = await readFile(path, 'utf8');
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => line === 'true');
}

async function writeCrashBlob(markerDir, jobId) {
  const payload = `okf-iii-blob-${jobId}-v1`;
  const path = join(markerDir, `blob_${jobId}.bin`);
  await writeFile(path, payload);
  const sha256 = sha256Buffer(Buffer.from(payload, 'utf8'));
  return { path, sha256, payload };
}

async function rssForCompose(nativeWorkdir) {
  const result = await wslBash(
    [
      `cd "${nativeWorkdir}"`,
      'pid=$(cat compose.pid 2>/dev/null || true)',
      'engine_pid=$(pgrep -f "/bin/iii" | head -n 1 || true)',
      'queue_pid=$(pgrep -af queue | awk "/api.workers.iii.dev\\/queue|iii-worker.*queue|name=queue/ {print \\$1; exit}" || true)',
      'rss() { if [ -n "$1" ] && [ -r "/proc/$1/status" ]; then awk "/VmRSS:/ {print \\$2*1024}" "/proc/$1/status"; else echo null; fi; }',
      'echo COMPOSE_PID=$pid',
      'echo ENGINE_PID=$engine_pid',
      'echo QUEUE_PID=$queue_pid',
      'echo COMPOSE_RSS=$(rss "$pid")',
      'echo ENGINE_RSS=$(rss "$engine_pid")',
      'echo QUEUE_RSS=$(rss "$queue_pid")',
    ].join('\n'),
    { timeout: 20_000 },
  );
  const out = result.stdout || '';
  const pick = (name) => {
    const match = out.match(new RegExp(`${name}=(\\d+|null)`));
    if (!match || match[1] === 'null') return null;
    return Number(match[1]);
  };
  return {
    compose_pid: pick('COMPOSE_PID'),
    engine_pid: pick('ENGINE_PID'),
    queue_pid: pick('QUEUE_PID'),
    compose_rss_bytes: pick('COMPOSE_RSS'),
    engine_rss_bytes: pick('ENGINE_RSS'),
    queue_rss_bytes: pick('QUEUE_RSS'),
    raw: out.trim(),
  };
}

async function countLines(path) {
  if (!(await exists(path))) return 0;
  const text = await readFile(path, 'utf8');
  return text.split(/\r?\n/).filter(Boolean).length;
}

async function writeReceipt(receipt) {
  await mkdir(outDir, { recursive: true });
  await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
}

async function main() {
  const header = await receiptHeader(root, ['qualification/iii', 'Cargo.toml', 'Cargo.lock']);
  await mkdir(outDir, { recursive: true });
  const started = new Date().toISOString();
  // Gate/marker dirs stay on the Windows filesystem for the Windows-built worker.
  const controlDir = join(CACHE_ROOT, 'qualify-control');
  const markerDir = join(CACHE_ROOT, 'qualify-markers');
  let decision = 'REJECTED_WITH_FALLBACK';
  let reason = 'not started';
  let cacheInfo = null;
  let elv2 = null;
  let tests = {};
  let memory = null;
  let ranOs = platform();
  let nativeWorkdir = null;
  let nativeCache = null;
  const blockers = [];

  try {
    const wslCheck = await run('wsl', ['-e', 'bash', '-lc', 'uname -s'], { timeout: 30_000 });
    if (wslCheck.code !== 0) {
      throw new Error('WSL is required: Windows release has no iii-worker/queue binary');
    }
    ranOs = `wsl:${(wslCheck.stdout || '').trim() || 'Linux'}`;

    elv2 = await fetchElv2();
    ELV2_NOTES.reviewed_text_sha256 = elv2.sha256;
    ELV2_NOTES.source_ref = elv2.source_ref;
    if (!elv2.text.includes('Elastic License 2.0')) {
      throw new Error('ELv2 license text did not contain expected header');
    }

    cacheInfo = await ensureCacheLinux();
    nativeCache = cacheInfo.cache_wsl_native;
    nativeWorkdir = `${nativeCache}/qualify-run`;
    await wslBash(`rm -rf "${nativeWorkdir}" && mkdir -p "${nativeWorkdir}"`);
    await rm(controlDir, { recursive: true, force: true });
    await rm(markerDir, { recursive: true, force: true });
    await mkdir(controlDir, { recursive: true });
    await mkdir(markerDir, { recursive: true });
    await writeComposeWsl(nativeWorkdir);

    const workerBin = await buildWorker();
    const composeStarted = Date.now();
    await startCompose(nativeWorkdir, nativeCache);
    const startupMs = Date.now() - composeStarted;
    memory = { startup_ms: startupMs, before: await rssForCompose(nativeWorkdir) };

    // ---- Test 1: redelivery (kill engine before side effect / before Ok) ----
    await rm(join(controlDir, 'READY'), { force: true });
    await writeFile(join(controlDir, GATE_BEFORE_WORK), '1');
    await unlink(join(controlDir, GATE_AFTER_EFFECT)).catch(() => {});
    const blob1 = await writeCrashBlob(markerDir, 'test1');
    let worker = startQualifyWorker(workerBin, controlDir, markerDir);
    if (!(await waitForFile(join(controlDir, 'READY'), 60_000))) {
      await killWorker(worker);
      throw new Error('qualify worker did not become READY');
    }
    await iiiTrigger(nativeCache, 'iii::durable::publish', {
      topic: 'okf.qualify.import',
      data: { job_id: 'test1', blob_path: blob1.path, blob_sha256: blob1.sha256 },
    });
    if (!(await waitForFile(join(controlDir, 'BLOCKED_GATE_BEFORE_WORK_test1'), 30_000))) {
      await killWorker(worker);
      throw new Error('Test1: worker never blocked on GATE_BEFORE_WORK');
    }
    if (await exists(join(markerDir, 'effect_test1'))) {
      await killWorker(worker);
      throw new Error('Test1: effect marker exists before kill (gate failed)');
    }
    await killEngineStack(worker, nativeWorkdir, nativeCache);
    await unlink(join(controlDir, GATE_BEFORE_WORK)).catch(() => {});
    await unlink(join(controlDir, 'READY')).catch(() => {});
    worker = startQualifyWorker(workerBin, controlDir, markerDir);
    if (!(await waitForFile(join(controlDir, 'READY'), 60_000))) {
      await killWorker(worker);
      throw new Error('Test1: worker restart READY failed');
    }
    const t1Ok = await waitForFile(join(markerDir, 'completed_test1'), 60_000);
    const t1Effect = await exists(join(markerDir, 'effect_test1'));
    const t1Deliveries = await countLines(join(markerDir, 'deliveries_test1'));
    const t1BlobOk = await exists(join(markerDir, 'blob_ok_test1'));
    const t1BlobIntact = (await sha256File(blob1.path)) === blob1.sha256;
    tests.test1_redelivery = {
      completed: t1Ok,
      effect_present: t1Effect,
      deliveries: t1Deliveries,
      kill_point: 'GATE_BEFORE_WORK; engine+queue+worker SIGKILL before Ok',
      blob_verified: t1BlobOk,
      blob_intact: t1BlobIntact,
      passed: Boolean(t1Ok && t1Effect && t1Deliveries >= 2 && t1BlobOk && t1BlobIntact),
    };
    await killWorker(worker);

    // ---- Test 2: idempotency (kill after effect, before Ok) ----
    await rm(join(controlDir, 'READY'), { force: true });
    await unlink(join(controlDir, GATE_BEFORE_WORK)).catch(() => {});
    await writeFile(join(controlDir, GATE_AFTER_EFFECT), '1');
    const blob2 = await writeCrashBlob(markerDir, 'test2');
    worker = startQualifyWorker(workerBin, controlDir, markerDir);
    if (!(await waitForFile(join(controlDir, 'READY'), 60_000))) {
      await killWorker(worker);
      throw new Error('Test2: worker READY failed');
    }
    await iiiTrigger(nativeCache, 'iii::durable::publish', {
      topic: 'okf.qualify.import',
      data: { job_id: 'test2', blob_path: blob2.path, blob_sha256: blob2.sha256 },
    });
    if (!(await waitForFile(join(markerDir, 'effect_test2'), 30_000))) {
      await killWorker(worker);
      throw new Error('Test2: effect marker never created');
    }
    if (!(await waitForFile(join(controlDir, 'BLOCKED_GATE_AFTER_EFFECT_test2'), 30_000))) {
      await killWorker(worker);
      throw new Error('Test2: never blocked on GATE_AFTER_EFFECT before Ok');
    }
    if (await exists(join(markerDir, 'completed_test2'))) {
      await killWorker(worker);
      throw new Error('Test2: completed before kill (acked too early)');
    }
    await killEngineStack(worker, nativeWorkdir, nativeCache);
    await unlink(join(controlDir, GATE_AFTER_EFFECT)).catch(() => {});
    await unlink(join(controlDir, 'READY')).catch(() => {});
    worker = startQualifyWorker(workerBin, controlDir, markerDir);
    if (!(await waitForFile(join(controlDir, 'READY'), 60_000))) {
      await killWorker(worker);
      throw new Error('Test2: restart READY failed');
    }
    const t2Ok = await waitForFile(join(markerDir, 'completed_test2'), 60_000);
    const t2Deliveries = await countLines(join(markerDir, 'deliveries_test2'));
    const t2Effect = await exists(join(markerDir, 'effect_test2'));
    const ledger = await readEffectLedger(markerDir, 'test2');
    const firstTrue = ledger.filter((v) => v === true).length;
    const firstFalse = ledger.filter((v) => v === false).length;
    const t2BlobOk = await exists(join(markerDir, 'blob_ok_test2'));
    const t2BlobIntact = (await sha256File(blob2.path)) === blob2.sha256;
    tests.test2_idempotency = {
      completed: t2Ok,
      effect_present: t2Effect,
      deliveries: t2Deliveries,
      kill_point: 'GATE_AFTER_EFFECT; engine+queue+worker SIGKILL before Ok',
      effect_ledger: ledger,
      first_effect_true_count: firstTrue,
      first_effect_false_count: firstFalse,
      blob_verified: t2BlobOk,
      blob_intact: t2BlobIntact,
      passed: Boolean(
        t2Ok &&
          t2Effect &&
          t2Deliveries >= 2 &&
          firstTrue === 1 &&
          firstFalse >= 1 &&
          t2BlobOk &&
          t2BlobIntact,
      ),
    };
    await killWorker(worker);

    // ---- DLQ fail → inspect → redrive ----
    await rm(join(controlDir, 'READY'), { force: true });
    await writeFile(join(controlDir, FAIL_GATE), '1');
    worker = startQualifyWorker(workerBin, controlDir, markerDir);
    if (!(await waitForFile(join(controlDir, 'READY'), 60_000))) {
      await killWorker(worker);
      throw new Error('DLQ: worker READY failed');
    }
    await iiiTrigger(nativeCache, 'iii::durable::publish', {
      topic: 'okf.qualify.fail',
      data: { job_id: 'dlq1' },
    });
    // max_retries=1 → expect deliveries == 2 before DLQ.
    let failDeliveries = 0;
    for (let i = 0; i < 40; i += 1) {
      failDeliveries = await countLines(join(markerDir, 'deliveries_fail_dlq1'));
      if (failDeliveries >= 2) break;
      await sleep(500);
    }
    let dlqMessages = null;
    let dlqTopics = null;
    let topicStats = null;
    let browseHit = false;
    let messageId = null;
    for (let i = 0; i < 40; i += 1) {
      const stats = await iiiTrigger(
        nativeCache,
        'engine::queue::topic_stats',
        { topic: 'okf.qualify.fail' },
        { namespace: 'default', allowFailure: true },
      );
      const topics = await iiiTrigger(
        nativeCache,
        'engine::queue::dlq_topics',
        {},
        { namespace: 'default', allowFailure: true },
      );
      const listed = await iiiTrigger(
        nativeCache,
        'engine::queue::dlq_messages',
        { topic: 'okf.qualify.fail', offset: 0, limit: 20 },
        { namespace: 'default', allowFailure: true },
      );
      if (stats.code === 0) topicStats = stats.json;
      if (topics.code === 0) dlqTopics = topics.json;
      if (listed.code === 0) dlqMessages = listed.json;
      const arr = Array.isArray(dlqMessages)
        ? dlqMessages
        : dlqMessages?.messages || dlqMessages?.items || [];
      if (Array.isArray(arr) && arr.length > 0) {
        browseHit = true;
        messageId = arr[0]?.id || arr[0]?.message_id || arr[0]?.messageId || null;
        break;
      }
      await sleep(500);
    }
    await unlink(join(controlDir, FAIL_GATE)).catch(() => {});
    let redrive;
    if (messageId) {
      redrive = await iiiTrigger(
        nativeCache,
        'iii::queue::redrive_message',
        { topic: 'okf.qualify.fail', message_id: messageId },
        { allowFailure: true },
      );
    } else {
      redrive = await iiiTrigger(
        nativeCache,
        'iii::queue::redrive',
        { topic: 'okf.qualify.fail' },
        { allowFailure: true },
      );
    }
    const dlqCompleted = await waitForFile(join(markerDir, 'completed_fail_dlq1'), 60_000);
    const redriven = Number(redrive.json?.redriven ?? (redrive.code === 0 && messageId ? 1 : 0));
    tests.dlq = {
      inspected: browseHit,
      browse_hit: browseHit,
      fail_deliveries_before_dlq: failDeliveries,
      expected_deliveries: 2,
      topic_stats: topicStats,
      dlq_topics: dlqTopics,
      dlq_messages: dlqMessages,
      message_id: messageId,
      redrive_api: messageId ? 'iii::queue::redrive_message' : 'iii::queue::redrive',
      redrive_exit: redrive.code,
      redrive_result: redrive.json,
      completed_after_redrive: dlqCompleted,
      passed: Boolean(
        browseHit && failDeliveries >= 2 && redrive.code === 0 && redriven > 0 && dlqCompleted,
      ),
      note: browseHit
        ? 'fail (max_retries=1) → dlq_messages inspect → redrive → success'
        : 'dlq_messages browse empty; cannot claim inspected',
    };
    await killWorker(worker);

    // ---- Channel / blob refs ----
    await rm(join(controlDir, 'READY'), { force: true });
    worker = startQualifyWorker(workerBin, controlDir, markerDir);
    if (!(await waitForFile(join(controlDir, 'READY'), 60_000))) {
      await killWorker(worker);
      throw new Error('channel: worker READY failed');
    }
    const blobPayload = 'okf-iii-channel-blob-v1';
    const expectedSha = sha256Buffer(Buffer.from(blobPayload, 'utf8'));
    const channelSend = await iiiTrigger(
      nativeCache,
      'qualify::channel_send',
      { job_id: 'chan1', payload: blobPayload },
      { allowFailure: true },
    );
    const channelDone = await waitForFile(join(markerDir, 'completed_channel_chan1'), 60_000);
    let channelSha = null;
    const channelShaPath = join(markerDir, 'channel_chan1.sha256');
    if (await exists(channelShaPath)) {
      channelSha = (await readFile(channelShaPath, 'utf8')).trim();
    }
    tests.channel_blob_refs = {
      send_exit: channelSend.code,
      completed: channelDone,
      expected_sha256: expectedSha,
      observed_sha256: channelSha,
      passed: Boolean(channelSend.code === 0 && channelDone && channelSha === expectedSha),
    };
    await killWorker(worker);

    memory = { ...(memory || {}), after: await rssForCompose(nativeWorkdir) };

    const crashPass = tests.test1_redelivery?.passed && tests.test2_idempotency?.passed;
    const extrasPass = tests.dlq?.passed && tests.channel_blob_refs?.passed;
    if (crashPass && extrasPass) {
      decision = 'PASS';
      reason = 'Crash redelivery + idempotency before Ok ack passed; DLQ and channel blob refs passed on WSL with builtin file_based queue';
    } else if (crashPass && !extrasPass) {
      decision = 'REJECTED_WITH_FALLBACK';
      reason = 'Crash tests passed but DLQ and/or channel blob-ref checks did not; selecting Tokio+RecordStore fallback';
      blockers.push('dlq_or_channel_incomplete');
    } else {
      decision = 'REJECTED_WITH_FALLBACK';
      reason = 'Crash tests did not honestly prove redelivery/idempotency before Ok acknowledgement; selecting Tokio+RecordStore fallback';
      blockers.push('crash_tests_failed');
    }
  } catch (error) {
    decision = 'REJECTED_WITH_FALLBACK';
    reason = `Qualification could not complete real crash checks honestly: ${error.message}`;
    blockers.push(error.message);
  } finally {
    try {
      if (nativeWorkdir) await stopCompose(nativeWorkdir);
    } catch {
      // ignore cleanup errors
    }
  }

  const receipt = {
    ...header,
    component: 'iii-phase0',
    decision,
    reason,
    fallback: decision === 'PASS' ? null : FALLBACK,
    engine: {
      tag: ENGINE_TAG,
      version: ENGINE_VERSION,
      sdk: 'iii-sdk = 0.24.4',
      queue_worker: { package: 'package://api.workers.iii.dev/queue', version: QUEUE_VERSION },
      adapter: {
        name: 'builtin',
        store_method: 'file_based',
        file_path: 'data/queue',
        redis: false,
      },
      artifact_sha256: cacheInfo?.measured ?? null,
    },
    ran_on: ranOs,
    cache: cacheInfo,
    elv2: ELV2_NOTES,
    tests,
    memory,
    blockers,
    started_at: started,
    finished_at: new Date().toISOString(),
    acknowledgement_model:
      'Returning Ok from the durable:subscriber handler is the queue ack; engine+queue+worker were killed before Ok.',
    product_ports: 'Not implemented (no RecordStore/JobQueue product wiring).',
  };
  await writeReceipt(receipt);
  if (process.argv.includes('--record')) {
    process.stdout.write(`iii receipt recorded: ${await recordReceipt(root, 'iii', receipt)}\n`);
  }
  process.stdout.write(`iii qualification receipt: ${receiptPath}\n`);
  process.stdout.write(`decision: ${decision}\n`);
  if (decision !== 'PASS') process.exitCode = 1;
}

main().catch(async (error) => {
  await mkdir(outDir, { recursive: true });
  const receipt = {
    component: 'iii-phase0',
    decision: 'REJECTED_WITH_FALLBACK',
    reason: `Orchestrator failure: ${error.message}`,
    fallback: FALLBACK,
    blockers: [error.message],
    finished_at: new Date().toISOString(),
  };
  await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  process.stderr.write(`${error.stack || error.message}\n`);
  process.exit(1);
});
