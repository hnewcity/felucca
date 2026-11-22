// Long run under steady mixed load, sampling server and agent resources to expose slow leaks.
//   DURATION_S=1500 WORKERS=8 node soak.mjs
import { writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, OUT, sleep, until, freePort } from '../lib/lab.mjs';
import { backend, roundTrip, download, upload, digestOf, loadFor } from '../lib/net.mjs';

const DURATION = Number(process.env.DURATION_S ?? 1500), WORKERS = Number(process.env.WORKERS ?? 8), SAMPLE = Number(process.env.SAMPLE_S ?? 30);
const MB = 1024 * 1024;
const lab = await Lab.start({ name: 'soak' }); const be = await backend(); const port = await freePort();
const samples = [], failures = { roundTrip: {}, bulk: {} }; let bulkRuns = 0, bulkBad = 0, trips = 0, tripBad = 0;
const t0 = Date.now(); let stop = false, summary;
try {
  const agent = await lab.addAgent('a1'); await lab.publish(agent, 1, { target: `127.0.0.1:${be.port}`, port });
  await until(async () => (await roundTrip(port)).ok, 'up');
  const sampler = (async () => { while (!stop) { samples.push({ t: Math.round((Date.now() - t0) / 1000), server: lab.stats(), agent: agent.stats(), trips, tripBad, bulkRuns, bulkBad }); console.log('[soak]', JSON.stringify(samples.at(-1))); await sleep(SAMPLE * 1000); } })();
  const small = loadFor(DURATION, WORKERS, async () => { const r = await roundTrip(port); trips++; if (!r.ok) { tripBad++; failures.roundTrip[r.why] = (failures.roundTrip[r.why] ?? 0) + 1; } await sleep(20); return r; });
  const bulk = (async () => { let i = 0; while (Date.now() - t0 < DURATION * 1000) { const seed = 1000 + i++; const size = (i % 3 + 1) * MB;
    const [d, u] = await Promise.all([download(port, seed, size), upload(port, seed, size)]);
    bulkRuns++; const ok = d.ok && d.hash === digestOf(seed, size) && u.ok; if (!ok) { bulkBad++; failures.bulk[`${d.why}|${u.why}`] = (failures.bulk[`${d.why}|${u.why}`] ?? 0) + 1; } await sleep(300); } })();
  summary = await small; await bulk; stop = true; await sampler;
  const first = samples[2] ?? samples[0], last = samples.at(-1);
  console.log('[soak] DONE', JSON.stringify({ summary, trips, tripBad, bulkRuns, bulkBad, failures, rssServerMB: [first.server.rssMB, last.server.rssMB], rssAgentMB: [first.agent.rssMB, last.agent.rssMB], fdsServer: [first.server.fds, last.server.fds], fdsAgent: [first.agent.fds, last.agent.fds], serverLogWarnings: lab.logTail(300).filter(l => /WARN|ERROR/.test(l)).length }));
} finally { stop = true; be.close(); await lab.stop(); await mkdir(OUT, { recursive: true }); await writeFile(join(OUT, `soak-${Date.now()}.json`), JSON.stringify({ summary, trips, tripBad, bulkRuns, bulkBad, samples, failures }, null, 2)); }
