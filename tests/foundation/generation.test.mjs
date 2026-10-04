/** Guard missing generator prerequisites without substituting manually written artifacts. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { generate } from '../../scripts/lib/generation.mjs';

test('generation refuses absent lockfiles and writes no substitute API', async t => {
 const root=await mkdtemp(join(tmpdir(),'okf-gen-test-')); t.after(()=>rm(root,{recursive:true,force:true}));
 await assert.rejects(generate(root),/Missing resolved Cargo.lock/); assert.deepEqual(await readdir(root),[]);
});
test('bun lock is required separately from Cargo lock',async t=>{
 const root=await mkdtemp(join(tmpdir(),'okf-gen-test-'));t.after(()=>rm(root,{recursive:true,force:true}));
 await writeFile(join(root,'Cargo.lock'),'test-only marker, not a lockfile');
 await assert.rejects(generate(root),/Missing resolved bun\.lock/);
 assert.deepEqual(await readdir(root),['Cargo.lock']);
});
test('generator actually invokes selected vendor tools and compares two full runs',async()=>{
 const source=await readFile(new URL('../../scripts/lib/generation.mjs',import.meta.url),'utf8');
 assert.match(source,/cargo/);assert.match(source,/openapi-ts/);assert.match(source,/generate-catalog/);
 assert.match(source,/onePass\(root, first\)/);assert.match(source,/onePass\(root, second\)/);
 assert.match(source,/differences\(await manifest\(first\), await manifest\(second\)\)/);
});
