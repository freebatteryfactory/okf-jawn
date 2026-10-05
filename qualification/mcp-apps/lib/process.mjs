/**
 * Long-running helper processes for the MCP Apps orchestrator.
 *
 * spawnGroup starts a process as the leader of its own process group on POSIX
 * (detached: true) so killProcessTree can signal the whole group; on Windows
 * taskkill /T walks the tree. waitForListening reports a process that dies before it
 * is ready at once, with its exit code and stderr, instead of a later TCP timeout.
 */

import { spawn } from 'node:child_process';

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const tail = (text, max = 2000) => (text.length > max ? text.slice(-max) : text);

/** Start a process whose output is kept and whose exit is always observable. */
export function spawnGroup(command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env ?? process.env,
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
    detached: process.platform !== 'win32',
  });
  let stdout = '';
  let stderr = '';
  child.stdout?.on('data', (chunk) => {
    stdout += chunk;
  });
  child.stderr?.on('data', (chunk) => {
    stderr += chunk;
  });
  const exited = new Promise((resolveExit) => {
    child.on('error', (error) => resolveExit({ code: null, signal: null, error: error.message }));
    child.on('close', (code, signal) => resolveExit({ code, signal: signal ?? null, error: null }));
  });
  return { child, stdout: () => stdout, stderr: () => stderr, exited };
}

/** Signal a POSIX process group; true while the group still has members. */
function signalGroup(pid, signal) {
  try {
    process.kill(-pid, signal);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

/** Stop a process started by spawnGroup together with everything it started. */
export async function killProcessTree(proc, { graceMs = 5_000 } = {}) {
  const pid = proc?.child?.pid;
  if (!pid) return proc ? proc.exited : { code: null, signal: null, error: 'never started' };
  if (process.platform === 'win32') {
    await new Promise((done) => {
      const killer = spawn('taskkill', ['/pid', String(pid), '/T', '/F'], {
        stdio: 'ignore',
        windowsHide: true,
      });
      killer.on('error', done);
      killer.on('close', done);
    });
  } else {
    signalGroup(pid, 'SIGTERM');
    const deadline = Date.now() + graceMs;
    while (Date.now() < deadline && signalGroup(pid, 0)) await sleep(50);
    signalGroup(pid, 'SIGKILL');
  }
  return Promise.race([
    proc.exited,
    sleep(graceMs).then(() => ({ code: null, signal: null, error: `still running ${graceMs} ms after kill` })),
  ]);
}

/** Wait for a readiness line; fail at once when the process exits first. */
export function waitForListening(proc, { pattern, timeoutMs = 60_000, label = 'process' }) {
  return new Promise((resolveReady, reject) => {
    let settled = false;
    let timer = null;
    const check = () => {
      const match = pattern.exec(proc.stderr()) ?? pattern.exec(proc.stdout());
      if (match) finish(resolveReady, match);
    };
    const finish = (settleWith, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      proc.child.stderr?.off('data', check);
      proc.child.stdout?.off('data', check);
      settleWith(value);
    };
    timer = setTimeout(
      () =>
        finish(
          reject,
          new Error(`${label} did not report listening within ${timeoutMs} ms\nstderr:\n${tail(proc.stderr())}`),
        ),
      timeoutMs,
    );
    // spawnGroup's own listeners were registered first, so the buffers are current when check runs.
    proc.child.stderr?.on('data', check);
    proc.child.stdout?.on('data', check);
    proc.exited.then(({ code, signal, error }) =>
      finish(
        reject,
        new Error(
          `${label} exited before listening (exit ${code ?? 'none'}${signal ? `, signal ${signal}` : ''}${error ? `, ${error}` : ''})\nstderr:\n${tail(proc.stderr())}`,
        ),
      ),
    );
    check();
  });
}
