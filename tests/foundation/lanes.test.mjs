/** One lane table drives worktrees, gates, scope, the AGENTS.md table and CODEOWNERS. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LANE_TABLE_BEGIN, LANE_TABLE_END, laneNamed, lanes, lanesParent, renderLaneTable } from '../../scripts/lib/lanes.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

/** Case-sensitive existence, so `/Justfile` cannot stand in for `justfile` on Windows or macOS. */
async function existsExact(relative){
 let directory=root;
 for(const segment of relative.split('/').filter(Boolean)){
  let names;try{names=await readdir(directory);}catch{return false;}
  if(!names.includes(segment))return false;
  directory=join(directory,segment);
 }
 return true;
}

test('the lane table is well formed and matches the crates it gates',async()=>{
 assert.equal(new Set(lanes.map(lane=>lane.name)).size,lanes.length);
 for(const lane of lanes){
  assert.equal(lane.branch,`build/${lane.name}`);
  assert.ok(['rust','ui'].includes(lane.kind),lane.name);
  assert.ok(lane.directories.length>0,lane.name);
  for(const directory of lane.directories){assert.ok(directory.endsWith('/'),directory);assert.ok(await existsExact(directory),`${lane.name}: ${directory} does not exist`);}
  for(const path of [...lane.files,...lane.exclude])assert.ok(await existsExact(path),`${lane.name}: ${path} does not exist`);
  assert.ok(lane.proves.length>0,lane.name);
  if(lane.kind==='ui'){assert.deepEqual(lane.crates,[]);assert.deepEqual(lane.features,[]);continue;}
  assert.ok(lane.crates.length>0,lane.name);
  const manifests=await Promise.all(lane.directories.map(directory=>read(`${directory}Cargo.toml`)));
  for(const crate of lane.crates){
   const manifest=manifests.find(text=>text.includes(`name = "${crate}"`));
   assert.ok(manifest,`${lane.name}: no manifest under its directories declares ${crate}`);
   for(const feature of lane.features.filter(entry=>entry.startsWith(`${crate}/`)))assert.match(manifest,new RegExp(`^${feature.slice(crate.length+1)} = \\[`,'m'),`${crate} declares no feature ${feature}`);
  }
  for(const feature of lane.features)assert.ok(lane.crates.includes(feature.split('/')[0]),`${lane.name}: ${feature} names a crate the lane does not gate`);
  for(const directory of lane.directories)assert.ok(lane.exclude.includes(`${directory}Cargo.toml`),`${lane.name}: ${directory}Cargo.toml is a manifest and must be excluded`);
 }
});
test('what the views lane owns is excluded from workspace-ui, and every unit spec runs in exactly one UI lane',async()=>{
 const views=laneNamed('views'),workspace=laneNamed('workspace-ui');
 for(const path of [...views.directories,...views.files])assert.ok(workspace.exclude.includes(path),`workspace-ui does not exclude ${path}`);
 const specs=(await readdir(join(root,'ui/tests/unit'))).filter(name=>/\.test\.tsx?$/.test(name)).map(name=>`tests/unit/${name}`);
 for(const spec of specs)assert.equal(views.tests.includes(spec),workspace.testExclude.includes(spec),`${spec} must run in exactly one UI lane`);
 for(const filter of views.tests.filter(entry=>entry.startsWith('tests/unit/')))assert.ok(specs.includes(filter),`views filter ${filter} matches no spec`);
 assert.throws(()=>laneNamed('no-such-lane'),/Unknown lane: no-such-lane/);
});
test('the lane parent directory is configurable',()=>{
 const configured=resolve(root,'..','elsewhere');
 assert.equal(lanesParent(root,{OKF_LANES_DIR:configured},'win32',()=>true),configured);
 assert.equal(lanesParent(root,{OKF_LANES_DIR:'  '},'win32',()=>true),'D:\\okf\\lanes');
 assert.equal(lanesParent(root,{},'win32',()=>false),resolve(root,'..','okf-jawn-lanes'));
 assert.equal(lanesParent(root,{},'linux',()=>true),resolve(root,'..','okf-jawn-lanes'));
});
test('AGENTS.md carries the table rendered from the lane table',async()=>{
 const agents=await read('AGENTS.md');
 const begin=agents.indexOf(LANE_TABLE_BEGIN),end=agents.indexOf(LANE_TABLE_END);
 assert.ok(begin>=0&&end>begin,'AGENTS.md is missing the lane-table markers');
 assert.equal(agents.slice(begin+LANE_TABLE_BEGIN.length,end).trim(),renderLaneTable());
 for(const lane of lanes)assert.ok(agents.includes(`\`bun scripts/dev.mjs lane ${lane.name}\``),lane.name);
});
test('CODEOWNERS names real paths and its lane rows agree with the lane table',async()=>{
 const rows=(await read('.github/CODEOWNERS')).split(/\r?\n/).filter(line=>line.trim()&&!line.startsWith('#'))
  .map(line=>({path:line.split(/\s+/)[0],lane:/# lane: (\S+)/.exec(line)?.[1]??null}));
 for(const row of rows){
  assert.ok(row.path.startsWith('/'),row.path);
  assert.ok(await existsExact(row.path),`CODEOWNERS path ${row.path} does not exist with that exact spelling`);
 }
 for(const lane of lanes)assert.deepEqual(rows.filter(row=>row.lane===lane.name).map(row=>row.path).sort(),[...lane.directories,...lane.files].map(path=>`/${path}`).sort(),lane.name);
 for(const row of rows.filter(entry=>entry.lane))assert.ok(lanes.some(lane=>lane.name===row.lane),`CODEOWNERS names unknown lane ${row.lane}`);
});
