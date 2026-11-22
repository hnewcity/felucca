// UDP forwarding under load, HTTP host routing at scale, and console mutation concurrency
// including crash safety of the persisted runtime snapshot.
//   node features-stress.mjs [udp] [http-routing] [admin-concurrency] [admin-crash]
// UDP uses an independent echo worker with a bounded 2 MiB receive request.
// Set UDP_ECHO_WORKER=0 UDP_ECHO_RCVBUF=0 to reproduce the original shared-loop fixture.
// Every case preserves its packet size, source count and packets per timer tick;
// actualPps reports delivered timer cadence separately from the requested pps.
import { createSocket } from 'node:dgram';
import { createServer, request } from 'node:http';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { writeFile, mkdir } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { Worker } from 'node:worker_threads';
import { once } from 'node:events';
import { Lab, OUT, sleep, until, freePort } from '../lib/lab.mjs';
import { buildService, serviceDraft } from '../../../apps/admin/src/features/product-flow.js';

const results = {}, log = (n, ...a) => console.log(`[${n}]`, ...a);
const wanted = process.argv.slice(2).length ? process.argv.slice(2) : ['udp', 'http-routing', 'admin-concurrency', 'admin-crash'];

async function udp() {
  const lab = await Lab.start({ name: 'udp' }); const echo = createSocket('udp4');
  let echoReceived = 0, echoSendErrors = 0;
  echo.on('message', (m, r) => { echoReceived++; echo.send(m, r.port, r.address, error => { if (error) echoSendErrors++; }); });
  await new Promise(r => echo.bind(0, '127.0.0.1', r));
  let defaultEchoReceiveBuffer = echo.getRecvBufferSize();
  const requestedEchoBuffer = Number(process.env.UDP_ECHO_RCVBUF ?? 2097152);
  if (requestedEchoBuffer) echo.setRecvBufferSize(requestedEchoBuffer);
  let echoReceiveBuffer = echo.getRecvBufferSize(), echoPort = echo.address().port, echoWorker;
  const echoCounters = new Int32Array(new SharedArrayBuffer(8));
  if (process.env.UDP_ECHO_WORKER !== '0') {
    echoWorker = new Worker(new URL('../lib/udp-echo.mjs', import.meta.url), { workerData: { counters: echoCounters.buffer, receiveBuffer: requestedEchoBuffer } });
    const [ready] = await once(echoWorker, 'message');
    echoPort = ready.port; defaultEchoReceiveBuffer = ready.defaultReceiveBuffer; echoReceiveBuffer = ready.receiveBuffer;
  }
  const echoCounts = () => echoWorker ? [Atomics.load(echoCounters, 0), Atomics.load(echoCounters, 1)] : [echoReceived, echoSendErrors];
  const port = await freePort();
  try {
    const agent = await lab.addAgent('a1');
    const tunnel = buildService({ ...serviceDraft(), name: 'udp', kind: 'udp', target: `127.0.0.1:${echoPort}`, port, bind: '127.0.0.1' }, undefined, 1);
    await lab.batch([{ method: 'PUT', path: `/api/managed/a1/tunnels/1`, value: tunnel }]);
    await until(() => new Promise(resolve => {
      const socket = createSocket('udp4'); let done = false;
      const finish = ok => { if (done) return; done = true; clearTimeout(timer); socket.close(); resolve(ok); };
      const timer = setTimeout(() => finish(false), 500);
      socket.once('error', () => finish(false));
      socket.once('message', m => finish(m.equals(Buffer.from('ready'))));
      socket.send(Buffer.from('ready'), port, '127.0.0.1');
    }), 'UDP forwarding ready');
    const run = (size, pps, seconds, sources, targetPort = port, tickMs = 10) => new Promise(resolve => {
      const [receivedBefore, errorsBefore] = echoCounts(); const started = Date.now(); let sendingMs = 0, sendErrors = 0;
      const socks = Array.from({ length: sources }, () => createSocket('udp4')); const sent = new Map(); const seen = new Set(); let dup = 0, bad = 0, ooo = 0, last = -1; const lat = [];
      for (const s of socks) s.on('message', m => { if (m.length !== size) { bad++; return; } const seq = m.readUInt32BE(0); if (!sent.has(seq) || !m.subarray(4).every(byte => byte === 7)) { bad++; return; } if (seen.has(seq)) { dup++; return; } seen.add(seq); if (seq < last) ooo++; last = Math.max(last, seq); lat.push(Date.now() - sent.get(seq)); });
      let seq = 0; const end = Date.now() + seconds * 1000; const per = Math.max(1, Math.round(pps * tickMs / 1000));
      const timer = setInterval(() => { for (let i = 0; i < per; i++) { const b = Buffer.alloc(size, 7); b.writeUInt32BE(seq, 0); sent.set(seq, Date.now()); socks[seq % sources].send(b, targetPort, '127.0.0.1', error => { if (error) sendErrors++; }); seq++; } if (Date.now() > end) { sendingMs = Date.now() - started; clearInterval(timer); setTimeout(() => { socks.forEach(s => s.close()); lat.sort((a, b) => a - b);
        const [receivedAfter, errorsAfter] = echoCounts();
        const lossBySecond = {};
        for (const [id, at] of sent) if (!seen.has(id)) { const second = Math.floor((at - started) / 1000); lossBySecond[second] = (lossBySecond[second] ?? 0) + 1; }
        resolve({ size, pps, sources, burstIntervalMs: tickMs, packetsPerBurst: per, sent: seq, received: seen.size, lossPct: +((1 - seen.size / seq) * 100).toFixed(2), duplicates: dup, corrupt: bad, reordered: ooo, p50ms: lat[Math.floor(lat.length / 2)], p99ms: lat[Math.floor(lat.length * 0.99)], sendingMs, actualPps: Math.round(seq * 1000 / sendingMs), sendErrors, echoReceived: receivedAfter - receivedBefore, echoSendErrors: errorsAfter - errorsBefore, defaultEchoReceiveBuffer, echoReceiveBuffer, independentEchoWorker: Boolean(echoWorker), lossBySecond }); }, 1500); } }, tickMs);
    });
    results.udp = []; const before = lab.stats();
    const cases = process.env.UDP_CASES ? JSON.parse(process.env.UDP_CASES) : [[64, 1000, 1, 1], [64, 1000, 1, 10], [512, 5000, 1, 10], [1400, 5000, 8, 10], [1400, 20000, 32, 10], [4000, 1000, 1, 10]];
    const traffic = async () => (await lab.overview()).clients.find(client => client.id === agent.id).traffic;
    for (const [size, pps, sources, tickMs = 10] of cases) {
      const directBaseline = await run(size, pps, 8, sources, echoPort, tickMs);
      const beforeTraffic = await traffic();
      const r = { ...await run(size, pps, 8, sources, port, tickMs), directBaseline };
      const afterTraffic = await traffic();
      r.tunnelInletPackets = (afterTraffic.inlet_bytes - beforeTraffic.inlet_bytes) / size;
      r.tunnelExportPackets = (afterTraffic.export_bytes - beforeTraffic.export_bytes) / size;
      results.udp.push(r); log('udp', JSON.stringify(r));
    }
    const complete = r => r.sent > 0 && r.received === r.sent && r.echoReceived === r.sent && !r.duplicates && !r.corrupt && !r.sendErrors && !r.echoSendErrors;
    results.udpPassed = results.udp.every(r => complete(r) && complete(r.directBaseline)
      && r.tunnelInletPackets === r.sent && r.tunnelExportPackets === r.sent);
    if (!results.udpPassed) process.exitCode = 1;
    await sleep(8000); results.udpResources = { before, afterBurst: lab.stats(), agent: agent.stats() }; log('udp', 'resources', JSON.stringify(results.udpResources));
  } finally { echo.close(); await echoWorker?.terminate(); await lab.stop(); }
}

