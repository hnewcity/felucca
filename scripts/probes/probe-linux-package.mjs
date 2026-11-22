// Run inside a disposable Linux container, beside an unmodified downloaded package.
// The backend is loopback-only and serves synthetic data; no host service is installed.
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

const folder = resolve(process.argv[2]);
const backend = createServer((_request, response) => {
  response.writeHead(200, { 'Content-Type': 'text/plain' });
  response.end('felucca-linux-package\n');
});
await new Promise((resolve, reject) => {
  backend.once('error', reject);
  backend.listen(23880, '127.0.0.1', resolve);
});
const child = spawn('sh', [join(folder, 'connect.sh')], {
  cwd: folder, stdio: ['ignore', 'inherit', 'inherit'],
});
await writeFile(join(folder, 'linux-launch.json'), JSON.stringify({
  platform: process.platform, architecture: process.arch,
  launcher: 'sh connect.sh', backend: '127.0.0.1:23880',
  configuration_edited: false, system_service_installed: false,
}, null, 2) + '\n', { mode: 0o600 });
let stopping = false;
function stop() {
  stopping = true;
  child.kill('SIGTERM');
  backend.close();
}
process.once('SIGTERM', stop);
process.once('SIGINT', stop);
child.once('error', error => { console.error(error.message); process.exitCode = 1; stop(); });
child.once('exit', code => { if (!stopping) process.exitCode = code || 1; backend.close(); });
