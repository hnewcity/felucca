// Exercise accept() resource failures in disposable loopback-only processes.
// Rebuild binaries first. --expect-broken records a baseline without treating it as a pass.
import assert from 'node:assert/strict';
import { connect } from 'node:net';
import { request } from 'node:http';
import { request as tlsRequest } from 'node:https';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, BIN, OUT, sleep, until, freePort, fdLimit } from './lib/lab.mjs';
import { backend, roundTrip } from './lib/net.mjs';

const baseline = process.argv.includes('--expect-broken');
const limit = 128;
const dir = join(OUT, `listener-recovery-${Date.now()}`);
await mkdir(dir, { recursive: true, mode: 0o700 });
const cert = join(dir, 'cert.pem'), key = join(dir, 'key.pem');
const generated = spawnSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes',
  '-keyout', key, '-out', cert, '-days', '1', '-subj', '/CN=localhost'], { encoding: 'utf8' });
assert.equal(generated.status, 0, generated.stderr);
const report = {
  date: new Date().toISOString(), platform: process.platform, baseline, descriptorLimit: limit,
  binaries: Object.fromEntries(await Promise.all(['felucca-server', 'felucca-agent'].map(async name =>
    [name, createHash('sha256').update(await readFile(join(BIN, name))).digest('hex')]))),
  cases: [],
};

function httpCheck(port, secure = false, expectedStatus = 404, path = '/') {
  return new Promise(resolve => {
    const req = (secure ? tlsRequest : request)({ host: '127.0.0.1', port, path,
      agent: false, rejectUnauthorized: false, headers: { Host: `127.0.0.1:${port}` } }, res => {
      res.resume(); res.on('end', () => resolve(res.statusCode === expectedStatus));
      res.on('error', () => resolve(false));
    });
    req.on('error', () => resolve(false));
    req.setTimeout(1000, () => req.destroy()); req.end();
  });
}

async function pressure(port, held) {
  // Only the child has a low descriptor limit. The driver owns at most 256 sockets.
  for (let batch = 0; batch < 16; batch++) {
    await Promise.all(Array.from({ length: 16 }, () => new Promise(resolve => {
      const socket = connect(port, '127.0.0.1'); held.add(socket);
      const timer = setTimeout(() => { socket.destroy(); resolve(); }, 600);
      const done = () => { clearTimeout(timer); resolve(); };
      socket.once('connect', done); socket.on('error', done);
      socket.once('close', () => held.delete(socket));
    })));
    await sleep(25);
  }
  await sleep(400);
}
function release(held) { for (const socket of held) socket.destroy(); held.clear(); }

function exchange(socket, send, expected) {
  return new Promise(resolve => {
    if (socket.destroyed) return resolve(false);
    let received = '';
    const done = ok => { clearTimeout(timer); socket.off('data', data); socket.off('close', closed); resolve(ok); };
    const data = chunk => { received += chunk; if (received.length >= expected.length) done(received === expected); };
    const closed = () => done(false);
    const timer = setTimeout(() => done(false), 3000);
    socket.on('data', data); socket.once('close', closed); socket.write(send);
  });
}

