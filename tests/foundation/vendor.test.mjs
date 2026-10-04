/** Vendor lookup remains useful without a connector, API key, or network. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { run } from '../../scripts/lib/process.mjs';
const root=new URL('../../',import.meta.url);

test('offline index records only the three executed Context7 queries as such',async()=>{
 const data=JSON.parse(await readFile(new URL('vendors.json',root),'utf8'));
 assert.equal(data.context7_queries_executed,3);
 assert.equal(data.vendors.filter(v=>v.context7_library_id!==null).length,3);
 for(const v of data.vendors){assert.ok(v.official_sources.length);assert.ok(v.offline_notes.length);assert.ok(v.use_sites.length);}
});
test('vendor task returns documented lookup without dependencies installed',async()=>{
 const result=await run(process.execPath,['scripts/dev.mjs','vendor','schemars'],{cwd:root,capture:true});
 assert.ok(JSON.parse(result.stdout).some(v=>v.context7_library_id==='/gresau/schemars'));
});
test('unknown task fails instead of printing help as success',async()=>{
 await assert.rejects(run(process.execPath,['scripts/dev.mjs','unknown-operation'],{cwd:root,capture:true}),/Unknown task/);
});
