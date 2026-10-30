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
