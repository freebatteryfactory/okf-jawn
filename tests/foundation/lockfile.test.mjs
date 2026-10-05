/** Cargo.lock agrees with the pins the records state; resolution is a deliberate act. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

/** Every `[[package]]` of a Cargo.lock as {name, version, dependencies}. */
function packages(lock){
 return lock.split(/\r?\n\[\[package\]\]\r?\n/).slice(1).map(block=>({
  name:/^name = "([^"]+)"/m.exec(block)?.[1],
  version:/^version = "([^"]+)"/m.exec(block)?.[1],
  dependencies:[...(/^dependencies = \[\r?\n([\s\S]*?)^\]/m.exec(block)?.[1].matchAll(/"([^"]+)"/g)??[])].map(m=>m[1]),
 }));
}

test('Docling sibling crates stay on the release the records name',async()=>{
 const locked=packages(await read('Cargo.lock'));
 const versions=['docling-core','docling-onnx','docling-pdf'].map(name=>{
  const found=locked.filter(p=>p.name===name);
  assert.equal(found.length,1,`${name} must be locked exactly once`);
  return found[0].version;
 });
 assert.equal(new Set(versions).size,1,`docling-core, docling-onnx and docling-pdf disagree: ${versions.join(', ')}`);
 const core=versions[0];
 const docling=JSON.parse(await read('vendors.json')).vendors.find(v=>v.name==='docling');
 assert.ok(docling.offline_notes.some(note=>note.includes(`docling-core ${core}`)),`Cargo.lock has docling-core ${core}; vendors.json does not name it`);
 assert.ok(JSON.parse(await read('verification.json')).current.direct_versions.docling.includes(`docling-core ${core}`),`Cargo.lock has docling-core ${core}; verification.json does not name it`);
 const pinned=/^docling = \{ version = "=([^"]+)"/m.exec(await read('Cargo.toml'))?.[1];
 assert.equal(locked.find(p=>p.name==='docling')?.version,pinned,'Cargo.lock docling differs from the Cargo.toml pin');
});
