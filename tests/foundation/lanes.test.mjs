/** One lane table drives worktrees, gates, scope, the AGENTS.md table and CODEOWNERS. */
import { afterAll } from 'bun:test';
import test from './concurrent-test.mjs';
import assert from 'node:assert/strict';
import { cpSync, existsSync } from 'node:fs';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LANE_TABLE_BEGIN, LANE_TABLE_END, checkScope, createLanes, laneNamed, lanes, lanesParent, outOfScope, renderLaneTable, resetLanes, scopeFor } from '../../scripts/lib/lanes.mjs';
import * as lanesModule from '../../scripts/lib/lanes.mjs';
import { run } from '../../scripts/lib/process.mjs';
import { commit, eachCase, git, sharedRepos } from './fixture-repo.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
// Every test starts from a copy of a repository prepared once for this file.
const shared=sharedRepos();
afterAll(shared.dispose);
const fixtureRepo=shared.fixtureRepo;
const read=file=>readFile(join(root,file),'utf8');

/** Case-sensitive existence, so `/Justfile` cannot stand in for `justfile` on Windows or macOS. */
async function existsExact(relative){
 let directory=root;
 for(const segment of relative.split('/').filter(Boolean)){
  let names;try{names=await readdir(directory);}catch{return false;}
  if(!names.includes(segment))return false;
  directory=join(directory,segment);
 }
 return true;
}

