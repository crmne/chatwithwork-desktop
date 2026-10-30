import { expect, test } from "@playwright/test";
import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { inWorker, launch, serve, type Setup } from "./harness.js";

const HTML = `<!doctype html><title>Full engine fixture</title><style>body{margin:24px;font:16px sans-serif}button,input,select{margin:12px;padding:12px}label{display:block;padding:20px;background:#eee;width:240px}#radio,#check{position:absolute;opacity:0;width:20px;height:20px}a{display:block;padding:15px}#contents{display:contents}</style>
<button type="button" id="button">Continue</button><a id="contents" href="#contents"><span>Contents link</span></a>
<input type="radio" id="radio"><label for="radio">Holder</label><input type="checkbox" id="check" checked><label for="check">Selected option</label>
<input aria-label="Message" id="text" value="Original"><input aria-label="Password" type="password" id="secret">
<select aria-label="Colour" id="colour"><option value="red">Red</option><option value="blue">Blue</option></select>
<a href="/next" id="nav">Next page</a><div id="shadow"></div><div id="slot"></div><p id="result">Count 0</p>
<script>window.count=0;button.onclick=()=>result.textContent='Count '+(++count);contents.onclick=e=>{e.preventDefault();result.textContent='Contents'};const root=shadow.attachShadow({mode:'closed'});root.innerHTML='<button>Shadow action</button>';root.firstChild.onclick=()=>result.textContent='Shadow';slot.attachShadow({mode:'open'}).innerHTML='<slot></slot>';slot.innerHTML='<button id="slotted">Slotted action</button>';slotted.onclick=()=>result.textContent='Slotted';</script>`;
let setup: Setup, origin: string, server: { close(): void };
test.beforeAll(async()=>{
  ({ origin, server } = await serve({ '/': HTML, '/next': '<h1>Destination</h1>' }));
  setup = await launch([origin]); await setup.daemon.attached(); setup.daemon.setPolicy({ changes: true });
});
test.afterAll(async()=>{ await setup?.cleanup(); server?.close(); });

async function fixture() {
  setup.daemon.setPolicy({ changes: true, stopped: false, sites: [origin] });
  const opened = await setup.daemon.call('open', { url: origin }); expect(opened.error).toBeUndefined();
  const tab = opened.result.tab;
  const page = setup.context.pages().find(p=>p.url() === origin + '/')!;
  const read = await setup.daemon.call('read', { tab }); expect(read.error).toBeUndefined();
  const ref = (name: string) => { const line = read.result.text.split('\n').find((line: string)=>line.includes(`"${name}"`)); const value = /\[ref=(e\d+)\]/.exec(line ?? '')?.[1]; expect(value, name).toBeTruthy(); return value!; };
  return { tab, page, ref };
}
async function action(action: string, params: Record<string,unknown>, beforeCommand?: (command: any) => Promise<void>) {
  const start = await setup.daemon.call('engine_begin', { action, params });
  if (start.error) return start;
  const operation = start.result;
  const child = spawn(process.execPath, [resolve('engine/entry.mjs')], { stdio: ['pipe','pipe','pipe'], env: { PATH: process.env.PATH } });
  let buffer = '', eventIndex = setup.daemon.events.length;
  child.stdin.on('error',()=>{});
  const methods: string[] = [];
  const inFlight = new Set<number>();
  const drain = () => {
    for (const event of setup.daemon.events.slice(eventIndex)) if (event.event === 'engine' && event.lease === operation.lease) child.stdin.write(JSON.stringify({type:'event',message:event.message})+'\n');
    eventIndex = setup.daemon.events.length;
  };
  const timer = setInterval(drain, 5);
  const result: any = await new Promise((done,reject)=>{
    child.stdout.on('data',chunk=>{ buffer+=chunk; let newline; while((newline=buffer.indexOf('\n'))>=0) {
      const message=JSON.parse(buffer.slice(0,newline));buffer=buffer.slice(newline+1);
      if(message.type==='command') {
        methods.push(message.command.method);inFlight.add(message.id);
        void Promise.resolve(beforeCommand?.(message.command)).then(()=>setup.daemon.call('engine_command',{lease:operation.lease,command:message.command})).then(reply=>{
          drain(); inFlight.delete(message.id); child.stdin.write(JSON.stringify({type:'reply',id:message.id,...(reply.error?{error:reply.error}:reply.result)})+'\n');
        }).catch(reject);
      } else if(message.type==='result' || message.type==='error') {
        if(message.type==='result' && inFlight.size) reject(Error('helper completed before command acknowledgements'));
        else done(message);
      }
    }});
    child.once('error',reject); child.once('exit',code=>{if(code)reject(Error(`helper exited ${code}`))});
    child.stdin.write(JSON.stringify({type:'call',operation})+'\n');
  }).finally(()=>{clearInterval(timer);child.stdin.end();});
  if(result.type==='error') { await setup.daemon.call('engine_cancel',{lease:operation.lease}); return {error:result.error, methods}; }
  const finished = await setup.daemon.call('engine_finish',{lease:operation.lease,...result.result});
  return {...finished,methods};
}

