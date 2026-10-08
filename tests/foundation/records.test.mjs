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
import { lanes } from '../../scripts/lib/lanes.mjs';
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
/** Where the findings that reopened Phase 0 are readable in full: `git show` of this, key `reopened.findings`. The one commit the live record names. */
const findingsAt='6ca72dd:verification.json';
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
/**
 * Whether this checkout has the history a receipt is held to. A source copy without `.git` (the
 * build container) and a shallow clone (the source-tooling job) do not: there the commit a
 * receipt cites cannot be compared with HEAD.
 */
async function hasHistory(){
 if(!existsSync(join(root,'.git')))return false;
 const shallow=await run('git',['rev-parse','--is-shallow-repository'],{cwd:root,capture:true,allowFailure:true});
 return shallow.code===0&&shallow.stdout.trim()==='false';
}
test('verification.json types what its receipts derive, in the form record.mjs rewrites',async()=>{
 const typed=await record();
 const typedStatus=gate=>typed.current.gates.phase_0.find(entry=>entry.id===gate.id).status;
 assert.equal(await read('verification.json'),`${JSON.stringify(typed,null,2)}\n`,'verification.json is not in two-space JSON form, so record.mjs could not rewrite only the derived values');
 if(await hasHistory()){
  // The comparison check-receipts makes: a hand edit of a status or of phase_0_qualified fails here too.
  const derived=await derivedRecord(root);
  assert.equal(derived.history,true);
  assert.deepEqual(derived.failures,[]);
  assert.deepEqual(derived.mismatches,[]);
  assert.equal(typed.phase_0_qualified,derived.phase_0_qualified);
  for(const gate of derived.gates)assert.equal(typedStatus(gate),gate.status,gate.id);
  return;
 }
 // Without history a receipt cannot be held to the commit it cites, so staleness is not judged and
 // what is derived is an upper bound: the record may say incomplete where a receipt went stale,
 // and never more than the receipts' content supports. check-receipts, in the job that has the
 // history, makes the exact comparison.
 const bound=await derivedRecord(root,undefined,{history:false});
 assert.equal(bound.history,false);
 assert.deepEqual(bound.failures,[]);
 for(const gate of bound.gates)assert.ok([gate.status,'incomplete'].includes(typedStatus(gate)),`${gate.id}: verification.json types ${typedStatus(gate)}, more than its receipt supports (${gate.status})`);
 assert.ok(typed.phase_0_qualified===false||bound.phase_0_qualified===true,'verification.json types phase_0_qualified true, which the receipts do not support');
});
/** The Phase 0 gates, in file order, each with its kind. Removing a gate or changing its kind changes what qualifies Phase 0, so it is a change to this list. */
const phase0Gates=[
 ['authored-typescript-seams','ci'],
 ['deterministic-foundation','ci'],
 ['docling-library-qualification','receipt'],
 ['converter-docling-pdf-font-run-patch','decision'],
 ['mcp-apps-protocol-qualification','receipt'],
 ['json-render-catalog-round-trip','ci'],
 ['hey-api-generated-runtime-probe','decision'],
 ['clean-checkout-rerun','ci'],
 ['github-actions-ci-receipt','ci'],
];
test('the Phase 0 gates are exactly these, each of this kind, and the two library qualifications are the receipt gates',async()=>{
 const gates=(await record()).current.gates.phase_0;
 assert.deepEqual(gates.map(gate=>[gate.id,gate.kind]),phase0Gates,'a Phase 0 gate was added, removed or given another kind: phase_0_qualified is derived from the receipt gates, so this list changes with it');
 // What derivedRecord reads is the same two gates, each with its harness.
 const derived=await derivedRecord(root,undefined,{history:await hasHistory()});
 assert.deepEqual(derived.gates.map(gate=>[gate.id,gate.harness]),[['docling-library-qualification','docling'],['mcp-apps-protocol-qualification','mcp-apps']]);
 assert.deepEqual(derived.unconverted,[]);
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
 // Why Phase 0 was reopened is a decision, not a status. The twelve findings behind it are
 // history: the entry names the commit at which this file still listed them in full.
 assert.deepEqual(Object.keys(typed.reopened),['decision','decided_on','decided_by','findings_at']);
 assertDecision(typed.reopened,'reopened');
 assert.equal(typed.reopened.findings_at,findingsAt);
 if(await hasHistory()){
  const [commit,path]=findingsAt.split(':');
  const shown=await run('git',['show',`${commit}:${path}`],{cwd:root,capture:true,allowFailure:true});
  assert.equal(shown.code,0,`${findingsAt} cannot be read from this repository's history`);
  const then=JSON.parse(shown.stdout).reopened;
  assert.equal(then.findings.length,12,'the commit the entry names does not hold the twelve findings');
  for(const finding of then.findings)assert.ok(typeof finding==='string'&&finding.length>40,'a finding there is not a sentence');
 }
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
 // The round-trip gate says the six components are rendered with data, and its test does that.
 const roundTrip=gates.find(gate=>gate.id==='json-render-catalog-round-trip');
 assert.match(roundTrip.covers,/renders all six catalog components twice: with nothing resolved, .* and with the committed source, dataset and binding fixtures, /);
 const roundTripTest=await read('ui/tests/unit/view-document-roundtrip.test.tsx');
 assert.ok(roundTrip.enforced_by.evidence.includes('ui/tests/unit/view-document-roundtrip.test.tsx'));
 assert.match(roundTripTest,/^ {2}it\('renders each of the six catalog components with the data it needs', async \(\) => \{$/m);
 for(const drawn of ["only('section.view-stack')","only('.view-columns', stack)","only('article.source-excerpt', columns)","only(':scope > table', columns)",'svg g[class~="role-mark"] > *',"only(':scope > ul', stack)"])assert.ok(roundTripTest.includes(drawn),`the round-trip test no longer looks for ${drawn}`);
 // The seams gate names a test file for each claim that only a test can hold.
 const seams=gates.find(gate=>gate.id==='authored-typescript-seams');
 assert.equal(seams.kind,'ci');
 for(const file of ['layout','wire','rules-form','mcp-apps-mime','mcp-apps-dispatch'])assert.ok(seams.enforced_by.evidence.some(entry=>entry.startsWith(`ui/tests/unit/${file}.test.`)),`authored-typescript-seams does not name ${file}`);
 assert.match(await read('ui/tests/unit/layout.test.tsx'),/refuses a chart specification that compile rejects/);
 assert.match(await read('ui/tests/unit/mcp-apps-dispatch.test.tsx'),/describe\('MCP App wire boundary'/);
 assert.match(await read('ui/package.json'),/"typecheck": "tsc -b && tsc -p tsconfig\.tests\.json --noEmit"/,'the typecheck step no longer covers ui/tests');
 // The record says how ui/tests is type-checked as it is: by the script, not by a project reference.
 const policy=(await record()).current.typescript_policy.authored;
 assert.ok(policy.endsWith('ui/tests is type-checked by ui/tsconfig.tests.json, which ui/tsconfig.json does not reference: the `typecheck` script of ui/package.json runs it after the build mode check (`tsc -b && tsc -p tsconfig.tests.json --noEmit`).'),'typescript_policy.authored does not say how ui/tests is type-checked');
 const references=JSON.parse(await read('ui/tsconfig.json')).references.map(reference=>reference.path);
 assert.deepEqual(references,['./tsconfig.generated.json','./tsconfig.tooling.json'],'ui/tsconfig.json references changed: the sentence in typescript_policy.authored must follow');
});
test('the Docling gate accepts no failure, its PDF crate is the fork the owner decided on, and the gates that carry the cures exist',async()=>{
 const gates=(await record()).current.gates;
 const docling=gates.phase_0.find(gate=>gate.id==='docling-library-qualification');
 // The font-run failure the owner accepted on 2026-10-06 is cured by the patched docling-pdf, so no criterion is accepted as failing.
 for(const gate of gates.phase_0)assert.ok(!Object.hasOwn(gate,'accepted_failures'),`${gate.id} accepts a failure`);
 assert.match(docling.covers,/for the Docling crate Cargo\.lock pins \(docling-pdf, docling-core and docling-onnx from the fork of converter-docling-pdf-font-run-patch\), called directly/);
 // The owner's decision of 2026-10-06, word for word.
 const patch=gates.phase_0.find(gate=>gate.id==='converter-docling-pdf-font-run-patch');
 assert.deepEqual([patch.kind,patch.decision,patch.decided_on,patch.decided_by],['decision','docling-pdf is built from a two-commit fork of docling.rs 2.3.0 so that a phrase that changes font is extracted as one run, as docling-parse 7.21 and later do, and a blank cell is never taken for overpaint on the text-layer path.','2026-10-08','owner']);
 // What it covers: the fork and the commit Cargo.toml patches in (named there, not here: no gate text carries a commit hash), the upstream base, the rule, upstream's change, nothing reported, and when it goes.
 const rev=/^docling-pdf = \{ git = "https:\/\/github\.com\/Heyoub\/docling\.rs", rev = "([0-9a-f]{40})" \}$/m.exec(await read('Cargo.toml'))?.[1];
 assert.ok(rev,'Cargo.toml does not take docling-pdf from the fork by rev');
 for(const part of ['the fork https://github.com/Heyoub/docling.rs','the commit the [patch.crates-io] section of Cargo.toml names by rev','two commits on the upstream docling.rs commit that crates.io publishes as 2.3.0','drop_overpainted_cells','dp_lines.rs, applicable','#351 (7.21.0)','docling.rs has not ported it','Nothing was reported upstream','when a docling.rs release carries both changes'])assert.ok(patch.covers.includes(part),`converter-docling-pdf-font-run-patch does not say ${part}`);
 // The tracker of the removed acceptance is gone, and nothing names it.
 assert.doesNotMatch(await read('verification.json'),/converter-font-run-spacing/);
 // The cures are construction work, each with an owner lane.
 const construction=Object.fromEntries(gates.construction.map(gate=>[gate.id,gate]));
 for(const gate of gates.construction)assert.deepEqual(Object.keys(gate),['id','status','owner','receipt','meaning'],gate.id);
 assert.deepEqual([construction['ingest-locates-unlocated-items'].owner,construction['ingest-locates-unlocated-items'].status],['ingest','blocked_on_lanes']);
 assert.deepEqual([construction['ingest-flags-undecodable-text'].owner,construction['ingest-flags-undecodable-text'].status],['ingest','blocked_on_lanes']);
 assert.match(construction['ingest-locates-unlocated-items'].meaning,/qualification\/docling\/src\/locate\.rs.*never given a guessed box/);
 assert.match(construction['ingest-flags-undecodable-text'].meaning,/qualification\/docling\/src\/glyphs\.rs.*never indexed as words.*reported as partly extracted/);
 // What the two rules cannot do is said where the lane reads it: in its gates and in its own rules.
 const detectorLimits="The detector has two known limits: it flags real text of the placeholder's shape that stands after a space (`/B747`, `/v100`, `/tmp123`), and it misses a placeholder glued to a preceding character (`x/g12`). A signal from the library that a glyph had no Unicode is preferred to this detector as soon as the library gives one";
 assert.ok(construction['ingest-flags-undecodable-text'].meaning.endsWith(detectorLimits),'ingest-flags-undecodable-text does not state the detector\'s two known limits');
 assert.match(construction['ingest-locates-unlocated-items'].meaning,/The rule does not guarantee that the box it finds is the item's own: that file states the two known wrong placements \(LOCATE_LIMITS\).*without applying a threshold\. Whether the product bounds that distance is the owner's decision, open until this gate is built$/);
 assert.match(docling.covers,/It does not prove that a box found through the text layer is the item's own \(the receipt states the rule's two known wrong placements, and records each such item's distance to the item whose page was searched as a measurement with no threshold\)/);
 const ingestRules=await read('crates/ingest/AGENTS.md');
 assert.ok(ingestRules.includes(`${detectorLimits}.`),'crates/ingest/AGENTS.md does not state the detector\'s two known limits');
 assert.match(ingestRules,/`LOCATE_LIMITS` in that file states the two known wrong placements/);
 const detector=await read('qualification/docling/src/glyphs.rs');
 assert.match(detector,/fn the_two_known_limits_of_the_detector_are_as_stated\(\)/,'the limits are stated without a test that holds them');
 for(const example of ['`/B747`','`/v100`','`/tmp123`','`x/g12`'])assert.ok(detector.includes(example),`src/glyphs.rs does not name ${example}`);
 assert.match(await read('qualification/docling/src/locate.rs'),/^pub\(crate\) const LOCATE_LIMITS: &str = "/m);
 // The status words of the two hand-typed groups are a closed list; none of them is a pass.
 const words=group=>[...new Set(gates[group].map(gate=>gate.status))].sort();
 assert.deepEqual(words('construction'),['blocked_on_lanes']);
 assert.deepEqual(words('acceptance'),['blocked_on_product','not_run']);
 // Every gate that names an owner names one that exists: a lane of scripts/lib/lanes.mjs, or the integration owner.
 const owners=new Set(['integration-owner',...lanes.map(entry=>entry.name)]);
 for(const gate of gates.construction)assert.ok(owners.has(gate.owner),`${gate.id}: owner ${gate.owner} is no lane of scripts/lib/lanes.mjs and not integration-owner`);
 for(const gate of gates.acceptance)assert.ok(gate.owner===undefined||owners.has(gate.owner),`${gate.id}: owner ${gate.owner} is no lane of scripts/lib/lanes.mjs and not integration-owner`);
 // The ingest lane is told: its instructions send it to the gates it owns (the two above among them, each with
 // its command) instead of copying them, and its rules name the harness functions as the reference.
 const lane=await read('crates/ingest/AGENTS.md');
 assert.ok(lane.includes("This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `ingest` (each names its command)."),'crates/ingest/AGENTS.md does not point to its gates');
 for(const id of ['ingest-locates-unlocated-items','ingest-flags-undecodable-text'])assert.match(construction[id].receipt,/^cargo test -p okf-jawn-ingest --features runtime -- [a-z_]+$/,`${id} does not name its command`);
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
 // What clean-checkout-rerun says of the caches is what the workflow configures: the cargo cache
 // falls back to any cargo cache of the runner OS, the Bun cache has no fallback.
 const cleanCheckout=gates.find(gate=>gate.id==='clean-checkout-rerun');
 const caches=Bun.YAML.parse(await read('.github/workflows/ci.yml')).jobs.foundation.steps.filter(step=>step.uses?.startsWith('actions/cache@')).map(step=>step.with);
 const cargoCache=caches.find(cache=>/^target$/m.test(cache.path)),bunCache=caches.find(cache=>cache.path.includes('~/.bun/install/cache'));
 assert.equal(cargoCache.key,"cargo-${{ runner.os }}-${{ hashFiles('rust-toolchain.toml', 'Cargo.lock') }}");
 assert.equal(cargoCache['restore-keys'].trim(),'cargo-${{ runner.os }}-');
 assert.equal(bunCache.key,"bun-${{ runner.os }}-${{ hashFiles('.bun-version', 'bun.lock') }}");
 assert.equal(bunCache['restore-keys'],undefined);
 assert.match(cleanCheckout.covers,/restored from a cache keyed on rust-toolchain\.toml and Cargo\.lock, with a fallback to the newest cargo cache of the same runner OS when none has that key, so `target` can come from another lockfile state/);
 assert.match(cleanCheckout.covers,/Bun's install cache is keyed on \.bun-version and bun\.lock and has no fallback\./);
 assert.doesNotMatch(cleanCheckout.covers,/caches keyed on the lockfiles/);
 // "and the checkout" in deterministic-foundation is held by a workflow step that is no dev.mjs
 // task (bootstrap regenerates before gen-check runs), so the test that pins that step is its evidence.
 const foundation=gates.find(gate=>gate.id==='deterministic-foundation');
 assert.match(foundation.covers,/requires both passes and the checkout to agree byte for byte/);
 assert.ok(foundation.enforced_by.evidence.includes('tests/foundation/ci.test.mjs'),'deterministic-foundation does not name the test that pins the clean-tree step');
 assert.match(await read('tests/foundation/ci.test.mjs'),/^test\('the job that runs the real generators fails when generation changed the checkout, before any gated step'/m);
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
 // One pointer into history is kept on purpose and is exactly this one: where the reopening findings can be read.
 assert.equal(live.reopened.findings_at,findingsAt);
 live.reopened={...live.reopened,findings_at:''};
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
test('README and AGENTS state that receipt-backed statuses are written by record.mjs, never typed',async()=>{
 for(const file of ['README.md','AGENTS.md']){
  const text=await read(file);
  assert.ok(text.includes('`bun qualification/record.mjs` writes'),`${file} does not say what writes the derived values`);
  assert.ok(text.includes('`bun scripts/dev.mjs check-receipts` verifies them'),`${file} does not say what verifies them`);
  assert.match(text,/carr(?:y|ies) no status/,`${file} does not say that a CI-enforced gate carries no status`);
  assert.match(text,/Nobody types a result into `verification\.json`/,file);
 }
 const readme=await read('README.md');
 assert.match(readme,/`bun scripts\/dev\.mjs clean-checkout` [^.]*; it is a local tool, not a gate/,'README must document clean-checkout as a tool');
 // What a changed input costs, and both ways out of it.
 assert.ok(readme.includes('when an input of a recorded receipt changes, `check-receipts` fails until the harness is re-run and recorded, or `bun qualification/record.mjs` is run to write the gate back to `incomplete`'),'README does not say what happens when an input of a recorded receipt changes');
 assert.match(readme,/is trusted for nothing, whatever result it types: its gate derives `incomplete`/);
 // The limit is said plainly where the records are described.
 assert.ok(readme.includes('receipts are produced on a developer machine and are not signed, so a consistent hand edit of a receipt is not detected by `check-receipts`'),'README does not state that receipts are unsigned');
 assert.ok(readme.includes('Producing the receipts in CI, where a contributor cannot edit them, is the planned cure.'),'README does not name the planned cure');
});
test('help names every task the entrypoint accepts, and just mirrors the gates',async()=>{
 const entry=await read('scripts/dev.mjs');
 const cases=[...entry.matchAll(/^\s+case '([a-z-]+)':/gm)].map(match=>match[1]).filter(name=>name!=='help');
 const help=/case 'help': process\.stdout\.write\('Tasks: ([^\\]+)\\n'\)/.exec(entry)?.[1].split(' ')??[];
 assert.deepEqual([...help].sort(),[...cases].sort());
 const just=await read('justfile');
 for(const recipe of ['lane','premerge','clean-checkout','lanes-reset','check-receipts'])assert.match(just,new RegExp(`^${recipe}\\b`,'m'),`justfile has no ${recipe} recipe`);
});
