// How the system behaves when concurrent connections approach descriptor and stream limits,
// and whether it recovers once they are released. Runs three process configurations:
//   server-1024  server started with `ulimit -n 1024` (the default shell limit), agent unlimited
//   agent-1024   agent started with `ulimit -n 1024`, server unlimited
//   baseline     both unlimited (isolates the per-session max_streams limit)
// Usage: (ulimit -n 100000; LAB_BIN_DIR=... node fd-exhaustion.mjs [server-1024|agent-1024|baseline]...)
import { createServer, connect } from 'node:net';
import { writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, OUT, sleep, until, freePort, fdLimit } from '../lib/lab.mjs';

const LEVELS = (process.env.LEVELS ?? '100,400,800,1000,1200,2000').split(',').map(Number);
const CASES = { 'server-1024': { server: 1024, agent: 65536 }, 'agent-1024': { server: 65536, agent: 1024 }, baseline: { server: 65536, agent: 65536 } };
const wanted = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(CASES);

function echoBackend() {
  const sockets = new Set();
  const server = createServer(s => { sockets.add(s); s.on('data', d => s.write(d)); s.on('error', () => {}); s.on('close', () => sockets.delete(s)); });
  return new Promise(res => server.listen(0, '127.0.0.1', () => res({ port: server.address().port, close: () => { for (const s of sockets) s.destroy(); server.close(); } })));
}
/** Open a connection through the tunnel and prove a round trip works. */
function probe(port, hold) {
  return new Promise(res => {
    const t0 = Date.now(); const s = connect(port, '127.0.0.1'); let done = false;
    const finish = (ok, why) => { if (done) return; done = true; clearTimeout(timer); if (!ok || !hold) s.destroy(); res({ ok, why, ms: Date.now() - t0, socket: ok && hold ? s : null }); };
    const timer = setTimeout(() => finish(false, 'timeout'), 15000);
    s.on('connect', () => s.write('ping'));
    s.on('data', d => finish(d.toString() === 'ping', 'bad-echo'));
    s.on('error', e => finish(false, e.code ?? e.message));
    s.on('close', () => finish(false, 'closed'));
  });
}
async function openMany(port, count) {
  const out = { ok: 0, fail: 0, why: {}, sockets: [], p50: 0, p99: 0 }; const ms = [];
  let next = 0;
  await Promise.all(Array.from({ length: 64 }, async () => {
    while (next < count) { next++; const r = await probe(port, true); if (r.ok) { out.ok++; out.sockets.push(r.socket); ms.push(r.ms); } else { out.fail++; out.why[r.why] = (out.why[r.why] ?? 0) + 1; } }
  }));
  ms.sort((a, b) => a - b); out.p50 = ms[Math.floor(ms.length * 0.5)] ?? 0; out.p99 = ms[Math.floor(ms.length * 0.99)] ?? 0;
  return out;
}
async function consoleLatency(lab) { const t = Date.now(); try { await lab.overview(); return Date.now() - t; } catch (e) { return `ERR ${e.message}`; } }

const report = {};
await mkdir(OUT, { recursive: true });
for (const name of wanted) {
  const cfg = CASES[name]; const lines = []; const log = (...a) => { const l = a.join(' '); lines.push(l); console.log(`[${name}] ${l}`); };
  const lab = await Lab.start({ name, nofile: cfg.server });
  const backend = await echoBackend(); const publicPort = await freePort();
  try {
    log(`server fd limit (soft,hard) = ${fdLimit(lab.child.pid)}, baseline stats ${JSON.stringify(lab.stats())}`);
    const agent = await lab.addAgent('stress', { nofile: cfg.agent });
    await lab.publish(agent, 1, { target: `127.0.0.1:${backend.port}`, port: publicPort });
    await until(async () => (await probe(publicPort, false)).ok, 'tunnel up');
    log(`agent fd limit = ${fdLimit(agent.child.pid)}`);
    const held = []; const steps = [];
    for (const level of LEVELS) {
      const need = level - held.length; if (need <= 0) continue;
      const r = await openMany(publicPort, need); held.push(...r.sockets);
      const fresh = await probe(publicPort, false);
      const step = { target: level, held: held.length, newOk: r.ok, newFail: r.fail, why: r.why, p50ms: r.p50, p99ms: r.p99, freshConnOk: fresh.ok, freshWhy: fresh.why,
        console: await consoleLatency(lab), server: lab.stats(), agent: agent.stats(), agentOnline: await lab.online('stress').catch(() => 'ERR') };
      steps.push(step); log(JSON.stringify(step));
    }
    log(`releasing ${held.length} connections`);
    for (const s of held) s.destroy(); await sleep(8000);
    const after = { server: lab.stats(), agent: agent.stats(), console: await consoleLatency(lab), agentOnline: await lab.online('stress').catch(() => 'ERR'), fresh: [] };
    for (let i = 0; i < 20; i++) { const r = await probe(publicPort, false); after.fresh.push(r.ok ? 'ok' : r.why); }
    after.fresh = after.fresh.reduce((m, v) => (m[v] = (m[v] ?? 0) + 1, m), {});
    log(`after release: ${JSON.stringify(after)}`);
    log(`server log tail:\n  ${lab.logTail(8).map(l => l.slice(0, 220)).join('\n  ')}`);
    report[name] = { steps, after };
  } catch (e) { log(`FAILED: ${e.stack}`); report[name] = { error: e.message }; }
  finally { backend.close(); await lab.stop(); await writeFile(join(lab.dir, 'report.txt'), lines.join('\n')); }
}
await writeFile(join(OUT, `fd-exhaustion-${Date.now()}.json`), JSON.stringify(report, null, 2));