for (const name of (process.env.LISTENER_CASES ?? 'tunnel,bridge,admin,http,https,visitor,shutdown').split(',')) {
  const held = new Set(); let lab, be, sentinel;
  const result = { name, rounds: [] }; report.cases.push(result);
  try {
    const httpPort = await freePort(), httpsPort = await freePort();
    lab = await Lab.start({ name: `listener-${name}`, nofile: name === 'visitor' ? 4096 : limit, extra: {
      http: { bind: `127.0.0.1:${httpPort}`, tls_bind: `127.0.0.1:${httpsPort}`, default_tls: { cert, key } },
    } });
    be = await backend(); const port = await freePort();
    const agent = await lab.addAgent('a1');
    await lab.publish(agent, 1, { target: `127.0.0.1:${be.port}`, port });
    await until(async () => (await roundTrip(port)).ok, 'initial forwarding');
    let visitor, visitPort;
    if (name === 'visitor') {
      const secret = randomUUID() + randomUUID(); visitPort = await freePort();
      await lab.publish(agent, 2, { target: `127.0.0.1:${be.port}`, port, extra: { public: false, port: 0, bind: null } });
      visitor = await lab.addAgent('guest', { nofile: limit, patch: {
        visitor: [{ name: 'visit', share: 'office', secret, mode: 'tcp', bind: `127.0.0.1:${visitPort}`, max_connections: 256 }],
      } });
      await lab.batch([{ method: 'PUT', path: '/api/shares/office', value: {
        client: 'a1', tunnel: 2, secret, allow_clients: ['guest'],
      } }]);
      await until(async () => (await roundTrip(visitPort)).ok, 'initial private forwarding');
    }
    result.actualLimit = fdLimit((visitor ?? { child: lab.child }).child.pid);
    const ingress = { tunnel: port, bridge: lab.bridgePort, admin: lab.adminPort,
      http: httpPort, https: httpsPort, visitor: visitPort, shutdown: lab.bridgePort }[name];
    if (['admin', 'http', 'https'].includes(name)) {
      assert.equal(await httpCheck(ingress, name === 'https', name === 'admin' ? 401 : 404,
        name === 'admin' ? '/api/session' : '/'), true, 'initial fresh HTTP connection');
    }
    sentinel = connect(name === 'visitor' ? visitPort : port, '127.0.0.1');
    sentinel.on('error', () => {});
    assert.equal(await exchange(sentinel, 'ECHO\n', 'READY\n'), true, 'initial held connection');
    for (let cycle = 0; cycle < (baseline || name === 'shutdown' ? 1 : 2); cycle++) {
      await pressure(ingress, held);
      const lines = visitor ? (await readFile(visitor.logFile, 'utf8')).split('\n') : lab.logTail(1000);
      const warnings = lines.filter(line => line.includes('TCP accept failed; retrying'));
      const round = { held: held.size, warnings: warnings.length, acceptErrors: warnings.slice(-3),
        atPressure: visitor ? visitor.stats() : lab.stats() };
      result.rounds.push(round);
      if (name === 'shutdown') {
        const started = Date.now(); await lab.stop();
        round.shutdownMs = Date.now() - started;
        round.exit = await lab.child.exited;
        assert.equal(round.exit.code, 0, 'shutdown must finish without forced killing');
      } else {
        release(held);
        const marker = randomUUID();
        round.existingConnectionPreserved = await exchange(sentinel, marker, marker);
        const started = Date.now();
        const check = name === 'http' || name === 'https' ? () => httpCheck(ingress, name === 'https')
          : name === 'admin' ? () => httpCheck(ingress, false, 401, '/api/session')
          : () => roundTrip(name === 'visitor' ? visitPort : port, '127.0.0.1', 500).then(r => r.ok);
        round.recovered = await until(check, 'automatic recovery', baseline ? 3500 : 12000, 100)
          .then(() => true, () => false);
        round.recoveryMs = Date.now() - started;
        if (name === 'bridge' && round.recovered) {
          await agent.kill('SIGKILL'); await agent.start();
          round.existingAgentReconnected = await until(() => lab.online('a1'), 'existing agent', 12000)
            .then(() => true, () => false);
          round.forwardingAfterReconnect = await until(async () => (await roundTrip(port, '127.0.0.1', 500)).ok,
            'forwarding after reconnect', 12000).then(() => true, () => false);
          const fresh = await lab.addAgent(`new${cycle}`, { waitOnline: false });
          round.newAgentConnected = await until(() => lab.online(fresh.id), 'new agent', 12000)
            .then(() => true, () => false);
          round.recovered = round.existingAgentReconnected && round.forwardingAfterReconnect && round.newAgentConnected;
        }
        if (!baseline) assert.equal(round.recovered, true, `${name} must recover without reconfiguration`);
        if (!baseline) assert.equal(round.existingConnectionPreserved, true, 'the original connection must survive');
        // Restarting the bridge agent intentionally closes its original streams.
        if (name === 'bridge' && cycle === 0 && !baseline) {
          sentinel.destroy(); sentinel = connect(port, '127.0.0.1'); sentinel.on('error', () => {});
          assert.equal(await exchange(sentinel, 'ECHO\n', 'READY\n'), true);
        }
      }
      if (!baseline) assert.ok(warnings.some(line => /Too many open files|os error (23|24)/i.test(line)),
        'the test must actually trigger descriptor exhaustion');
    }
  } catch (error) { result.error = error.message; }
  finally { release(held); sentinel?.destroy(); be?.close(); if (lab) await lab.stop(); }
  console.log(JSON.stringify(result));
  await writeFile(join(dir, 'result.json'), JSON.stringify(report, null, 2));
}
report.passed = !baseline && report.cases.every(result => !result.error);
await writeFile(join(dir, 'result.json'), JSON.stringify(report, null, 2));
console.log(`report: ${join(dir, 'result.json')}`);
if (!baseline && !report.passed) process.exitCode = 1;
