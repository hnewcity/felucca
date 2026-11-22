// Self-contained process lab for stress and failure-injection probes. It starts real
// felucca-server / felucca-agent processes on loopback with a chosen file-descriptor limit,
// drives the console API like the web UI does, and samples process resources.
//   LAB_BIN_DIR   directory with the binaries (default target/debug)
//   LAB_OUT       output directory (default target/tmp/stress-lab)
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdir, writeFile } from 'node:fs/promises';
import { openSync, closeSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID, webcrypto } from 'node:crypto';
import { buildService, deviceConfig, deviceOperations, randomKey, serviceDraft } from '../../../apps/admin/src/features/product-flow.js';

// Node 18 has no global `crypto`, which the console's key generator uses at call time.
globalThis.crypto ??= webcrypto;

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
export const BIN = process.env.LAB_BIN_DIR ?? join(root, 'target/debug');
export const OUT = resolve(process.env.LAB_OUT ?? join(root, 'target/tmp/stress-lab'));

export const sleep = ms => new Promise(r => setTimeout(r, ms));
export async function until(fn, label, timeout = 30000, step = 100) {
  const end = Date.now() + timeout; let last;
  while (Date.now() < end) { try { const v = await fn(); if (v) return v; } catch (e) { last = e; } await sleep(step); }
  throw new Error(`${label}: ${last?.message ?? 'timeout'}`);
}
export function freePort() {
  return new Promise((res, rej) => { const s = createServer(); s.once('error', rej); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => res(p)); }); });
}

/** Resident memory, thread and descriptor counts of a process (Linux /proc, else ps/lsof). */
export function procStats(pid) {
  try {
    const st = readFileSync(`/proc/${pid}/status`, 'utf8');
    return { rssMB: Math.round(+(/VmRSS:\s+(\d+)/.exec(st)?.[1] ?? 0) / 1024), threads: +(/Threads:\s+(\d+)/.exec(st)?.[1] ?? 0), fds: readdirSync(`/proc/${pid}/fd`).length };
  } catch {
    const r = spawnSync('sh', ['-c', `ps -o rss= -p ${pid}; lsof -p ${pid} 2>/dev/null | wc -l`], { encoding: 'utf8' });
    const [rss, fds] = r.stdout.trim().split(/\s+/).map(Number);
    return rss ? { rssMB: Math.round(rss / 1024), threads: null, fds } : null;
  }
}
export function fdLimit(pid) {
  try { return /Max open files\s+(\d+)\s+(\d+)/.exec(readFileSync(`/proc/${pid}/limits`, 'utf8')).slice(1).map(Number); } catch { return null; }
}

function launch(bin, args, { nofile, logFile }) {
  const fd = openSync(logFile, 'a', 0o600);
  let child;
  try {
    child = spawn('sh', ['-c', `ulimit -n ${nofile} || exit; exec "$0" "$@"`, bin, ...args], { stdio: ['ignore', fd, fd] });
  } finally { closeSync(fd); }
  child.exited = new Promise(r => child.once('exit', (code, signal) => r({ code, signal })));
  return child;
}
async function terminate(child, signal = 'SIGTERM', grace = 10000) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill(signal);
  const done = await Promise.race([child.exited, sleep(grace).then(() => null)]);
  if (!done) { child.kill('SIGKILL'); await child.exited; }
}

