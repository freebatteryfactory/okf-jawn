/** Vendor lookup remains useful without a connector, API key, or network. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { run } from '../../scripts/lib/process.mjs';
const root=new URL('../../',import.meta.url);
const generatedRoots=['api/','generated/cli/','ui/src/api/generated'];

async function containsFile(path){
 const info=await stat(new URL(path,root)).catch(()=>null);
 if(!info)return false;
 if(info.isFile())return true;
 for(const entry of await readdir(new URL(path,root)))if(await containsFile(`${path.replace(/\/$/,'')}/${entry}`))return true;
 return false;
}

test('offline index records only the three executed Context7 queries as such',async()=>{
 const data=JSON.parse(await readFile(new URL('vendors.json',root),'utf8'));
 assert.equal(data.context7_queries_executed,3);
 assert.equal(data.vendors.filter(v=>v.context7_library_id!==null).length,3);
 for(const v of data.vendors){
  assert.ok(v.official_sources.length);assert.ok(v.offline_notes.length);
  const planned=v.planned_use_sites??[],generated=v.generated_sites??[];
  assert.ok(v.use_sites.length+planned.length+generated.length,`${v.name} has no site`);
  for(const site of v.use_sites)assert.ok(await containsFile(site),`${v.name} existing use site ${site} has no file; list it as planned`);
  for(const site of [...planned,...generated])assert.ok(!v.use_sites.includes(site),`${v.name} lists ${site} as both existing and not`);
  for(const site of generated)assert.ok(generatedRoots.some(g=>site.startsWith(g)),`${v.name} generated site ${site} is not a generated directory`);
 }
});
test('vendor task returns documented lookup without dependencies installed',async()=>{
 const result=await run(process.execPath,['scripts/dev.mjs','vendor','schemars'],{cwd:root,capture:true});
 assert.ok(JSON.parse(result.stdout).some(v=>v.context7_library_id==='/gresau/schemars'));
});
test('unknown task fails instead of printing help as success',async()=>{
 await assert.rejects(run(process.execPath,['scripts/dev.mjs','unknown-operation'],{cwd:root,capture:true}),/Unknown task/);
});
