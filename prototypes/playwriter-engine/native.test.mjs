import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer as createHttp } from 'node:http';
import { createServer as createSocket } from 'node:net';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, chmod, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { chromium } from '@xmorse/playwright-core';
import { connectEngine, FixedTools } from './engine.mjs';

const html=`<!doctype html><title>Single-debugger synthetic fixtures</title><style>body{margin:20px;font:16px sans-serif}.holder{position:relative;width:240px;height:70px;margin:20px}#radio{display:none}label{display:block;padding:20px;background:#eee}#link{display:contents}</style>
<button id="button">Continue</button><a id="link" href="#done"><span>Destination</span></a><div class="holder"><input id="radio" type="radio"><label id="label" for="radio">Account holder</label></div><div id="shadow"></div><div id="slot"></div><p id="result">No action</p><script>
const closed=shadow.attachShadow({mode:'closed'});closed.innerHTML='<button id="closed-button">Closed action</button>';closed.firstChild.onclick=()=>result.textContent='Closed';slot.attachShadow({mode:'open'}).innerHTML='<slot></slot>';slot.innerHTML='<a id="slotted" href="#slotted">Slotted</a>';document.addEventListener('click',e=>{if(e.target.closest('a'))e.preventDefault()});button.onclick=()=>result.textContent='Clicked';radio.onchange=()=>result.textContent='Checked';</script>`;

test('single debugger: native pipe, genuine background tab, unchanged engine',{timeout:60000},async()=>{
 const home=await mkdtemp(join(tmpdir(),'cww-engine-native-'));
 let child,engine,socket;
 const waits=new Map();let next=0;let connected;
 const ready=new Promise(resolve=>connected=resolve);
 const server=createSocket(s=>{
  socket=s;let buffer='';
  s.on('data',chunk=>{buffer+=chunk;let newline;while((newline=buffer.indexOf('\n'))>=0){const message=JSON.parse(buffer.slice(0,newline));buffer=buffer.slice(newline+1);if(message.hello)connected();else{const done=waits.get(message.id);waits.delete(message.id);message.error?done?.reject(Error(message.error)):done?.resolve(message.result)}}});
 });
 const endpoint=join(home,'engine.sock');await new Promise(resolve=>server.listen(endpoint,resolve));await chmod(endpoint,0o600);
 const rpc=(action,params)=>new Promise((resolve,reject)=>{const id=++next;waits.set(id,{resolve,reject});socket.write(JSON.stringify({id,action,params})+'\n')});
 const http=createHttp((req,res)=>res.writeHead(200,{'content-type':'text/html'}).end(html));await new Promise(resolve=>http.listen(0,'127.0.0.1',resolve));
 const url=`http://127.0.0.1:${http.address().port}/`;
 const profile=join(home,'profile');await mkdir(join(profile,'NativeMessagingHosts'),{recursive:true});
 const wrapper=join(home,'host');await writeFile(wrapper,`#!/bin/sh\nCWW_PROTO_SOCKET='${endpoint}' exec /usr/bin/python3 '${resolve('native-host.py')}'\n`,{mode:0o700});
 // Derive the unpacked id exactly as Chromium does for its extension path.
 const {createHash}=await import('node:crypto');const hex=createHash('sha256').update(resolve('extension')).digest('hex').slice(0,32);const id=hex.replace(/[0-9a-f]/g,c=>String.fromCharCode(97+parseInt(c,16)));
 await writeFile(join(profile,'NativeMessagingHosts/com.chatwithwork.engine_prototype.json'),JSON.stringify({name:'com.chatwithwork.engine_prototype',description:'Synthetic engine test',path:wrapper,type:'stdio',allowed_origins:[`chrome-extension://${id}/`]}));
 try {
  child=spawn(process.env.CWW_CHROMIUM || chromium.executablePath(),['--headless=new',`--user-data-dir=${profile}`,`--disable-extensions-except=${resolve('extension')}`,`--load-extension=${resolve('extension')}`,'--use-angle=gl-egl','--use-gl=angle','--no-first-run','--no-default-browser-check','about:blank'],{stdio:['ignore','ignore','ignore']});
  await Promise.race([ready,new Promise((_,reject)=>setTimeout(()=>reject(Error('native host did not attach')),10000).unref())]);
  const owned=await rpc('init',url);
  const bridge={send:command=>rpc('send',command),drain:()=>rpc('drain'),policy:next=>rpc('policy',next)};
  engine=await connectEngine(bridge,home);
  const tools=new FixedTools(engine);tools.approve(true);
  const cdp=(method,params={})=>bridge.send({method,params,sessionId:owned.session});
  await cdp('DOM.enable');
  const evaluate=async expression=>(await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true})).result.value;
  const gpu=await evaluate(`(()=>{const gl=document.createElement('canvas').getContext('webgl');return gl.getParameter(gl.getExtension('WEBGL_debug_renderer_info').UNMASKED_RENDERER_WEBGL)})()`);
  assert.match(gpu,/NVIDIA/);
  const ref=async id=>{const {nodes}=await cdp('DOM.getFlattenedDocument',{depth:-1,pierce:true});const node=nodes.find(n=>n.attributes?.some((name,i)=>i%2===0 && name==='id' && n.attributes[i+1]===id));assert.ok(node,id);return tools.bind(node.backendNodeId)};
  const results=[];
  const scheduler=async()=>{const context=await engine.page.mainFrame()._utilityContext();const injected=await context.injectedScript();return injected.evaluateExpression(`async injected=>{let frames=0,running=true;const tick=()=>{if(running){frames++;injected.utils.builtins.requestAnimationFrame(tick)}};injected.utils.builtins.requestAnimationFrame(tick);await new Promise(done=>setTimeout(done,300));running=false;return {frames,visibility:document.visibilityState,focused:document.hasFocus()}}`,{isFunction:true},undefined)};
  results.push({mode:'default',scheduler:await scheduler()});
  for(const mode of ['default','screencast']) {
    const render=await rpc('render',mode);assert.deepEqual(render.before,render.after);
    for(const [target,method] of [['button','click'],['link','click'],['closed-button','click'],['slotted','click'],['label','check']]) {
      let outcome;try{await tools.call(method,{ref:await ref(target)});outcome='success'}catch(error){outcome='refused';results.push({mode,target,method,outcome,reason:error.message.split('\n')[0],logs:error.actionability});if(mode==='screencast')throw error;continue;}
      results.push({mode,target,method,outcome});
    }
    const bound=await ref('button');
    const timing=await scheduler();
    await new Promise(done=>setTimeout(done,350));
    let idle='success';try{await tools.call('click',{ref:bound})}catch(error){idle='refused';if(mode==='screencast')throw error;}
    results.push({mode,scheduler:timing,idle_bound_click:idle,render});
  }
  console.log(JSON.stringify({gpu,results},null,2));
  if(process.env.CWW_ENGINE_NATIVE_EVIDENCE)await writeFile(process.env.CWW_ENGINE_NATIVE_EVIDENCE,JSON.stringify({gpu,results,transport:await rpc('evidence')},null,2));
 } finally {
  await engine?.close();socket?.destroy();if(child && child.exitCode===null){child.kill('SIGTERM');await new Promise(resolve=>child.once('exit',resolve))}
  await new Promise(resolve=>server.close(resolve));await new Promise(resolve=>http.close(resolve));await rm(home,{recursive:true,force:true});
 }
});