export class Lab {
  static async start({ name, nofile = 65536, bridge = {}, extra = {} } = {}) {
    const dir = join(OUT, `${name}-${Date.now()}`); await mkdir(dir, { recursive: true, mode: 0o700 });
    const bin = join(BIN, 'felucca-server');
    const password = randomKey();
    const hash = spawnSync(bin, ['--hash-password'], { input: password + '\n', encoding: 'utf8' }).stdout.trim();
    const adminPort = await freePort(), bridgePort = await freePort();
    const config = { bridge: { bind: `127.0.0.1:${bridgePort}`, ...bridge }, user: { team: {} },
      admin: { bind: `127.0.0.1:${adminPort}`, state_path: join(dir, 'runtime.json'), account: { root: { role: 'admin', password_hash: hash } } }, ...extra };
    const file = join(dir, 'server.json'); await writeFile(file, JSON.stringify(config), { mode: 0o600 });
    const lab = new Lab(); Object.assign(lab, { dir, bin, file, password, adminPort, bridgePort, nofile, origin: `http://127.0.0.1:${adminPort}`, agents: [] });
    await lab.boot(); return lab;
  }
  async boot() {
    this.child = launch(this.bin, ['--config', this.file], { nofile: this.nofile, logFile: join(this.dir, 'server.log') });
    await until(async () => { await fetch(this.origin, { signal: AbortSignal.timeout(2000) }); return true; }, 'admin listener', 30000, 200);
    this.cookie = undefined;
    this.csrf = (await this.api('/api/login', 'POST', { account: 'root', password: this.password })).csrf;
    this.launch = await this.api('/api/launch');
  }
  async api(path, method = 'GET', value, revision) {
    const response = await fetch(this.origin + path, { method, headers: { 'Content-Type': 'application/json', Origin: this.origin,
      ...(this.cookie ? { Cookie: this.cookie, 'X-Felucca-CSRF': this.csrf } : {}), ...(revision !== undefined ? { 'If-Match': `"${revision}"`, 'Idempotency-Key': randomUUID() } : {}) },
      body: value === undefined ? undefined : JSON.stringify(value), signal: AbortSignal.timeout(20000) });
    if (path === '/api/login') this.cookie = response.headers.get('set-cookie').split(';')[0];
    const result = await response.json(); if (!response.ok) { const e = new Error(`${path}: ${response.status} ${result.error}`); e.status = response.status; throw e; } return result;
  }
  async batch(operations) { const { revision } = await this.api('/api/overview'); return this.api('/api/batch', 'POST', { operations }, revision); }
  async overview() { return this.api('/api/overview'); }
  async online(id) { return (await this.overview()).clients.some(c => c.id === id && c.online); }
  stats() { return procStats(this.child.pid); }
  /** Register a device and start its agent; resolves once the console shows it online. */
  async addAgent(id, { transport = 'tls', nofile = 65536, patch = {}, waitOnline = true, managed = true } = {}) {
    const key = randomKey();
    const operations = deviceOperations(id, key, id, null);
    if (!managed) {
      operations.splice(1);
      operations[0].value.allow_config_push = true;
    }
    await this.batch(operations);
    const config = { ...deviceConfig({ id, key, host: '127.0.0.1', launch: this.launch }), transport, ...patch };
    const agent = new Agent(this, id, key, config, nofile); await agent.start();
    this.agents.push(agent);
    if (waitOnline) await until(() => this.online(id), `agent ${id} online`, 45000, 200);
    return agent;
  }
  async publish(agent, tunnelId, { target, port, bind = '127.0.0.1', extra = {} }) {
    const tunnel = { ...buildService({ ...serviceDraft(), name: `t${tunnelId}`, target, port, bind }, undefined, tunnelId), ...extra };
    await this.batch([{ method: 'PUT', path: `/api/managed/${agent.id}/tunnels/${tunnelId}`, value: tunnel }]);
    return tunnel;
  }
  async restart(signal = 'SIGTERM') { await terminate(this.child, signal); await this.boot(); }
  async stop() { await Promise.all(this.agents.map(a => a.stop())); await terminate(this.child); }
  logTail(lines = 15) { try { return readFileSync(join(this.dir, 'server.log'), 'utf8').replace(/\x1b\[[0-9;]*m/g, '').trim().split('\n').slice(-lines); } catch { return []; } }
}

export class Agent {
  constructor(lab, id, key, config, nofile) { Object.assign(this, { lab, id, key, config, nofile, file: join(lab.dir, `agent-${id}.json`), logFile: join(lab.dir, `agent-${id}.log`) }); }
  async start() {
    await writeFile(this.file, JSON.stringify(this.config), { mode: 0o600 });
    this.child = launch(join(BIN, 'felucca-agent'), ['--config', this.file], { nofile: this.nofile, logFile: this.logFile });
  }
  stats() { return procStats(this.child.pid); }
  kill(signal = 'SIGKILL') { this.child.kill(signal); return this.child.exited; }
  async stop() { await terminate(this.child); }
}
