/** Records, prose and scripts agree with the generated surface and with what was actually run. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
import { libraryGates } from '../../scripts/lib/receipts.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

test('nothing tracked is also ignored, and no snapshot of the tree is kept',async()=>{
 await assert.rejects(read('REPOSITORY-TREE.txt'),{code:'ENOENT'});
 for(const file of ['README.md','AGENTS.md','.github/CODEOWNERS','justfile','scripts/dev.mjs'])assert.doesNotMatch(await read(file),/REPOSITORY-TREE/,file);
 assert.match(await read('scripts/dev.mjs'),/case 'tree':/,'the on-demand view stays');
 // A source copy without history (the build container) has no index to inspect.
 if(!existsSync(join(root,'.git')))return;
 const ignored=await run('git',['ls-files','-ci','--exclude-standard'],{cwd:root,capture:true});
 assert.equal(ignored.stdout.trim(),'','these tracked files are ignored by .gitignore');
});
test('verification.json records no pass while Phase 0 is reopened, and only terminal gates once qualified',async()=>{
 const record=JSON.parse(await read('verification.json'));
 const gates=record.current.gates.phase_0;
 assert.equal(new Set(gates.map(gate=>gate.id)).size,gates.length);
 for(const id of libraryGates)assert.ok(gates.some(gate=>gate.id===id),`Phase 0 gate ${id} is missing`);
 if(record.phase_0_qualified===true){
  assert.equal(record.current.deterministic_foundation_green,true);
  for(const gate of gates)assert.ok(['passed','rejected_with_fallback','bounded_upstream_exception'].includes(gate.status),`${gate.id} is ${gate.status} although phase_0_qualified is true`);
  return;
 }
 assert.equal(record.current.deterministic_foundation_green,false,'deterministic_foundation_green cannot be true while Phase 0 is unqualified');
 for(const gate of gates)assert.notEqual(gate.status,'passed',`${gate.id} is recorded as passed while Phase 0 is reopened`);
 for(const check of record.current.checks)assert.notEqual(check.status,'passed',`${check.command.join(' ')} is recorded as passed while Phase 0 is reopened`);
});
test('the acceptance script calls only declared operations, and edits through a draft and a Snapshot',async()=>{
 const paths=new Set(JSON.parse(await read('api/operations.json')).map(operation=>operation.path));
 const source=await read('tests/integration/acceptance.mjs');
 const calls=[...source.matchAll(/call\('([a-z]+)','([a-z_]+)'/g)];
 assert.ok(calls.length>0);
 for(const [,domain,id] of calls)assert.ok(paths.has(`/api/${domain}/${id.replaceAll('_','-')}`),`acceptance.mjs calls ${domain}/${id}, which is not a declared operation`);
 assert.match(source,/call\('items','save_draft'/);
 assert.match(source,/call\('history','commit_items',\{workspace_id,item_ids:\[item_id\],message:/);
});
test('the deployment example declares and describes every image the compose file and Dockerfile require',async()=>{
 const example=await read('deploy/.env.example');
 const required=[...(await read('deploy/compose.yaml')).matchAll(/\$\{([A-Z_]+):\?/g)].map(match=>match[1]);
 const images=[...(await read('deploy/Dockerfile')).matchAll(/^ARG ([A-Z_]+)$/gm)].map(match=>match[1]);
 assert.ok(required.length>0&&images.length>0);
 for(const name of new Set([...required,...images]))assert.match(example,new RegExp(`^${name}=`,'m'),`deploy/.env.example does not declare ${name}`);
 const described=name=>example.split(/\r?\n/).find(line=>line.startsWith(`# ${name}:`))??'';
 for(const name of images)assert.ok(described(name).length>0,`${name} has no description`);
 assert.match(described('BUN_IMAGE'),/\.bun-version/,'BUN_IMAGE must be described the way compose.yaml and the Dockerfile require it');
});
test('README states the generated counts and tool names, and neither prose file remembers a number',async()=>{
 const readme=await read('README.md'),agents=await read('AGENTS.md');
 const operations=JSON.parse(await read('api/operations.json')),transports=JSON.parse(await read('api/transports.json'));
 const tools=JSON.parse(await read('api/mcp-tools.json')).tools;
 assert.ok(readme.includes(`There are ${operations.length} typed JSON application commands and ${transports.length} distinct transport declarations.`),'README operation and transport counts differ from api/');
 for(const tool of tools.filter(entry=>entry._meta.ui.visibility.includes('model')))assert.ok(readme.includes(`\`${tool.name}\``),`README does not name the model tool ${tool.name}`);
 assert.doesNotMatch(readme,/Utoipa schema types/);
 for(const [name,text] of [['README.md',readme],['AGENTS.md',agents]])assert.doesNotMatch(text,/\b(?:three|3)\b[^.\n]*Context7/i,name);
 for(const term of ['TestResult','err_of','git merge --no-ff','Why:','PowerShell'])assert.ok(agents.includes(term),`AGENTS.md does not state ${term}`);
});
test('help names every task the entrypoint accepts, and just mirrors the gates',async()=>{
 const entry=await read('scripts/dev.mjs');
 const cases=[...entry.matchAll(/^\s+case '([a-z-]+)':/gm)].map(match=>match[1]).filter(name=>name!=='help');
 const help=/case 'help': process\.stdout\.write\('Tasks: ([^\\]+)\\n'\)/.exec(entry)?.[1].split(' ')??[];
 assert.deepEqual([...help].sort(),[...cases].sort());
 const just=await read('justfile');
 for(const recipe of ['lane','premerge','clean-checkout','lanes-reset','check-receipts'])assert.match(just,new RegExp(`^${recipe}\\b`,'m'),`justfile has no ${recipe} recipe`);
});
