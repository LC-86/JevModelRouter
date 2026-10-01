#!/usr/bin/env node
// Fictional Codex app-server stand-in for the isolated acceptance build only.
// It speaks newline-delimited JSON-RPC 2.0 over stdio and never performs real work.
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';

const queue = (name, fallback) => (process.env[name] || fallback)
  .split(',').map(value => value.trim()).filter(Boolean);
const scenarios = queue('AUTOJEV_FAKE_HELPER_SCENARIOS', 'success');
// 额度（account/rateLimits/read）与目录（model/list）各自一条场景队列：每次桌面运行只排练一组，
// 按该方法的调用次序取场景，队列用完后重复最后一项（与登录场景队列的既有语义一致）。
const readScenarios = queue('AUTOJEV_FAKE_HELPER_READS', 'multi');
const catalogScenarios = queue('AUTOJEV_FAKE_HELPER_CATALOG', 'success');
const accountScenarios = queue('AUTOJEV_FAKE_HELPER_ACCOUNTS', 'connected');
const logPath = process.env.AUTOJEV_FAKE_HELPER_LOG || '';
const delay = Number(process.env.AUTOJEV_FAKE_HELPER_DELAY_MS || 150);
const codexHome = process.env.CODEX_HOME || '';
const secretNames = ['OPENAI_API_KEY', 'CODEX_API_KEY', 'CODEX_ACCESS_TOKEN', 'CODEX_REFRESH_TOKEN', 'CHATGPT_ACCESS_TOKEN'];
const envSecrets = secretNames.filter(name => (process.env[name] || '').length > 0);
const realHome = resolve(homedir());
const realCodexHome = join(realHome, '.codex');
const resolvedHome = codexHome ? resolve(codexHome) : '';
const isolatedHome = Boolean(resolvedHome) && resolvedHome !== realHome && resolvedHome !== realCodexHome && !resolvedHome.startsWith(realCodexHome + '/');
// "Real credentials" means: the app leaked a credential variable, or did not point CODEX_HOME at an isolation path.
const realCredentials = envSecrets.length > 0 || !isolatedHome;
const identities = { success: 'standin-success@example.invalid', late: 'standin-late@example.invalid' };
const fictionalTokens = ['fictional-access-token-4f2c9a', 'fictional-refresh-token-8b1d7e'];

const log = entry => {
  if (!logPath) return;
  appendFileSync(logPath, JSON.stringify({
    ts: new Date().toISOString(), pid: process.pid, codexHome,
    isolatedHome, envSecrets, realCredentials, ...entry,
  }) + '\n');
};
const send = message => process.stdout.write(JSON.stringify(message) + '\n');

// 一个隔离桌面运行内 helper 可在 logout 后重启；只读队列仍按该运行的 RPC 次序消费。
let previousEntries = [];
try { if (logPath) previousEntries = readFileSync(logPath, 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line)); } catch {}
const previousReads = action => previousEntries.filter(entry => entry.action === action).length;
const state = { attempt: 0, readIndex: previousReads('quota-read'), catalogIndex: previousReads('catalog-read'),
  accountIndex: previousReads('account-read'), account: null, pendingLate: new Map(), timers: new Map() };
const scenarioFor = index => scenarios[Math.min(index, scenarios.length - 1)] || 'success';
const nextScenario = (list, index, fallback) => list[Math.min(index, list.length - 1)] || fallback;
const nextRead = () => nextScenario(readScenarios, state.readIndex++, 'multi');
const nextCatalog = () => nextScenario(catalogScenarios, state.catalogIndex++, 'success');

