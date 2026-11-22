// Multi-day endurance run: steady traffic over several transports while agents are killed,
// paused and restarted, configuration is changed live, silent connections are abandoned on
// the management port, and the server itself is restarted. Samples are appended to a JSONL
// file as the run goes, so a crash of the driver still leaves the evidence up to that point.
//   DURATION_H=72 node endurance.mjs
//   Optional: WORKERS (per tunnel, 2), SAMPLE_S (60), KILL_S (900), PAUSE_S (3600),
//   CONFIG_S (300), SILENT_S (600), SERVER_RESTART_S (21600), IDLE_PROBE_S (3600), BULK_MIB (4),
//   TRANSPORTS (tls,wss,quic). LAB_BIN_DIR selects the binaries, LAB_OUT the output.
import { appendFile, mkdir, writeFile } from 'node:fs/promises';
import { connect } from 'node:net';
import { join } from 'node:path';
import { Lab, OUT, sleep, until, freePort } from '../lib/lab.mjs';
import { backend, roundTrip, download, upload, digestOf } from '../lib/net.mjs';

const env = (name, value) => Number(process.env[name] ?? value);
const DURATION = env('DURATION_H', 72) * 3600 * 1000, WORKERS = env('WORKERS', 2), SAMPLE = env('SAMPLE_S', 60) * 1000;
const KILL = env('KILL_S', 900) * 1000, PAUSE = env('PAUSE_S', 3600) * 1000, CONFIG = env('CONFIG_S', 300) * 1000;
const IDLE_PROBE = env('IDLE_PROBE_S', 3600) * 1000;
const SILENT = env('SILENT_S', 600) * 1000, RESTART = env('SERVER_RESTART_S', 21600) * 1000, BULK = env('BULK_MIB', 4) * 1024 * 1024;
// Managed devices take their configuration from the server, which needs a verified bridge.
const TRANSPORTS = (process.env.TRANSPORTS ?? 'tls,wss,quic').split(',');

await mkdir(OUT, { recursive: true });
const started = Date.now(), file = join(OUT, `endurance-${started}.jsonl`);
const record = value => appendFile(file, JSON.stringify({ t: Math.round((Date.now() - started) / 1000), ...value }) + '\n');
const elapsed = () => Date.now() - started;
let stop = false, stopped;
const stopping = new Promise(resolve => { stopped = resolve; });

// Disruption windows: a failure on a tunnel while its agent or the server is deliberately
// down is expected; any other failure counts against the run.
const disrupted = new Map(); // tunnel id or '*' -> until timestamp
const expected = id => [id, '*'].some(key => (disrupted.get(key) ?? 0) > Date.now());
const disrupt = (key, ms) => disrupted.set(key, Math.max(disrupted.get(key) ?? 0, Date.now() + ms));

const window = () => ({ trips: 0, slow: 0, unexpected: 0, expected: 0, why: {}, maxms: 0, bulk: 0, bulkBad: 0 });
let current = window();
const totals = { trips: 0, unexpected: 0, expected: 0, bulk: 0, bulkBad: 0, events: [] };

