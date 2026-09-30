#!/usr/bin/env node
// Fictional Codex app-server stand-in for the isolated acceptance build only.
// It speaks newline-delimited JSON-RPC 2.0 over stdio and never performs real work.
import { appendFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';

const scenarios = (process.env.AUTOJEV_FAKE_HELPER_SCENARIOS || 'success')
  .split(',').map(value => value.trim()).filter(Boolean);
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

const state = { attempt: 0, account: null, pendingLate: new Map(), timers: new Map() };
const scenarioFor = index => scenarios[Math.min(index, scenarios.length - 1)] || 'success';

const complete = (loginId, scenario, index) => {
  const ok = scenario !== 'failed';
  const account = ok ? { email: scenario === 'late' ? identities.late : identities.success, planType: 'fictional-plus' } : undefined;
  if (account) state.account = account;
  const params = { loginId, ok };
  if (account) params.account = account;
  if (!ok) params.error = `login failed: refresh_token=${fictionalTokens[1]}&access_token=${fictionalTokens[0]}`;
  send({ jsonrpc: '2.0', method: 'account/login/completed', params });
  log({ event: 'notification', method: 'account/login/completed', loginId, ok, scenario, attempt: index, error: params.error });
};
const handlers = {
  initialize: () => ({ result: { version: '0.0.0-fictional', codexHome } }),
  'account/login/start': () => {
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
    return { result: { loginId, authorizationUrl: `https://fictional.invalid/authorize?login=${loginId}`, userCode: `FICT-${index + 1}` } };
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
    return { result: state.account ? { account: state.account, requiresAuth: false } : { requiresAuth: true } };
  },
  'account/logout': () => {
    state.account = null;
    return { result: { local: 'cleared', remote: 'revoked' } };
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
log({ event: 'lifecycle', method: null, action: 'started', scenarios, delay, argv: process.argv.slice(2) });
createInterface({ input: process.stdin, crlfDelay: Infinity }).on('line', handleLine).on('close', () => shutdown(0, 'readline-close'));
// A well-behaved stdio client child dies when its parent disappears and the pipe closes.
process.stdin.on('end', () => shutdown(0, 'stdin-end'));
process.stdin.on('error', error => log({ event: 'lifecycle', method: null, action: 'stdin-error', error: String((error && error.message) || error) }));
process.stdout.on('error', error => log({ event: 'lifecycle', method: null, action: 'stdout-error', error: String((error && error.message) || error) }));
process.on('SIGTERM', () => shutdown(0, 'sigterm'));
process.on('SIGINT', () => shutdown(0, 'sigint'));
process.on('uncaughtException', error => { log({ event: 'lifecycle', method: null, action: 'crash', error: String((error && error.message) || error) }); shutdown(70, 'uncaught-exception'); });