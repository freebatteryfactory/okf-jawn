/** Tracked hooks: POSIX sh, installed through core.hooksPath, never a tool that is not installed. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { delimiter, join, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
import { fixtureRepo } from './fixture-repo.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

test('tracked hooks are POSIX sh, drop the hook environment, and run the agreed checks',async()=>{
 const commitHook=await read('scripts/hooks/pre-commit'),pushHook=await read('scripts/hooks/pre-push');
 for(const [name,source] of [['pre-commit',commitHook],['pre-push',pushHook]]){
  assert.ok(source.startsWith('#!/bin/sh\n'),`${name} must start with #!/bin/sh`);
  assert.ok(!source.includes('\r'),`${name} must use LF line endings`);
  assert.match(source,/^set -eu$/m,name);
  assert.match(source,/^unset \$\(git rev-parse --local-env-vars\)$/m,`${name} must drop GIT_DIR and GIT_INDEX_FILE before running tests`);
  assert.doesNotMatch(source,/\[\[|\bfunction\b|<<<|\blefthook\b/,`${name} uses a non-POSIX construct or an uninstalled tool`);
 }
 assert.match(commitHook,/^bun scripts\/dev\.mjs check-offline --fast$/m,'pre-commit runs only the fast tests');
 assert.doesNotMatch(commitHook,/^bun scripts\/dev\.mjs check-offline$/m,'pre-commit must not run the full suite');
 assert.match(pushHook,/^bun scripts\/dev\.mjs check-offline$/m,'pre-push runs the full suite');
 assert.doesNotMatch(commitHook,/cargo/,'pre-commit stays dependency-free');
 assert.match(pushHook,/^cargo fmt --all --check$/m);
 assert.match(pushHook,/^refs=\$\(cat\)$/m,'pre-push must read the pushed refs from stdin before anything can consume it');
 assert.doesNotMatch(pushHook,/rev-parse --abbrev-ref HEAD/,'pre-push must not take the branch from the current HEAD');
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
test('pre-push scope-checks each pushed build/* ref at its pushed commit and ignores the rest',async t=>{
 // `bun` and `cargo` are recorded stubs, so only the hook's own control flow runs.
 const posix=path=>path.split(sep).join('/');
 const {base,root:repo}=await fixtureRepo(t);
 const stubs=join(base,'stubs'),log=join(base,'calls.log');
 await mkdir(stubs);
 for(const name of ['bun','cargo']){
  await writeFile(join(stubs,name),`#!/bin/sh\necho "${name} $*" >> "${posix(log)}"\n`);
  await chmod(join(stubs,name),0o755);
 }
 const zero='0'.repeat(40),sha='a'.repeat(40),other='c'.repeat(40);
 const stdin=[`refs/heads/build/storage ${sha} refs/heads/build/storage ${zero}`,`refs/heads/feature/x ${'b'.repeat(40)} refs/heads/feature/x ${zero}`,
  `refs/heads/build/views ${other} refs/heads/build/views ${zero}`,`(delete) ${zero} refs/heads/build/ingest ${sha}`].join('\n')+'\n';
 const result=spawnSync('sh',[posix(join(root,'scripts/hooks/pre-push'))],{cwd:repo,input:stdin,encoding:'utf8',env:{...process.env,PATH:`${stubs}${delimiter}${process.env.PATH}`}});
 assert.equal(result.status,0,result.stderr);
 assert.deepEqual((await readFile(log,'utf8')).trim().split(/\r?\n/),[
  'bun scripts/dev.mjs check-offline','cargo fmt --all --check',
  `bun scripts/dev.mjs scope storage --head ${sha}`,`bun scripts/dev.mjs scope views --head ${other}`]);
});
