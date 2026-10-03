import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { respondFromFictionalAccount } from './cpa-upstream-fixture.mjs';

const binary = resolve(process.argv[2]);
const artifact = JSON.parse(await readFile(new URL('./cpa-artifact.json', import.meta.url), 'utf8'));
const checksum = createHash('sha256').update(await readFile(binary)).digest('hex');
assert.equal(checksum, artifact.binary_sha256, 'Refuse an unproved CPA binary before execution');
const root = await mkdtemp(join(tmpdir(), 'jev-r1-cpa-probe-'));
console.log(`Owned CPA selection fixture: ${root}`);
const records = [];
const behavior = new Map();
const waits = [];
const upstream = createServer(async (req, res) => {
  let text = ''; for await (const part of req) text += part;
  const body = JSON.parse(text || '{}');
  const account = req.url.split('/')[1];
  records.push({ account, key: req.headers.authorization, model: body.model, prompt: body.messages?.[0]?.content, stream: !!body.stream });
  const mode = behavior.get(account) || 'ok';
  if (mode === 'hang') { waits.push(res); return; }
  respondFromFictionalAccount(res, account, !!body.stream, mode);
});
upstream.listen(0,'127.0.0.1'); await once(upstream,'listening');
const upstreamBase = `http://127.0.0.1:${upstream.address().port}`;
const reserve = createServer(); reserve.listen(0,'127.0.0.1'); await once(reserve,'listening');
const port = reserve.address().port; await new Promise(r=>reserve.close(r));
await mkdir(join(root,'auth'));
const clientKey = 'fictional-model-client-r1';
const managementKey = 'fictional-backend-management-r1';
const accountNames = ['a1','a2','b1','paid'];
const groups = () => accountNames.map(account=>({name:`source-${account}`,prefix:`jev-${account}`, 'base-url':`${upstreamBase}/${account}/v1`,keys:[{'api-key':`fictional-${account}`,'proxy-url':'direct'}],models:[{name:'same-model',alias:'same-model'}]}));
const config = {
  'config-version':8,
  server:{host:'127.0.0.1',port,discovery:{enabled:false}},
  management:{'allow-remote':false,'secret-key':managementKey,'disable-control-panel':true,'disable-auto-update-panel':true},
  access:{'api-keys':[clientKey]},
  oauth:{'auth-dir':join(root,'auth')},
  routing:{'force-model-prefix':true,'session-affinity':false,retry:{'request-retry':0,'max-retry-interval':0},cooldown:{'disable-cooling':false}},
  requests:{'proxy-url':'',streaming:{'bootstrap-retries':0}},
  'api-keys':{'openai-compatibility':groups()},
};
const configPath = join(root,'config.json');
const save = ()=>writeFile(configPath, JSON.stringify(config,null,2),{mode:0o600});
const policy = '(version 1) (allow default) (deny network*) (allow network-bind (local ip "localhost:*")) (allow network-inbound (local ip "localhost:*")) (allow network-outbound (remote ip "localhost:*"))';
const base = `http://127.0.0.1:${port}`;
let child, log='';
const sleep = ms=>new Promise(r=>setTimeout(r,ms));
async function listing(){ const r=await fetch(base+'/v1/models',{headers:{authorization:`Bearer ${clientKey}`},signal:AbortSignal.timeout(1000)}); assert.equal(r.status,200); return (await r.json()).data.map(m=>m.id).sort(); }
async function until(fn){ let error; for(let i=0;i<100;i++){try {if(await fn())return;}catch(e){error=e;} await sleep(100);} throw error || new Error('Timed out'); }
async function start(){
  await save(); log='';
  child=spawn('/usr/bin/sandbox-exec',['-p',policy,binary,'--config',configPath,'--local-model'],{cwd:root,env:{PATH:'/usr/bin:/bin'},stdio:'pipe'});
  child.stdout.on('data',b=>log+=b); child.stderr.on('data',b=>log+=b);
  await until(async()=> (await listing()).includes('jev-a1/same-model'));
  await until(async()=>log.includes('file watcher started for config'));
  const management = await fetch(base+'/v8/management/config', {headers:{authorization:`Bearer ${managementKey}`},signal:AbortSignal.timeout(1000)});
  assert.equal(management.status, 200);
  assert.equal(management.headers.get('x-cpa-version'), artifact.version);
  assert.equal(management.headers.get('x-cpa-commit'), artifact.commit);
}
async function stop(){if(child&&child.exitCode===null){const closed=once(child,'exit');child.kill('SIGTERM');await Promise.race([closed,sleep(3000)]);if(child.exitCode===null){child.kill('SIGKILL');await closed;}}}
async function call(account,{stream=false,timeout=3000,prompt='fictional'}={}){
  const model = account.includes('/') || account === 'same-model' ? account : `jev-${account}/same-model`;
  const r=await fetch(base+'/v1/chat/completions',{method:'POST',headers:{authorization:`Bearer ${clientKey}`,'content-type':'application/json'},body:JSON.stringify({model,messages:[{role:'user',content:prompt}],max_tokens:4,stream}),signal:AbortSignal.timeout(timeout)});
  return {status:r.status,body:await r.text()};
}
function receivedSince(offset,allowed){const recent=records.slice(offset);assert.ok(recent.every(r=>allowed.includes(r.account)),JSON.stringify(recent)); return recent;}
const evidence=[];
try {
  await start();
  evidence.push({scenario:'catalog',models:await listing()});
  for(const account of accountNames){const offset=records.length;assert.equal((await call(account)).status,200);assert.equal(records.length,offset+1);const [r]=receivedSince(offset,[account]);assert.equal(r.key,`Bearer fictional-${account}`);assert.equal(r.model,'same-model');evidence.push({scenario:'fixed-target',account,receiver:r});}
  for(const model of ['same-model','jev-missing/same-model']){const offset=records.length;const r=await call(model);assert.ok(r.status>=400,JSON.stringify(r));assert.equal(records.length,offset);evidence.push({scenario:'missing-or-unqualified',model,status:r.status});}
  const parallelOffset=records.length;
  const concurrent = await Promise.all(Array.from({length:24},(_,i)=>call(accountNames[i%3],{prompt:`parallel-${i}`})));
  assert.ok(concurrent.every(r=>r.status===200), 'Concurrent fixed calls must succeed');
  const parallel=records.slice(parallelOffset); assert.equal(parallel.length,24);for(const r of parallel){const i=Number(r.prompt.split('-')[1]);assert.equal(r.account,accountNames[i%3]);}
  evidence.push({scenario:'concurrency',requests:parallel.length});
  for(const status of [401,429,500,502,503]){
    await stop(); behavior.set('a1',status); await start(); const offset=records.length;
    const r=await call('a1');assert.ok(r.status>=400,JSON.stringify(r));const recent=receivedSince(offset,['a1']);assert.equal(recent.length,1);
    const secondOffset=records.length; const cooled=await call('a1');assert.ok(cooled.status>=400);receivedSince(secondOffset,['a1']);
    assert.equal(records.length,secondOffset,'A cooling credential must not be retried or traversed');
    assert.equal((await call('a2')).status,200);
    evidence.push({scenario:'failure-and-cooldown',upstream:status,status:r.status,cooled:cooled.status,targetCalls:records.slice(secondOffset).filter(r=>r.account==='a1').length,otherDuringFailure:0});
  }
  await stop();behavior.set('a1','drop');await start();const streamOffset=records.length;const dropped=await call('a1',{stream:true});assert.ok(dropped.status>=400);receivedSince(streamOffset,['a1']);evidence.push({scenario:'stream-before-first-byte',status:dropped.status,other:0});
  await stop();behavior.delete('a1');await start();
  config['api-keys']['openai-compatibility'][0].disabled=true;await save();await until(async()=>!(await listing()).includes('jev-a1/same-model'));
  const disabledOffset=records.length;assert.ok((await call('a1')).status>=400);assert.equal(records.length,disabledOffset);assert.equal((await call('a2')).status,200);evidence.push({scenario:'hot-reload-disabled',other:0});
  config['api-keys']['openai-compatibility']=groups();await save();await until(async()=>(await listing()).includes('jev-a1/same-model'));const reloadOffset=records.length;
  await Promise.all(accountNames.slice(0,3).map(account=>call(account)));assert.equal(records.length,reloadOffset+3);for(const r of records.slice(reloadOffset)){assert.equal(r.key,`Bearer fictional-${r.account}`);}evidence.push({scenario:'reload-identity',requests:3});
  const output={layer:'real-cpa-direct-http',source:`${artifact.repository}@${artifact.commit}`,sha256:checksum,root,policy,evidence,records};
  await writeFile(join(root,'result.json'),JSON.stringify(output,null,2));
  console.log(JSON.stringify({result:'pass',report:join(root,'result.json'),sha256:checksum,evidence},null,2));
} catch(error){await writeFile(join(root,'failure.log'),log);console.error(error);console.error(log.slice(-4000));process.exitCode=1;}
finally {await stop();for(const res of waits)res.destroy();upstream.closeAllConnections();await new Promise(r=>upstream.close(r));await rm(configPath,{force:true});await rm(join(root,'auth'),{recursive:true,force:true});}
