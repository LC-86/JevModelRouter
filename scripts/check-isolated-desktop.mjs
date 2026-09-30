import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { once } from 'node:events';

const root = await mkdtemp(join(tmpdir(), 'autojev-issue-11-'));
const binary = resolve(process.argv[2] || 'src-tauri/target/debug/autojev');
const requests = [];
const fixture = createServer(async (request, response) => {
  let data = ''; for await (const part of request) data += part;
  const body = JSON.parse(data);
  requests.push({ path: request.url, headers: request.headers, body });
  if (request.url.startsWith('/redirect/')) {
    response.writeHead(302, { location: 'https://example.invalid/forbidden' }); response.end(); return;
  }
  if (request.url !== '/v1/chat/completions' || request.headers.authorization !== 'Bearer fixture-key' || body.model !== 'fixture-model') {
    response.writeHead(400); response.end(JSON.stringify({ error: { message: 'Wrong fixture target or credential' } })); return;
  }
  if (body.stream) {
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    response.write('data: ' + JSON.stringify({ id: 'fixture', choices: [{ index: 0, delta: { role: 'assistant', content: 'OK' }, finish_reason: null }] }) + '\n\n');
    await new Promise(r => setTimeout(r, 20));
    response.end('data: ' + JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 12, completion_tokens: 7 } }) + '\n\ndata: [DONE]\n\n');
  } else {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ id: 'fixture', choices: [{ message: { role: 'assistant', content: 'OK' }, finish_reason: 'stop' }], usage: { prompt_tokens: 12, completion_tokens: 7 } }));
  }
});
fixture.listen(0, '127.0.0.1'); await once(fixture, 'listening');
const base = `http://127.0.0.1:${fixture.address().port}`;
const portReservation = createServer(); portReservation.listen(0, '127.0.0.1'); await once(portReservation, 'listening');
const uiPort = portReservation.address().port;
await new Promise(r => portReservation.close(r));
// Do not inherit API keys, proxy variables, client configuration overrides or login environment.
const environment = Object.fromEntries(['PATH', 'TMPDIR', 'LANG', 'LC_ALL', 'SYSTEMROOT', 'DISPLAY', 'WAYLAND_DISPLAY', 'XDG_RUNTIME_DIR'].filter(k => process.env[k]).map(k => [k, process.env[k]]));
const vite = spawn(process.execPath, [resolve('node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(uiPort), '--strictPort'], { env: environment, stdio: 'pipe' });
let viteLog = ''; vite.stdout.on('data', b => { viteLog += b; }); vite.stderr.on('data', b => { viteLog += b; });
let desktop;
try {
  const deadline = Date.now() + 15000;
  while (true) {
    try { if ((await fetch(`http://127.0.0.1:${uiPort}`)).ok) break; } catch { /* Our Vite is starting. */ }
    assert.ok(Date.now() < deadline, `Isolated UI startup failed: ${viteLog}`);
    await new Promise(r => setTimeout(r, 100));
  }
  for (const reload of [false, true]) {
    const args = ['--autojev-isolated', root, '--autojev-upstream', base, '--autojev-ui-url', `http://127.0.0.1:${uiPort}`, '--autojev-ui-check', base];
    if (reload) args.push('--autojev-check-reload');
    desktop = spawn(binary, args, { env: environment, stdio: 'pipe' });
    let log = ''; desktop.stdout.on('data', b => { log += b; }); desktop.stderr.on('data', b => { log += b; });
    const timer = setTimeout(() => desktop.kill('SIGTERM'), 60000);
    const [code, signal] = await once(desktop, 'exit'); clearTimeout(timer);
    await writeFile(join(root, reload ? 'reload.log' : 'desktop.log'), log);
    const report = JSON.parse(await readFile(join(root, 'isolation-report.json'), 'utf8'));
    await writeFile(join(root, reload ? 'reload-report.json' : 'desktop-report.json'), JSON.stringify(report, null, 2));
    assert.equal(code, 0, `Desktop failed (${signal}): ${JSON.stringify(report)}\n${log}`);
    assert.equal(report.ok, true, JSON.stringify(report));
    console.log(report.checks.join('\n'));
  }
  assert.ok(requests.some(r => r.headers['user-agent'] === 'AutoJev/ProviderTest' && r.body.messages[0].content === 'Say OK'));
  assert.equal(requests.filter(r => r.headers['user-agent'] === 'AutoJev/ModelTest').length, 3);
  assert.equal(requests.filter(r => r.headers['user-agent'] === 'AutoJev/Debug').length, 3);
  assert.ok(requests.filter(r => !r.path.startsWith('/redirect/')).every(r => r.body.model === 'fixture-model' && r.headers.authorization === 'Bearer fixture-key'));
  await writeFile(join(root, 'requests.json'), JSON.stringify(requests, null, 2));
  console.log(`Native desktop acceptance passed. Fictional evidence: ${root}`);
} finally {
  await writeFile(join(root, 'requests.json'), JSON.stringify(requests, null, 2));
  if (desktop?.exitCode === null) desktop.kill('SIGTERM');
  vite.kill('SIGTERM'); fixture.closeAllConnections(); fixture.close();
  console.log(`Evidence retained: ${root}`);
}
