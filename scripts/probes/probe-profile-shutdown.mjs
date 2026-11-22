// Real-process coverage for cancellation while the profiler owns its signal handler.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, OUT, sleep } from './lib/lab.mjs';

const lab = await Lab.start({ name: 'profile-shutdown' });
const report = { platform: process.platform, rounds: [] };
const profile = seconds => fetch(lab.origin + '/api/profile', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json', Origin: lab.origin, Cookie: lab.cookie, 'X-Felucca-CSRF': lab.csrf },
  body: JSON.stringify({ seconds }), signal: AbortSignal.timeout(5000),
});
try {
  const { config } = await lab.api('/api/config');
  config.admin.profiling = true;
  await lab.batch([{ method: 'PUT', path: '/api/config', value: config }]);
  for (let round = 0; round < 3; round++) {
    const pending = profile(30).then(response => response.text(), () => null);
    await sleep(200);
    const concurrent = await profile(1);
    assert.equal(concurrent.status, 409, 'the long-running sampler must actually hold the process lock');
    await concurrent.text();
    const started = Date.now();
    await lab.stop();
    const shutdownMs = Date.now() - started;
    const exit = await lab.child.exited;
    await pending;
    assert.equal(exit.code, 0);
    assert.ok(shutdownMs < 2000, 'shutdown must cancel and join the sampler promptly');
    await lab.boot();
    const response = await profile(1);
    const svg = await response.text();
    assert.equal(response.status, 200);
    assert.ok(svg.includes('<svg'));
    report.rounds.push({ shutdownMs, exit, subsequentProfileStatus: response.status, svg: true });
  }
  report.passed = true;
} finally {
  await lab.stop();
  await writeFile(join(OUT, 'profile-shutdown.json'), JSON.stringify(report, null, 2));
}
console.log(JSON.stringify(report));
