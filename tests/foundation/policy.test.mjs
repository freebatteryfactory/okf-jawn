/** Inspect authored source policy; Rust AST enforcement runs separately through xtask. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { files } from '../../scripts/lib/files.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const declarations=await readFile(join(root,'crates/contract/src/operations.rs'),'utf8');
const pattern=/\((\w+),\s*\$crate::([\w:]+),\s*\$crate::([\w:]+),\s*"([^"]+)",\s*"([^"]*)",\s*"([^"]*)",\s*"([^"]*)",\s*(\w+),\s*"([^"]*)",\s*(\d+),\s*"([^"]*)"\)/g;
const operations=[...declarations.matchAll(pattern)].map(m=>({id:m[1],request:m[2],response:m[3],path:m[4],label:m[5],alias:m[6],visibility:m[7],permission:m[8],ui:m[9]}));

test('complete operation surface has unique canonical identifiers and paths',()=>{
 assert.equal(operations.length,66);assert.equal(new Set(operations.map(o=>o.id)).size,operations.length);
 assert.equal(new Set(operations.map(o=>o.path)).size,operations.length);
 for(const o of operations)assert.match(o.id,/^[a-z]+(?:_[a-z]+)*$/);
});
test('every operation references declared real types',async()=>{
 for(const o of operations)for(const symbol of [o.request,o.response]){
  const parts=symbol.split('::');const module=parts[0],name=parts.at(-1);
  const source=await readFile(join(root,'crates/contract/src',`${module}.rs`),'utf8');
  assert.match(source,new RegExp(String.raw`pub (?:struct|enum|type) ${name}\b`),symbol);
 }
});
test('agent exposure cannot include human approval or verification',()=>{
 const tools=operations.filter(o=>o.alias);assert.equal(tools.filter(o=>o.visibility==='model').length,11);
 assert.equal(tools.filter(o=>o.visibility==='app').length,1);
 assert.equal(new Set(tools.map(o=>o.alias)).size,tools.length);
 for(const o of tools)assert.ok(['Read','Propose'].includes(o.permission),o.id);
 assert.equal(operations.find(o=>o.id==='create_review').alias,'');
 assert.equal(operations.find(o=>o.id==='accept_proposal').alias,'');
});
test('saved views, human naming UX, and full reading surfaces remain declared',()=>{
 for(const id of ['read_item','preview_names','apply_names','get_graph','get_view','present_view','resolve_view','export_view','create_review','accept_proposal','backup_workspace','restore_workspace','get_object'])assert.ok(operations.some(o=>o.id===id),id);
});
test('connector credentials are owner-administered and never agent tools',()=>{
 for(const id of ['create_connector','list_connectors','revoke_connector']){
  const o=operations.find(op=>op.id===id);assert.ok(o,id);assert.equal(o.permission,'Admin',id);assert.equal(o.alias,'',id);assert.equal(o.visibility,'',id);
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