const lab = await Lab.start({ name: 'endurance' });
const be = await backend();
const tunnels = [];
try {
  if (TRANSPORTS.includes('quic')) {
    // QUIC listens on the bridge port number over UDP.
    const { config } = await lab.api('/api/config');
    config.bridge.quic_bind = `127.0.0.1:${lab.bridgePort}`;
    await lab.batch([{ method: 'PUT', path: '/api/config', value: config }]);
    await sleep(2500);
  }
  for (const [index, transport] of TRANSPORTS.entries()) {
    const agent = await lab.addAgent(`e${index}`, { transport });
    const port = await freePort();
    await lab.publish(agent, 1, { target: `127.0.0.1:${be.port}`, port });
    await until(async () => (await roundTrip(port)).ok, `${transport} tunnel up`, 60000);
    tunnels.push({ id: agent.id, transport, agent, port });
  }
  await record({ kind: 'start', transports: TRANSPORTS, durationH: DURATION / 3600000, server: lab.stats() });

  const trip = async tunnel => {
    const result = await roundTrip(tunnel.port);
    current.trips++; totals.trips++;
    current.maxms = Math.max(current.maxms, result.ms);
    if (result.ms > 2000) current.slow++;
    if (!result.ok) {
      const kind = expected(tunnel.id) ? 'expected' : 'unexpected';
      current[kind]++; totals[kind]++;
      const why = `${tunnel.transport}:${result.why}`; current.why[why] = (current.why[why] ?? 0) + 1;
    }
  };
  const small = tunnels.flatMap(tunnel => Array.from({ length: WORKERS }, async () => {
    while (!stop) { await trip(tunnel); await sleep(50); }
  }));
  const bulk = (async () => {
    for (let i = 0; !stop; i++) {
      const tunnel = tunnels[i % tunnels.length], seed = 5000 + i;
      const [d, u] = await Promise.all([download(tunnel.port, seed, BULK), upload(tunnel.port, seed, BULK)]);
      const ok = d.ok && d.hash === digestOf(seed, BULK) && u.ok;
      current.bulk++; totals.bulk++;
      if (!ok && !expected(tunnel.id)) {
        current.bulkBad++; totals.bulkBad++;
        current.why[`${tunnel.transport}:bulk ${d.why}|${u.why}`] = (current.why[`${tunnel.transport}:bulk ${d.why}|${u.why}`] ?? 0) + 1;
      }
      await sleep(2000);
    }
  })();

  // One long-lived idle connection per tunnel must stay usable: echo a byte each IDLE_PROBE_S.
  // A close outside a disruption window means something reaped a healthy idle session.
  const idleStats = { opened: 0, probes: 0, probeBad: 0, unexpectedClose: 0, expectedClose: 0 };
  const openIdle = tunnel => new Promise(resolve => {
    const socket = connect(tunnel.port, '127.0.0.1'); let reply = '', ready = false;
    const closed = new Promise(done => socket.once('close', done));
    const timer = setTimeout(() => { socket.destroy(); resolve(null); }, 15000);
    socket.on('error', () => {});
    socket.once('connect', () => socket.write('ECHO\n'));
    socket.on('data', data => { reply += data; if (!ready && reply === 'READY\n') { ready = true; clearTimeout(timer); reply = ''; resolve({ socket, closed, read: () => { const r = reply; reply = ''; return r; } }); } });
    closed.then(() => { clearTimeout(timer); if (!ready) resolve(null); });
  });
  const keepIdle = tunnel => (async () => {
    while (!stop) {
      const held = await openIdle(tunnel);
      if (!held) { await sleep(5000); continue; }
      idleStats.opened++;
      while (!stop) {
        const outcome = await Promise.race([held.closed.then(() => 'closed'), sleep(IDLE_PROBE).then(() => 'probe'), stopping.then(() => 'stop')]);
        if (outcome === 'stop') break;
        if (outcome === 'closed') { idleStats[expected(tunnel.id) ? 'expectedClose' : 'unexpectedClose']++; break; }
        idleStats.probes++; held.socket.write('x');
        await Promise.race([held.closed, sleep(10000)]);
        if (held.read() !== 'x') { if (!expected(tunnel.id)) idleStats.probeBad++; held.socket.destroy(); break; }
      }
      if (!stop) await sleep(5000);
      held.socket.destroy();
    }
  })();
  const idle = tunnels.map(keepIdle);

  // Recovery is timed from the end of the disruption, such as SIGCONT after a pause.
  const recover = async (tunnel, label, action) => {
    await action();
    const t0 = Date.now();
    await until(async () => (await roundTrip(tunnel.port, '127.0.0.1', 5000)).ok, `${label} recovery`, 180000, 250);
    const event = { kind: 'event', event: label, transport: tunnel.transport, recoveryMs: Date.now() - t0 };
    totals.events.push(event); await record(event);
  };

  const schedule = (every, task) => (async () => {
    for (let next = every; !stop; next += every) {
      while (!stop && elapsed() < next) await sleep(1000);
      if (stop) return;
      try { await task(next / every); } catch (error) {
        const event = { kind: 'event', event: 'task-failed', error: error.message }; totals.events.push(event); await record(event);
      }
    }
  })();

  const tasks = [
    schedule(KILL, async n => {
      const tunnel = tunnels[n % tunnels.length];
      await recover(tunnel, 'agent-sigkill', async () => {
        disrupt(tunnel.id, 60000); await tunnel.agent.kill('SIGKILL'); await tunnel.agent.start();
      });
    }),
    schedule(PAUSE, async n => {
      const tunnel = tunnels[n % tunnels.length];
      await recover(tunnel, 'agent-sigstop-40s', async () => {
        disrupt(tunnel.id, 100000); tunnel.agent.child.kill('SIGSTOP'); await sleep(40000); tunnel.agent.child.kill('SIGCONT');
      });
    }),
    // Live configuration churn: add, then on the next run remove, a second tunnel on one agent.
    schedule(CONFIG, async n => {
      const tunnel = tunnels[Math.ceil(n / 2) % tunnels.length];
      if (n % 2) {
        const port = await freePort();
        await lab.publish(tunnel.agent, 2, { target: `127.0.0.1:${be.port}`, port });
        await until(async () => (await roundTrip(port, '127.0.0.1', 5000)).ok, 'added tunnel', 60000, 250);
      } else {
        await lab.batch([{ method: 'DELETE', path: `/api/managed/${tunnel.agent.id}/tunnels/2` }]);
      }
      await record({ kind: 'event', event: n % 2 ? 'tunnel-added' : 'tunnel-removed', transport: tunnel.transport });
    }),
    // A client that connects to the management port and never speaks must be closed.
    schedule(SILENT, async () => {
      const t0 = Date.now();
      const closedAfter = await new Promise(resolve => {
        const socket = connect(lab.adminPort, '127.0.0.1');
        const timer = setTimeout(() => { socket.destroy(); resolve(null); }, 60000);
        socket.on('error', () => {});
        socket.once('close', () => { clearTimeout(timer); resolve(Date.now() - t0); });
      });
      const event = { kind: 'event', event: 'silent-admin-connection', closedAfterMs: closedAfter };
      totals.events.push(event); await record(event);
    }),
    schedule(RESTART, async n => {
      const signal = n % 2 ? 'SIGTERM' : 'SIGKILL';
      disrupt('*', 120000);
      const t0 = Date.now();
      await lab.restart(signal);
      await Promise.all(tunnels.map(tunnel => until(async () => (await roundTrip(tunnel.port, '127.0.0.1', 5000)).ok, 'server restart recovery', 180000, 250)));
      const event = { kind: 'event', event: `server-${signal.toLowerCase()}`, recoveryMs: Date.now() - t0 };
      totals.events.push(event); await record(event);
    }),
  ];

  while (elapsed() < DURATION) {
    await sleep(SAMPLE);
    const sample = { kind: 'sample', server: lab.stats(), agents: Object.fromEntries(tunnels.map(t => [t.transport, t.agent.stats()])), ...current };
    current = window();
    await record(sample);
    console.log('[endurance]', JSON.stringify({ h: +(elapsed() / 3600000).toFixed(2), server: sample.server, trips: sample.trips, unexpected: sample.unexpected, bulkBad: sample.bulkBad }));
  }
  stop = true; stopped();
  await Promise.all([...small, bulk, ...tasks, ...idle]);

  const summary = { kind: 'summary', hours: DURATION / 3600000, ...totals, events: undefined, eventCount: totals.events.length,
    worstRecoveryMs: Math.max(0, ...totals.events.map(e => e.recoveryMs ?? 0)),
    silentClosedWithinMs: Math.max(0, ...totals.events.filter(e => e.event === 'silent-admin-connection').map(e => e.closedAfterMs ?? Infinity)),
    taskFailures: totals.events.filter(e => e.event === 'task-failed').length, idle: idleStats,
    serverLogWarnings: lab.logTail(100000).filter(line => /WARN|ERROR/.test(line)).length, samples: file };
  await record(summary);
  console.log('[endurance] DONE', JSON.stringify(summary));
} finally {
  stop = true; stopped(); be.close(); await lab.stop();
  await writeFile(join(OUT, `endurance-${started}-done.txt`), `${new Date().toISOString()}\n`);
}
// Pending probe timers would otherwise keep the driver alive for up to an hour.
process.exit(0);
