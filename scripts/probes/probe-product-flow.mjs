// Exercises the task-to-configuration boundary against freshly built real processes.
// --interactive keeps an empty fixture open for a browser walkthrough.
import { spawn, spawnSync } from 'node:child_process';
import { createServer, request as httpRequest } from 'node:http';
import { createServer as tcpServer } from 'node:net';
import { createSocket } from 'node:dgram';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import assert from 'node:assert/strict';
import { connectionPackage, packageScripts } from '../../apps/admin/src/features/connection-package.js';
import { buildService, deviceConfig, deviceOperations, randomKey, serviceDraft, visitConfig } from '../../apps/admin/src/features/product-flow.js';
import { connectionCommand } from '../../apps/admin/src/features/connection-command.js';

const root = resolve(import.meta.dirname, '../..');
const output = resolve(root, process.env.FELUCCA_PROBE_OUTPUT ?? 'target/tmp/product-flow-runtime');
const interactive = process.argv.includes('--interactive');
const oneCommand = process.argv.includes('--one-command');
const serverBin = join(root, 'target/debug/felucca-server'), agentBin = join(root, 'target/debug/felucca-agent');
await mkdir(output, { recursive: true, mode: 0o700 });
const children = [], checks = [];
const agentBuild = JSON.parse(command(agentBin, ['--build-info']));
const english = JSON.parse(await readFile(join(root, 'apps/admin/src/locales/en.json'), 'utf8'));
const translate = (key, args = {}) => Object.entries(args).reduce((text, [key, value]) => text.replaceAll(`{${key}}`, value), english[key]);
const assets = join(output, 'agent-assets');
command('python3', [join(root, 'scripts/stage-agent.py'), agentBin, '--output', assets]);
process.env.FELUCCA_AGENT_ASSETS = assets;
if (oneCommand) process.env.FELUCCA_DATA_DIR = join(output, 'connected-devices');
let cookie, csrf, origin, server;
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(fn, label, timeout = 30000) {
  const end = Date.now() + timeout; let last;
  while (Date.now() < end) { try { const result = await fn(); if (result) return result; } catch (error) { last = error; } await wait(150); }
  throw new Error(`${label}: ${last?.message ?? 'timeout'}`);
}
async function port() { const socket = tcpServer(); await new Promise(r => socket.listen(0, '127.0.0.1', r)); const p = socket.address().port; await new Promise(r => socket.close(r)); return p; }
function command(bin, args, input) { const r = spawnSync(bin, args, { input, encoding: 'utf8' }); if (r.status !== 0) throw new Error(r.stderr); return r.stdout.trim(); }
function start(bin, args, name, grouped = false) {
  const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'], detached: grouped }); child.grouped = grouped; children.push(child);
  const log = []; for (const stream of [child.stdout, child.stderr]) stream.on('data', s => log.push(s));
  child.on('close', () => writeFile(join(output, `${name}.log`), Buffer.concat(log), { mode: 0o600 }));
  return child;
}
async function stop(child) { if (child.grouped) { try { process.kill(-child.pid, 'SIGTERM'); } catch { /* Already stopped. */ } } else if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM'); await until(() => child.exitCode !== null || child.signalCode !== null, 'shutdown'); }
async function api(path, method = 'GET', value, revision) {
  const response = await fetch(origin + path, { method, headers: { 'Content-Type': 'application/json', Origin: origin,
    ...(cookie ? { Cookie: cookie, 'X-Felucca-CSRF': csrf } : {}), ...(revision !== undefined ? { 'If-Match': `"${revision}"`, 'Idempotency-Key': randomUUID() } : {}) },
    body: value === undefined ? undefined : JSON.stringify(value), signal: AbortSignal.timeout(15000) });
  if (path === '/api/login') cookie = response.headers.get('set-cookie').split(';')[0];
  const result = await response.json(); if (!response.ok) throw new Error(`${path}: ${response.status} ${result.error}`); return result;
}
async function batch(operations) { const { revision } = await api('/api/overview'); return api('/api/batch', 'POST', { operations }, revision); }
async function connected(id) { return until(async () => (await api('/api/overview')).clients.some(c => c.id === id && c.online), `device ${id} online`); }
async function agent(config, name) {
  const packages = (await api('/api/agent-packages')).packages;
  const available = packages.find(item => item.target === agentBuild.target); assert(available);
  assert.equal((await fetch(origin + `/api/agent-packages/${agentBuild.target}`)).status, 401);
  const response = await fetch(origin + `/api/agent-packages/${agentBuild.target}`, { headers: { Cookie: cookie, Origin: origin } });
  assert.equal(response.status, 200);
  const binary = new Uint8Array(await response.arrayBuffer());
  assert.equal(createHash('sha256').update(binary).digest('hex'), available.sha256);
  const id = `felucca-${name}`;
  const blob = connectionPackage({ config, binary, target: agentBuild.target, id, name, locale: 'en', t: translate });
  const archive = join(output, `${name}.zip`);
  await writeFile(archive, new Uint8Array(await blob.arrayBuffer()), { mode: 0o600 });
  command('python3', ['-c', `import sys,zipfile,pathlib
with zipfile.ZipFile(sys.argv[1]) as z:
 assert z.testzip() is None
 z.extractall(sys.argv[2])
 for entry in z.infolist():
  (pathlib.Path(sys.argv[2])/entry.filename).chmod((entry.external_attr>>16)&0o777)`, archive, output]);
  const folder = join(output, id), file = join(folder, 'connection.json');
  assert.deepEqual(JSON.parse(await readFile(file, 'utf8')), config);
  const extracted = join(folder, available.filename);
  command(extracted, ['--config', file, '--check']);
  command(extracted, ['service', '--name', id, '--scope', 'user', 'render', '--platform', agentBuild.target.includes('apple') ? 'launchd' : 'systemd', '--config', file, '--workdir', folder]);
  checks.push(`${name}: authenticated binary download verified; ZIP extracted; unedited launcher and service rendering use exact connection credentials`);
  if (oneCommand) {
    const commandText = connectionCommand(config, origin, 'unix', 'en');
    await writeFile(join(output, `${name}-command.txt`), commandText, { mode: 0o600 });
    checks.push(`${name}: original one-line command downloads to a private cache without an admin session or preinstalled client`);
    return start('sh', ['-c', commandText], name, true);
  }
  return start('sh', [join(folder, packageScripts(agentBuild.target).connect)], name);
}
async function requestService(port, headers = {}, path = '/') {
  // Node fetch may normalize Host to the URL authority. An explicit HTTP client
  // is required to test virtual hosting without modifying system DNS.
  return new Promise((resolve, reject) => {
    const req = httpRequest({ hostname: '127.0.0.1', port, path, headers, agent: false }, res => {
      let body = ''; res.on('data', data => body += data); res.on('end', () => {
        try { assert.equal(res.statusCode, 200); assert.equal(body, 'felucca-guided-flow\n'); resolve(true); } catch (error) { reject(error); }
      });
    }); req.on('error', reject); req.setTimeout(3000, () => req.destroy(new Error('HTTP timeout'))); req.end();
  });
}
async function udpEcho(port) {
  const sock = createSocket('udp4'); const payload = Buffer.from('guided-udp-datagram');
  try { return await new Promise((resolve, reject) => { const timer = setTimeout(() => reject(new Error('UDP timeout')), 1500); sock.once('message', data => { clearTimeout(timer); resolve(data.equals(payload)); }); sock.once('error', reject); sock.send(payload, port, '127.0.0.1'); }); }
  finally { sock.close(); }
}
async function status(port, method = 'GET', path = '/', body) {
  return new Promise((resolve, reject) => {
    const req = httpRequest({ hostname: '127.0.0.1', port, path, method, agent: false }, res => {
      res.resume(); res.on('end', () => resolve(res.statusCode));
    });
    req.on('error', reject); req.setTimeout(3000, () => req.destroy(new Error('HTTP timeout'))); req.end(body);
  });
}
const backend = createServer((req, res) => { res.writeHead(200, { 'content-type': 'text/plain' }); res.end('felucca-guided-flow\n'); });
const udp = createSocket('udp4'); udp.on('message', (data, peer) => udp.send(data, peer.port, peer.address));
try {
  await new Promise(r => backend.listen(0, '127.0.0.1', r)); await new Promise(r => udp.bind(0, '127.0.0.1', r));
  const backendPort = backend.address().port, udpPort = udp.address().port;
  const adminPort = await port(), bridgePort = await port(), p2pPort = await port(), publicPort = await port(), domainPort = await port(), localPort = await port(), localUdpPort = await port();
  const password = randomKey(), hash = command(serverBin, ['--hash-password'], password + '\n');
  const cert = join(output, 'bridge.pem'), key = join(output, 'bridge.key');
  const openssl = join(output, 'openssl.cnf');
  await writeFile(openssl, '[req]\ndistinguished_name=dn\nx509_extensions=ext\n[dn]\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n');
  command('/usr/bin/openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-sha256', '-days', '2', '-config', openssl, '-subj', '/CN=localhost', '-keyout', key, '-out', cert]);
  const serverFile = join(output, 'server.json');
  const config = { bridge: { bind: `127.0.0.1:${bridgePort}`, ...(oneCommand ? {} : { tls: { cert, key } }) },
    p2p: { bind: `127.0.0.1:${p2pPort}`, advertise: `127.0.0.1:${p2pPort}` }, user: { team: {} },
    admin: { bind: `127.0.0.1:${adminPort}`, state_path: join(output, `runtime-${randomUUID()}.json`), profiling: true,
      account: { root: { role: 'admin', password_hash: hash }, reader: { role: 'viewer', user: 'team', password_hash: hash }, operator: { role: 'operator', user: 'team', password_hash: hash } } } };
  await writeFile(serverFile, JSON.stringify(config), { mode: 0o600 }); command(serverBin, ['--config', serverFile, '--check']);
  server = start(serverBin, ['--config', serverFile], 'server'); origin = `http://127.0.0.1:${adminPort}`;
  await until(async () => (await fetch(origin)).ok, 'admin listener');
  await writeFile(join(output, 'fixture.json'), JSON.stringify({ origin, password, backendPort, udpPort, publicPort, domainPort, localPort, localUdpPort, serverFile }, null, 2), { mode: 0o600 });
  if (interactive) {
    console.log(`Empty browser fixture ready: ${origin}. Details saved privately in ${output}/fixture.json`);
    await new Promise(resolve => { process.once('SIGTERM', resolve); process.once('SIGINT', resolve); });
  } else {
    csrf = (await api('/api/login', 'POST', { account: 'root', password })).csrf;
    const launch = await api('/api/launch'); assert.equal(launch.p2p, true); assert.equal(launch.http_bind, null);
    const id = 'guided-provider', vkey = randomKey();
    await batch(deviceOperations(id, vkey, 'Office computer', 'team'));
    const folder = join(output, 'shared-folder'); await mkdir(folder, { recursive: true });
    await writeFile(join(folder, 'sample.txt'), 'felucca-guided-flow\n');
    const providerConfig = deviceConfig({ id, key: vkey, host: '127.0.0.1', launch, folder });
    await agent(providerConfig, 'provider'); await connected(id); checks.push('new device: generated package accepted, pinned TLS, observed online');
    if (oneCommand) {
      const repeated = spawnSync('sh', ['-c', connectionCommand(providerConfig, origin)], { encoding: 'utf8', timeout: 15000 });
      assert.notEqual(repeated.status, 0);
      assert.match(repeated.stderr, /already connected in another window/);
      await connected(id);
      checks.push('repeating the command rejects the duplicate process while the original device stays online');
    }
    const tunnel = buildService({ ...serviceDraft(), name: 'Office web app', target: `127.0.0.1:${backendPort}`, port: publicPort, bind: '127.0.0.1' }, undefined, 1);
    await batch([{ method: 'PUT', path: `/api/managed/${id}/tunnels/1`, value: tunnel }]);
    await until(() => requestService(publicPort), 'public TCP forwards'); checks.push('public web app: real HTTP payload through TCP forwarding');
    const site = buildService({ ...serviceDraft(), name: 'Domain site', target: `127.0.0.1:${backendPort}`, domain: 'guided.test', port: domainPort, bind: '127.0.0.1' }, undefined, 2);
    const current = (await api('/api/config')).config; current.http = { ...current.http, bind: `127.0.0.1:${domainPort}` };
    await batch([{ method: 'PUT', path: '/api/config', value: current }, { method: 'PUT', path: `/api/managed/${id}/tunnels/2`, value: site }]);
    await until(() => requestService(domainPort, { Host: 'guided.test' }), 'domain HTTP forwards'); checks.push('domain route + HTTP listener atomically activated, real payload');
    const filePort = await port(), proxyPort = await port();
    const files = buildService({ ...serviceDraft(), name: 'Authorized files', kind: 'file', port: filePort, bind: '127.0.0.1' }, undefined, 5);
    const proxy = buildService({ ...serviceDraft(), name: 'Authenticated HTTP proxy', kind: 'httpproxy', port: proxyPort, bind: '127.0.0.1' }, undefined, 6);
    const proxyPassword = randomKey(); proxy.options = { auth: { users: { test: proxyPassword } } };
    await batch([files, proxy].map(value => ({ method: 'PUT', path: `/api/managed/${id}/tunnels/${value.id}`, value })));
    await until(() => requestService(filePort, {}, '/sample.txt'), 'authorized file folder');
    await until(() => requestService(proxyPort, { 'Proxy-Authorization': `Basic ${Buffer.from(`test:${proxyPassword}`).toString('base64')}` }, `http://127.0.0.1:${backendPort}/`), 'authenticated proxy forwards');
    assert.equal(await status(proxyPort), 407);
    assert((await status(filePort, 'PUT', '/sample.txt', 'unauthorized write')) >= 400);
    assert.equal(await readFile(join(folder, 'sample.txt'), 'utf8'), 'felucca-guided-flow\n');
    checks.push('authorized folder serves real file, rejects writes; HTTP proxy requires credentials and forwards authenticated traffic');
    if (process.env.FELUCCA_PROBE_INTERNET === '1') {
      const internetFile = join(output, 'proxy-internet.html');
      const internet = () => spawnSync('curl', ['-q', '--config', '-', '--silent', '--show-error', '--fail',
        '--connect-timeout', '10', '--max-time', '30', '--noproxy', '', '--proxy', `http://127.0.0.1:${proxyPort}`,
        '--output', internetFile, '--write-out', '%{http_code} %{ssl_verify_result}', 'https://example.com/'],
        { input: `proxy-user = "test:${proxyPassword}"\n`, encoding: 'utf8', timeout: 35000 });
      const first = internet(); assert.equal(first.status, 0, first.stderr); assert.equal(first.stdout, '200 0');
      assert.match(await readFile(internetFile, 'utf8'), /Example Domain/);
      checks.push('default one-command device proxies public HTTPS without adding a destination grant; public CA verification succeeds');
      const managed = (await api(`/api/managed/${id}`)).config;
      await batch([{ method: 'PUT', path: `/api/managed/${id}`, value: { ...managed,
        target_policy: { allow: ['ip:127.0.0.0/8', 'ip:::1/128'] } } }]);
      await until(async () => {
        const denied = internet();
        await writeFile(join(output, 'restriction-response.json'), JSON.stringify({ status: denied.status, stdout: denied.stdout, stderr: denied.stderr }));
        const refused = denied.status === 22 && /returned error: 403/.test(denied.stderr);
        if (!refused) throw new Error(`curl status=${denied.status} response=${denied.stdout} error=${denied.stderr}`);
        return true;
      }, 'restricted device blocks public HTTPS');
      await requestService(publicPort);
      checks.push('explicit local-only restriction blocks public HTTPS while the local website remains available');
      await batch([{ method: 'PUT', path: `/api/managed/${id}`, value: { ...managed, target_policy: { allow: [] } } }]);
      await until(() => { const reopened = internet(); return reopened.status === 0 && reopened.stdout === '200 0'; }, 'all networks restores public HTTPS');
      checks.push('removing the destination restriction restores public HTTPS without replacing the connection command');
    }
    const privateTcp = buildService({ ...serviceDraft(), name: 'Private web app', target: `127.0.0.1:${backendPort}`, access: 'private' }, undefined, 3);
    const privateUdp = buildService({ ...serviceDraft(), name: 'Private UDP', kind: 'udp', target: `127.0.0.1:${udpPort}`, access: 'private' }, undefined, 4);
    await batch([privateTcp, privateUdp].map(value => ({ method: 'PUT', path: `/api/managed/${id}/tunnels/${value.id}`, value })));
    const receiverKey = randomKey();
    const tcpShare = { id: 'guided-tcp', config: { client: id, tunnel: 3, secret: randomKey(), allow_clients: ['guided-receiver'], direct: true, dynamic: false } };
    const udpShare = { id: 'guided-udp', config: { client: id, tunnel: 4, secret: randomKey(), allow_clients: ['guided-receiver'], direct: false, dynamic: false } };
    await batch([...deviceOperations('guided-receiver', receiverKey, 'My laptop', 'team').slice(0, 1), ...[tcpShare, udpShare].map(s => ({ method: 'PUT', path: `/api/shares/${s.id}`, value: s.config }))]);
    const receiverConfig = deviceConfig({ id: 'guided-receiver', key: receiverKey, host: '127.0.0.1', launch }); receiverConfig.remote.enabled = false;
    receiverConfig.visitor = [visitConfig(tcpShare, privateTcp, localPort), visitConfig(udpShare, privateUdp, localUdpPort)];
    await agent(receiverConfig, 'receiver'); await connected('guided-receiver');
    await until(() => requestService(localPort), 'private TCP forwards'); await until(() => udpEcho(localUdpPort), 'private UDP forwards');
    const diag = await api('/api/diagnostics');
    assert(diag.diagnostics.peers.events.some(event => event.state === 'direct' && event.tunnel === 3));
    assert(diag.diagnostics.peers.events.some(event => event.state === 'relay' && event.tunnel === 4));
    await writeFile(join(output, 'diagnostics.json'), JSON.stringify(diag, null, 2));
    checks.push('private TCP: real payload with observed direct session; UDP: observed relay with preserved datagrams');
    const redacted = JSON.stringify(await api('/api/config')); assert(!redacted.includes(vkey)); assert(!redacted.includes(tcpShare.config.secret)); checks.push('display responses redact generated credentials');
    const rootCookie = cookie, rootCsrf = csrf;
    csrf = (await api('/api/login', 'POST', { account: 'reader', password })).csrf;
    const revision = (await api('/api/overview')).revision;
    await assert.rejects(api('/api/clients/forbidden', 'PUT', {}, revision), /403/);
    cookie = rootCookie; csrf = rootCsrf; checks.push('viewer cannot mutate devices');
    await batch([{ method: 'PUT', path: `/api/managed/${id}/tunnels/1`, value: { ...tunnel, enabled: false } }]);
    await until(async () => !(await api('/api/overview')).tunnels.some(t => t.client === id && t.spec.tunnel_id === 1), 'disabled service unpublished');
    await batch([{ method: 'PUT', path: `/api/managed/${id}/tunnels/1`, value: tunnel }]);
    await until(() => requestService(publicPort), 'reenabled service forwards'); checks.push('disable/enable observed on real registration and forwarding');
    await stop(server); server = start(serverBin, ['--config', serverFile], 'server-restarted');
    await until(async () => (await fetch(origin)).ok, 'restarted admin'); csrf = (await api('/api/login', 'POST', { account: 'root', password })).csrf;
    if (oneCommand) assert.equal((await api('/api/launch')).fingerprint, launch.fingerprint);
    await until(() => requestService(publicPort), 'persisted service forwards after restart'); await until(() => requestService(localPort), 'persisted private access after restart'); checks.push('restart restores generated device/share credentials and working public/private services');
    const report = { status: 'passed', one_command: oneCommand, binary_sha256: createHash('sha256').update(await readFile(serverBin)).digest('hex'), checks, limits: ['All processes run on loopback; this does not establish public NAT traversal success rates.', 'Browser walkthrough is recorded separately.'] };
    await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n'); console.log(JSON.stringify(report, null, 2));
  }
} finally {
  for (const child of children.reverse()) await stop(child).catch(() => child.kill('SIGKILL'));
  backend.closeAllConnections(); backend.close(); udp.close();
}