for(const name of ['Continue','Contents link','Shadow action','Slotted action']) test(`full engine clicks ${name} without focusing the tab`,async()=>{
 const {tab,page,ref}=await fixture();
 const active=await inWorker(setup.context,async()=> (await chrome.tabs.query({active:true})).map(t=>t.id));
 const result=await action('click',{tab,ref:ref(name),submits:false}); expect(result.error).toBeUndefined();
 expect(result.methods).toContain('Input.dispatchMouseEvent');
 await expect(page.locator('#result')).not.toHaveText('Count 0');
 expect(await inWorker(setup.context,async()=> (await chrome.tabs.query({active:true})).map(t=>t.id))).toEqual(active);
 await page.close();
});
test('styled labels preserve click toggling and preview has no effect',async()=>{
 const {tab,page,ref}=await fixture();
 const preview=await action('click',{tab,ref:ref('Holder'),submits:false,dry_run:true}); expect(preview.error).toBeUndefined(); expect(preview.result.image.data.length).toBeGreaterThan(100);
 await expect(page.locator('#radio')).not.toBeChecked();
 expect((await action('click',{tab,ref:ref('Holder'),submits:false})).error).toBeUndefined(); await expect(page.locator('#radio')).toBeChecked();
 expect((await action('click',{tab,ref:ref('Selected option'),submits:false})).error).toBeUndefined(); await expect(page.locator('#check')).not.toBeChecked();
 await page.close();
});
test('typing and selecting use upstream APIs while protected input refuses',async()=>{
 const {tab,page,ref}=await fixture();
 expect((await action('type',{tab,ref:ref('Message'),text:'Hello'})).error).toBeUndefined(); await expect(page.locator('#text')).toHaveValue('Hello');
 expect((await action('type',{tab,ref:ref('Message'),text:' there',clear:false})).error).toBeUndefined(); await expect(page.locator('#text')).toHaveValue('Hello there');
 expect((await action('select',{tab,ref:ref('Colour'),values:['Blue']})).error).toBeUndefined(); await expect(page.locator('#colour')).toHaveValue('blue');
 expect((await action('type',{tab,ref:ref('Password'),text:'never'})).error?.code).toBe('sensitive_field');
 await page.close();
});
test('real overlays refuse, temporary overlays wait, and unknown options never dispatch',async()=>{
 const {tab,page,ref}=await fixture();
 await page.evaluate(()=>{const cover=document.createElement('div');cover.id='cover';cover.style.cssText='position:fixed;inset:0;background:white;z-index:99';document.body.append(cover)});
 expect((await action('click',{tab,ref:ref('Continue'),submits:false,dry_run:true})).error).toBeTruthy(); await expect(page.locator('#result')).toHaveText('Count 0');
 const run=action('click',{tab,ref:ref('Continue'),submits:false});await page.waitForTimeout(300);await page.locator('#cover').evaluate(el=>el.remove());expect((await run).error).toBeUndefined();await expect(page.locator('#result')).toHaveText('Count 1');
 expect((await action('click',{tab,ref:ref('Continue'),submits:false,force:true})).error?.code).toBe('invalid_argument');
 await page.close();
});
test('navigation settles once and the old document reference expires',async()=>{
 const {tab,page,ref}=await fixture();
 const result=await action('click',{tab,ref:ref('Next page'),submits:false});expect(result.error).toBeUndefined();expect(result.result.url).toBe(origin+'/next');
 expect((await action('click',{tab,ref:ref('Continue'),submits:false})).error).toBeTruthy();await page.close();
});