// 契约 A：目录响应。`data` 是已核验的真实主形状（含 nextCursor），`models` 是契约 A 仍要求
// 兼容的旧形状（只在排练场景里产出，不代表当前辅助进程版本）。
const catalogModels = [
  { id: 'codex-fixture-model', model: 'codex-fixture-model', displayName: 'Codex Fixture', hidden: false, isDefault: true },
  { model: 'codex-fixture-legacy-id', name: 'Codex Fixture Legacy', hidden: false, isDefault: false },
];
const catalogResponse = scenario => {
  if (scenario === 'fail-catalog') return { error: { code: -32000, message: 'fictional catalog read failure' } };
  if (scenario === 'legacy') return { result: { models: [{ id: 'codex-fixture-legacy-shape', displayName: 'Legacy Shape' }] } };
  if (scenario === 'missing') return { result: { data: [{ displayName: 'No usable identifier' }, catalogModels[0]], nextCursor: null } };
  return { result: { data: catalogModels, nextCursor: null } };
};

// 契约 A：额度响应。所有数字都是虚构值，绝不代表真实账号、真实额度或任何金额。
// 已核验的真实形状里 ordinaryUsageAllowed 位于结果根层，rateLimits（旧版单桶）恒存在；
// `single` 场景刻意省略 rateLimitsByLimitId 以排练旧形状分支，`multi` 场景同时给出两者以核对多桶优先。
const window = (usedPercent, windowDurationMins, resetsAt) => ({ usedPercent, windowDurationMins, resetsAt });
const quotaRoot = (rates, ordinaryUsageAllowed) => {
  const result = {
    accountId: 'fictional-account-0001',
    rateLimits: rates.rateLimits ?? Object.values(rates.rateLimitsByLimitId ?? {})[0] ?? null,
    rateLimitResetCredits: { availableCount: 2, totalCount: 5 },
    rateLimitUpsell: null,
  };
  if (rates.rateLimitsByLimitId) result.rateLimitsByLimitId = rates.rateLimitsByLimitId;
  // 缺失与 null 必须可区分：`no-permission` 完全省略该键，`null-permission` 显式给 null。
  if (ordinaryUsageAllowed !== 'absent') result.ordinaryUsageAllowed = ordinaryUsageAllowed;
  return result;
};
const quotaResponse = scenario => {
  if (scenario === 'fail-quota') return { error: { code: -32000, message: 'fictional quota read failure' } };
  if (scenario === 'single') {
    return { result: quotaRoot({ rateLimits: {
      limitId: 'fictional-single', limitName: 'Fictional single snapshot', planType: 'fictional-plus',
      primary: window(33, 120, 1767232800),
      credits: { hasCredits: true, unlimited: false, balance: '3 fictional credits' },
    } }, true) };
  }
  if (scenario === 'missing') {
    return { result: quotaRoot({ rateLimitsByLimitId: { 'fictional-missing': {
      limitId: 'fictional-missing', limitName: 'Fictional missing fields',
      primary: { usedPercent: 55, windowDurationMins: 300 },
    } } }, true) };
  }
  if (scenario === 'invalid') {
    return { result: quotaRoot({ rateLimitsByLimitId: { 'fictional-invalid': {
      limitId: 'fictional-invalid', limitName: 'Fictional invalid fields',
      primary: window(142, -1, 0),
      secondary: window(50, 60, 1767232800),
    } } }, true) };
  }
  if (scenario === 'denied') {
    return { result: quotaRoot({ rateLimitsByLimitId: { 'fictional-denied': {
      limitId: 'fictional-denied', limitName: 'Fictional denied ordinary usage',
      primary: window(12, 60, 1767232800),
    } } }, false) };
  }
  if (scenario === 'bucket-denied') {
    // fail-closed：根层普通许可为 true，但桶内显式 false 不得被覆盖。
    return { result: quotaRoot({ rateLimitsByLimitId: { 'fictional-bucket-denied': {
      limitId: 'fictional-bucket-denied', limitName: 'Fictional bucket-level denial',
      primary: window(15, 60, 1767232800),
      credits: { hasCredits: true, unlimited: false, balance: '4 fictional credits' },
      ordinaryUsageAllowed: false,
    } } }, true) };
  }
  if (scenario === 'no-permission') {
    return { result: quotaRoot({ rateLimitsByLimitId: { 'fictional-noperm': {
      limitId: 'fictional-noperm', limitName: 'Fictional absent permission',
      primary: window(20, 60, 1767232800),
      credits: { hasCredits: true, unlimited: false, balance: '1 fictional credit' },
    } } }, 'absent') };
  }
  if (scenario === 'null-permission') {
    return { result: quotaRoot({ rateLimitsByLimitId: { 'fictional-nullperm': {
      limitId: 'fictional-nullperm', limitName: 'Fictional null permission',
      primary: window(21, 60, 1767232800),
      credits: { hasCredits: true, unlimited: false, balance: '2 fictional credits' },
    } } }, null) };
  }
  return { result: quotaRoot({ rateLimitsByLimitId: {
    'fictional-primary': {
      limitId: 'fictional-primary', limitName: 'Fictional primary window', planType: 'fictional-plus',
      primary: window(42, 300, 1767225600),
      secondary: window(7, 10080, 1767830400),
      credits: { hasCredits: true, unlimited: false, balance: '12.5 fictional credits' },
    },
    'fictional-secondary': {
      limitId: 'fictional-secondary', limitName: 'Fictional secondary window',
      primary: window(88, 60, 1767229200),
      credits: { hasCredits: false, unlimited: false, balance: '0 fictional credits' },
    },
  },
  // 旧版单桶同时存在：多桶优先，断言里旧桶不得出现。
  rateLimits: {
    limitId: 'fictional-legacy-decoy', limitName: 'Fictional legacy decoy', planType: 'fictional-plus',
    primary: window(99, 60, 1767232800),
    credits: { hasCredits: true, unlimited: true, balance: 'decoy fictional credits' },
  } }, true) };
};

