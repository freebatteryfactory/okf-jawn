/** Inspect a real MCP endpoint. This is a wire check, not a rendered-host certification. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const address=process.env['OKF_TEST_MCP_URL'];
if(!address)throw new Error('Set OKF_TEST_MCP_URL to an actual test MCP endpoint; no simulated server is used.');
const endpoint=new URL(address);
if(endpoint.protocol!=='https:' && !(endpoint.protocol==='http:' && ['localhost','127.0.0.1','[::1]'].includes(endpoint.hostname)))throw new Error('Remote qualification requires HTTPS');
let session;let protocol=process.env['OKF_TEST_MCP_PROTOCOL']??'2025-11-25';let id=0;
async function message(method,params,notification=false){
 const requestId=notification?undefined:++id;
 const headers={'content-type':'application/json',accept:'application/json, text/event-stream','MCP-Protocol-Version':protocol};
 if(session)headers['Mcp-Session-Id']=session;
 if(process.env['OKF_TEST_AGENT_TOKEN'])headers.authorization=`Bearer ${process.env['OKF_TEST_AGENT_TOKEN']}`;
 const response=await fetch(endpoint,{method:'POST',headers,redirect:'error',signal:AbortSignal.timeout(30000),body:JSON.stringify({jsonrpc:'2.0',...(notification?{}:{id:requestId}),method,params})});
 if(!response.ok)throw new Error(`MCP ${method}: HTTP ${response.status}`);
 session=response.headers.get('mcp-session-id')??session;
 if(notification){await response.body?.cancel();return undefined;}
 const reader=response.body?.getReader();if(!reader)throw new Error('Missing MCP response body');
 const decoder=new TextDecoder();let buffer='';let bytes=0;
 try{
  while(true){const {value,done}=await reader.read();if(done)break;bytes+=value.byteLength;
   if(bytes>8*1024*1024)throw new Error('MCP control response exceeded 8 MiB');buffer+=decoder.decode(value,{stream:true});
   if(response.headers.get('content-type')?.includes('text/event-stream')){
    buffer=buffer.replaceAll('\r\n','\n');let boundary;
    while((boundary=buffer.indexOf('\n\n'))>=0){const frame=buffer.slice(0,boundary);buffer=buffer.slice(boundary+2);
     const data=frame.split('\n').filter(line=>line.startsWith('data:')).map(line=>line.slice(5).trimStart()).join('\n');
     if(!data)continue;const result=JSON.parse(data);if(result.id===requestId){if(result.error)throw new Error(`MCP ${method}: ${result.error.code}`);return result.result;}
    }
   }
  }
  const result=JSON.parse(buffer+decoder.decode());assert.equal(result.id,requestId);if(result.error)throw new Error(`MCP ${method}: ${result.error.code}`);return result.result;
 }finally{await reader.cancel();}
}
const initialized=await message('initialize',{protocolVersion:protocol,capabilities:{},clientInfo:{name:'okf-jawn-wire-qualification',version:'0.1.0'}});
assert.equal(typeof initialized.protocolVersion,'string');protocol=initialized.protocolVersion;
await message('notifications/initialized',{},true);
const expected=JSON.parse(await readFile(new URL('../../api/mcp-tools.json',import.meta.url),'utf8')).tools;
let tools=[];let cursor;
do{const page=await message('tools/list',cursor?{cursor}:{});tools.push(...page.tools);cursor=page.nextCursor;}while(cursor);
const names=new Set(tools.map(tool=>tool.name));
for(const tool of expected){assert.ok(names.has(tool.name),`Missing declared tool ${tool.name}`);const actual=tools.find(value=>value.name===tool.name);assert.deepEqual(actual.inputSchema,tool.inputSchema,`${tool.name} input schema drift`);}
assert.ok(!names.has('approve')&&!names.has('verify'),'human authority must not be a model tool');
const uris=[...new Set(expected.map(tool=>tool._meta?.ui?.resourceUri).filter(Boolean))];
for(const uri of uris){const result=await message('resources/read',{uri});assert.ok(result.contents.some(value=>value.uri===uri&&typeof value.text==='string'&&value.text.includes('<html')),'UI resource has no actual HTML');}
process.stdout.write(JSON.stringify({status:'passed',scope:'MCP discovery/schema/UI-resource wire only',tool_count:tools.length,ui_resource_count:uris.length,rendered_in_host:false},null,2)+'\n');
