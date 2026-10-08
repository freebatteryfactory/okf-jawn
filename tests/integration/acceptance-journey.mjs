/**
 * The acceptance journey, independent of generated client code. `call(domain, id, body, agent, expected)`
 * performs one operation and returns its JSON body; the real harness (acceptance.mjs) binds it to fetch,
 * and tests/integration/acceptance-contract.test.mjs binds it to a recorder that validates every request
 * against the generated input schema. Every write carries its own fresh idempotency key.
 */
import assert from 'node:assert/strict';
import { randomUUID,createHash } from 'node:crypto';
import { readResolvedRevision,reviewCoversRevision } from '../support/assertions.mjs';

export async function runJourney(call){
 const key=randomUUID();const workspace=await call('workspaces','create_workspace',{name:`Acceptance ${key}`,description:'Disposable independent test',idempotency_key:key});
 const workspace_id=workspace.id;const properties={our_extension:{kept:true}};
 const item=await call('items','create_item',{workspace_id,base_revision:workspace.head,path:'case.md',title:'Case',type_name:'Note',kind:'note',body:'Proposal is not approved.\n',properties,idempotency_key:randomUUID()});
 const item_id=item.summary.id;const revision=item.summary.revision;
 const read=await call('reads','read_item',{workspace_id,item_id,at:{kind:'revision',revision},view:'text',selection:{kind:'all'},max_bytes:65536,max_images:0},true);
 readResolvedRevision(read,revision);assert.match(read.markdown,/not approved/);
 const content_digest=createHash('sha256').update(read.markdown).digest('hex');
 await call('reviews','create_review',{source:read.source,content_digest,confirmation_id:randomUUID(),idempotency_key:randomUUID()},true,403);
 const confirm=await call('reviews','create_confirmation',{workspace_id,action:'review',target:{kind:'item',item_id},revision,content_digest,idempotency_key:randomUUID()});
 const review=await call('reviews','create_review',{source:read.source,content_digest,confirmation_id:confirm.id,idempotency_key:randomUUID()});reviewCoversRevision(review,revision);
 await call('items','save_draft',{workspace_id,item_id,base_revision:revision,body:'The proposal is now recorded as changed.\n',properties,idempotency_key:randomUUID()});
 const snapshot=await call('history','commit_items',{workspace_id,item_ids:[item_id],message:'Record the change',idempotency_key:randomUUID()});
 assert.notEqual(snapshot.revision,revision,'a Snapshot must create a new revision; saving a draft must not');
 const reviews=await call('reviews','list_reviews',{workspace_id,item_id,at:{kind:'latest'}});
 const prior=reviews.items.find(value=>value.id===review.id);assert.ok(prior);assert.notEqual(prior.coverage,'current','old review must not cover changed text');
 const historical=await call('reads','read_item',{workspace_id,item_id,at:{kind:'revision',revision},view:'text',selection:{kind:'all'},max_bytes:65536,max_images:0},true);readResolvedRevision(historical,revision);assert.equal(historical.markdown,read.markdown);
 const head=(await call('workspaces','open_workspace',{workspace_id})).head;
 const proposal=await call('proposals','open_proposal',{workspace_id,base_revision:head,title:'Allowed draft',description:'Tests useful capability and denied authority together.',changes:[{kind:'create',path:'proposed.md',type_name:'Note',body:'Only a proposal.\n',properties:{}}],idempotency_key:randomUUID()},true);
 assert.equal(proposal.status,'open');
 const approval={workspace_id,proposal_id:proposal.id,expected_head:head,proposal_revision:proposal.proposal_revision,confirmation_id:randomUUID()};
 await call('proposals','accept_proposal',{...approval,idempotency_key:randomUUID()},true,403);
 const acceptedConfirmation=await call('reviews','create_confirmation',{workspace_id,action:'accept_proposal',target:{kind:'proposal',proposal_id:proposal.id},revision:proposal.proposal_revision,content_digest:proposal.content_digest,idempotency_key:randomUUID()});
 await call('proposals','accept_proposal',{...approval,confirmation_id:acceptedConfirmation.id,idempotency_key:randomUUID()});
 const accepted=await call('proposals','get_proposal',{workspace_id,proposal_id:proposal.id});assert.equal(accepted.status,'accepted');
 return {workspace_id};
}