test('dry-run emits no input to earlier capture listeners',async()=>{
 const {tab,page,ref}=await fixture();
 await page.evaluate(()=>{(window as any).captured=0; for(const name of ['pointermove','mousemove','pointerdown','mousedown','pointerup','mouseup','click']) window.addEventListener(name,()=>{(window as any).captured++},true)});
 const result=await action('click',{tab,ref:ref('Continue'),submits:false,dry_run:true});expect(result.error).toBeUndefined();
 expect(await page.evaluate(()=>(window as any).captured)).toBe(0);await expect(page.locator('#result')).toHaveText('Count 0');await page.close();
});
for(const revoke of ['stopped','changes','sites'] as const) test(`${revoke} revocation interrupts waiting and leaves normal page input usable`,async()=>{
 const {tab,page,ref}=await fixture();
 await page.locator('#button').evaluate(el=>el.setAttribute('disabled',''));
 const waiting=action('click',{tab,ref:ref('Continue'),submits:false});await page.waitForTimeout(300);
 setup.daemon.setPolicy(revoke==='sites'?{sites:[]}:revoke==='stopped'?{stopped:true}:{changes:false});
 expect((await waiting).error).toBeTruthy();
 await page.locator('#button').evaluate(el=>el.removeAttribute('disabled'));
 await page.locator('#button').click();await expect(page.locator('#result')).toHaveText('Count 1');await page.close();
});
test('protected fields and submission classification are checked at actual dispatch',async()=>{
 const {tab,page,ref}=await fixture();let changed=false;
 const typed=await action('type',{tab,ref:ref('Message'),text:'blocked'},async command=>{
  if(!changed && command.method==='Input.insertText') {changed=true;await page.locator('#text').evaluate(el=>el.setAttribute('type','password'))}
 });expect(typed.error).toBeTruthy();await expect(page.locator('#text')).toHaveValue('Original');
 changed=false;let actual=false;
 const clicked=await action('click',{tab,ref:ref('Continue'),submits:false},async command=>{
  if(command.method==='Cww.actionPhase') actual=true;
  if(actual && !changed && command.method==='Input.dispatchMouseEvent' && command.params.type==='mousePressed') {changed=true;await page.locator('#button').evaluate(el=>el.setAttribute('data-method','post'))}
 });expect(clicked.error).toBeTruthy();await expect(page.locator('#result')).toHaveText('Count 0');await page.close();
});
test('associated labels cannot activate a nested other button or a rebound control',async()=>{
 const {tab,page,ref}=await fixture();
 await page.locator('label[for=radio]').evaluate(el=>{el.innerHTML='<button id="other" style="width:100%">Other</button>';el.querySelector('button')!.onclick=()=>document.querySelector('#result')!.textContent='Wrong'});
 expect((await action('click',{tab,ref:ref('Holder'),submits:false})).error).toBeTruthy();await expect(page.locator('#radio')).not.toBeChecked();await expect(page.locator('#result')).toHaveText('Count 0');
 await page.locator('label[for=radio]').evaluate(el=>{el.textContent='Holder';el.addEventListener('mousedown',()=>el.setAttribute('for','check'),{once:true})});
 expect((await action('click',{tab,ref:ref('Holder'),submits:false})).error).toBeTruthy();await expect(page.locator('#radio')).not.toBeChecked();await expect(page.locator('#check')).toBeChecked();await page.close();
});
test('navigation to a blocked origin is an executed action with destination content withheld',async()=>{
 const other=await serve({'/':'<h1>Private destination text</h1>'});
 try {
 const {tab,page,ref}=await fixture();await page.locator('#nav').evaluate((el,url)=>el.setAttribute('href',url),other.origin);
 const result=await action('click',{tab,ref:ref('Next page'),submits:false});expect(result.error).toBeUndefined();expect(result.result.allowed).toBe(false);expect(JSON.stringify(result)).not.toContain('Private destination text');await page.close();
 } finally {other.server.close()}
});
test('tab closed after dispatch returns closed, never a retryable stale ref',async()=>{
 const {tab,page,ref}=await fixture();let actual=false,closed=false;
 const result=await action('click',{tab,ref:ref('Continue'),submits:false},async command=>{
  if(command.method==='Cww.actionPhase') actual=true;
  if(actual && !closed && command.method==='Runtime.callFunctionOn' && (await page.locator('#result').textContent())==='Count 1') {closed=true;await page.close()}
 });expect(result.error).toBeUndefined();expect(result.result.closed).toBe(true);
});
test('focus theft never sends text to a protected control',async()=>{
 const {tab,page,ref}=await fixture();
 await page.locator('#text').evaluate(el=>el.addEventListener('focus',()=>document.querySelector<HTMLInputElement>('#secret')!.focus()));
 expect((await action('type',{tab,ref:ref('Message'),text:'Do not insert'})).error).toBeTruthy();
 await expect(page.locator('#secret')).toHaveValue('');await expect(page.locator('#text')).toHaveValue('Original');await page.close();
});
for(const state of ['disabled','inert']) test(`full engine respects ${state} and waits until the actual control is usable`,async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#button').evaluate((el,state)=>el.setAttribute(state,''),state);
 const run=action('click',{tab,ref:ref('Continue'),submits:false});await page.waitForTimeout(350);await expect(page.locator('#result')).toHaveText('Count 0');
 await page.locator('#button').evaluate((el,state)=>el.removeAttribute(state),state);
 expect((await run).error).toBeUndefined();await expect(page.locator('#result')).toHaveText('Count 1');await page.close();
});
test('revocation after upstream installs its input interceptor cleans that interceptor',async()=>{
 const {tab,page,ref}=await fixture();let actual=false,revoked=false;
 const result=await action('click',{tab,ref:ref('Continue'),submits:false},async command=>{
  if(command.method==='Cww.actionPhase') actual=true;
  if(actual && !revoked && command.method==='Input.dispatchMouseEvent') {revoked=true;setup.daemon.setPolicy({changes:false})}
 });expect(revoked).toBe(true);expect(result.error).toBeTruthy();await expect(page.locator('#result')).toHaveText('Count 0');
 await page.locator('#button').click();await expect(page.locator('#result')).toHaveText('Count 1');await page.close();
});
test('leased transport refuses unrelated sessions, global mutations and parallel admission',async()=>{
 const {tab,page,ref}=await fixture();const params={action:'click',params:{tab,ref:ref('Continue'),submits:false}};
 const replies=await Promise.all([setup.daemon.call('engine_begin',params),setup.daemon.call('engine_begin',params)]);
 expect(replies.filter(r=>r.result)).toHaveLength(1);expect(replies.find(r=>r.error)?.error?.code).toBe('busy');
 const op=replies.find(r=>r.result)!.result;
 for(const command of [{id:1,method:'Browser.close'}, {id:2,method:'Network.clearBrowserCookies',sessionId:op.session}, {id:3,method:'Runtime.evaluate',sessionId:'unrelated',params:{expression:'1'}}]) {
  const reply=await setup.daemon.call('engine_command',{lease:op.lease,command});expect(reply.result.error).toBeTruthy();
 }
 await setup.daemon.call('engine_cancel',{lease:op.lease});await expect(page.locator('#result')).toHaveText('Count 0');await page.close();
});
test('fresh allowed cross-origin navigation completes without disclosing stale events',async()=>{
 const other=await serve({'/':'<h1>Allowed destination</h1>'});
 try {
 const {tab,page,ref}=await fixture();setup.daemon.setPolicy({sites:[origin,other.origin]});await page.locator('#nav').evaluate((el,url)=>el.setAttribute('href',url),other.origin);
 const result=await action('click',{tab,ref:ref('Next page'),submits:false});expect(result.error).toBeUndefined();expect(result.result.url).toBe(other.origin+'/');await page.close();
 }finally{other.server.close()}
});
test('a control hidden after reading uses its native label and still toggles once',async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#check').evaluate(el=>(el as HTMLElement).style.display='none');
 const result=await action('click',{tab,ref:ref('Selected option'),submits:false});expect(result.error).toBeUndefined();await expect(page.locator('#check')).not.toBeChecked();await page.close();
});
for(const nested of ['different label','closed shadow','slotted closed shadow']) test(`an associated label refuses ${nested} as an alternate activation surface`,async()=>{
 const {tab,page,ref}=await fixture();
 await page.locator('label[for=radio]').evaluate((el,nested)=>{
  if(nested==='different label') {el.innerHTML='<label for="check" style="margin:0;padding:0">Other control</label>';return}
  const host=document.createElement('div');host.style.cssText='display:block;padding:10px';const root=host.attachShadow({mode:'closed'});root.innerHTML='<button style="width:100%">Other action</button>';root.querySelector('button')!.onclick=()=>document.querySelector('#result')!.textContent='Wrong';el.replaceChildren(host);
  if(nested==='slotted closed shadow') {const holder=document.createElement('div');el.replaceWith(holder);const shadow=holder.attachShadow({mode:'open'});shadow.innerHTML='<label for="radio"><slot></slot></label>';holder.append(host);shadow.append(document.querySelector('#radio')!)}
 },nested);
 const result=await action('click',{tab,ref:ref('Holder'),submits:false});expect(result.error).toBeTruthy();await expect(page.locator('#check')).toBeChecked();await expect(page.locator('#result')).toHaveText('Count 0');await page.close();
});
test('daemon reconnect invalidates the prior operation even while its native host stays alive',async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#button').evaluate(el=>el.setAttribute('disabled',''));
 const run=action('click',{tab,ref:ref('Continue'),submits:false});await page.waitForTimeout(350);
 setup.daemon.notify({event:'daemon',state:'not_running'});expect((await run).error).toBeTruthy();
 setup.daemon.notify({event:'daemon',state:'connected'});
 await page.locator('#button').evaluate(el=>el.removeAttribute('disabled'));await page.locator('#button').click();await expect(page.locator('#result')).toHaveText('Count 1');await page.close();
});
test('an intercepted actual press is dispatched only once even if upstream retries', async () => {
  const { tab, page, ref } = await fixture();
  await inWorker(setup.context, async (tabId) => {
    const original = chrome.debugger.sendCommand.bind(chrome.debugger);
    (globalThis as any).enginePressSpy = { original, count: 0 };
    chrome.debugger.sendCommand = (async (target: any, method: string, params: any) => {
      if (target.tabId === tabId && method === 'Input.dispatchMouseEvent' && params.type === 'mousePressed') {
        (globalThis as any).enginePressSpy.count++;
        await original(target, 'Runtime.evaluate', { expression: `
          const cover=document.createElement('div');
          cover.style.cssText='position:fixed;inset:0;background:white;z-index:99';
          document.body.append(cover);setTimeout(()=>cover.remove(),100);
        ` });
      }
      return original(target, method, params);
    }) as typeof chrome.debugger.sendCommand;
  }, tab);
  try {
    const result = await action('click', { tab, ref: ref('Continue'), submits: false });
    expect(result.error).toBeTruthy();
    expect(await inWorker(setup.context, () => (globalThis as any).enginePressSpy.count)).toBe(1);
    await expect(page.locator('#result')).toHaveText('Count 0');
  } finally {
    await inWorker(setup.context, () => { chrome.debugger.sendCommand = (globalThis as any).enginePressSpy.original; delete (globalThis as any).enginePressSpy; });
    await page.close();
  }
});

