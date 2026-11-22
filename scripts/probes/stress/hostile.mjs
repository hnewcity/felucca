// Malformed and abusive input against the bridge, console and a published port, all on
// loopback processes started by the lab. After each abuse the question is the same: does a
// well-behaved client still get service, and are the resources released?
import { connect } from 'node:net';
import { connect as tlsConnect } from 'node:tls';
import { writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { Lab, OUT, sleep, until, freePort } from '../lib/lab.mjs';
import { backend, roundTrip } from '../lib/net.mjs';

const lab = await Lab.start({ name: 'hostile', nofile: Number(process.env.NOFILE ?? 65536) });
const be = await backend(); const publicPort = await freePort();
const results = {}; const log = (...a) => console.log('[hostile]', ...a);
const good = async (n = 20) => { const r = await Promise.all(Array.from({ length: n }, () => roundTrip(publicPort, '127.0.0.1', 8000))); return `${r.filter(x => x.ok).length}/${n}`; };
const consoleOk = () => lab.overview().then(() => true, () => false);
const agentOnline = () => lab.online('a1').catch(() => false);

/** Open a socket, send `data` (if any), report how it ended within `wait` ms. */
function poke(port, data, wait = 4000, host = '127.0.0.1') {
  return new Promise(res => {
    const t0 = Date.now(); const s = connect(port, host); let got = 0;
    const end = how => { s.destroy(); res({ how, ms: Date.now() - t0, got }); };
    const timer = setTimeout(() => end('still-open'), wait);
    s.on('connect', () => data && s.write(data)); s.on('data', d => got += d.length);
    s.on('close', () => { clearTimeout(timer); end('closed'); }); s.on('error', e => { clearTimeout(timer); end(e.code); });
  });
}
function holdMany(port, count, host = '127.0.0.1') {
  return Promise.all(Array.from({ length: count }, () => new Promise(res => { const s = connect(port, host); s.on('connect', () => res(s)); s.on('error', () => res(null)); }))).then(a => a.filter(Boolean));
}

try {
  const agent = await lab.addAgent('a1');
  await lab.publish(agent, 1, { target: `127.0.0.1:${be.port}`, port: publicPort });
  await until(async () => (await roundTrip(publicPort)).ok, 'tunnel up');
  results.baseline = { good: await good(), stats: lab.stats() };

  // 1. Garbage on the bridge port: random bytes, TLS-looking prefixes, HTTP, oversized claims.
  const garbage = {
    random1k: randomBytes(1024), zeros: Buffer.alloc(4096), http: 'GET / HTTP/1.1\r\nHost: x\r\n\r\n',
    tlsHelloTruncated: Buffer.from('160301ffff0100ffff0303', 'hex'), tlsHugeRecord: Buffer.from('1603037fff', 'hex'),
    lengthBomb: Buffer.from('ffffffffffffffff', 'hex'), oneMiB: randomBytes(1 << 20),
  };
  const g = {}; for (const [k, v] of Object.entries(garbage)) g[k] = await poke(lab.bridgePort, v, 3000);
  results.bridgeGarbage = { outcomes: Object.fromEntries(Object.entries(g).map(([k, v]) => [k, `${v.how}@${v.ms}ms got=${v.got}`])), goodAfter: await good(), agentOnline: await agentOnline(), stats: lab.stats() };
  log('bridge garbage', JSON.stringify(results.bridgeGarbage));

  // 2. Connection hoarding on the bridge: many idle sockets that never speak.
  const idle = await holdMany(lab.bridgePort, Number(process.env.IDLE ?? 800));
  await sleep(2000);
  const during = { held: idle.length, newAgentCanConnect: false, good: await good(), stats: lab.stats() };
  const extra = await lab.addAgent('a2', { waitOnline: false }); during.newAgentCanConnect = await until(() => lab.online('a2'), 'a2', 20000, 300).then(() => true, () => false);
  const t0 = Date.now(); let closedByServer = 0;
  await Promise.all(idle.map(s => new Promise(r => { s.on('close', () => { closedByServer++; r(); }); setTimeout(r, Number(process.env.IDLE_WAIT_MS ?? 70000)); })));
  results.bridgeIdleHoarding = { ...during, closedByServerAfterWaitMs: Date.now() - t0, closedByServer, of: idle.length, statsAfter: lab.stats() };
  idle.forEach(s => s.destroy()); log('bridge idle', JSON.stringify(results.bridgeIdleHoarding));

  // 3. Half-open TLS handshakes (slowloris on the bridge): ClientHello prefix then silence.
  const hung = await holdMany(lab.bridgePort, 300); for (const s of hung) s.write(Buffer.from('16030100f8010000f4', 'hex'));
  await sleep(1500); const withHung = { held: hung.length, good: await good(), canConnect: (await poke(lab.bridgePort, null, 500)).how !== 'ECONNREFUSED' };
  hung.forEach(s => s.destroy()); results.bridgeHalfTls = withHung; log('bridge half-open TLS', JSON.stringify(withHung));

  // 4. Console abuse: wrong passwords, oversized and malformed bodies, missing Origin/CSRF.
  const statuses = {}; for (let i = 0; i < 60; i++) { const r = await fetch(lab.origin + '/api/login', { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: lab.origin }, body: JSON.stringify({ account: 'root', password: 'wrong' + i }) }); statuses[r.status] = (statuses[r.status] ?? 0) + 1; }
  const loginAfter = await fetch(lab.origin + '/api/login', { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: lab.origin }, body: JSON.stringify({ account: 'root', password: lab.password }) });
  const big = await fetch(lab.origin + '/api/login', { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: lab.origin }, body: 'x'.repeat(64 << 20) }).then(r => r.status, e => e.cause?.code ?? e.message);
  const bad = await fetch(lab.origin + '/api/login', { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: lab.origin }, body: '{"account":' }).then(r => r.status);
  const noOrigin = await fetch(lab.origin + '/api/overview', { headers: { Cookie: lab.cookie } }).then(r => r.status);
  const noCsrf = await fetch(lab.origin + '/api/batch', { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: lab.origin, Cookie: lab.cookie }, body: '{"operations":[]}' }).then(r => r.status);
  const hugeHeader = await poke(lab.adminPort, `GET / HTTP/1.1\r\nHost: x\r\nX-A: ${'a'.repeat(1 << 20)}\r\n\r\n`, 3000);
  results.consoleAbuse = { wrongPasswordStatuses: statuses, correctLoginAfterBruteForce: loginAfter.status, body64MiB: big, malformedJson: bad, noOrigin, noCsrf, hugeHeader: hugeHeader.how, consoleOk: await consoleOk() };
  log('console abuse', JSON.stringify(results.consoleAbuse));

  // 5. Hoarding on a published port: how many idle visitors can starve everyone else?
  const hoard = await holdMany(publicPort, Number(process.env.HOARD ?? 1200)); await sleep(2000);
  const starved = await good(10);
  results.publishedPortHoarding = { held: hoard.length, goodWhileHoarded: starved, serverStats: lab.stats(), agentStats: agent.stats() };
  log('published-port hoarding', JSON.stringify(results.publishedPortHoarding));
  await sleep(Number(process.env.HOARD_WAIT_MS ?? 30000));
  results.publishedPortHoarding.goodAfterWait = await good(10); results.publishedPortHoarding.stillOpenAfterWait = hoard.filter(s => !s.destroyed && s.readyState === 'open').length;
  hoard.forEach(s => s.destroy()); await sleep(5000); results.publishedPortHoarding.goodAfterRelease = await good(10); results.publishedPortHoarding.statsAfterRelease = lab.stats();
  log('after hoard release', JSON.stringify(results.publishedPortHoarding));
  results.logWarnings = lab.logTail(300).filter(l => /WARN|ERROR|panic/i.test(l)).slice(-10).map(l => l.slice(0, 220));
} catch (e) { log('FAILED', e.stack); results.error = e.message; }
finally { be.close(); await lab.stop(); await mkdir(OUT, { recursive: true }); await writeFile(join(OUT, `hostile-${Date.now()}.json`), JSON.stringify(results, null, 2)); console.log(JSON.stringify(results, null, 2)); }
