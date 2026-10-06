/** Tracked hooks: POSIX sh, installed through core.hooksPath, never a tool that is not installed. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { cpSync, existsSync } from 'node:fs';
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { delimiter, join, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
import { commit, fixtureReceipt, fixtureRecord, fixtureRecordFiles, fixtureRepo, git, posixShell } from './fixture-repo.mjs';
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
 const result=spawnSync(posixShell(),[posix(join(root,'scripts/hooks/pre-push'))],{cwd:repo,input:stdin,encoding:'utf8',env:{...process.env,PATH:`${stubs}${delimiter}${process.env.PATH}`}});
 assert.equal(result.status,0,result.stderr);
 assert.deepEqual((await readFile(log,'utf8')).trim().split(/\r?\n/),[
  'bun scripts/dev.mjs check-offline','cargo fmt --all --check',
  `bun scripts/dev.mjs scope storage --head ${sha}`,`bun scripts/dev.mjs check-receipts --head ${sha}`,
  `bun scripts/dev.mjs check-receipts --head ${'b'.repeat(40)}`,
  `bun scripts/dev.mjs scope views --head ${other}`,`bun scripts/dev.mjs check-receipts --head ${other}`]);
});

// The receipt rule runs the real `check-receipts --head` from a copy of scripts/ inside a temporary
// repository; `bun` forwards that task to the real Bun and only records the other calls, and
// `cargo` is a stub. The repository has a record with receipt gates, their criteria files and a
// receipt its record agrees with, so only what is under test can be reported: a stale input, or
// with `edited` a hand edit ('receipt': a result typed PASS over a failed criterion; 'status': a
// gate typed passed without a receipt).
const failed=[{id:'corpus/a.pdf/content',required:true,result:'pass'},{id:'corpus/a.pdf/provenance',required:true,result:'fail'}];
async function pushWithReceipts(t,{stale,refs,edited}){
 const posix=path=>path.split(sep).join('/');
 const {base,root:repo}=await fixtureRepo(t,fixtureRecordFiles());
 const first=await git(repo,'rev-parse','HEAD');
 const typedPass={'verification.json':fixtureRecord({statuses:{docling:'passed'}})};
 const recorded=edited==='status'?typedPass
  :edited==='receipt'?{...typedPass,'qualification/receipts/docling.json':fixtureReceipt('docling',first,{criteria:failed,result:'PASS'})}
  :stale===null?{'notes.txt':'no receipts'}
  :{...typedPass,'qualification/receipts/docling.json':fixtureReceipt('docling',first)};
 await commit(repo,recorded,'record');
 const head=stale===true?await commit(repo,{'harness/run.mjs':'// v2\n'},'change an input'):await git(repo,'rev-parse','HEAD');
 cpSync(join(root,'scripts'),join(repo,'scripts'),{recursive:true});
 const stubs=join(base,'stubs');
 await mkdir(stubs);
 await writeFile(join(stubs,'bun'),`#!/bin/sh\nif [ "$2" = check-receipts ]; then exec "${posix(process.execPath)}" "$@"; fi\necho "bun $*" >> "${posix(join(base,'calls.log'))}"\n`);
 await writeFile(join(stubs,'cargo'),'#!/bin/sh\nexit 0\n');
 for(const name of ['bun','cargo'])await chmod(join(stubs,name),0o755);
 const stdin=refs.map(([local,remote,sha=head])=>`${local} ${sha} ${remote} ${'0'.repeat(40)}`).join('\n')+'\n';
 const result=spawnSync(posixShell(),[posix(join(root,'scripts/hooks/pre-push'))],{cwd:repo,input:stdin,encoding:'utf8',env:{...process.env,PATH:`${stubs}${delimiter}${process.env.PATH}`}});
 return {result,output:result.stdout+result.stderr};
}
const rerun='bun qualification/docling/run.mjs, then bun qualification/record.mjs docling';
test('pre-push only warns about a stale receipt on a lane or cure ref',async t=>{
 for(const ref of ['refs/heads/build/storage','refs/heads/cure/hook','refs/heads/feature/x']){
  const {result,output}=await pushWithReceipts(t,{stale:true,refs:[[ref,ref]]});
  assert.equal(result.status,0,`${ref}: ${output}`);
  assert.match(output,/^warning: stale receipt docling\.json: inputs changed after [0-9a-f]{40}: harness\/run\.mjs -- re-run: bun qualification\/docling\/run\.mjs, then bun qualification\/record\.mjs docling$/m,ref);
 }
});
test('pre-push blocks a stale receipt on main and integration/*, naming the receipt and the harness',async t=>{
 for(const ref of ['refs/heads/main','refs/heads/integration/x']){
  const {result,output}=await pushWithReceipts(t,{stale:true,refs:[['refs/heads/cure/hook',ref]]});
  assert.notEqual(result.status,0,`${ref}: ${output}`);
  assert.match(output,/^stale receipt docling\.json: inputs changed after [0-9a-f]{40}: harness\/run\.mjs -- re-run: /m,ref);
  assert.ok(output.includes(rerun),output);
  assert.doesNotMatch(output,/warning:/,'a blocking ref is not downgraded to a warning');
 }
});
test('pre-push blocks a hand-edited receipt or status on main and integration/*, and only warns elsewhere',async t=>{
 const said={
  receipt:'untrusted receipt docling.json: result is PASS but its criteria fold to FAIL -- re-run: bun qualification/docling/run.mjs, then bun qualification/record.mjs docling',
  status:/^(warning: )?typed record docling-library-qualification: verification\.json at [0-9a-f]{40} types status "passed" but the derived status is "incomplete" \(qualification\/receipts\/docling\.json is absent\); run `bun qualification\/record\.mjs` to rewrite it$/m,
 };
 for(const edited of ['receipt','status']){
  for(const ref of ['refs/heads/main','refs/heads/integration/x']){
   const {result,output}=await pushWithReceipts(t,{stale:false,edited,refs:[['refs/heads/cure/hook',ref]]});
   assert.notEqual(result.status,0,`${edited} to ${ref}: ${output}`);
   if(edited==='receipt')assert.ok(output.split(/\r?\n/).includes(said.receipt),output);else assert.match(output,said.status);
   assert.doesNotMatch(output,/warning:/,'a blocking ref is not downgraded to a warning');
  }
  const {result,output}=await pushWithReceipts(t,{stale:false,edited,refs:[['refs/heads/cure/hook','refs/heads/cure/hook']]});
  assert.equal(result.status,0,`${edited}: ${output}`);
  if(edited==='receipt')assert.ok(output.split(/\r?\n/).includes(`warning: ${said.receipt}`),output);else assert.match(output,said.status);
  assert.match(output,/^warning: /m);
 }
});
test('pre-push says nothing about receipts that are valid or absent, and skips a deleted ref',async t=>{
 const zero='0'.repeat(40);
 for(const stale of [false,null]){
  const {result,output}=await pushWithReceipts(t,{stale,refs:[['refs/heads/main','refs/heads/main'],['(delete)','refs/heads/old',zero]]});
  assert.equal(result.status,0,output);
  assert.doesNotMatch(output,/receipt/i,output);
 }
 // A deleted ref alone neither crashes nor checks anything, even though the remote ref is main.
 const {result,output}=await pushWithReceipts(t,{stale:true,refs:[['(delete)','refs/heads/main',zero]]});
 assert.equal(result.status,0,output);
 assert.doesNotMatch(output,/receipt/i,output);
});
