/** check-receipts: receipts describe HEAD, and a qualified Phase 0 has one per library gate. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { checkReceipts, libraryGates, staleReceiptLines } from '../../scripts/lib/receipts.mjs';
import { commit, fixtureRepo, git } from './fixture-repo.mjs';

const paths=Object.fromEntries(libraryGates.map(id=>[id,`qualification/receipts/${id}.json`]));
const verification=(qualified,receipts={})=>JSON.stringify({phase_0_qualified:qualified,current:{gates:{phase_0:libraryGates.map(id=>({id,status:qualified?'passed':'incomplete',receipt:receipts[id]??null}))}}});
const receipt=(sha,inputs=['harness/run.mjs'])=>JSON.stringify({git_sha:sha,inputs,produced_at:'2026-10-05T18:00:00Z'});

test('an empty receipts directory is accepted only while Phase 0 is unqualified',async t=>{
 const {root}=await fixtureRepo(t,{'verification.json':verification(false),'qualification/receipts/.gitkeep':''});
 assert.match(await checkReceipts(root),/accepted only because verification\.json records phase_0_qualified false/);
 await commit(root,{'verification.json':verification(true,paths)},'claim qualification without receipts');
 await assert.rejects(checkReceipts(root),error=>libraryGates.every(id=>error.message.includes(`${id}: phase_0_qualified is true but its receipt`)));
});
test('a qualified Phase 0 passes with one current receipt per library gate',async t=>{
 const {root}=await fixtureRepo(t,{'verification.json':verification(false),'harness/run.mjs':'// harness\n'});
 const sha=await git(root,'rev-parse','HEAD');
 await commit(root,{'verification.json':verification(true,paths),...Object.fromEntries(Object.values(paths).map(path=>[path,receipt(sha)]))},'record R');
 assert.match(await checkReceipts(root),/3 receipt\(s\) valid against HEAD; all 3 Phase 0 library gates have one/);
 await commit(root,{'verification.json':verification(true,{...paths,'iii-library-qualification':'.artifacts/qualification/iii/receipt.json'})},'point a gate outside the committed receipts');
 await assert.rejects(checkReceipts(root),/iii-library-qualification: phase_0_qualified is true but its receipt \(\.artifacts\/qualification\/iii\/receipt\.json\)/);
});
test('a receipt is rejected when an input changed, its commit is foreign, or its header is incomplete',async t=>{
 const {root}=await fixtureRepo(t,{'verification.json':verification(false),'harness/run.mjs':'// v1\n'});
 const sha=await git(root,'rev-parse','HEAD');
 await commit(root,{'qualification/receipts/a.json':receipt(sha)},'receipt');
 assert.match(await checkReceipts(root),/1 receipt\(s\) valid against HEAD\./);
 await commit(root,{'harness/run.mjs':'// v2\n'},'change an input');
 await assert.rejects(checkReceipts(root),/a\.json: inputs changed after [0-9a-f]{40}:\n  harness\/run\.mjs/);
 await commit(root,{'qualification/receipts/a.json':receipt('0'.repeat(40))},'foreign commit');
 await assert.rejects(checkReceipts(root),/a\.json: git_sha 0{40} is not an ancestor of HEAD/);
 await commit(root,{'qualification/receipts/a.json':JSON.stringify({git_sha:sha,inputs:[],produced_at:'2026-10-05T18:00:00Z'})},'no inputs');
 await assert.rejects(checkReceipts(root),/a\.json: inputs must be a non-empty string\[\]/);
 await commit(root,{'qualification/receipts/a.json':JSON.stringify({git_sha:sha,inputs:['harness/run.mjs']})},'no time');
 await assert.rejects(checkReceipts(root),/a\.json: produced_at must be an RFC 3339 time/);
 await commit(root,{'qualification/receipts/a.json':'{not json'},'not json');
 await assert.rejects(checkReceipts(root),/a\.json: not valid JSON/);
});
test('staleReceiptLines judges the given commit and its receipts, whatever is checked out',async t=>{
 const {root}=await fixtureRepo(t,{'verification.json':verification(false),'harness/run.mjs':'// v1\n'});
 const sha=await git(root,'rev-parse','HEAD');
 const early=await commit(root,{'qualification/receipts/docling-library-qualification.json':receipt(sha)},'record');
 assert.deepEqual(await staleReceiptLines(root,early),[]);
 const late=await commit(root,{'harness/run.mjs':'// v2\n'},'change an input');
 const lines=await staleReceiptLines(root,late);
 assert.equal(lines.length,1);
 assert.match(lines[0],/^stale receipt docling-library-qualification\.json: inputs changed after [0-9a-f]{40}: harness\/run\.mjs -- re-run: bun qualification\/docling\/run\.mjs, then bun qualification\/record\.mjs docling$/);
 // Checked out at the earlier commit, the later one is still stale and the earlier one still valid.
 await git(root,'checkout','--quiet','--detach',early);
 assert.deepEqual(await staleReceiptLines(root,early),[]);
 assert.equal((await staleReceiptLines(root,late)).length,1);
 // Checked out before any receipt existed, receipts are still read from the given commit.
 await git(root,'checkout','--quiet','--detach',sha);
 assert.equal((await staleReceiptLines(root,late)).length,1);
 assert.deepEqual(await staleReceiptLines(root,sha),[],'no receipts at that commit');
 await assert.rejects(staleReceiptLines(root,'f'.repeat(40)),/not a commit/);
});
test('staleReceiptLines names a non-ancestor commit and the harness to re-run',async t=>{
 const {root}=await fixtureRepo(t,{'harness/run.mjs':'// v1\n'});
 const head=await commit(root,{'qualification/receipts/mcp-apps-protocol-qualification.json':receipt('0'.repeat(40)),'qualification/receipts/iii-library-qualification.json':receipt('1'.repeat(40))},'foreign');
 const lines=await staleReceiptLines(root,head);
 assert.equal(lines.length,2);
 assert.match(lines.join('\n'),/iii-library-qualification\.json: git_sha 1{40} is not an ancestor of [0-9a-f]{40} -- re-run: bun qualification\/iii\/run\.mjs, then bun qualification\/record\.mjs iii/);
 assert.match(lines.join('\n'),/mcp-apps-protocol-qualification\.json: .* re-run: bun qualification\/mcp-apps\/run\.mjs, then bun qualification\/record\.mjs mcp-apps/);
});
