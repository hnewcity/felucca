// Many devices at once: registration, connection, per-device tunnels, reconnect storms and
// console responsiveness at scale. One agent process carries GROUP device configs
// (felucca-agent accepts repeated --config), which keeps memory use realistic for a small host.
//   FLEET=200 GROUP=25 node fleet.mjs
import { spawn } from 'node:child_process';
import { openSync, closeSync } from 'node:fs';
import { writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, OUT, BIN, sleep, until, freePort, fdLimit } from '../lib/lab.mjs';
import { backend, roundTrip, download, digestOf } from '../lib/net.mjs';

const N = Number(process.env.FLEET ?? 100), GROUP = Number(process.env.GROUP ?? 25);
if (!Number.isInteger(N) || N < 1 || !Number.isInteger(GROUP) || GROUP < 1 || GROUP > 32)
  throw new Error('FLEET must be positive; GROUP must be 1..32 (the CLI instance limit)');
const MB = 1024 * 1024;
const lab = await Lab.start({ name: `fleet${N}`, nofile: Number(process.env.SERVER_NOFILE ?? 65536) });
const be = await backend();
const log = (...a) => console.log(`[fleet ${N}]`, ...a);
const results = { devices: N, group: GROUP, serverDescriptorLimit: fdLimit(lab.child.pid) };
const keyOf = new Map(), ports = new Map(), procs = [];

const onlineCount = async () => (await lab.overview()).clients.filter(c => c.online && c.id.startsWith('d')).length;
const time = async fn => { const t = Date.now(); const v = await fn(); return [Date.now() - t, v]; };
async function startAgents(ids) {
  const { deviceConfig } = await import('../../../apps/admin/src/features/product-flow.js');
  for (let i = 0; i < ids.length; i += GROUP) {
    const files = [];
    for (const id of ids.slice(i, i + GROUP)) {
      const f = join(lab.dir, `agent-${id}.json`);
      await writeFile(f, JSON.stringify(deviceConfig({ id, key: keyOf.get(id), host: '127.0.0.1', launch: lab.launch })), { mode: 0o600 }); files.push(f);
    }
    const fd = openSync(join(lab.dir, `agents-${i / GROUP}.log`), 'a', 0o600);
    const child = spawn(join(BIN, 'felucca-agent'), files.flatMap(f => ['--config', f]), { stdio: ['ignore', fd, fd] });
    closeSync(fd);
    child.exited = new Promise(r => child.once('exit', (code, signal) => r({ code, signal }))); procs.push(child);
  }
}
async function waitAll(label, timeout = 240000) {
  const t0 = Date.now(); let last = 0;
  while (Date.now() - t0 < timeout) {
    if (procs.some(p => p.exitCode !== null || p.signalCode !== null)) throw new Error('an agent process exited before all devices came online');
    last = await onlineCount().catch(() => last); if (last >= N) return { ms: Date.now() - t0, online: last }; await sleep(500);
  }
  throw new Error(`only ${last}/${N} devices came online within ${timeout}ms`);
}

