// Production-risk scenarios against real server/agent processes on loopback.
//   node core-stress.mjs [scenario...]   (default: all)
// Scenarios: integrity churn slow-consumer agent-kill server-restart server-crash agent-freeze backend-down
import { writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, OUT, sleep, until, freePort } from '../lib/lab.mjs';
import { backend, download, upload, roundTrip, loadFor, digestOf } from '../lib/net.mjs';

const MB = 1024 * 1024;
const P = (k, d) => Number(process.env[k] ?? d);
const results = {};

async function withLab(name, fn, labOptions = {}) {
  const lab = await Lab.start({ name, ...labOptions }); const be = await backend(); const publicPort = await freePort();
  const out = []; const log = (...a) => { const l = a.join(' '); out.push(l); console.log(`[${name}] ${l}`); };
  const ctx = { lab, be, publicPort, log, snap: (agent) => ({ server: lab.stats(), agent: agent?.stats() }) };
  try {
    ctx.agent = await lab.addAgent('a1');
    await lab.publish(ctx.agent, 1, { target: `127.0.0.1:${be.port}`, port: publicPort });
    await until(async () => (await roundTrip(publicPort)).ok, 'tunnel up', 20000);
    results[name] = await fn(ctx);
    log(`RESULT ${JSON.stringify(results[name])}`);
  } catch (e) { log(`FAILED: ${e.stack}`); results[name] = { error: e.message }; }
  finally { be.close(); await lab.stop(); await writeFile(join(lab.dir, 'scenario.txt'), out.join('\n')); }
}
const settle = async (lab, agent, ms = 6000) => { await sleep(ms); return { server: lab.stats(), agent: agent.stats() }; };
const recovered = async (publicPort, timeout = 60000) => { const t0 = Date.now(); await until(async () => (await roundTrip(publicPort, '127.0.0.1', 3000)).ok, 'tunnel recovered', timeout, 200); return Date.now() - t0; };

