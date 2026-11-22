// Bound the bridge in each direction without changing system networking.
// Verify whether bulk traffic leaves the device and independent requests usable.
import { createServer, connect } from 'node:net';
import { once } from 'node:events';
import { readFileSync } from 'node:fs';
import { writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { Lab, OUT, freePort, sleep, until } from './lib/lab.mjs';
import { backend, roundTrip, download, upload, digestOf } from './lib/net.mjs';

const sockets = new Set();
const rate = Number(process.env.BRIDGE_BYTES_PER_SECOND ?? 64000);
const bulkMiB = Number(process.env.BULK_MIB ?? 4);
const bulkDirection = process.env.BULK_DIRECTION ?? 'download';
if (!['download', 'upload'].includes(bulkDirection)) throw new Error('BULK_DIRECTION must be download or upload');
if (!Number.isInteger(rate) || rate < 1024 || rate > 1048576 || !Number.isInteger(bulkMiB) || bulkMiB < 1 || bulkMiB > 64) {
  throw new Error('Use BRIDGE_BYTES_PER_SECOND in 1024..1048576 and BULK_MIB in 1..64');
}
const chunkBytes = Math.max(1, Math.floor(rate / 64));
const lab = await Lab.start({ name: 'congested-bridge' });
const be = await backend();
const shim = createServer({ allowHalfOpen: true }, client => {
  const upstream = connect({ host: '127.0.0.1', port: lab.bridgePort, allowHalfOpen: true });
  for (const socket of [client, upstream]) {
    sockets.add(socket); socket.on('error', () => {});
    socket.once('close', () => sockets.delete(socket));
  }
  const pipe = async (source, destination) => {
    try {
      for await (const chunk of source) {
        for (let offset = 0; offset < chunk.length; offset += chunkBytes) {
          await sleep(1000 / 64);
          if (!destination.write(chunk.subarray(offset, offset + chunkBytes))) await once(destination, 'drain');
        }
      }
      destination.end();
    } catch { source.destroy(); destination.destroy(); }
  };
  void pipe(client, upstream); void pipe(upstream, client);
});
await new Promise(resolve => shim.listen(0, '127.0.0.1', resolve));
let result;
try {
  const agent = await lab.addAgent('limited', { patch: { server: `127.0.0.1:${shim.address().port}` } });
  const port = await freePort();
  await lab.publish(agent, 1, { target: `127.0.0.1:${be.port}`, port });
  await until(async () => (await roundTrip(port)).ok, 'forwarding ready');
  await sleep(3000);
  const warnings = () => readFileSync(agent.logFile, 'utf8').replace(/\x1b\[[0-9;]*m/g, '').split('\n').filter(line => /WARN|ERROR/.test(line));
  const bootstrapWarnings = warnings();
  let finished = false;
  const checks = [];
  const monitor = (async () => {
    while (!finished) {
      checks.push({ online: await lab.online(agent.id), request: await roundTrip(port, '127.0.0.1', 5000) });
      await sleep(500);
    }
  })();
  const size = bulkMiB * 1024 * 1024;
  const bulk = await (bulkDirection === 'download' ? download : upload)(port, 81, size, { ms: Math.ceil(size / rate * 2000 + 30000) });
  finished = true; await monitor;
  result = { bridgeBytesPerSecond: rate, chunkBytes, bulkMiB, bulkDirection, bulk: { ...bulk, ok: bulk.ok && (bulkDirection === 'upload' || bulk.hash === digestOf(81, size)) }, checks,
    bootstrapWarnings, agentWarnings: warnings().slice(bootstrapWarnings.length) };
  result.passed = result.bulk.ok && checks.length > 0 && checks.every(check => check.online && check.request.ok) && result.agentWarnings.length === 0;
  if (!result.passed) process.exitCode = 1;
  console.log(JSON.stringify(result));
} finally {
  for (const socket of sockets) socket.destroy();
  shim.close(); be.close(); await lab.stop();
  await mkdir(OUT, { recursive: true });
  if (result) await writeFile(join(OUT, 'congested-bridge.json'), JSON.stringify(result, null, 2));
}
