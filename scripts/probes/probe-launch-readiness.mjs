// Readiness audit: isolated loopback processes, no host service or firewall changes.
// Records current behavior, including usability failures; it is not a release gate.
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import assert from 'node:assert/strict';
import { deviceConfig, deviceOperations, buildService, serviceDraft, randomKey } from '../../apps/admin/src/features/product-flow.js';

const root = resolve(import.meta.dirname, '../..');
const output = resolve(root, process.env.FELUCCA_PROBE_OUTPUT ?? 'target/tmp/launch-readiness');
await mkdir(output, { recursive: true, mode: 0o700 });
const serverBin = join(root, 'target/debug/felucca-server');
const agentBin = join(root, 'target/debug/felucca-agent');
const children = [], logs = new Map();
const delay = ms => new Promise(r => setTimeout(r, ms));
let browserSocket;
async function until(check, label, timeout = 15000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try { const result = await check(); if (result) return result; } catch (error) { last = error; }
    await delay(100);
  }
  throw new Error(`${label}: ${last?.message ?? 'timeout'}`);
}
async function port() {
  const listener = createServer();
  await new Promise(r => listener.listen(0, '127.0.0.1', r));
  const value = listener.address().port;
  await new Promise(r => listener.close(r));
  return value;
}
function start(binary, args, name) {
  const child = spawn(binary, args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
  children.push(child); logs.set(child, { name, text: '' });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', bytes => { logs.get(child).text += bytes; });
  child.on('error', error => { logs.get(child).text += error.message; });
  return child;
}
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill('SIGTERM');
  await until(() => child.exitCode !== null || child.signalCode !== null, 'process exit');
}
const report = {
  validated_at: new Date().toISOString(),
  source_commit: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  binary_sha256: createHash('sha256').update(await readFile(serverBin)).digest('hex'),
  checks: {},
  limits: ['Loopback only; no public NAT, CA, Gatekeeper, SmartScreen or autostart installation acceptance.', 'Debug binaries; build freshness must be established by the caller.'],
};
try {
  const adminPort = await port(), bridgePort = await port(), publicPort = await port(), closedPort = await port();
  const origin = `http://127.0.0.1:${adminPort}`;
  const password = randomKey();
  const hashed = spawnSync(serverBin, ['--hash-password'], { input: password + '\n', encoding: 'utf8' });
  assert.equal(hashed.status, 0);
  const account = { role: 'admin', password_hash: hashed.stdout.trim() };
  const configFile = join(output, 'server.json');
  await writeFile(configFile, JSON.stringify({
    bridge: { bind: `127.0.0.1:${bridgePort}` },
    admin: { bind: `127.0.0.1:${adminPort}`, state_path: join(output, `runtime-${randomUUID()}.json`), account: { root: account, second: account } },
  }), { mode: 0o600 });
  let server = start(serverBin, ['--config', configFile], 'server');
  await until(async () => (await fetch(origin)).ok, 'server ready');
  let cookie, csrf;
  async function login(name = 'root') {
    const response = await fetch(origin + '/api/login', { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ account: name, password }) });
    if (response.ok) { cookie = response.headers.get('set-cookie').split(';')[0]; csrf = (await response.json()).csrf; }
    return response.status;
  }
  async function api(path, method = 'GET', body, revision) {
    const response = await fetch(origin + path, { method, headers: {
      Origin: origin, Cookie: cookie, 'Content-Type': 'application/json', 'X-Felucca-CSRF': csrf,
      ...(revision === undefined ? {} : { 'If-Match': `"${revision}"`, 'Idempotency-Key': randomUUID() }),
    }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(10000) });
    const value = await response.json(); assert(response.ok, `${path}: ${response.status} ${JSON.stringify(value)}`); return value;
  }
  assert.equal(await login(), 200);
  const launch = await api('/api/launch');
  const agentConfig = deviceConfig({ id: 'audit-device', key: randomKey(), host: '127.0.0.1', launch });
  const service = buildService({ ...serviceDraft(), name: 'Readiness closed backend', target: `127.0.0.1:${closedPort}`, port: publicPort, bind: '127.0.0.1' }, undefined, 1);
  await api('/api/batch', 'POST', { operations: [
    ...deviceOperations(agentConfig.key_id, agentConfig.vkey, 'Readiness device', null, agentConfig.remote.target_policy),
    { method: 'PUT', path: '/api/managed/audit-device/tunnels/1', value: service },
  ] }, (await api('/api/overview')).revision);
  const agentFile = join(output, 'agent.json');
  await writeFile(agentFile, JSON.stringify(agentConfig), { mode: 0o600 });
  let agent = start(agentBin, ['--config', agentFile], 'agent-original');
  await until(async () => (await api('/api/overview')).tunnels.some(row => row.client === 'audit-device'), 'service registered');
  let accessFailure;
  try { await fetch(`http://127.0.0.1:${publicPort}`, { signal: AbortSignal.timeout(3000) }); }
  catch (error) { accessFailure = error.cause?.code ?? error.name; }
  assert(accessFailure, 'closed backend unexpectedly answered');
  const health = await api('/api/diagnostics/audit-device/1');
  assert.equal(health.diagnostics.report.probing, false);
  assert.equal(health.diagnostics.report.backends[0].healthy, true);
  assert.equal(health.diagnostics.report.backends[0].last_probe_ms, 0);
  report.checks.closed_backend = { request_failed: true, access_failure: accessFailure, service_registered: true, diagnostics: health.diagnostics };

  // Inspect the real embedded UI in an isolated Chrome profile.
  const chrome = start(process.env.FELUCCA_CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-sync',
    `--user-data-dir=${join(output, 'chrome')}`, '--remote-debugging-port=0', 'about:blank',
  ], 'chrome');
  const endpoint = await until(() => logs.get(chrome).text.match(/DevTools listening on (ws:\/\/[^\s]+)/)?.[1], 'Chrome ready');
  browserSocket = new WebSocket(endpoint);
  await new Promise((r, reject) => { browserSocket.onopen = r; browserSocket.onerror = reject; });
  let seq = 0;
  const pending = new Map(), errors = [];
  browserSocket.onmessage = event => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) { const task = pending.get(message.id); pending.delete(message.id); message.error ? task.reject(new Error(JSON.stringify(message.error))) : task.resolve(message.result); }
    if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails.text);
  };
  function cdp(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++seq, timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 15000);
      pending.set(id, { resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); } });
      browserSocket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
  const { targetId } = await cdp('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await cdp('Target.attachToTarget', { targetId, flatten: true });
  const call = (method, params) => cdp(method, params, sessionId);
  async function evaluate(expression) {
    const value = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (value.exceptionDetails) throw new Error(value.exceptionDetails.text);
    return value.result.value;
  }
  await call('Page.enable'); await call('Runtime.enable');
  await call('Page.addScriptToEvaluateOnNewDocument', { source: "localStorage.setItem('felucca.locale', 'zh')" });
  await call('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await call('Page.navigate', { url: origin + '/#tunnels' });
  await until(() => evaluate("Boolean(document.querySelector('#login-form'))"), 'login form');
  await evaluate(`document.querySelector('[name=account]').value='root'; document.querySelector('[name=password]').value=${JSON.stringify(password)}; document.querySelector('#login-form').requestSubmit()`);
  await until(() => evaluate("document.body.innerText.includes('Readiness closed backend')"), 'service page');
  await evaluate("[...document.querySelectorAll('button')].find(b=>b.textContent==='访问方式').click()");
  await until(() => evaluate("Boolean(document.querySelector('#service-detail'))"), 'service detail');
  await evaluate("[...document.querySelectorAll('#service-detail button')].find(b=>b.textContent==='检查目标服务').click()");
  await until(() => evaluate("document.querySelector('#service-detail').innerText.includes('后端默认被视为可用')"), 'diagnostic result');
  const text = await evaluate("document.querySelector('#service-detail').innerText");
  const shot = await call('Page.captureScreenshot', { format: 'png' });
  await writeFile(join(output, 'closed-backend.png'), Buffer.from(shot.data, 'base64'));
  report.checks.browser = { embedded_build: true, closed_backend_detail: text, runtime_errors: errors, screenshot: 'closed-backend.png' };
  browserSocket.close(); browserSocket = null; await stop(chrome);

  await stop(server);
  server = start(serverBin, ['--config', configFile], 'server-restarted');
  await until(async () => (await fetch(origin)).ok, 'server restarted');
  assert.equal(await login(), 200);
  const nextLaunch = await api('/api/launch');
  assert.notEqual(nextLaunch.fingerprint, launch.fingerprint);
  await until(() => /pinned certificate mismatch|certificate does not match the pinned fingerprint/i.test(logs.get(agent).text), 'agent rejects changed identity');
  assert.equal((await api('/api/overview')).clients.find(row => row.id === 'audit-device').online, false);
  await stop(agent);
  agentConfig.trust.fingerprint = nextLaunch.fingerprint;
  await writeFile(agentFile, JSON.stringify(agentConfig), { mode: 0o600 });
  agent = start(agentBin, ['--config', agentFile], 'agent-new-fingerprint');
  await until(async () => (await api('/api/overview')).clients.some(row => row.id === 'audit-device' && row.online), 'new fingerprint restores connection');
  report.checks.ephemeral_identity = { initial_connection: true, fingerprint_changed_after_restart: true, original_agent_rejected_fingerprint: true, replacing_only_fingerprint_restored_connection: true };
  const statuses = [200];
  for (let i = 1; i < 11; i++) statuses.push(await login(i % 2 ? 'second' : 'root'));
  assert.deepEqual(statuses, [...Array(10).fill(200), 429]);
  report.checks.login_throttle = { accounts: 2, source_ips: 1, all_credentials_correct: true, statuses };
  report.status = 'audit_observations_reproduced';
} catch (error) {
  report.status = 'probe_failed'; report.error = error.message; process.exitCode = 1;
} finally {
  browserSocket?.close();
  for (const child of [...children].reverse()) {
    try { await stop(child); } catch { child.kill('SIGKILL'); }
    const log = logs.get(child);
    await writeFile(join(output, `${log.name}.log`), log.text, { mode: 0o600 });
  }
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ status: report.status, checks: Object.keys(report.checks), error: report.error, report: join(output, 'report.json') }));
}
