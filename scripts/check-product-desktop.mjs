// A normal desktop binary, no isolation-check commands or fixture injection.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn,execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join, extname, sep } from 'node:path';
const [binary, webPath, cpaBinary] = process.argv.slice(2).map(p=>resolve(p));
const root = await mkdtemp(join(tmpdir(), 'jev-product-'));
let report;let round=1;let child;
const bad=join(await mkdtemp(join(tmpdir(),'jev-bad-artifact-')),'wrong-cpa');await writeFile(bad,'fictional wrong artifact');
const occupied=createServer((req,res)=>res.end('owned port sentinel'));occupied.listen(0,'127.0.0.1');await once(occupied,'listening');
const driver = await readFile(new URL('./product-desktop-driver.js',import.meta.url),'utf8');
const web = createServer(async(req,res)=>{
  try {
    if(req.url==='/__kill_owned'){let body='';for await(const chunk of req)body+=chunk;const pid=JSON.parse(body).pid;
      assert.equal(Number(execFileSync('/bin/ps',['-p',String(pid),'-o','ppid='],{encoding:'utf8'}).trim()),child.pid,'Kill only this app-owned CPA child');process.kill(pid,'SIGTERM');res.end('{}');return;}
    if(req.url==='/__capture'){
      let result={captured:false,reason:'not requested'};
      if(process.argv.includes('--capture')){
        try{const windowId=execFileSync('/usr/bin/swift',['-module-cache-path',join(root,'swift-cache'),new URL('./cpa-owned-window.swift',import.meta.url).pathname,String(child.pid)],{encoding:'utf8',timeout:20000}).trim();assert.match(windowId,/^\d+$/);const path=join(root,`native-product-${round}.png`);execFileSync('/usr/sbin/screencapture',['-x',`-l${windowId}`,path],{timeout:10000});result={captured:true,path,pid:child.pid,window_id:windowId};}catch(e){result={captured:false,reason:String(e)};}
      }
      res.end(JSON.stringify(result));return;
    }
    if(req.url==='/__report'){let body='';for await(const chunk of req)body+=chunk;report=JSON.parse(body);res.end('{}');return;}
    const path=resolve(webPath,`.${req.url==='/'?'/index.html':new URL(req.url,'http://localhost').pathname}`);assert.ok(path.startsWith(webPath+sep));
    let contents=await readFile(path);if(path.endsWith('index.html'))contents=Buffer.from(contents.toString().replace('</body>',`<script>window.__PRODUCT_TEST__=${JSON.stringify({reopened:round===2,cpa:cpaBinary||null,bad,occupied:occupied.address().port})};${driver}</script></body>`));
    const type={'.html':'text/html','.js':'text/javascript','.css':'text/css','.woff2':'font/woff2'};
    res.writeHead(200,{'content-type':type[extname(path)]||'application/octet-stream'});res.end(contents);
  }catch(e){res.writeHead(500);res.end(String(e));}
});
web.listen(0,'127.0.0.1');await once(web,'listening');
const args=['--autojev-profile',root,'--autojev-offline','--autojev-ui-url',`http://127.0.0.1:${web.address().port}`];
const env=Object.fromEntries(['PATH','TMPDIR','LANG','LC_ALL'].filter(k=>process.env[k]).map(k=>[k,process.env[k]]));
child=spawn(binary,args,{env,stdio:'pipe',detached:true});let log='';child.stdout.on('data',b=>log+=b);child.stderr.on('data',b=>log+=b);
const terminate=()=>{if(child.exitCode===null)try{process.kill(-child.pid,'SIGTERM')}catch(e){if(e.code!=='ESRCH')throw e}};
const timer=setTimeout(terminate,45000);
try{
 const [code]=await once(child,'exit');clearTimeout(timer);
 await writeFile(join(root,'desktop.log'),log);await writeFile(join(root,'report.json'),JSON.stringify(report||{ok:false,code,log},null,2));
 assert.equal(code,0,JSON.stringify({report,log}));assert.equal(report?.ok,true,JSON.stringify(report));
 assert.ok((await readFile(join(root,'.autojev-profile'),'utf8')).includes('AutoJev'));
 if(cpaBinary){const previous=report.service;round=2;report=null;child=spawn(binary,args,{env,stdio:'pipe',detached:true});log='';child.stdout.on('data',b=>log+=b);child.stderr.on('data',b=>log+=b);const timeout=setTimeout(terminate,45000);const [reopenCode]=await once(child,'exit');clearTimeout(timeout);await writeFile(join(root,'reopen-report.json'),JSON.stringify(report,null,2));assert.equal(reopenCode,0,JSON.stringify({report,log}));assert.equal(report?.ok,true,JSON.stringify(report));assert.equal(report.service.connection_instance_id,previous.connection_instance_id);assert.equal(report.service.owned_service.port,previous.owned_service.port);}
 console.log(JSON.stringify({ok:true,root,ordinary:true,isolation_check:false,real_actions:0,reopened:!!cpaBinary}));
}finally{clearTimeout(timer);terminate();web.close();occupied.close();}
