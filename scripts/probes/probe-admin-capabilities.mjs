// Execute current editor functions without HTTP writes. The original audit
// result remains in admin-capability-functions.json as historical evidence.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tunnelEditor, resourceEditor } from "../../apps/admin/src/features/resource-editors.js";
import { agentTemplate, hasMissingCredentials } from "../../apps/admin/src/features/launch.js";
const root = fileURLToPath(new URL("../../", import.meta.url));
const t = (key) => key;
const values = (editor) => Object.fromEntries(editor.fields.map((field) => [field.key, field.value ?? ""]));
const backend = {addr:"backend.example:443",weight:3,tls:true,trust:{mode:"pinned",fingerprint:"ab".repeat(32)}};
const base = {id:7,mode:"tcp",public:true,enabled:true,port:19007,bind:"127.0.0.1",hosts:[],remark:"",backend:[backend],options:{}};
const checks=[];
async function check(name, run) { await run(); checks.push({name,passed:true}); }
await check("new backend supports weight TLS and pinned trust",async()=>{
  let sent; const editor=tunnelEditor(null,"/api/managed/example",(...args)=>{sent=args[2];},t);
  await editor.save({...values(editor),id:7,port:19007,backend:[backend]},editor.extra);
  assert.deepEqual(sent.backend,[backend]);
});
await check("changing address preserves TLS weight and trust",async()=>{
  let sent; const editor=tunnelEditor(base,"/api/managed/example",(...args)=>{sent=args[2];},t);
  const next={...backend,addr:"replacement.example:443"};
  await editor.save({...values(editor),backend:[next]},editor.extra);
  assert.deepEqual(sent.backend,[next]);
});
await check("backend advanced parameters can be edited",async()=>{
  let sent; const editor=tunnelEditor(base,"/api/managed/example",(...args)=>{sent=args[2];},t);
  const next={...backend,weight:9,tls:false,trust:{mode:"webpki"}};
  await editor.save({...values(editor),backend:[next]},editor.extra);
  assert.deepEqual(sent.backend,[next]);
});
await check("advanced routes health and payload options survive",async()=>{
  let sent; const editor=tunnelEditor(base,"/api/managed/example",(...args)=>{sent=args[2];},t);
  const extra={...editor.extra,balance:"failover",health:{type:"tcp",interval:"10s",timeout:"2s"},options:{payload:{compress:true}}};
  await editor.save(values(editor),extra);
  for(const key of Object.keys(extra))assert.deepEqual(sent[key],extra[key]);
});
await check("local services have a structured tunnel group editor",async()=>{
  const editor=resourceEditor("locals",{id:"office",config:{tunnel:[base]}},{role:"admin"},()=>{},false,t);
  assert.equal(editor.fields.find(field=>field.key==="config").descriptor,"LocalConfig");
});
await check("account token can be revoked without altering password",async()=>{
  let sent;const editor=resourceEditor("accounts",{id:"root",config:{role:"admin",user:null,password_hash:"<redacted>",token_hash:"<redacted>"}},{role:"admin"},(...args)=>{sent=args[2];},false,t);
  await editor.save({...values(editor),password:""},{token_hash:null});
  assert.equal(sent.token_hash,null);assert.equal(sent.password_hash,"<redacted>");
});
await check("agent template pins TLS and requires explicit credentials",async()=>{
  const config=agentTemplate({id:"office"},{bridge:{bind:"0.0.0.0:14028"},fingerprint:"ab".repeat(32)},"server.example");
  assert.equal(config.trust.mode,"pinned");assert.equal(hasMissingCredentials(config),true);
  config.vkey="test-config-only";assert.equal(hasMissingCredentials(config),false);
});
const source="apps/admin/src/features/resource-editors.js";
const output=resolve(root,process.env.FELUCCA_PROBE_OUTPUT||"docs/audit/admin-capability-functions-completed.json");
await mkdir(dirname(output),{recursive:true});
await writeFile(output,JSON.stringify({generated_at:new Date().toISOString(),method:"Current editor function execution; no HTTP, browser or forwarding runtime",source,sha256:createHash("sha256").update(await readFile(resolve(root,source))).digest("hex"),checks},null,2)+"\n");
console.log(`${checks.length} checks passed: ${output}`);
