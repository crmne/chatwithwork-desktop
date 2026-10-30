// Full, unchanged Playwriter action engine, pinned from npm. Private server
// APIs are confined to this prototype adapter; no copy of its click machinery.
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
const require=createRequire(import.meta.url);
const root=dirname(require.resolve('@xmorse/playwright-core/package.json'));
const {createPlaywright}=require(join(root,'lib/server/playwright.js'));
const {CRBrowser}=require(join(root,'lib/server/chromium/crBrowser.js'));
const {ProgressController}=require(join(root,'lib/server/progress.js'));
const {RecentLogsCollector}=require(join(root,'lib/server/utils/debugLogger.js'));

export async function connectEngine(worker, artifactsDir) {
  let closed=false;
  const transport={
    send(command) {
      worker.evaluate(async command=>({result:await globalThis.bridge.send(command),events:globalThis.bridge.drain()}),command)
        .then(({result,events})=>{for(const event of events) transport.onmessage?.(event);transport.onmessage?.({id:command.id,sessionId:command.sessionId,result})},
          error=>transport.onmessage?.({id:command.id,sessionId:command.sessionId,error:{message:error.message,code:-32000}}));
    },
    close(){closed=true;transport.onclose?.()},
  };
  const pump=(async()=>{
    while(!closed) {
      const events=await worker.evaluate(()=>globalThis.bridge.drain()).catch(()=>[]);
      for(const event of events) transport.onmessage?.(event);
      await new Promise(resolve=>setTimeout(resolve,5));
    }
  })();
  let browser;
  try { browser=await CRBrowser.connect(createPlaywright({sdkLanguage:'javascript'}),transport,{
    name:'chromium',isChromium:true,persistent:{noDefaultViewport:true},protocolLogger:()=>{},
    browserProcess:{close:async()=>transport.close(),kill:async()=>transport.close()},
    browserLogsCollector:new RecentLogsCollector(),
    artifactsDir,downloadsPath:artifactsDir,tracesDir:artifactsDir,originalLaunchOptions:{},
  });
  } catch(error) { transport.close(); await pump; throw error; }
  const page=browser._defaultContext.pages()[0];
  if(!page) {transport.close();await pump;throw Error('engine did not attach to the single owned tab')}
  const controllers=new Set();
  const run=async(task,timeout=2000)=>{
    const controller=new ProgressController(); controllers.add(controller);
    try{return await controller.run(task,timeout)}catch(error){error.actionability=controller.metadata.log.slice(-5);throw error}finally{controllers.delete(controller)}
  };
  const handle=async(backendNodeId)=>page.delegate._mainFrameSession._adoptBackendNodeId(backendNodeId,await page.mainFrame()._utilityContext());
  return {
    browser,page,transport,handle,run,
    async stop(){await worker.evaluate(()=>globalThis.bridge.policy({stopped:true})); await Promise.all([...controllers].map(c=>c.abort(Error('CWW Stop is active'))));},
    async close(){transport.close();await pump},
  };
}

// This fixed adapter cannot accept selectors, source strings or CDP methods.
// The ref table is local and bound to one backend node, preserving CWW's
// stale-ref safety even where a role/id locator could match a replacement.
export class FixedTools {
  #engine; #refs=new Map(); #next=0; #approved=false;
  constructor(engine){this.#engine=engine}
  approve(value){this.#approved=value}
  bind(backendNodeId){const ref='e'+(++this.#next);this.#refs.set(ref,backendNodeId);return ref}
  async call(method, args) {
    if(!['click','check','fill'].includes(method)) throw Error('unsupported fixed tool');
    const keys={click:['ref','dry_run'],check:['ref','dry_run'],fill:['ref','dry_run','text']}[method];
    if(Object.keys(args).some(key=>!keys.includes(key))) throw Error('unsupported argument');
    if(!this.#approved && args.dry_run!==true) throw Error('approval required');
    const id=this.#refs.get(args.ref);if(!id) throw Error('unknown or stale ref');
    const handle=await this.#engine.handle(id);
    try {
      // Fixed trusted policy predicate, never page/model supplied. The
      // production adapter must use existing CWW classification verbatim.
      const facts=await this.#engine.run(progress=>handle.evaluateExpression('node => ({ connected:node.isConnected, type:node.type, autocomplete:node.autocomplete })',{isFunction:true},undefined));
      if(!facts.connected) throw Error('stale ref');
      if(['password','file'].includes(facts.type) || /cc-|one-time-code/.test(facts.autocomplete||'')) throw Error('protected field');
      if(method==='fill' && args.dry_run) return {dry_run:true};
      await this.#engine.run(progress=>method==='click'?handle.click(progress,{trial:args.dry_run===true}):method==='check'?handle.check(progress,{trial:args.dry_run===true}):handle.fill(progress,args.text,{}));
      return {dry_run:args.dry_run===true};
    } finally {await handle.dispose()}
  }
}