test('the lane table is well formed and matches the crates it gates',async()=>{
 assert.equal(new Set(lanes.map(lane=>lane.name)).size,lanes.length);
 for(const lane of lanes){
  assert.equal(lane.branch,`build/${lane.name}`);
  assert.ok(['rust','ui'].includes(lane.kind),lane.name);
  assert.ok(lane.directories.length>0,lane.name);
  for(const directory of lane.directories){assert.ok(directory.endsWith('/'),directory);assert.ok(await existsExact(directory),`${lane.name}: ${directory} does not exist`);}
  for(const path of [...lane.files,...lane.exclude])assert.ok(await existsExact(path),`${lane.name}: ${path} does not exist`);
  assert.ok(lane.proves.length>0,lane.name);
  if(lane.kind==='ui'){assert.deepEqual(lane.crates,[]);assert.deepEqual(lane.features,[]);continue;}
  assert.ok(lane.crates.length>0,lane.name);
  const manifests=await Promise.all(lane.directories.map(directory=>read(`${directory}Cargo.toml`)));
  for(const crate of lane.crates){
   const manifest=manifests.find(text=>text.includes(`name = "${crate}"`));
   assert.ok(manifest,`${lane.name}: no manifest under its directories declares ${crate}`);
   for(const feature of lane.features.filter(entry=>entry.startsWith(`${crate}/`)))assert.match(manifest,new RegExp(`^${feature.slice(crate.length+1)} = \\[`,'m'),`${crate} declares no feature ${feature}`);
  }
  for(const feature of lane.features)assert.ok(lane.crates.includes(feature.split('/')[0]),`${lane.name}: ${feature} names a crate the lane does not gate`);
  for(const directory of lane.directories)assert.ok(lane.exclude.includes(`${directory}Cargo.toml`),`${lane.name}: ${directory}Cargo.toml is a manifest and must be excluded`);
 }
});
test('what the views lane owns is excluded from workspace-ui, and every unit spec runs in exactly one UI lane',async()=>{
 const views=laneNamed('views'),workspace=laneNamed('workspace-ui');
 for(const path of [...views.directories,...views.files])assert.ok(workspace.exclude.includes(path),`workspace-ui does not exclude ${path}`);
 const specs=(await readdir(join(root,'ui/tests/unit'))).filter(name=>/\.test\.tsx?$/.test(name)).map(name=>`tests/unit/${name}`);
 for(const spec of specs)assert.equal(views.tests.includes(spec),workspace.testExclude.includes(spec),`${spec} must run in exactly one UI lane`);
 for(const filter of views.tests.filter(entry=>entry.startsWith('tests/unit/')))assert.ok(specs.includes(filter),`views filter ${filter} matches no spec`);
 assert.throws(()=>laneNamed('no-such-lane'),/Unknown lane: no-such-lane/);
});
test('the lane parent directory is configurable',()=>{
 const configured=resolve(root,'..','elsewhere');
 assert.equal(lanesParent(root,{OKF_LANES_DIR:configured},'win32',()=>true),configured);
 assert.equal(lanesParent(root,{OKF_LANES_DIR:'  '},'win32',()=>true),'D:\\okf\\lanes');
 assert.equal(lanesParent(root,{},'win32',()=>false),resolve(root,'..','okf-jawn-lanes'));
 assert.equal(lanesParent(root,{},'linux',()=>true),resolve(root,'..','okf-jawn-lanes'));
});
test('AGENTS.md carries the table rendered from the lane table',async()=>{
 const agents=await read('AGENTS.md');
 const begin=agents.indexOf(LANE_TABLE_BEGIN),end=agents.indexOf(LANE_TABLE_END);
 assert.ok(begin>=0&&end>begin,'AGENTS.md is missing the lane-table markers');
 assert.equal(agents.slice(begin+LANE_TABLE_BEGIN.length,end).trim(),renderLaneTable());
 for(const lane of lanes)assert.ok(agents.includes(`\`bun scripts/dev.mjs lane ${lane.name}\``),lane.name);
});
test('a lane-local AGENTS.md points to the root table and to verification.json, and copies neither',async()=>{
 // Each lane's own instructions sit at the top of its directories; the contract's are the integration owner's.
 const local=[...lanes.flatMap(lane=>lane.directories.map(directory=>[`${directory}AGENTS.md`,lane.name])),['crates/contract/AGENTS.md','integration-owner']];
 const record=JSON.parse(await read('verification.json'));
 for(const [file,owner] of local){
  const text=await read(file);
  assert.ok(text.includes(`This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the \`verification.json\` entries whose \`owner\` is \`${owner}\` (each names its command).`),`${file} does not point to the root table and to the gates owned by ${owner}`);
  assert.doesNotMatch(text,/^\*\*(?:Directories|Gates?|Receipt)\b/m,`${file} copies a directory list, gate or receipt that the root table states`);
  assert.doesNotMatch(text,/^\| Gate \|/m,`${file} copies a gate table that verification.json states`);
  assert.ok(owner==='integration-owner'||record.current.gates.construction.some(gate=>gate.owner===owner),`${owner} owns no construction gate, so ${file} points at nothing`);
 }
});
test('CODEOWNERS names real paths and its lane rows agree with the lane table',async()=>{
 const rows=(await read('.github/CODEOWNERS')).split(/\r?\n/).filter(line=>line.trim()&&!line.startsWith('#'))
  .map(line=>({path:line.split(/\s+/)[0],lane:/# lane: (\S+)/.exec(line)?.[1]??null}));
 for(const row of rows){
  assert.ok(row.path.startsWith('/'),row.path);
  assert.ok(await existsExact(row.path),`CODEOWNERS path ${row.path} does not exist with that exact spelling`);
 }
 for(const lane of lanes)assert.deepEqual(rows.filter(row=>row.lane===lane.name).map(row=>row.path).sort(),[...lane.directories,...lane.files].map(path=>`/${path}`).sort(),lane.name);
 for(const row of rows.filter(entry=>entry.lane))assert.ok(lanes.some(lane=>lane.name===row.lane),`CODEOWNERS names unknown lane ${row.lane}`);
});
test('paths outside a lane, and manifests inside it, are out of scope',()=>{
 assert.deepEqual(outOfScope(['crates/storage/src/lib.rs','api/operations.json','crates/storage/Cargo.toml','crates/core/src/lib.rs','Cargo.lock'],scopeFor('build/storage')),['crates/storage/Cargo.toml','crates/core/src/lib.rs','Cargo.lock']);
 assert.deepEqual(outOfScope(['ui/src/routes/index.tsx','ui/src/features/views/Chart.tsx','ui/package.json','ui/tests/unit/layout.test.tsx'],scopeFor('build/workspace-ui')),['ui/src/features/views/Chart.tsx','ui/package.json','ui/tests/unit/layout.test.tsx']);
 assert.deepEqual(outOfScope(['ui/src/features/views/Chart.tsx','ui/tests/unit/layout.test.tsx','ui/tests/unit/wire.test.ts'],scopeFor('build/views')),['ui/tests/unit/wire.test.ts']);
 assert.equal(scopeFor('feature/unknown'),null);
});
test('the scope check fails a lane branch that changed an out-of-lane file',async t=>{
 const {root}=await fixtureRepo(t,{'crates/storage/src/lib.rs':'//! storage\n','crates/core/src/lib.rs':'//! core\n'});
 await git(root,'checkout','--quiet','-b','build/storage');
 await commit(root,{'crates/storage/src/lib.rs':'//! storage, changed\n','api/operations.json':'[]\n'},'in lane, with generated output');
 assert.deepEqual((await checkScope(root)).changed,['api/operations.json','crates/storage/src/lib.rs']);
 await commit(root,{'crates/core/src/lib.rs':'//! core, changed from the storage lane\n'},'out of lane');
 await assert.rejects(checkScope(root),/scope check failed for storage: 1 path\(s\) outside its scope:\n  crates\/core\/src\/lib\.rs/);
 await assert.rejects(checkScope(root,{lane:'core-cli'}),/crates\/storage\/src\/lib\.rs/);
});
test('uncommitted and untracked files count, and a branch without a row has no scope',async t=>{
 const {root}=await fixtureRepo(t);
 await git(root,'checkout','--quiet','-b','build/ingest');
 await writeFile(join(root,'stray.txt'),'x');
 await assert.rejects(checkScope(root),/scope check failed for ingest[\s\S]*stray\.txt/);
 await git(root,'checkout','--quiet','-b','feature/unknown');
 await assert.rejects(checkScope(root),/No scope row for branch feature\/unknown/);
});
test('only lane branches have a scope; cure branches are finished and have none',()=>{
 assert.equal(scopeFor('cure/gates'),null);
 assert.equal(scopeFor('cure/tooling'),null);
 assert.ok(!('cures' in lanesModule),'the temporary cure rows are gone');
});
test('a detached HEAD gets one clear line that asks for a lane',async t=>{
 const {root}=await fixtureRepo(t);
 await git(root,'checkout','--quiet','--detach');
 await assert.rejects(checkScope(root),error=>{
  assert.doesNotMatch(error.message,/[\r\n]/,'one line');
  assert.match(error.message,/detached HEAD.*pass a lane: scope <lane>/);
  return true;
 });
 assert.equal((await checkScope(root,{lane:'storage'})).changed.length,0,'a named lane still works on a detached HEAD');
});
test('lanes-reset removes clean, empty lanes without forcing anything',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');const calls=[];
 await createLanes(root,{names:['storage','views'],parent});
 const recording=(args,options={})=>{calls.push(args);return run('git',args,{cwd:options.cwd??root,capture:true,allowFailure:true});};
 assert.match(await resetLanes(root,{parent,git:recording}),/removed 2 worktree\(s\) and 2 branch\(es\); nothing was forced/);
 assert.equal(existsSync(join(parent,'storage')),false);assert.equal(existsSync(join(parent,'views')),false);
 assert.equal(await git(root,'branch','--list','build/*'),'');
 for(const args of calls)for(const flag of ['--force','-f','-D'])assert.ok(!args.includes(flag),`git ${args.join(' ')}`);
});
test('lanes-reset refuses a dirty worktree even when its branch was deleted',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');
 await createLanes(root,{names:['storage','ingest'],parent});
 const storage=join(parent,'storage');
 await git(storage,'checkout','--quiet','--detach');
 await git(root,'branch','-D','build/storage');
 await writeFile(join(storage,'unsaved-work.txt'),'not committed anywhere');
 await assert.rejects(resetLanes(root,{parent}),/lanes-reset refused; nothing was removed:[\s\S]*storage is dirty/);
 assert.equal(await readFile(join(storage,'unsaved-work.txt'),'utf8'),'not committed anywhere');
 assert.equal(existsSync(join(parent,'ingest')),true,'a refusal leaves every lane in place');
});
test('lanes-reset treats a failed git status as dirty, and lane commits as work',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');
 await createLanes(root,{names:['server'],parent});
 const failing=(args,options={})=>args[0]==='status'&&options.cwd?Promise.resolve({code:128,stdout:'',stderr:'fatal: simulated'}):run('git',args,{cwd:options.cwd??root,capture:true,allowFailure:true});
 await assert.rejects(resetLanes(root,{parent,git:failing}),/could not read its status \(git exit 128\); treated as dirty/);
 await commit(join(parent,'server'),{'crates/server/src/new.rs':'//! lane work\n'},'lane work');
 await assert.rejects(resetLanes(root,{parent}),/1 commit\(s\) that are not in HEAD/);
 assert.equal(existsSync(join(parent,'server')),true);
});
test('a directory that is not a worktree of this repository is never removed',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');
 await mkdir(join(parent,'views'),{recursive:true});await writeFile(join(parent,'views','notes.txt'),'someone else');
 await assert.rejects(resetLanes(root,{parent}),/views exists but is not a worktree of this repository/);
 assert.equal(existsSync(join(parent,'views','notes.txt')),true);
});
test('the entrypoint lists no lane and no lane directory of its own, and nothing forces git',async()=>{
 const entry=await read('scripts/dev.mjs');
 for(const lane of lanes)assert.doesNotMatch(entry,new RegExp(`'${lane.name}'`),lane.name);
 assert.doesNotMatch(entry,/okf-jawn-lanes|--force/);
 assert.doesNotMatch(await read('scripts/lib/lanes.mjs'),/'--force'|'-f'|'-D'/);
});
test('the scope check can address a pushed commit instead of HEAD, and then ignores the working tree',async t=>{
 const {root}=await fixtureRepo(t,{'crates/storage/src/lib.rs':'//! storage\n','crates/core/src/lib.rs':'//! core\n'});
 await git(root,'checkout','--quiet','-b','build/storage');
 const good=await commit(root,{'crates/storage/src/lib.rs':'//! storage, changed\n'},'in lane');
 const bad=await commit(root,{'crates/core/src/lib.rs':'//! core, changed from the storage lane\n'},'out of lane');
 await git(root,'checkout','--quiet','main');
 await writeFile(join(root,'stray.txt'),'not part of any pushed commit');
 assert.deepEqual((await checkScope(root,{lane:'storage',head:good})).changed,['crates/storage/src/lib.rs']);
 await assert.rejects(checkScope(root,{lane:'storage',head:bad}),/scope check failed for storage: 1 path\(s\) outside its scope:\n  crates\/core\/src\/lib\.rs/);
 await assert.rejects(checkScope(root,{lane:'storage'}),/stray\.txt/,'without a head the working tree still counts');
});

