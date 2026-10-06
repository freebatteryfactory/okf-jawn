/** Records, prose and scripts agree with the generated surface and with what was actually run. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
import { acceptedFailureFields, derivedRecord, receiptGateStatuses } from '../../scripts/lib/receipts.mjs';
import { premergeSteps } from '../../scripts/lib/gates.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');
const record=async()=>JSON.parse(await read('verification.json'));
/**
 * The fields of each kind of Phase 0 gate, in file order. A gate of kind ci carries no status: CI
 * states it on every push. A receipt gate may also carry `accepted_failures` (after `status`):
 * the failing criteria the owner accepted, which check-receipts holds to its own rules.
 */
const gateFields={
 receipt:['id','kind','harness','receipt','criteria','status','covers'],
 ci:['id','kind','enforced_by','covers'],
 decision:['id','kind','decision','decided_on','decided_by','covers'],
};
/** The status-check contexts the ruleset on main requires; each is the name of a ci.yml job. */
const requiredChecks=['Dependency-free source tooling, not product acceptance','Actual pinned generators and consumers, every feature'];
/**
 * Phase 0 gates left as they were, each with the claim no CI step enforces. A gate leaves this
 * list by gaining that enforcement and a kind, never by losing the claim. None is left:
 * authored-typescript-seams was the last, and became a ci gate when ui/tests/unit gained a test
 * for each of its two unenforced claims.
 */
const unconverted={};
/**
 * Every field of verification.json outside the archive, in file order. Each has one verdict:
 * derived by a tool and verified (phase_0_qualified, receipt gate statuses), checked against
 * the tree by a test below, or a decision with a date and a role. A field in neither list fails.
 */
const recordFields={
 top:['project','phase_0_qualified','reopened','application_implemented','current','historical_archive_record'],
 current:['direct_versions','typescript_policy','typescript_boundary_fixes','advisories_remaining','gates'],
};
/** Fields that were typed claims CI enforces, a tool derives or history narrates; none may come back. `delivery` restated a status in prose ("not a ... qualified application") that phase_0_qualified derives. */
const removedFields=['delivery','recorded_at_utc','base_commit','environment','lockfiles_resolved','lockfile_provenance','rust_compiled','rust_generator_executed','frontend_generation_executed',
 'generation_deterministic','project_typescript_checked','project_typescript_passed','authored_typescript_error_count','deterministic_foundation_green','phase_0_note','checks','code_tested_commits'];
/** SHA-256 of the file's text from the `historical_archive_record` key to its end, as first committed. */
const archiveDigest='8a2e38568aa78c9292d722e09c89e31cbd2e72e953c742d1c457565d52a1a1f2';
/** A decision names what was decided, the calendar day and the role that decided it. */
function assertDecision(entry,where){
 assert.ok(typeof entry.decision==='string'&&entry.decision.length>0,`${where}.decision`);
 assert.match(entry.decided_on,/^\d{4}-\d{2}-\d{2}$/,`${where}.decided_on`);
 assert.equal(new Date(`${entry.decided_on}T00:00:00Z`).toISOString().slice(0,10),entry.decided_on,`${where}: decided_on is not a calendar date`);
 assert.ok(['owner','integration-owner'].includes(entry.decided_by),`${where}.decided_by is ${entry.decided_by}`);
}

