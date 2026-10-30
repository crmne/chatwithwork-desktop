import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { chromium } from '@xmorse/playwright-core';
import { connectEngine, FixedTools } from './engine.mjs';

const html=`<!doctype html><title>Synthetic engine fixtures</title><style>
body{font:16px sans-serif;margin:24px}.control{position:relative;min-height:64px;width:280px;margin:16px}input[type=radio],input[type=checkbox]{width:24px;height:24px}
.cover{position:fixed;inset:0;background:white;z-index:99}label{display:inline-block;padding:16px;background:#eee}
</style><button id="button">Continue</button><a id="link" href="#done"><span>Destination</span></a>
<div class="control"><input id="radio" type="radio"><label id="label" for="radio">Account holder</label></div>
<input id="password" type="password" aria-label="Password"><p id="result">No action</p>
<script>window.actions=0;button.onclick=()=>result.textContent='Actions: '+(++window.actions);radio.onchange=()=>result.textContent='Checked';link.onclick=e=>{e.preventDefault();result.textContent='Linked'}</script>`;

const results=[];
const evidencePath=process.env.CWW_ENGINE_EVIDENCE;

test('unchanged Playwriter engine through extension debugger transport', {timeout:120000}, async()=>{
 const home=await mkdtemp(join(tmpdir(),'cww-engine-prototype-'));
 const server=createServer((req,res)=>res.writeHead(200,{'content-type':'text/html'}).end(html));
 await new Promise(done=>server.listen(0,'127.0.0.1',done));
 const url=`http://127.0.0.1:${server.address().port}/`;
 let context,engine;
 try {
  const extension=resolve('extension');
  context=await chromium.launchPersistentContext(join(home,'profile'),{executablePath:process.env.CWW_CHROMIUM || chromium.executablePath(),headless:true,args:[`--disable-extensions-except=${extension}`,`--load-extension=${extension}`,'--use-angle=gl-egl','--use-gl=angle']});
  const gpu=await context.pages()[0].evaluate(()=>{const gl=document.createElement('canvas').getContext('webgl');return gl.getParameter(gl.getExtension('WEBGL_debug_renderer_info').UNMASKED_RENDERER_WEBGL)});
  assert.match(gpu,/NVIDIA/);console.log('GPU:',gpu);
  const worker=context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  await worker.evaluate(url=>globalThis.bridge.init(url),url);
  engine=await connectEngine(worker,home);
  const render=await worker.evaluate(mode=>bridge.render(mode),process.env.CWW_ENGINE_RENDER_MODE || 'default');
  assert.deepEqual(render.after,render.before,'engine renderer setup must not activate a Chrome tab');
  console.log('Render setup:',JSON.stringify(render));
  const page=context.pages().find(p=>p.url().startsWith(url)) || await context.waitForEvent('page');
  await page.locator('#button').waitFor();
  const cdp=await context.newCDPSession(page);await cdp.send('DOM.enable');
  const tools=new FixedTools(engine);tools.approve(true);
  const ref=async(id)=>{
   const {root}=await cdp.send('DOM.getDocument',{depth:-1,pierce:true});
   const {nodeId}=await cdp.send('DOM.querySelector',{nodeId:root.nodeId,selector:'#'+id});
   const {node}=await cdp.send('DOM.describeNode',{nodeId});return tools.bind(node.backendNodeId);
  };
  const probe=async(name,method,target,expected='success',dry=false)=>{
   try {await tools.call(method,{ref:target,dry_run:dry});results.push({name,method,status:'success'});console.log(name+': success');if(expected)assert.equal(expected,'success',name)}
   catch(error){if(error.code==='ERR_ASSERTION')throw error;results.push({name,method,status:'refused',reason:error.message.split('\n')[0],actionability:error.actionability});console.log(name+': refused',JSON.stringify(error.actionability||error.message));if(expected)assert.equal(expected,'refused',name+': '+error.message)}
  };
  await probe('button preview','click',await ref('button'),'success',true);
  assert.equal(await page.locator('#result').textContent(),'No action');
  await probe('button click','click',await ref('button'));assert.equal(await page.locator('#result').textContent(),'Actions: 1');
  await page.evaluate(()=>link.style.display='contents');
  await probe('display contents link','click',await ref('link'),render.mode==='screencast'?'success':null);
  await page.locator('#link span').evaluate(node=>node.id='link-content');
  await probe('display contents visible child','click',await ref('link-content'));
  await page.evaluate(()=>link.style.display='inline');
  const radio=await ref('radio'),label=await ref('label');
  await page.evaluate(()=>{radio.style.cssText='position:absolute;opacity:0;left:0;top:0';label.style.cssText='position:absolute;inset:0;z-index:2';});
  await probe('styled input direct check','check',radio,null);
  await probe('styled label check via upstream retarget','check',label);assert.equal(await page.locator('#radio').isChecked(),true);
  await page.evaluate(()=>{radio.checked=false;radio.style.display='none'});
  await probe('display none input direct check','check',radio,null);
  await probe('display none label check','check',label);assert.equal(await page.locator('#radio').isChecked(),true);
  const buttonRef=await ref('button');
  await page.evaluate(()=>{const host=document.createElement('div');document.body.prepend(host);host.attachShadow({mode:'closed'}).append(button);});
  await probe('closed shadow target by bound backend handle','click',buttonRef,render.mode==='screencast'?'success':null);
  const linkRef=await ref('link');
  await page.evaluate(()=>{const host=document.createElement('div');document.body.prepend(host);host.attachShadow({mode:'open'}).innerHTML='<slot></slot>';host.append(link);});
  await probe('slotted link','click',linkRef);
  await page.evaluate(()=>{const cover=document.createElement('div');cover.id='cover';cover.className='cover';cover.textContent='Actual overlay';document.body.append(cover)});
  await probe('actual overlay','click',linkRef,'refused');
  await page.locator('#cover').evaluate(node=>node.remove());
  const utility=await engine.page.mainFrame()._utilityContext();
  const injected=await utility.injectedScript();
  const raf=await injected.evaluateExpression(`async injected => { let frames=0;let running=true; const tick=()=>{if(!running)return;frames++;injected.utils.builtins.requestAnimationFrame(tick)};injected.utils.builtins.requestAnimationFrame(tick);await new Promise(resolve=>setTimeout(resolve,300));running=false;return {frames,visibility:document.visibilityState,focus:document.hasFocus()};}`,{isFunction:true},undefined);
  results.push({name:'background engine scheduler in300ms',...raf});console.log('Engine scheduler:',JSON.stringify(raf));
  await page.evaluate(()=>{link.replaceWith(link.cloneNode(true))});
  await probe('stale backend ref after identical replacement','click',linkRef,'refused');
  await probe('protected password','fill',await ref('password'),'refused');
  await assert.rejects(tools.call('evaluate',{code:'document.cookie'}),/unsupported/);
  await assert.rejects(tools.call('click',{ref:label,force:true}),/unsupported/);
  await assert.rejects(worker.evaluate(()=>bridge.send({method:'Network.clearBrowserCookies',sessionId:bridge.evidence().session})),/forbidden/);
  await assert.rejects(worker.evaluate(()=>bridge.send({method:'Runtime.enable',sessionId:'another-tab'})),/unowned session/);
  const currentLink=await ref('link');
  await page.evaluate(()=>{const cover=document.createElement('div');cover.className='cover';document.body.append(cover)});
  const waiting=tools.call('click',{ref:currentLink});
  const stopped=assert.rejects(waiting,/Stop|stopped/);
  await new Promise(resolve=>setTimeout(resolve,100));
  await engine.stop();
  await stopped;
  results.push({name:'Stop during engine auto-wait',status:'refused'});
  console.log(JSON.stringify(results,null,2));
  if(evidencePath)await writeFile(evidencePath,JSON.stringify({gpu,results,transport:await worker.evaluate(()=>bridge.evidence())},null,2));
 } finally {await engine?.close();await context?.close();await new Promise(done=>server.close(done));await rm(home,{recursive:true,force:true});}
});