test('terminal success waits for delayed upstream handle-disposal acknowledgements',async()=>{
 const {tab,page,ref}=await fixture();let releases=0;
 const result=await action('type',{tab,ref:ref('Message'),text:'Acknowledged'},async command=>{
  if(command.method==='Runtime.releaseObject') {releases++;await new Promise(done=>setTimeout(done,100))}
 });expect(releases).toBeGreaterThan(0);expect(result.error).toBeUndefined();await expect(page.locator('#text')).toHaveValue('Acknowledged');await page.close();
});
test('select binds value and label so reordered options cannot change the requested choice',async()=>{
 const {tab,page,ref}=await fixture();let reordered=false;
 const result=await action('select',{tab,ref:ref('Colour'),values:['Blue']},async command=>{
  if(!reordered && command.method==='Runtime.callFunctionOn' && String(command.params.arguments?.[3]?.value).includes('.selectOptions(')) {
   reordered=true;await page.locator('#colour').evaluate(el=>el.prepend(el.lastElementChild!));
  }
 });expect(reordered).toBe(true);expect(result.error).toBeUndefined();await expect(page.locator('#colour')).toHaveValue('blue');await page.close();
});
test('navigation during focus never reports that text was entered',async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#text').evaluate(el=>el.addEventListener('focus',()=>location.href='/next'));
 const result=await action('type',{tab,ref:ref('Message'),text:'Never entered'});expect(result.error).toBeTruthy();await expect(page).toHaveURL(origin+'/next');await page.close();
});
test('navigation after completed typing and its requested Enter submission settles normally',async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#text').evaluate(el=>{const form=document.createElement('form');form.action='/next';el.replaceWith(form);form.append(el)});
 const result=await action('type',{tab,ref:ref('Message'),text:'Written before submit',submit:true});expect(result.error).toBeUndefined();expect(result.result.url).toContain(origin+'/next');await page.close();
});
test('engine cancellation after Stop never reattaches the debugger',async()=>{
 const {tab,page,ref}=await fixture();let actual=false,stopped=false;
 await inWorker(setup.context,async()=>{const original=chrome.debugger.attach.bind(chrome.debugger);(globalThis as any).engineAttachSpy={original,stopped:false,afterStop:0};chrome.debugger.attach=((...args:any[])=>{if((globalThis as any).engineAttachSpy.stopped)(globalThis as any).engineAttachSpy.afterStop++;return (original as any)(...args)}) as any});
 try {
 const result=await action('click',{tab,ref:ref('Continue'),submits:false},async command=>{
  if(command.method==='Cww.actionPhase') actual=true;
  if(actual && !stopped && command.method==='Input.dispatchMouseEvent') {stopped=true;await inWorker(setup.context,()=>{(globalThis as any).engineAttachSpy.stopped=true});setup.daemon.setPolicy({stopped:true})}
 });expect(stopped).toBe(true);expect(result.error).toBeTruthy();
 expect(await inWorker(setup.context,()=>(globalThis as any).engineAttachSpy.afterStop)).toBe(0);
 const cancelled=await setup.daemon.call('engine_cancel',{lease:'already-ended'});expect(cancelled.error).toBeUndefined();
 await page.locator('#button').click();await expect(page.locator('#result')).toHaveText('Count 1');
 }finally{await inWorker(setup.context,()=>{chrome.debugger.attach=(globalThis as any).engineAttachSpy.original;delete(globalThis as any).engineAttachSpy});await page.close()}
});

