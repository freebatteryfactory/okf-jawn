/** Gate sequences are data; the runner logs every step and ends with one verdict line. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { cleanCheckout, cleanCheckoutTasks, foundationTests, laneSteps, premergeSteps, revisionLabel, runLane, runPremerge } from '../../scripts/lib/gates.mjs';
import { laneNamed, lanes } from '../../scripts/lib/lanes.mjs';
import { run } from '../../scripts/lib/process.mjs';
import { fixtureRepo, git } from './fixture-repo.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');
const fixture={'.gitignore':'/.artifacts/\n','README.md':'fixture\n'};
const passing=async(step,write)=>{write(`ran ${step.id}\n`);return 0;};
const quiet={echo:()=>{}};
const lastLine=async path=>(await readFile(path,'utf8')).trimEnd().split('\n').at(-1);

test('a Rust lane gate is fmt, Clippy and tests for its crates and features, source policy, then scope',()=>{
 const steps=laneSteps('/repo',laneNamed('storage'));
 assert.deepEqual(steps.map(step=>step.id),['fmt','clippy','test','source-policy','scope']);
 assert.deepEqual(steps.slice(0,4).map(step=>step.command),['cargo','cargo','cargo','cargo']);
 assert.deepEqual(steps[0].args,['fmt','--check','-p','okf-jawn-storage']);
 assert.deepEqual(steps[1].args,['clippy','--locked','-p','okf-jawn-storage','--features','okf-jawn-storage/runtime','--all-targets','--','-D','warnings']);
 assert.deepEqual(steps[2].args,['test','--locked','-p','okf-jawn-storage','--features','okf-jawn-storage/runtime']);
 assert.deepEqual(steps[3].args,['xtask','source-policy','--root','/repo']);
 assert.deepEqual(steps[4].args,['scripts/dev.mjs','scope','storage']);
 const core=laneSteps('/repo',laneNamed('core-cli'));
 assert.deepEqual(core[1].args,['clippy','--locked','-p','okf-jawn-core','-p','okf-jawn-cli','--all-targets','--','-D','warnings']);
 assert.deepEqual(core[2].args,['test','--locked','-p','okf-jawn-core','-p','okf-jawn-cli']);
});
test('every Rust lane checks the formatting of its own crates only, never the workspace',()=>{
 for(const lane of lanes.filter(entry=>entry.kind==='rust')){
  const args=laneSteps('/repo',lane)[0].args;
  assert.deepEqual(args,['fmt','--check',...lane.crates.flatMap(name=>['-p',name])],lane.name);
  assert.ok(!args.includes('--all'),lane.name);
 }
});
test('a UI lane gate is Biome, route generation, tsc, Vitest filtered to the lane, then scope',async()=>{
 const views=laneSteps('/repo',laneNamed('views'));
 assert.deepEqual(views.map(step=>step.id),['biome','routes','typecheck','test','scope']);
 assert.deepEqual(views[0].args,['--bun','run','lint']);
 assert.deepEqual(views[1].args,['scripts/dev.mjs','routes']);
 assert.deepEqual(views[2].args,['--bun','run','typecheck']);
 assert.deepEqual(views[3].args,['--bun','run','test',...laneNamed('views').tests]);
 assert.ok(views[3].cwd.endsWith('ui'));
 const workspace=laneSteps('/repo',laneNamed('workspace-ui'))[3].args;
 assert.deepEqual(workspace.slice(0,5),['--bun','run','test','--exclude','src/features/views/**']);
 assert.equal(workspace.filter(argument=>argument==='--exclude').length,laneNamed('workspace-ui').testExclude.length);
 const config=await read('ui/vitest.config.ts');
 for(const glob of ['tests/unit/**/*.test.{ts,tsx}','src/**/*.test.{ts,tsx}'])assert.ok(config.includes(`'${glob}'`),`ui/vitest.config.ts does not include ${glob}`);
 const entry=await read('scripts/dev.mjs');
 assert.match(entry,/resolveConfig\(\{\}, 'build'\)/);assert.match(entry,/case 'routes':/);
});
test('every lane has a gate, and the lane task ends its log with one verdict line',async t=>{
 for(const lane of lanes)assert.equal(laneSteps('/repo',lane).at(-1).id,'scope',lane.name);
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const result=await runLane(repo,'storage',{execute:passing,...quiet});
 assert.equal(result.passed,true);assert.equal(result.line,`PASS storage ${sha}`);
 assert.ok(result.logPath.endsWith(join('.artifacts','lane','storage',`${sha}.log`)));
 const log=await readFile(result.logPath,'utf8');
 assert.equal(await lastLine(result.logPath),`PASS storage ${sha}`);
 for(const id of ['fmt','clippy','test','source-policy','scope'])assert.ok(log.includes(`=== ${id}: `)&&log.includes(`ran ${id}\n=== ${id} exit 0\n`),id);
});
test('a failing step stops the lane gate and is named; an uncommitted tree is never labelled as its commit',async t=>{
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const ran=[];
 const failing=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);return step.id==='clippy'?101:0;};
 const result=await runLane(repo,'storage',{execute:failing,...quiet});
 assert.equal(result.passed,false);assert.equal(result.line,`FAIL storage ${sha} clippy`);
 assert.deepEqual(ran,['fmt','clippy']);
 assert.equal(await lastLine(result.logPath),`FAIL storage ${sha} clippy`);
 await writeFile(join(repo,'README.md'),'edited, not committed\n');
 assert.equal((await runLane(repo,'storage',{execute:passing,...quiet})).line,`PASS storage ${sha}-dirty`);
 await assert.rejects(runLane(repo,'no-such-lane',{execute:passing,...quiet}),/Unknown lane: no-such-lane/);
});
test('premerge is the whole CI sequence with every feature, and includes check-receipts',()=>{
 const steps=premergeSteps('/repo');
 assert.deepEqual(steps.map(step=>step.id),['check-offline','gen-check','fmt','clippy','source-policy','test','ui-lint','ui-build','ui-typecheck','ui-test','check-receipts']);
 const args=id=>steps.find(step=>step.id===id).args;
 assert.deepEqual(args('clippy'),['clippy','--locked','--workspace','--all-features','--all-targets','--','-D','warnings']);
 assert.deepEqual(args('test'),['test','--locked','--workspace','--all-features']);
 assert.deepEqual(args('gen-check'),['scripts/dev.mjs','gen-check']);
 assert.deepEqual(args('check-receipts'),['scripts/dev.mjs','check-receipts']);
});
test('premerge runs every step and reports all failures, not the first',async t=>{
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const ran=[];
 const execute=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);return ['fmt','ui-test'].includes(step.id)?1:0;};
 const result=await runPremerge(repo,{execute,...quiet});
 assert.deepEqual(ran,premergeSteps(repo).map(step=>step.id));
 assert.equal(result.passed,false);assert.equal(result.line,`FAIL premerge ${sha} fmt,ui-test`);
 assert.equal(await lastLine(result.logPath),`FAIL premerge ${sha} fmt,ui-test`);
 assert.equal((await runPremerge(repo,{execute:passing,...quiet})).line,`PASS premerge ${sha}`);
 await assert.rejects(runPremerge(repo,{only:'no-such-step'}),/Unknown premerge step: no-such-step/);
});
test('check and test are subsets of premerge, so the cargo flags are written once',async()=>{
 const entry=await read('scripts/dev.mjs');
 assert.match(entry,/case 'check': await runNamed\(\['fmt', 'clippy', 'source-policy', 'ui-lint', 'ui-typecheck', 'gen-check'\]\); break;/);
 assert.match(entry,/case 'test': await runNamed\(\['test', 'ui-test'\]\); break;/);
 assert.doesNotMatch(entry,/'--all-targets'|'--all-features'/);
});
test('clean-checkout runs the whole foundation in a detached worktree of HEAD and removes it',async t=>{
 const {base,root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const parent=join(base,'lanes');const worktree=join(parent,`clean-checkout-${sha.slice(0,12)}`);const ran=[];
 const execute=async(step,write)=>{ran.push([step.id,step.cwd,step.args.join(' ')]);write(`ran ${step.id}\n`);return 0;};
 const result=await cleanCheckout(repo,{parent,execute,...quiet,now:()=>new Date('2026-10-05T18:00:00Z')});
 assert.deepEqual(ran.map(([id])=>id),['bootstrap','gen-check-1','gen-check-2','foundation','check','test','check-offline','audit']);
 assert.deepEqual(ran.map(([,,args])=>args),['bootstrap','gen-check','gen-check','foundation','check','test','check-offline','audit'].map(name=>`scripts/dev.mjs ${name}`));
 for(const [,cwd] of ran)assert.equal(cwd,worktree);
 assert.deepEqual(result.receipt,{git_sha:sha,inputs:['.'],produced_at:'2026-10-05T18:00:00.000Z',exit_codes:Object.fromEntries(ran.map(([id])=>[id,0])),git_status_empty:true});
 assert.deepEqual(Object.keys(result.receipt),['git_sha','inputs','produced_at','exit_codes','git_status_empty']);
 assert.deepEqual(JSON.parse(await readFile(join(repo,'.artifacts','qualification','clean-checkout','receipt.json'),'utf8')),result.receipt);
 assert.equal(result.passed,true);assert.equal(result.removed,true);
 assert.equal(existsSync(worktree),false);
 assert.equal((await git(repo,'worktree','list','--porcelain')).split(/\r?\n/).filter(line=>line.startsWith('worktree ')).length,1);
});
test('clean-checkout records a failing step and a drifted tree, keeps the evidence, and never forces',async t=>{
 const {base,root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const parent=join(base,'lanes');const ran=[];
 const execute=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);if(step.id==='gen-check-2')await writeFile(join(step.cwd,'drift.json'),'{}');return step.id==='check'?3:0;};
 const result=await cleanCheckout(repo,{parent,execute,...quiet});
 assert.equal(ran.length,cleanCheckoutTasks.length,'a failing step does not stop the run');
 assert.equal(result.receipt.exit_codes.check,3);assert.equal(result.receipt.git_status_empty,false);
 assert.equal(result.passed,false);assert.equal(result.removed,false);
 assert.equal(existsSync(join(parent,`clean-checkout-${sha.slice(0,12)}`,'drift.json')),true,'a drifted worktree is left for inspection');
 await assert.rejects(cleanCheckout(repo,{parent,execute,...quiet}),/clean-checkout refused: .* already exists/);
 // `-D` is Clippy's deny flag here, so only the force spellings are forbidden in this file.
 assert.doesNotMatch(await read('scripts/lib/gates.mjs'),/'--force'|'-f'/,'gates.mjs must never force git');
});
test('every foundation test file is in exactly one of the fast and slow lists, and a fast file spawns nothing',async()=>{
 const onDisk=(await readdir(join(root,'tests/foundation'))).filter(name=>name.endsWith('.test.mjs')).sort();
 assert.deepEqual([...foundationTests.fast,...foundationTests.slow].sort(),onDisk,'a *.test.mjs file is in neither list, in both, or listed but missing');
 for(const name of foundationTests.fast){
  const source=await read(`tests/foundation/${name}`);
  assert.doesNotMatch(source,/fixture-repo|process\.mjs|child_process|Bun\.spawn|Bun\.\$|\$\{?\s*\bgit\b/,`${name} is listed fast but creates a repository or spawns a process`);
 }
 const entry=await read('scripts/dev.mjs');
 assert.match(entry,/case 'check-offline': await offlineChecks\(\{ fast: args\.includes\('--fast'\) \}\); break;/);
 assert.match(entry,/!fast \|\| foundationTests\.fast\.includes\(name\)/);
});
/** Files that may start with a UTF-8 byte-order mark. None: no tracked file needs one, and every tool here reads UTF-8 without it. */
const bomAllowed=[];
/** The offset of the first byte that makes `bytes` invalid UTF-8. */
function firstInvalidUtf8(bytes){
 // With `stream` an unfinished final sequence is not an error, so a prefix fails only once it holds the bad byte.
 const valid=length=>{try{new TextDecoder('utf-8',{fatal:true}).decode(bytes.subarray(0,length),{stream:true});return true;}catch{return false;}};
 if(valid(bytes.length))return bytes.length-1;
 let low=0,high=bytes.length;
 while(high-low>1){const middle=(low+high)>>1;if(valid(middle))low=middle;else high=middle;}
 return high-1;
}
/**
 * Every tracked text file of `repo` with its bytes. Git decides what is text. A file whose `text`
 * attribute is unset (the `binary` macro in .gitattributes) is binary. For every other file
 * (`text=auto`) git's own content detection decides, as `git ls-files --eol` reports it, with
 * one correction: that detection calls a file with a single NUL byte binary, which is one of the
 * defects looked for here, so a detected-binary file is left out only when it is not UTF-8 either.
 * No list of extensions is kept.
 */
async function trackedText(repo){
 const listed=(await run('git',['ls-files','--eol','-z'],{cwd:repo,capture:true})).stdout.split('\0').filter(Boolean);
 const files=[];
 for(const line of listed){
  const match=/^i\/(\S*)\s+w\/(\S*)\s+attr\/(.*?)\s*\t(.*)$/s.exec(line);
  assert.ok(match,`git ls-files --eol printed a line this guard cannot read: ${JSON.stringify(line)}`);
  const [,index,work,attributes,path]=match;
  if(/^-text\b/.test(attributes))continue;
  // Deleted in the working tree and not yet staged: there are no bytes to read.
  if(!existsSync(join(repo,path)))continue;
  const bytes=await readFile(join(repo,path));
  let utf8=true;
  try{new TextDecoder('utf-8',{fatal:true}).decode(bytes);}catch{utf8=false;}
  if(!utf8&&(work||index)==='-text')continue;
  files.push({path,bytes,utf8});
 }
 return files;
}
/** What is wrong with the tracked text files of `repo`, as `{ path, kind, message }`: a raw control byte, bytes that are not UTF-8, or a byte-order mark. */
async function textProblems(repo){
 const files=await trackedText(repo);
 const problems=[];
 for(const {path,bytes,utf8} of files){
  // Tab, LF and CR are legitimate; every other byte below 0x20 (a NUL, a backspace from a mistyped `\b`) is not.
  const at=bytes.findIndex(byte=>byte<0x20&&byte!==0x09&&byte!==0x0a&&byte!==0x0d);
  if(at>=0)problems.push({path,kind:'control',message:`${path} has the raw control byte 0x${bytes[at].toString(16).padStart(2,'0')} at offset ${at}`});
  if(!utf8)problems.push({path,kind:'utf8',message:`${path} is not valid UTF-8: byte 0x${bytes[firstInvalidUtf8(bytes)].toString(16)} at offset ${firstInvalidUtf8(bytes)}`});
  else if(bytes[0]===0xef&&bytes[1]===0xbb&&bytes[2]===0xbf&&!bomAllowed.includes(path))problems.push({path,kind:'bom',message:`${path} starts with a UTF-8 byte-order mark`});
 }
 return {scanned:files.length,problems};
}
test('no tracked text file, in any directory, contains a raw control byte',async()=>{
 const {scanned,problems}=await textProblems(root);
 assert.ok(scanned>300,`the scan read only ${scanned} files`);
 assert.deepEqual(problems.filter(problem=>problem.kind==='control').map(problem=>problem.message),[]);
});
test('every tracked text file is valid UTF-8 and has no byte-order mark',async()=>{
 const {problems}=await textProblems(root);
 assert.deepEqual(problems.filter(problem=>problem.kind!=='control').map(problem=>problem.message),[]);
 assert.deepEqual(bomAllowed,[],'a file allowed a byte-order mark needs its reason written beside it');
});
test('the text guards read every directory and leave out only what git treats as binary',async t=>{
 const bytes=(...parts)=>Buffer.concat(parts.map(part=>typeof part==='string'?Buffer.from(part,'utf8'):Buffer.from(part)));
 const zip=bytes([0x50,0x4b,0x03,0x04,0x00,0x00,0xff,0xfe,0x08,0x00,0x9c,0xa7]);
 const {root:repo}=await fixtureRepo(t,{
  '.gitattributes':'* text=auto eol=lf\n*.png binary\n',
  'README.md':'plain text with a tab\tand a section sign \u00a7 in UTF-8\n',
  'crates/storage/src/backspace.rs':bytes('//! a word boundary typed raw: ',[0x08],'\n'),
  'crates/storage/src/zeroes.rs':bytes('//! two NUL bytes ',[0x00,0x00],'\n'),
  'docs/plans/latin1.md':bytes('Section ',[0xa7],' 5, saved as Latin-1\n'),
  'ui/src/bom.ts':bytes([0xef,0xbb,0xbf],'export const marked = true;\n'),
  'scripts/escape.mjs':bytes('const bell = "',[0x1b],'[0m";\n'),
  // Declared binary by attribute, although git's content detection would call these bytes text.
  'tests/fixtures/declared.png':bytes('reads like text ',[0x08,0xa7],'\n'),
  'tests/fixtures/undeclared.docx':zip,
 });
 const {scanned,problems}=await textProblems(repo);
 assert.equal(scanned,7,'the declared and the detected binary file are the only ones left out');
 assert.deepEqual(problems.map(problem=>problem.message).sort(),[
  'crates/storage/src/backspace.rs has the raw control byte 0x08 at offset 31',
  'crates/storage/src/zeroes.rs has the raw control byte 0x00 at offset 18',
  'docs/plans/latin1.md is not valid UTF-8: byte 0xa7 at offset 8',
  'scripts/escape.mjs has the raw control byte 0x1b at offset 14',
  'ui/src/bom.ts starts with a UTF-8 byte-order mark',
 ]);
 // An unfinished sequence at the very end is named at the last byte.
 assert.equal(firstInvalidUtf8(Buffer.from([0x61,0xc3])),1);
 assert.equal(firstInvalidUtf8(Buffer.from([0x61,0x62,0xff,0x63])),2);
});
test('a failed git status counts as dirty, never as the clean commit',async t=>{
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 assert.equal(await revisionLabel(repo),sha);
 // A corrupt index makes `git status` fail while `git rev-parse HEAD` still works.
 await writeFile(join(repo,'.git','index'),'not an index');
 await assert.rejects(git(repo,'status','--porcelain'));
 assert.equal(await revisionLabel(repo),`${sha}-dirty`);
});
