#!/usr/bin/env node
// Fictional Grok CLI stand-in for the isolated acceptance build only (issue #16).
// It answers the read-only subcommands (`account|models|usage --json`) with the frozen
// line-JSON events of docs/testing/grok-catalog-quota.md §2 and never performs real work.
//
// Scenario queue: `AUTOJEV_GROK_FAKE_SCENARIOS` is the authoritative name (it reaches the helper
// through `helper.rs`'s `AUTOJEV_GROK_*` env whitelist); `AUTOJEV_FAKE_GROK_SCENARIOS` is kept as a
// compatibility alias carrying the same value. Read order is not significant. One entry is consumed
// per helper invocation: the invocation index is the current line count of the JSONL log, which is
// safe because the adapter reads a provider's evidence strictly sequentially (account → models → usage).
import { appendFileSync, readFileSync } from 'node:fs';

const queueKeys = ['AUTOJEV_FAKE_GROK_SCENARIOS', 'AUTOJEV_GROK_FAKE_SCENARIOS'];
const logKeys = ['AUTOJEV_FAKE_GROK_LOG', 'AUTOJEV_GROK_FAKE_LOG'];
const firstDefined = keys => keys.map(key => process.env[key]).find(value => value !== undefined && value !== '') || '';
const scenarios = firstDefined(queueKeys).split(',').map(value => value.trim()).filter(Boolean);
const logPath = firstDefined(logKeys);
const argv = process.argv.slice(2);
const subcommand = argv[0] || '';
const home = process.env.GROK_HOME || process.env.HOME || '';

const invocations = () => {
  try { return readFileSync(logPath, 'utf8').split('\n').filter(Boolean).length; } catch { return 0; }
};
const index = logPath ? invocations() : 0;
const scenario = scenarios[index] || 'success';

// 身份与时间都是固定虚构值：断言可以据此证明「保留历史」而不是重新伪造。
const identity = 'standin-grok@example.invalid';
const catalogSource = 'grok-cli:models';
const catalogObservedAt = '2026-01-02T03:04:05Z';
const quotaSource = 'grok-cli:usage';
const quotaObservedAt = '2026-01-02T03:04:06Z';
const surviving = { id: 'grok-build', display_name: 'Grok Build' };
const retired = { id: 'grok-mini', display_name: 'Grok Mini' };

if (logPath) {
  appendFileSync(logPath, JSON.stringify({
    ts: new Date().toISOString(), pid: process.pid, argv, subcommand, scenario, index, home,
    // 边界证据：辅助进程只应看到白名单环境（PATH/TMPDIR/LANG/TERM + AUTOJEV_GROK_* + GROK_HOME/HOME）。
    envKeys: Object.keys(process.env).sort(),
  }) + '\n');
}

const emit = value => process.stdout.write(JSON.stringify(value) + '\n');
const done = () => emit({ event: 'done' });

const account = () => { emit({ event: 'account', identity }); done(); };
const catalog = models => { emit({ event: 'catalog', source: catalogSource, observed_at: catalogObservedAt, models }); done(); };
const quota = usageAllowed => {
  emit({
    event: 'quota', source: quotaSource, observed_at: quotaObservedAt,
    plan: { code: 'standin-plan', name: 'Stand-in plan' },
    pool: { period: { type: 'monthly', end: '2026-02-01T00:00:00Z' }, used_percent: 42.5, usage_allowed: usageAllowed },
    extra_usage: { has_credits: true, unlimited: false, balance: '12.50', unit: 'USD', permitted: false },
  });
  done();
};
const unsupported = what => { emit({ event: 'unsupported', interface: what }); done(); };

if (subcommand === 'account') {
  // 账号是只读读取的前置：除显式失败场景外一律如实报到同一个虚构身份。
  if (scenario === 'fail-account') { emit({ event: 'error', code: 'account_unavailable', message: 'stand-in account read failed' }); done(); }
  else account();
} else if (subcommand === 'models') {
  if (scenario === 'unsupported-catalog') unsupported('catalog');
  else if (scenario === 'removed') catalog([surviving]);
  else if (scenario === 'fail-catalog') { emit({ event: 'error', code: 'catalog_unavailable', message: 'stand-in catalog read failed' }); done(); }
  else catalog([surviving, retired]);
} else if (subcommand === 'usage') {
  if (scenario === 'fail-quota') { emit({ event: 'error', code: 'quota_unavailable', message: 'stand-in quota read failed' }); done(); }
  else if (scenario === 'unsupported-quota') unsupported('quota');
  else quota(scenario !== 'denied');
} else {
  emit({ event: 'error', code: 'unknown_subcommand', message: `unsupported subcommand: ${subcommand}` });
  done();
}
