// Verifies a server on a real public host with an agent behind this machine's NAT.
// The console is reached through a local port forward to the server's loopback listener,
// so its Origin check sees the address it was configured with.
//   FELUCCA_PROBE_HOST=<public ip> FELUCCA_PROBE_PASSWORD=<root password> \
//   [FELUCCA_PROBE_ADMIN=http://127.0.0.1:8990] [FELUCCA_PROBE_PORT=18081] \
//   node scripts/probes/probe-public-host.mjs
import { spawn } from 'node:child_process';
import { createServer, get as httpGet } from 'node:http';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomUUID } from 'node:crypto';
import assert from 'node:assert/strict';
import { buildService, deviceConfig, deviceOperations, randomKey, serviceDraft } from '../../apps/admin/src/features/product-flow.js';

const root = resolve(import.meta.dirname, '../..');
const output = resolve(root, process.env.FELUCCA_PROBE_OUTPUT ?? 'target/tmp/public-host-audit');
const host = process.env.FELUCCA_PROBE_HOST, password = process.env.FELUCCA_PROBE_PASSWORD;
const origin = process.env.FELUCCA_PROBE_ADMIN ?? 'http://127.0.0.1:8990';
const publicPort = Number(process.env.FELUCCA_PROBE_PORT ?? 18081);
if (!host || !password) throw new Error('set FELUCCA_PROBE_HOST and FELUCCA_PROBE_PASSWORD');
const agentBin = join(root, 'target/debug/felucca-agent');
await mkdir(output, { recursive: true, mode: 0o700 });

const checks = [];
let cookie, csrf, agent;
const wait = ms => new Promise(r => setTimeout(r, ms));
async function until(fn, label, timeout = 30000) {
  const end = Date.now() + timeout; let last;
  while (Date.now() < end) { try { const v = await fn(); if (v) return v; } catch (e) { last = e; } await wait(300); }
  throw new Error(`${label}: ${last?.message ?? 'timeout'}`);
}
async function api(path, method = 'GET', value, revision) {
  const response = await fetch(origin + path, { method, headers: { 'Content-Type': 'application/json', Origin: origin,
    ...(cookie ? { Cookie: cookie, 'X-Felucca-CSRF': csrf } : {}), ...(revision !== undefined ? { 'If-Match': `"${revision}"`, 'Idempotency-Key': randomUUID() } : {}) },
    body: value === undefined ? undefined : JSON.stringify(value), signal: AbortSignal.timeout(15000) });
  if (path === '/api/login') cookie = response.headers.get('set-cookie').split(';')[0];
  const result = await response.json(); if (!response.ok) throw new Error(`${path}: ${response.status} ${result.error}`); return result;
}
async function batch(operations) { const { revision } = await api('/api/overview'); return api('/api/batch', 'POST', { operations }, revision); }
function fetchPublic(path = '/') {
  return new Promise((res, rej) => {
    const req = httpGet({ host, port: publicPort, path, agent: false }, r => { let b = ''; r.on('data', d => b += d); r.on('end', () => res({ status: r.statusCode, body: b })); });
    req.on('error', rej); req.setTimeout(5000, () => req.destroy(new Error('HTTP timeout')));
  });
}

const payload = `felucca-public-host ${randomUUID()}\n`;
const backend = createServer((req, res) => { res.writeHead(200, { 'content-type': 'text/plain' }); res.end(payload); });
const id = `public-probe-${randomKey().slice(0, 8)}`, vkey = randomKey();
try {
  await new Promise(r => backend.listen(0, '127.0.0.1', r));
  const backendPort = backend.address().port;
  csrf = (await api('/api/login', 'POST', { account: 'root', password })).csrf;
  const launch = await api('/api/launch');
  checks.push(`console login through forwarded loopback; bridge ${launch.bridge.bind}, pinned fingerprint ${launch.fingerprint.slice(0, 12)}...`);

  await batch(deviceOperations(id, vkey, 'Public probe device', null));
  const config = deviceConfig({ id, key: vkey, host, launch });
  const file = join(output, 'agent.json'); await writeFile(file, JSON.stringify(config), { mode: 0o600 });
  const log = []; agent = spawn(agentBin, ['--config', file], { stdio: ['ignore', 'pipe', 'pipe'] });
  for (const s of [agent.stdout, agent.stderr]) s.on('data', d => log.push(d));
  await until(async () => (await api('/api/overview')).clients.some(c => c.id === id && c.online), 'device online', 45000);
  checks.push(`agent behind NAT connected to ${host} over pinned TLS and shows online`);

  const tunnel = buildService({ ...serviceDraft(), name: 'Public probe web', target: `127.0.0.1:${backendPort}`, port: publicPort, bind: '0.0.0.0' }, undefined, 1);
  await batch([{ method: 'PUT', path: `/api/managed/${id}/tunnels/1`, value: tunnel }]);
  const result = await until(async () => { const r = await fetchPublic(); return r.status === 200 && r; }, `public port ${publicPort} forwards`, 30000);
  assert.equal(result.body, payload);
  checks.push(`request to ${host}:${publicPort} from this machine returned the payload served by the local backend`);

  const big = await new Promise((res, rej) => { const req = httpGet({ host, port: publicPort, path: '/', agent: false }, r => { let n = 0; r.on('data', d => n += d.length); r.on('end', () => res(n)); }); req.on('error', rej); });
  assert.equal(big, Buffer.byteLength(payload)); checks.push('second request on a fresh connection also succeeds');

  await batch([{ method: 'PUT', path: `/api/managed/${id}/tunnels/1`, value: { ...tunnel, enabled: false } }]);
  await until(async () => { try { await fetchPublic(); return false; } catch { return true; } }, 'disabled service closes public port', 15000);
  checks.push('disabling the service closes the public port');
  await writeFile(join(output, 'agent.log'), Buffer.concat(log), { mode: 0o600 });
} finally {
  if (agent && agent.exitCode === null) agent.kill('SIGTERM');
  backend.close();
  try { if (cookie) await api(`/api/clients/${id}`, 'DELETE', undefined, (await api('/api/overview')).revision); } catch { /* best-effort cleanup of the probe device */ }
  await writeFile(join(output, 'result.json'), JSON.stringify({ host, publicPort, checks, at: new Date().toISOString() }, null, 2));
  console.log(checks.map(c => `ok  ${c}`).join('\n'));
}
