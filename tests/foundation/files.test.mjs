/** Unit tests of actual source tooling; these do not claim product acceptance. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, readdir, rm, stat, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { files, manifest, differences, replaceGenerated } from '../../scripts/lib/files.mjs';

async function fixture(t) { const p = await mkdtemp(join(tmpdir(), 'okf-files-test-')); t.after(() => rm(p, { recursive:true, force:true })); return p; }
test('manifest detects changed bytes and removed files, not only existing file hashes', async t => {
  const root = await fixture(t); await writeFile(join(root,'b'),'second'); await writeFile(join(root,'a'),'first');
  const before = await manifest(root); assert.deepEqual(Object.keys(before), ['a','b']);
  await writeFile(join(root,'a'),'different'); await rm(join(root,'b'));
  assert.deepEqual(differences(before,await manifest(root)),['a','b']);
});
test('repeated manifests are identical', async t => {
  const root=await fixture(t); await writeFile(join(root,'same'),'payload');
  assert.deepEqual(await manifest(root),await manifest(root));
});
test('generator traversal refuses symlinks instead of following unrelated source', async t => {
  const root=await fixture(t); await symlink('/etc/passwd',join(root,'link'));
  await assert.rejects(files(root),/symlink/);
});
test('publishing removes stale generated files but leaves authored siblings intact', async t => {
  const root=await fixture(t); const source=join(root,'stage'); await mkdir(source); await writeFile(join(source,'new.json'),'{}');
  const output=join(root,'api'); await mkdir(output); await writeFile(join(output,'old.json'),'old'); await writeFile(join(root,'SPEC.md'),'keep');
  await replaceGenerated(source,output,root);
  assert.deepEqual(await files(output),['new.json']); assert.equal(await readFile(join(root,'SPEC.md'),'utf8'),'keep');
});
test('publishing keeps the watched destination directory and leaves no staging behind', async t => {
  const root=await fixture(t); const source=join(root,'stage'); await mkdir(join(source,'nested'),{recursive:true});
  await writeFile(join(source,'nested','types.ts'),'new');
  const output=join(root,'generated','cli'); await mkdir(join(output,'stale'),{recursive:true}); await writeFile(join(output,'stale','x'),'old');
  const before=(await stat(output)).ino;
  await replaceGenerated(source,output,root);
  assert.equal((await stat(output)).ino,before);
  assert.deepEqual(await files(output),['nested/types.ts']);
  assert.deepEqual(await readdir(join(root,'generated')),['cli']);
});
test('publishing to an authored directory is refused', async t => {
  const root=await fixture(t); await mkdir(join(root,'source')); await assert.rejects(replaceGenerated(join(root,'source'),join(root,'crates'),root),/non-generated/);
});
test('invalid staged source cannot remove previous generated content', async t => {
  const root=await fixture(t); await mkdir(join(root,'stage')); await symlink('/etc/passwd',join(root,'stage','bad'));
  await mkdir(join(root,'api')); await writeFile(join(root,'api','old.json'),'preserved');
  await assert.rejects(replaceGenerated(join(root,'stage'),join(root,'api'),root),/symlink/);
  assert.equal(await readFile(join(root,'api','old.json'),'utf8'),'preserved');
});

test('a symlinked generated-output ancestor cannot redirect publication outside the repo', async t => {
  const root=await fixture(t); const outside=await fixture(t); await mkdir(join(root,'stage')); await writeFile(join(root,'stage','types.ts'),'generated');
  await symlink(outside,join(root,'ui'));
  await assert.rejects(replaceGenerated(join(root,'stage'),join(root,'ui','src','api','generated'),root),/ancestor/);
  assert.deepEqual(await files(outside),[]);
});
