// Backends and clients shared by the stress probes.
import { createServer, connect } from 'node:net';
import { createHash, randomUUID } from 'node:crypto';

/** Deterministic pseudo-random bytes so both ends can compute the expected digest. */
export function* stream(seed, total, chunk = 65536) {
  let x = (seed >>> 0) || 1, left = total;
  while (left > 0) {
    const n = Math.min(chunk, left), b = Buffer.allocUnsafe(n);
    for (let i = 0; i < n; i += 4) { x ^= x << 13; x >>>= 0; x ^= x >>> 17; x ^= x << 5; x >>>= 0; b.writeUInt32LE(x, Math.min(i, n - 4)); }
    left -= n; yield b;
  }
}
export function digestOf(seed, total) { const h = createHash('sha256'); for (const b of stream(seed, total)) h.update(b); return h.digest('hex'); }

/** Listens on loopback. Protocol per connection, first line: "ECHO", "GET <seed> <bytes>" or "PUT". */
export function backend({ bind = '127.0.0.1', port = 0 } = {}) {
  const identity = randomUUID();
  const sockets = new Set(); const uploads = new Map();
  let uploadSequence = 0;
  const server = createServer(s => {
    sockets.add(s); s.on('close', () => sockets.delete(s)); s.on('error', () => {});
    let head = Buffer.alloc(0), mode = null, hash, got = 0, uploadState;
    const receivedUpload = d => {
      hash.update(d); got += d.length;
      uploadState.bytes = got;
      uploadState.lastDataAt = Date.now();
    };
    s.on('data', d => {
      if (mode === 'echo') return void s.write(d);
      if (mode === 'echo1') return void s.end(d);
      if (mode === 'put') { receivedUpload(d); return; }
      head = Buffer.concat([head, d]); const nl = head.indexOf(10); if (nl < 0) return;
      const line = head.subarray(0, nl).toString(), rest = head.subarray(nl + 1); const [cmd, a, b] = line.split(' ');
      if (cmd === 'IDENTITY') { mode = 'identity'; s.end(`${identity}\n`); }
      else if (cmd === 'ECHO') { mode = 'echo'; s.write('READY\n'); if (rest.length) s.write(rest); }
      // ECHO1: answer one message, then close first (as an HTTP server would), so TIME_WAIT
      // lands on the fixed-port side and a single load generator does not run out of ports.
      else if (cmd === 'ECHO1') { mode = 'echo1'; s.write('READY\n'); }
      else if (cmd === 'GET') {
        mode = 'get'; const it = stream(+a, +b);
        const pump = () => { for (;;) { const n = it.next(); if (n.done) return void s.end(); if (!s.write(n.value)) return void s.once('drain', pump); } };
        pump();
      } else if (cmd === 'PUT') {
        mode = 'put'; hash = createHash('sha256');
        uploadState = { id: ++uploadSequence, bytes: 0, startedAt: Date.now(), lastDataAt: null, ended: false, closed: false };
        uploads.set(uploadSequence, uploadState);
        // Diagnostics retain only the latest 256 uploads, with no payload data.
        if (uploads.size > 256) uploads.delete(uploads.keys().next().value);
        if (rest.length) receivedUpload(rest);
        s.on('end', () => { uploadState.ended = true; uploadState.endedAt = Date.now(); s.end(`${got} ${hash.digest('hex')}\n`); });
        s.on('close', () => { uploadState.closed = true; });
      }
    });
  });
  return new Promise(res => server.listen(port, bind, () => res({ port: server.address().port, identity, connections: () => sockets.size,
    uploadProgress: () => Array.from(uploads.values(), state => ({ ...state })),
    close: () => { for (const s of sockets) s.destroy(); server.close(); } })));
}

const timeoutError = (what, ms) => Object.assign(new Error(`${what} timeout after ${ms}ms`), { code: 'TIMEOUT' });

export function backendIdentity(port, host = '127.0.0.1') {
  return new Promise(resolve => {
    const socket = connect(port, host); let reply = '';
    const finish = value => { clearTimeout(timer); socket.destroy(); resolve(value); };
    const timer = setTimeout(() => finish(null), 5000);
    socket.once('connect', () => socket.write('IDENTITY\n'));
    socket.on('data', data => { reply += data; if (reply.length > 100) finish(null); });
    socket.once('end', () => finish(reply.trim())); socket.on('error', () => finish(null));
    socket.once('close', () => finish(null));
  });
}