/** The real scripts/dev.mjs run inside a repository (a copy of one), as a user runs it; its stderr one entry per line. */
async function devIn(repo,...args){
 const result=await run(process.execPath,['scripts/dev.mjs',...args],{cwd:repo,capture:true,allowFailure:true});
 return {...result,said:result.stderr.trim().split(/\r?\n/)};
}
async function repoWithTools(t,files){
 const {root:repo}=await fixtureRepo(t,files);
 cpSync(join(root,'scripts'),join(repo,'scripts'),{recursive:true});
 return repo;
}

test('every option that takes a value takes it as --name value and as --name=value, in scope and premerge',async t=>{
 const repo=await repoWithTools(t,{'crates/storage/src/lib.rs':'//! storage\n','crates/core/src/lib.rs':'//! core\n','.gitignore':'scripts/\n'});
 await git(repo,'checkout','--quiet','-b','build/storage');
 const good=await commit(repo,{'crates/storage/src/lib.rs':'//! storage, changed\n'},'in lane');
 const bad=await commit(repo,{'crates/core/src/lib.rs':'//! core, changed from the storage lane\n'},'out of lane');
 // HEAD is the good commit, so a --head that is dropped judges the good one and passes; the pushed commit is the bad one.
 await git(repo,'checkout','--quiet','--detach',good);
 for(const spelling of [['--head',bad],[`--head=${bad}`]]){
  const result=await devIn(repo,'scope','storage',...spelling);
  assert.equal(result.code,1,spelling.join(' '));
  assert.match(result.stderr,/scope check failed for storage: 1 path\(s\) outside its scope:\n {2}crates\/core\/src\/lib\.rs/,spelling.join(' '));
 }
 for(const spelling of [['--base','no-such-ref'],['--base=no-such-ref']]){
  const result=await devIn(repo,'scope','storage',...spelling);
  assert.equal(result.code,1,spelling.join(' '));
  assert.deepEqual(result.said,['scope check needs no-such-ref or origin/no-such-ref to compare against; neither exists here.'],spelling.join(' '));
 }
 for(const spelling of [['--step','no-such-step'],['--step=no-such-step']]){
  const result=await devIn(repo,'premerge',...spelling);
  assert.equal(result.code,1,spelling.join(' '));
  assert.match(result.stderr,/^Unknown premerge step: no-such-step\. Steps: /m,spelling.join(' '));
 }
});