test('nothing tracked is also ignored, and no snapshot of the tree is kept',async()=>{
 await assert.rejects(read('REPOSITORY-TREE.txt'),{code:'ENOENT'});
 for(const file of ['README.md','AGENTS.md','.github/CODEOWNERS','justfile','scripts/dev.mjs'])assert.doesNotMatch(await read(file),/REPOSITORY-TREE/,file);
 assert.match(await read('scripts/dev.mjs'),/case 'tree':/,'the on-demand view stays');
 // A source copy without history (the build container) has no index to inspect.
 if(!existsSync(join(root,'.git')))return;
 const ignored=await run('git',['ls-files','-ci','--exclude-standard'],{cwd:root,capture:true});
 assert.equal(ignored.stdout.trim(),'','these tracked files are ignored by .gitignore');
});
test('verification.json types what its receipts derive, in the form record.mjs rewrites',async()=>{
 const typed=await record();
 // The comparison check-receipts makes, without git: a hand edit of a status or of phase_0_qualified fails here too.
 const derived=await derivedRecord(root);
 assert.deepEqual(derived.failures,[]);
 assert.deepEqual(derived.mismatches,[]);
 assert.equal(typed.phase_0_qualified,derived.phase_0_qualified);
 for(const gate of derived.gates)assert.equal(typed.current.gates.phase_0.find(entry=>entry.id===gate.id).status,gate.status,gate.id);
 assert.equal(await read('verification.json'),`${JSON.stringify(typed,null,2)}\n`,'verification.json is not in two-space JSON form, so record.mjs could not rewrite only the derived values');
});
test('the record holds no hand-typed result: no "passed" outside a receipt gate, no boolean claim, no field CI or a tool states',async()=>{
 const typed=await record();
 assert.deepEqual(Object.keys(typed),recordFields.top,'a new top-level field needs a verdict first: derived, checked against the tree, or a decision');
 assert.deepEqual(Object.keys(typed.current),recordFields.current,'a new field of current needs a verdict first: derived, checked against the tree, or a decision');
 for(const field of removedFields)assert.ok(!Object.hasOwn(typed.current,field)&&!Object.hasOwn(typed,field),`${field} was removed: CI enforces it, a tool derives it, or it narrated history`);
 const walk=(value,path,visit)=>{visit(value,path);if(value!==null&&typeof value==='object')for(const [key,child] of Object.entries(value))walk(child,[...path,key],visit);};
 const receiptStatus=path=>path.length===4&&path[0]==='gates'&&path[1]==='phase_0'&&path[3]==='status'&&typed.current.gates.phase_0[path[2]].kind==='receipt';
 let seen=0;
 walk(typed.current,[],(value,path)=>{
  seen+=1;
  assert.notEqual(typeof value,'boolean',`current.${path.join('.')} is a typed boolean claim`);
  if(value==='passed')assert.ok(receiptStatus(path),`current.${path.join('.')} types "passed" outside a gate of kind receipt`);
 });
 assert.ok(seen>100,'the walk read almost nothing');
 // Two booleans remain at the top. phase_0_qualified is derived (the test above). Nothing derives
 // application_implemented yet, so it cannot be typed true: acceptance gates need receipts first.
 assert.deepEqual(Object.entries(typed).filter(([,value])=>typeof value==='boolean').map(([key])=>key),['phase_0_qualified','application_implemented']);
 assert.equal(typed.application_implemented,false);
 // Why Phase 0 was reopened is a decision, not a status.
 assert.deepEqual(Object.keys(typed.reopened),['decision','decided_on','decided_by']);
 assertDecision(typed.reopened,'reopened');
});
test('the archive record is kept byte for byte',async()=>{
 const text=await read('verification.json');
 const start=text.indexOf('  "historical_archive_record": {');
 assert.ok(start>0);
 assert.equal(createHash('sha256').update(text.slice(start)).digest('hex'),archiveDigest,'historical_archive_record changed; it is history and stays as it was written');
});
test('every version the record names is the pin in the manifests',async()=>{
 const versions=(await record()).current.direct_versions;
 const cargo=await read('Cargo.toml');
 const ui=JSON.parse(await read('ui/package.json'));
 const packages={...ui.dependencies,...ui.devDependencies};
 assert.ok(Object.keys(versions).length>0);
 for(const [name,stated] of Object.entries(versions)){
  const line=new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')} = \\{ version = "=([^"]+)"(.*)\\}$`,'m').exec(cargo);
  if(!line){
   assert.ok(Object.hasOwn(packages,name),`${name} is pinned in neither Cargo.toml nor ui/package.json`);
   assert.equal(stated,packages[name],`${name}: the record and ui/package.json differ`);
   continue;
  }
  // A crate: the version, then a note that restates the feature selection of the same line.
  assert.equal(stated.split(' ')[0],line[1],`${name}: the record and Cargo.toml differ`);
  assert.equal(stated.includes('default-features=false'),line[2].includes('default-features = false'),`${name}: default-features`);
  const features=/features=\[([^\]]*)\]/.exec(stated.replace('default-features=',''))?.[1];
  const declared=/(?<!default-)features = \[([^\]]*)\]/.exec(line[2])?.[1].replace(/["\s]/g,'');
  assert.equal(features,declared,`${name}: features`);
 }
});
test('the advisories the record calls remaining are exactly the ones the audit configuration ignores',async()=>{
 const remaining=(await record()).current.advisories_remaining;
 const ids=prefix=>remaining.filter(entry=>entry.id.startsWith(prefix)).map(entry=>entry.id).sort();
 const rustsec=ids('RUSTSEC-');
 assert.ok(rustsec.length>0);
 assert.deepEqual([...(await read('deny.toml')).matchAll(/\{ id = "(RUSTSEC-\d{4}-\d{4})"/g)].map(match=>match[1]).sort(),rustsec,'deny.toml ignores a different set');
 assert.deepEqual([.../^ignore = \[([^\]]*)\]/m.exec(await read('.cargo/audit.toml'))[1].matchAll(/"([^"]+)"/g)].map(match=>match[1]).sort(),rustsec,'.cargo/audit.toml ignores a different set');
 assert.deepEqual(ids('GHSA-'),[/const bracesException = '([^']+)'/.exec(await read('scripts/dev.mjs'))?.[1]],'the audit task ignores a different Bun advisory');
 assert.equal(rustsec.length+ids('GHSA-').length,remaining.length,'an advisory that no audit configuration ignores is not "remaining"');
});
test('every Phase 0 gate has exactly one kind and only the fields of that kind',async()=>{
 const gates=(await record()).current.gates.phase_0;
 assert.equal(new Set(gates.map(gate=>gate.id)).size,gates.length,'a gate id is repeated');
 assert.deepEqual(gates.filter(gate=>gate.kind===undefined).map(gate=>gate.id),Object.keys(unconverted),'a gate without a kind must be listed in unconverted with what is missing');
 for(const gate of gates.filter(entry=>entry.kind!==undefined)){
  assert.ok(Object.hasOwn(gateFields,gate.kind),`${gate.id} has the unknown kind ${gate.kind}`);
  const fields=gate.kind==='receipt'&&Object.hasOwn(gate,'accepted_failures')?gateFields.receipt.flatMap(field=>field==='status'?[field,'accepted_failures']:[field]):gateFields[gate.kind];
  assert.deepEqual(Object.keys(gate),fields,`${gate.id} (${gate.kind})`);
  for(const field of ['id','covers'])assert.ok(typeof gate[field]==='string'&&gate[field].length>0,`${gate.id}.${field}`);
 }
 for(const gate of gates.filter(entry=>entry.kind==='receipt')){
  assert.match(gate.harness,/^[a-z0-9]+(?:-[a-z0-9]+)*$/,gate.id);
  // recordReceipt(root, name, receipt) writes qualification/receipts/<name>.json; the harness is that name.
  assert.equal(gate.receipt,`qualification/receipts/${gate.harness}.json`,gate.id);
  assert.equal(gate.criteria,`qualification/${gate.harness}/criteria.json`,gate.id);
  assert.ok(existsSync(join(root,'qualification',gate.harness,'run.mjs')),`${gate.id}: no harness at qualification/${gate.harness}/run.mjs`);
  assert.ok(receiptGateStatuses.includes(gate.status),`${gate.id} status ${gate.status}`);
  // What an acceptance must be is check-receipts' rule (receipts.test.mjs); here only its shape in the file.
  for(const entry of gate.accepted_failures??[])assert.deepEqual(Object.keys(entry),[...acceptedFailureFields],`${gate.id}: an accepted failure has exactly these fields, in this order`);
 }
 for(const gate of gates.filter(entry=>entry.kind!=='receipt'))assert.ok(!Object.hasOwn(gate,'accepted_failures'),`${gate.id}: only a receipt gate can accept a failing criterion`);
 assert.ok(gates.some(gate=>gate.kind==='receipt'),'no gate is backed by a receipt');
 for(const gate of gates.filter(entry=>entry.kind==='decision'))assertDecision(gate,gate.id);
 // A gate that was not converted keeps its typed status, which is never a pass.
 for(const gate of gates.filter(entry=>entry.kind===undefined))assert.equal(gate.status,'incomplete',gate.id);
 // The seams gate names a test file for each claim that only a test can hold.
 const seams=gates.find(gate=>gate.id==='authored-typescript-seams');
 assert.equal(seams.kind,'ci');
 for(const file of ['layout','wire','rules-form','mcp-apps-mime','mcp-apps-dispatch'])assert.ok(seams.enforced_by.evidence.some(entry=>entry.startsWith(`ui/tests/unit/${file}.test.`)),`authored-typescript-seams does not name ${file}`);
 assert.match(await read('ui/tests/unit/layout.test.tsx'),/refuses a chart specification that compile rejects/);
 assert.match(await read('ui/tests/unit/mcp-apps-dispatch.test.tsx'),/describe\('MCP App wire boundary'/);
 assert.match(await read('ui/package.json'),/"typecheck": "tsc -b && tsc -p tsconfig\.tests\.json --noEmit"/,'the typecheck step no longer covers ui/tests');
});
test('the Docling gate carries the one failure the owner accepted, and the gates that carry the cures exist',async()=>{
 const gates=(await record()).current.gates;
 const docling=gates.phase_0.find(gate=>gate.id==='docling-library-qualification');
 // The owner's decision of 2026-10-06, word for word; nobody but the owner changes or adds to it.
 assert.deepEqual(docling.accepted_failures,[{
  criterion:'corpus/redp5110_sampled.pdf/content_across_font_runs',
  decision:'Docling stays the converter. Where a phrase changes font the library ends a text cell and joins cells with a space, so "(WRKFCNUSG)" is extracted as "( WRKFCNUSG )". Accepted for now and reported upstream; no newer version, option or pdfium changes it.',
  decided_on:'2026-10-06',
  decided_by:'owner',
  tracked_by:'converter-font-run-spacing',
 }]);
 for(const gate of gates.phase_0.filter(entry=>entry.id!==docling.id))assert.ok(!Object.hasOwn(gate,'accepted_failures'),`${gate.id} accepts a failure`);
 const pinned=JSON.parse(await read('qualification/docling/criteria.json')).required;
 assert.ok(pinned.includes(docling.accepted_failures[0].criterion),'the accepted criterion is not one the harness pins');
 // The cures and the limitation are construction work, each with an owner lane.
 const construction=Object.fromEntries(gates.construction.map(gate=>[gate.id,gate]));
 for(const gate of gates.construction)assert.deepEqual(Object.keys(gate),['id','status','owner','receipt','meaning'],gate.id);
 assert.deepEqual([construction['ingest-locates-unlocated-items'].owner,construction['ingest-locates-unlocated-items'].status],['ingest','blocked_on_lanes']);
 assert.deepEqual([construction['ingest-flags-undecodable-text'].owner,construction['ingest-flags-undecodable-text'].status],['ingest','blocked_on_lanes']);
 assert.deepEqual([construction['converter-font-run-spacing'].owner,construction['converter-font-run-spacing'].status],['integration-owner','blocked_upstream']);
 assert.match(construction['ingest-locates-unlocated-items'].meaning,/qualification\/docling\/src\/locate\.rs.*never given a guessed box/);
 assert.match(construction['ingest-flags-undecodable-text'].meaning,/qualification\/docling\/src\/glyphs\.rs.*never indexed as words.*reported as partly extracted/);
 assert.match(construction['converter-font-run-spacing'].meaning,/the acceptance entry is removed$/);
 // The status words of the two hand-typed groups are a closed list; none of them is a pass.
 const words=group=>[...new Set(gates[group].map(gate=>gate.status))].sort();
 assert.deepEqual(words('construction'),['blocked_on_lanes','blocked_upstream']);
 assert.deepEqual(gates.construction.filter(gate=>gate.status==='blocked_upstream').map(gate=>gate.id),['converter-font-run-spacing'],'blocked_upstream is for a limitation only an upstream release cures');
 assert.deepEqual(words('acceptance'),['blocked_on_product','not_run']);
 // The ingest lane is told: its gate table lists both gates, and its rules name the harness functions as the reference.
 const lane=await read('crates/ingest/AGENTS.md');
 for(const id of ['ingest-locates-unlocated-items','ingest-flags-undecodable-text'])assert.ok(lane.includes(`| \`${id}\` | \`${construction[id].receipt}\` |`),`crates/ingest/AGENTS.md does not list ${id} with its command`);
 assert.match(lane,/`locate::locate_items`/);
 assert.match(lane,/`glyphs::undecoded_glyphs`/);
 for(const name of ['locate_items','undecoded_glyphs','placeholder_glyph_tokens'])assert.match(await read(`qualification/docling/src/${name==='locate_items'?'locate':'glyphs'}.rs`),new RegExp(`pub\\(crate\\) fn ${name}\\(`),`${name} is not a function of the harness`);
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
test('no gate text, and nothing else outside the archive, carries a commit hash or a CI run number; history lives in git',async()=>{
 const {historical_archive_record:_archive,...live}=await record();
 assert.deepEqual(Object.keys(live.current.gates),['phase_0','construction','acceptance']);
 // The gates left unconverted keep their old text until they gain a kind.
 live.current.gates.phase_0=live.current.gates.phase_0.filter(gate=>!Object.hasOwn(unconverted,gate.id));
 // Seven or more hex digits in a row: a short or full commit, or a run number.
 const hashLike=/\b[0-9a-f]{7,40}\b/i;
 const strings=value=>typeof value==='string'?[value]:value!==null&&typeof value==='object'?Object.values(value).flatMap(strings):[];
 const texts=strings(live);
 assert.ok(texts.length>100,'the scan read almost nothing');
 for(const text of texts){
  assert.doesNotMatch(text,hashLike,text.slice(0,80));
  assert.doesNotMatch(text,/actions\/runs\/|\bCI \d+/,text.slice(0,80));
 }
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
test('README, AGENTS and the close-out plan state that receipt-backed statuses are written by record.mjs, never typed',async()=>{
 for(const file of ['README.md','AGENTS.md']){
  const text=await read(file);
  assert.ok(text.includes('`bun qualification/record.mjs` writes'),`${file} does not say what writes the derived values`);
  assert.ok(text.includes('`bun scripts/dev.mjs check-receipts` verifies them'),`${file} does not say what verifies them`);
  assert.match(text,/carr(?:y|ies) no status/,`${file} does not say that a CI-enforced gate carries no status`);
  assert.match(text,/Nobody types a result into `verification\.json`/,file);
 }
 const readme=await read('README.md');
 assert.match(readme,/`bun scripts\/dev\.mjs clean-checkout` [^.]*; it is a local tool, not a gate/,'README must document clean-checkout as a tool');
 const plan=await read('docs/plans/stage-1a/90-orchestrator-close.md');
 assert.match(plan,/The orchestrator types no status in `verification\.json`/);
 assert.doesNotMatch(plan,/Decide terminal states|gets the state decided in O\.4/,'the plan still has the orchestrator typing gate states');
});
test('help names every task the entrypoint accepts, and just mirrors the gates',async()=>{
 const entry=await read('scripts/dev.mjs');
 const cases=[...entry.matchAll(/^\s+case '([a-z-]+)':/gm)].map(match=>match[1]).filter(name=>name!=='help');
 const help=/case 'help': process\.stdout\.write\('Tasks: ([^\\]+)\\n'\)/.exec(entry)?.[1].split(' ')??[];
 assert.deepEqual([...help].sort(),[...cases].sort());
 const just=await read('justfile');
 for(const recipe of ['lane','premerge','clean-checkout','lanes-reset','check-receipts'])assert.match(just,new RegExp(`^${recipe}\\b`,'m'),`justfile has no ${recipe} recipe`);
});