const complete = (loginId, scenario, index) => {
  const success = scenario !== 'failed';
  const account = success ? { type: 'chatgpt', email: scenario === 'late' ? identities.late : identities.success, planType: 'fictional-plus' } : undefined;
  if (account) state.account = account;
  const params = { loginId, success };
  if (!success) params.error = `login failed: refresh_token=${fictionalTokens[1]}&access_token=${fictionalTokens[0]}`;
  send({ jsonrpc: '2.0', method: 'account/login/completed', params });
  // 只有真正的成功登录才在专用 CODEX_HOME 留一个虚构凭据文件：这是「引用隔离 + 受控存储 +
  // 退出清理」的可观察证据。替身自己绝不删除它——清理必须由应用在注销时完成。
  if (scenario === 'success' && codexHome) {
    const credentialPath = join(codexHome, 'fictional-auth.json');
    try {
      mkdirSync(codexHome, { recursive: true });
      writeFileSync(credentialPath, JSON.stringify({ email: identities.success, access_token: fictionalTokens[0], refresh_token: fictionalTokens[1] }) + '\n');
      log({ event: 'lifecycle', method: null, action: 'credential-file-written', path: credentialPath });
    } catch (error) {
      log({ event: 'lifecycle', method: null, action: 'credential-file-error', error: String((error && error.message) || error) });
    }
  }
  log({ event: 'notification', method: 'account/login/completed', loginId, success, scenario, attempt: index, error: params.error });
};
const handlers = {
  initialize: () => ({ result: { version: '0.0.0-fictional', codexHome } }),
  'account/login/start': params => {
    if (params.type !== 'chatgpt') {
      return { error: { code: -32602, message: 'Invalid request: missing field type' } };
    }
    const index = state.attempt++;
    const scenario = scenarioFor(index);
    const loginId = `fictional-login-${index + 1}`;
    log({ event: 'lifecycle', method: null, action: 'login-started', loginId, scenario, attempt: index });
    if (scenario === 'late') {
      // A `late` login only completes once the app cancels it or reads the account again.
      state.pendingLate.set(loginId, { index });
    } else {
      const timer = setTimeout(() => { state.timers.delete(loginId); complete(loginId, scenario, index); }, delay);
      state.timers.set(loginId, timer);
    }
    return { result: { type: 'chatgpt', loginId, authUrl: `https://fictional.invalid/authorize?login=${loginId}` } };
  },
  'account/login/cancel': params => {
    const loginId = String(params.loginId || '');
    const timer = state.timers.get(loginId);
    if (timer) { clearTimeout(timer); state.timers.delete(loginId); }
    const pending = state.pendingLate.get(loginId);
    if (pending) { state.pendingLate.delete(loginId); complete(loginId, 'late', pending.index); }
    return { result: { cancelled: true } };
  },
  'account/read': () => {
    // Reading the account is the second trigger that flushes a pending `late` completion.
    for (const [loginId, pending] of [...state.pendingLate]) { state.pendingLate.delete(loginId); complete(loginId, 'late', pending.index); }
    const scenario = state.account ? nextScenario(accountScenarios, state.accountIndex++, 'connected') : 'signed-out';
    log({ event: 'lifecycle', method: null, action: 'account-read', scenario });
    if (scenario === 'signed-out') state.account = null;
    const account = scenario === 'incomplete' && state.account ? { ...state.account, email: null } : state.account;
    return { result: { account, requiresOpenaiAuth: true } };
  },
  'account/logout': () => {
    state.account = null;
    return { result: { local: 'cleared', remote: 'revoked' } };
  },
  // 契约 A 的两个只读读取：目录与额度各自独立取场景，成功与失败都在 JSONL 里可核对。
  'model/list': () => {
    const scenario = nextCatalog();
    log({ event: 'lifecycle', method: null, action: 'catalog-read', scenario, index: state.catalogIndex - 1 });
    return catalogResponse(scenario);
  },
  'account/rateLimits/read': () => {
    const scenario = nextRead();
    log({ event: 'lifecycle', method: null, action: 'quota-read', scenario, index: state.readIndex - 1 });
    return quotaResponse(scenario);
  },
};

