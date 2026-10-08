/** Exercise a real disposable deployment independently of generated client code. */
import assert from 'node:assert/strict';
import { runJourney } from './acceptance-journey.mjs';

if(process.env['OKF_TEST_DISPOSABLE']!=='1')throw new Error('This test creates and changes data. Set OKF_TEST_DISPOSABLE=1 only for a disposable test deployment.');
for(const name of ['OKF_TEST_BASE_URL','OKF_TEST_BROWSER_COOKIE','OKF_TEST_CSRF','OKF_TEST_AGENT_TOKEN'])if(!process.env[name])throw new Error(`Missing ${name}`);
const base=new URL(process.env['OKF_TEST_BASE_URL']);
const local=['localhost','127.0.0.1','[::1]'].includes(base.hostname);
if(!local && (base.protocol!=='https:'||process.env['OKF_TEST_ALLOW_REMOTE']!=='1'))throw new Error('Remote test needs HTTPS and OKF_TEST_ALLOW_REMOTE=1');
async function call(domain,id,body,agent=false,expected=200){
 const headers={'content-type':'application/json'};
 if(agent)headers.authorization=`Bearer ${process.env['OKF_TEST_AGENT_TOKEN']}`;
 else{headers.cookie=process.env['OKF_TEST_BROWSER_COOKIE'];headers['x-csrf-token']=process.env['OKF_TEST_CSRF'];headers.origin=base.origin;}
 const response=await fetch(new URL(`/api/${domain}/${id.replaceAll('_','-')}`,base),{method:'POST',headers,redirect:'error',signal:AbortSignal.timeout(30000),body:JSON.stringify(body)});
 assert.equal(response.status,expected,`${id} status`);return response.json();
}
const {workspace_id}=await runJourney(call);
process.stdout.write(JSON.stringify({status:'passed',scope:'real authorization/proposal/review/pinned-read journey',workspace_id,not_covered:['converter fidelity','export/backup restoration','job crash/restart recovery','host rendering','complete human UI']},null,2)+'\n');