test('a value-taking option given no value, or twice, is an error that says so, never ignored',async t=>{
 const repo=await repoWithTools(t,{'README.md':'fixture\n','.gitignore':'scripts/\n'});
 const sha=await git(repo,'rev-parse','HEAD');
 for(const [args,said] of [
  [['scope','storage','--base'],'scope --base needs a commit: bun scripts/dev.mjs scope --base <sha>'],
  [['scope','storage','--base='],'scope --base needs a commit: bun scripts/dev.mjs scope --base <sha>'],
  [['scope','storage','--head','--base',sha],'scope --head needs a commit: bun scripts/dev.mjs scope --head <sha>'],
  [['scope','storage','--head='],'scope --head needs a commit: bun scripts/dev.mjs scope --head <sha>'],
  [['premerge','--step'],'premerge --step needs a step id: bun scripts/dev.mjs premerge --step <id>'],
  [['premerge','--step='],'premerge --step needs a step id: bun scripts/dev.mjs premerge --step <id>'],
  [['premerge','--step=a','--step','b'],'premerge --step was given more than once; name one step id'],
  [['scope','storage','--base',sha,'--base='+sha],'scope --base was given more than once; name one commit'],
 ]){
  const result=await devIn(repo,...args);
  assert.equal(result.code,1,args.join(' '));
  assert.deepEqual(result.said,[said],args.join(' '));
  assert.equal(result.stdout,'',args.join(' '));
 }
});

