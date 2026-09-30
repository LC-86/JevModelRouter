import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir, homedir } from 'node:os';
import { resolve, join } from 'node:path';
import { once } from 'node:events';

// 隔离根目录要与 Rust 侧 canonicalize 后的路径比较（macOS 上 /var 是指向 /private/var 的符号链接）。
const root = realpathSync(await mkdtemp(join(tmpdir(), 'autojev-issue-13-')));
const binary = resolve(process.argv[2] || join(process.env.CARGO_TARGET_DIR || 'src-tauri/target', 'debug/autojev'));
const helper = resolve('scripts/fake-codex-app-server.mjs');
const realHome = (() => { try { return realpathSync(homedir()); } catch { return resolve(homedir()); } })();
// 替身注入的虚构凭据与身份：应用输出里出现任何一个都算脱敏失败或身份复活。
const fictionalTokens = ['fictional-access-token-4f2c9a', 'fictional-refresh-token-8b1d7e'];
const fictionalIdentities = { success: 'standin-success@example.invalid', late: 'standin-late@example.invalid' };
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
const sleep = ms => new Promise(r => setTimeout(r, ms));
const alive = pid => { try { process.kill(pid, 0); return true; } catch (error) { return error.code !== 'ESRCH'; } };
const readLog = async path => {
  try { return (await readFile(path, 'utf8')).split('\n').filter(Boolean).flatMap(line => { try { return [JSON.parse(line)]; } catch { return []; } }); }
  catch { return []; }
};
const helperPids = async path => [...new Set((await readLog(path)).map(entry => entry.pid).filter(Boolean))];
let desktop;
let sentinel;
const observation = (report, label) => Object.fromEntries((report.details?.observations || []).map(item => [item.label, item.value]))[label];
const allObservations = report => Object.fromEntries((report.details?.observations || []).map(item => [item.label, item.value]));
const reaped = async pids => {
  const until = Date.now() + 5000;
  while (Date.now() < until) {
    if (pids.every(pid => !alive(pid))) return true;
    await sleep(100);
  }
  return false;
};
// 每次运行只排练一个登录场景，替身的场景队列与该运行一一对应；替身证据写进独立 JSONL。
const runDesktop = async ({ label, scenarios, reload = false, loginMode = null }) => {
  const helperLog = join(root, `helper-${label}.jsonl`);
  const args = ['--autojev-isolated', root, '--autojev-upstream', base, '--autojev-ui-url', `http://127.0.0.1:${uiPort}`, '--autojev-ui-check', base, '--autojev-helper', helper];
  if (reload) args.push('--autojev-check-reload');
  if (loginMode) args.push('--autojev-login-check', loginMode);
  const env = { ...environment, AUTOJEV_FAKE_HELPER_SCENARIOS: scenarios, AUTOJEV_FAKE_HELPER_LOG: helperLog, AUTOJEV_FAKE_HELPER_DELAY_MS: '150' };
  const child = spawn(binary, args, { env, stdio: 'pipe' });
  desktop = child;
  let log = ''; child.stdout.on('data', b => { log += b; }); child.stderr.on('data', b => { log += b; });
  const liveDuringRun = new Set();
  let probing = true;
  const probe = (async () => { while (probing) { for (const pid of await helperPids(helperLog)) if (alive(pid)) liveDuringRun.add(pid); await sleep(120); } })();
  const timer = setTimeout(() => child.kill('SIGTERM'), 120000);
  const [code, signal] = await once(child, 'exit'); clearTimeout(timer);
  probing = false; await probe;
  await writeFile(join(root, `${label}.desktop.log`), log);
  const report = JSON.parse(await readFile(join(root, 'isolation-report.json'), 'utf8'));
  await writeFile(join(root, `${label}.report.json`), JSON.stringify(report, null, 2));
  assert.equal(code, 0, `Desktop (${label}) failed (${signal}): ${JSON.stringify(report)}\n${log}`);
  assert.equal(report.ok, true, `Desktop (${label}) report: ${JSON.stringify(report)}`);
  const pids = await helperPids(helperLog);
  assert.ok(await reaped(pids), `Stand-in children ${JSON.stringify(pids)} must be reaped once the desktop (${label}) exits`);
  console.log(report.checks.join('\n'));
  return { label, report, helperLog, liveDuringRun };
};

