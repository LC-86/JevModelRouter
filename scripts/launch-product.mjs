// Ordinary desktop, persistent explicitly chosen profile, no test-driver injection.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir,readFile } from 'node:fs/promises';
import { resolve,join,extname,sep,isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
const [profile,mode]=process.argv.slice(2);
assert.ok(profile && isAbsolute(profile),'Pass an absolute dedicated profile directory');
assert.ok(!mode || mode==='--offline','Optional mode is --offline (no real actions)');
const folder=fileURLToPath(new URL('../',import.meta.url));
await mkdir(profile,{recursive:true,mode:0o700});
const webRoot=join(folder,'web');
const web=createServer(async(req,res)=>{try{
 const path=resolve(webRoot,`.${new URL(req.url,'http://localhost').pathname==='/'?'/index.html':new URL(req.url,'http://localhost').pathname}`);
 assert.ok(path.startsWith(webRoot+sep));
 const type={'.html':'text/html','.js':'text/javascript','.css':'text/css','.woff2':'font/woff2'};
 res.writeHead(200,{'content-type':type[extname(path)]||'application/octet-stream'});res.end(await readFile(path));
}catch{res.writeHead(404);res.end('Not found')}});
web.listen(0,'127.0.0.1');await once(web,'listening');
const args=['--autojev-profile',profile,'--autojev-ui-url',`http://127.0.0.1:${web.address().port}`];
if(mode)args.push('--autojev-offline');
const env=Object.fromEntries(['PATH','TMPDIR','LANG','LC_ALL'].filter(k=>process.env[k]).map(k=>[k,process.env[k]]));env.HOME=profile;
const child=spawn(join(folder,'bin/jev'),args,{env,stdio:'inherit',detached:true});
console.log(`Dedicated profile: ${profile}\nPinned CPA file: ${join(folder,'bin/cpa')}\nReal generation starts disabled; evidence and finite HAND_RUN confirmation are required.`);
const terminate=()=>{try{process.kill(-child.pid,'SIGTERM')}catch(e){if(e.code!=='ESRCH')throw e}};
process.once('SIGINT',terminate);process.once('SIGTERM',terminate);
try{const [code]=await once(child,'exit');process.exitCode=code??1;}finally{terminate();web.close();}
