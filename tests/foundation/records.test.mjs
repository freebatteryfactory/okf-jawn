/** Records, prose and scripts agree with the generated surface and with what was actually run. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
import { libraryGates } from '../../scripts/lib/receipts.mjs';
import { premergeSteps } from '../../scripts/lib/gates.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');
const record=async()=>JSON.parse(await read('verification.json'));
/** The fields of each kind of Phase 0 gate, in file order. A gate of kind ci carries no status: CI states it on every push. */
const gateFields={
 receipt:['id','kind','harness','receipt','criteria','status','covers'],
 ci:['id','kind','enforced_by','covers'],
 decision:['id','kind','decision','decided_on','decided_by','covers'],
};
/** The status-check contexts the ruleset on main requires; each is the name of a ci.yml job. */
const requiredChecks=['Dependency-free source tooling, not product acceptance','Actual pinned generators and consumers, every feature'];
/**
 * Phase 0 gates left as they were, each with the claim no CI step enforces. A gate leaves this
 * list by gaining that enforcement and a kind, never by losing the claim.
 */
const unconverted={
 'authored-typescript-seams':'no test gives PresentView a Vega-Lite specification that compile rejects, and none holds that main.tsx applies omitUndefined once at the wire boundary',
};

test('nothing tracked is also ignored, and no snapshot of the tree is kept',async()=>{
 await assert.rejects(read('REPOSITORY-TREE.txt'),{code:'ENOENT'});
 for(const file of ['README.md','AGENTS.md','.github/CODEOWNERS','justfile','scripts/dev.mjs'])assert.doesNotMatch(await read(file),/REPOSITORY-TREE/,file);
 assert.match(await read('scripts/dev.mjs'),/case 'tree':/,'the on-demand view stays');
 // A source copy without history (the build container) has no index to inspect.
 if(!existsSync(join(root,'.git')))return;
 const ignored=await run('git',['ls-files','-ci','--exclude-standard'],{cwd:root,capture:true});
 assert.equal(ignored.stdout.trim(),'','these tracked files are ignored by .gitignore');
});
test('verification.json records no pass while Phase 0 is reopened, and only terminal gates once qualified',async()=>{
 const record=JSON.parse(await read('verification.json'));
 const gates=record.current.gates.phase_0;
 assert.equal(new Set(gates.map(gate=>gate.id)).size,gates.length);
 for(const id of libraryGates)assert.ok(gates.some(gate=>gate.id===id),`Phase 0 gate ${id} is missing`);
 if(record.phase_0_qualified===true){
  assert.equal(record.current.deterministic_foundation_green,true);
  for(const gate of gates)assert.ok(['passed','rejected_with_fallback','bounded_upstream_exception'].includes(gate.status),`${gate.id} is ${gate.status} although phase_0_qualified is true`);
  return;
 }
 assert.equal(record.current.deterministic_foundation_green,false,'deterministic_foundation_green cannot be true while Phase 0 is unqualified');
 for(const gate of gates)assert.notEqual(gate.status,'passed',`${gate.id} is recorded as passed while Phase 0 is reopened`);
 for(const check of record.current.checks)assert.notEqual(check.status,'passed',`${check.command.join(' ')} is recorded as passed while Phase 0 is reopened`);
});
test('every Phase 0 gate has exactly one kind and only the fields of that kind',async()=>{
 const gates=(await record()).current.gates.phase_0;
 assert.equal(new Set(gates.map(gate=>gate.id)).size,gates.length,'a gate id is repeated');
 assert.deepEqual(gates.filter(gate=>gate.kind===undefined).map(gate=>gate.id),Object.keys(unconverted),'a gate without a kind must be listed in unconverted with what is missing');
 for(const gate of gates.filter(entry=>entry.kind!==undefined)){
  assert.ok(Object.hasOwn(gateFields,gate.kind),`${gate.id} has the unknown kind ${gate.kind}`);
  assert.deepEqual(Object.keys(gate),gateFields[gate.kind],`${gate.id} (${gate.kind})`);
  for(const field of ['id','covers'])assert.ok(typeof gate[field]==='string'&&gate[field].length>0,`${gate.id}.${field}`);
 }
 for(const gate of gates.filter(entry=>entry.kind==='receipt')){
  assert.match(gate.harness,/^[a-z0-9]+(?:-[a-z0-9]+)*$/,gate.id);
  // recordReceipt(root, name, receipt) writes qualification/receipts/<name>.json; the harness is that name.
  assert.equal(gate.receipt,`qualification/receipts/${gate.harness}.json`,gate.id);
  assert.equal(gate.criteria,`qualification/${gate.harness}/criteria.json`,gate.id);
  assert.ok(existsSync(join(root,'qualification',gate.harness,'run.mjs')),`${gate.id}: no harness at qualification/${gate.harness}/run.mjs`);
  assert.ok(['passed','failed','incomplete'].includes(gate.status),`${gate.id} status ${gate.status}`);
 }
 assert.ok(gates.some(gate=>gate.kind==='receipt'),'no gate is backed by a receipt');
 for(const gate of gates.filter(entry=>entry.kind==='decision')){
  assert.ok(typeof gate.decision==='string'&&gate.decision.length>0,gate.id);
  assert.match(gate.decided_on,/^\d{4}-\d{2}-\d{2}$/,gate.id);
  assert.equal(new Date(`${gate.decided_on}T00:00:00Z`).toISOString().slice(0,10),gate.decided_on,`${gate.id}: decided_on is not a calendar date`);
  assert.ok(['owner','integration-owner'].includes(gate.decided_by),`${gate.id} decided_by ${gate.decided_by}`);
 }
 // A gate that was not converted keeps its typed status, which is never a pass.
 for(const gate of gates.filter(entry=>entry.kind===undefined))assert.equal(gate.status,'incomplete',gate.id);
});
test('a ci gate names a job of the workflow by its check name, and that job runs the evidence',async()=>{
 const gates=(await record()).current.gates.phase_0.filter(gate=>gate.kind==='ci');
 assert.ok(gates.length>0);
 const premerge=premergeSteps(root);
 const members=/^members = \[([^\]]+)\]/m.exec(await read('Cargo.toml'))?.[1].split(',').map(entry=>entry.trim().replace(/"/g,''))??[];
 const vitest=await read('ui/vitest.config.ts');
 for(const gate of gates){
  assert.deepEqual(Object.keys(gate.enforced_by),['workflow','job','check','evidence'],gate.id);
  const {workflow:file,job:id,check,evidence}=gate.enforced_by;
  assert.equal(file,'.github/workflows/ci.yml',gate.id);
  const job=Bun.YAML.parse(await read(file)).jobs[id];
  assert.ok(job,`${gate.id}: ${file} has no job ${id}`);
  assert.equal(job.name,check,`${gate.id}: job ${id} is not named ${check}`);
  assert.ok(requiredChecks.includes(check),`${gate.id}: ${check} is not a check the ruleset on main requires`);
  assert.match(job.if,/^github\.event_name != 'schedule'$/,`${gate.id}: job ${id} must run on every push`);
  assert.ok(job.steps.some(step=>step.uses?.startsWith('actions/checkout@')),`${gate.id}: job ${id} does not start from a clone`);
  const runs=job.steps.map(step=>step.run).filter(run=>typeof run==='string');
  assert.ok(Array.isArray(evidence)&&evidence.length>0,`${gate.id}: no evidence`);
  assert.equal(new Set(evidence).size,evidence.length,`${gate.id}: evidence is repeated`);
  for(const entry of evidence){
   const where=`${gate.id}: evidence ${entry}`;
   if(entry.startsWith('bun ')){
    // A step command: the job runs exactly this line, and a premerge step is one that exists.
    assert.ok(runs.includes(entry),`${where} is not a step of job ${id}`);
    const step=/^bun scripts\/dev\.mjs premerge --step (\S+)$/.exec(entry)?.[1];
    if(step!==undefined)assert.ok(premerge.some(candidate=>candidate.id===step),`${where} names no premerge step`);
    continue;
   }
   // A test file: it exists, and a step of this job is the one that runs files of its sort.
   assert.ok(existsSync(join(root,entry)),`${where} does not exist`);
   const rust=/^(crates\/[^/]+|xtask)\/tests\/[^/]+\.rs$/.exec(entry);
   if(rust){
    assert.ok(members.includes(rust[1]),`${where}: ${rust[1]} is not a workspace member`);
    const args=premerge.find(candidate=>candidate.id==='test').args;
    assert.ok(args[0]==='test'&&args.includes('--workspace')&&args.includes('--all-features'),`${where}: the test step does not cover the workspace`);
    assert.ok(runs.includes('bun scripts/dev.mjs premerge --step test'),`${where}: job ${id} does not run the workspace tests`);
   }else if(/^ui\/tests\/unit\/.+\.test\.tsx?$/.test(entry)){
    assert.ok(vitest.includes("'tests/unit/**/*.test.{ts,tsx}'"),`${where}: ui/vitest.config.ts does not include it`);
    assert.deepEqual(premerge.find(candidate=>candidate.id==='ui-test').args,['--bun','run','test'],where);
    assert.ok(runs.includes('bun scripts/dev.mjs premerge --step ui-test'),`${where}: job ${id} does not run the UI unit tests`);
   }else if(/^tests\/foundation\/[^/]+\.test\.mjs$/.test(entry)){
    assert.ok(runs.some(run=>run==='bun scripts/dev.mjs check-offline'||run==='bun scripts/dev.mjs premerge --step check-offline'),`${where}: job ${id} does not run the offline suite`);
   }else assert.fail(`${where}: no rule says which CI step runs a file of this sort`);
  }
 }
 // The ruleset lives on GitHub, not in the tree. It requires these two contexts by name, so a
 // renamed job would never report its required check; each one has a job and a gate here.
 const names=Object.values(Bun.YAML.parse(await read('.github/workflows/ci.yml')).jobs).map(job=>job.name);
 for(const check of requiredChecks){
  assert.ok(names.includes(check),`ci.yml has no job named ${check}`);
  assert.ok(gates.some(gate=>gate.enforced_by.check===check),`no ci gate names the required check ${check}`);
 }
});
test('no gate text carries a commit hash or a CI run number; history lives in git',async()=>{
 const groups=(await record()).current.gates;
 assert.deepEqual(Object.keys(groups),['phase_0','construction','acceptance']);
 // Seven or more hex digits in a row: a short or full commit, or a run number.
 const hashLike=/\b[0-9a-f]{7,40}\b/i;
 const strings=value=>typeof value==='string'?[value]:value!==null&&typeof value==='object'?Object.values(value).flatMap(strings):[];
 let scanned=0;
 for(const [group,gates] of Object.entries(groups))for(const gate of gates){
  if(group==='phase_0'&&Object.hasOwn(unconverted,gate.id))continue;
  for(const text of strings(gate)){
   scanned+=1;
   assert.doesNotMatch(text,hashLike,`${group} gate ${gate.id}`);
   assert.doesNotMatch(text,/actions\/runs\/|\bCI \d+/,`${group} gate ${gate.id}`);
  }
 }
 assert.ok(scanned>50,'the scan read almost nothing');
});
test('the acceptance script calls only declared operations, and edits through a draft and a Snapshot',async()=>{
 const paths=new Set(JSON.parse(await read('api/operations.json')).map(operation=>operation.path));
 const source=await read('tests/integration/acceptance.mjs');
 const calls=[...source.matchAll(/call\('([a-z]+)','([a-z_]+)'/g)];
 assert.ok(calls.length>0);
 for(const [,domain,id] of calls)assert.ok(paths.has(`/api/${domain}/${id.replaceAll('_','-')}`),`acceptance.mjs calls ${domain}/${id}, which is not a declared operation`);
 assert.match(source,/call\('items','save_draft'/);
 assert.match(source,/call\('history','commit_items',\{workspace_id,item_ids:\[item_id\],message:/);
});
test('the deployment example declares and describes every image the compose file and Dockerfile require',async()=>{
 const example=await read('deploy/.env.example');
 const required=[...(await read('deploy/compose.yaml')).matchAll(/\$\{([A-Z_]+):\?/g)].map(match=>match[1]);
 const images=[...(await read('deploy/Dockerfile')).matchAll(/^ARG ([A-Z_]+)$/gm)].map(match=>match[1]);
 assert.ok(required.length>0&&images.length>0);
 for(const name of new Set([...required,...images]))assert.match(example,new RegExp(`^${name}=`,'m'),`deploy/.env.example does not declare ${name}`);
 const described=name=>example.split(/\r?\n/).find(line=>line.startsWith(`# ${name}:`))??'';
 for(const name of images)assert.ok(described(name).length>0,`${name} has no description`);
 assert.match(described('BUN_IMAGE'),/\.bun-version/,'BUN_IMAGE must be described the way compose.yaml and the Dockerfile require it');
});
test('README states the generated counts and tool names, and neither prose file remembers a number',async()=>{
 const readme=await read('README.md'),agents=await read('AGENTS.md');
 const operations=JSON.parse(await read('api/operations.json')),transports=JSON.parse(await read('api/transports.json'));
 const tools=JSON.parse(await read('api/mcp-tools.json')).tools;
 assert.ok(readme.includes(`There are ${operations.length} typed JSON application commands and ${transports.length} distinct transport declarations.`),'README operation and transport counts differ from api/');
 for(const tool of tools.filter(entry=>entry._meta.ui.visibility.includes('model')))assert.ok(readme.includes(`\`${tool.name}\``),`README does not name the model tool ${tool.name}`);
 assert.doesNotMatch(readme,/Utoipa schema types/);
 for(const [name,text] of [['README.md',readme],['AGENTS.md',agents]])assert.doesNotMatch(text,/\b(?:three|3)\b[^.\n]*Context7/i,name);
 for(const term of ['TestResult','err_of','git merge --no-ff','Why:','PowerShell'])assert.ok(agents.includes(term),`AGENTS.md does not state ${term}`);
});
test('help names every task the entrypoint accepts, and just mirrors the gates',async()=>{
 const entry=await read('scripts/dev.mjs');
 const cases=[...entry.matchAll(/^\s+case '([a-z-]+)':/gm)].map(match=>match[1]).filter(name=>name!=='help');
 const help=/case 'help': process\.stdout\.write\('Tasks: ([^\\]+)\\n'\)/.exec(entry)?.[1].split(' ')??[];
 assert.deepEqual([...help].sort(),[...cases].sort());
 const just=await read('justfile');
 for(const recipe of ['lane','premerge','clean-checkout','lanes-reset','check-receipts'])assert.match(just,new RegExp(`^${recipe}\\b`,'m'),`justfile has no ${recipe} recipe`);
});
