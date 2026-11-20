import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const entry = fileURLToPath(new URL('../entry.mjs', import.meta.url));
async function childTest(input, check) {
  const child=spawn(process.execPath,[entry],{stdio:['pipe','pipe','pipe'],env:{}});
  let raw='',errors='',ready;
  const started=new Promise(resolve=>ready=resolve);
  const messages=[];
  child.stdout.on('data',chunk=>{raw+=chunk;let newline;while((newline=raw.indexOf('\n'))>=0){const value=JSON.parse(raw.slice(0,newline));raw=raw.slice(newline+1);messages.push(value);if(value.type==='ready')ready()}});
  child.stderr.on('data',chunk=>errors+=chunk);
  child.stdin.on('error',()=>{});
  const exited=new Promise(resolve=>child.once('exit',resolve));
  const timer=setTimeout(()=>child.kill(),10000);
  try {await started; child.stdin.end(typeof input==='string'?input:JSON.stringify(input)+'\n');await exited;assert.equal(errors,'');assert.deepEqual(messages[0],{type:'ready',protocol:1});check(messages)}
  finally {clearTimeout(timer);if(child.exitCode===null)child.kill()}
}
for(const input of [{type:'execute',code:'throw new Error("private marker")'},{type:'reply',id:999,result:{}},{type:'call',operation:{action:'evaluate',code:'private marker'}}]) test('private protocol refuses unsupported inputs without echoing contents',async()=>{
 await childTest(input,messages=>{assert.equal(messages.at(-1).type,'error');assert.equal(messages.at(-1).error.code,'invalid_argument');assert.ok(!JSON.stringify(messages).includes('private marker'));assert.ok(!messages.some(m=>m.type==='command'))});
});
test('oversized and malformed frames cannot start the browser engine',async()=>{
 for(const input of ['{broken\n','x'.repeat(2*1024*1024+1)]) await childTest(input,messages=>{assert.equal(messages.at(-1).type,'error');assert.ok(!messages.some(m=>m.type==='command'))});
});
test('cancel before a call exits without issuing a browser command',async()=>{
 await childTest({type:'cancel'},messages=>{assert.equal(messages.at(-1).error.code,'browser_stopped');assert.ok(!messages.some(m=>m.type==='command'))});
});
// #262: the extension cancelling mid-call (its own deadline, a navigation
// before input) must end the helper with a bare error, never a result, so the
// daemon asks engine_cancel how the operation ended. No page text comes back.
test('the extension cancelling during a call ends it with a bare error the daemon can ask about',async()=>{
 const child=spawn(process.execPath,[entry],{stdio:['pipe','pipe','pipe'],env:{}});
 const messages=[];let raw='',cancelled=false;
 const timer=setTimeout(()=>child.kill(),10000);
 child.stdin.on('error',()=>{});
 const ended=new Promise(resolve=>child.stdout.on('data',chunk=>{raw+=chunk;let newline;while((newline=raw.indexOf('\n'))>=0){
  const value=JSON.parse(raw.slice(0,newline));raw=raw.slice(newline+1);messages.push(value);
  if(value.type==='ready') child.stdin.write(JSON.stringify({type:'call',operation:{action:'click',backendNodeId:7,session:'s',params:{tab:1,ref:'e7',submits:false},labelBackendNodeIds:[]}})+'\n');
  else if(value.type==='command'&&!cancelled){cancelled=true;child.stdin.write(JSON.stringify({type:'event',message:{method:'Cww.operationCancelled',params:{secret:'private marker'}}})+'\n');}
  else if(value.type==='error'||value.type==='result') resolve(value);
 }}));
 try {
  const last=await ended;
  assert.ok(cancelled);
  assert.equal(last.type,'error');
  assert.ok(typeof last.error.code==='string');
  assert.ok(!JSON.stringify(messages).includes('private marker'));
 } finally {clearTimeout(timer);child.stdin.end();if(child.exitCode===null)child.kill()}
});