test('admission work consumes the native operation deadline before a lease can dispatch',async()=>{
 const {tab,page,ref}=await fixture();
 await inWorker(setup.context,async()=>{
  const original=chrome.debugger.sendCommand.bind(chrome.debugger);(globalThis as any).admissionSpy={original,delayed:false,input:0};
  chrome.debugger.sendCommand=(async(target:any,method:string,params:any)=>{
   if(method==='Target.getTargetInfo'){(globalThis as any).admissionSpy.delayed=true;await new Promise(done=>setTimeout(done,1100))}
   if(method.startsWith('Input.'))(globalThis as any).admissionSpy.input++;
   return original(target,method,params);
  }) as typeof chrome.debugger.sendCommand;
 });
 try {
  const result=await setup.daemon.call('engine_begin',{action:'click',params:{tab,ref:ref('Continue'),submits:false},timeout_ms:1000});
  expect(result.error?.code).toBe('timeout');
  expect(await inWorker(setup.context,()=>({delayed:(globalThis as any).admissionSpy.delayed,input:(globalThis as any).admissionSpy.input}))).toEqual({delayed:true,input:0});
  await expect(page.locator('#result')).toHaveText('Count 0');
 }finally{await inWorker(setup.context,()=>{chrome.debugger.sendCommand=(globalThis as any).admissionSpy.original;delete(globalThis as any).admissionSpy});await page.close()}
});
