import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve,join} from 'node:path';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {randomUUID} from 'node:crypto';

const binary=resolve(process.argv[2]||'../build-target/debug/autojev');
const root=await mkdtemp(join(tmpdir(),'jev-r3-desktop-'));
console.log(`R3 owned evidence: ${root}`);
let mode='waiting',account='fictional-a',credential=null,session=0;
const receipts={models:0,downloads:0,accountQueries:0,sessions:[],deleted:[]};
const command=async(program,args)=>{const p=spawn(program,args,{env,stdio:'pipe'});let output='';p.stdout.on('data',c=>output+=c);p.stderr.on('data',c=>output+=c);const [code]=await once(p,'exit');assert.equal(code,0,output);return output.trim();};
const fixture=createServer(async(req,res)=>{
  res.setHeader('access-control-allow-origin','*');res.setHeader('access-control-allow-headers','content-type');
  if(req.method==='OPTIONS'){res.writeHead(204);res.end();return;}
  let text='';for await(const chunk of req)text+=chunk;
  const body=JSON.parse(text||'{}'),url=new URL(req.url,'http://127.0.0.1');
  const reply=(value,status=200)=>{res.writeHead(status,{'content-type':'application/json','x-cpa-commit':'e2bff0107bb307337aaa19018ccddd55f64253d5'});res.end(JSON.stringify(value));};
  if(url.pathname==='/__mode'){mode=body.mode;account=body.account||account;reply({ok:true});return;}
  if(url.pathname==='/__receipts'){reply(receipts);return;}
  if(url.pathname==='/__capture'){
    if(process.argv.includes('--capture')){
      try{
        const id=await command('/usr/bin/swift',['-module-cache-path',join(root,'swift-cache'),resolve('scripts/cpa-owned-window.swift'),String(desktop.pid)]);
        assert.match(id,/^\d+$/,'The owned app must have a visible window');
        await command('/usr/sbin/screencapture',['-x','-l',id,join(root,`${body.run_id}.png`)]);
      }catch(error){console.log(`Owned window screenshot unavailable: ${error}`);}
    }
    reply({ok:true});return;
  }
  if(url.pathname==='/__gateway'){
    assert.ok(Number.isInteger(body.port)&&body.port>0&&![9526,9527,11434].includes(body.port));
    assert.ok(/^autojev\/model\/[\w-]+$/.test(body.model));
    try{const r=await fetch(`http://127.0.0.1:${body.port}/v1/chat/completions`,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({model:body.model,messages:[{role:'user',content:`fictional-${body.run_id}`}],max_tokens:4}),signal:AbortSignal.timeout(5000)});reply({status:r.status,body:await r.json()});}
    catch(error){reply({status:599,error:String(error)});}return;
  }
  if(url.pathname.startsWith('/v8/management/'))assert.equal(req.headers.authorization,'Bearer fictional-management-r3');
  if(url.pathname==='/v8/management/oauth/auth-url'){session++;reply({status:'ok',state:`fictional-${session}`,url:'https://auth.example.invalid/authorize'});return;}
  if(url.pathname==='/v8/management/oauth/status'){
    if(mode==='failed'){reply({status:'error',error:'fictional auth rejection'});return;}
    if(mode==='waiting'){reply({status:'wait'});return;}
    credential={name:`fictional-${account}.json`,provider:'codex',account,id_token:{chatgpt_account_id:account,plan_type:'fictional-plan'},access_token:'fictional-never-project'};reply({status:'ok'});return;
  }
  if(url.pathname==='/v8/management/oauth/session'){receipts.sessions.push(url.searchParams.get('state'));reply({status:'ok',cancelled:true});return;}
  if(url.pathname==='/v8/management/credentials'){
    if(req.method==='DELETE'){assert.equal(url.searchParams.get('name'),credential?.name);receipts.deleted.push(credential.name);credential=null;reply({status:'ok'});}
    else reply({files:credential?[credential]:[]});return;
  }
  if(url.pathname==='/v8/management/credentials/models'){assert.equal(url.searchParams.get('name'),credential?.name);reply({models:[{id:'same-model',display_name:'同名订阅模型'}]},mode==='read-failed'?502:200);return;}
  if(url.pathname.includes('download'))receipts.downloads++;
  else if(url.pathname.includes('quota')||url.pathname.includes('account'))receipts.accountQueries++;
  else if(url.pathname.includes('chat/completions'))receipts.models++;
  reply({unexpected: req.url},404);
});
fixture.listen(0,'127.0.0.1');await once(fixture,'listening');
const base=`http://127.0.0.1:${fixture.address().port}`;
const reservation=createServer();reservation.listen(0,'127.0.0.1');await once(reservation,'listening');
const port=reservation.address().port;await new Promise(r=>reservation.close(r));
const env=Object.fromEntries(['PATH','TMPDIR','LANG','LC_ALL'].filter(k=>process.env[k]).map(k=>[k,process.env[k]]));
env.TAURI_DEV_HOST='127.0.0.1';
const vite=spawn(process.execPath,[resolve('node_modules/vite/bin/vite.js'),'--host','127.0.0.1','--port',String(port),'--strictPort'],{env,stdio:'pipe'});
let log='';vite.stdout.on('data',c=>log+=c);vite.stderr.on('data',c=>log+=c);
let desktop;
const terminate=()=>{if(desktop?.pid)try{process.kill(-desktop.pid,'SIGTERM');}catch(error){if(error.code!=='ESRCH')throw error;}};
const run=async reload=>{
  const runId=randomUUID();await rm(join(root,'isolation-report.json'),{force:true});
  const args=['--autojev-isolated',root,'--autojev-upstream',base,'--autojev-ui-url',`http://127.0.0.1:${port}`,'--autojev-ui-check',base,'--autojev-cpa-auth-run',runId];
  if(!reload)args.push('--autojev-cpa-auth-check');
  // Re-read uses the same native harness but leaves the service unavailable.
  if(reload)args.push('--autojev-cpa-auth-check','--autojev-cpa-auth-reload','--autojev-check-reload');
  desktop=spawn(binary,args,{env,stdio:'pipe',detached:true});
  let output='';desktop.stdout.on('data',c=>output+=c);desktop.stderr.on('data',c=>output+=c);
  const timer=setTimeout(terminate,60000);
  const [code]=await once(desktop,'exit');clearTimeout(timer);
  const name=reload?'reload':'first';await writeFile(join(root,`${name}.desktop.log`),output);
  let report;
  try{report=JSON.parse(await readFile(join(root,'isolation-report.json'),'utf8'));}
  catch(error){throw new Error(`Desktop exited ${code} without a fresh report; see ${root}/${name}.desktop.log`,{cause:error});}
  await writeFile(join(root,`${name}.report.json`),JSON.stringify(report,null,2));
  assert.equal(report.run_id,runId);assert.equal(code,0,report.error||output);assert.equal(report.ok,true,report.error);
  return report;
};
try{
  const until=Date.now()+15000;
  while(true){try{if((await fetch(`http://127.0.0.1:${port}`)).ok)break;}catch{}assert.ok(Date.now()<until,log);await new Promise(r=>setTimeout(r,100));}
  const first=await run(false),reload=await run(true);
  assert.deepEqual(reload.saved,first.saved);
  assert.deepEqual(reload.api_source,first.api_source,'R2 explicit source identity must persist independently of CPA switching');
  await writeFile(join(root,'receiver-records.json'),JSON.stringify(receipts,null,2));
  console.log(JSON.stringify({root,checks:[...first.checks,...reload.checks],receipts},null,2));
}finally{terminate();vite.kill('SIGTERM');fixture.closeAllConnections();await new Promise(r=>fixture.close(r));}
