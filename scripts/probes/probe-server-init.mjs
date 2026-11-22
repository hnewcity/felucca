// Exercise the shipped --init entry point, its printed login and restart identity.
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';

const root = resolve(import.meta.dirname, '../..');
const output = resolve(root, process.env.FELUCCA_PROBE_OUTPUT ?? 'target/tmp/server-init');
await mkdir(output, { recursive: true, mode: 0o700 });
const binary = join(root, 'target/debug/felucca-server'), file = join(output, 'server.toml');
const children = [], checks = [];
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check) {
  const end = Date.now() + 30000;
  while (Date.now() < end) { try { const result = await check(); if (result) return result; } catch { /* Starting. */ } await delay(100); }
  throw new Error('Server did not become ready; inspect the private process log.');
}
function start(args, name) {
  const child = spawn(binary, args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
  child.text = ''; child.name = name; children.push(child);
  for (const stream of [child.stdout, child.stderr]) stream.on('data', data => { child.text += data; });
  child.on('error', error => { child.text += error.message; });
  return child;
}
async function stop(child) {
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  await until(() => child.exitCode !== null || child.signalCode !== null);
}
const report = { checks, limits: ['Local HTTP console and native debug process; no public deployment.'] };
try {
  const listener = createServer();
  await new Promise(resolve => listener.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${listener.address().port}`;
  await new Promise(resolve => listener.close(resolve));
  const first = start(['--init', '--config', file, '--admin-url', origin, '--lang', 'en'], 'initialized');
  const password = await until(() => first.text.match(/Initial login: root \/ (\S+)/)?.[1]);
  await until(async () => (await fetch(origin, { signal: AbortSignal.timeout(1000) })).ok);
  async function fingerprint() {
    const login = await fetch(origin + '/api/login', { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ account: 'root', password }) });
    assert.equal(login.status, 200);
    const response = await fetch(origin + '/api/launch', { headers: { Cookie: login.headers.get('set-cookie').split(';')[0], Origin: origin } });
    assert.equal(response.status, 200);
    return (await response.json()).fingerprint;
  }
  const original = await fingerprint();
  assert.equal(original.length, 64);
  const saved = await readFile(file, 'utf8');
  assert(!saved.includes(password));
  checks.push('actual --init creates a configuration, starts the console and prints a working login; plaintext password is not persisted');
  await stop(first);
  const second = start(['--config', file], 'restarted');
  await until(async () => (await fetch(origin, { signal: AbortSignal.timeout(1000) })).ok);
  assert.equal(await fingerprint(), original);
  checks.push('normal restart preserves the printed login and automatic bridge fingerprint');
  await stop(second);
  const repeated = start(['--init', '--config', file, '--admin-url', origin], 'repeated-init');
  await until(() => repeated.exitCode !== null);
  assert.notEqual(repeated.exitCode, 0);
  assert(!repeated.text.includes('Initial login:'));
  assert.equal(await readFile(file, 'utf8'), saved);
  checks.push('repeated initialization refuses existing data without changing configuration or printing a misleading new login');
  report.binary_sha256 = createHash('sha256').update(await readFile(binary)).digest('hex');
  report.status = 'passed';
} catch (error) {
  report.status = 'failed'; report.error = error.message; process.exitCode = 1;
} finally {
  for (const child of children.reverse()) {
    await stop(child).catch(() => child.kill('SIGKILL'));
    await writeFile(join(output, `${child.name}.log`), child.text, { mode: 0o600 });
  }
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
}
