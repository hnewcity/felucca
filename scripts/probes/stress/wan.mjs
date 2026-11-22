// Real-network scenarios: agent on this machine (behind NAT) -> server on a public host.
// The console is reached through a port forward to the server's loopback listener.
//   FELUCCA_PROBE_HOST FELUCCA_PROBE_PASSWORD [FELUCCA_PROBE_ADMIN=http://127.0.0.1:8990]
//   [FELUCCA_PROBE_PORT=18081] [WAN_STEPS=transports,latency,throughput,concurrency,freeze]
import { createConnection } from 'node:net';
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, OUT, sleep, until } from '../lib/lab.mjs';
import { backend, roundTrip, download, upload, digestOf, loadFor, backendIdentity } from '../lib/net.mjs';

const host = process.env.FELUCCA_PROBE_HOST, password = process.env.FELUCCA_PROBE_PASSWORD;
const origin = process.env.FELUCCA_PROBE_ADMIN ?? 'http://127.0.0.1:8990', basePort = Number(process.env.FELUCCA_PROBE_PORT ?? 18081);
const steps = (process.env.WAN_STEPS ?? 'transports,latency,throughput,concurrency,freeze').split(',');
if (!host || !password) throw new Error('set FELUCCA_PROBE_HOST and FELUCCA_PROBE_PASSWORD');
const MB = 1024 * 1024, log = (...a) => console.log('[wan]', ...a), out = {};
const transportMiB = Number(process.env.WAN_TRANSPORT_MIB ?? 8);
const bulkMiB = Number(process.env.WAN_BULK_MIB ?? 64);
const bulkParallel = Number(process.env.WAN_BULK_PARALLEL ?? 8);
const parallelMiB = Number(process.env.WAN_PARALLEL_MIB ?? 16);
const transports = (process.env.WAN_TRANSPORTS ?? 'tcp,tls,ws,wss,quic,kcp').split(',');
const workloadTransport = process.env.WAN_WORKLOAD_TRANSPORT ?? 'tls';
const remoteProbeDirectory = process.env.FELUCCA_REMOTE_PROBE_DIR ?? '/root/felucca-test/src/scripts/probes/lib';
if (!/^\/[A-Za-z0-9_./-]+$/.test(remoteProbeDirectory)) throw new Error('Remote probe directory must be a simple absolute path');
if (![transportMiB, bulkMiB, bulkParallel, parallelMiB].every(n => Number.isInteger(n) && n > 0 && n <= 256)) throw new Error('WAN payload sizes and parallelism must be integers in 1..256');
if (!transports.every(t => ['tcp', 'tls', 'ws', 'wss', 'quic', 'kcp'].includes(t))) throw new Error('Unknown WAN transport');
if (!['tls', 'wss', 'quic'].includes(workloadTransport)) throw new Error('WAN workloads need TLS, WSS or QUIC');
out.parameters = { transportMiB, bulkMiB, bulkParallel, parallelMiB, transports, workloadTransport };
const dir = join(OUT, `wan-${Date.now()}`); await mkdir(dir, { recursive: true, mode: 0o700 });
const lab = Object.assign(new Lab(), { dir, origin, password, agents: [] });
lab.csrf = (await lab.api('/api/login', 'POST', { account: 'root', password })).csrf;
lab.launch = await lab.api('/api/launch');
const bridgePort = lab.launch.bridge.bind.split(':').at(-1);
const be = await backend();
const tcpConnectMs = () => new Promise(res => { const t = Date.now(); const s = createConnection(Number(bridgePort), host); s.setTimeout(10000, () => { s.destroy(); res(null); }); s.once('connect', () => { s.destroy(); res(Date.now() - t); }); s.once('error', () => res(null)); });
const rtt = []; for (let i = 0; i < 10; i++) { rtt.push(await tcpConnectMs()); await sleep(100); }
out.tcpHandshakeMs = { min: Math.min(...rtt.filter(Boolean)), median: rtt.filter(Boolean).sort((a, b) => a - b)[5] }; log('baseline network', JSON.stringify(out.tcpHandshakeMs));


// Visitors run on the server host, so only the agent's leg crosses this machine's own link.
// Must stay asynchronous: this process also hosts the local backend the tunnel forwards to.
const remoteVisit = (...args) => new Promise(resolve => {
  const child = spawn('/tmp/felucca-ssh/ssh.sh', ['-o', 'ServerAliveInterval=20', `root@${host}`, `cd ${remoteProbeDirectory} && node visit.mjs ${args.join(' ')}`],
    { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, FELUCCA_TEST_PW: process.env.FELUCCA_SSH_PW } });
  let stdout = '', stderr = ''; child.stdout.on('data', d => stdout += d); child.stderr.on('data', d => stderr += d);
  const timer = setTimeout(() => child.kill('SIGKILL'), 600000);
  child.on('close', (status, signal) => { clearTimeout(timer);
    try { resolve(JSON.parse(stdout.trim().split('\n').at(-1))); } catch { resolve({ error: (stderr || stdout || 'no output').slice(0, 300), status, signal }); } });
});

