import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { realpathSync, readdirSync, statSync } from 'node:fs';
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
// 专用授权目录（应用自己管理）与用户真实账号目录（只做元数据指纹，绝不读取内容）。
const helperHome = join(root, '.autojev', 'helpers', 'codex', 'codex-subscription');
const realCodexHome = join(realHome, '.codex');
const codexHomeFingerprint = () => {
  const stat = statSync(realCodexHome, { throwIfNoEntry: false });
  return stat ? { exists: true, mtimeMs: stat.mtimeMs, ino: stat.ino } : { exists: false };
};
const dirEntries = path => {
  try { return readdirSync(path); } catch (error) { if (error.code === 'ENOENT') return null; throw error; }
};
const helperHomeEntries = () => dirEntries(helperHome);
// Grok 只读替身的专用 home（`helper::helper_home` 约定：<home>/.autojev/subscription-helpers/grok_subscription/<id>/home）。
const grokHelperHomePath = join(root, '.autojev', 'subscription-helpers', 'grok_subscription', 'grok-subscription', 'home');
// 本机正在运行的外部 Codex 客户端会持续写 ~/.codex；只要它在跑，mtime 就不能归因给本应用。
const externalCodexClients = () => {
  try { return execFileSync('pgrep', ['-fl', 'ChatGPT.app.*codex'], { encoding: 'utf8' }).trim().split('\n').filter(Boolean); }
  catch { return []; }
};
const requests = [];
// 受控目录验收要从 webview 侧观察真实网关的公共目录。页面直接读回环网关会被同源策略挡下，
// 因此由替身的 Node 侧代取一次：请求仍然落在真实网关的 /v1/models 上，不经过任何应用旁路。
const catalogCors = response => {
  response.setHeader('access-control-allow-origin', '*');
  response.setHeader('access-control-allow-headers', 'content-type');
  response.setHeader('access-control-allow-methods', 'POST, OPTIONS');
};
const fixture = createServer(async (request, response) => {
  if (request.url === '/__catalog-list') {
    catalogCors(response);
    if (request.method === 'OPTIONS') { response.writeHead(204); response.end(); return; }
    let data = ''; for await (const part of request) data += part;
    const payload = JSON.parse(data || '{}');
    const port = Number(payload.port);
    if (!Number.isInteger(port) || port <= 0 || port > 65535) {
      response.writeHead(400, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: `Invalid gateway port: ${payload.port}` }));
      return;
    }
    const listing = await fetch(`http://127.0.0.1:${port}/v1/models`);
    const body = await listing.json();
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ status: listing.status, ids: (body.data || []).map(entry => entry.id) }));
    return;
  }
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
// 每次运行只排练一组场景，替身的场景队列与该运行一一对应；替身证据写进独立 JSONL。
// `reads`（额度）与 `catalog`（目录）各自一条队列：#15 的 catalog 运行用它排练多桶/单桶/缺字段/
// 越界/拒绝/许可缺失/读取失败等场景；`grokScenarios` 只给 #16 的 Grok 只读替身运行。
// `catalogFixture`（对象）是 #17 的受控目录替身输入：写进隔离根目录的 JSON 文件并交给
// `--autojev-catalog-fixture`，只在该运行生效；两条 loginMode 互不冒充。
const runDesktop = async ({ label, scenarios, reload = false, loginMode = null, grokScenarios = null, reads = null, catalog = null, catalogFixture = null, accounts = null }) => {
  const helperLog = join(root, `helper-${label}.jsonl`);
  const grokHelperLog = join(root, `grok-helper-${label}.jsonl`);
  const args = ['--autojev-isolated', root, '--autojev-upstream', base, '--autojev-ui-url', `http://127.0.0.1:${uiPort}`, '--autojev-ui-check', base, '--autojev-helper', helper];
  // Grok 只读替身只在 grok-read 运行里注入；其它运行必须保持「无 Grok helper」的边界。
  if (grokScenarios) args.push('--autojev-grok-helper', resolve('scripts/fake-grok-read-helper.mjs'));
  if (reload) args.push('--autojev-check-reload');
  if (loginMode) args.push('--autojev-login-check', loginMode);
  if (catalogFixture) {
    const fixturePath = join(root, `catalog-${label}.json`);
    await writeFile(fixturePath, JSON.stringify(catalogFixture, null, 2));
    args.push('--autojev-catalog-fixture', fixturePath);
  }
  const env = { ...environment, AUTOJEV_FAKE_HELPER_SCENARIOS: scenarios, AUTOJEV_FAKE_HELPER_LOG: helperLog, AUTOJEV_FAKE_HELPER_DELAY_MS: '150' };
  if (grokScenarios) Object.assign(env, {
    AUTOJEV_FAKE_GROK_SCENARIOS: grokScenarios, AUTOJEV_FAKE_GROK_LOG: grokHelperLog,
    // 权威名是 `AUTOJEV_GROK_FAKE_*`（落在 helper.rs 的 `AUTOJEV_GROK_*` 白名单内，零 Rust 改动）；
    // 冻结名 `AUTOJEV_FAKE_GROK_*` 作为兼容别名带同一份值。
    AUTOJEV_GROK_FAKE_SCENARIOS: grokScenarios, AUTOJEV_GROK_FAKE_LOG: grokHelperLog,
  });
  if (reads) env.AUTOJEV_FAKE_HELPER_READS = reads;
  if (catalog) env.AUTOJEV_FAKE_HELPER_CATALOG = catalog;
  if (accounts) env.AUTOJEV_FAKE_HELPER_ACCOUNTS = accounts;
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
  return { label, report, helperLog, grokHelperLog, liveDuringRun };
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
  // 真实 ~/.codex 可能同时被用户自己的 Codex 客户端写入。先用一段完全没有隔离进程的
  // 校准窗口测量环境噪声：安静时按 mtime 逐位断言，环境本身在变时如实记录并退回
  // 存在性、inode 与「本应用虚构凭据绝不出现」这些可归因的硬断言。
  const calibrationSamples = [];
  for (let index = 0; index < 3; index += 1) { calibrationSamples.push(codexHomeFingerprint()); if (index < 2) await sleep(10000); }
  const externalClients = externalCodexClients();
  const realCodexNoisy = new Set(calibrationSamples.map(sample => JSON.stringify(sample))).size > 1 || externalClients.length > 0;
  const realCodexBefore = codexHomeFingerprint();
  if (realCodexNoisy) console.log(`Real ~/.codex mtime is not attributable here: control samples ${JSON.stringify(calibrationSamples)}, external Codex clients ${externalClients.length}.`);
  const first = await runDesktop({ label: 'first', scenarios: 'success' });
  // AC4：登录成功时替身在专用目录里留下了虚构凭据；退出后该目录必须已被应用删除或清空。
  const firstLog = await readLog(first.helperLog);
  const credentialWrites = firstLog.filter(entry => entry.action === 'credential-file-written');
  assert.ok(credentialWrites.length > 0, 'The stand-in must leave a fictional credential file behind a successful sign-in');
  for (const write of credentialWrites) assert.ok(String(write.path).startsWith(helperHome + '/'), `The fictional credential must stay inside the dedicated helper home: ${write.path}`);
  const entriesAfterSignOut = helperHomeEntries();
  assert.ok(entriesAfterSignOut === null || entriesAfterSignOut.length === 0, `Sign-out must remove or empty the dedicated helper home: ${JSON.stringify(entriesAfterSignOut)}`);
  console.log(`Sign-out cleanup asserted: ${credentialWrites.length} fictional credential file(s) removed with the dedicated helper home.`);
  const reload = await runDesktop({ label: 'reload', scenarios: 'success', reload: true });
  const late = await runDesktop({ label: 'late', scenarios: 'late', loginMode: 'late' });
  const failedRun = await runDesktop({ label: 'failed', scenarios: 'failed', loginMode: 'failed' });
  const requestsBeforeGrok = requests.length;
  const grokRun = await runDesktop({ label: 'grok', scenarios: 'success', loginMode: 'grok' });
  const grokRunRequests = requests.slice(requestsBeforeGrok);
  // 「无 Grok helper」运行的证据必须在 grok-read 之前采集：后者会按设计准备专用 home。
  const grokEntriesAfterNoHelperRun = dirEntries(grokHelperHomePath);
  // #16 端到端：Grok 只读替身真的回复。三次刷新 = 9 次一次性 helper 调用（每次刷新 account → models → usage）。
  const grokReadScenarios = 'success,success,success,success,removed,denied,success,unsupported-catalog,fail-quota';
  const requestsBeforeGrokRead = requests.length;
  const grokReadRun = await runDesktop({ label: 'grok-read', scenarios: 'success', loginMode: 'grok-read', grokScenarios: grokReadScenarios });
  const grokReadRequests = requests.slice(requestsBeforeGrokRead);
  // #15：目录/额度只读验收。一次运行排练一组队列：登录成功后按刷新次序逐个取场景。
  const catalogReads = 'multi,single,missing,invalid,denied,bucket-denied,no-permission,null-permission,fail-quota,fail-quota,fail-quota,multi';
  const catalogScenarios = 'success,legacy,missing,success,success,success,success,success,success,fail-catalog,fail-catalog,success';
  const catalogRun = await runDesktop({ label: 'catalog', scenarios: 'success', loginMode: 'catalog', reads: catalogReads, catalog: catalogScenarios, accounts: [...Array(11).fill('connected'), 'incomplete', 'connected', 'connected', 'connected', 'signed-out'].join(',') });
  // #17：受控目录把「上游目录与额度读取」换成脚本可控文件，其余（登录、世代、准入、派发、快照）仍是生产代码。
  // 读取顺序与 isolation-check.js 的 modelSelectionLifecycle 一致：
  // 发现 → 同账号读取失败 → 权威移除 → 换号后重核 → 删除模型行后重建。
  const modelSelectionRun = await runDesktop({
    label: 'model-selection',
    scenarios: 'success',
    loginMode: 'model-selection',
    catalogFixture: {
      quota: 'unsupported',
      reads: [
        { models: [{ model_id: 'codex-catalog-alpha', name: 'Catalog Alpha', eligible: true }] },
        { fail: true },
        { models: [] },
        { models: [{ model_id: 'codex-catalog-alpha', name: 'Catalog Alpha (renamed upstream)', eligible: true }] },
        { models: [{ model_id: 'codex-catalog-alpha', name: 'Catalog Alpha (renamed upstream)', eligible: true }] },
      ],
    },
  });
  const runs = [first, reload, late, failedRun, grokRun, grokReadRun, catalogRun, modelSelectionRun];
  const logs = {};
  for (const run of runs) logs[run.label] = await readLog(run.helperLog);
  const allEntries = Object.values(logs).flat();
  // #15 新增的两个只读 RPC 与账号方法同属允许集合；生成方法仍必须一个都不出现。
  const allowedMethods = new Set(['initialize', 'account/login/start', 'account/login/cancel', 'account/read', 'account/logout', 'model/list', 'account/rateLimits/read']);
  assert.ok(allEntries.length > 0, 'The stand-in must have logged its traffic');
  assert.ok(allEntries.some(entry => entry.event === 'request' && entry.method === 'initialize'), 'The stand-in must have been initialized');
  // 官方 app-server 握手：initialize 必须带客户端自述，响应之后再发一条无 id 的 initialized 通知。
  const initializeCalls = allEntries.filter(entry => entry.event === 'request' && entry.method === 'initialize');
  assert.ok(
    initializeCalls.every(entry => entry.params?.clientInfo?.name === 'autojev' && Boolean(entry.params?.clientInfo?.version)),
    `Every initialize must carry an AutoJev clientInfo: ${JSON.stringify(initializeCalls)}`,
  );
  assert.ok(
    allEntries.some(entry => entry.event === 'notification-in' && entry.method === 'initialized'),
    'The app must send the initialized notification after the initialize response',
  );
  assert.equal(allEntries.filter(entry => entry.event === 'malformed').length, 0, 'The stand-in must never see a malformed frame');
  // 第 7 条：真实生成仍被拒绝 —— 替身只收到契约列出的账号方法，从未收到任何生成请求。
  const unsupported = allEntries.filter(entry => entry.event === 'request' && !allowedMethods.has(entry.method));
  assert.deepEqual(unsupported, [], `The stand-in must never receive a generation request: ${JSON.stringify(unsupported)}`);
  assert.equal(requests.filter(r => r.body?.model === 'codex-fixture-model').length, 0, 'Denied subscription generation must never reach an upstream');
  assert.equal(requests.filter(r => r.body?.model === 'grok-fixture-model').length, 0, 'Denied Grok subscription generation must never reach an upstream');
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
  assert.match(String(logoutObs.status), /local=cleared/, `The successful sign-out path must report local=cleared: ${logoutObs.status}`);
  assert.match(String(logoutObs.status), /remote=revoked/, `The successful sign-out path must report remote=revoked: ${logoutObs.status}`);
  assert.equal(logoutObs.logout?.local, 'cleared', `The successful sign-out must clear locally: ${JSON.stringify(logoutObs.logout)}`);
  assert.equal(logoutObs.logout?.remote, 'revoked', `The successful sign-out must revoke remotely: ${JSON.stringify(logoutObs.logout)}`);
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
  // #12 边界：Codex helper 已自述时，Grok 行仍必须显示 Unknown，不得借用它的 version/auth_home。
  const borrow = firstObs.grokBorrowCheck;
  assert.ok(borrow?.codex?.version && borrow.codex.version !== 'Unknown', `The Codex helper must have reported a version: ${JSON.stringify(borrow)}`);
  assert.ok(String(borrow?.codex?.home).startsWith(root + '/'), `The Codex helper home must be an isolation path: ${JSON.stringify(borrow)}`);
  assert.ok(borrow?.grok?.helper?.available !== true && !borrow?.grok?.helper?.version && !borrow?.grok?.helper?.auth_home, `The Grok row must not attach the Codex helper report: ${JSON.stringify(borrow?.grok)}`);
  assert.match(String(borrow?.grok?.status), /helper=Unknown auth_home=Unknown/, `The Grok row must show Unknown helper fields: ${borrow?.grok?.status}`);
  assert.ok(!String(borrow?.grok?.status).includes(borrow.codex.version) && !String(borrow?.grok?.status).includes(borrow.codex.home), 'The Grok row must not display the Codex helper values');
  // Grok 登录被拒：明确错误、零连接改动、Codex 行不受影响。
  const grokRejected = observation(grokRun.report, 'grokRejected');
  const grokStartObs = observation(grokRun.report, 'grokStart');
  assert.ok(grokRejected && grokStartObs, 'The Grok sign-in rejection must be observed');
  assert.match(String(grokRejected.directError), /grok/i, `The Grok rejection must name the provider: ${grokRejected.directError}`);
  assert.match(String(grokRejected.directError), /not implemented|unsupported|not supported/i, `The Grok rejection must be explicit: ${grokRejected.directError}`);
  assert.match(String(grokRejected.rowError), /not implemented|unsupported|not supported/i, `The row must show the Grok rejection: ${grokRejected.rowError}`);
  assert.equal(grokStartObs.helper?.available, false, `The Grok row must report an unavailable helper: ${JSON.stringify(grokStartObs.helper)}`);
  assert.match(String(grokStartObs.status), /helper=Unknown auth_home=Unknown/, `The Grok row must show Unknown helper fields: ${grokStartObs.status}`);
  assert.equal(grokRejected.after?.state, 'not_connected', `A rejected Grok sign-in must not change its connection: ${JSON.stringify(grokRejected.after)}`);
  assert.equal(grokRejected.after?.login?.stage, 'idle', `A rejected Grok sign-in must not create a session: ${JSON.stringify(grokRejected.after)}`);
  assert.equal(grokRejected.codex?.state, grokStartObs.codexBefore?.state, `The Codex row state must not change because of Grok: ${JSON.stringify(grokRejected)}`);
  assert.equal(grokRejected.codex?.generation, grokStartObs.codexBefore?.generation, `The Codex generation must not change because of Grok: ${JSON.stringify(grokRejected)}`);
  assert.equal(logs.grok.length, 0, `The Grok run must not start the Codex stand-in: ${JSON.stringify(logs.grok)}`);
  // Issue #16 只读诚实性：隔离下 Grok 只读 refresh 必须如实失败、零字段改写、零伪造数字，且官方入口可见。
  const grokReadOnly = observation(grokRun.report, 'grokReadOnly');
  assert.ok(grokReadOnly, 'The isolated Grok read-only refresh must be observed');
  assert.match(String(grokReadOnly?.refreshError), /isolated|helper|not implemented|unsupported|not supported/i, `The isolated Grok read-only refresh must fail honestly: ${JSON.stringify(grokReadOnly)}`);
  assert.equal(grokReadOnly?.unchanged, true, `A failed isolated Grok refresh must not rewrite any connection field: ${JSON.stringify(grokReadOnly)}`);
  assert.equal(grokReadOnly?.after?.state, 'not_connected', `The Grok connection must stay unconnected: ${JSON.stringify(grokReadOnly?.after)}`);
  assert.equal(grokReadOnly?.after?.generation, grokReadOnly?.before?.generation, `The Grok generation must not move: ${JSON.stringify(grokReadOnly)}`);
  assert.ok(!grokReadOnly?.after?.identity, `The Grok refresh must not invent an identity: ${JSON.stringify(grokReadOnly?.after)}`);
  assert.equal(grokReadOnly?.after?.catalogState, 'unknown', `Grok catalog evidence must stay unknown: ${JSON.stringify(grokReadOnly?.after)}`);
  assert.equal(grokReadOnly?.after?.quotaState, 'unknown', `Grok quota evidence must stay unknown: ${JSON.stringify(grokReadOnly?.after)}`);
  assert.equal(grokReadOnly?.after?.buckets, 0, `An unread Grok quota must keep its buckets empty: ${JSON.stringify(grokReadOnly?.after)}`);
  assert.equal(grokReadOnly?.after?.models, 0, `An unread Grok catalog must keep its models empty: ${JSON.stringify(grokReadOnly?.after)}`);
  assert.match(String(grokReadOnly?.catalogText), /catalog_state=unknown/, `Grok catalog must read catalog_state=unknown: ${grokReadOnly?.catalogText}`);
  assert.match(String(grokReadOnly?.quotaText), /quota_state=unknown/, `Grok quota must read quota_state=unknown: ${grokReadOnly?.quotaText}`);
  assert.ok(!/bucket=/.test(String(grokReadOnly?.quotaText)), `An unread Grok quota must not render a bucket segment: ${grokReadOnly?.quotaText}`);
  assert.ok(!/\d/.test(String(grokReadOnly?.quotaText)), `An unread Grok quota must not fabricate numbers: ${grokReadOnly?.quotaText}`);
  assert.ok(!/\d/.test(String(grokReadOnly?.catalogText)), `An unread Grok catalog must not fabricate numbers: ${grokReadOnly?.catalogText}`);
  assert.ok(!/models=\S/.test(String(grokReadOnly?.catalogText)), `An unread Grok catalog must not list models: ${grokReadOnly?.catalogText}`);
  assert.ok((grokReadOnly?.links || []).some(href => String(href).includes('docs.x.ai/build/cli/reference')), `The unknown Grok catalog must offer the official reference: ${JSON.stringify(grokReadOnly?.links)}`);
  assert.ok((grokReadOnly?.links || []).some(href => String(href).includes('docs.x.ai/grok/faq')), `The unknown Grok quota must offer the official reference: ${JSON.stringify(grokReadOnly?.links)}`);
  assert.ok(grokReadOnly?.after?.helper?.available !== true && !grokReadOnly?.after?.helper?.version && !grokReadOnly?.after?.helper?.auth_home, `Grok must not borrow the Codex helper report: ${JSON.stringify(grokReadOnly?.after?.helper)}`);
  assert.match(String(grokReadOnly?.status), /helper=Unknown auth_home=Unknown/, `The Grok row must show Unknown helper fields: ${grokReadOnly?.status}`);
  // 生成准入仍拒绝：Grok 订阅模型零派发，且该次运行没有新增任何上游模型请求。
  // `samples` 是尝试计数（失败的尝试也计入），所以「没有派发」由上面的 fixture 账本判定；
  // 这里断言本次拒绝没有被记成任何成功测量。
  const grokDenied = observation(grokRun.report, 'grokDenied');
  assert.ok(grokDenied, 'The denied Grok subscription probe must be observed');
  assert.ok(grokDenied?.reason, `A denied Grok subscription test must report a reason: ${JSON.stringify(grokDenied)}`);
  assert.equal(grokDenied?.successRate, 0, `A denied Grok subscription test must not be recorded as a success: ${JSON.stringify(grokDenied)}`);
  assert.equal(grokRunRequests.filter(r => r.body?.model === 'grok-fixture-model' || r.body?.model === 'codex-fixture-model').length, 0, `The Grok run must not dispatch any subscription model upstream: ${JSON.stringify(grokRunRequests)}`);
  assert.equal(grokRunRequests.filter(r => r.headers['user-agent'] === 'AutoJev/ModelTest').length, 0, `The Grok run must not run upstream model tests: ${JSON.stringify(grokRunRequests)}`);
  // 无 Grok 辅助进程被拉起：应用自有 Grok home 从未写入（不存在或为空）。证据在 grok-read 运行之前采集。
  assert.ok(grokEntriesAfterNoHelperRun === null || grokEntriesAfterNoHelperRun.length === 0, `The no-helper Grok run must not spawn a helper that leaves state behind: ${JSON.stringify(grokEntriesAfterNoHelperRun)}`);
  // #16 grok-read：只读替身的调用账本（顺序、一次性进程、回收、环境白名单、专用 home）。
  const grokEntries = await readLog(grokReadRun.grokHelperLog);
  assert.equal(grokEntries.length, 9, `The Grok stand-in must serve exactly three refreshes: ${JSON.stringify(grokEntries)}`);
  assert.deepEqual(grokEntries.map(entry => entry.subcommand), ['account', 'models', 'usage', 'account', 'models', 'usage', 'account', 'models', 'usage'], `Each refresh must call account → models → usage sequentially: ${JSON.stringify(grokEntries.map(entry => entry.subcommand))}`);
  assert.deepEqual(grokEntries.map(entry => entry.scenario), grokReadScenarios.split(','), `One queue entry must be consumed per helper invocation, in order: ${JSON.stringify(grokEntries.map(entry => entry.scenario))}`);
  assert.deepEqual(grokEntries.map(entry => entry.index), [0, 1, 2, 3, 4, 5, 6, 7, 8], `The stand-in must consume the queue in order: ${JSON.stringify(grokEntries.map(entry => entry.index))}`);
  const grokStandinPids = [...new Set(grokEntries.map(entry => entry.pid).filter(Boolean))];
  assert.equal(grokStandinPids.length, 9, `Each read must be its own one-shot process: ${JSON.stringify(grokStandinPids)}`);
  assert.ok(await reaped(grokStandinPids), `Grok stand-in children ${JSON.stringify(grokStandinPids)} must be reaped once the desktop exits`);
  for (const entry of grokEntries) {
    assert.equal(entry.home, grokHelperHomePath, `The Grok stand-in must run with the app-owned helper home: ${entry.home}`);
    assert.ok(!String(entry.home).startsWith(realHome + '/'), `The Grok stand-in must never see the real home: ${entry.home}`);
  }
  const inheritedEnv = [...new Set(grokEntries.flatMap(entry => entry.envKeys || []).filter(key => /AUTOJEV_FAKE_HELPER|CODEX_HOME|API_?KEY|TOKEN|SECRET|PASSWORD/i.test(key)))];
  assert.deepEqual(inheritedEnv, [], `The Grok stand-in must not inherit Codex stand-in or credential env: ${JSON.stringify(inheritedEnv)}`);
  assert.ok((dirEntries(grokHelperHomePath) || []).length === 0, `The Grok helper home must stay free of residue: ${JSON.stringify(dirEntries(grokHelperHomePath))}`);
  // 三轮的机器文本与「没有伪造 0/100%」复核。
  const grokReadObs = allObservations(grokReadRun.report);
  assert.ok(grokReadObs.grokReadRound1 && grokReadObs.grokReadRound2 && grokReadObs.grokReadRound3 && grokReadObs.grokReadFabrication, `All three Grok stand-in rounds must be observed: ${JSON.stringify(Object.keys(grokReadObs))}`);
  assert.match(String(grokReadObs.grokReadRound1.catalogText), /catalog_state=available/, `Round 1 catalog must be available: ${grokReadObs.grokReadRound1.catalogText}`);
  assert.match(String(grokReadObs.grokReadRound1.quotaText), /used=42\.5/, `Round 1 quota must carry the stand-in usage: ${grokReadObs.grokReadRound1.quotaText}`);
  assert.match(String(grokReadObs.grokReadRound2.catalogText), /removed=grok-mini/, `Round 2 must report the retired model: ${grokReadObs.grokReadRound2.catalogText}`);
  assert.match(String(grokReadObs.grokReadRound2.quotaText), /quota_state=denied/, `Round 2 quota must be denied: ${grokReadObs.grokReadRound2.quotaText}`);
  assert.match(String(grokReadObs.grokReadRound3.catalogText), /catalog_state=unsupported/, `Round 3 catalog must be unsupported: ${grokReadObs.grokReadRound3.catalogText}`);
  assert.match(String(grokReadObs.grokReadRound3.quotaText), /quota_state=failed/, `Round 3 quota must be failed: ${grokReadObs.grokReadRound3.quotaText}`);
  assert.match(String(grokReadObs.grokReadRound3.quotaText), /history=true/, `Round 3 must flag the retained history: ${grokReadObs.grokReadRound3.quotaText}`);
  assert.deepEqual(grokReadObs.grokReadFabrication.fabricated ?? [], [], `The stand-in never supplied a zero/100% value, so none may appear: ${JSON.stringify(grokReadObs.grokReadFabrication)}`);
  assert.equal(grokReadRequests.filter(r => r.body?.model === 'grok-fixture-model' || r.body?.model === 'codex-fixture-model').length, 0, `The Grok stand-in run must not dispatch any subscription model upstream: ${JSON.stringify(grokReadRequests)}`);
  assert.equal(grokReadRequests.filter(r => r.headers['user-agent'] === 'AutoJev/ModelTest').length, 0, `The Grok stand-in run must not run upstream model tests: ${JSON.stringify(grokReadRequests)}`);
  // #17 受控目录全过程：替身只替换上游目录与额度读取，登录/准入/派发/快照都走生产代码。
  const catalogObs = allObservations(modelSelectionRun.report);
  const catalogPublicId = 'codex-subscription/codex-catalog-alpha';
  const discovered = catalogObs.discovered;
  assert.equal(catalogObs.start?.state, 'not_connected', `The catalog rehearsal must start disconnected: ${JSON.stringify(catalogObs.start)}`);
  assert.equal((catalogObs.start?.catalog || []).some(item => item.model_id === 'codex-catalog-alpha'), false, `The controlled model must not exist before discovery: ${JSON.stringify(catalogObs.start?.catalog)}`);
  assert.ok(discovered, `The controlled directory must be discovered: ${JSON.stringify(Object.keys(catalogObs))}`);
  assert.equal(discovered.availability, 'available', `A successful read must confirm the entry: ${JSON.stringify(discovered)}`);
  assert.equal(discovered.eligibility, 'eligible', `A confirmed entry must be eligible: ${JSON.stringify(discovered)}`);
  assert.equal(discovered.selected, false, `A newly discovered model must start unselected: ${JSON.stringify(discovered)}`);
  assert.equal(discovered.disabled, false, `A newly discovered model must start enabled: ${JSON.stringify(discovered)}`);
  assert.ok(discovered.internal_id, 'A discovered model must get a stable internal ID');
  assert.deepEqual((discovered.upstream || []).map(item => item.model_id), ['codex-catalog-alpha'], `Discovery must be driven by the controlled directory: ${JSON.stringify(discovered.upstream)}`);
  assert.equal(discovered.publicCatalog?.includes(catalogPublicId), false, `An unselected model must stay out of the public catalog: ${JSON.stringify(discovered.publicCatalog)}`);
  assert.equal(catalogObs.unselected?.containsDiscovered, false, `The unselected observation must stay unselected: ${JSON.stringify(catalogObs.unselected)}`);
  // 勾选进入公共目录；取消选择移出目录，但显式原 ID 直调的准入结论必须完全不变。
  assert.equal(catalogObs.selected?.publicCatalog?.includes(catalogPublicId), true, `Selecting must add the model to the public catalog: ${JSON.stringify(catalogObs.selected)}`);
  assert.equal(catalogObs.deselected?.publicCatalog_contains, false, `Deselecting must remove the model from the public catalog: ${JSON.stringify(catalogObs.deselected)}`);
  assert.ok(catalogObs.deselected?.admissionWhileSelected?.code, `The direct-call probe must report a stable denial code: ${JSON.stringify(catalogObs.deselected)}`);
  assert.notEqual(catalogObs.deselected?.admissionWhileSelected?.code, 'model_disabled', `Selecting must not disable the model: ${JSON.stringify(catalogObs.deselected?.admissionWhileSelected)}`);
  assert.equal(catalogObs.deselected?.admissionWhileDeselected?.code, catalogObs.deselected?.admissionWhileSelected?.code, `Deselection must not change the direct-call admission result: ${JSON.stringify(catalogObs.deselected)}`);
  assert.equal(catalogObs.deselected?.admissionWhileDeselected?.status, catalogObs.deselected?.admissionWhileSelected?.status, `Deselection must not change the direct-call status: ${JSON.stringify(catalogObs.deselected)}`);
  // 停用：原 ID 直调、公共目录与手动测速全部禁止，且上游零派发。
  assert.equal(catalogObs.disabled?.direct?.code, 'model_disabled', `Disabling must deny every direct call: ${JSON.stringify(catalogObs.disabled)}`);
  assert.equal(catalogObs.disabled?.publicCatalog?.includes(catalogPublicId), false, `A disabled model must stay out of the public catalog: ${JSON.stringify(catalogObs.disabled?.publicCatalog)}`);
  assert.match(String(catalogObs.disabled?.speedTestError), /disabled|enabled models/i, `A disabled model must not be measured: ${JSON.stringify(catalogObs.disabled)}`);
  assert.equal(catalogObs.disabled?.internal_id, discovered.internal_id, `Disabling must not change the internal ID: ${JSON.stringify(catalogObs.disabled)}`);
  // 同账号目录失败：保留已核实项并标陈旧，配置不变，仍按当前世代合格。
  // #15 之后失败不再让 refresh 命令报错，而是如实写进只读证据（catalog=stale），
  // 因此这里断言「证据状态 + 目录资格」两层，而不是断言命令异常。
  assert.equal(catalogObs.stale?.error, null, `A failed directory read must not surface as a command error: ${JSON.stringify(catalogObs.stale)}`);
  assert.equal(catalogObs.stale?.evidence, 'stale', `A failed read must mark the read-only catalog evidence stale: ${JSON.stringify(catalogObs.stale)}`);
  assert.equal(catalogObs.stale?.availability, 'stale', `A failed read must mark the entry stale: ${JSON.stringify(catalogObs.stale)}`);
  assert.equal(catalogObs.stale?.eligibility, 'stale', `A stale entry must stay qualified for the same account: ${JSON.stringify(catalogObs.stale)}`);
  assert.equal(catalogObs.stale?.selected, true, `A failed read must keep the selection: ${JSON.stringify(catalogObs.stale)}`);
  assert.equal(catalogObs.stale?.disabled, false, `A failed read must keep the model enabled: ${JSON.stringify(catalogObs.stale)}`);
  assert.equal(catalogObs.stale?.direct?.code, catalogObs.deselected?.admissionWhileSelected?.code, `A stale entry must keep the direct-call admission result: ${JSON.stringify(catalogObs.stale)}`);
  // 权威移除：不可用、移出目录，但配置、选择与稳定标识都保留。
  assert.equal(catalogObs.removed?.availability, 'removed', `An authoritatively removed model must be marked removed: ${JSON.stringify(catalogObs.removed)}`);
  assert.equal(catalogObs.removed?.direct?.code, 'model_removed', `A removed model must be denied with its own code: ${JSON.stringify(catalogObs.removed)}`);
  assert.equal(catalogObs.removed?.internal_id, discovered.internal_id, `An authoritative removal must not change the internal ID: ${JSON.stringify(catalogObs.removed)}`);
  assert.equal(catalogObs.removed?.selected, true, `An authoritative removal must keep the selection: ${JSON.stringify(catalogObs.removed)}`);
  assert.equal(catalogObs.removed?.disabled, false, `An authoritative removal must keep the model enabled: ${JSON.stringify(catalogObs.removed)}`);
  assert.equal(catalogObs.removed?.publicCatalog?.includes(catalogPublicId), false, `A removed model must stay out of the public catalog: ${JSON.stringify(catalogObs.removed?.publicCatalog)}`);
  // 换号：资格整体作废（保留选择/停用与标识），重新登录并核对后恢复可用。
  assert.equal(catalogObs.switched?.invalidated?.availability, 'unknown', `Switching accounts must invalidate the confirmed catalogue: ${JSON.stringify(catalogObs.switched)}`);
  assert.equal(catalogObs.switched?.invalidated?.eligibility, 'account_changed', `Switching accounts must rebind eligibility: ${JSON.stringify(catalogObs.switched)}`);
  assert.equal(catalogObs.switched?.invalidated?.selected, true, `Switching accounts must keep the selection: ${JSON.stringify(catalogObs.switched)}`);
  assert.equal(catalogObs.switched?.invalidated?.disabled, true, `Switching accounts must keep the disable state: ${JSON.stringify(catalogObs.switched)}`);
  assert.ok(catalogObs.switched?.to?.generation > catalogObs.switched?.from?.generation, `Switching accounts must advance the generation: ${JSON.stringify(catalogObs.switched)}`);
  assert.equal(catalogObs.switched?.reverified?.availability, 'available', `A re-verified catalogue must be available again: ${JSON.stringify(catalogObs.switched?.reverified)}`);
  assert.equal(catalogObs.switched?.reverified?.eligibility, 'eligible', `A re-verified catalogue must be eligible again: ${JSON.stringify(catalogObs.switched?.reverified)}`);
  assert.equal(catalogObs.switched?.reverified?.internal_id, discovered.internal_id, `Re-verifying must reuse the stable internal ID: ${JSON.stringify(catalogObs.switched?.reverified)}`);
  assert.equal(catalogObs.switched?.reverified?.disabled, true, `The preserved disable state must survive the switch: ${JSON.stringify(catalogObs.switched?.reverified)}`);
  assert.equal(catalogObs.switched?.direct?.code, 'model_disabled', `The preserved disable state must still deny calls: ${JSON.stringify(catalogObs.switched)}`);
  // 删除模型行：目录项与稳定标识保留，下一次核对用同一个 internal_id 重建。
  assert.equal(catalogObs.deletedRow?.restored, discovered.internal_id, `Deleting a row must not drift the internal ID: ${JSON.stringify(catalogObs.deletedRow)}`);
  // Agent 保存目录与待同步：外部待同步不推迟后端撤销。
  assert.equal(catalogObs.pendingSync?.connected?.pending, false, `A freshly saved Agent catalog must not be pending: ${JSON.stringify(catalogObs.pendingSync)}`);
  assert.equal(catalogObs.pendingSync?.pending, true, `An out-of-date Agent catalog must be reported as pending: ${JSON.stringify(catalogObs.pendingSync)}`);
  assert.equal(catalogObs.pendingSync?.pendingWhileDisabled, true, `The pending flag must survive a backend revocation: ${JSON.stringify(catalogObs.pendingSync)}`);
  assert.equal(catalogObs.pendingSync?.admissionWhilePending?.code, catalogObs.deselected?.admissionWhileSelected?.code, `An external pending sync must not change backend admission: ${JSON.stringify(catalogObs.pendingSync)}`);
  assert.equal(catalogObs.pendingSync?.admissionAfterDisable?.code, 'model_disabled', `The backend revocation must not wait for the external configuration: ${JSON.stringify(catalogObs.pendingSync)}`);
  // 收起会话：注销走生产路径，目录资格整体作废（凭据清理由既有的专用授权目录断言覆盖）。
  assert.equal(catalogObs.closedOut?.eligibility, 'account_changed', `Sign-out must invalidate the catalogue eligibility: ${JSON.stringify(catalogObs.closedOut)}`);
  assert.ok(catalogObs.closedOut?.logout, `Sign-out must report its local and remote outcome: ${JSON.stringify(catalogObs.closedOut)}`);
  assert.ok(!catalogObs.closedOut?.identity, `Sign-out must clear the verified identity: ${JSON.stringify(catalogObs.closedOut)}`);
  // 全程替身上游零派发：受控目录运行没有把任何目录模型送到真实上游。
  assert.equal(requests.filter(entry => JSON.stringify(entry.body ?? {}).includes('codex-catalog-alpha')).length, 0, 'A controlled catalogue model must never reach an upstream');
  // #15：替身侧佐证——catalog 运行确实按队列次序、每次刷新各收到一次目录与额度读取，且两者互不串线。
  // 真正的验收断言在 isolation-check.js 的 catalog 模式里，取自界面稳定文本与后端 snapshot，不依赖这些日志。
  const readScenariosSeen = logs.catalog.filter(entry => entry.event === 'lifecycle' && entry.action === 'quota-read').map(entry => entry.scenario);
  const catalogScenariosSeen = logs.catalog.filter(entry => entry.event === 'lifecycle' && entry.action === 'catalog-read').map(entry => entry.scenario);
  assert.deepEqual(readScenariosSeen, catalogReads.split(','), `The quota queue must be consumed in order: ${JSON.stringify(readScenariosSeen)}`);
  assert.deepEqual(catalogScenariosSeen, catalogScenarios.split(','), `The catalog queue must be consumed in order: ${JSON.stringify(catalogScenariosSeen)}`);
  assert.ok(logs.catalog.some(entry => entry.event === 'request' && entry.method === 'model/list'), 'The catalog run must issue model/list');
  assert.ok(logs.catalog.some(entry => entry.event === 'request' && entry.method === 'account/rateLimits/read'), 'The catalog run must issue account/rateLimits/read');
  assert.equal(logs.catalog.filter(entry => entry.event === 'request' && entry.method === 'account/login/start').length, 2, 'The catalog run signs in again before testing helper-side disconnect');
  const accountScenariosSeen = logs.catalog.filter(entry => entry.action === 'account-read').map(entry => entry.scenario);
  assert.deepEqual(accountScenariosSeen.slice(-5), ['incomplete', 'connected', 'connected', 'connected', 'signed-out']);
  const incompleteIndex = logs.catalog.findIndex(entry => entry.action === 'account-read' && entry.scenario === 'incomplete');
  const nextAccountIndex = logs.catalog.findIndex((entry, index) => index > incompleteIndex && entry.action === 'account-read');
  assert.ok(incompleteIndex >= 0 && nextAccountIndex > incompleteIndex);
  assert.ok(!logs.catalog.slice(incompleteIndex + 1, nextAccountIndex).some(entry =>
    entry.method === 'model/list' || entry.method === 'account/rateLimits/read'), 'No new evidence is read for an unconfirmed identity');
  const helperOutIndex = logs.catalog.findLastIndex(entry => entry.action === 'account-read' && entry.scenario === 'signed-out');
  const lastCatalogIndex = logs.catalog.findLastIndex(entry => entry.action === 'catalog-read');
  assert.ok(helperOutIndex > lastCatalogIndex);
  assert.ok(!logs.catalog.slice(lastCatalogIndex, helperOutIndex).some(entry => entry.method === 'account/logout'),
    'Helper disconnect is reached by refresh after connected A, with no intervening app logout');
  const catalogChecks = catalogRun.report.checks || [];
  for (const [pattern, label] of [['eligible=false', 'eligible=false'], ['permission=denied', 'permission=denied'], ['fail-closed', 'bucket-level fail-closed'], ['stale', 'stale catalog'], ['historical', 'historical quota'], ['invalid', 'invalid fields'], ['missing', 'missing fields'], ['remaining', 'remaining derivation']]) {
    assert.ok(catalogChecks.some(name => name.includes(pattern)), `The catalog run must assert ${label}: ${JSON.stringify(catalogChecks)}`);
  }
  console.log(`Catalog rehearsal scenarios: quota=${JSON.stringify(readScenariosSeen)} catalog=${JSON.stringify(catalogScenariosSeen)}`);
  // 第 6 条：桌面退出后替身子进程被回收，且未波及无关进程。
  const helperPidsSeen = new Set(allEntries.map(entry => entry.pid).filter(Boolean));
  const liveObserved = new Set(runs.flatMap(run => [...run.liveDuringRun]));
  assert.ok(liveObserved.size > 0, 'At least one stand-in child must have been observed alive while its desktop ran');
  assert.ok(alive(sentinel.pid), 'Unrelated processes must survive the desktop runs');
  for (const pid of helperPidsSeen) assert.ok(!alive(pid), `Stand-in child ${pid} must be reaped after the desktop exits`);
  // 既有 #11 断言：真实上游仍只收到固定回环替身的请求。
  assert.ok(requests.some(r => r.headers['user-agent'] === 'AutoJev/ProviderTest' && r.body.messages[0].content === 'Say OK'));
  assert.equal(requests.filter(r => r.headers['user-agent'] === 'AutoJev/ModelSpeedTest').length, 3);
  assert.equal(requests.filter(r => r.headers['user-agent'] === 'AutoJev/Debug').length, 3);
  assert.ok(requests.filter(r => !r.path.startsWith('/redirect/')).every(r => r.body.model === 'fixture-model' && r.headers.authorization === 'Bearer fixture-key'));
  // AC4 收尾：全部运行结束后专用授权目录仍然为空，且用户真实 ~/.codex 未被创建或修改（只看元数据）。
  const finalEntries = helperHomeEntries();
  assert.ok(finalEntries === null || finalEntries.length === 0, `The dedicated helper home must stay empty after the runs: ${JSON.stringify(finalEntries)}`);
  const realCodexAfter = codexHomeFingerprint();
  assert.equal(realCodexAfter.exists, realCodexBefore.exists, `The real user ~/.codex must not be created or deleted by the isolated runs: ${JSON.stringify({ before: realCodexBefore, after: realCodexAfter })}`);
  if (realCodexBefore.exists) assert.equal(realCodexAfter.ino, realCodexBefore.ino, `The real user ~/.codex must not be replaced by the isolated runs: ${JSON.stringify({ before: realCodexBefore, after: realCodexAfter })}`);
  // 可归因的泄漏探针：本应用替身的虚构凭据文件绝不能出现在真实账号目录里。
  const leakedCredential = join(realCodexHome, 'fictional-auth.json');
  assert.ok(!statSync(leakedCredential, { throwIfNoEntry: false }), `The isolated stand-in artifact must never appear in the real account directory: ${leakedCredential}`);
  if (realCodexNoisy) {
    console.log(`Real ~/.codex untouched by this run (existence and inode unchanged, no stand-in artifact); mtime drifted externally (${realCodexBefore.mtimeMs} -> ${realCodexAfter.mtimeMs}) so mtime equality is not asserted.`);
  } else {
    assert.deepEqual(realCodexAfter, realCodexBefore, 'The real user ~/.codex must not be created or modified by the isolated runs');
    console.log('Real ~/.codex mtime unchanged across the isolated runs (quiet environment).');
  }
  // 授权链接 ACL：只做静态配置断言；隔离环境不真实打开浏览器，真实打开未执行。
  const capabilities = JSON.parse(await readFile(resolve('src-tauri/capabilities/default.json'), 'utf8'));
  const opener = (capabilities.permissions || []).find(entry => entry && entry.identifier === 'opener:allow-open-url');
  assert.ok(opener, 'The desktop capability must allow opening authorization URLs');
  const allowedUrls = (opener.allow || []).map(entry => entry.url || '');
  assert.ok(allowedUrls.some(url => url.includes('auth.openai.com')), `The OpenAI authorization host must be allowed: ${JSON.stringify(allowedUrls)}`);
  assert.ok(allowedUrls.some(url => url.includes('chatgpt.com')), `The ChatGPT authorization host must be allowed: ${JSON.stringify(allowedUrls)}`);
  // 只读块的官方查看入口走同一条 openUrl 路径：桌面端必须允许该主机，否则未知/unsupported 的兜底入口实际打不开。
  assert.ok(allowedUrls.some(url => url.includes('docs.x.ai')), `The Grok reference host must be allowed: ${JSON.stringify(allowedUrls)}`);
  console.log('Authorization-link ACL asserted statically (auth.openai.com, chatgpt.com, docs.x.ai); the real browser open was NOT executed.');
  console.log(`Native desktop login lifecycle acceptance passed. Fictional evidence: ${root}`);
} finally {
  await writeFile(join(root, 'requests.json'), JSON.stringify(requests, null, 2));
  if (sentinel?.exitCode === null) sentinel.kill('SIGTERM');
  if (desktop?.exitCode === null) desktop.kill('SIGTERM');
  vite.kill('SIGTERM'); fixture.closeAllConnections(); fixture.close();
  console.log(`Evidence retained: ${root}`);
}
