/** Records, prose and scripts agree with the generated surface and with what was actually run. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
import { libraryGates } from '../../scripts/lib/receipts.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

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
