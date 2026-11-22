// Real Mac server -> Lima agent -> guest-only backends, with the production installer.
// Requires a running Lima VM and current Linux binaries staged by scripts/stage-agent.py.
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, readFile, writeFile, chmod } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { createServer, createConnection } from 'node:net';
import { createSocket } from 'node:dgram';
import { request } from 'node:http';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { deviceOperations, deviceConfig, buildService, serviceDraft, randomKey } from '../../apps/admin/src/features/product-flow.js';
import { connectionCommand } from '../../apps/admin/src/features/connection-command.js';

const root = resolve(import.meta.dirname, '../..');
const output = resolve(root, process.env.FELUCCA_PROBE_OUTPUT ?? 'target/tmp/lima-network-20261027/cross');
const vm = process.env.FELUCCA_LIMA_VM ?? 'ubuntu2604';
const guestRoot = process.env.FELUCCA_LIMA_ROOT ?? '/tmp/felucca-probe-lima';
const guest = `${guestRoot}/cross-${randomUUID()}`;
const helper = `${guestRoot}/scripts/probes/lima-fixture.py`;
const serverBin = join(root, 'target/debug/felucca-server');
const assets = resolve(root, process.env.FELUCCA_AGENT_ASSETS ?? 'target/tmp/lima-network-20261027/agent-assets');
const children = [], checks = [], logs = new Map();
const report = { status: 'running', checks, topology: 'Mac client -> Mac Felucca server -> Lima Linux agent -> guest loopback backend',
  limits: ['Local Lima virtual network; no public Internet NAT success-rate or WAN throughput claim.',
    'Synthetic H.264/AAC video; does not establish compatibility with commercial streaming services.'] };