/** One short request/response through the tunnel. Resolves {ok, why, ms}. */
export function roundTrip(port, host = '127.0.0.1', ms = 15000) {
  return new Promise(res => {
    const t0 = Date.now(); const s = connect(port, host); let done = false, buf = '';
    const finish = (ok, why) => { if (done) return; done = true; clearTimeout(timer); s.destroy(); res({ ok, why, ms: Date.now() - t0 }); };
    const timer = setTimeout(() => finish(false, 'TIMEOUT'), ms);
    s.on('connect', () => s.write('ECHO1\n'));
    s.on('data', d => { buf += d; if (buf === 'READY\n') s.write('ping'); });
    // Success needs the echo and then an orderly close initiated by the far side.
    s.on('end', () => finish(buf === 'READY\nping', buf === 'READY\nping' ? '' : `BAD ${JSON.stringify(buf.slice(0, 20))}`));
    s.on('error', e => finish(false, e.code ?? e.message));
    s.on('close', () => finish(buf === 'READY\nping', 'CLOSED'));
  });
}
/** Download `bytes` and verify the digest. Optionally stall reading for `stallMs` after `stallAt` bytes. */
export function download(port, seed, bytes, { host = '127.0.0.1', stallAt = Infinity, stallMs = 0, ms = 120000, onStall } = {}) {
  return new Promise(res => {
    const t0 = Date.now(); const s = connect(port, host); const h = createHash('sha256'); let n = 0, done = false, stalled = false;
    const finish = (ok, why) => { if (done) return; done = true; clearTimeout(timer); s.destroy(); res({ ok, why, bytes: n, ms: Date.now() - t0, hash: h.digest('hex') }); };
    const timer = setTimeout(() => finish(false, 'TIMEOUT'), ms);
    s.on('connect', () => s.write(`GET ${seed} ${bytes}\n`));
    s.on('data', d => { h.update(d); n += d.length;
      if (!stalled && n >= stallAt) { stalled = true; s.pause(); (async () => { await onStall?.(); setTimeout(() => s.resume(), stallMs); })(); } });
    s.on('end', () => finish(n === bytes, n === bytes ? '' : `SHORT ${n}/${bytes}`));
    s.on('error', e => finish(false, e.code ?? e.message));
    s.on('close', () => finish(n === bytes, `CLOSED ${n}/${bytes}`));
  });
}
export function upload(port, seed, bytes, { host = '127.0.0.1', ms = 120000 } = {}) {
  return new Promise(res => {
    const t0 = Date.now(); const s = connect(port, host); let reply = '', done = false;
    const finish = (ok, why, extra = {}) => { if (done) return; done = true; clearTimeout(timer); s.destroy(); res({ ok, why, ms: Date.now() - t0, ...extra }); };
    const timer = setTimeout(() => finish(false, 'TIMEOUT'), ms);
    s.on('connect', () => { s.write('PUT\n'); const it = stream(seed, bytes);
      const pump = () => { for (;;) { const x = it.next(); if (x.done) return void s.end(); if (!s.write(x.value)) return void s.once('drain', pump); } }; pump(); });
    s.on('data', d => reply += d);
    s.on('error', e => finish(false, e.code ?? e.message));
    s.on('close', () => { const [got, hash] = reply.trim().split(' '); finish(+got === bytes && hash === digestOf(seed, bytes), `reply ${got}/${bytes}`, { got: +got }); });
  });
}
/** Run `workers` loops of `fn` for `seconds`, collecting outcomes and latencies. */
export async function loadFor(seconds, workers, fn) {
  const end = Date.now() + seconds * 1000; const r = { n: 0, ok: 0, why: {}, ms: [] };
  await Promise.all(Array.from({ length: workers }, async () => {
    while (Date.now() < end) { const x = await fn(); r.n++; if (x.ok) { r.ok++; r.ms.push(x.ms); } else r.why[x.why] = (r.why[x.why] ?? 0) + 1; }
  }));
  r.ms.sort((a, b) => a - b); const q = p => r.ms[Math.floor(r.ms.length * p)] ?? null;
  return { n: r.n, ok: r.ok, ratePerSec: Math.round(r.n / seconds), fail: r.n - r.ok, why: r.why, p50ms: q(0.5), p99ms: q(0.99), maxms: r.ms.at(-1) ?? null };
}