try {
  const { randomKey, buildService, serviceDraft, deviceOperations } = await import('../../../apps/admin/src/features/product-flow.js');
  const ids = Array.from({ length: N }, (_, i) => `d${String(i).padStart(4, '0')}`);
  // 1. Registration in one batch, as the console does for bulk changes.
  const ops = []; for (const id of ids) { keyOf.set(id, randomKey()); ops.push(...deviceOperations(id, keyOf.get(id), id, null)); }
  const [regMs] = await time(async () => { for (let i = 0; i < ops.length; i += 100) await lab.batch(ops.slice(i, i + 100)); });
  results.registerMs = regMs; log(`registered ${N} devices in ${regMs}ms; server ${JSON.stringify(lab.stats())}`);

  // 2. Everyone connects at once.
  await startAgents(ids);
  const conn = await waitAll(); results.connectAll = conn; log(`all-online: ${JSON.stringify(conn)}; server ${JSON.stringify(lab.stats())}`);

  // 3. A public tunnel per device, then prove every one forwards.
  const tops = [], allocated = new Set();
  for (const id of ids) { let p; do { p = await freePort(); } while (allocated.has(p)); allocated.add(p); ports.set(id, p); tops.push({ method: 'PUT', path: `/api/managed/${id}/tunnels/1`, value: buildService({ ...serviceDraft(), name: id, target: `127.0.0.1:${be.port}`, port: p, bind: '127.0.0.1' }, undefined, 1) }); }
  const [tunMs] = await time(async () => { for (let i = 0; i < tops.length; i += 100) await lab.batch(tops.slice(i, i + 100)); });
  const check = async () => { const bad = []; await Promise.all([...ports].map(async ([id, p]) => { const r = await until(async () => { const x = await roundTrip(p, '127.0.0.1', 5000); return x.ok ? x : null; }, id, 60000, 300).catch(() => null); if (!r) bad.push(id); })); return bad; };
  const [verifyMs, bad] = await time(check);
  results.tunnels = { createMs: tunMs, allForwardingAfterMs: verifyMs, notForwarding: bad.length }; log(`tunnels: ${JSON.stringify(results.tunnels)}; server ${JSON.stringify(lab.stats())}`);

  // 4. Console responsiveness at this size.
  const lat = []; for (let i = 0; i < 30; i++) { const t = Date.now(); await lab.overview(); lat.push(Date.now() - t); await sleep(50); }
  lat.sort((a, b) => a - b); results.overviewMs = { p50: lat[15], max: lat.at(-1) };
  const [cfgMs] = await time(() => lab.api('/api/config')); results.configMs = cfgMs; log(`console: overview ${JSON.stringify(results.overviewMs)}, config ${cfgMs}ms`);

  // 5. Traffic from every device concurrently while measuring integrity.
  const t0 = Date.now(); const dl = await Promise.all([...ports].slice(0, 40).map(([, p], i) => download(p, 500 + i, 8 * MB)));
  results.parallelDownload = { devices: dl.length, failed: dl.filter((d, i) => !d.ok || d.hash !== digestOf(500 + i, 8 * MB)).length, seconds: +((Date.now() - t0) / 1000).toFixed(1) }; log(`download across devices: ${JSON.stringify(results.parallelDownload)}`);

  // 6. Reconnect storms: graceful and crash restarts of the server.
  for (const sig of ['SIGTERM', 'SIGKILL']) {
    const t = Date.now(); await lab.restart(sig); const upMs = Date.now() - t;
    const back = await waitAll(); const [fwdMs, notFwd] = await time(check);
    results[`storm-${sig}`] = { serverUpMs: upMs, allBack: back, forwardingAfterMs: fwdMs, notForwarding: notFwd.length, server: lab.stats() }; log(`${sig} restart: ${JSON.stringify(results[`storm-${sig}`])}`);
  }
  // 7. Agent processes crash and come back (half the fleet).
  const half = procs.slice(0, Math.ceil(procs.length / 2)); for (const p of half) p.kill('SIGKILL'); await Promise.all(half.map(p => p.exited));
  await sleep(3000); const mid = await onlineCount(); log(`after killing ${half.length} agent processes online=${mid}`);
  const killed = ids.slice(0, half.length * GROUP); procs.splice(0, half.length); await startAgents(killed);
  const rec = await waitAll(); results.halfFleetCrash = { onlineWhileDown: mid, recovered: rec }; log(`half-fleet crash recovery: ${JSON.stringify(rec)}`);
  results.logWarnings = lab.logTail(400).filter(l => /WARN|ERROR/.test(l)).slice(-8).map(l => l.slice(0, 200));
} catch (e) { log('FAILED', e.stack); results.error = e.message; }
finally {
  for (const p of procs) p.kill('SIGTERM'); be.close(); await lab.stop();
  await Promise.all(procs.map(p => p.exited));
  await mkdir(OUT, { recursive: true }); await writeFile(join(OUT, `fleet-${N}-${Date.now()}.json`), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
}
