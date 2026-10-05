/** Child command failures are evidence, never silently converted into success. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { CARGO_TIMEOUT_MS, DEFAULT_TIMEOUT_MS, cargoEnvironment, childEnvironment, defaultTimeout, gitLocalEnvironment, run, treeKill, version } from '../../scripts/lib/process.mjs';

test('argument vectors are passed literally without shell interpolation', async () => {
 const value = '$(echo not-a-command)'; const result=await run(process.execPath,['-e','process.stdout.write(process.argv[1])',value],{capture:true});
 assert.equal(result.stdout,value);
});
test('nonzero child exit rejects', async () => { await assert.rejects(run(process.execPath,['-e','process.stderr.write("failure"); process.exitCode=7'],{capture:true}),/exited 7/); });
test('missing commands are unavailable, never invented version strings', async () => { assert.equal(typeof (await version('okf-tool-that-does-not-exist')).unavailable,'string'); });
test('timeout rejects the command', async () => { await assert.rejects(run(process.execPath,['-e','setInterval(()=>{},1000)'],{capture:true,timeout:150}),/timed out/); });
test('cargo gets 45 minutes; every other command keeps ten', () => {
 assert.equal(CARGO_TIMEOUT_MS, 45 * 60 * 1000);
 assert.equal(defaultTimeout('cargo'), CARGO_TIMEOUT_MS);
 assert.equal(defaultTimeout('git'), 10 * 60 * 1000);
 assert.equal(defaultTimeout('C:\\Users\\x\\.bun\\bin\\bun.exe'), DEFAULT_TIMEOUT_MS);
});
test('cargo on Windows never sees a usr\\bin directory, whose link.exe shadows the MSVC linker', () => {
 const path = ['C:\\Program Files\\Git\\usr\\bin', 'C:\\Users\\x\\.cargo\\bin', 'C:\\Program Files\\Git\\mingw64\\bin', 'C:/msys64/usr/bin/', 'C:\\Windows\\System32'].join(';');
 assert.deepEqual(cargoEnvironment({ Path: path, HOME: 'x' }, 'win32'), { Path: 'C:\\Users\\x\\.cargo\\bin;C:\\Program Files\\Git\\mingw64\\bin;C:\\Windows\\System32' });
 assert.deepEqual(cargoEnvironment({ PATH: 'C:\\Git\\usr\\bin;C:\\x' }, 'win32'), { PATH: 'C:\\x' });
 assert.deepEqual(cargoEnvironment({ Path: 'C:\\Users\\x\\.cargo\\bin' }, 'win32'), {});
 assert.deepEqual(cargoEnvironment({ PATH: '/usr/bin:/usr/local/bin' }, 'linux'), {});
 const base = { Path: 'C:\\Git\\usr\\bin;C:\\x' };
 assert.equal(childEnvironment('cargo', { EXTRA: '1' }, base, 'win32').Path, 'C:\\x');
 assert.equal(childEnvironment('cargo', { EXTRA: '1' }, base, 'win32').EXTRA, '1');
 assert.equal(childEnvironment('git', {}, base, 'win32').Path, base.Path);
});
test('a timed-out command ends its whole process tree on Windows', () => {
 assert.deepEqual(treeKill(4242, 'win32'), ['taskkill', ['/PID', '4242', '/T', '/F']]);
 assert.equal(treeKill(4242, 'linux'), null);
});
test('tee receives live output from both streams without capture', async () => {
 const seen = { stdout: '', stderr: '' };
 const result = await run(process.execPath, ['-e', 'process.stdout.write("out"); process.stderr.write("err")'], { tee: (chunk, stream) => { seen[stream] += chunk; } });
 assert.deepEqual(seen, { stdout: 'out', stderr: 'err' });
 assert.equal(result.stdout, '');
});
test('the task entrypoint drops the repository variables Git exports to hooks', async () => {
 for (const name of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR']) assert.ok(gitLocalEnvironment.includes(name), name);
 const { readFile } = await import('node:fs/promises');
 const source = await readFile(new URL('../../scripts/dev.mjs', import.meta.url), 'utf8');
 assert.match(source, /for \(const name of gitLocalEnvironment\) delete process\.env\[name\];/);
 assert.ok(source.indexOf('delete process.env[name]') < source.indexOf('async function doctor'), 'the variables must be dropped before any task runs');
});
