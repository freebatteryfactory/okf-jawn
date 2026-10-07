/** Initializing a checkout never overwrites configuration or adds a remote. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp,mkdir,writeFile,readFile,rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { initialize, installHooks } from '../../scripts/lib/init.mjs';
import { run } from '../../scripts/lib/process.mjs';
test('init is idempotent and preserves existing local configuration',async t=>{
 const root=await mkdtemp(join(tmpdir(),'okf-init-'));t.after(()=>rm(root,{recursive:true,force:true}));
 await mkdir(join(root,'deploy'));await writeFile(join(root,'deploy','.env.example'),'sample');
 await initialize(root);await writeFile(join(root,'.env'),'user configuration');await initialize(root);
 assert.equal(await readFile(join(root,'.env'),'utf8'),'user configuration');
 assert.equal((await run('git',['remote'],{cwd:root,capture:true})).stdout,'');
 await assert.rejects(run('git',['rev-parse','--verify','HEAD'],{cwd:root,capture:true}));
});
test('init points the repository at the tracked hooks and leaves a copy without history alone',async t=>{
 const root=await mkdtemp(join(tmpdir(),'okf-hooks-'));t.after(()=>rm(root,{recursive:true,force:true}));
 await mkdir(join(root,'scripts','hooks'),{recursive:true});
 assert.equal(await installHooks(root),false,'no .git: nothing to configure, and no error');
 assert.equal((await initialize(root)).hooksInstalled,true);
 assert.equal((await run('git',['config','--get','core.hooksPath'],{cwd:root,capture:true})).stdout.trim(),'scripts/hooks');
});
