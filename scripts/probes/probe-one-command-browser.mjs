// Real browser actions and the exact copied commands against an empty server.
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import assert from 'node:assert/strict';

const root = resolve(import.meta.dirname, '../..');
const output = resolve(root, process.env.FELUCCA_PROBE_OUTPUT ?? 'target/tmp/one-command-browser');
await mkdir(output, { recursive: true, mode: 0o700 });
const children = [], logs = new Map(), errors = [], checks = [];
const delay = ms => new Promise(r => setTimeout(r, ms));
let socket, evaluate, call;
async function until(check, label, timeout = 30000) {
  const end = Date.now() + timeout; let last;
  while (Date.now() < end) { try { const value = await check(); if (value) return value; } catch (error) { last = error; } await delay(100); }
  throw new Error(`${label}: ${last?.message ?? 'timeout'}`);
}
function start(binary, args, name, env = {}) {
  const child = spawn(binary, args, { cwd: root, detached: true, env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'] });
  children.push(child); logs.set(child, { name, text: '' });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', bytes => { logs.get(child).text += bytes; });
  child.on('error', error => { logs.get(child).text += error.message; });
  return child;
}
const report = { checks, errors, limits: ['Native loopback and headless Chrome only; no Windows, Gatekeeper or public NAT acceptance.'] };
try {
  const fixtureDir = join(output, 'fixture');
  start(process.execPath, ['scripts/probes/probe-product-flow.mjs', '--interactive', '--one-command'], 'fixture', { FELUCCA_PROBE_OUTPUT: fixtureDir });
  const fixture = await until(async () => JSON.parse(await readFile(join(fixtureDir, 'fixture.json'), 'utf8')), 'fixture');
  const origin = fixture.origin;
  const chrome = start(process.env.FELUCCA_CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-sync',
    `--user-data-dir=${join(output, 'chrome')}`, '--remote-debugging-port=0', 'about:blank',
  ], 'chrome');
  const endpoint = await until(() => logs.get(chrome).text.match(/DevTools listening on (ws:\/\/[^\s]+)/)?.[1], 'Chrome');
  socket = new WebSocket(endpoint);
  await new Promise((r, reject) => { socket.onopen = r; socket.onerror = reject; });
  let sequence = 0;
  const pending = new Map();
  socket.onmessage = event => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) { const task = pending.get(message.id); pending.delete(message.id); message.error ? task.reject(new Error(JSON.stringify(message.error))) : task.resolve(message.result); }
    if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails.text);
  };
  function cdp(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++sequence, timeout = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 15000);
      pending.set(id, { resolve: value => { clearTimeout(timeout); resolve(value); }, reject: error => { clearTimeout(timeout); reject(error); } });
      socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
  const { targetId } = await cdp('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await cdp('Target.attachToTarget', { targetId, flatten: true });
  call = (method, params) => cdp(method, params, sessionId);
  evaluate = async expression => {
    const value = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (value.exceptionDetails) throw new Error(value.exceptionDetails.exception?.description ?? value.exceptionDetails.text);
    return value.result.value;
  };
  const click = (text, selector = 'button') => evaluate(`(() => { const button = [...document.querySelectorAll(${JSON.stringify(selector)})].find(b => b.textContent.trim() === ${JSON.stringify(text)} && !b.disabled); if (!button) throw new Error('Button missing'); button.click(); })()`);
  const text = value => until(() => evaluate(`document.body.innerText.includes(${JSON.stringify(value)})`), value);
  async function fill(selector, value) {
    await evaluate(`(() => { const input = document.querySelector(${JSON.stringify(selector)}); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(String(value))}); input.dispatchEvent(new Event('input', {bubbles:true})); })()`);
  }
  async function screenshot(name) {
    await evaluate(`document.querySelectorAll('input[readonly]').forEach(input => { if (input.value.startsWith('curl ') || input.value.startsWith('& (')) { input.type = 'password'; } })`);
    const shot = await call('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(output, name), Buffer.from(shot.data, 'base64'));
  }
  await call('Page.enable'); await call('Runtime.enable');
  await call('Page.addScriptToEvaluateOnNewDocument', { source: "localStorage.setItem('felucca.locale', 'zh')" });
  await call('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await call('Page.navigate', { url: origin });
  await until(() => evaluate("Boolean(document.querySelector('#login-form'))"), 'login');
  await evaluate(`document.querySelector('[name=account]').value='root'; document.querySelector('[name=password]').value=${JSON.stringify(fixture.password)}; document.querySelector('#login-form').requestSubmit()`);
  await until(() => evaluate("Boolean(document.querySelector('#workspace'))"), 'workspace');
  await evaluate("location.hash='deployment'");
  await until(() => evaluate("Boolean(document.querySelector('[name=deviceName]'))"), 'connect form');
  assert(await evaluate("document.querySelector('details.advanced')?.open === false"), 'permissions must start collapsed');
  assert(await evaluate("Boolean(document.querySelector('.scope-field input[value=any]:checked'))"), 'new device must allow all networks');
  await screenshot('default-network-access.png');
  await fill('[name=deviceName]', '浏览器一行命令设备');
  await evaluate("document.querySelector('[name=deviceName]').form.requestSubmit()");
  await text('一行命令接入设备');
  const provider = await evaluate("document.querySelector('input[aria-label=\"一行命令接入设备\"]').value");
  assert(provider.startsWith('curl '));
  assert(!await evaluate("Boolean(document.querySelector('a[download]'))"), 'ZIP must not download until requested');
  start('sh', ['-c', provider], 'provider', { FELUCCA_DATA_DIR: join(output, 'provider-home') });
  await until(() => evaluate("fetch('/api/overview').then(r=>r.json()).then(data=>data.clients.some(c=>c.online))"), 'provider online session');
  await until(() => evaluate("[...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='添加服务')"), 'connected action');
  checks.push('created device in browser; copied primary one-line command installs from empty cache without admin cookies and reports actual online state');
  // Screenshots redact credential-bearing commands while retaining layout.
  await evaluate("document.querySelector('input[aria-label=\"一行命令接入设备\"]').value='curl … | sh … [device credentials hidden]' ");
  await screenshot('device-online.png');
  await call('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await call('Emulation.setPageScaleFactor', { pageScaleFactor: 1 });
  await evaluate('document.activeElement?.blur(); window.scrollTo(0, 0)');
  await delay(400); // Let the responsive drawer transition and viewport settle.
  assert(await evaluate('document.documentElement.scrollWidth <= innerWidth'), 'mobile page overflow');
  assert(await evaluate("document.querySelector('h1').getBoundingClientRect().left >= 0 && document.querySelector('.sidebar').getBoundingClientRect().right <= 1"), 'mobile content hidden by sidebar');
  await screenshot('device-mobile.png');
  await call('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await delay(400);
  await click('添加服务');
  await until(() => evaluate("Boolean(document.querySelector('.filter-toolbar'))"), 'services');
  await click('添加服务');
  await until(() => evaluate("Boolean(document.querySelector('#service-wizard'))"), 'service form');
  await fill('[name=serviceName]', '浏览器穿透网站');
  await fill('[name=serviceTarget]', `127.0.0.1:${fixture.backendPort}`);
  await fill('[name=servicePort]', fixture.publicPort);
  await evaluate("document.querySelector('#service-wizard form').requestSubmit()");
  await until(() => evaluate("!document.querySelector('#service-wizard')"), 'service saved');
  await until(async () => (await (await fetch(`http://127.0.0.1:${fixture.publicPort}`)).text()) === 'felucca-guided-flow\n', 'public payload');
  await text('已发布');
  await screenshot('public-service.png');
  checks.push('browser creates a service and actual public forwarding returns the local backend payload');
  await click('编辑');
  await until(() => evaluate("Boolean(document.querySelector('#service-wizard'))"), 'edit service');
  await evaluate("[...document.querySelectorAll('#service-wizard input[type=radio]')].find(input=>input.value==='private').click()");
  await evaluate("document.querySelector('#service-wizard form').requestSubmit()");
  await until(() => evaluate("!document.querySelector('#service-wizard')"), 'private service saved');
  await evaluate("location.hash='shares'");
  await until(() => evaluate("document.querySelector('.page-content') && !document.querySelector('.loading') && location.hash==='#shares'"), 'shares');
  await until(() => evaluate("[...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='在另一台设备访问')"), 'private action');
  await click('在另一台设备访问');
  await until(() => evaluate("Boolean(document.querySelector('#share-wizard'))"), 'receiver form');
  await fill('[name=receivingName]', '浏览器接收端');
  await fill('[name=localPort]', fixture.localPort);
  await evaluate("document.querySelector('#share-wizard form').requestSubmit()");
  await until(() => evaluate("Boolean(document.querySelector('#share-wizard input[aria-label=\"一行命令接入设备\"]'))"), 'receiver command');
  const receiver = await evaluate("document.querySelector('#share-wizard input[aria-label=\"一行命令接入设备\"]').value");
  start('sh', ['-c', receiver], 'receiver', { FELUCCA_DATA_DIR: join(output, 'receiver-home') });
  await until(async () => (await (await fetch(`http://127.0.0.1:${fixture.localPort}`)).text()) === 'felucca-guided-flow\n', 'private payload');
  checks.push('browser private-access command installs a second client without console login and forwards the real private service');
  await evaluate("location.hash='clients'");
  await until(() => evaluate("[...document.querySelectorAll('tr')].some(row => row.innerText.includes('浏览器一行命令设备'))"), 'device row');
  await evaluate("(()=>{const row=[...document.querySelectorAll('tr')].find(row=>row.innerText.includes('浏览器一行命令设备'));[...row.querySelectorAll('button')].find(b=>b.textContent.trim()==='编辑').click()})()");
  await until(() => evaluate("Boolean(document.querySelector('#editor'))"), 'device editor');
  assert(await evaluate("document.querySelector('#editor details.advanced')?.open === false"), 'device access restrictions start in advanced settings');
  await evaluate("document.querySelector('#editor details.advanced summary').click()");
  await evaluate("document.querySelector('#editor .scope-field input[value=device]').click()");
  await screenshot('advanced-network-restriction.png');
  await evaluate("document.querySelector('#editor-form').requestSubmit()");
  await until(() => evaluate("!document.querySelector('#editor')"), 'restriction saved');
  const configuration = await evaluate("fetch('/api/config').then(r=>r.json()).then(r=>r.config)");
  const providerId = Object.entries(configuration.client).find(([,client]) => client.remark === '浏览器一行命令设备')[0];
  assert.deepEqual(configuration.managed[providerId].target_policy.allow, ['ip:127.0.0.0/8', 'ip:::1/128']);
  await until(async () => (await (await fetch(`http://127.0.0.1:${fixture.localPort}`)).text()) === 'felucca-guided-flow\n', 'private payload after restriction');
  checks.push('all networks selected by default; collapsed advanced editor saves local-only restriction and private forwarding stays usable');
  assert.deepEqual(errors, []);
  report.status = 'passed';
} catch (error) {
  report.status = 'failed'; report.error = error.message; process.exitCode = 1;
} finally {
  socket?.close();
  for (const child of children.reverse()) {
    try { process.kill(-child.pid, 'SIGTERM'); } catch { /* Already stopped. */ }
    await until(() => child.exitCode !== null || child.signalCode !== null, 'cleanup', 5000).catch(() => { try { process.kill(-child.pid, 'SIGKILL'); } catch { /* Gone. */ } });
    const log = logs.get(child); await writeFile(join(output, `${log.name}.log`), log.text, { mode: 0o600 });
  }
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
}