async function httpRouting() {
  const lab = await Lab.start({ name: 'http-routing' }); const httpPort = await freePort();
  const seenHosts = new Map(); const be = createServer((req, res) => { seenHosts.set(req.headers.host, (seenHosts.get(req.headers.host) ?? 0) + 1); res.end(`host=${req.headers.host}`); });
  await new Promise(r => be.listen(0, '127.0.0.1', r));
  try {
    const cfg = (await lab.api('/api/config')).config; cfg.http = { ...cfg.http, bind: `127.0.0.1:${httpPort}` }; await lab.batch([{ method: 'PUT', path: '/api/config', value: cfg }]); await sleep(1500);
    const agents = []; const DOMAINS = Number(process.env.DOMAINS ?? 50);
    for (let a = 0; a < 5; a++) agents.push(await lab.addAgent(`h${a}`));
    for (let d = 0; d < DOMAINS; d++) {
      const t = buildService({ ...serviceDraft(), name: `site${d}`, kind: 'website', domain: `site${d}.test`, access: 'public', port: httpPort, target: `127.0.0.1:${be.address().port}`, bind: '127.0.0.1' }, undefined, d + 1);
      await lab.batch([{ method: 'PUT', path: `/api/managed/h${d % 5}/tunnels/${d + 1}`, value: t }]);
    }
    const get = (host, keepAlive) => new Promise(res => { const t0 = Date.now(); const r = request({ host: '127.0.0.1', port: httpPort, path: '/', headers: { Host: host }, agent: keepAlive }, resp => { let b = ''; resp.on('data', d => b += d); resp.on('end', () => res({ status: resp.statusCode, body: b, ms: Date.now() - t0 })); }); r.on('error', e => res({ status: 0, body: e.code })); r.setTimeout(10000, () => r.destroy(new Error('t'))); r.end(); });
    await until(async () => (await get('site0.test', false)).status === 200, 'http route up', 20000, 300);
    await sleep(2000);
    const N = Number(process.env.REQS ?? 6000), ka = new (await import('node:http')).Agent({ keepAlive: true, maxSockets: 64 });
    let misrouted = 0, failed = 0, next = 0; const codes = {}; const t0 = Date.now(); const ms = [];
    await Promise.all(Array.from({ length: 64 }, async () => { while (next < N) { const i = next++; const host = `site${i % DOMAINS}.test`; const r = await get(host, i % 2 ? ka : false);
      codes[r.status] = (codes[r.status] ?? 0) + 1; if (r.status !== 200) failed++; else { ms.push(r.ms); if (r.body !== `host=${host}`) misrouted++; } } }));
    ms.sort((a, b) => a - b);
    const unknown = await get('nobody.test', false), noHost = await get('', false);
    results.httpRouting = { domains: DOMAINS, requests: N, seconds: +((Date.now() - t0) / 1000).toFixed(1), failed, misrouted, codes, p50ms: ms[Math.floor(ms.length / 2)], p99ms: ms[Math.floor(ms.length * 0.99)], unknownHostStatus: unknown.status, emptyHostStatus: noHost.status, server: lab.stats() };
    log('http', JSON.stringify(results.httpRouting));
  } finally { be.close(); await lab.stop(); }
}

