import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { randomUUID } from 'node:crypto';

const binary = resolve(process.argv[2] || join(process.env.CARGO_TARGET_DIR || 'src-tauri/target', 'debug/autojev'));
const root = await mkdtemp(join(tmpdir(), 'jev-r2-desktop-'));
console.log(`Owned R2 fixture: ${root}`);
const records = [], modes = new Map();
const fixture = createServer(async (req, res) => {
  res.setHeader('access-control-allow-origin', '*');
  res.setHeader('access-control-allow-headers', 'content-type');
  if (req.method === 'OPTIONS') { res.end(); return; }
  let text = ''; for await (const chunk of req) text += chunk;
  const body = JSON.parse(text || '{}');
  const reply = data => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(data)); };
  if (req.url === '/__records') { reply(records); return; }
  if (req.url === '/__mode') { modes.set(body.source, body.status); reply({ ok: true }); return; }
  if (req.url === '/__progress') { console.log(body.message); reply({ ok: true }); return; }
  if (req.url === '/__gateway') {
    assert.ok(Number.isInteger(body.port) && body.port > 0 && ![9526, 9527, 11434].includes(body.port));
    assert.ok(['/v1/chat/completions', '/v1/responses', '/v1/messages', '/v1/models'].includes(body.endpoint));
    const response = await fetch(`http://127.0.0.1:${body.port}${body.endpoint}`, body.endpoint === '/v1/models' ? {} : {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body.request), signal: AbortSignal.timeout(10000),
    });
    reply({ status: response.status, body: await response.text() }); return;
  }
  const source = req.url.split('/')[1];
  const expectedPath = `/${source}/${source === 'coding' ? 'v4' : 'v1'}/chat/completions`;
  const credentialMatch = req.headers.authorization === `Bearer fictional-r2-${source}`;
  records.push({ source, path: req.url, credentialMatch, model: body.model, stream: !!body.stream });
  if (!['official', 'openrouter', 'zenmux', 'coding', 'legacy'].includes(source) || req.url !== expectedPath || !credentialMatch || body.model !== 'same-model') {
    res.writeHead(400, { 'content-type': 'application/json' }); res.end(JSON.stringify({ error: { message: 'Wrong fixed fixture target' } })); return;
  }
  const mode = modes.get(source) || 200;
  if (JSON.stringify(body).includes('inflight-r2')) await new Promise(r => setTimeout(r, 300));
  if (mode !== 200) {
    res.writeHead(mode, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { message: `fixture rejection ${req.headers.authorization}` } })); return;
  }
  if (body.stream) {
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (JSON.stringify(body).includes('delta-secret-r2')) {
      for (const content of ['fictional-r2-', source]) res.write('data: ' + JSON.stringify({ choices: [{ index: 0, delta: { content }, finish_reason: null }] }) + '\n\n');
      res.end('data: ' + JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] }) + '\n\ndata: [DONE]\n\n'); return;
    }
    res.write('data: ' + JSON.stringify({ id: 'r2', choices: [{ index: 0, delta: { role: 'assistant', content: `OK ${source}` }, finish_reason: null }] }) + '\n\n');
    res.end('data: ' + JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 2, completion_tokens: 2 } }) + '\n\ndata: [DONE]\n\n');
  } else {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ id: 'r2', choices: [{ message: { role: 'assistant', content: `OK ${source}` }, finish_reason: 'stop' }], usage: { prompt_tokens: 2, completion_tokens: 2 } }));
  }
});
fixture.listen(0, '127.0.0.1'); await once(fixture, 'listening');
const base = `http://127.0.0.1:${fixture.address().port}`;
const reservation = createServer(); reservation.listen(0, '127.0.0.1'); await once(reservation, 'listening');
const uiPort = reservation.address().port; await new Promise(r => reservation.close(r));
const environment = Object.fromEntries(['PATH', 'TMPDIR', 'LANG', 'LC_ALL'].filter(k => process.env[k]).map(k => [k, process.env[k]]));
const vite = spawn(process.execPath, [resolve('node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(uiPort), '--strictPort'], { env: environment, stdio: 'pipe' });
let viteLog = ''; vite.stdout.on('data', b => viteLog += b); vite.stderr.on('data', b => viteLog += b);
let desktop;
const terminate = () => { if (desktop?.pid) { try { process.kill(-desktop.pid, 'SIGTERM'); } catch (e) { if (e.code !== 'ESRCH') throw e; } } };
const run = async reload => {
  modes.clear();
  const runId = randomUUID();
  await rm(join(root, 'isolation-report.json'), { force: true });
  const args = ['--autojev-isolated', root, '--autojev-upstream', base, '--autojev-ui-url', `http://127.0.0.1:${uiPort}`, '--autojev-ui-check', base, '--autojev-api-source-check', runId];
  if (reload) args.push('--autojev-check-reload');
  desktop = spawn(binary, args, { env: environment, stdio: 'pipe', detached: true });
  let log = ''; desktop.stdout.on('data', b => log += b); desktop.stderr.on('data', b => log += b);
  const timer = setTimeout(terminate, 90000);
  const [code] = await once(desktop, 'exit'); clearTimeout(timer);
  const label = reload ? 'reload' : 'first';
  await writeFile(join(root, `${label}.desktop.log`), log);
  const report = JSON.parse(await readFile(join(root, 'isolation-report.json'), 'utf8'));
  assert.equal(report.run_id, runId, 'Require the current desktop run report');
  await writeFile(join(root, `${label}.report.json`), JSON.stringify(report, null, 2));
  assert.equal(code, 0, report.error || log); assert.equal(report.ok, true, report.error);
  for (const source of ['official', 'openrouter', 'zenmux', 'coding', 'legacy']) {
    const sourceRecords = records.filter(r => r.source === source);
    assert.ok(sourceRecords.length > 0, `Source never reached: ${source}`);
    assert.ok(sourceRecords.every(r => r.credentialMatch && r.model === 'same-model'), `Wrong target: ${source}`);
  }
  return report;
};
try {
  const deadline = Date.now() + 20000;
  for (;;) {
    try { if ((await fetch(`http://127.0.0.1:${uiPort}`)).ok) break; } catch {}
    assert.ok(Date.now() < deadline, viteLog); await new Promise(r => setTimeout(r, 100));
  }
  const first = await run(false), reopened = await run(true);
  assert.deepEqual(first.saved, reopened.saved, 'Reopening preserves every connection instance, model UUID and legacy reference');
  await writeFile(join(root, 'receiver-records.json'), JSON.stringify(records, null, 2));
  console.log(JSON.stringify({ root, requests: records.length, checks: [...first.checks, ...reopened.checks] }, null, 2));
} finally {
  terminate(); vite.kill('SIGTERM'); fixture.closeAllConnections(); await new Promise(r => fixture.close(r));
}
