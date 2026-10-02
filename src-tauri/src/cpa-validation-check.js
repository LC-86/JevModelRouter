// Acceptance at public desktop IPC and the listening gateway, with a real CPA process.
(async () => {
  if (window.__CPA_CHECK_RUNNING__) return;
  window.__CPA_CHECK_RUNNING__ = true;
  const report = { ok: false, run_id: window.__CPA_CHECK__.run_id, layer: 'native-desktop-real-cpa-loopback', checks: [] };
  const progress = name => { void fetch(`${window.__CPA_CHECK__.base}/__progress`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name }) }).then(r => r.json()).catch(() => {}); };
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  const wait = async (fn, label) => {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) { const value = await fn(); if (value) return value; await new Promise(r => setTimeout(r, 50)); }
    throw new Error(`Timed out: ${label}`);
  };
  const click = async selector => (await wait(() => { const e = document.querySelector(selector); return e && !e.disabled && e; }, selector)).click();
  const set = async (selector, value) => {
    const element = await wait(() => document.querySelector(selector), selector);
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value').set.call(element, value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise(r => setTimeout(r, 0));
  };
  const rejected = async (command, args, expected) => {
    let error;
    try { await invoke(command, args); } catch (e) { error = String(e); }
    check(error && error.includes(expected), `${command} must reject ${expected}: ${error || 'accepted'}`);
  };
  try {
    const { base, binary, reload } = window.__CPA_CHECK__;
    const accounts = ['a1', 'a2', 'b1', 'paid'];
    const targets = accounts.map(account => ({ prefix: `jev-${account}`, source: account.startsWith('a') ? 'source-a' : account === 'b1' ? 'source-b' : 'source-paid', account, plan: `fixture-plan-${account}`, model: 'same-model', aliases: ['same-model'], keys: [`fictional-${account}`], disabled: false }));
    let snapshot = await invoke('get_snapshot');
    const profile = { upstream: base, port: reload ? Number(new URL(snapshot.providers.find(p => p.id === 'cpa-a1').base_url).port) : 0, targets };
    await rejected('start_cpa_validation', { binary: `${binary}-missing`, profile }, 'executable is missing');
    await rejected('start_cpa_validation', { binary: '/bin/echo', profile }, 'version/checksum');
    await rejected('start_cpa_validation', { binary, profile: { ...profile, port: Number(new URL(base).port) } }, 'port is already occupied');
    report.checks.push('missing artifact, incompatible version/checksum and occupied port are diagnosed before dispatch');
    const start = async () => {
      report.service = await invoke('start_cpa_validation', { binary, profile });
      profile.port = Number(new URL(report.service.base_url).port);
      report.pids = [...(report.pids || []), report.service.pid];
      check(!JSON.stringify(report.service).includes('management_key'), 'Management credential must remain in the backend');
    };
    await start();
    await rejected('reload_cpa_validation', { profile: { ...profile, upstream: report.service.base_url } }, 'explicitly configured loopback fixture');
    await progress('CPA ready');
    report.checks.push('desktop owns a healthy pinned CPA with a fictional single-credential namespace');

    // The existing provider dialog performs the configuration save, without a new provider framework.
    (await wait(() => document.querySelectorAll('nav .nav-item')[1], 'provider navigation')).click();
    if (!reload) {
      for (const target of targets) {
        const id = `cpa-${target.account}`;
        await progress(`configure ${id}`);
        await click('.provider-actions .button.primary');
        await click('.provider-dialog .search-select-trigger');
        (await wait(() => [...document.querySelectorAll('.provider-dialog [role="option"]')].find(e => /OpenAI Compatible|OpenAI 兼容/.test(e.textContent)), 'custom OpenAI provider')).click();
        await set('.provider-dialog input[pattern="[a-zA-Z0-9_-]+"]', id);
        await set(`.provider-dialog input[placeholder="${id}"]`, `${target.source} / ${target.account} / ${target.plan}`);
        await set('.provider-dialog input[type="url"]', report.service.base_url);
        await set('.provider-dialog input[type="password"]', report.service.model_client_key);
        await set('#provider-test-model', `${target.prefix}/${target.model}`);
        await click('.provider-dialog-actions button[type="button"]');
        await wait(() => [...document.querySelectorAll('.provider-test-result')].some(e => /succeeded|成功/.test(e.textContent)), 'provider request succeeds through CPA');
        await click('.provider-dialog-actions button[type="submit"]');
        await progress(`submitted ${id}: dialog=${!!document.querySelector('.provider-dialog')}`);
        await wait(() => !document.querySelector('.provider-dialog'), 'configuration saved');
        await progress(`saved ${id}`);
      }
    }
    snapshot = await invoke('get_snapshot');
    await progress('snapshot reread');
    report.saved = targets.map(t => {
      const provider = snapshot.providers.find(p => p.id === `cpa-${t.account}`);
      const model = snapshot.models.find(m => m.provider_id === provider?.id && m.model_id === `${t.prefix}/${t.model}`);
      check(provider && model, `Saved source/model missing: ${t.account}`);
      check(provider.name === `${t.source} / ${t.account} / ${t.plan}`, 'Saved identity label changed');
      check(provider.base_url === report.service.base_url, 'Saved endpoint changed');
      return { account: t.account, provider_id: provider.id, model_id: model.id, upstream_model: model.model_id, name: provider.name };
    });
    report.checks.push(reload ? 'a new desktop process rereads the same provider/model UUIDs and source/account/plan labels' : 'existing provider UI tests, saves and rereads each qualified model and identity label');
    await invoke('save_gateway_settings', { gateway: { ...snapshot.gateway, proxy_mode: 'direct', response_timeout_seconds: 1, stream_idle_seconds: 1, failure_threshold: 20 } });
    snapshot = await invoke('start_proxy');
    const control = async (path, payload, signal) => {
      const response = await fetch(`${base}${path}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(payload || {}), signal });
      check(response.ok, `Fixture control error ${response.status}`); return response.json();
    };
    const receipts = () => control('/__receipts');
    const call = (account, prompt = 'fictional', stream = false, signal) => control('/__gateway', { port: snapshot.proxy.port, model: `autojev/model/${report.saved.find(m => m.account === account).model_id}`, prompt, stream }, signal);
    const restart = async () => {
      await invoke('stop_cpa_validation'); await start();
      // Each fault case starts fresh; its second request still observes the same cooldown.
      await invoke('reset_gateway_health', { modelId: null });
    };
    const only = async (offset, account, count) => {
      const recent = (await receipts()).requests.slice(offset);
      check(recent.every(r => r.account === account && r.key === `Bearer fictional-${account}` && r.model === 'same-model'), `Wrong receiver: ${JSON.stringify(recent)}`);
      if (count !== undefined) check(recent.length === count, `Expected ${count} receiver calls, got ${recent.length}`);
    };
    for (const account of accounts.slice(0, 3)) {
      const offset = (await receipts()).requests.length;
      check((await call(account)).status === 200, `Fixed ${account} request failed`); await only(offset, account, 1);
    }
    report.checks.push('ordinary Jev model HTTP fixes A1/A2/B1 with the same bare model; receiver proves account and credential');
    const parallelOffset = (await receipts()).requests.length;
    const responses = await Promise.all(Array.from({ length: 18 }, (_, i) => call(accounts[i % 3], `parallel-${i}`)));
    check(responses.every(r => r.status === 200), 'Concurrent requests failed');
    const parallel = (await receipts()).requests.slice(parallelOffset);
    check(parallel.length === 18 && parallel.every(r => r.account === accounts[Number(r.prompt.split('-')[1]) % 3]), 'Concurrency changed identity');
    report.checks.push('18 concurrent gateway requests retain their source/account/model binding');

    // Configuration rejection must leave both the running namespace and receivers unchanged.
    for (const [change, reason] of [
      [p => { p.targets[1].prefix = p.targets[0].prefix; }, 'Conflicting CPA prefix'],
      [p => { p.targets[0].keys.push('fictional-another'); }, 'exactly one credential'],
      [p => { p.targets[0].aliases.push('shared-model'); }, 'Shared alias'],
      [p => { p.targets[0].plan = 'different-plan'; }, 'Cannot rebind'],
      [p => { p.targets[0].account = 'other-account'; p.targets[0].keys = ['fictional-other-account']; }, 'Cannot rebind'],
    ]) {
      const invalid = structuredClone(profile); change(invalid);
      const offset = (await receipts()).requests.length;
      await rejected('reload_cpa_validation', { profile: invalid }, reason);
      check((await receipts()).requests.length === offset, 'Rejected config dispatched a request');
    }
    report.checks.push('prefix collision, multiple credentials, shared alias and silent account/plan rebinding reject with zero dispatch');
    await invoke('stop_cpa_validation');
    const rebound = structuredClone(profile); rebound.targets[0].plan = 'different-plan';
    await rejected('start_cpa_validation', { binary, profile: rebound }, 'Cannot rebind');
    await start();
    report.checks.push('owned service restart preserves namespace history and rejects silently changing a saved target');

    const disabled = structuredClone(profile); disabled.targets[0].disabled = true;
    await invoke('reload_cpa_validation', { profile: disabled });
    let offset = (await receipts()).requests.length;
    check((await call('a1')).status >= 400, 'Disabled namespace was callable'); await only(offset, 'a1', 0);
    const removed = structuredClone(profile); removed.targets = removed.targets.slice(1);
    await invoke('reload_cpa_validation', { profile: removed });
    offset = (await receipts()).requests.length;
    check((await call('a1')).status >= 400, 'Missing account was callable'); await only(offset, 'a1', 0);
    await invoke('reload_cpa_validation', { profile });
    const reloadOffset = (await receipts()).requests.length;
    const reloadWork = Promise.all(Array.from({ length: 12 }, (_, i) => call(accounts[i % 3], `reload-${i}`)));
    await invoke('reload_cpa_validation', { profile: { ...profile, targets: profile.targets.map(t => ({ ...t, disabled: t.account === 'paid' })) } });
    check((await reloadWork).every(r => r.status === 200), 'Reload interrupted fixed calls');
    const duringReload = (await receipts()).requests.slice(reloadOffset);
    check(duringReload.length === 12 && duringReload.every(r => r.account === accounts[Number(r.prompt.split('-')[1]) % 3]), 'Reload mixed bindings');
    report.checks.push('disabled/missing account dispatches zero; concurrent hot reload keeps A1/A2/B1 identity');

    for (const status of [401, 429, 500, 502, 503, 'drop']) {
      await restart();
      await control('/__mode', { account: 'a1', mode: status });
      offset = (await receipts()).requests.length;
      const failed = await call('a1', 'fictional-failure', status === 'drop');
      check(failed.status >= 400, `Failure ${status} succeeded`); await only(offset, 'a1', 1);
      if (status !== 'drop') {
        const cooldownOffset = (await receipts()).requests.length;
        check((await call('a1')).status >= 400, 'Cooling account succeeded'); await only(cooldownOffset, 'a1', 0);
      }
      await control('/__mode', { account: 'a1', mode: 'ok' });
    }
    report.checks.push('401/429/5xx, cooldown and stream-before-first-byte failure never dispatch another account or paid source');

    await restart();
    offset = (await receipts()).requests.length;
    const timeout = await call('a1', 'hang-timeout');
    check(timeout.status === 504, `Timeout must return 504: ${JSON.stringify(timeout)}`); await only(offset, 'a1', 1);
    await wait(async () => (await receipts()).cancelled.includes('hang-timeout'), 'timed out CPA request reclaimed');
    await restart();
    const abort = new AbortController();
    const cancelled = call('a1', 'hang-cancel', true, abort.signal).catch(() => null);
    await wait(async () => (await receipts()).requests.some(r => r.prompt === 'hang-cancel'), 'stream reached receiver');
    abort.abort(); await cancelled;
    await wait(async () => (await receipts()).cancelled.includes('hang-cancel'), 'cancelled stream reclaimed');
    report.checks.push('gateway header timeout and client stream cancellation reclaim the owned upstream request');

    await invoke('pause_proxy');
    offset = (await receipts()).requests.length;
    check((await call('a2')).status === 404, 'Paused gateway accepted generation'); await only(offset, 'a2', 0);
    snapshot = await invoke('start_proxy');
    check((await call('a2')).status === 200, 'Resume lost the selected account');
    report.checks.push('pause dispatches zero and resume retains the fixed target');
    report.receipts = await receipts();
    report.ok = true;
  } catch (error) { report.error = String(error?.stack || error); await progress(`error: ${report.error}`); }
  await invoke('isolation_check_report', { report });
})();
