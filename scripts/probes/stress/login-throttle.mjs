// Login throttling as seen by a console behind a reverse proxy: the throttle keys on the TCP
// peer, so every visitor behind one proxy shares the same budget unless the forwarded address
// is honoured. Measures the window length as well.
import { Lab, sleep } from '../lib/lab.mjs';

const lab = await Lab.start({ name: 'login-throttle' });
const login = (password, xff) => fetch(lab.origin + '/api/login', { method: 'POST',
  headers: { 'Content-Type': 'application/json', Origin: lab.origin, ...(xff ? { 'X-Forwarded-For': xff } : {}) },
  body: JSON.stringify({ account: 'root', password }) }).then(r => r.status);
const out = {};
try {
  const attacker = []; for (let i = 0; i < 12; i++) attacker.push(await login('wrong' + i, '203.0.113.9'));
  out.attackerStatuses = attacker.join(',');
  out.legitimateUserOtherForwardedAddress = await login(lab.password, '198.51.100.7');
  out.legitimateUserNoHeader = await login(lab.password);
  const t0 = Date.now(); let status = 429;
  while (status === 429 && Date.now() - t0 < 90000) { await sleep(2000); status = await login(lab.password, '198.51.100.7'); }
  out.secondsUntilLegitimateLoginWorks = +((Date.now() - t0) / 1000).toFixed(0); out.finalStatus = status;
  // The attacker can simply keep the window full.
  const again = []; for (let i = 0; i < 10; i++) again.push(await login('x' + i, '203.0.113.9'));
  out.afterRefill = await login(lab.password, '198.51.100.7');
} finally { await lab.stop(); }
console.log(JSON.stringify(out, null, 2));
