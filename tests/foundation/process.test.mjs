/** Child command failures are evidence, never silently converted into success. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { run, version } from '../../scripts/lib/process.mjs';

test('argument vectors are passed literally without shell interpolation', async () => {
 const value = '$(echo not-a-command)'; const result=await run(process.execPath,['-e','process.stdout.write(process.argv[1])',value],{capture:true});
 assert.equal(result.stdout,value);
});
test('nonzero child exit rejects', async () => { await assert.rejects(run(process.execPath,['-e','process.stderr.write("failure"); process.exitCode=7'],{capture:true}),/exited 7/); });
test('missing commands are unavailable, never invented version strings', async () => { assert.equal(typeof (await version('okf-tool-that-does-not-exist')).unavailable,'string'); });
test('timeout rejects the command', async () => { await assert.rejects(run(process.execPath,['-e','setInterval(()=>{},1000)'],{capture:true,timeout:150}),/timed out/); });
