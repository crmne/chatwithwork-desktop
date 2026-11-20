import { expect, test } from "@playwright/test";
import { engineAction, inWorker, launch, serve, type Setup } from "./harness.js";

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
const action = (name: string, params: Record<string,unknown>, beforeCommand?: (command: any) => Promise<void>) => engineAction(setup, name, params, beforeCommand);

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
 const interrupted=await waiting;expect(interrupted.error).toBeTruthy();
 // Nothing was sent; after a revocation the extension doesn't look at the page to say why.
 expect(interrupted.outcome).toEqual({dispatched:false,reason:revoke==='stopped'?'the browser was stopped':revoke==='changes'?'changes were turned off before anything was sent':'the site was no longer allowed before anything was sent'});
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

// #262: how an operation the engine couldn't finish ended, as engine_cancel
// answers it: certain when nothing left, observations when something did.
test('an operation that gave up before any input says nothing was sent, and why',async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#button').evaluate(el=>el.setAttribute('disabled',''));
 try {
  const run=await engineAction(setup,'click',{tab,ref:ref('Continue'),submits:false},undefined,1500);
  expect(run.error).toBeDefined();
  expect(run.outcome).toEqual({dispatched:false,reason:"the element didn't become ready in time (that element is disabled)"});
  expect(run.methods.some((method:string)=>method.startsWith('Input.'))).toBe(false);
  await expect(page.locator('#result')).toHaveText('Count 0');
 } finally { await page.close(); }
});

test('an operation that stopped after input left says what the page looks like',async()=>{
 const {tab,page,ref}=await fixture();
 let acting=false;
 try {
  // Hold the real release (after the trial) past the operation's deadline.
  const run=await engineAction(setup,'click',{tab,ref:ref('Continue'),submits:false},async command=>{
   if(command.method==='Cww.actionPhase') acting=true;
   if(acting&&command.method==='Input.dispatchMouseEvent'&&command.params?.type==='mouseReleased') await new Promise(done=>setTimeout(done,10000));
  },8000);
  expect(run.error).toBeDefined();
  expect(run.outcome).toEqual({dispatched:true,complete:false,observed:{responding:true,navigated:false,address_changed:false,element_present:true}});
 } finally { await page.close(); }
});

test('an operation that stopped after its click opened a dialog says so',async()=>{
 const {tab,page,ref}=await fixture();
 await page.evaluate(()=>{(document.querySelector('#button') as HTMLElement).onmousedown=()=>{alert('Saved')};});
 let acting=false;
 try {
  const run=await engineAction(setup,'click',{tab,ref:ref('Continue'),submits:false},async command=>{
   if(command.method==='Cww.actionPhase') acting=true;
   if(acting&&command.method==='Input.dispatchMouseEvent'&&command.params?.type==='mouseReleased') await new Promise(done=>setTimeout(done,10000));
  },8000);
  expect(run.error).toBeDefined();
  expect(run.outcome).toEqual({dispatched:true,complete:false,observed:{responding:true,navigated:false,address_changed:false,dialog:true,element_present:true}});
 } finally { await page.close(); }
});

// The outcome's checks stop the moment access is revoked or their time runs
// out: no page command after, and no reason read from the page. The
// extension's own element-state probe (it says "that element is disabled")
// is held to make the moment; engine_begin's own use of it, at the start,
// is let through.
async function holdStateProbe(ms: number): Promise<void> {
 await inWorker(setup.context,(ms:number)=>{const g=globalThis as any;g.probeSpy={log:[],held:false,armedAt:Date.now(),original:chrome.debugger.sendCommand};
  chrome.debugger.sendCommand=(async(target:any,method:string,params:any)=>{
   g.probeSpy.log.push({at:Date.now(),method,detail:String(params?.expression??params?.functionDeclaration??'').slice(0,200)});
   if(method==='Runtime.callFunctionOn'&&!g.probeSpy.held&&Date.now()-g.probeSpy.armedAt>1000&&String(params?.functionDeclaration??'').includes('that element is disabled')){g.probeSpy.held=true;g.probeSpy.heldAt=Date.now();await new Promise(done=>setTimeout(done,ms));}
   return g.probeSpy.original(target,method,params);
  }) as typeof chrome.debugger.sendCommand;},ms);
}
// Every command sent after `since`, except the operation's own cleanup:
// removing its guard (a Runtime.evaluate naming it, with the extension's own
// isolated world set up just before it for that), released objects and
// stopped interceptors. None of those reads the page.
const sentAfter=(log:{at:number,method:string,detail:string}[],since:number)=>{
 const cleanup=(entry:{method:string,detail:string})=>entry.method==='Runtime.releaseObject'
  || (entry.method==='Runtime.evaluate'&&entry.detail.includes('__cwwEngine'))
  || (entry.method==='Runtime.callFunctionOn'&&entry.detail.includes('this.stop()'));
 const prelude=(method:string)=>method==='Page.getFrameTree'||method==='Page.createIsolatedWorld';
 return log.filter((entry,index)=>{
  if(entry.at<=since||cleanup(entry)) return false;
  if(!prelude(entry.method)) return true;
  const next=log.slice(index+1).find(other=>!prelude(other.method));
  return !(next&&next.method==='Runtime.evaluate'&&next.detail.includes('__cwwEngine'));
 }).map(entry=>entry.method);
};
async function releaseStateProbe(): Promise<{log:{at:number,method:string,detail:string}[],held:boolean,heldAt:number}> {
 return inWorker(setup.context,()=>{const g=globalThis as any;chrome.debugger.sendCommand=g.probeSpy.original;const {log,held,heldAt}=g.probeSpy;delete g.probeSpy;return {log,held,heldAt};});
}

test('site access revoked during the outcome check stops it, and no page reason comes back',async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#button').evaluate(el=>el.setAttribute('disabled',''));
 await holdStateProbe(500);
 try {
  const running=engineAction(setup,'click',{tab,ref:ref('Continue'),submits:false},undefined,1500);
  let revokedAt=0;
  for(let i=0;i<200&&!revokedAt;i++){
   if(await inWorker(setup.context,()=>(globalThis as any).probeSpy?.held===true)){setup.daemon.setPolicy({sites:[]});revokedAt=await inWorker(setup.context,()=>Date.now());}
   else await page.waitForTimeout(25);
  }
  const run=await running;
  const spy=await releaseStateProbe();
  expect(spy.held).toBe(true);
  expect(run.outcome).toEqual({dispatched:false,reason:'the site was no longer allowed before anything was sent'});
  expect(sentAfter(spy.log,revokedAt)).toEqual([]);
 } finally { await releaseStateProbe().catch(()=>{}); await page.close(); }
});

test('an outcome check past its own deadline stops, and no page reason comes back',async()=>{
 const {tab,page,ref}=await fixture();await page.locator('#button').evaluate(el=>el.setAttribute('disabled',''));
 await holdStateProbe(1200);
 try {
  const run=await engineAction(setup,'click',{tab,ref:ref('Continue'),submits:false},undefined,1500);
  // Wait out the held probe, then look at what was sent after it.
  await page.waitForTimeout(1300);
  const spy=await releaseStateProbe();
  expect(spy.held).toBe(true);
  expect(run.outcome).toEqual({dispatched:false,reason:"the element didn't become ready in time (it may still have been moving or loading)"});
  expect(sentAfter(spy.log,spy.heldAt+1200)).toEqual([]);
 } finally { await releaseStateProbe().catch(()=>{}); await page.close(); }
});
