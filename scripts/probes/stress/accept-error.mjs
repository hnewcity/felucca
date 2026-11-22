// What happens to a listener when accept() fails (here: descriptor exhaustion, EMFILE)?
// Phase 1 targets a published tunnel port, phase 2 the bridge that agents connect to.
// For each: is the listener still bound, can service resume by itself, what does the console
// report, and what is the least disruptive way to recover.
import { connect } from 'node:net';
import { spawnSync } from 'node:child_process';
import { Lab, sleep, until, freePort } from '../lib/lab.mjs';
import { backend, roundTrip } from '../lib/net.mjs';

const LIMIT = Number(process.env.NOFILE ?? 256);
const log = (...a) => console.log('[accept-error]', ...a);
// Whether this process still has a listening socket on the port (lsof on macOS, ss on Linux).
const listening = (pid, port) => {
  const cmd = process.platform === 'linux' ? `ss -ltnH 'sport = :${port}' | wc -l` : `lsof -nP -a -p ${pid} -iTCP:${port} -sTCP:LISTEN 2>/dev/null | tail -n +2 | wc -l`;
  return spawnSync('sh', ['-c', cmd], { encoding: 'utf8' }).stdout.trim() === '1';
};
const hold = (port, n) => Promise.all(Array.from({ length: n }, () => new Promise(res => { const s = connect(port, '127.0.0.1'); s.on('connect', () => res(s)); s.on('error', () => res(null)); }))).then(a => a.filter(Boolean));
const out = { limit: LIMIT };

// Phase 1: published tunnel port.
{
  const lab = await Lab.start({ name: 'accept-error-tunnel', nofile: LIMIT }); const be = await backend(); const port = await freePort();
  try {
    const agent = await lab.addAgent('a1'); await lab.publish(agent, 1, { target: `127.0.0.1:${be.port}`, port });
    await until(async () => (await roundTrip(port)).ok, 'up');
    const before = { listening: listening(lab.child.pid, port), fds: lab.stats().fds };
    const held = []; for (let i = 0; i < Number(process.env.MAX_ITER ?? 40) && listening(lab.child.pid, port); i++) { held.push(...await hold(port, 10)); await sleep(150); }
    await sleep(1500);
    const afterFail = { listening: listening(lab.child.pid, port), heldBeforeFailure: held.length, fresh: (await roundTrip(port, '127.0.0.1', 3000)).why || 'ok', fds: lab.stats().fds,
      consoleSaysAgentOnline: await lab.online('a1'), consoleTunnelEnabled: (await lab.overview()).tunnels.map(t => t.spec?.enabled ?? t.enabled) };
    held.forEach(s => s.destroy()); await sleep(6000);
    const afterRelease = { listening: listening(lab.child.pid, port), fresh: (await roundTrip(port, '127.0.0.1', 3000)).why || 'ok' };
    // Operator recovery options.
    const tunnel = (await lab.api(`/api/managed/${agent.id}`)).config.tunnel[0];
    await lab.batch([{ method: 'PUT', path: `/api/managed/${agent.id}/tunnels/1`, value: { ...tunnel, enabled: false } }]); await sleep(500);
    await lab.batch([{ method: 'PUT', path: `/api/managed/${agent.id}/tunnels/1`, value: { ...tunnel, enabled: true } }]);
    const afterToggle = await until(async () => (await roundTrip(port, '127.0.0.1', 3000)).ok, 'toggle recovers', 10000, 300).then(() => 'recovered', () => 'still down');
    out.tunnelPort = { before, afterFail, afterRelease, recoveryByDisableEnable: afterToggle, serverLogMentionsIt: lab.logTail(200).some(l => /accept|listener|too many/i.test(l)) };
    log('tunnel port', JSON.stringify(out.tunnelPort));
  } catch (e) { out.tunnelPort = { error: e.stack }; log('FAILED', e.stack); } finally { be.close(); await lab.stop(); }
}

// Phase 2: bridge listener.
{
  const lab = await Lab.start({ name: 'accept-error-bridge', nofile: LIMIT }); const be = await backend(); const port = await freePort();
  try {
    const a1 = await lab.addAgent('a1'); await lab.publish(a1, 1, { target: `127.0.0.1:${be.port}`, port });
    await until(async () => (await roundTrip(port)).ok, 'up');
    const held = []; for (let i = 0; i < Number(process.env.MAX_ITER ?? 60) && listening(lab.child.pid, lab.bridgePort); i++) { held.push(...await hold(lab.bridgePort, 10)); await sleep(150); }
    await sleep(1500);
    const stillListening = listening(lab.child.pid, lab.bridgePort);
    held.forEach(s => s.destroy()); await sleep(15000);
    const afterRelease = { bridgeListening: listening(lab.child.pid, lab.bridgePort), consoleOk: await lab.overview().then(() => true, () => false) };
    // A well-behaved agent that was connected (a1) drops and comes back, as after a network blip.
    await a1.kill('SIGKILL'); await sleep(1000); await a1.start();
    const a1Back = await until(() => lab.online('a1'), 'a1 reconnect', 20000, 300).then(() => true, () => false);
    const a2 = await lab.addAgent('a2', { waitOnline: false }); const newAgent = await until(() => lab.online('a2'), 'a2', 20000, 300).then(() => true, () => false);
    out.bridge = { heldWhenListenerState: held.length, bridgeStillListeningAtPeak: stillListening, afterRelease, existingAgentReconnects: a1Back, brandNewAgentConnects: newAgent };
    log('bridge', JSON.stringify(out.bridge));
  } catch (e) { out.bridge = { error: e.stack }; log('FAILED', e.stack); } finally { be.close(); await lab.stop(); }
}
console.log(JSON.stringify(out, null, 2));