try {
  const deadline = Date.now() + 15000;
  while (true) {
    try { if ((await fetch(`http://127.0.0.1:${uiPort}`)).ok) break; } catch { /* Our Vite is starting. */ }
    assert.ok(Date.now() < deadline, `Isolated UI startup failed: ${viteLog}`);
    await sleep(100);
  }
  sentinel = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
  assert.ok(alive(sentinel.pid), 'Unrelated sentinel process must be alive before the desktop runs');
  const first = await runDesktop({ label: 'first', scenarios: 'success' });
  const reload = await runDesktop({ label: 'reload', scenarios: 'success', reload: true });
  const late = await runDesktop({ label: 'late', scenarios: 'late', loginMode: 'late' });
  const failedRun = await runDesktop({ label: 'failed', scenarios: 'failed', loginMode: 'failed' });
  const runs = [first, reload, late, failedRun];
  const logs = {};
  for (const run of runs) logs[run.label] = await readLog(run.helperLog);
  const allEntries = Object.values(logs).flat();
  const allowedMethods = new Set(['initialize', 'account/login/start', 'account/login/cancel', 'account/read', 'account/logout']);
  assert.ok(allEntries.length > 0, 'The stand-in must have logged its traffic');
  assert.ok(allEntries.some(entry => entry.event === 'request' && entry.method === 'initialize'), 'The stand-in must have been initialized');
  assert.equal(allEntries.filter(entry => entry.event === 'malformed').length, 0, 'The stand-in must never see a malformed frame');
  // 第 7 条：真实生成仍被拒绝 —— 替身只收到契约列出的账号方法，从未收到任何生成请求。
  const unsupported = allEntries.filter(entry => entry.event === 'request' && !allowedMethods.has(entry.method));
  assert.deepEqual(unsupported, [], `The stand-in must never receive a generation request: ${JSON.stringify(unsupported)}`);
  assert.equal(requests.filter(r => r.body?.model === 'codex-fixture-model').length, 0, 'Denied subscription generation must never reach an upstream');
  // 第 5 条：替身看到的 CODEX_HOME 位于隔离根目录下，日志中没有真实凭据。
  const helperHomes = [...new Set(allEntries.map(entry => entry.codexHome))];
  assert.ok(helperHomes.length > 0, 'The stand-in must have reported its CODEX_HOME');
  for (const entry of allEntries) {
    assert.equal(entry.realCredentials, false, `No real credentials may reach the stand-in: ${JSON.stringify(entry)}`);
    assert.equal(entry.isolatedHome, true, `CODEX_HOME must not be the real account directory: ${JSON.stringify(entry)}`);
    assert.ok(entry.codexHome.startsWith(root + '/'), `CODEX_HOME must live under the isolation root: ${entry.codexHome} vs ${root}`);
    assert.ok(!entry.codexHome.startsWith(realHome + '/'), `The stand-in must never see the real home: ${entry.codexHome}`);
  }
  // 第 1 条：界面按钮完成一次成功登录，stage 从 pending 到 completed。
  const firstObs = allObservations(first.report);
  assert.equal(firstObs.pendingToCompleted?.from, 'pending', `Login must pass through pending: ${JSON.stringify(firstObs.pendingToCompleted)}`);
  assert.equal(firstObs.pendingToCompleted?.to, 'completed', `Login must complete: ${JSON.stringify(firstObs.pendingToCompleted)}`);
  assert.equal(firstObs.completed?.identity, fictionalIdentities.success, `The verified identity must come from the stand-in: ${JSON.stringify(firstObs.completed)}`);
  assert.match(String(firstObs.completed?.status), /generation=\d+/, `The row must show the generation: ${firstObs.completed?.status}`);
  assert.match(String(firstObs.completed?.status), /helper=(?!Unknown)\S+/, `The row must show the helper version: ${firstObs.completed?.status}`);
  assert.ok(String(firstObs.completed?.home).startsWith(root + '/'), `The helper home must be an isolation path: ${firstObs.completed?.home}`);
  assert.ok(logs.first.some(entry => entry.event === 'notification' && entry.scenario === 'success' && entry.ok === true), 'The stand-in must complete a successful login');
  // 第 2 条（换号部分）：换号递增世代、旧身份立刻消失、新登录绑定新世代。
  assert.ok(firstObs.switch && firstObs.switch.to.generation > firstObs.switch.from.generation, `Switching accounts must advance the generation: ${JSON.stringify(firstObs.switch)}`);
  assert.ok(!firstObs.switch.to.identity, `Switching accounts must drop the previous identity: ${JSON.stringify(firstObs.switch)}`);
  assert.equal(firstObs.reconnected?.identity, fictionalIdentities.success, `The switched sign-in must verify the stand-in account: ${JSON.stringify(firstObs.reconnected)}`);
  assert.equal(firstObs.reconnected?.generation, firstObs.switch?.to?.generation, `The switched sign-in must stay on the new generation: ${JSON.stringify(firstObs.reconnected)}`);
  // 第 3 条：失败场景在界面与后端都显示失败，且不含替身注入的凭据原文。
  const failedObs = observation(failedRun.report, 'failed');
  assert.equal(failedObs?.stage, 'failed', `A failed login must stay failed in the snapshot: ${JSON.stringify(failedObs)}`);
  assert.ok(failedObs?.error, `A failed login must carry an error: ${JSON.stringify(failedObs)}`);
  const rawFailure = logs.failed.find(entry => entry.event === 'notification' && entry.ok === false);
  assert.ok(rawFailure, 'The stand-in must emit a failure notification');
  assert.ok(fictionalTokens.every(token => String(rawFailure.error).includes(token)), `The stand-in must inject fictional secrets: ${rawFailure?.error}`);
  const failedEvidence = [failedObs.error, failedObs.status, failedObs.row, failedObs.snapshot].join('\n');
  for (const token of fictionalTokens) assert.ok(!failedEvidence.includes(token), `The failure must be redacted; leaked ${token}: ${failedEvidence}`);
  // 第 4 条：退出/换号递增世代、清空缓存、保留配置，并同时展示 local= 与 remote=。
  const logoutObs = observation(first.report, 'afterLogout');
  assert.ok(logoutObs, 'A sign-out outcome must be observed');
  assert.ok(logoutObs.generation > firstObs.reconnected.generation, `Sign-out must advance the generation: ${JSON.stringify(logoutObs)}`);
  assert.ok(!logoutObs.identity, `Sign-out must clear the identity: ${JSON.stringify(logoutObs)}`);
  assert.match(String(logoutObs.status), /local=(cleared|retained)/, `The row must report the local clearing result: ${logoutObs.status}`);
  assert.match(String(logoutObs.status), /remote=(revoked|failed|unsupported|unknown)/, `The row must report the remote revocation result: ${logoutObs.status}`);
  assert.ok(logs.first.some(entry => entry.event === 'request' && entry.method === 'account/logout'), 'The stand-in must receive the sign-out call');
  // 第 2 条：迟到的完成结果被丢弃，不影响新世代。
  const lateObs = allObservations(late.report);
  assert.equal(lateObs.start?.state, 'not_connected', `The late rehearsal must start disconnected: ${JSON.stringify(lateObs.start)}`);
  assert.ok(lateObs.cancelled && lateObs.cancelled.stage !== 'completed', `Cancelling must not complete the sign-in: ${JSON.stringify(lateObs.cancelled)}`);
  assert.notEqual(lateObs.lateCompletion?.stage, 'completed', `A late completion must not complete the cancelled login: ${JSON.stringify(lateObs.lateCompletion)}`);
  assert.ok(!lateObs.lateCompletion?.identity, `A late completion must not revive an identity: ${JSON.stringify(lateObs.lateCompletion)}`);
  assert.ok(lateObs.lateCompletion.generation >= lateObs.lateCompletion.pendingGeneration, `A late completion must not roll the generation back: ${JSON.stringify(lateObs.lateCompletion)}`);
  assert.ok(logs.late.some(entry => entry.event === 'request' && entry.method === 'account/login/cancel'), 'The late run must cancel the pending login');
  assert.ok(logs.late.some(entry => entry.event === 'notification' && entry.scenario === 'late' && entry.ok === true), 'The stand-in must emit the late completion');
  // 重启归位：late 运行带着挂起登录退出，failed 运行启动时必须已回到未连接。
  assert.equal(lateObs.exitPending?.login?.stage, 'pending', `The late run must exit with a pending sign-in: ${JSON.stringify(lateObs.exitPending)}`);
  const reconciled = observation(failedRun.report, 'reconciled');
  assert.equal(reconciled?.state, 'not_connected', `A restarted desktop must reconcile the orphaned pending sign-in: ${JSON.stringify(reconciled)}`);
  assert.match(String(reconciled?.status), /login=idle/, `The reconciled row must be idle again: ${reconciled?.status}`);
  for (const label of ['cancelled', 'lateCompletion', 'exitPending']) {
    const text = String(lateObs[label]?.status ?? '');
    assert.ok(!text.includes(fictionalIdentities.late), `${label} must not show the late identity: ${text}`);
    assert.ok(!text.includes(fictionalIdentities.success), `${label} must not revive the previous identity: ${text}`);
  }
  // 第 6 条：桌面退出后替身子进程被回收，且未波及无关进程。
  const helperPidsSeen = new Set(allEntries.map(entry => entry.pid).filter(Boolean));
  const liveObserved = new Set(runs.flatMap(run => [...run.liveDuringRun]));
  assert.ok(liveObserved.size > 0, 'At least one stand-in child must have been observed alive while its desktop ran');
  assert.ok(alive(sentinel.pid), 'Unrelated processes must survive the desktop runs');
  for (const pid of helperPidsSeen) assert.ok(!alive(pid), `Stand-in child ${pid} must be reaped after the desktop exits`);
  // 既有 #11 断言：真实上游仍只收到固定回环替身的请求。
  assert.ok(requests.some(r => r.headers['user-agent'] === 'AutoJev/ProviderTest' && r.body.messages[0].content === 'Say OK'));
  assert.equal(requests.filter(r => r.headers['user-agent'] === 'AutoJev/ModelTest').length, 3);
  assert.equal(requests.filter(r => r.headers['user-agent'] === 'AutoJev/Debug').length, 3);
  assert.ok(requests.filter(r => !r.path.startsWith('/redirect/')).every(r => r.body.model === 'fixture-model' && r.headers.authorization === 'Bearer fixture-key'));
  console.log(`Native desktop login lifecycle acceptance passed. Fictional evidence: ${root}`);
} finally {
  await writeFile(join(root, 'requests.json'), JSON.stringify(requests, null, 2));
  if (sentinel?.exitCode === null) sentinel.kill('SIGTERM');
  if (desktop?.exitCode === null) desktop.kill('SIGTERM');
  vite.kill('SIGTERM'); fixture.closeAllConnections(); fixture.close();
  console.log(`Evidence retained: ${root}`);
}