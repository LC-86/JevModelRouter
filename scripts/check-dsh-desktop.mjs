// Native UI -> listening Jev gateway -> checksum-pinned CPA -> fictional loopback.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, extname, sep } from 'node:path';
import { randomUUID, createHash } from 'node:crypto';
import { dshFixture } from './dsh-upstream-fixture.mjs';

const binary = resolve(process.argv[2]);
const cpa = resolve(process.argv[3]);
const artifact = JSON.parse(await readFile(new URL('./cpa-artifact.json', import.meta.url), 'utf8'));
assert.equal(createHash('sha256').update(await readFile(cpa)).digest('hex'), artifact.binary_sha256);
const root = await mkdtemp(join(tmpdir(), process.argv.includes('--service-check') ? 'jev-r6-desktop-' : 'jev-r5-desktop-'));
const execFileAsync = promisify(execFile);
console.log(`Owned desktop fixture: ${root}`);
const records = [], cancelled = [], held = new Map(), modes = new Map(), streams = new Map();
const upstream = dshFixture(records, cancelled, held, modes);
let activeRun, gatewayPort;
const ownedCpa = new Set();
const fixture = createServer(async (req, res) => {
  res.setHeader('access-control-allow-origin', '*');
  res.setHeader('access-control-allow-headers', 'content-type');
  if (req.method === 'OPTIONS') { res.writeHead(204); res.end(); return; }
  const reply = value => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(value)); };
  try {
    let text = ''; for await (const chunk of req) text += chunk;
    const body = JSON.parse(text || '{}');
    if (!req.url.startsWith('/__')) { await upstream(req, res, body); return; }
    assert.equal(body.run_id, activeRun, 'Control belongs to the current native process');
    if (req.url === '/__progress') { console.log(body.message); reply({ ok: true }); return; }
    if (req.url === '/__own_cpa') { assert.ok(Number.isInteger(body.pid) && body.pid > 1); ownedCpa.add(body.pid); reply({ ok: true }); return; }
    if (req.url === '/__exit_cpa') { assert.ok(ownedCpa.has(body.pid), 'Only a CPA returned by this native run may be stopped'); process.kill(body.pid, 'SIGTERM'); reply({ ok: true }); return; }
    if (req.url === '/__capture') {
      if (!process.argv.includes('--capture')) { reply({ captured: false, reason: 'not requested' }); return; }
      try {
        const { stdout } = await execFileAsync('/usr/bin/swift', ['-module-cache-path',join(root,'swift-cache'),new URL('./cpa-owned-window.swift', import.meta.url).pathname,String(desktop.pid)], { timeout: 20000 });
        const windowId = stdout.trim(); assert.match(windowId, /^\d+$/);
        assert.ok(!body.label || body.label === 'service');
        const path = join(root, `native-${activeRun}${body.label ? '-service' : ''}.png`);
        await execFileAsync('/usr/sbin/screencapture', ['-x',`-l${windowId}`,path], { timeout: 10000 });
        reply({ captured: true, path, pid: desktop.pid, window_id: windowId });
      } catch (error) { reply({ captured: false, reason: String(error) }); }
      return;
    }
    if (req.url === '/__register') { assert.ok(Number.isInteger(body.port) && body.port > 0 && ![9526,9527,11434].includes(body.port)); gatewayPort = body.port; reply({ ok: true }); return; }
    if (req.url === '/__records') { reply({ records, cancelled }); return; }
    if (req.url === '/__mode') { if (body.mode === null) modes.delete(body.account); else modes.set(body.account, body.mode); reply({ ok: true }); return; }
    if (req.url === '/__release') { assert.ok(held.has(body.tag)); held.get(body.tag)(); reply({ ok: true }); return; }
    if (req.url === '/__cancel') { const s = streams.get(body.id); assert.ok(s); s.abort.abort(); await s.reader.cancel().catch(() => {}); streams.delete(body.id); reply({ cancelled: true }); return; }
    if (req.url === '/__finish') {
      const s = streams.get(body.id); assert.ok(s); let wire = s.first;
      while (true) { const next = await s.reader.read(); if (next.done) break; wire += s.decoder.decode(next.value, { stream: true }); }
      wire += s.decoder.decode(); streams.delete(body.id); reply({ wire }); return;
    }
    assert.ok(['/__call', '/__open'].includes(req.url));
    assert.ok(gatewayPort && body.request.model.startsWith('autojev/model/'));
    const endpoint = body.endpoint || '/v1/chat/completions';
    assert.ok(['/v1/chat/completions', '/v1/responses'].includes(endpoint));
    const abort = new AbortController();
    const signal = AbortSignal.any([abort.signal, AbortSignal.timeout(15000)]);
    const response = await fetch(`http://127.0.0.1:${gatewayPort}${endpoint}`, { method: 'POST', headers: { 'content-type': 'application/json', 'x-autojev-session-id': body.session || activeRun }, body: JSON.stringify(body.request), signal });
    const metadata = { status: response.status, request_id: response.headers.get('x-autojev-request-id'), upstream_model: response.headers.get('x-autojev-model'), route_source: response.headers.get('x-autojev-route-source') };
    if (req.url === '/__call') { reply({ ...metadata, body: await response.text() }); return; }
    assert.equal(response.status, 200);
    const reader = response.body.getReader(), decoder = new TextDecoder(); let first = '';
    while (!first.includes('data:')) { const next = await reader.read(); assert.ok(!next.done, 'Stream must produce content before switching/cancelling'); first += decoder.decode(next.value, { stream: true }); }
    const id = randomUUID(); streams.set(id, { reader, decoder, first, abort }); reply({ ...metadata, id, first });
  } catch (error) { console.error(error); if (!res.headersSent) { res.writeHead(500, { 'content-type': 'application/json' }); res.end(JSON.stringify({ error: String(error) })); } else res.destroy(error); }
});
fixture.listen(0, '127.0.0.1'); await once(fixture, 'listening');
const base = `http://127.0.0.1:${fixture.address().port}`;
const reservation = createServer(); reservation.listen(0, '127.0.0.1'); await once(reservation, 'listening');
const uiPort = reservation.address().port; await new Promise(r => reservation.close(r));
const env = Object.fromEntries(['PATH','TMPDIR','LANG','LC_ALL'].filter(k => process.env[k]).map(k => [k, process.env[k]]));
const webIndex = process.argv.indexOf('--web-root');
let vite, web, viteLog = '';
if (webIndex >= 0) {
  const webRoot = resolve(process.argv[webIndex + 1]);
  web = createServer(async (req, res) => {
    try {
      const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
      const path = resolve(webRoot, `.${pathname === '/' ? '/index.html' : pathname}`);
      assert.ok(path.startsWith(`${webRoot}${sep}`));
      const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.woff2': 'font/woff2', '.svg': 'image/svg+xml', '.png': 'image/png' };
      const contents = await readFile(path);
      res.writeHead(200, { 'content-type': mime[extname(path)] || 'application/octet-stream' }); res.end(contents);
    } catch { res.writeHead(404); res.end(); }
  });
  web.listen(uiPort, '127.0.0.1'); await once(web, 'listening');
} else {
  vite = spawn(process.execPath, [resolve('node_modules/vite/bin/vite.js'), '--host','127.0.0.1','--port',String(uiPort),'--strictPort'], { env, stdio: 'pipe' });
  vite.stdout.on('data', b => viteLog += b); vite.stderr.on('data', b => viteLog += b);
}
let desktop;
const unrelated = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { env, stdio: 'ignore' });
const terminate = () => { if (desktop?.pid && desktop.exitCode === null) try { process.kill(-desktop.pid, 'SIGTERM'); } catch (e) { if (e.code !== 'ESRCH') throw e; } };
process.on('SIGINT', terminate); process.on('SIGTERM', terminate);
async function run(reload) {
  modes.clear(); ownedCpa.clear(); activeRun = randomUUID(); gatewayPort = null;
  await rm(join(root, 'isolation-report.json'), { force: true });
  const args = ['--autojev-isolated',root,'--autojev-upstream',base,'--autojev-ui-url',`http://127.0.0.1:${uiPort}`];
  const manual = process.argv.includes('--manual');
  if (!manual) args.push('--autojev-ui-check',base,'--autojev-dsh-check',cpa,'--autojev-cpa-run',activeRun);
  if (reload) args.push('--autojev-check-reload');
  if (process.argv.includes('--service-check') || manual) args.push('--autojev-cpa-service-check', '--autojev-cpa-service', cpa);
  desktop = spawn(binary, args, { env, stdio: 'pipe', detached: true });
  let log = ''; desktop.stdout.on('data', b => log += b); desktop.stderr.on('data', b => log += b);
  const timer = manual ? null : setTimeout(terminate, 180000);
  const [code] = await once(desktop, 'exit'); if (timer) clearTimeout(timer);
  const label = reload ? 'reload' : 'first';
  await writeFile(join(root, `${label}.desktop.log`), log);
  if (manual) return { ok: code === 0, manual: true };
  const report = JSON.parse(await readFile(join(root, 'isolation-report.json'), 'utf8'));
  await writeFile(join(root, `${label}.report.json`), JSON.stringify(report, null, 2));
  assert.equal(report.run_id, activeRun); assert.equal(code, 0, report.error || log); assert.equal(report.ok, true, report.error);
  assert.equal(unrelated.exitCode, null, 'An unrelated process must remain alive');
  for (const pid of report.pids) { let alive = true; try { process.kill(pid, 0); } catch (e) { if (e.code === 'ESRCH') alive = false; else throw e; } assert.equal(alive, false, `Owned CPA ${pid} must be reaped`); }
  return report;
}
try {
  const deadline = Date.now() + 20000;
  while (true) { try { if ((await fetch(`http://127.0.0.1:${uiPort}`)).ok) break; } catch {} assert.ok(Date.now() < deadline, viteLog); await new Promise(r => setTimeout(r, 100)); }
  const first = await run(false);
  if (!process.argv.includes('--manual')) {
    const reload = await run(true);
    assert.deepEqual(first.saved, reload.saved, 'Native restart preserves UUID and immutable source identity');
  }
  await writeFile(join(root, 'receivers.json'), JSON.stringify({ records, cancelled }, null, 2));
  console.log(JSON.stringify({ ok: true, root, layers: ['native-mac-ui', 'real-pinned-cpa-loopback', 'dsh-shaped-http-replay'], requests: records.length, real_dsh_runtime: false, real_model_calls: 0, logins: 0, account_quota_queries: 0 }));
} finally {
  terminate(); for (const s of streams.values()) { s.abort.abort(); await s.reader.cancel().catch(() => {}); }
  unrelated.kill('SIGTERM'); await once(unrelated, 'exit');
  vite?.kill('SIGTERM');
  if (web) { web.closeAllConnections(); await new Promise(r => web.close(r)); }
  fixture.closeAllConnections(); await new Promise(r => fixture.close(r));
  await writeFile(join(root, 'receivers.json'), JSON.stringify({ records, cancelled }, null, 2));
  console.log(`Retained fictional reports: ${root}`);
}
