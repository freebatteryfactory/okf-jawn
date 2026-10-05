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
test('lane branches run their own gate; audit runs on main and on a schedule, not in the foundation job',()=>{
 const lane=workflow.jobs.lane;
 assert.match(lane.if,/startsWith\(github\.ref, 'refs\/heads\/build\/'\)/);
 assert.ok(lane.steps.some(step=>step.run?.includes('bun scripts/dev.mjs lane "${GITHUB_REF_NAME#build/}"')));
 assert.equal(checkout('lane').with['fetch-depth'],0,'the scope check compares against origin/main');
 const audit=workflow.jobs.audit;
 assert.match(audit.if,/schedule/);assert.match(audit.if,/refs\/heads\/main/);
 assert.ok(audit.steps.some(step=>step.run==='bun scripts/dev.mjs audit'));
 assert.ok(!workflow.jobs.foundation.steps.some(step=>step.run?.includes('audit')));
 assert.ok(Array.isArray(workflow.on.schedule)&&workflow.on.schedule.length===1);
 for(const job of ['source-tooling','foundation'])assert.match(workflow.jobs[job].if,/github\.event_name != 'schedule'/,job);
});
