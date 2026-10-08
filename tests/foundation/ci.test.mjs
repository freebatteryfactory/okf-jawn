/** The workflow runs the premerge sequence step by step; this file keeps the two from drifting. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { premergeSteps } from '../../scripts/lib/gates.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');
const workflow=Bun.YAML.parse(await read('.github/workflows/ci.yml'));
const always='${{ !cancelled() }}';
const prefix='bun scripts/dev.mjs premerge --step ';
// Commits whose action.yml declares `using: node20`: checkout v4.2.2, upload-artifact v4.6.2.
const node20Pins=['11bd71901bbe5b1630ceea73d27597364c9af683','ea165f8d65b6e75b540449e92b4886f43607fa02'];
const checkout=job=>workflow.jobs[job].steps.find(step=>step.uses?.startsWith('actions/checkout@'));

test('CI runs exactly the premerge sequence, one step each, and a failure does not hide later steps',()=>{
 const steps=workflow.jobs.foundation.steps;
 const gated=steps.filter(step=>typeof step.run==='string'&&step.run.startsWith(prefix));
 assert.deepEqual(gated.map(step=>step.run.slice(prefix.length)),premergeSteps(root).map(step=>step.id));
 for(const step of gated)assert.equal(step.if,always,step.run);
 const bootstrap=steps.findIndex(step=>step.run==='bun scripts/dev.mjs bootstrap');
 assert.ok(bootstrap>=0&&bootstrap<steps.indexOf(gated[0]),'bootstrap must precede the gated steps');
 assert.equal(steps.find(step=>step.uses?.startsWith('actions/upload-artifact@')).if,always);
 assert.equal(checkout('foundation').with['fetch-depth'],0,'check-receipts compares against history');
});
test('the job that runs the real generators fails when generation changed the checkout, before any gated step',async()=>{
 const steps=workflow.jobs.foundation.steps;
 // bootstrap is the step that generates: it replaces the committed generated directories.
 const entry=await read('scripts/dev.mjs');
 const bootstrapTask=entry.slice(entry.indexOf('async function bootstrap()'),entry.indexOf('async function vendor()'));
 assert.match(bootstrapTask,/await generate\(root\);/,'bootstrap no longer generates, so the clean-tree step guards nothing');
 const generation=steps.findIndex(step=>step.run==='bun scripts/dev.mjs bootstrap');
 const clean=steps.findIndex(step=>step.name==='The checkout is unchanged by generation');
 assert.ok(generation>=0,'no bootstrap step');
 assert.equal(clean,generation+1,'the clean-tree step must directly follow generation');
 const gated=steps.map((step,index)=>typeof step.run==='string'&&step.run.startsWith(prefix)?index:-1).filter(index=>index>=0);
 assert.ok(gated.length>0&&gated.every(index=>index>clean),'the clean-tree step must precede every gated step, the tests among them');
 const step=steps[clean];
 // It runs only after a bootstrap that succeeded: a half-written tree proves nothing either way.
 assert.equal(step.if,undefined);
 assert.equal(step['continue-on-error'],undefined,'a changed checkout must fail the job');
 const lines=step.run.trim().split('\n').map(line=>line.trim());
 assert.equal(lines[0],'status="$(git status --porcelain --untracked-files=all)"','untracked files count: a generated file nobody committed is stale output too');
 assert.equal(lines[1],'if [ -n "$status" ]; then');
 assert.ok(lines.includes('git diff'),'the difference is printed');
 assert.deepEqual(lines.slice(-2),['exit 1','fi']);
 // Nothing between checkout and this step may hide a difference.
 for(const earlier of steps.slice(0,clean))assert.doesNotMatch(earlier.run??'',/git (stash|checkout|restore|reset|clean|add)\b/,earlier.run);
 // The directories it guards are the ones generation replaces, and all of them are tracked.
 const replaced=[...(await read('scripts/lib/generation.mjs')).matchAll(/\['[a-z]+', '([a-z/]+)'\]/g)].map(match=>match[1]);
 assert.deepEqual(replaced,['api','ui/src/api/generated','generated/cli']);
 const ignore=(await read('.gitignore')).split(/\r?\n/);
 for(const directory of replaced)assert.ok(!ignore.some(line=>line.replace(/^\//,'').replace(/\/$/,'')===directory),`${directory} is ignored, so git status could not see it change`);
});
test('Clippy and tests cover every feature, so no separate runtime job exists',()=>{
 for(const id of ['clippy','test']){
  const args=premergeSteps(root).find(step=>step.id===id).args;
  assert.ok(args.includes('--workspace')&&args.includes('--all-features'),id);
 }
 assert.equal(workflow.jobs['runtime-clippy'],undefined);
});
test('superseded runs are cancelled, cargo and bun are cached, and no action runs on Node 20',async()=>{
 assert.match(workflow.concurrency.group,/github\.ref/);
 assert.ok('cancel-in-progress' in workflow.concurrency);
 for(const job of ['foundation','lane']){
  const caches=workflow.jobs[job].steps.filter(step=>step.uses?.startsWith('actions/cache@')).map(step=>step.with.path);
  assert.ok(caches.some(path=>path.includes('~/.cargo/registry')&&/^target$/m.test(path)),`${job}: no cargo cache`);
  assert.ok(caches.some(path=>path.includes('~/.bun/install/cache')),`${job}: no bun cache`);
 }
 for(const file of (await readdir(join(root,'.github/workflows'))).filter(name=>name.endsWith('.yml'))){
  const uses=[...(await read(`.github/workflows/${file}`)).matchAll(/^\s*-?\s*uses:\s*(\S+)/gm)].map(match=>match[1]);
  assert.ok(uses.length>0,file);
  for(const action of uses){
   assert.match(action,/^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/,`${file}: ${action} is not pinned to a commit`);
   assert.ok(!node20Pins.includes(action.split('@')[1]),`${file}: ${action} runs on Node 20`);
  }
 }
});
test('lane branches run their own gate; audit runs on every push and on the schedule, not in the foundation job',()=>{
 const lane=workflow.jobs.lane;
 assert.match(lane.if,/startsWith\(github\.ref, 'refs\/heads\/build\/'\)/);
 assert.ok(lane.steps.some(step=>step.run?.includes('bun scripts/dev.mjs lane "${GITHUB_REF_NAME#build/}"')));
 assert.equal(checkout('lane').with['fetch-depth'],0,'the scope check compares against origin/main');
 const audit=workflow.jobs.audit;
 // No condition: an advisory shows on the push that brings it in, on any branch, and the nightly run catches new ones.
 assert.equal(audit.if,undefined,'the audit job runs on every push, on dispatch and on the schedule');
 assert.ok(audit.steps.some(step=>step.run==='bun scripts/dev.mjs audit'));
 assert.ok(!workflow.jobs.foundation.steps.some(step=>step.run?.includes('audit')));
 assert.ok(Array.isArray(workflow.on.schedule)&&workflow.on.schedule.length===1);
 for(const job of ['source-tooling','foundation'])assert.match(workflow.jobs[job].if,/github\.event_name != 'schedule'/,job);
});
test('a pull request from this repository runs CI once, and an upload that finds nothing fails',async()=>{
 assert.ok('push' in workflow.on&&'workflow_dispatch' in workflow.on);
 assert.ok(!('pull_request' in workflow.on),'pull_request would run CI a second time for a branch of this repository');
 const qualify=Bun.YAML.parse(await read('.github/workflows/qualify.yml'));
 const uploads=Object.values(qualify.jobs).flatMap(job=>job.steps).filter(step=>step.uses?.startsWith('actions/upload-artifact@'));
 assert.ok(uploads.length>0);
 for(const upload of uploads)assert.equal(upload.with['if-no-files-found'],'error',upload.with.name);
 const ciUploads=Object.values(workflow.jobs).flatMap(job=>job.steps).filter(step=>step.uses?.startsWith('actions/upload-artifact@'));
 assert.ok(ciUploads.length>0);
 for(const upload of ciUploads)assert.equal(upload.with['if-no-files-found'],'error',`ci.yml ${upload.with.name}`);
});
