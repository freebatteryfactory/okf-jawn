/** Run argument-vector commands without shell interpolation or swallowed failures. */
import { spawn } from 'node:child_process';

export async function run(command, args, options = {}) {
  const { cwd, env = {}, capture = false, timeout = 600_000, allowFailure = false } = options;
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env: { ...process.env, ...env }, shell: false,
      stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'inherit' });
    let stdout = ''; let stderr = ''; let timer;
    if (capture) {
      child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
      child.stdout.on('data', chunk => { stdout += chunk; });
      child.stderr.on('data', chunk => { stderr += chunk; });
    }
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', (code, signal) => {
      clearTimeout(timer);
      if (code === 0 || allowFailure) resolve({ code: code ?? 1, stdout, stderr });
      else reject(new Error(`${command} ${args.join(' ')} exited ${code ?? signal}\n${stderr}`));
    });
    timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`${command} timed out; no success recorded`)); }, timeout);
  });
}

export async function version(command, args = ['--version']) {
  try { return (await run(command, args, { capture: true, timeout: 20_000 })).stdout.trim(); }
  catch (error) { return { unavailable: String(error.message) }; }
}