test('the lane named on the command line is the same lane wherever the options stand and however they are written',async t=>{
 // On main there is no lane row, so a lane that is dropped from the arguments is an error and one that is seen succeeds.
 const repo=await repoWithTools(t,{'README.md':'fixture\n','.gitignore':'scripts/\n'});
 const head=await git(repo,'rev-parse','HEAD');
 for(const args of [['--head='+head,'storage'],['storage','--head='+head],['--head',head,'storage'],['storage','--head',head],
  ['--base=main','storage'],['--base','main','storage'],['--unknown-flag','storage']]){
  const result=await devIn(repo,'scope',...args);
  assert.equal(result.code,0,args.join(" ")+": "+result.stderr);
  assert.match(result.stdout,/^scope: 0 changed path\(s\) since [0-9a-f]{40}, all inside storage\.$/m,args.join(' '));
 }
 // The value of a two-token option is not a lane.
 const lonely=await devIn(repo,'scope','--head',head);
 assert.equal(lonely.code,1);
 assert.match(lonely.stderr,/No scope row for branch main/);
});

test('concurrent cases settle before cleanup, and every failing case is named',async()=>{
 // A stand-in for the test: its cleanups run when this test says, after the cases.
 const cleanups=[];
 const scope={after:cleanup=>{cleanups.push(cleanup);}};
 const bases=[];
 const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
 const cases=[['fails at once',0],['succeeds slowly',300],['fails while a sibling is still copying',50]];
 const failure=await eachCase(cases,async ([name,delay])=>{
  const {base}=await fixtureRepo(scope);
  bases.push(base);
  await sleep(delay);
  if(name.startsWith('fails'))throw new Error(name+' on purpose');
  return name;
 }).then(()=>null,error=>error);
 assert.ok(failure,'two of three cases failed, so the call fails');
 assert.match(failure.message,/^2 of 3 cases failed:\n- fails at once: fails at once on purpose\n- fails while a sibling is still copying: fails while a sibling is still copying on purpose$/);
 assert.equal(failure.errors.length,2);
 // Every case, including the slow sibling that outlived both failures, had finished before the call returned.
 assert.equal(bases.length,3);
 assert.ok(bases.every(base=>existsSync(base)),'the copies are still there until the cleanup runs');
 for(const cleanup of cleanups.reverse())await cleanup();
 assert.deepEqual(bases.filter(base=>existsSync(base)),[],'no scratch directory of any case is left behind');
 assert.deepEqual(await eachCase(['a','b'],async name=>name+name),['aa','bb'],'when every case passes the results come back in order');
});