const delay = ms => new Promise(r => setTimeout(r, ms));
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
let cookie, csrf, origin, hostOrigin, server, adminPort, cdpSocket;
await mkdir(output, { recursive: true, mode: 0o700 });
await chmod(output, 0o700);
async function until(fn, label, timeout = 45000) {
  const end = Date.now() + timeout; let last;
  while (Date.now() < end) { try { const value = await fn(); if (value) return value; } catch (error) { last = error; } await delay(150); }
  throw new Error(`${label}: ${last?.message ?? 'timeout'}`);
}
function command(bin, args, input) {
  const done = spawnSync(bin, args, { cwd: root, input, encoding: 'utf8', timeout: 120000, maxBuffer: 8 * 1024 * 1024 });
  if (done.status !== 0) throw new Error(`${bin}: ${done.error?.message ?? done.stderr}`);
  return done.stdout.trim();
}
const remote = (...args) => command('limactl', ['shell', vm, '--', ...args]);
const copy = (local, target) => command('limactl', ['copy', local, `${vm}:${target}`]);
async function guestFile(name, text) {
  const local = join(output, name); await writeFile(local, text, { mode: 0o600 }); copy(local, `${guest}/${name}`); return `${guest}/${name}`;
}
function guestStart(name, args) { return JSON.parse(remote('python3', helper, 'start', guest, name, ...args)); }
function start(bin, args, name, env = {}) {
  const child = spawn(bin, args, { cwd: root, detached: true, stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, ...env } });
  children.push(child); logs.set(child, { name, text: '' });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', bytes => { logs.get(child).text += bytes; });
  child.on('error', error => { logs.get(child).text += error.message; }); return child;
}
async function stop(child) {
  try { process.kill(-child.pid, 'SIGTERM'); } catch { /* Gone. */ }
  await until(() => child.exitCode !== null || child.signalCode !== null, 'process shutdown', 5000).catch(() => {
    try { process.kill(-child.pid, 'SIGKILL'); } catch { /* Gone. */ }
  });
}
async function freePort() { const s = createServer(); s.listen(0, '127.0.0.1'); await once(s, 'listening'); const p = s.address().port; await new Promise(r => s.close(r)); return p; }
async function http(port, path = '/', headers = {}, method = 'GET', timeout = 15000) {
  return new Promise((resolve, reject) => {
    const req = request({ hostname: '127.0.0.1', port, path, method, headers, agent: false }, res => {
      const chunks = []; res.on('data', x => chunks.push(x)); res.on('error', reject);
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }));
    }); req.on('error', reject); req.setTimeout(timeout, () => req.destroy(new Error('HTTP timeout'))); req.end();
  });
}
async function api(path, method = 'GET', value, revision) {
  return new Promise((resolve, reject) => {
    const body = value === undefined ? undefined : JSON.stringify(value);
    const req = request({ hostname: '127.0.0.1', port: adminPort, path, method, agent: false,
      headers: { 'Content-Type': 'application/json', Origin: hostOrigin, Host: new URL(hostOrigin).host,
        ...(body ? { 'Content-Length': Buffer.byteLength(body) } : {}),
        ...(cookie ? { Cookie: cookie, 'X-Felucca-CSRF': csrf } : {}),
        ...(revision !== undefined ? { 'If-Match': `"${revision}"`, 'Idempotency-Key': randomUUID() } : {}) } }, response => {
      const chunks = []; response.on('data', x => chunks.push(x)); response.on('error', reject);
      response.on('end', () => {
        try {
          const data = JSON.parse(Buffer.concat(chunks));
          if (response.statusCode >= 400) throw new Error(`${path}: ${response.statusCode} ${data.error}`);
          if (path === '/api/login') cookie = response.headers['set-cookie'][0].split(';')[0];
          resolve(data);
        } catch (error) { reject(error); }
      });
    }); req.on('error', reject); req.setTimeout(15000, () => req.destroy(new Error('API timeout'))); req.end(body);
  });
}
async function batch(operations) { return api('/api/batch', 'POST', { operations }, (await api('/api/overview')).revision); }
async function check(name, fn) {
  const began = Date.now();
  try { const detail = await fn(); checks.push({ name, ok: true, detail, elapsed_ms: Date.now() - began }); console.log(`PASS ${name}`); }
  catch (error) { checks.push({ name, ok: false, detail: error.message, elapsed_ms: Date.now() - began }); console.log(`FAIL ${name}: ${error.message}`); }
}
async function connect(port) {
  const socket = createConnection({ host: '127.0.0.1', port });
  socket.setTimeout(15000, () => socket.destroy(new Error('socket timeout')));
  await once(socket, 'connect'); return socket;
}
class Reader {
  constructor(socket) {
    this.socket = socket; this.data = Buffer.alloc(0); this.error = null; this.ended = false;
    socket.on('data', bytes => { this.data = Buffer.concat([this.data, bytes]); this.wake?.(); });
    socket.on('error', error => { this.error = error; this.wake?.(); });
    socket.on('end', () => { this.ended = true; this.wake?.(); });
  }
  async wait() { if (this.error) throw this.error; await new Promise(r => { this.wake = r; }); if (this.error) throw this.error; }
  async take(n) { while (this.data.length < n) { if (this.ended) throw new Error('unexpected EOF'); await this.wait(); } const value = this.data.subarray(0, n); this.data = this.data.subarray(n); return value; }
  async line() { while (!this.data.includes('\r\n')) { if (this.ended) throw new Error('unexpected EOF'); await this.wait(); } return this.take(this.data.indexOf('\r\n') + 2); }
  async all() { while (!this.ended) await this.wait(); if (this.error) throw this.error; return this.data; }
}
async function binaryTransfer(port, size = 2 * 1024 * 1024) {
  const socket = await connect(port), reader = new Reader(socket), bytes = randomBytes(size);
  try { socket.end(bytes); const response = await reader.all(); assert.deepEqual(response, Buffer.concat([bytes, Buffer.from('\nHALF-CLOSE-ACK\n')])); return { bytes: size, sha256: sha(bytes), half_close: true }; }
  finally { socket.destroy(); }
}
async function socks(port, username, password, targetPort, commandId = 1) {
  const socket = await connect(port), reader = new Reader(socket);
  try {
    socket.write(Buffer.from([5, 1, 2])); assert.deepEqual(await reader.take(2), Buffer.from([5, 2]));
    const user = Buffer.from(username), pass = Buffer.from(password);
    socket.write(Buffer.concat([Buffer.from([1, user.length]), user, Buffer.from([pass.length]), pass]));
    assert.deepEqual(await reader.take(2), Buffer.from([1, 0]));
    const addr = Buffer.from([5, commandId, 0, 1, 127, 0, 0, 1, targetPort >> 8, targetPort & 255]);
    if (commandId === 3) addr.fill(0, 4);
    socket.write(addr); const header = await reader.take(4); assert.equal(header[1], 0); assert.equal(header[3], 1);
    const bound = await reader.take(6); return { socket, reader, port: bound.readUInt16BE(4) };
  } catch (error) { socket.destroy(); throw error; }
}
async function udpSequence(port, payloads, prefix = null) {
  const sock = createSocket('udp4'); const latencies = []; let received = 0;
  try {
    for (const body of payloads) {
      const start = performance.now();
      const reply = await new Promise((resolve, reject) => {
        const timer = setTimeout(() => { sock.removeListener('message', done); reject(new Error('UDP timeout')); }, 5000);
        function done(bytes) { clearTimeout(timer); resolve(bytes); }
        sock.once('message', done);
        sock.send(prefix ? Buffer.concat([prefix, body]) : body, port, '127.0.0.1', error => { if (error) { clearTimeout(timer); sock.removeListener('message', done); reject(error); } });
      });
      assert.deepEqual(prefix ? reply.subarray(10) : reply, body); received++; latencies.push(performance.now() - start); await delay(2);
    }
    latencies.sort((a, b) => a - b);
    return { sent: payloads.length, received, p95_roundtrip_ms: Number(latencies[Math.floor(latencies.length * 0.95)].toFixed(2)) };
  } finally { sock.close(); }
}
async function videoBrowser(port) {
  const chrome = start(process.env.FELUCCA_CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-sync',
    '--autoplay-policy=no-user-gesture-required', `--user-data-dir=${join(output, 'chrome')}`, '--remote-debugging-port=0', 'about:blank',
  ], 'chrome');
  const endpoint = await until(() => logs.get(chrome).text.match(/DevTools listening on (ws:\/\/[^\s]+)/)?.[1], 'Chrome');
  cdpSocket = new WebSocket(endpoint); await new Promise((r, reject) => { cdpSocket.onopen = r; cdpSocket.onerror = reject; });
  let seq = 0; const pending = new Map(), ranges = [];
  cdpSocket.onmessage = event => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) { const entry = pending.get(message.id); pending.delete(message.id); message.error ? entry.reject(new Error(JSON.stringify(message.error))) : entry.resolve(message.result); }
    if (message.method === 'Network.responseReceived' && message.params.response.url.includes('/sample.mp4')) ranges.push(message.params.response.status);
  };
  function call(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => { const id = ++seq, timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 20000);
      pending.set(id, { resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); } });
      cdpSocket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })); });
  }
  const { targetId } = await call('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
  const page = (method, params) => call(method, params, sessionId);
  const evaluate = async expression => { const result = await page('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text); return result.result.value; };
  await page('Page.enable'); await page('Network.enable'); await page('Runtime.enable');
  await page('Emulation.setDeviceMetricsOverride', { width: 1100, height: 850, deviceScaleFactor: 1, mobile: false });
  await page('Page.navigate', { url: `http://127.0.0.1:${port}/video` });
  await until(() => evaluate('Boolean(document.querySelector("video")?.readyState >= 2)'), 'video ready');
  await evaluate('document.querySelector("video").play().then(()=>true)');
  await until(() => evaluate('document.querySelector("video").currentTime > 2'), 'video playback');
  const before = await evaluate('(()=>{const v=document.querySelector("video");return {time:v.currentTime,frames:v.getVideoPlaybackQuality().totalVideoFrames,width:v.videoWidth,height:v.videoHeight,duration:v.duration}})()');
  assert(before.frames > 0 && before.width === 1280 && before.height === 720);
  await evaluate('(()=>{const v=document.querySelector("video");v.currentTime=9;return true})()');
  await until(() => evaluate('(()=>{const v=document.querySelector("video");return v.currentTime > 10 && !v.seeking && !v.error})()'), 'seek and continue playback');
  const after = await evaluate('(()=>{const v=document.querySelector("video");v.pause();return {time:v.currentTime,frames:v.getVideoPlaybackQuality().totalVideoFrames,dropped:v.getVideoPlaybackQuality().droppedVideoFrames,error:v.error?.message ?? null}})()');
  assert(after.frames > before.frames); assert(ranges.includes(206));
  await writeFile(join(output, 'video-playing.png'), Buffer.from((await page('Page.captureScreenshot', { format: 'png' })).data, 'base64'));
  cdpSocket.close(); cdpSocket = null; await stop(chrome); return { before, after, http_statuses: ranges };
}

try {
  remote('mkdir', '-p', guest);
  copy(join(root, 'scripts/probes/lima-fixture.py'), helper);
  command('/usr/local/bin/ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=1280x720:rate=24', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000', '-t', '15', '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '22', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-movflags', '+faststart', join(output, 'sample.mp4')]);
  copy(join(output, 'sample.mp4'), `${guest}/sample.mp4`);
  guestStart('backend', ['python3', helper, 'backend', guest]);
  const backend = await until(() => JSON.parse(remote('cat', `${guest}/backend.json`)), 'guest backend');
  report.backend = backend;
  const bridgePort = await freePort(), entryPort = await freePort(); adminPort = await freePort();
  origin = `http://127.0.0.1:${adminPort}`; hostOrigin = `http://192.168.5.2:${adminPort}`;
  const password = randomKey(), hash = command(serverBin, ['--hash-password'], password + '\n');
  const config = { bridge: { bind: `0.0.0.0:${bridgePort}`, quic_bind: `0.0.0.0:${bridgePort}` }, http: { bind: `127.0.0.1:${entryPort}` },
    user: { team: {} }, admin: { bind: `0.0.0.0:${adminPort}`, origin: hostOrigin, state_path: join(output, 'runtime.json'),
      account: { root: { role: 'admin', password_hash: hash } } } };
  const configPath = join(output, 'server.json'); await writeFile(configPath, JSON.stringify(config), { mode: 0o600 });
  command(serverBin, ['--config', configPath, '--check']);
  server = start(serverBin, ['--config', configPath], 'server', { FELUCCA_AGENT_ASSETS: assets });
  await until(async () => (await http(adminPort, '/', { Host: new URL(hostOrigin).host })).status === 200, 'admin');
  csrf = (await api('/api/login', 'POST', { account: 'root', password })).csrf;
  const launch = await api('/api/launch');
  report.build = { mac: JSON.parse(command(serverBin, ['--build-info'])), mac_sha256: sha(await readFile(serverBin)),
    linux: JSON.parse(remote(`${guestRoot}/target/debug/felucca-agent`, '--build-info')),
    linux_manifest: JSON.parse(await readFile(join(assets, 'x86_64-unknown-linux-gnu/manifest.json'), 'utf8')) };
  const id = 'lima-provider', vkey = randomKey();
  await batch(deviceOperations(id, vkey, 'Lima actual Linux device', 'team'));
  const provider = deviceConfig({ id, key: vkey, host: '192.168.5.2', launch });
  const connection = connectionCommand(provider, hostOrigin, 'unix', 'en');
  const launcher = await guestFile('connect.sh', connection + '\n');
  guestStart('provider', ['env', `FELUCCA_DATA_DIR=${guest}/installed`, 'sh', launcher]);
  await check('Linux one-command installation from empty cache and authenticated TLS session', async () => {
    await until(async () => (await api('/api/overview')).clients.some(c => c.id === id && c.online), 'Linux provider online', 90000);
    const installed = remote('find', `${guest}/installed/bin`, '-name', 'felucca-agent').split('\n'); assert.equal(installed.length, 1);
    const digest = remote('sha256sum', installed[0]).split(' ')[0]; assert.equal(digest, report.build.linux_manifest.sha256);
    return { installed_sha256: digest, platform: report.build.linux.target, cached_before: false };
  });
  if (!checks.at(-1).ok) throw new Error('Linux installer did not connect; see guest logs');
  const ports = {};
  for (const name of ['web', 'tcp', 'udp', 'socks', 'http', 'mixed', 'codec']) ports[name] = await freePort();
  const proxyPassword = randomKey(), auth = { users: { probe: proxyPassword } };
  const specs = [
    ['web', 'website', backend.http], ['tcp', 'tcp', backend.tcp], ['udp', 'udp', backend.udp],
    ['socks', 'socks5'], ['http', 'httpproxy'], ['mixed', 'mixedproxy'], ['codec', 'tcp', backend.tcp],
  ].map(([name, kind, target], index) => {
    const value = buildService({ ...serviceDraft(), name, kind, target: `127.0.0.1:${target ?? 1}`, port: ports[name], bind: '127.0.0.1' }, undefined, index + 1);
    if (['socks', 'http', 'mixed'].includes(name)) value.options = { auth };
    if (name === 'codec') value.options = { payload: { compress: true, encrypt: true } };
    return value;
  });
  const website = buildService({ ...serviceDraft(), name: 'Domain website', domain: 'lima.test', target: `127.0.0.1:${backend.http}`, port: entryPort, bind: '127.0.0.1' }, undefined, 8);
  await batch([...specs, website].map(value => ({ method: 'PUT', path: `/api/managed/${id}/tunnels/${value.id}`, value })));
  await until(async () => (await http(ports.web)).body.toString() === backend.marker, 'cross-VM website');
  await check('Website over TCP and HTTP domain reverse proxy', async () => {
    for (const [port, headers] of [[ports.web, {}], [entryPort, { Host: 'lima.test' }]]) { const reply = await http(port, '/', headers); assert.equal(reply.status, 200); assert.equal(reply.body.toString(), backend.marker); }
    return 'Both returned the unique guest-only backend marker';
  });
  await check('TCP four concurrent 8 MiB binary transfers and half-close', async () => {
    const start = performance.now(), streams = await Promise.all(Array.from({ length: 4 }, () => binaryTransfer(ports.tcp, 8 * 1024 * 1024)));
    return { streams, elapsed_ms: Math.round(performance.now() - start) };
  });
  await check('Compression + encryption preserve binary stream and half-close', () => binaryTransfer(ports.codec));
  report.mac_udp_maxdgram = command('sysctl', ['-n', 'net.inet.udp.maxdgram']);
  await check('UDP empty, small, MTU-sized and 8,192-byte datagrams', () => udpSequence(ports.udp, [Buffer.alloc(0), randomBytes(1), randomBytes(1400), randomBytes(8192)]));
  await check('UDP four concurrent senders, 1,200 paced request/reply datagrams', () => Promise.all(Array.from({ length: 4 }, () => udpSequence(ports.udp, Array.from({ length: 300 }, () => randomBytes(1024))))));
  const proxyHeader = { 'Proxy-Authorization': `Basic ${Buffer.from(`probe:${proxyPassword}`).toString('base64')}` };
  for (const [name, port] of [['SOCKS5', ports.socks], ['Mixed SOCKS5', ports.mixed]]) {
    await check(`${name} authenticated CONNECT`, async () => {
      const { socket, reader } = await socks(port, 'probe', proxyPassword, backend.tcp); const bytes = randomBytes(65536);
      try { socket.end(bytes); assert.deepEqual(await reader.all(), Buffer.concat([bytes, Buffer.from('\nHALF-CLOSE-ACK\n')])); return { bytes: bytes.length }; } finally { socket.destroy(); }
    });
    await check(`${name} authenticated UDP ASSOCIATE`, async () => {
      const association = await socks(port, 'probe', proxyPassword, 0, 3);
      try { const prefix = Buffer.from([0, 0, 0, 1, 127, 0, 0, 1, backend.udp >> 8, backend.udp & 255]); return await udpSequence(association.port, [Buffer.alloc(0), randomBytes(1200), randomBytes(8192)], prefix); }
      finally { association.socket.destroy(); }
    });
    await check(`${name} wrong password rejected`, async () => { await assert.rejects(socks(port, 'probe', 'incorrect', backend.tcp)); return true; });
  }
  for (const [name, port] of [['HTTP proxy', ports.http], ['Mixed HTTP', ports.mixed]]) {
    await check(`${name} authenticated absolute URL request`, async () => {
      const reply = await http(port, `http://127.0.0.1:${backend.http}/`, proxyHeader); assert.equal(reply.status, 200); assert.equal(reply.body.toString(), backend.marker); return true;
    });
    await check(`${name} authenticated CONNECT binary tunnel`, async () => {
      const socket = await connect(port), reader = new Reader(socket); const bytes = randomBytes(65536);
      try {
        socket.write(`CONNECT 127.0.0.1:${backend.tcp} HTTP/1.1\r\nHost: 127.0.0.1:${backend.tcp}\r\nProxy-Authorization: ${proxyHeader['Proxy-Authorization']}\r\n\r\n`);
        assert.match((await reader.line()).toString(), /^HTTP\/1.[01] 200/); while ((await reader.line()).length > 2) { /* Drain headers. */ }
        socket.end(bytes); assert.deepEqual(await reader.all(), Buffer.concat([bytes, Buffer.from('\nHALF-CLOSE-ACK\n')])); return true;
      } finally { socket.destroy(); }
    });
    await check(`${name} unauthenticated access rejected`, async () => { assert.equal((await http(port, `http://127.0.0.1:${backend.http}/`)).status, 407); return true; });
  }
  if (process.env.FELUCCA_PROBE_INTERNET === '1') {
    // New devices allow public destinations without an additional scope grant.
    for (const [name, scheme, port] of [['SOCKS5', 'socks5h', ports.socks], ['HTTP CONNECT', 'http', ports.http], ['Mixed SOCKS5', 'socks5h', ports.mixed], ['Mixed CONNECT', 'http', ports.mixed]]) {
      await check(`${name} accesses public HTTPS example.com with CA verification`, async () => {
        const slug = name.toLowerCase().replaceAll(' ', '-'), file = join(output, `${slug}-internet.curl`), destination = join(output, `${slug}-internet.html`);
        await writeFile(file, [
          'silent', 'show-error', 'fail', 'connect-timeout = 10', 'max-time = 30', 'noproxy = ""',
          `proxy = "${scheme}://127.0.0.1:${port}"`, `proxy-user = "probe:${proxyPassword}"`,
          'url = "https://example.com/"', `output = "${destination}"`, 'write-out = "%{http_code} %{ssl_verify_result}"',
        ].join('\n') + '\n', { mode: 0o600 });
        const result = command('curl', ['--config', file]); assert.equal(result, '200 0');
        assert.match(await readFile(destination, 'utf8'), /Example Domain/); return { url: 'https://example.com/', status: 200, tls_verify_result: 0 };
      });
    }
  }
  await check('Video complete file SHA256 and middle/suffix Range downloads', async () => {
    const video = await readFile(join(output, 'sample.mp4')), whole = await http(ports.web, '/sample.mp4'); assert.equal(whole.status, 200); assert.equal(sha(whole.body), backend.video_sha256);
    for (const [range, expected] of [['bytes=12345-23456', video.subarray(12345, 23457)], ['bytes=-4096', video.subarray(-4096)]]) {
      const partial = await http(ports.web, '/sample.mp4', { Range: range }); assert.equal(partial.status, 206); assert.deepEqual(partial.body, expected);
    } return { bytes: video.length, sha256: sha(video) };
  });
  await check('Chrome decodes and plays video, seeks to 9 s and continues', () => videoBrowser(ports.web));
  await check('Native QUIC cross-VM TCP and UDP payloads', async () => {
    const qid = 'lima-quic', key = randomKey(); await batch(deviceOperations(qid, key, 'Lima QUIC', 'team'));
    const qconfig = deviceConfig({ id: qid, key, host: '192.168.5.2', launch }); qconfig.transport = 'quic';
    const file = await guestFile('quic.json', JSON.stringify(qconfig)); guestStart('quic', [`${guestRoot}/target/debug/felucca-agent`, '--config', file]);
    const tcp = await freePort(), udp = await freePort();
    await batch([['tcp', tcp, backend.tcp], ['udp', udp, backend.udp]].map(([kind, port, target], index) => ({ method: 'PUT', path: `/api/managed/${qid}/tunnels/${index + 1}`,
      value: buildService({ ...serviceDraft(), name: kind, kind, port, bind: '127.0.0.1', target: `127.0.0.1:${target}` }, undefined, index + 1) })));
    await until(async () => (await api('/api/overview')).clients.some(c => c.id === qid && c.online), 'QUIC online');
    await until(async () => { const overview = await api('/api/overview'); return [1, 2].every(id => overview.tunnels.some(t => t.client === qid && t.spec.tunnel_id === id)); }, 'QUIC routes');
    return { tcp: await binaryTransfer(tcp), udp: await udpSequence(udp, [Buffer.alloc(0), randomBytes(1400), randomBytes(8192)]) };
  });
  await check('Wrong server fingerprint and device key cannot take over a working device', async () => {
    for (const [name, change] of [['wrong-pin', { trust: { mode: 'pinned', fingerprint: '00'.repeat(32) } }], ['wrong-key', { vkey: randomKey() }]]) {
      const file = await guestFile(`${name}.json`, JSON.stringify({ ...provider, ...change })); guestStart(name, [`${guestRoot}/target/debug/felucca-agent`, '--config', file]);
    }
    await until(() => /pinned certificate mismatch/.test(remote('cat', `${guest}/wrong-pin.log`)), 'wrong pin rejection');
    await until(() => /authentication failed/.test(remote('cat', `${guest}/wrong-key.log`)), 'wrong key rejection');
    assert.equal((await http(ports.web)).body.toString(), backend.marker); return 'Rejected attempts; original published website remains usable';
  });
  await check('Restart retains server certificate and existing Linux client reconnects', async () => {
    const began = Date.now(); await stop(server); server = start(serverBin, ['--config', configPath], 'server-restarted', { FELUCCA_AGENT_ASSETS: assets });
    await until(async () => (await http(adminPort, '/', { Host: new URL(hostOrigin).host })).status === 200, 'restarted server');
    csrf = (await api('/api/login', 'POST', { account: 'root', password })).csrf;
    assert.equal((await api('/api/launch')).fingerprint, launch.fingerprint);
    await until(async () => (await http(ports.web)).body.toString() === backend.marker, 'TLS client reconnects');
    await binaryTransfer(ports.tcp, 65536); await udpSequence(ports.udp, [randomBytes(100)]); return { recovery_ms: Date.now() - began, same_certificate: true };
  });
  report.status = checks.every(check => check.ok) ? 'passed' : 'failed';
} catch (error) {
  report.status = 'failed'; report.error = error.stack;
} finally {
  cdpSocket?.close();
  try { report.cleanup = JSON.parse(remote('python3', helper, 'stop', guest)); } catch (error) { report.cleanup_error = error.message; }
  for (const child of children.reverse()) { await stop(child); const log = logs.get(child); await writeFile(join(output, `${log.name}.log`), log.text, { mode: 0o600 }); }
  try { command('limactl', ['copy', '-r', `${vm}:${guest}`, join(output, 'guest')]); } catch (error) { report.copy_error = error.message; }
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify({ status: report.status, passed: checks.filter(c => c.ok).length, total: checks.length, error: report.error, report: join(output, 'report.json') }, null, 2));
  if (report.status !== 'passed') process.exitCode = 1;
}