const scenarios = {
  // Payload integrity in both directions, concurrently, plus throughput.
  async integrity({ publicPort, lab, agent, log }) {
    const size = P('SIZE_MB', 64) * MB, n = P('PARALLEL', 8);
    const t0 = Date.now();
    const downs = await Promise.all(Array.from({ length: n }, (_, i) => download(publicPort, 100 + i, size)));
    const dt = (Date.now() - t0) / 1000;
    const bad = downs.filter((d, i) => !d.ok || d.hash !== digestOf(100 + i, size));
    log(`download ${n}x${size / MB}MB in ${dt.toFixed(1)}s = ${(n * size / MB / dt).toFixed(0)} MB/s aggregate, corrupt/failed=${bad.length}`);
    const t1 = Date.now();
    const ups = await Promise.all(Array.from({ length: n }, (_, i) => upload(publicPort, 200 + i, size)));
    const ut = (Date.now() - t1) / 1000;
    log(`upload ${n}x${size / MB}MB in ${ut.toFixed(1)}s = ${(n * size / MB / ut).toFixed(0)} MB/s aggregate, corrupt/failed=${ups.filter(u => !u.ok).length}`);
    const mixed = await Promise.all([...Array.from({ length: 4 }, (_, i) => download(publicPort, 300 + i, size)), ...Array.from({ length: 4 }, (_, i) => upload(publicPort, 400 + i, size))]);
    const mixedFailed = mixed.filter((m, i) => !m.ok || (i < 4 && m.hash !== digestOf(300 + i, size))).length;
    log(`mixed 4 down + 4 up failed=${mixedFailed}`);
    return { downFailed: bad.length, upFailed: ups.filter(u => !u.ok).length, mixedFailed, downMBps: +(n * size / MB / dt).toFixed(0), upMBps: +(n * size / MB / ut).toFixed(0), after: await settle(lab, agent) };
  },

  // Sustained open/close churn; descriptors and memory must return to baseline.
  async churn({ publicPort, lab, agent, log, snap }) {
    const before = snap(agent); const secs = P('SECONDS', 60), workers = P('WORKERS', 128);
    const r = await loadFor(secs, workers, () => roundTrip(publicPort));
    log(`churn ${secs}s x ${workers} workers: ${JSON.stringify(r)}`);
    const after = await settle(lab, agent, 10000);
    const leakFds = after.server.fds - before.server.fds, leakAgentFds = after.agent.fds - before.agent.fds;
    log(`before ${JSON.stringify(before)} after ${JSON.stringify(after)}`);
    return { ...r, before, after, leakedServerFds: leakFds, leakedAgentFds: leakAgentFds };
  },

  // One stalled reader must not block other connections nor make server memory grow without bound.
  async 'slow-consumer'({ publicPort, lab, agent, log, snap }) {
    const stallMs = P('STALL_MS', 20000); const samples = []; let peak = 0;
    const timer = setInterval(() => { const s = snap(agent); peak = Math.max(peak, s.server.rssMB, s.agent.rssMB); samples.push(s); }, 1000);
    const slow = download(publicPort, 7, 512 * MB, { stallAt: 4 * MB, stallMs, ms: 180000 });
    await sleep(3000);
    const during = await loadFor(Math.max(5, Math.floor(stallMs / 1000) - 8), 16, () => roundTrip(publicPort));
    log(`other connections while one reader is stalled: ${JSON.stringify(during)}`);
    const finished = await slow; clearInterval(timer);
    log(`stalled reader finished ok=${finished.ok} bytes=${finished.bytes} why=${finished.why}; peak RSS ${peak} MB`);
    return { during, stalledReaderOk: finished.ok, peakRssMB: peak, after: await settle(lab, agent) };
  },

  // Agent process dies mid-transfer: clients must fail promptly and the tunnel must come back.
  async 'agent-kill'({ publicPort, lab, agent, log }) {
    let killedAt = 0;
    const d = download(publicPort, 9, 256 * MB, { stallAt: 8 * MB, stallMs: 0, ms: 60000, onStall: async () => { killedAt = Date.now(); await agent.kill('SIGKILL'); } });
    const r = await d; const clientErrMs = Date.now() - killedAt;
    log(`mid-transfer SIGKILL: client ok=${r.ok} why=${r.why} saw failure after ${clientErrMs}ms`);
    const offlineAt = Date.now(); await until(async () => !(await lab.online('a1')), 'offline', 60000, 250); const offlineMs = Date.now() - offlineAt;
    const refused = await roundTrip(publicPort, '127.0.0.1', 4000);
    log(`console shows offline after ${offlineMs}ms; request to published port now: ok=${refused.ok} why=${refused.why} (${refused.ms}ms)`);
    await agent.start(); const t0 = Date.now(); const ms = await recovered(publicPort);
    log(`agent restarted; tunnel serving again after ${ms}ms`);
    const check = await download(publicPort, 11, 16 * MB);
    return { clientSawFailureMs: clientErrMs, hungClient: r.why === 'TIMEOUT', offlineDetectedMs: offlineMs, requestWhileOffline: refused.why, recoveryMs: ms, integrityAfter: check.ok && check.hash === digestOf(11, 16 * MB) };
  },

  // Graceful server restart with an agent attached and a transfer in flight.
  async 'server-restart'({ publicPort, lab, agent, log }) {
    const d = download(publicPort, 13, 128 * MB, { stallAt: 4 * MB, stallMs: 0, ms: 60000, onStall: async () => { setTimeout(() => lab.restart('SIGTERM').catch(e => log(`restart error ${e.message}`)), 0); } });
    const r = await d; log(`in-flight client during SIGTERM restart: ok=${r.ok} why=${r.why}`);
    const t0 = Date.now(); await until(async () => lab.child.exitCode === null && (await lab.online('a1').catch(() => false)), 'agent back online', 90000, 250);
    const onlineMs = Date.now() - t0; const ms = await recovered(publicPort);
    log(`agent reconnected and tunnel restored; online after ${onlineMs}ms, serving after ${ms}ms (tunnel config persisted)`);
    return { inFlightFailed: !r.ok, reconnectMs: onlineMs, servingMs: ms };
  },

  // Server killed with SIGKILL (crash): can it restart on its own state, locks and all?
  async 'server-crash'({ publicPort, lab, agent, log }) {
    await lab.restart('SIGKILL');
    const t0 = Date.now(); await until(async () => lab.child.exitCode === null && (await lab.online('a1').catch(() => false)), 'agent back online', 90000, 250);
    const ms = await recovered(publicPort); log(`after SIGKILL: online ${Date.now() - t0}ms; serving ${ms}ms`);
    const ov = await lab.overview();
    return { restarted: true, reconnectMs: Date.now() - t0, tunnelsAfter: ov.tunnels.length, logTail: lab.logTail(4).map(l => l.slice(0, 160)) };
  },

  // Frozen agent (network blackhole equivalent): how long until the server notices, and what clients see meanwhile.
  async 'agent-freeze'({ publicPort, lab, agent, log }) {
    agent.child.kill('SIGSTOP'); const t0 = Date.now();
    const probes = []; let offlineMs = null;
    while (Date.now() - t0 < P('FREEZE_MAX_S', 150) * 1000) {
      const p = await roundTrip(publicPort, '127.0.0.1', 4000); probes.push({ t: Date.now() - t0, ok: p.ok, why: p.why });
      if (!(await lab.online('a1').catch(() => true))) { offlineMs = Date.now() - t0; break; }
      await sleep(2000);
    }
    log(`SIGSTOP agent: server marked it offline after ${offlineMs ?? '>' + (Date.now() - t0)}ms; ${probes.filter(p => !p.ok).length}/${probes.length} requests failed meanwhile (${[...new Set(probes.map(p => p.why))]})`);
    agent.child.kill('SIGCONT'); const t1 = Date.now();
    const ms = await recovered(publicPort, 90000); log(`SIGCONT: tunnel serving again after ${ms}ms`);
    return { offlineDetectedMs: offlineMs, requestsDuringFreeze: probes.length, failedDuringFreeze: probes.filter(p => !p.ok).length, recoveryMs: ms };
  },

  // Backend (the local service) goes away and returns.
  async 'backend-down'({ publicPort, be, lab, agent, log }) {
    be.close(); const down = await Promise.all(Array.from({ length: 20 }, () => roundTrip(publicPort, '127.0.0.1', 8000)));
    log(`backend closed: ${down.filter(d => !d.ok).length}/20 failed; slowest failure ${Math.max(...down.map(d => d.ms))}ms; reasons ${JSON.stringify(down.reduce((m, d) => (m[d.why] = (m[d.why] ?? 0) + 1, m), {}))}`);
    const stats = await settle(lab, agent, 3000); const online = await lab.online('a1');
    return { failedWhileDown: down.filter(d => !d.ok).length, slowestFailMs: Math.max(...down.map(d => d.ms)), agentStillOnline: online, stats };
  },
};

const wanted = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(scenarios);
await mkdir(OUT, { recursive: true });
for (const name of wanted) {
  if (!scenarios[name]) { console.log(`unknown scenario ${name}`); continue; }
  await withLab(name, scenarios[name]);
}
await writeFile(join(OUT, `core-stress-${Date.now()}.json`), JSON.stringify(results, null, 2));
console.log('\nSUMMARY\n' + JSON.stringify(results, null, 2));
