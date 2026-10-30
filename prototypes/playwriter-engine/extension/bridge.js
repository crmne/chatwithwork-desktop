// Prototype-only transport. No listener, runtime.onMessage, externally
// connectable route or native host. The synthetic test driver calls this
// object through its private browser pipe. A product sidecar would receive
// only daemon-owned messages on its inherited pipe, never model CDP/JS.
// Session mapping follows Playwriter's MIT relay: one attached tab is one
// synthetic CDP root session. The engine itself remains unmodified.
let tab, group, origin, info, session, stopped = false, changes = true;
const events = [];
const methods = new Set();
let renderEvidence;
const globalForbidden = /^(Browser\.(close|crash|setPermission|grantPermissions|resetPermissions)|Target\.(createBrowserContext|disposeBrowserContext|createTarget|closeTarget)|Network\.(clearBrowserCookies|clearBrowserCache|setCookie|setCookies|deleteCookies))/;
chrome.debugger.onEvent.addListener((source, method, params) => {
  if(source.tabId===tab && method==='Page.screencastFrame') {void chrome.debugger.sendCommand({tabId:tab},'Page.screencastFrameAck',{sessionId:params.sessionId});return;}
  if (source.tabId === tab) events.push({method,params,sessionId:source.sessionId || session});
});
globalThis.bridge = {
  async init(url) {
    origin = new URL(url).origin;
    const windows = await chrome.windows.getAll({windowTypes:['normal']});
    const created = await chrome.tabs.create({windowId:windows[0].id,url:'about:blank',active:false});
    tab = created.id;
    group = await chrome.tabs.group({tabIds:[tab]});
    await chrome.tabGroups.update(group,{title:'Chat with Work prototype'});
    await chrome.tabs.update(tab,{url});
    const deadline=Date.now()+10000;
    while(true){const current=await chrome.tabs.get(tab);if(current.status==='complete' && current.url===url)break;if(Date.now()>deadline)throw Error('fixture navigation timed out');await new Promise(done=>setTimeout(done,20));}
    await chrome.debugger.attach({tabId:tab},'1.3');
    ({targetInfo:info} = await chrome.debugger.sendCommand({tabId:tab},'Target.getTargetInfo'));
    session = 'cww-prototype-'+tab;
    return {tab,group,session};
  },
  async send(command) {
    const {method,params={},sessionId}=command;
    methods.add(method);
    if(stopped) throw Error('CWW Stop is active');
    const current = await chrome.tabs.get(tab);
    if(current.groupId!==group) throw Error('tab no longer belongs to CWW');
    if(new URL(current.url || '').origin!==origin) throw Error('site is no longer allowed');
    if(globalForbidden.test(method)) throw Error('global browser mutation is forbidden');
    if(!changes && (/^Input\./.test(method) || /^Runtime\.(evaluate|callFunctionOn)/.test(method))) throw Error('browser actions revoked');
    if(!sessionId) {
      if(method==='Browser.getVersion') return {protocolVersion:'1.3',product:'Chrome/'+(navigator.userAgent.match(/(?:Headless)?Chrome\/([\d.]+)/)?.[1]||'0'),revision:'',userAgent:navigator.userAgent,jsVersion:''};
      if(method==='Target.setAutoAttach') {
        events.push({method:'Target.attachedToTarget',params:{sessionId:session,targetInfo:{...info,attached:true},waitingForDebugger:false}});
        return {};
      }
      if(method==='Target.getTargetInfo') {
        if(params.targetId && params.targetId!==info.targetId) throw Error('unowned target');
        return {targetInfo:info};
      }
      if(method==='Target.getTargets') return {targetInfos:[info]};
      if(method==='Target.getBrowserContexts') return {browserContextIds:[]};
      if(method==='Target.setDiscoverTargets') return {};
      // The prototype has no download support. Never change the user's
      // browser-global download directory/permissions as engine setup.
      if(method==='Browser.setDownloadBehavior') return {};
      throw Error('unsupported root command '+method);
    }
    if(sessionId!==session) throw Error('unowned session');
    if(method==='Page.bringToFront') return {}; // never focus a user surface
    return chrome.debugger.sendCommand({tabId:tab},method,params);
  },
  async render(mode) {
    const before=(await chrome.tabs.query({active:true})).map(tab=>tab.id);
    if(mode==='front') await chrome.debugger.sendCommand({tabId:tab},'Page.bringToFront');
    if(mode==='screencast') await chrome.debugger.sendCommand({tabId:tab},'Page.startScreencast',{format:'jpeg',quality:1,maxWidth:8,maxHeight:8,everyNthFrame:1});
    const after=(await chrome.tabs.query({active:true})).map(tab=>tab.id);
    renderEvidence={mode,before,after};return renderEvidence;
  },
  drain(){return events.splice(0)},
  policy(next){if('stopped'in next) stopped=next.stopped; if('changes'in next) changes=next.changes; if('origin'in next) origin=next.origin},
  evidence(){return {tab,group,session,renderEvidence,methods:[...methods].sort()}},
};

// Synthetic-only native transport to prove behavior without a second CDP
// connection. Its host exists solely in the throwaway test profile. This is
// not a product entry point and no script/evaluate action is accepted here.
const native = chrome.runtime.connectNative('com.chatwithwork.engine_prototype');
native.onDisconnect.addListener(()=>void chrome.runtime.lastError);
native.onMessage.addListener(async ({id,action,params})=>{
  try {
    if(!['init','send','drain','policy','render','evidence'].includes(action)) throw Error('unsupported fixture operation');
    native.postMessage({id,result:await globalThis.bridge[action](params)});
  } catch(error) {native.postMessage({id,error:error.message})}
});
native.postMessage({hello:true});