let n = 0;
async function session(transport, id, fn) {
  const port = basePort + (n++ % 8);
  // Plain TCP/WS may not carry remote configuration, P2P or visitors, so those are switched off for them.
  const plain = ['tcp', 'ws', 'kcp'].includes(transport);
  // Plaintext bridges cannot carry console-managed tunnels, so the tunnel lives in the agent's own file.
  const transportPort = transport === 'kcp' ? Number(bridgePort) + 1 : bridgePort;
  const patch = { server: `${host}:${transportPort}`, ...(plain ? { p2p: { enabled: false }, remote: { enabled: false },
    tunnel: [{ id: 1, mode: 'tcp', port, bind: '0.0.0.0', remark: 'static', backend: [{ addr: `127.0.0.1:${be.port}` }] }] } : {}) };
  const agent = await lab.addAgent(id, { transport, patch, waitOnline: false, managed: !plain });
  try {
    await until(() => lab.online(id), `${id} online`, 25000, 300);
    if (!plain) await lab.publish(agent, 1, { target: `127.0.0.1:${be.port}`, port, bind: '0.0.0.0' });
    await until(async () => (await roundTrip(port, host, 4000)).ok, `${id} forwards`, 20000, 300);
    if (await backendIdentity(port, host) !== be.identity) throw new Error('public ingress reached the wrong backend');
    const remoteIdentity = await remoteVisit('identity', port);
    if (remoteIdentity.identity !== be.identity) throw new Error('server-side ingress did not reach this backend');
    return { ...await fn({ port, agent }), backendIdentityVerified: true };
  } finally {
    await agent.kill('SIGKILL').catch(() => {});
    await lab.api(`/api/clients/${id}`, 'DELETE', undefined, (await lab.overview()).revision).catch(() => {});
  }
}
try {
  if (steps.includes('transports')) {
    // Enable the UDP bridge for QUIC; the server reloads its listeners.
    const cfg = (await lab.api('/api/config')).config;
    if (!cfg.bridge.quic_bind || !cfg.bridge.kcp_bind) {
      cfg.bridge.quic_bind = `0.0.0.0:${bridgePort}`;
      cfg.bridge.kcp_bind = `0.0.0.0:${Number(bridgePort) + 1}`;
      await lab.batch([{ method: 'PUT', path: '/api/config', value: cfg }]); await sleep(2500);
    }
    out.transports = {};
    for (const t of transports) {
      try {
        out.transports[t] = await session(t, `t-${t}`, async ({ port }) => {
          const trips = await Promise.all(Array.from({ length: 30 }, () => roundTrip(port, host, 10000)));
          const d = await download(port, 21, transportMiB * MB, { host });
          const previousUpload = be.uploadProgress().at(-1)?.id ?? 0;
          const u = await upload(port, 22, transportMiB * MB, { host });
          const backendUploads = be.uploadProgress().filter(state => state.id > previousUpload);
          return { payloadMiB: transportMiB, roundTrips: `${trips.filter(x => x.ok).length}/30`, p50ms: trips.map(x => x.ms).sort((a, b) => a - b)[15], download: { ok: d.ok && d.hash === digestOf(21, transportMiB * MB), bytesReceived: d.bytes, MiBps: d.ok ? +(transportMiB * 1000 / d.ms).toFixed(3) : null, ms: d.ms, why: d.why }, upload: { ok: u.ok, MiBps: u.ok ? +(transportMiB * 1000 / u.ms).toFixed(3) : null, ms: u.ms, why: u.why, backendUploads } };
        });
      } catch (e) { out.transports[t] = { error: e.message }; }
      log(`transport ${t}:`, JSON.stringify(out.transports[t]));
    }
    out.transportsPassed = transports.every(t => {
      const result = out.transports[t];
      return result.backendIdentityVerified && result.roundTrips === '30/30' && result.download?.ok && result.upload?.ok;
    });
    if (!out.transportsPassed) process.exitCode = 1;
  }
  if (steps.includes('latency')) out.latency = await session(workloadTransport, 'lat', async ({ port }) => { const r = await remoteVisit('latency', port, process.env.WAN_LAT_N ?? 200); log('latency, visitor on server, new connection per request', JSON.stringify(r)); return r; });
  if (steps.includes('throughput')) out.throughput = await session(workloadTransport, 'thr', async ({ port }) => { const r = await remoteVisit('throughput', port, bulkMiB, bulkParallel, parallelMiB); log('throughput, visitor on server', JSON.stringify(r)); return r; });
  if (steps.includes('concurrency')) out.concurrency = await session(workloadTransport, 'con', async ({ port }) => {
    const res = {}; for (const w of [20, 100, 400]) { res[`workers${w}`] = await remoteVisit('concurrency', port, w, 20); log(`concurrency ${w} workers`, JSON.stringify(res[`workers${w}`])); } return res;
  });
  if (steps.includes('hold')) out.hold = await session(workloadTransport, 'hold', async ({ port }) => { log('HOLD port', port); await sleep(Number(process.env.WAN_HOLD_S ?? 150) * 1000); return { port }; });
  if (steps.includes('freeze')) out.freeze = await session(workloadTransport, 'frz', async ({ port, agent }) => {
    agent.child.kill('SIGSTOP'); const t0 = Date.now(); let offline = null; const probes = [];
    while (Date.now() - t0 < 120000) { const p = await roundTrip(port, host, 4000); probes.push(p.ok ? 'ok' : p.why); if (!(await lab.online('frz'))) { offline = Date.now() - t0; break; } await sleep(2000); }
    agent.child.kill('SIGCONT'); const t1 = Date.now(); await until(async () => (await roundTrip(port, host, 3000)).ok, 'recover', 90000, 500);
    const r = { serverNoticedAfterMs: offline, requestsDuringFreeze: [...new Set(probes)], recoveryMs: Date.now() - t1 }; log('freeze', JSON.stringify(r)); return r;
  });
} catch (e) { out.error = e.stack; log('FAILED', e.stack); process.exitCode = 1; }
finally { be.close(); await writeFile(join(dir, 'result.json'), JSON.stringify(out, null, 2)); console.log(JSON.stringify(out, null, 2)); }
