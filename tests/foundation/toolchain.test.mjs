/** The selected JavaScript toolchain is declared once and every active entrypoint uses it. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { pins } from '../../scripts/lib/toolchain.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

test('Bun is selected once and its version file agrees',async()=>{
 const selected=await pins(root);
 assert.match(selected.bun,/^\d+\.\d+\.\d+$/);
 assert.equal(JSON.parse(await read('package.json')).packageManager,`bun@${selected.bun}`);
 assert.equal((await read('.bun-version')).trim(),selected.bun);
 const ui=JSON.parse(await read('ui/package.json'));
 assert.equal(ui.packageManager,undefined);assert.equal(ui.engines,undefined);
});
test('disagreeing Bun declarations are rejected, not reconciled',async t=>{
 const dir=await mkdtemp(join(tmpdir(),'okf-pins-'));t.after(()=>rm(dir,{recursive:true,force:true}));
 await writeFile(join(dir,'package.json'),JSON.stringify({packageManager:'bun@1.4.2'}));
 await writeFile(join(dir,'.bun-version'),'1.4.1\n');
 await writeFile(join(dir,'rust-toolchain.toml'),'[toolchain]\nchannel = "1.99.0"\n');
 await assert.rejects(pins(dir),/disagrees/);
});
test('CI and the build container take Bun from the version declarations',async()=>{
 const ci=await read('.github/workflows/ci.yml');
 assert.match(ci,/oven-sh\/setup-bun@[0-9a-f]{40}/);assert.match(ci,/bun-version-file: \.bun-version/);
 assert.doesNotMatch(ci,/setup-node|pnpm|node-version/);
 const docker=await read('deploy/Dockerfile');
 assert.match(docker,/ARG BUN_IMAGE/);assert.doesNotMatch(docker,/pnpm|NODE_IMAGE|npm install/);
});
test('no active configuration requires pnpm or a Node version',async()=>{
 for(const file of ['package.json','ui/package.json','justfile','lefthook.yml','scripts/dev.mjs','scripts/lib/generation.mjs','deploy/compose.yaml','deploy/.env.example','AGENTS.md','README.md']){
  const source=await read(file);
  assert.doesNotMatch(source,/\bpnpm(?:@|\s+(?:install|i|add|run|exec|dlx|test|build|-r|--filter)\b)|pnpm-(?:lock|workspace)|onlyBuiltDependencies|strict-peer-dependencies=|\.node-version|NODE_IMAGE|\bnode scripts\//,file);
 }
 for(const gone of ['pnpm-workspace.yaml','.npmrc','.node-version'])await assert.rejects(readFile(join(root,gone)),{code:'ENOENT'},gone);
});
test('only reviewed dependencies may run install scripts, and Scarf is disabled',async()=>{
 assert.deepEqual(JSON.parse(await read('package.json')).trustedDependencies,[]);
 assert.equal(JSON.parse(await read('package.json')).scarfSettings?.enabled,false);
});