const handleLine = line => {
  if (!line.trim()) return;
  let message;
  try { message = JSON.parse(line); } catch { log({ event: 'malformed', method: null, line }); return; }
  if (!message || typeof message.method !== 'string') return;
  if (message.id === undefined) { log({ event: 'notification-in', method: message.method, params: message.params ?? null }); return; }
  log({ event: 'request', method: message.method, id: message.id, params: message.params ?? null });
  const handler = handlers[message.method];
  if (!handler) { send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: `Unsupported method: ${message.method}` } }); return; }
  try { send({ jsonrpc: '2.0', id: message.id, ...handler(message.params || {}) }); }
  catch (error) { send({ jsonrpc: '2.0', id: message.id, error: { code: -32603, message: String((error && error.message) || error) } }); }
};

let exiting = false;
// 子进程退出原因：`readline-close` 表示父进程消失后 stdin 关闭（未被孤儿化留下）。
const shutdown = (code, reason) => {
  if (exiting) return;
  exiting = true;
  log({ event: 'lifecycle', method: null, action: 'exit', code, reason });
  // Give any just-written response a chance to reach the parent before the process ends.
  setTimeout(() => process.exit(code), 120);
};
log({ event: 'lifecycle', method: null, action: 'started', scenarios, readScenarios, catalogScenarios, delay, argv: process.argv.slice(2) });
createInterface({ input: process.stdin, crlfDelay: Infinity }).on('line', handleLine).on('close', () => shutdown(0, 'readline-close'));
// A well-behaved stdio client child dies when its parent disappears and the pipe closes.
process.stdin.on('end', () => shutdown(0, 'stdin-end'));
process.stdin.on('error', error => log({ event: 'lifecycle', method: null, action: 'stdin-error', error: String((error && error.message) || error) }));
process.stdout.on('error', error => log({ event: 'lifecycle', method: null, action: 'stdout-error', error: String((error && error.message) || error) }));
process.on('SIGTERM', () => shutdown(0, 'sigterm'));
process.on('SIGINT', () => shutdown(0, 'sigint'));
process.on('uncaughtException', error => { log({ event: 'lifecycle', method: null, action: 'crash', error: String((error && error.message) || error) }); shutdown(70, 'uncaught-exception'); });
