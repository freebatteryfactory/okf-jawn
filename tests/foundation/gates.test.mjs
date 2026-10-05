/** Gate sequences are data; the runner logs every step and ends with one verdict line. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { cleanCheckout, cleanCheckoutTasks, laneSteps, premergeSteps, runLane, runPremerge } from '../../scripts/lib/gates.mjs';
import { laneNamed, lanes } from '../../scripts/lib/lanes.mjs';
import { fixtureRepo, git } from './fixture-repo.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');
const fixture={'.gitignore':'/.artifacts/\n','README.md':'fixture\n'};
const passing=async(step,write)=>{write(`ran ${step.id}\n`);return 0;};
const quiet={echo:()=>{}};
const lastLine=async path=>(await readFile(path,'utf8')).trimEnd().split('\n').at(-1);

test('a Rust lane gate is fmt, Clippy and tests for its crates and features, source policy, then scope',()=>{
 const steps=laneSteps('/repo',laneNamed('storage'));
 assert.deepEqual(steps.map(step=>step.id),['fmt','clippy','test','source-policy','scope']);
 assert.deepEqual(steps.slice(0,4).map(step=>step.command),['cargo','cargo','cargo','cargo']);
 assert.deepEqual(steps[0].args,['fmt','--check','-p','okf-jawn-storage']);
 assert.deepEqual(steps[1].args,['clippy','--locked','-p','okf-jawn-storage','--features','okf-jawn-storage/runtime','--all-targets','--','-D','warnings']);
 assert.deepEqual(steps[2].args,['test','--locked','-p','okf-jawn-storage','--features','okf-jawn-storage/runtime']);
 assert.deepEqual(steps[3].args,['xtask','source-policy','--root','/repo']);
 assert.deepEqual(steps[4].args,['scripts/dev.mjs','scope','storage']);
 const core=laneSteps('/repo',laneNamed('core-cli'));
 assert.deepEqual(core[1].args,['clippy','--locked','-p','okf-jawn-core','-p','okf-jawn-cli','--all-targets','--','-D','warnings']);
 assert.deepEqual(core[2].args,['test','--locked','-p','okf-jawn-core','-p','okf-jawn-cli']);
});
test('every Rust lane checks the formatting of its own crates only, never the workspace',()=>{
 for(const lane of lanes.filter(entry=>entry.kind==='rust')){
  const args=laneSteps('/repo',lane)[0].args;
  assert.deepEqual(args,['fmt','--check',...lane.crates.flatMap(name=>['-p',name])],lane.name);
  assert.ok(!args.includes('--all'),lane.name);
 }
});
test('a UI lane gate is Biome, route generation, tsc, Vitest filtered to the lane, then scope',async()=>{
 const views=laneSteps('/repo',laneNamed('views'));
 assert.deepEqual(views.map(step=>step.id),['biome','routes','typecheck','test','scope']);
 assert.deepEqual(views[0].args,['--bun','run','lint']);
 assert.deepEqual(views[1].args,['scripts/dev.mjs','routes']);
 assert.deepEqual(views[2].args,['--bun','run','typecheck']);
 assert.deepEqual(views[3].args,['--bun','run','test',...laneNamed('views').tests]);
 assert.ok(views[3].cwd.endsWith('ui'));
 const workspace=laneSteps('/repo',laneNamed('workspace-ui'))[3].args;
 assert.deepEqual(workspace.slice(0,5),['--bun','run','test','--exclude','src/features/views/**']);
 assert.equal(workspace.filter(argument=>argument==='--exclude').length,laneNamed('workspace-ui').testExclude.length);
 const config=await read('ui/vitest.config.ts');
 for(const glob of ['tests/unit/**/*.test.{ts,tsx}','src/**/*.test.{ts,tsx}'])assert.ok(config.includes(`'${glob}'`),`ui/vitest.config.ts does not include ${glob}`);
 const entry=await read('scripts/dev.mjs');
 assert.match(entry,/resolveConfig\(\{\}, 'build'\)/);assert.match(entry,/case 'routes':/);
});
test('every lane has a gate, and the lane task ends its log with one verdict line',async t=>{
 for(const lane of lanes)assert.equal(laneSteps('/repo',lane).at(-1).id,'scope',lane.name);
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const result=await runLane(repo,'storage',{execute:passing,...quiet});
 assert.equal(result.passed,true);assert.equal(result.line,`PASS storage ${sha}`);
 assert.ok(result.logPath.endsWith(join('.artifacts','lane','storage',`${sha}.log`)));
 const log=await readFile(result.logPath,'utf8');
 assert.equal(await lastLine(result.logPath),`PASS storage ${sha}`);
 for(const id of ['fmt','clippy','test','source-policy','scope'])assert.ok(log.includes(`=== ${id}: `)&&log.includes(`ran ${id}\n=== ${id} exit 0\n`),id);
});
test('a failing step stops the lane gate and is named; an uncommitted tree is never labelled as its commit',async t=>{
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const ran=[];
 const failing=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);return step.id==='clippy'?101:0;};
 const result=await runLane(repo,'storage',{execute:failing,...quiet});
 assert.equal(result.passed,false);assert.equal(result.line,`FAIL storage ${sha} clippy`);
 assert.deepEqual(ran,['fmt','clippy']);
 assert.equal(await lastLine(result.logPath),`FAIL storage ${sha} clippy`);
 await writeFile(join(repo,'README.md'),'edited, not committed\n');
 assert.equal((await runLane(repo,'storage',{execute:passing,...quiet})).line,`PASS storage ${sha}-dirty`);
 await assert.rejects(runLane(repo,'no-such-lane',{execute:passing,...quiet}),/Unknown lane: no-such-lane/);
});
test('premerge is the whole CI sequence with every feature, and includes check-receipts',()=>{
 const steps=premergeSteps('/repo');
 assert.deepEqual(steps.map(step=>step.id),['check-offline','gen-check','fmt','clippy','source-policy','test','ui-lint','ui-build','ui-typecheck','ui-test','check-receipts']);
 const args=id=>steps.find(step=>step.id===id).args;
 assert.deepEqual(args('clippy'),['clippy','--locked','--workspace','--all-features','--all-targets','--','-D','warnings']);
 assert.deepEqual(args('test'),['test','--locked','--workspace','--all-features']);
 assert.deepEqual(args('gen-check'),['scripts/dev.mjs','gen-check']);
 assert.deepEqual(args('check-receipts'),['scripts/dev.mjs','check-receipts']);
});
test('premerge runs every step and reports all failures, not the first',async t=>{
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const ran=[];
 const execute=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);return ['fmt','ui-test'].includes(step.id)?1:0;};
 const result=await runPremerge(repo,{execute,...quiet});
 assert.deepEqual(ran,premergeSteps(repo).map(step=>step.id));
 assert.equal(result.passed,false);assert.equal(result.line,`FAIL premerge ${sha} fmt,ui-test`);
 assert.equal(await lastLine(result.logPath),`FAIL premerge ${sha} fmt,ui-test`);
 assert.equal((await runPremerge(repo,{execute:passing,...quiet})).line,`PASS premerge ${sha}`);
 await assert.rejects(runPremerge(repo,{only:'no-such-step'}),/Unknown premerge step: no-such-step/);
});
test('check and test are subsets of premerge, so the cargo flags are written once',async()=>{
 const entry=await read('scripts/dev.mjs');
 assert.match(entry,/case 'check': await runNamed\(\['fmt', 'clippy', 'source-policy', 'ui-lint', 'ui-typecheck', 'gen-check'\]\); break;/);
 assert.match(entry,/case 'test': await runNamed\(\['test', 'ui-test'\]\); break;/);
 assert.doesNotMatch(entry,/'--all-targets'|'--all-features'/);
});
test('clean-checkout runs the whole foundation in a detached worktree of HEAD and removes it',async t=>{
 const {base,root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const parent=join(base,'lanes');const worktree=join(parent,`clean-checkout-${sha.slice(0,12)}`);const ran=[];
 const execute=async(step,write)=>{ran.push([step.id,step.cwd,step.args.join(' ')]);write(`ran ${step.id}\n`);return 0;};
 const result=await cleanCheckout(repo,{parent,execute,...quiet,now:()=>new Date('2026-10-05T18:00:00Z')});
 assert.deepEqual(ran.map(([id])=>id),['bootstrap','gen-check-1','gen-check-2','foundation','check','test','check-offline','audit']);
 assert.deepEqual(ran.map(([,,args])=>args),['bootstrap','gen-check','gen-check','foundation','check','test','check-offline','audit'].map(name=>`scripts/dev.mjs ${name}`));
 for(const [,cwd] of ran)assert.equal(cwd,worktree);
 assert.deepEqual(result.receipt,{git_sha:sha,inputs:['.'],produced_at:'2026-10-05T18:00:00.000Z',exit_codes:Object.fromEntries(ran.map(([id])=>[id,0])),git_status_empty:true});
 assert.deepEqual(Object.keys(result.receipt),['git_sha','inputs','produced_at','exit_codes','git_status_empty']);
 assert.deepEqual(JSON.parse(await readFile(join(repo,'.artifacts','qualification','clean-checkout','receipt.json'),'utf8')),result.receipt);
 assert.equal(result.passed,true);assert.equal(result.removed,true);
 assert.equal(existsSync(worktree),false);
 assert.equal((await git(repo,'worktree','list','--porcelain')).split(/\r?\n/).filter(line=>line.startsWith('worktree ')).length,1);
});
test('clean-checkout records a failing step and a drifted tree, keeps the evidence, and never forces',async t=>{
 const {base,root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const parent=join(base,'lanes');const ran=[];
 const execute=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);if(step.id==='gen-check-2')await writeFile(join(step.cwd,'drift.json'),'{}');return step.id==='check'?3:0;};
 const result=await cleanCheckout(repo,{parent,execute,...quiet});
 assert.equal(ran.length,cleanCheckoutTasks.length,'a failing step does not stop the run');
 assert.equal(result.receipt.exit_codes.check,3);assert.equal(result.receipt.git_status_empty,false);
 assert.equal(result.passed,false);assert.equal(result.removed,false);
 assert.equal(existsSync(join(parent,`clean-checkout-${sha.slice(0,12)}`,'drift.json')),true,'a drifted worktree is left for inspection');
 await assert.rejects(cleanCheckout(repo,{parent,execute,...quiet}),/clean-checkout refused: .* already exists/);
 // `-D` is Clippy's deny flag here, so only the force spellings are forbidden in this file.
 assert.doesNotMatch(await read('scripts/lib/gates.mjs'),/'--force'|'-f'/,'gates.mjs must never force git');
});