async function adminConcurrency() {
  const lab = await Lab.start({ name: 'admin-conc' });
  try {
    const { revision } = await lab.overview(); const mk = i => [{ method: 'PUT', path: `/api/clients/c${i}`, value: { user: null, remark: `c${i}`, vkey: randomUUID().replaceAll('-', '') + randomUUID().replaceAll('-', ''), enabled: true, allow_config_push: false } }];
    const racers = await Promise.all(Array.from({ length: 40 }, (_, i) => lab.api('/api/batch', 'POST', { operations: mk(i) }, revision).then(() => 200, e => e.status)));
    const tally = racers.reduce((m, s) => (m[s] = (m[s] ?? 0) + 1, m), {});
    const after = await lab.overview();
    // Same Idempotency-Key replayed must apply exactly once.
    const key = randomUUID(); const rev = (await lab.overview()).revision; const body = JSON.stringify({ operations: mk(900) });
    const send = () => fetch(lab.origin + '/api/batch', { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: lab.origin, Cookie: lab.cookie, 'X-Felucca-CSRF': lab.csrf, 'If-Match': `"${rev}"`, 'Idempotency-Key': key }, body }).then(async r => [r.status, (await r.json()).revision]);
    const [r1, r2] = [await send(), await send()];
    // Bulk sequential edits keep working and stay consistent.
    let t0 = Date.now(); for (let i = 0; i < 200; i++) await lab.batch(mk(1000 + i)); const bulkMs = Date.now() - t0;
    const finalCount = (await lab.overview()).clients.length;
    results.adminConcurrency = { sameRevisionRace: tally, clientsCreatedByRace: after.clients.length, idempotentReplay: { first: r1, second: r2 }, bulk200SequentialMs: bulkMs, clientsTotal: finalCount };
    log('admin', JSON.stringify(results.adminConcurrency));
  } finally { await lab.stop(); }
}

async function adminCrash() {
  const lab = await Lab.start({ name: 'admin-crash' }); const stateFile = join(lab.dir, 'runtime.json');
  try {
    let stop = false; const acked = new Set(); let i = 0;
    // A write is only counted once the console answered 200; failures retry the same id.
    const writer = (async () => { while (!stop) { try { await lab.batch([{ method: 'PUT', path: `/api/clients/k${i}`, value: { user: null, remark: `k${i}`, vkey: randomUUID().replaceAll('-', '') + randomUUID().replaceAll('-', ''), enabled: true, allow_config_push: false } }]); acked.add(`k${i}`); i++; } catch { await sleep(50); } } })();
    const rounds = [];
    for (let round = 0; round < Number(process.env.ROUNDS ?? 10); round++) {
      await sleep(300 + Math.random() * 1200); const snapshot = new Set(acked);
      lab.child.kill('SIGKILL'); await lab.child.exited; let valid = true, why = '';
      try { JSON.parse(readFileSync(stateFile, 'utf8')); } catch (e) { valid = false; why = e.message.slice(0, 80); }
      await lab.boot().catch(e => { valid = false; why += ` boot: ${e.message.slice(0, 100)}`; });
      let missing = null, extra = null;
      if (valid) { const present = new Set((await lab.overview()).clients.map(c => c.id)); missing = [...snapshot].filter(id => !present.has(id)).length; extra = [...present].filter(id => id.startsWith('k') && !acked.has(id)).length; }
      rounds.push({ acknowledgedBeforeKill: snapshot.size, acknowledgedButMissingAfterRestart: missing, presentButNeverAcknowledged: extra, snapshotValidJson: valid, ...(why ? { why } : {}) });
      if (!valid) break;
    }
    stop = true; await writer;
    results.adminCrash = rounds; log('admin-crash', JSON.stringify(rounds));
  } finally { await lab.stop(); }
}

await mkdir(OUT, { recursive: true });
const table = { udp, 'http-routing': httpRouting, 'admin-concurrency': adminConcurrency, 'admin-crash': adminCrash };
for (const name of wanted) { try { await table[name](); } catch (e) { results[name] = { error: e.stack }; log(name, 'FAILED', e.stack); process.exitCode = 1; } }
await writeFile(join(OUT, `features-stress-${Date.now()}.json`), JSON.stringify(results, null, 2));
