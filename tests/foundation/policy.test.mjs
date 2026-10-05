/** Inspect authored source policy; Rust AST enforcement runs separately through xtask. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { files } from '../../scripts/lib/files.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
// Both files are generator output of crates/contract/src/operations.rs; gen-check proves they match it.
const operations=JSON.parse(await readFile(join(root,'api/operations.json'),'utf8'));
const tools=JSON.parse(await readFile(join(root,'api/mcp-tools.json'),'utf8')).tools;
const byId=new Map(operations.map(o=>[o.id,o]));
const exposed=operations.filter(o=>o.alias);
// Owner decision: a model tool whose name is not in this list needs a new decision, not a new row.
const approvedModelTools=['ls','grep','show','log','diff','blame','sources','links','propose','catalog','present','workspaces'];
const humanOnly=['approve','review','admin'];

test('complete operation surface has unique canonical identifiers and paths',()=>{
 assert.ok(operations.length>0,'api/operations.json is empty');
 assert.equal(new Set(operations.map(o=>o.id)).size,operations.length);
 assert.equal(new Set(operations.map(o=>o.path)).size,operations.length);
 for(const o of operations){
  assert.match(o.id,/^[a-z]+(?:_[a-z]+)*$/);
  assert.match(o.path,/^\/api\/[a-z]+\/[a-z]+(?:-[a-z]+)*$/,o.id);
  assert.ok(o.path.endsWith(`/${o.id.replaceAll('_','-')}`),`${o.id} is served at ${o.path}`);
  assert.ok(o.description.length>0,o.id);
 }
});
test('every operation has generated input and output schemas and no schema is orphaned',async()=>{
 const expected=operations.flatMap(o=>[`${o.id}.input.json`,`${o.id}.output.json`]).sort();
 assert.deepEqual((await readdir(join(root,'api/schemas'))).sort(),expected);
});
test('agent exposure cannot include human approval or verification',()=>{
 assert.equal(new Set(exposed.map(o=>o.alias)).size,exposed.length);
 for(const o of operations){
  if(o.alias)assert.ok(['model','app'].includes(o.visibility),`${o.id} has a tool alias but visibility "${o.visibility}"`);
  else assert.equal(o.visibility,'',`${o.id} has a visibility but no tool alias`);
 }
 for(const o of exposed)assert.ok(['read','propose'].includes(o.permission),`${o.id} is a tool but requires ${o.permission}`);
 for(const o of operations.filter(o=>humanOnly.includes(o.permission)))assert.equal(o.alias,'',`${o.id} requires ${o.permission} and must never be a tool`);
 for(const id of ['create_review','create_confirmation','accept_proposal','decline_proposal'])assert.equal(byId.get(id)?.alias,'',id);
 const model=exposed.filter(o=>o.visibility==='model').map(o=>o.alias);
 assert.ok(model.length>0,'no model tool is declared');
 for(const alias of model)assert.ok(approvedModelTools.includes(alias),`model tool ${alias} is not an approved name`);
 assert.deepEqual(exposed.filter(o=>o.visibility==='app').map(o=>o.id),['get_object']);
});
test('the generated MCP tool list is exactly the exposed operations',()=>{
 assert.deepEqual(tools.map(t=>t.name).sort(),exposed.map(o=>o.alias).sort());
 for(const tool of tools){
  const operation=exposed.find(o=>o.alias===tool.name);
  assert.deepEqual(tool._meta.ui.visibility,operation.visibility==='model'?['model','app']:['app'],tool.name);
  assert.equal(tool.annotations.readOnlyHint,operation.permission==='read',tool.name);
  assert.equal(tool.annotations.destructiveHint,false,tool.name);
 }
});
test('saved views, human naming UX, and full reading surfaces remain declared',()=>{
 for(const id of ['read_item','preview_names','apply_names','get_graph','get_view','present_view','resolve_view','export_view','create_review','accept_proposal','backup_workspace','restore_workspace','get_object'])assert.ok(byId.has(id),id);
});
test('connector credentials are owner-administered and never agent tools',()=>{
 for(const id of ['create_connector','list_connectors','revoke_connector']){
  const o=byId.get(id);assert.ok(o,id);assert.equal(o.permission,'admin',id);assert.equal(o.alias,'',id);assert.equal(o.visibility,'',id);
 }
});
test('core composes ports only and never depends on concrete adapter crates',async()=>{
 const manifest=await readFile(join(root,'crates/core/Cargo.toml'),'utf8');
 for(const crate of ['okf-jawn-storage','okf-jawn-ingest','okf-jawn-server','okf-jawn-mcp','okf-jawn-cli'])assert.doesNotMatch(manifest,new RegExp(crate),crate);
});
test('test-support applications are included only by test targets',async()=>{
 for(const file of (await files(root)).filter(f=>f.endsWith('.rs'))){
  const normalized=file.replaceAll('\\','/');
  if(normalized.startsWith('tests/')||/^crates\/[^/]+\/tests\//.test(normalized)||normalized.startsWith('xtask/tests/'))continue;
  assert.doesNotMatch(await readFile(join(root,file),'utf8'),/tests\/support|FixtureApplication/,file);
 }
});
test('authored Rust has purpose headers and no direct lint suppression attributes',async()=>{
 for(const file of (await files(root)).filter(f=>f.endsWith('.rs'))){
  const source=await readFile(join(root,file),'utf8');assert.match(source,/^\s*\/\/!/,file);
  assert.doesNotMatch(source,/#\s*!?\[\s*(?:allow|expect)\s*\(/,file);
 }
});
test('canceled product runtimes are not imported as application dependencies',async()=>{
 const manifest=JSON.parse(await readFile(join(root,'ui/package.json'),'utf8'));
 const keys=Object.keys({...manifest.dependencies,...manifest.devDependencies});
 for(const key of keys)assert.doesNotMatch(key,/^(?:@copilotkit\/|@ag-ui\/|@assistant-ui\/|@json-render\/mcp$|openai$|@anthropic-ai\/sdk$)/);
});
test('CLI, UI, browser tests and examples agree on the local service endpoint',async()=>{
 for(const file of ['crates/cli/src/lib.rs','ui/vite.config.ts','ui/playwright.config.ts','deploy/.env.example']){
  const source=await readFile(join(root,file),'utf8');
  assert.match(source,/127\.0\.0\.1:7711/,file);
  assert.doesNotMatch(source,/127\.0\.0\.1:(?!7711\b)\d+/,file);
 }
});
