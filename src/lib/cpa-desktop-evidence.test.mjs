import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { expect, it } from 'vitest';

const driver = readFileSync(new URL('../../src-tauri/src/cpa-validation-check.js', import.meta.url), 'utf8');
const accounts = ['a1', 'a2', 'b1', 'paid'];
const base = 'http://127.0.0.1:32101';
const cpaBase = 'http://127.0.0.1:32102';
const receiver = (prompt, account = 'a1') => ({ account, key: `Bearer fictional-${account}`, model: 'same-model', prompt });

// Exercise the actual driver's report at its IPC/HTTP boundaries, without CPA or GUI.
async function acceptanceReport(fault = {}) {
  const history = {
    requests: [receiver('hang-cancel'), receiver('hang-cancel@previous-desktop')],
    cancelled: ['hang-timeout', 'hang-cancel', 'hang-timeout@previous-desktop', 'hang-cancel@previous-desktop'],
  };
  const stats = { received: 0, cancelled: 0, timeoutCancelled: 0 };
  const snapshot = {
    providers: accounts.map(account => ({ id: `cpa-${account}`, name: `${account.startsWith('a') ? 'source-a' : account === 'b1' ? 'source-b' : 'source-paid'} / ${account} / fixture-plan-${account}`, base_url: cpaBase })),
    models: accounts.map(account => ({ id: `model-${account}`, provider_id: `cpa-${account}`, model_id: `jev-${account}/same-model` })),
    gateway: {}, proxy: { port: 32103 },
  };
  let profile, mode = 'ok', cooled = false, paused = false, clock = 0, report, latePreviousRequest = false;
  const invoke = async (command, args = {}) => {
    if (command === 'get_snapshot' || command === 'save_gateway_settings') return structuredClone(snapshot);
    if (command === 'start_proxy') { paused = false; return structuredClone(snapshot); }
    if (command === 'pause_proxy') { paused = true; return; }
    if (command === 'reset_gateway_health' || command === 'stop_cpa_validation') return;
    if (command === 'isolation_check_report') { report = structuredClone(args.report); return; }
    if (command === 'start_cpa_validation') {
      if (args.binary.endsWith('-missing')) throw new Error('executable is missing');
      if (args.binary === '/bin/echo') throw new Error('version/checksum');
      if (args.profile.port === 32101) throw new Error('port is already occupied');
      if (args.profile.targets[0].plan !== 'fixture-plan-a1') throw new Error('Cannot rebind');
      profile = structuredClone(args.profile); cooled = false;
      return { base_url: cpaBase, pid: 100, targets: profile.targets };
    }
    if (command === 'reload_cpa_validation') {
      const targets = args.profile.targets;
      if (args.profile.upstream !== base) throw new Error('explicitly configured loopback fixture');
      if (new Set(targets.map(t => t.prefix)).size !== targets.length) throw new Error('Conflicting CPA prefix');
      if (targets.some(t => t.keys.length !== 1)) throw new Error('exactly one credential');
      if (targets.some(t => t.aliases.length !== 1)) throw new Error('Shared alias');
      if (targets.some(t => t.plan !== `fixture-plan-${t.account}`)) throw new Error('Cannot rebind');
      profile = structuredClone(args.profile); return;
    }
    throw new Error(`Unexpected IPC command: ${command}`);
  };
  const fetch = async (url, options) => {
    const body = JSON.parse(options.body);
    let data;
    if (url.endsWith('/__progress')) data = {};
    else if (url.endsWith('/__receipts')) data = structuredClone(history);
    else if (url.endsWith('/__mode')) { mode = body.mode; data = {}; }
    else if (url.endsWith('/__gateway')) {
      const account = body.model.replace('autojev/model/model-', '');
      const target = profile.targets.find(t => t.account === account);
      if (paused || !target || target.disabled || cooled) data = { status: 404 };
      else if (body.prompt.startsWith('hang-timeout')) {
        history.requests.push(receiver(body.prompt, account));
        if (!fault.timeoutNotReclaimed) { history.cancelled.push(body.prompt); stats.timeoutCancelled++; }
        data = { status: 504 };
      } else if (body.prompt.startsWith('hang-cancel')) {
        await new Promise((_resolve, reject) => {
          let delivered = false;
          const timer = fault.cancelNotReceived ? null : setTimeout(() => {
            history.requests.push(receiver(body.prompt, account)); delivered = true; stats.received++;
            // Late records from the previous desktop must not change this run's evidence.
            history.requests.push(receiver('hang-cancel@previous-desktop'));
            history.cancelled.push('hang-cancel@previous-desktop');
          }, 1);
          options.signal.addEventListener('abort', () => {
            if (timer) clearTimeout(timer);
            if (delivered && !fault.cancelNotReclaimed) { history.cancelled.push(body.prompt); stats.cancelled++; }
            reject(new Error('AbortError'));
          }, { once: true });
        });
      } else {
        if (fault.latePreviousDuringConcurrency && !latePreviousRequest && body.prompt.startsWith('parallel-')) {
          history.requests.push(receiver('parallel-0@previous-desktop', 'paid'));
          latePreviousRequest = true;
        }
        history.requests.push(receiver(body.prompt, account));
        if (mode !== 'ok') { data = { status: mode === 'drop' ? 500 : mode }; cooled = true; }
        else data = { status: 200 };
      }
    } else throw new Error(`Unexpected fixture URL: ${url}`);
    return { ok: true, json: async () => data };
  };
  await vm.runInNewContext(driver, {
    window: { __CPA_CHECK__: { base, binary: '/fictional-cpa', reload: true, run_id: 'current-desktop' }, __TAURI_INTERNALS__: { invoke } },
    document: { querySelectorAll: () => [{}, { click() {} }] },
    fetch, AbortController, URL, structuredClone,
    Date: { now: () => clock },
    setTimeout: (callback, delay) => setTimeout(() => { clock += delay; callback(); }, 0),
  });
  return { report, stats };
}

it('rejects prior cancellation receipts when the current request never reaches the receiver', async () => {
  const { report, stats } = await acceptanceReport({ cancelNotReceived: true });
  expect(report.ok).toBe(false);
  expect(report.error).toContain('stream reached receiver');
  expect(stats).toEqual({ received: 0, cancelled: 0, timeoutCancelled: 1 });
});

it('rejects prior cancellation receipts when the current receiver has not reclaimed its stream', async () => {
  const { report, stats } = await acceptanceReport({ cancelNotReclaimed: true });
  expect(report.ok).toBe(false);
  expect(report.error).toContain('cancelled stream reclaimed');
  expect(stats).toEqual({ received: 1, cancelled: 0, timeoutCancelled: 1 });
});

it('rejects prior timeout receipts when the current timed out request has not been reclaimed', async () => {
  const { report, stats } = await acceptanceReport({ timeoutNotReclaimed: true });
  expect(report.ok).toBe(false);
  expect(report.error).toContain('timed out CPA request reclaimed');
  expect(stats).toEqual({ received: 0, cancelled: 0, timeoutCancelled: 0 });
});

it('accepts current reclamation and fixed targets while ignoring late previous-run receipts', async () => {
  const { report, stats } = await acceptanceReport({ latePreviousDuringConcurrency: true });
  expect(report.error).toBeUndefined();
  expect(report.ok).toBe(true);
  expect(stats).toEqual({ received: 1, cancelled: 1, timeoutCancelled: 1 });
  expect(report.receipts.requests.every(r => r.prompt.endsWith('@current-desktop'))).toBe(true);
  expect(report.receipts.cancelled).toEqual(['hang-timeout@current-desktop', 'hang-cancel@current-desktop']);
});
