/** Initializing a checkout never overwrites configuration or adds a remote. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp,mkdir,writeFile,readFile,rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { initialize } from '../../scripts/lib/init.mjs';
import { run } from '../../scripts/lib/process.mjs';
test('init is idempotent and preserves existing local configuration',async t=>{
 const root=await mkdtemp(join(tmpdir(),'okf-init-'));t.after(()=>rm(root,{recursive:true,force:true}));
 await mkdir(join(root,'deploy'));await writeFile(join(root,'deploy','.env.example'),'sample');
 await initialize(root);await writeFile(join(root,'.env'),'user configuration');await initialize(root);
 assert.equal(await readFile(join(root,'.env'),'utf8'),'user configuration');
 assert.equal((await run('git',['remote'],{cwd:root,capture:true})).stdout,'');
 await assert.rejects(run('git',['rev-parse','--verify','HEAD'],{cwd:root,capture:true}));
});
