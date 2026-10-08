/** Vendor lookup remains useful without a connector, API key, or network. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { run } from '../../scripts/lib/process.mjs';
const root=new URL('../../',import.meta.url);
const generatedRoots=['api/','generated/cli/','generated/converter/','ui/src/api/generated'];

async function containsFile(path){
 const info=await stat(new URL(path,root)).catch(()=>null);
 if(!info)return false;
 if(info.isFile())return true;
 for(const entry of await readdir(new URL(path,root)))if(await containsFile(`${path.replace(/\/$/,'')}/${entry}`))return true;
 return false;
}

test('recorded Context7 lookups are counted from the entries, never from memory',async()=>{
 const data=JSON.parse(await readFile(new URL('vendors.json',root),'utf8'));
 const recorded=data.vendors.filter(v=>v.context7_library_id!==null);
 assert.equal(data.context7_queries_executed,recorded.length,'context7_queries_executed must equal the entries that record a library id and its query');
 assert.equal(new Set(data.vendors.map(v=>v.name)).size,data.vendors.length,'vendor names must be unique');
 for(const v of data.vendors){
  if(v.context7_library_id===null)assert.equal(v.context7_query,null,`${v.name} records a query without a library id`);
  else{
   assert.match(v.context7_library_id,/^\/[^/\s]+\/\S+$/,`${v.name} library id`);
   assert.ok(typeof v.context7_query==='string'&&v.context7_query.length>0,`${v.name} records a library id without its query`);
   assert.match(v.knowledge_source,/Context7/,`${v.name} knowledge_source must name the lookup`);
  }
  assert.ok(v.official_sources.length);assert.ok(v.offline_notes.length);
  const planned=v.planned_use_sites??[],generated=v.generated_sites??[];
  assert.ok(v.use_sites.length+planned.length+generated.length,`${v.name} has no site`);
  for(const site of v.use_sites)assert.ok(await containsFile(site),`${v.name} existing use site ${site} has no file; list it as planned`);
  for(const site of [...planned,...generated])assert.ok(!v.use_sites.includes(site),`${v.name} lists ${site} as both existing and not`);
  for(const site of generated)assert.ok(generatedRoots.some(g=>site.startsWith(g)),`${v.name} generated site ${site} is not a generated directory`);
 }
});
test('a vendor entry types no qualification status: it names the verification.json gates that record one',async()=>{
 const data=JSON.parse(await readFile(new URL('vendors.json',root),'utf8'));
 const gates=JSON.parse(await readFile(new URL('verification.json',root),'utf8')).current.gates;
 const ids=new Set(Object.values(gates).flat().map(gate=>gate.id));
 for(const v of data.vendors){
  assert.ok(Array.isArray(v.qualification),`${v.name}.qualification must be a list of verification.json gate ids, not a typed status`);
  assert.equal(new Set(v.qualification).size,v.qualification.length,`${v.name} names a gate twice`);
  for(const id of v.qualification)assert.ok(ids.has(id),`${v.name} names ${id}, which is no gate of verification.json`);
 }
 // A result is typed in neither field: verification.json derives or states it.
 for(const v of data.vendors)for(const note of v.offline_notes)assert.doesNotMatch(note,/^(?:PASS|FAIL|incomplete)\b|still need an executed qualification/i,`${v.name}: ${note.slice(0,60)}`);
});
test('vendor task returns documented lookup without dependencies installed',async()=>{
 const result=await run(process.execPath,['scripts/dev.mjs','vendor','schemars'],{cwd:root,capture:true});
 assert.ok(JSON.parse(result.stdout).some(v=>v.context7_library_id==='/gresau/schemars'));
});
test('unknown task fails instead of printing help as success',async()=>{
 await assert.rejects(run(process.execPath,['scripts/dev.mjs','unknown-operation'],{cwd:root,capture:true}),/Unknown task/);
});
