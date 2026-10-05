/**
 * Run one Docling fixture process and read its peak resident memory once.
 *
 * Protocol with qualification/docling/src/main.rs when OKF_DOCLING_HOLD=1:
 *   1. the converter process writes its per-fixture receipt, then prints one stdout line
 *      {"okf_docling":"done","only":"<fixture>","receipt":"<path>"};
 *   2. it blocks reading stdin until EOF;
 *   3. this runner samples the process peak exactly once (PeakWorkingSet64 on Windows,
 *      VmHWM on Linux: both are high-water marks the OS keeps, so one late read is the
 *      true peak), then closes stdin and the process exits.
 *
 * The 'error' and 'close' listeners are attached synchronously after spawn, before
 * anything is awaited, so a process that exits at once is always observed. There is no
 * polling loop and no second sample. runFixtureProcess never rejects: a missing peak is
 * a recorded reason, judged by the caller.
 */

import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';

export const DONE_MARKER = 'done';

const NOT_SAMPLED = 'converter exited before printing the done marker; peak RSS was not sampled';
const SAMPLER_TIMEOUT_MS = 30_000;
const WINDOWS_NOTE = 'PeakWorkingSet64 of the converter process, read once via Get-Process while it waited on stdin';
const LINUX_NOTE = 'VmHWM of the converter process, read once from /proc/<pid>/status while it waited on stdin';

/** The parsed done marker when `line` is one; null for every other stdout line. */
export function parseDoneMarker(line) {
  const text = line.trim();
  if (!text.startsWith('{')) return null;
  try {
    const value = JSON.parse(text);
    return value && value.okf_docling === DONE_MARKER ? value : null;
  } catch {
    return null;
  }
}

/** Largest valid sample; a null, NaN or non-positive value never replaces a number. */
export function mergePeak(current, next) {
  const valid = (value) => typeof value === 'number' && Number.isFinite(value) && value > 0;
  if (!valid(next)) return valid(current) ? current : null;
  return valid(current) ? Math.max(current, next) : next;
}

/** VmHWM in bytes from the text of /proc/<pid>/status, or null when the line is absent. */
export function parseVmHwm(statusText) {
  const match = /^VmHWM:\s+(\d+)\s+kB\r?$/m.exec(statusText);
  return match ? Number(match[1]) * 1024 : null;
}

function sampleWindows(pid) {
  return new Promise((resolveSample) => {
    let settled = false;
    let timer = null;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolveSample(value);
    };
    const ps = spawn(
      'powershell',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `(Get-Process -Id ${pid} -ErrorAction SilentlyContinue).PeakWorkingSet64`,
      ],
      { stdio: ['ignore', 'pipe', 'ignore'], windowsHide: true },
    );
    let buffer = '';
    timer = setTimeout(() => {
      ps.kill();
      finish({ bytes: null, note: `Get-Process did not answer within ${SAMPLER_TIMEOUT_MS} ms` });
    }, SAMPLER_TIMEOUT_MS);
    ps.stdout.on('data', (chunk) => {
      buffer += chunk;
    });
    ps.on('error', (error) => finish({ bytes: null, note: `powershell could not start: ${error.message}` }));
    ps.on('close', () => {
      const bytes = Number(buffer.trim());
      finish(
        Number.isFinite(bytes) && bytes > 0
          ? { bytes, note: WINDOWS_NOTE }
          : {
              bytes: null,
              note: `Get-Process reported no PeakWorkingSet64 for pid ${pid} (output ${JSON.stringify(buffer.trim())})`,
            },
      );
    });
  });
}

/** One reading of a live process's peak resident memory, with the source or the reason it is missing. */
export async function samplePeakRss(pid, platform = process.platform) {
  if (!Number.isInteger(pid) || pid <= 0) return { bytes: null, note: 'no process id to sample' };
  if (platform === 'win32') return sampleWindows(pid);
  if (platform === 'linux') {
    try {
      const bytes = parseVmHwm(await readFile(`/proc/${pid}/status`, 'utf8'));
      return bytes === null
        ? { bytes: null, note: `/proc/${pid}/status has no VmHWM line` }
        : { bytes, note: LINUX_NOTE };
    } catch (error) {
      return { bytes: null, note: `/proc/${pid}/status unreadable: ${error.message}` };
    }
  }
  return { bytes: null, note: `no peak RSS source on platform ${platform}` };
}

/** Spawn one fixture process, sample its peak on the done marker, release it, report what happened. */
export function runFixtureProcess({
  command,
  args = [],
  cwd,
  env,
  sample = samplePeakRss,
  timeoutMs = 900_000,
  onStdout,
  onStderr,
}) {
  return new Promise((resolveRun) => {
    const state = {
      stdout: '',
      stderr: '',
      pending: '',
      done: null,
      peak: null,
      note: NOT_SAMPLED,
      timedOut: false,
      sampling: null,
    };
    const report = (exit) => ({
      exitCode: exit.exitCode,
      signal: exit.signal,
      spawnError: exit.spawnError,
      timedOut: state.timedOut,
      done: state.done,
      peakRssBytes: state.peak,
      peakRssNote:
        state.timedOut && state.peak === null
          ? `converter was killed after ${timeoutMs} ms; peak RSS was not sampled`
          : state.note,
      stdout: state.stdout,
      stderr: state.stderr,
    });

    let child;
    try {
      child = spawn(command, args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    } catch (error) {
      resolveRun(report({ exitCode: null, signal: null, spawnError: error.message }));
      return;
    }

    let settled = false;
    let timer = null;
    const settle = (exit) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      // An in-flight sample finishes first so its result (or its reason) is in the report.
      Promise.resolve(state.sampling).then(() => resolveRun(report(exit)));
    };

    // Listeners first. Nothing in this function awaits before they exist.
    child.on('error', (error) => settle({ exitCode: null, signal: null, spawnError: error.message }));
    child.on('close', (code, signal) => settle({ exitCode: code, signal: signal ?? null, spawnError: null }));
    child.stdin?.on('error', () => {});

    timer = setTimeout(() => {
      state.timedOut = true;
      child.kill();
    }, timeoutMs);

    const release = () => {
      try {
        child.stdin?.end();
      } catch {
        // stdin already closed: the process is gone.
      }
    };

    const onLine = (line) => {
      const marker = parseDoneMarker(line);
      if (!marker || state.sampling) return; // single flight: only the first marker samples
      state.done = marker;
      state.sampling = Promise.resolve()
        .then(() => sample(child.pid))
        .then(
          (sampled) => {
            state.peak = mergePeak(state.peak, sampled?.bytes ?? null);
            state.note = sampled?.note ?? 'sampler returned no note';
          },
          (error) => {
            state.note = `peak RSS sampler failed: ${error.message}`;
          },
        )
        .finally(release);
    };

    child.stdout?.on('data', (chunk) => {
      const text = String(chunk);
      state.stdout += text;
      onStdout?.(text);
      state.pending += text;
      let newline = state.pending.indexOf('\n');
      while (newline >= 0) {
        onLine(state.pending.slice(0, newline));
        state.pending = state.pending.slice(newline + 1);
        newline = state.pending.indexOf('\n');
      }
    });
    child.stderr?.on('data', (chunk) => {
      const text = String(chunk);
      state.stderr += text;
      onStderr?.(text);
    });
  });
}
