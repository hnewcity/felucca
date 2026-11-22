// Visitor-side load generator, meant to run on the server host so that only the agent's leg
// crosses the real network. Prints one JSON object.
//   node visit.mjs latency <port> [count]
//   node visit.mjs throughput <port> [mb] [parallel] [parallel-mb]
//   node visit.mjs concurrency <port> <workers> <seconds>
import { roundTrip, download, upload, digestOf, loadFor, backendIdentity } from './net.mjs';

const [mode, portArg, a, b, c] = process.argv.slice(2); const port = Number(portArg), MB = 1024 * 1024;
const q = (xs, p) => xs[Math.min(xs.length - 1, Math.floor(xs.length * p))];
let out;
if (mode === 'identity') {
  out = { identity: await backendIdentity(port) };
} else if (mode === 'latency') {
  const n = Number(a ?? 200), ms = [], first = [], why = {}; let bad = 0;
  for (let i = 0; i < n; i++) { const r = await roundTrip(port, '127.0.0.1', 10000); if (first.length < 10) first.push(r.ok ? r.ms : `${r.why}@${r.ms}`); if (r.ok) ms.push(r.ms); else { bad++; why[r.why] = (why[r.why] ?? 0) + 1; } }
  ms.sort((x, y) => x - y); out = { n, failed: bad, why, first, min: ms[0], p50: q(ms, 0.5), p95: q(ms, 0.95), p99: q(ms, 0.99), max: ms.at(-1) };
} else if (mode === 'throughput') {
  const size = Number(a ?? 64) * MB, par = Number(b ?? 8), parallelMiB = Number(c ?? 16);
  const d = await download(port, 31, size, { ms: 300000 }), u = await upload(port, 32, size, { ms: 300000 });
  const t = Date.now(); const ds = await Promise.all(Array.from({ length: par }, (_, i) => download(port, 40 + i, parallelMiB * MB, { ms: 300000 }))); const dt = (Date.now() - t) / 1000;
  const t2 = Date.now(); const us = await Promise.all(Array.from({ length: par }, (_, i) => upload(port, 60 + i, parallelMiB * MB, { ms: 300000 }))); const ut = (Date.now() - t2) / 1000;
  out = { singleDownMBps: d.ok && d.hash === digestOf(31, size) ? +(size / MB / (d.ms / 1000)).toFixed(3) : `FAIL ${d.why}`, singleUpMBps: u.ok ? +(size / MB / (u.ms / 1000)).toFixed(3) : `FAIL ${u.why}`,
    parallelDownMBps: +(par * parallelMiB / dt).toFixed(3), parallelDownFailed: ds.filter((x, i) => !x.ok || x.hash !== digestOf(40 + i, parallelMiB * MB)).length,
    parallelUpMBps: +(par * parallelMiB / ut).toFixed(3), parallelUpFailed: us.filter(x => !x.ok).length };
} else if (mode === 'concurrency') {
  out = await loadFor(Number(b ?? 20), Number(a ?? 50), () => roundTrip(port, '127.0.0.1', 15000));
} else throw new Error('unknown mode');
console.log(JSON.stringify(out));
