import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, readdir, rm } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { respondFromFictionalAccount } from './cpa-upstream-fixture.mjs';

const binary = resolve(process.argv[2] || join(process.env.CARGO_TARGET_DIR || 'src-tauri/target', 'debug/autojev'));
const cpa = resolve(process.argv[3] || '../cpa-pinned');
const root = await mkdtemp(join(tmpdir(), 'jev-r1-desktop-'));
console.log(`Owned desktop fixture: ${root}`);
const receipts = [];
const cancelled = [];
const modes = new Map();
const fixture = createServer(async (req, res) => {
  res.setHeader('access-control-allow-origin', '*');
  res.setHeader('access-control-allow-headers', 'content-type');
  res.setHeader('access-control-allow-methods', 'POST, OPTIONS');
  if (req.method === 'OPTIONS') { res.writeHead(204); res.end(); return; }
  let text = ''; for await (const part of req) text += part;
  const body = JSON.parse(text || '{}');
  const reply = value => { res.writeHead(200, {'content-type':'application/json'}); res.end(JSON.stringify(value)); };
  if (req.url === '/__progress') { console.log(body.name); reply({ok:true}); return; }
  if (req.url === '/__receipts') { reply({ requests: receipts, cancelled }); return; }
  if (req.url === '/__mode') { modes.set(body.account, body.mode); reply({ok:true}); return; }
  if (req.url === '/__gateway') {
    assert.ok(Number.isInteger(body.port) && body.port > 0 && ![9526,9527,11434].includes(body.port));
    assert.ok(body.model.startsWith('autojev/model/'));
    const abort = new AbortController();
    res.on('close', () => abort.abort());
    const timer = setTimeout(() => abort.abort(), 5000);
    try {
      const response = await fetch(`http://127.0.0.1:${body.port}/v1/chat/completions`, { method:'POST', headers:{'content-type':'application/json'}, body:JSON.stringify({model:body.model,messages:[{role:'user',content:body.prompt}],max_tokens:4,stream:!!body.stream}), signal:abort.signal });
      const content = await response.text();
      reply({status:response.status,body:content,model:response.headers.get('x-autojev-model')});
    } catch (error) { if (!res.destroyed) reply({status:599,error:String(error)}); }
    finally { clearTimeout(timer); }
    return;
  }
  const account = req.url.split('/')[1];
  const prompt = body.messages?.[0]?.content;
  assert.ok(['a1','a2','b1','paid'].includes(account), `Unexpected fixture route ${req.url}`);
  receipts.push({ path: req.url, account, key: req.headers.authorization, model: body.model, prompt, stream:!!body.stream });
  res.on('close', () => { if (!res.writableEnded) cancelled.push(prompt); });
  if (prompt?.startsWith('hang-')) {
    if (body.stream) {
      res.writeHead(200, {'content-type':'text/event-stream'});
      res.write('data: '+JSON.stringify({id:'fixture',object:'chat.completion.chunk',model:'same-model',choices:[{index:0,delta:{role:'assistant',content:'first'},finish_reason:null}]})+'\n\n');
    }
    return;
  }
  const mode = modes.get(account) || 'ok';
  respondFromFictionalAccount(res, account, !!body.stream, mode);
});
fixture.listen(0,'127.0.0.1'); await once(fixture,'listening');
const base = `http://127.0.0.1:${fixture.address().port}`;
const reservation = createServer(); reservation.listen(0,'127.0.0.1'); await once(reservation,'listening');
const uiPort = reservation.address().port; await new Promise(r=>reservation.close(r));
const environment = Object.fromEntries(['PATH','TMPDIR','LANG','LC_ALL'].filter(k=>process.env[k]).map(k=>[k,process.env[k]]));
const vite = spawn(process.execPath,[resolve('node_modules/vite/bin/vite.js'),'--host','127.0.0.1','--port',String(uiPort),'--strictPort'],{env:environment,stdio:'pipe'});
let uiLog='';vite.stdout.on('data',b=>uiLog+=b);vite.stderr.on('data',b=>uiLog+=b);
let desktop;
const alive = pid => { try { process.kill(pid,0); return true; } catch(error) { if(error.code==='ESRCH')return false;throw error; } };
const terminateDesktop = () => { if(desktop?.pid) { try { process.kill(-desktop.pid,'SIGTERM'); } catch(error) { if(error.code!=='ESRCH')throw error; } } };
const sentinel = spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{env:environment,stdio:'ignore'});
const run = async reload => {
  modes.clear();
  const runId = randomUUID();
  await rm(join(root,'isolation-report.json'), {force:true});
  const args=['--autojev-isolated',root,'--autojev-upstream',base,'--autojev-ui-url',`http://127.0.0.1:${uiPort}`,'--autojev-ui-check',base,'--autojev-cpa-check',cpa,'--autojev-cpa-run',runId];
  if(reload)args.push('--autojev-check-reload');
  desktop=spawn(binary,args,{env:environment,stdio:'pipe',detached:true});
  let log='';desktop.stdout.on('data',b=>log+=b);desktop.stderr.on('data',b=>log+=b);
  const timer=setTimeout(terminateDesktop,120000);
  const [code]=await once(desktop,'exit');clearTimeout(timer);
  const label=reload?'reload':'first';
  await writeFile(join(root,`${label}.desktop.log`),log);
  let report;
  try { report=JSON.parse(await readFile(join(root,'isolation-report.json'),'utf8')); }
  catch(error) { throw new Error(`Desktop exited ${code} without an acceptance report; inspect ${join(root,`${label}.desktop.log`)}. Screen availability is a prerequisite.`, {cause:error}); }
  await writeFile(join(root,`${label}.report.json`),JSON.stringify(report,null,2));
  assert.equal(report.run_id, runId, 'Require a fresh report from this desktop process');
  assert.equal(code,0,report.error || log);assert.equal(report.ok,true,report.error);
  for(const pid of report.pids)assert.ok(!alive(pid),`Owned CPA ${pid} must be reaped`);
  assert.ok(alive(sentinel.pid),'Unrelated process must remain alive');
  const owned=await readdir(join(root,'.autojev'));
  assert.ok(!owned.some(name=>name.startsWith('cpa-r1-')),'Owned auth/config directories must be removed');
  return report;
};
try {
  const until=Date.now()+20000;
  while(true){try{if((await fetch(`http://127.0.0.1:${uiPort}`)).ok)break;}catch{}assert.ok(Date.now()<until,uiLog);await new Promise(r=>setTimeout(r,100));}
  const first=await run(false);
  const reload=await run(true);
  assert.deepEqual(reload.saved,first.saved,'Restart must preserve source, account, plan and stable model UUIDs');
  await writeFile(join(root,'receiver-records.json'),JSON.stringify({requests:receipts,cancelled},null,2));
  console.log(JSON.stringify({root,checks:[...first.checks,...reload.checks],requests:receipts.length,ownedProcessesReaped:first.pids.length+reload.pids.length,unrelatedProcessPreserved:true},null,2));
} finally {
  terminateDesktop();
  // Retain our DB/reports, remove only CPA service directories in this temporary home.
  const owned = await readdir(join(root,'.autojev'), {withFileTypes:true}).catch(error => {
    if (error.code === 'ENOENT') return []; throw error;
  });
  for (const entry of owned) {
    if (entry.isDirectory() && entry.name.startsWith('cpa-r1-')) {
      await rm(join(root,'.autojev',entry.name), {recursive:true,force:true});
    }
  }
  sentinel.kill('SIGTERM');vite.kill('SIGTERM');fixture.closeAllConnections();await new Promise(r=>fixture.close(r));
}
