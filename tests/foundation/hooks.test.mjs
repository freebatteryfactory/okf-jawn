/** Tracked hooks: POSIX sh, installed through core.hooksPath, never a tool that is not installed. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

test('tracked hooks are POSIX sh, drop the hook environment, and run the agreed checks',async()=>{
 const commitHook=await read('scripts/hooks/pre-commit'),pushHook=await read('scripts/hooks/pre-push');
 for(const [name,source] of [['pre-commit',commitHook],['pre-push',pushHook]]){
  assert.ok(source.startsWith('#!/bin/sh\n'),`${name} must start with #!/bin/sh`);
  assert.ok(!source.includes('\r'),`${name} must use LF line endings`);
  assert.match(source,/^set -eu$/m,name);
  assert.match(source,/^unset \$\(git rev-parse --local-env-vars\)$/m,`${name} must drop GIT_DIR and GIT_INDEX_FILE before running tests`);
  assert.match(source,/^bun scripts\/dev\.mjs check-offline$/m,name);
  assert.doesNotMatch(source,/\[\[|\bfunction\b|<<<|\blefthook\b/,`${name} uses a non-POSIX construct or an uninstalled tool`);
 }
 assert.doesNotMatch(commitHook,/cargo/,'pre-commit stays dependency-free');
 assert.match(pushHook,/^cargo fmt --all --check$/m);
 assert.match(pushHook,/build\/\*\|cure\/\*\) bun scripts\/dev\.mjs scope ;;/);
 await assert.rejects(read('lefthook.yml'),{code:'ENOENT'});
 // A source copy without history (the build container) has no index to inspect.
 if(!existsSync(join(root,'.git')))return;
 const modes=(await run('git',['ls-files','-s','scripts/hooks'],{cwd:root,capture:true})).stdout.trim().split(/\r?\n/);
 assert.equal(modes.length,2);
 for(const line of modes)assert.ok(line.startsWith('100755 '),`not executable in the index: ${line}`);
});
test('bootstrap and init install the hooks',async()=>{
 const entry=await read('scripts/dev.mjs');
 const bootstrap=entry.slice(entry.indexOf('async function bootstrap()'),entry.indexOf('async function vendor()'));
 assert.match(bootstrap,/await installHooks\(root\);/);
 assert.match(await read('scripts/lib/init.mjs'),/hooksInstalled: await installHooks\(root\)/);
});
