// Real Chrome + real server/agent. No browser automation dependency is required.
// FELUCCA_PROBE_PROFILE=debug node scripts/probes/probe-admin.mjs
import {spawn,spawnSync} from "node:child_process";
import {createServer,request as httpRequest} from "node:http";
import {createServer as tcpServer} from "node:net";
import {createHash,randomBytes} from "node:crypto";
import {mkdtemp,mkdir,readFile,writeFile,rm} from "node:fs/promises";
import {resolve,join} from "node:path";
import assert from "node:assert/strict";

const root=resolve(import.meta.dirname,"../.."),profile=process.env.FELUCCA_PROBE_PROFILE??"release";
const development=process.env.FELUCCA_PROBE_VITE==="1";
const auditOutput=resolve(root,process.env.FELUCCA_PROBE_OUTPUT??"docs/audit");
await mkdir(auditOutput,{recursive:true});
const serverBin=join(root,"target",profile,"felucca-server"),agentBin=join(root,"target",profile,"felucca-agent");
const chromeBin=process.env.FELUCCA_CHROME??"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
await mkdir(join(root,"target/tmp"),{recursive:true});
const temp=await mkdtemp(join(root,"target/tmp/admin-browser-"));
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const children=[],errors=[],logs=new Map();
let browserSocket,backend,notificationProxy,successful=false,captureDiagnostics;
async function until(check,label,timeout=20000){const deadline=Date.now()+timeout;let last;while(Date.now()<deadline){try{const result=await check();if(result)return result;}catch(error){last=error;}await delay(80);}throw new Error(`Timed out: ${label}${last?`: ${last.message}`:""}`);}
async function port(){const socket=tcpServer();await new Promise(resolve=>socket.listen(0,"127.0.0.1",resolve));const port=socket.address().port;await new Promise(resolve=>socket.close(resolve));return port;}
function command(binary,args,input){const result=spawnSync(binary,args,{input,encoding:"utf8",cwd:root});if(result.status!==0)throw new Error(`${binary} failed: ${result.stderr}`);return result.stdout.trim();}
function start(binary,args,options={}){const child=spawn(binary,args,{cwd:root,stdio:["ignore","pipe","pipe"],...options});children.push(child);logs.set(child,"");for(const stream of [child.stdout,child.stderr])stream.on("data",bytes=>logs.set(child,logs.get(child)+bytes));child.on("error",error=>errors.push(error.message));return child;}
async function stop(child){if(child.exitCode!==null||child.signalCode!==null)return child.exitCode;child.kill("SIGTERM");await until(()=>child.exitCode!==null||child.signalCode!==null,"process shutdown",15000);return child.exitCode;}
try {
  const password="browser-test-management-password",hash=command(serverBin,["--hash-password"],password+"\n");
  const adminPort=await port(),bridgePort=await port(),publicPort=await port();
  backend=createServer((request,response)=>{response.writeHead(200,{"content-type":"text/plain"});response.end("browser-tested-backend\n");});
  await new Promise(resolve=>backend.listen(0,"127.0.0.1",resolve));
  const backendPort=backend.address().port;
  const cert=join(temp,"cert.pem"),key=join(temp,"key.pem");
  const opensslConfig=join(temp,"openssl.cnf");
  await writeFile(opensslConfig,"[req]\ndistinguished_name=dn\nx509_extensions=ext\n[dn]\n[ext]\nsubjectAltName=DNS:browser.test\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n");
  command("/usr/bin/openssl",["req","-x509","-newkey","rsa:2048","-nodes","-sha256","-days","1","-config",opensslConfig,"-subj","/CN=browser.test","-keyout",key,"-out",cert]);
  const vkey=randomBytes(32).toString("hex"),serverPath=join(temp,"server.json"),agentPath=join(temp,"agent.json");
  const serverConfig={
    bridge:{bind:`127.0.0.1:${bridgePort}`,tls:{cert,key}},
    user:{"browser-team":{}},client:{browser:{user:"browser-team",vkey,allow_config_push:false}},
    admin:{bind:`127.0.0.1:${adminPort}`,state_path:join(temp,"runtime.json"),profiling:true,
      account:{root:{role:"admin",password_hash:hash},reader:{role:"viewer",user:"browser-team",password_hash:hash},operator:{role:"operator",user:"browser-team",password_hash:hash}}},
  };
  await writeFile(serverPath,JSON.stringify(serverConfig),{mode:0o600});
  const fingerprint=command(serverBin,["--config",serverPath,"--fingerprint"]);
  await writeFile(agentPath,JSON.stringify({
    server:`127.0.0.1:${bridgePort}`,key_id:"browser",vkey,transport:"tls",
    trust:{mode:"pinned",fingerprint},remote:{enabled:true,target_policy:{ports:[backendPort]}},
    reconnect_min:"50ms",reconnect_max:"100ms",
  }),{mode:0o600});
  let server=start(serverBin,["--config",serverPath]);
  let origin=`http://127.0.0.1:${adminPort}`;
  await until(async()=>{const response=await fetch(origin,{signal:AbortSignal.timeout(1000)});return response.status===200;},"management listener");
  if(development){
    const vitePort=await port();
    start(process.execPath,[join(root,"apps/admin/node_modules/vite/bin/vite.js"),"--host","127.0.0.1","--port",String(vitePort)],{cwd:join(root,"apps/admin"),env:{...process.env,FELUCCA_ADMIN_TARGET:origin}});
    origin=`http://127.0.0.1:${vitePort}`;
    await until(async()=>{const response=await fetch(origin,{signal:AbortSignal.timeout(1000)});return response.status===200;},"Vite development server");
  }
  // Forward real responses through a local proxy so the probe can close just
  // the notification stream. Browser "offline" emulation leaves an existing
  // SSE socket open, while URL blocking permanently fails EventSource.
  const upstreamOrigin=origin,notificationStreams=new Set();
  let pauseNotifications=false;
  notificationProxy=createServer((incoming,outgoing)=>{
    const events=incoming.url==="/api/events";
    if(events&&pauseNotifications){
      outgoing.writeHead(200,{"content-type":"text/event-stream","cache-control":"no-cache"});
      outgoing.end();
      return;
    }
    const url=new URL(incoming.url,upstreamOrigin),headers={...incoming.headers,host:url.host};
    if(headers.origin)headers.origin=url.origin;
    const upstream=httpRequest(url,{method:incoming.method,headers},response=>{
      outgoing.writeHead(response.statusCode,response.headers);
      if(events){notificationStreams.add(outgoing);outgoing.flushHeaders();}
      response.pipe(outgoing);
    });
    upstream.on("error",()=>{if(!outgoing.headersSent)outgoing.writeHead(502);outgoing.end();});
    outgoing.on("close",()=>{notificationStreams.delete(outgoing);upstream.destroy();});
    incoming.pipe(upstream);
  });
  await new Promise(resolve=>notificationProxy.listen(0,"127.0.0.1",resolve));
  origin=`http://127.0.0.1:${notificationProxy.address().port}`;
  const agent=start(agentBin,["--config",agentPath]);
  const chrome=start(chromeBin,["--headless=new","--no-first-run","--no-default-browser-check",
    "--disable-background-networking","--disable-sync",`--user-data-dir=${join(temp,"chrome")}`,
    "--remote-debugging-port=0","about:blank"]);
  const endpoint=await until(()=>logs.get(chrome).match(/DevTools listening on (ws:\/\/[^\s]+)/)?.[1],"Chrome DevTools");
  browserSocket=new WebSocket(endpoint);
  await new Promise((resolve,reject)=>{browserSocket.onopen=resolve;browserSocket.onerror=reject;});
  let sequence=0;
  const pending=new Map();
  browserSocket.onmessage=event=>{
    const message=JSON.parse(event.data);
    if(message.id){const entry=pending.get(message.id);if(!entry)return;pending.delete(message.id);message.error?entry.reject(new Error(JSON.stringify(message.error))):entry.resolve(message.result);}
    if(message.method==="Log.entryAdded"&&message.params.entry.source==="security"&&message.params.entry.level==="error")errors.push(message.params.entry.text);
    if(message.method==="Runtime.exceptionThrown")errors.push(JSON.stringify(message.params.exceptionDetails));
    if(message.method==="Runtime.consoleAPICalled"&&message.params.type==="error")errors.push(JSON.stringify(message.params.args));
  };
  function cdp(method,params={},sessionId){return new Promise((resolve,reject)=>{const id=++sequence;const timer=setTimeout(()=>{pending.delete(id);reject(new Error(`CDP timeout: ${method}`));},15000);pending.set(id,{resolve:value=>{clearTimeout(timer);resolve(value);},reject:error=>{clearTimeout(timer);reject(error);}});browserSocket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));});}
  const downloads=join(temp,"downloads");
  await mkdir(downloads);
  await cdp("Browser.setDownloadBehavior",{behavior:"allow",downloadPath:downloads});
  const {targetId}=await cdp("Target.createTarget",{url:"about:blank"});
  const {sessionId}=await cdp("Target.attachToTarget",{targetId,flatten:true});
  const call=(method,params={})=>cdp(method,params,sessionId);
  await call("Runtime.enable");await call("Page.enable");await call("Log.enable");
  await call("Page.addScriptToEvaluateOnNewDocument",{source:"performance.setResourceTimingBufferSize(4096);"});
  await call("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  async function evaluate(expression){const response=await call("Runtime.evaluate",{expression,awaitPromise:true,returnByValue:true});if(response.exceptionDetails)throw new Error(JSON.stringify(response.exceptionDetails));return response.result.value;}
  const click=text=>evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find(b=>b.textContent===${JSON.stringify(text)}&&!b.disabled);if(!button)throw new Error('button not found');button.click();return true;})()`);
  const waitText=text=>until(()=>evaluate(`document.body.textContent.includes(${JSON.stringify(text)})`),text);
  async function field(label,value){await evaluate(`(()=>{const label=[...document.querySelectorAll('#fields label')].find(l=>l.firstChild.textContent===${JSON.stringify(label)});if(!label)throw new Error('field not found');const input=label.querySelector('input,select,textarea');const prototype=input instanceof HTMLSelectElement?HTMLSelectElement.prototype:input instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(prototype,'value').set.call(input,${JSON.stringify(String(value))});input.dispatchEvent(new Event(input instanceof HTMLSelectElement?'change':'input',{bubbles:true}));})()`);}
  async function save(){await click("保存");await until(()=>evaluate("!document.getElementById('editor')"),"form saved");}
  async function confirm(){await until(()=>evaluate("Boolean(document.getElementById('confirmation'))"),"confirmation");await evaluate("document.querySelector('#confirmation button[value=confirm]').click()");await until(()=>evaluate("!document.getElementById('confirmation')"),"confirmed");}
  const downloaded=name=>until(()=>readFile(join(downloads,name),"utf8"),`${name} downloaded`);
  async function fullScreenshot(name){const {cssContentSize:size}=await call("Page.getLayoutMetrics");const image=await call("Page.captureScreenshot",{format:"png",captureBeyondViewport:true,clip:{x:0,y:0,width:size.width,height:size.height,scale:1}});await writeFile(join(auditOutput,name),Buffer.from(image.data,"base64"));}
  async function screenshot(name){const image=await call("Page.captureScreenshot",{format:"png",captureBeyondViewport:false});await writeFile(join(auditOutput,name),Buffer.from(image.data,"base64"));}
  captureDiagnostics=async()=>{await screenshot("admin-failure.png");await writeFile(join(temp,"page.txt"),await evaluate("document.body.innerText"));};
  async function login(account="root"){
    await until(()=>evaluate("Boolean(document.getElementById('login-form'))"),"login form");
    await evaluate(`document.querySelector('[name=account]').value=${JSON.stringify(account)};document.querySelector('[name=password]').value=${JSON.stringify(password)};document.getElementById('login-form').requestSubmit()`);
    await until(()=>evaluate("Boolean(document.getElementById('workspace')) && !document.querySelector('.loading')"),"login completed");
  }
  async function logout(){await evaluate("document.querySelector('[aria-label=\"退出登录\"]').click()");await until(()=>evaluate("Boolean(document.getElementById('login-form'))"),"logout");}
  // The assertions below are written against the Chinese console. Pin the locale
  // before the bundle boots, so the probe does not follow the browser's own
  // language and only changes language where it means to.
  await call("Page.navigate",{url:origin});
  await evaluate(`window.localStorage.setItem("felucca.locale","zh")`);
  await call("Page.navigate",{url:origin});
  await until(()=>evaluate("document.getElementById('login') && !document.getElementById('login').hidden"),"login page");
  await evaluate("document.fonts.ready");
  await screenshot("admin-login.png");
  await login();
  await call("Page.reload");
  await until(()=>evaluate("Boolean(document.getElementById('workspace')) && !document.querySelector('.loading')"),"cookie session survives browser reload");
  assert(await evaluate("document.fonts.check('16px \"Inter Variable\"')"),"self-hosted font loads under CSP");
  const assetPaths=await evaluate("performance.getEntriesByType('resource').map(r=>new URL(r.name).pathname)");
  assert(assetPaths.some(path=>development?path==="/src/main.jsx":path.startsWith("/assets/index-")&&path.endsWith(".js")),"Vite JavaScript served");
  assert(assetPaths.some(path=>path.endsWith(".woff2")),"embedded font served");
  // The desktop sidebar collapses to an icon rail, and the choice outlives the
  // page: the same console opens collapsed for the operator who left it so.
  await evaluate("document.querySelector('[aria-label=\"收起侧边栏\"]').click()");
  await until(()=>evaluate("document.querySelector('.workspace').classList.contains('sidebar-collapsed')"),"sidebar collapses");
  assert.equal(await evaluate("document.querySelector('.sidebar').getBoundingClientRect().width"),78,"collapsed rail width");
  assert.equal(await evaluate("document.querySelector('.workspace-main').getBoundingClientRect().left"),78,"page follows the rail");
  await call("Emulation.setDeviceMetricsOverride",{width:1100,height:900,deviceScaleFactor:1,mobile:false});
  assert.equal(await evaluate("document.querySelector('.sidebar').getBoundingClientRect().width"),78,"narrow desktop keeps the same rail");
  await call("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.nav-group > p')).display"),"none","rail hides the group headings");
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.nav-label')).display"),"none","rail hides the nav labels");
  assert.equal(await evaluate("document.querySelector('.collapse-toggle').getAttribute('aria-expanded')"),"false","collapsed toggle reports its state");
  assert.equal(await evaluate("document.querySelector('.collapse-toggle').getAttribute('aria-label')"),"展开侧边栏","collapsed toggle offers the way back");
  assert.equal(await evaluate("document.querySelector('.nav-item').getAttribute('aria-label')"),"概览","rail keeps every destination named");
  assert.equal(await evaluate("document.querySelector('.locale-switch select').selectedOptions[0].textContent"),"中文","rail language list keeps full names");
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.locale-switch-name')).display"),"none","rail language picker shows only its icon");
  await click("共享访问");
  await until(()=>evaluate("document.querySelector('.breadcrumb strong').textContent==='共享访问'"),"rail navigation still works");
  await click("概览");
  await until(()=>evaluate("document.querySelector('.breadcrumb strong').textContent==='概览'"),"rail returns to the overview");
  await screenshot("admin-rail.png");
  await call("Page.reload");
  await until(()=>evaluate("Boolean(document.getElementById('workspace')) && !document.querySelector('.loading')"),"reloaded console");
  assert(await evaluate("document.querySelector('.workspace').classList.contains('sidebar-collapsed')"),"collapse survives a reload");
  await evaluate("document.querySelector('[aria-label=\"展开侧边栏\"]').click()");
  await until(()=>evaluate("!document.querySelector('.workspace').classList.contains('sidebar-collapsed')"),"sidebar expands");
  assert.equal(await evaluate("document.querySelector('.sidebar').getBoundingClientRect().width"),238,"expanded sidebar width");
  assert.equal(await evaluate("document.querySelector('.locale-switch select').selectedOptions[0].textContent"),"中文","expanded language picker names the language");
  // Close the real SSE response while keeping ordinary page requests available.
  await until(()=>notificationStreams.size>0,"configuration stream is open");
  pauseNotifications=true;
  for(const stream of notificationStreams)stream.end();
  await waitText("配置变更通知已断开");
  assert(await evaluate("Boolean(document.querySelector('#page-title')) && document.querySelectorAll('.card').length===3"),"last snapshot remains visible without notifications");
  await evaluate("document.querySelector('[aria-label=\"刷新数据\"]').click()");
  await until(()=>evaluate("Boolean(document.querySelector('#page-title')) && !document.querySelector('.loading')"),"manual refresh works without notifications");
  await screenshot("admin-notifications-disconnected.png");
  pauseNotifications=false;
  await until(()=>evaluate("!document.body.innerText.includes('配置变更通知已断开')"),"configuration notifications reconnect",15000);
  await until(()=>notificationStreams.size>0,"configuration stream reopened");
  // Check every management page and every console language at desktop and phone
  // widths, including the empty and unavailable states before resources exist.
  const consoleLocales=["zh","en","ja","ko","fr","es","ru"];
  const consolePages=["overview","clients","tunnels","shares","diagnostics","deployment","users","locals","accounts","config","node","platforms","webhooks"];
  for(const locale of consoleLocales){
    const catalog=JSON.parse(await readFile(join(root,`apps/admin/src/locales/${locale}.json`),"utf8"));
    await evaluate(`(()=>{const select=document.querySelector('.locale-switch select');Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,${JSON.stringify(locale)});select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    for(const page of consolePages){
      await evaluate(`window.location.hash=${JSON.stringify(page)}`);
      const title=catalog[`${["diagnostics","deployment"].includes(page)?"cap":"nav"}.${page}`];
      await until(()=>evaluate(`document.querySelector('#page-title')?.textContent===${JSON.stringify(title)} && !document.querySelector('.loading')`),`${locale} ${page} loads`);
      assert.equal(await evaluate("Boolean(document.querySelector('.eyebrow,.page-footer,.revision-label,.connection-dot'))"),false,"console has no decorative or duplicate status chrome");
      assert(await evaluate("parseFloat(getComputedStyle(document.querySelector('#page-title')).fontSize)<=32"),"page title leaves room for operational data");
      assert(await evaluate("document.documentElement.scrollWidth<=innerWidth"),`${locale} ${page} desktop fits`);
      if(locale==="zh")await screenshot(`admin-page-${page}.png`);
      await call("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});
      await until(()=>evaluate("!document.querySelector('.workspace').classList.contains('sidebar-collapsed')"),"phone uses a drawer");
      assert(await evaluate("parseFloat(getComputedStyle(document.querySelector('#page-title')).fontSize)<=28"),`${locale} ${page} phone title stays compact`);
      assert(await evaluate("document.documentElement.scrollWidth<=innerWidth"),`${locale} ${page} phone fits`);
      await call("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
    }
  }
  await evaluate("(()=>{const select=document.querySelector('.locale-switch select');Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,'zh');select.dispatchEvent(new Event('change',{bubbles:true}));window.location.hash='overview';})()");
  await until(()=>evaluate("document.querySelector('#page-title')?.textContent==='概览'"),"returns to Chinese overview");
  // One listener, many operators: the language follows the request, so the same
  // error must come back in every language the console ships.
  const localized=await evaluate(`(async()=>{
    const listed=await(await fetch('/api/languages')).json();
    const failed=async locale=>(await(await fetch('/api/not-a-resource',{headers:{'X-Felucca-Locale':locale}})).json()).error;
    return {codes:listed.languages.map(language=>language.code),preferred:listed.preferred,
      english:await failed('en'),japanese:await failed('ja'),chinese:await failed('zh')};
  })()`);
  assert.deepEqual(localized.codes,["zh","en","ja","ko","fr","es","ru"],"server languages");
  assert.equal(localized.preferred,"zh","a Chinese browser is answered in Chinese");
  assert.equal(localized.english,"resource not found","English stays the source text");
  assert.notEqual(localized.japanese,"resource not found","Japanese is translated");
  assert.match(localized.japanese,/[\u3040-\u30ff\u4e00-\u9faf]/,"the Japanese message is Japanese");
  assert.notEqual(localized.chinese,"resource not found","Chinese is translated");
  await click("隧道");await waitText("启用集中配置");
  await click("启用集中配置");await evaluate("document.querySelector('#confirmation button[value=confirm]').click()");
  await waitText("新增隧道");
  await click("新增隧道");
  await field("ID",1);await field("服务端公开端口",publicPort);await field("服务端绑定地址","127.0.0.1");
  await evaluate("document.querySelector('#fields .config-collection button').click()");
  await until(()=>evaluate("Boolean(document.querySelector('input[name=\"backend.0.addr\"]'))"),"structured backend row");
  await evaluate(`(()=>{const input=document.querySelector('input[name="backend.0.addr"]');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(`127.0.0.1:${backendPort}`)});input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await evaluate(`(()=>{const input=document.querySelector('input[name="backend.0.weight"]');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,'3');input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await field("备注","浏览器创建的隧道");
  await save();
  async function forwarded(){try{return await (await fetch(`http://127.0.0.1:${publicPort}`,{signal:AbortSignal.timeout(1500)})).text();}catch{return "";}}
  await until(async()=>(await forwarded()).includes("browser-tested-backend"),"UI-created tunnel forwards");
  await click("停用");await until(()=>evaluate("[...document.querySelectorAll('button')].some(b=>b.textContent==='启用')"),"stopped UI");
  await until(async()=>!(await forwarded()),"stopped listener");
  await click("启用");await until(async()=>(await forwarded()).includes("browser-tested-backend"),"resumed tunnel");
  await click("编辑");await field("备注","浏览器修改并持久化");await save();await waitText("浏览器修改并持久化");
  await click("编辑");
  await evaluate("(()=>{const input=[...document.querySelectorAll('#fields label')].find(l=>l.firstChild.textContent==='开放服务端端口').querySelector('input');input.click();})()");
  await save();await until(async()=>!(await forwarded()),"private tunnel closes public port");
  await click("编辑");
  await evaluate("(()=>{const input=[...document.querySelectorAll('#fields label')].find(l=>l.firstChild.textContent==='开放服务端端口').querySelector('input');input.click();})()");
  await field("服务端公开端口",publicPort);await field("服务端绑定地址","127.0.0.1");await save();
  await until(async()=>(await forwarded()).includes("browser-tested-backend"),"public tunnel restored");
  await evaluate("document.querySelector('[aria-label=\"选择隧道 1\"]').click()");
  await click("批量停用");await until(async()=>!(await forwarded()),"batch stop closes listener");
  await waitText("已停用");
  await evaluate("document.querySelector('[aria-label=\"选择隧道 1\"]').click()");
  await click("批量启用");await until(async()=>(await forwarded()).includes("browser-tested-backend"),"batch start restores listener");
  await until(()=>evaluate("Boolean(document.querySelector('[aria-label=\"选择隧道 1\"]'))"),"batch refresh completed");
  await evaluate("document.querySelector('[aria-label=\"刷新数据\"]').click()");
  await waitText("已发布");
  await screenshot("admin-tunnels.png");
  await click("共享访问");await waitText("暂无记录。");await click("新增");
  await field("共享名称","browser-share");await field("提供服务的节点（本机服务填 local.名称）","browser");await field("隧道 ID",1);
  await field("共享密钥（至少 16 字节）","browser-share-test-secret");await field("允许的访客节点（逗号分隔，留空允许持密钥的已认证节点）","browser");await save();
  await waitText("browser-share");await click("编辑");await save();
  await waitText("browser-share");
  // A concurrent writer must not silently rebase the form onto a newer revision.
  await click("编辑");
  await evaluate(`(async()=>{const session=await(await fetch('/api/session')).json();const resource=await(await fetch('/api/clients/browser')).json();const response=await fetch('/api/clients/browser',{method:'PUT',headers:{'Content-Type':'application/json','X-Felucca-CSRF':session.csrf,'If-Match':'"'+resource.revision+'"','Idempotency-Key':crypto.randomUUID()},body:JSON.stringify({...resource.config,remark:'concurrent writer'})});if(!response.ok)throw new Error('external edit failed');})()`);
  await until(()=>evaluate("document.querySelector('.changed')?.textContent.includes('配置已有更新')"),"SSE update banner");
  await click("保存");
  await until(()=>evaluate("document.querySelector('#editor-error')?.textContent.includes('配置已被其他操作更新')"),"stale form rejected");
  await click("取消");await click("刷新数据");await waitText("browser-share");
  const sharesImage=await call("Page.captureScreenshot",{format:"png",captureBeyondViewport:false});
  await writeFile(join(auditOutput,"admin-shares.png"),Buffer.from(sharesImage.data,"base64"));
  await click("配置");await waitText("编辑 JSON");await click("编辑 JSON");await until(()=>evaluate("Boolean(document.querySelector('.config-editor'))"),"configuration editor");
  const exported=await evaluate("JSON.parse(document.querySelector('.config-editor').value)");
  assert.equal(exported.managed.browser.tunnel[0].backend[0].weight,3,"structured backend weight persists");
  assert.equal(exported.client.browser.vkey,"<redacted>");
  assert.equal(exported.managed.browser.tunnel[0].remark,"浏览器修改并持久化");
  assert.equal(exported.admin.account.root.password_hash,"<redacted>");
  assert.equal(exported.share["browser-share"].secret,"<redacted>");
  await click("验证");await waitText("配置校验通过");
  await screenshot("admin-config.png");
  // A startup-only node change can be validated and exported without changing
  // the running daemon. Validate the actual downloaded pair with the real CLI.
  await evaluate(`(()=>{const input=document.querySelector('.config-editor');const draft=JSON.parse(input.value);draft.node={id:'exported-node',state_path:${JSON.stringify(join(temp,"restart-node.json"))}};Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(input,JSON.stringify(draft));input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await click("验证");await waitText("以下修改需要重启进程");
  await click("保存配置");
  assert.equal(await evaluate("Boolean(document.querySelector('#confirmation'))"),false,"startup changes are not presented as a live save");
  await click("导出重启配置包");await confirm();
  await click("felucca-server-restart.json");await click("felucca-runtime-restart.json");
  const startup=JSON.parse(await downloaded("felucca-server-restart.json"));
  const runtime=JSON.parse(await downloaded("felucca-runtime-restart.json"));
  assert.equal(startup.node.id,"exported-node");assert.equal(startup.client.browser.vkey,vkey);
  assert.equal(runtime.config.node.id,"exported-node");
  command(serverBin,["--config",join(downloads,"felucca-server-restart.json"),"--runtime-config",join(downloads,"felucca-runtime-restart.json"),"--check"]);
  assert.equal(await evaluate("(async()=>(await(await fetch('/api/overview')).json()).node)()"),false,"export did not activate node");
  await screenshot("admin-restart-export.png");
  // Upload the fixture identity through the actual file inputs; the returned
  // server paths belong only to the draft until an explicit configuration save.
  await evaluate("document.querySelector('input[aria-label=\"certificate-pem\"]').closest('details').open=true");
  const dom=await call("DOM.getDocument");
  for(const [selector,file] of [['input[aria-label="certificate-pem"]',cert],['input[aria-label="private-key-pem"]',key]]){
    const {nodeId}=await call("DOM.querySelector",{nodeId:dom.root.nodeId,selector});
    await call("DOM.setFileInputFiles",{nodeId,files:[file]});
  }
  await evaluate("(()=>{const input=document.querySelector('input[aria-label=\"certificate-host\"]');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,'bridge');input.dispatchEvent(new Event('input',{bubbles:true}));})()");
  await click("上传证书和私钥");
  await until(()=>evaluate("JSON.parse(document.querySelector('.config-editor').value).bridge.tls.cert.includes('.certificates/')"),"uploaded paths in draft");
  assert.equal(await evaluate("(async()=>(await(await fetch('/api/config')).json()).config.bridge.tls.cert)()"),cert,"upload did not activate identity");
  assert.equal(await evaluate("document.body.textContent.includes('felucca-runtime-restart.json')"),false,"editing invalidates the previously prepared restart download");
  // Token generation and revocation remain normal versioned account writes.
  await click("管理账号");await waitText("reader");
  await evaluate("[...document.querySelectorAll('tr')].find(row=>row.querySelector('strong')?.textContent==='reader').querySelectorAll('button')[1].click()");
  await confirm();
  const readerToken=(await downloaded("felucca-token.txt")).trim();
  assert.equal((await fetch(`${origin}/api/session`,{headers:{Authorization:`Bearer ${readerToken}`}})).status,200);
  await until(()=>evaluate("[...document.querySelectorAll('tr')].find(row=>row.querySelector('strong')?.textContent==='reader')?.querySelectorAll('button')[2]?.disabled===false"),"issued token visible");
  await evaluate("[...document.querySelectorAll('tr')].find(row=>row.querySelector('strong')?.textContent==='reader').querySelectorAll('button')[2].click()");
  await confirm();
  assert.equal((await fetch(`${origin}/api/session`,{headers:{Authorization:`Bearer ${readerToken}`}})).status,401,"revoked token rejected");
  await click("Agent 与部署");await waitText("填入已配置凭据");
  await evaluate("(()=>{const select=[...document.querySelectorAll('main select')].find(s=>[...s.options].some(o=>o.value==='browser-share'));Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(select,'browser-share');select.dispatchEvent(new Event('change',{bubbles:true}));})()");
  await click("填入已配置凭据");await confirm();await click("导出");
  const launch=JSON.parse(await downloaded("felucca-agent.json"));
  assert.equal(launch.vkey,vkey);assert.equal(launch.visitor[0].secret,"browser-share-test-secret");
  assert.equal(launch.trust.fingerprint,fingerprint);
  command(agentBin,["--config",join(downloads,"felucca-agent.json"),"--check"]);
  await screenshot("admin-deployment.png");
  await click("运行诊断");await waitText("后端健康");await click("后端健康");await waitText(`127.0.0.1:${backendPort}`);
  assert(await evaluate("document.body.textContent.includes('未配置主动健康检查')"));
  assert(await evaluate("document.body.textContent.includes('bridge')"),"active bridge certificate shown");
  await evaluate("[...document.querySelectorAll('section')].find(s=>s.textContent.includes('最近的服务日志')).querySelector('button').click()");
  await waitText("未配置日志文件");
  await screenshot("admin-diagnostics.png");
  const protocol=await evaluate("(async()=> (await (await fetch('/api/overview')).json()).protocol)()");
  await click("监控");
  await until(()=>evaluate("document.querySelectorAll('.metric-plot[data-engine=echarts] canvas').length>=4 && Number(document.querySelector('.metric-plot')?.dataset.sampleCount)>=3"),"real host metric charts");
  assert.equal(await evaluate("document.querySelector('main').textContent.includes('Prometheus') || Boolean(document.querySelector('.metrics-editor'))"),false);
  const hostSnapshot=await evaluate("(async()=>await(await fetch('/api/monitor')).json())()");
  assert.equal(hostSnapshot.sample_interval_ms,5000);assert.equal(hostSnapshot.retention_seconds,3600);
  assert(hostSnapshot.samples.at(-1).memory_total_bytes>0);assert(hostSnapshot.samples.at(-1).cpu_percent>=0);
  const point=await evaluate("(()=>{const r=document.querySelector('.metric-plot').getBoundingClientRect();return {x:r.x+68+(r.width-90)*0.87,y:r.y+110};})()");
  await call("Input.dispatchMouseEvent",{type:"mouseMoved",...point});
  const hovered=await evaluate("document.querySelector('.metric-inspector time').dateTime");
  const sample=hostSnapshot.samples.find(s=>new Date(s.timestamp_ms).toISOString()===hovered);
  assert(sample,"hover selects an exact server sample");
  assert((await evaluate("document.querySelector('.metric-head-value').textContent")).includes(sample.cpu_percent.toFixed(1)+"%"));
  const tooltipVisible=()=>evaluate("(()=>{let dark=0;for(const canvas of document.querySelectorAll('.metric-chart:first-child canvas')){const p=canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;for(let i=0;i<p.length;i+=4){if(p[i]===25&&p[i+1]===28&&p[i+2]===31&&p[i+3]>200)dark++;}}return dark>500;})()");
  await until(tooltipVisible,"native ECharts rich-text tooltip is rendered");
  await fullScreenshot("admin-monitor-tooltip.png");
  await call("Input.dispatchMouseEvent",{type:"mousePressed",button:"left",clickCount:1,...point});
  await call("Input.dispatchMouseEvent",{type:"mouseReleased",button:"left",clickCount:1,...point});
  await waitText("已锁定采样点");
  const countBefore=await evaluate("Number(document.querySelector('.metric-plot').dataset.sampleCount)");
  await until(()=>evaluate(`Number(document.querySelector('.metric-plot').dataset.sampleCount)>${countBefore}`),"live polling adds real points",12000);
  assert.equal(await evaluate("document.querySelector('.metric-inspector time').dateTime"),hovered,"polling preserves pinned time");
  assert.equal(await evaluate("new Set([...document.querySelectorAll('.metric-inspector time')].map(t=>t.dateTime)).size"),1,"all charts share a selected time");
  await evaluate("document.querySelector('.metric-plot').focus()");
  await call("Input.dispatchKeyEvent",{type:"keyDown",key:"Home",code:"Home",windowsVirtualKeyCode:36});
  assert.equal(await evaluate("document.querySelector('.metric-inspector time').dateTime"),new Date(hostSnapshot.samples[0].timestamp_ms).toISOString());
  await call("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape",code:"Escape",windowsVirtualKeyCode:27});
  await until(()=>evaluate("!document.querySelector('main').textContent.includes('已锁定采样点')"),"keyboard returns to latest sample");
  await evaluate("document.querySelector('[aria-label=\"CPU放大时间范围\"]').click()");
  await until(()=>evaluate("[...document.querySelectorAll('.metric-plot')].every(p=>p.dataset.zoomed==='true')"),"zoom links all four charts");
  const zoomBounds=await evaluate("[...document.querySelectorAll('.metric-plot')].map(p=>[p.dataset.visibleStart,p.dataset.visibleEnd])");
  assert.equal(new Set(zoomBounds.map(b=>JSON.stringify(b))).size,1);
  const zoomCount=await evaluate("Number(document.querySelector('.metric-plot').dataset.sampleCount)");
  await until(()=>evaluate(`Number(document.querySelector('.metric-plot').dataset.sampleCount)>${zoomCount}`),"sampling continues while zoomed",12000);
  assert.deepEqual(await evaluate("[...document.querySelectorAll('.metric-plot')].map(p=>[p.dataset.visibleStart,p.dataset.visibleEnd])"),zoomBounds,"polling preserves the absolute zoom window");
  await evaluate("document.querySelector('[aria-label=\"CPU重置缩放\"]').click()");
  const slider=await evaluate("(()=>{const r=document.querySelector('.metric-plot').getBoundingClientRect();return {x:r.right-22,y:r.bottom-21,delta:(r.width-90)*0.3};})()");
  await call("Input.dispatchMouseEvent",{type:"mousePressed",button:"left",buttons:1,clickCount:1,x:slider.x,y:slider.y});
  for(let i=1;i<=8;i++)await call("Input.dispatchMouseEvent",{type:"mouseMoved",buttons:1,x:slider.x-slider.delta*i/8,y:slider.y});
  await call("Input.dispatchMouseEvent",{type:"mouseReleased",button:"left",clickCount:1,x:slider.x-slider.delta,y:slider.y});
  await until(()=>evaluate("document.querySelector('.metric-plot').dataset.zoomed==='true'"),"native slider drag changes time range");
  await evaluate("document.querySelector('[aria-label=\"CPU重置缩放\"]').click()");
  await evaluate("document.querySelector('[aria-label=\"网络发送曲线\"]').click()");
  assert.equal(await evaluate("document.querySelector('[aria-label=\"网络发送曲线\"]').getAttribute('aria-pressed')"),"false");
  await evaluate("document.querySelector('[aria-label=\"网络发送曲线\"]').click()");
  await click("5 分钟");await until(()=>evaluate("performance.getEntriesByType('resource').some(e=>e.name.includes('/api/monitor?window=300'))"),"5 minute history request");
  await click("1 小时");await until(()=>evaluate("performance.getEntriesByType('resource').some(e=>e.name.includes('/api/monitor?window=3600'))"),"1 hour history request");
  await click("15 分钟");
  await cdp("Browser.setDownloadBehavior",{behavior:"allow",downloadPath:temp});
  await click("采样 CPU（5 秒）");
  const profileSvg=await until(async()=>{const bytes=await readFile(join(temp,"felucca-cpu.svg"),"utf8");return bytes.includes("<svg")?bytes:false;},"CPU flamegraph download",20000);
  assert(profileSvg.length<8*1024*1024);
  await evaluate("document.querySelector('[aria-label=\"关闭通知\"]')?.click();document.activeElement?.blur()");
  await fullScreenshot("admin-monitor.png");
  await call("Emulation.setDeviceMetricsOverride",{width:390,height:1000,deviceScaleFactor:1,mobile:true});
  await until(()=>evaluate("document.querySelector('.sidebar').getBoundingClientRect().right <= 1"),"mobile monitor drawer transition");
  assert(await evaluate("document.documentElement.scrollWidth <= innerWidth"),"mobile monitor has no page overflow");
  assert(await evaluate("[...document.querySelectorAll('.metric-plot')].every(p=>p.clientWidth>=300 && p.clientHeight>=240)"),"mobile chart keeps readable pixel dimensions");
  await evaluate("document.querySelector('.metric-plot').scrollIntoView({block:'center'})");
  const touchPoint=await evaluate("(()=>{const r=document.querySelector('.metric-plot').getBoundingClientRect();return {x:r.x+68+(r.width-90)*0.8,y:r.y+110};})()");
  await call("Input.dispatchTouchEvent",{type:"touchStart",touchPoints:[{...touchPoint,id:1}]});
  await call("Input.dispatchTouchEvent",{type:"touchEnd",touchPoints:[]});
  await waitText("已锁定采样点");
  await click("返回实时");
  await evaluate("document.activeElement?.blur();window.scrollTo(0,0)");
  await fullScreenshot("admin-monitor-mobile.png");
  await call("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  await click("概览");await until(()=>evaluate("document.querySelectorAll('.card').length===3"),"overview cards");
  const overviewScreenshot=await call("Page.captureScreenshot",{format:"png",captureBeyondViewport:false});
  await writeFile(join(auditOutput,"admin-overview.png"),Buffer.from(overviewScreenshot.data,"base64"));
  await call("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await until(()=>evaluate("document.querySelector('.sidebar').getBoundingClientRect().right <= 1"),"mobile sidebar transition completed");
  assert(await evaluate("document.documentElement.scrollWidth <= innerWidth"),"mobile has no page overflow");
  const mobile=await call("Page.captureScreenshot",{format:"png",captureBeyondViewport:false});
  await writeFile(join(auditOutput,"admin-mobile.png"),Buffer.from(mobile.data,"base64"));
  await evaluate("document.querySelector('[aria-label=\"打开导航\"]').click()");
  assert(await evaluate("document.querySelector('.sidebar').classList.contains('sidebar-open')"),"mobile drawer opens");
  await click("节点");await waitText("concurrent writer");
  assert(await evaluate("!document.querySelector('.sidebar').classList.contains('sidebar-open')"),"navigation closes mobile drawer");
  // A stored collapse is a desktop affordance: on a phone the drawer ignores it,
  // because a rail cannot also be an overlay.
  await call("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  await evaluate("window.localStorage.setItem('felucca.sidebar','collapsed')");
  await call("Page.reload");
  await until(()=>evaluate("Boolean(document.getElementById('workspace')) && !document.querySelector('.loading')"),"console reloads with a stored rail");
  assert(await evaluate("document.querySelector('.workspace').classList.contains('sidebar-collapsed')"),"stored rail is read back");
  await call("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await until(()=>evaluate("document.querySelector('.sidebar').getBoundingClientRect().right <= 1"),"drawer stays closed on a phone");
  assert.equal(await evaluate("document.querySelector('.sidebar').getBoundingClientRect().width"),248,"phone drawer keeps its full width");
  assert.notEqual(await evaluate("getComputedStyle(document.querySelector('.nav-label')).display"),"none","phone drawer labels its destinations");
  assert.equal(await evaluate("document.querySelector('.collapse-toggle').getBoundingClientRect().width"),0,"phone topbar hides the rail toggle");
  await evaluate("window.localStorage.setItem('felucca.sidebar','expanded')");
  await call("Page.reload");
  await until(()=>evaluate("Boolean(document.getElementById('workspace')) && !document.querySelector('.loading')"),"console reloads on a phone");
  assert(await evaluate("!document.querySelector('.workspace').classList.contains('sidebar-collapsed')"),"phone console never rails");
  await call("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  await call("Page.reload");
  await until(()=>evaluate("Boolean(document.getElementById('workspace')) && !document.querySelector('.loading')"),"console returns to the desktop shell");
  assert(await evaluate("!document.querySelector('.workspace').classList.contains('sidebar-collapsed')"),"console returns to the full sidebar");
  await logout();await login("reader");
  assert.equal(await evaluate("[...document.querySelectorAll('nav button')].some(b=>['配置','管理账号','管理平台'].includes(b.textContent))"),false);
  await click("隧道");await waitText("集中配置");
  assert.equal(await evaluate("[...document.querySelectorAll('main button')].some(b=>['编辑','删除','启动','停用','新增隧道'].includes(b.textContent))"),false);
  await click("查看");await until(()=>evaluate("Boolean(document.querySelector('#editor'))"),"readonly editor opens");
  assert.equal(await evaluate("Boolean(document.querySelector('#editor button[type=submit]'))"),false);
  await evaluate("document.querySelector('#editor [aria-label=\"关闭\"]').click()");
  await click("监控");await waitText("仅对管理员开放");
  assert.equal(await evaluate("document.querySelectorAll('.metric-chart').length"),0);
  assert.equal(await evaluate("(async()=>(await fetch('/api/monitor')).status)()"),403);
  await logout();await login("operator");
  await click("隧道");await waitText("新增隧道");
  assert.equal(await evaluate("[...document.querySelectorAll('nav button')].some(b=>['配置','管理账号','管理平台'].includes(b.textContent))"),false);
  await click("监控");await waitText("仅对管理员开放");
  assert.equal(await evaluate("(async()=>(await fetch('/api/monitor')).status)()"),403);
  await logout();await login();
  const serverExit=await stop(server);assert.equal(serverExit,0);
  server=start(serverBin,["--config",serverPath]);
  await until(async()=>(await forwarded()).includes("browser-tested-backend"),"saved UI configuration survives process restart");
  await call("Page.reload");
  await until(()=>evaluate("Boolean(document.getElementById('login'))"),"old browser session invalid after restart");
  const agentExit=await stop(agent),restartExit=await stop(server);
  assert.equal(agentExit,0);assert.equal(restartExit,0);
  // Node identity is a startup setting, so use a separate configuration and
  // persistence file after the first server has stopped.
  const integrationPath=join(temp,"integrations.json");
  await writeFile(integrationPath,JSON.stringify({...serverConfig,
    admin:{...serverConfig.admin,state_path:join(temp,"integrations-runtime.json")},
    node:{id:"browser-node",state_path:join(temp,"node.json"),platform:{"browser-platform":{token:"browser-platform-test-token",scope:"full",mode:"direct"}}},
  }),{mode:0o600});
  const integrationServer=start(serverBin,["--config",integrationPath]);
  await until(async()=>(await fetch(origin,{signal:AbortSignal.timeout(1000)})).status===200,"integration listener");
  await call("Page.reload");await login();
  await click("平台连接");await waitText("browser-platform");
  await waitText("配置变更记录");
  await evaluate(`(async()=>{const session=await(await fetch('/api/session')).json();let resource=await(await fetch('/api/clients/browser')).json();for(let index=0;index<105;index++){const response=await fetch('/api/clients/browser',{method:'PUT',headers:{'Content-Type':'application/json','X-Felucca-CSRF':session.csrf,'If-Match':'"'+resource.revision+'"','Idempotency-Key':crypto.randomUUID()},body:JSON.stringify({...resource.config,remark:'history-'+index})});if(!response.ok)throw new Error('history fixture update failed');resource={config:resource.config,revision:(await response.json()).revision};}})()`);
  await evaluate("document.querySelector('[aria-label=\"刷新数据\"]').click()");
  await waitText("配置变更记录");
  await until(()=>evaluate("[...document.querySelectorAll('section')].find(s=>s.textContent.includes('配置变更记录'))?.querySelectorAll('button')[1]?.disabled===false"),"history has another cursor page");
  const historyText=()=>evaluate("[...document.querySelectorAll('section')].find(s=>s.textContent.includes('配置变更记录')).textContent");
  const firstHistory=await historyText();
  await evaluate("[...document.querySelectorAll('section')].find(s=>s.textContent.includes('配置变更记录')).querySelectorAll('button')[1].click()");
  await until(async()=>(await historyText())!==firstHistory,"history next cursor page");
  await evaluate("[...document.querySelectorAll('section')].find(s=>s.textContent.includes('配置变更记录')).querySelectorAll('button')[0].click()");
  await until(async()=>(await historyText())===firstHistory,"history previous cursor page");
  await screenshot("admin-platform-active.png");
  await click("管理平台");await waitText("平台连接节点");
  await screenshot("admin-platforms-active.png");
  const integrationExit=await stop(integrationServer);assert.equal(integrationExit,0);
  assert.deepEqual(errors,[]);
  const binary=async name=>{const bytes=await readFile(name);return {bytes:bytes.length,sha256:createHash("sha256").update(bytes).digest("hex")};};
  const result={profile,protocol,frontend:development?"React + Vite development proxy":"React + Vite embedded production build",batch_stop_start:true,configuration_validate:true,cookie_reload:true,self_hosted_font:true,stale_form_conflict:true,sse_configuration_notice:true,mobile_drawer:true,mobile_no_page_overflow:true,viewer_readonly:true,operator_navigation:true,monitor_metrics:true,host_cpu_memory_disk_network:true,exact_sample_hover:true,synchronized_charts:true,pinned_time_survives_polling:true,keyboard_sample_navigation:true,history_range_switching:true,host_metrics_admin_only:true,monitor_mobile_no_overflow:true,monitor_mobile_readable_dimensions:true,echarts_native_tooltip:true,native_slider_zoom:true,synchronized_zoom:true,zoom_preserved_during_polling:true,interactive_network_legend:true,csp_no_violations:true,touch_sample_selection:true,cpu_profile_download:true,share_create_edit_redaction:true,private_public_toggle:true,server_languages:localized.codes,localized_errors:true,browser:"Chrome headless via CDP",login:true,managed_tunnel_create_edit_stop_start:true,
    forwarded_backend_verified:true,config_credentials_redacted:true,configuration_survives_process_restart:true,sidebar_rail:true,rail_survives_reload:true,drawer_ignores_stored_rail:true,notification_disconnect_recovery:true,console_copy_review:true,console_layout_locales:consoleLocales,console_layout_pages:consolePages,integration_status_page:true,
    structured_backend_weight:true,restart_package_download_cli_check:true,restart_export_does_not_apply:true,certificate_upload_draft_only:true,account_token_issue_revoke:true,agent_visitor_download_cli_check:true,backend_diagnostics:true,active_certificate_metadata:true,log_unconfigured_state:true,platform_history_cursor_paging:true,
    restart_revokes_cookie:true,javascript_exceptions:errors,sigterm_exit_codes:[serverExit,agentExit,restartExit,integrationExit],
    screenshots:["admin-login.png","admin-rail.png","admin-overview.png","admin-mobile.png","admin-tunnels.png","admin-shares.png","admin-monitor.png","admin-monitor-mobile.png","admin-monitor-tooltip.png","admin-config.png","admin-notifications-disconnected.png","admin-platform-active.png","admin-platforms-active.png","admin-restart-export.png","admin-deployment.png","admin-diagnostics.png",...consolePages.map(page=>`admin-page-${page}.png`)],binaries:{server:await binary(serverBin),agent:await binary(agentBin)}};
  await writeFile(join(auditOutput,"admin-runtime.json"),JSON.stringify(result,null,2)+"\n");
  console.log(JSON.stringify(result,null,2));successful=true;
}finally{
  if(!successful&&captureDiagnostics)await captureDiagnostics().catch(()=>{});
  browserSocket?.close();
  for(const child of children.reverse()){if(child.exitCode===null&&child.signalCode===null){try{await stop(child);}catch{child.kill("SIGKILL");}}}
  if(backend)await new Promise(resolve=>backend.close(resolve));
  if(notificationProxy)await new Promise(resolve=>notificationProxy.close(resolve));
  for(const [i,child]of children.entries())await writeFile(join(temp,`process-${i}.log`),logs.get(child)??"");
  if(successful)await rm(temp,{recursive:true,force:true});
  else console.error(`Diagnostic files retained at ${temp}`);
}
