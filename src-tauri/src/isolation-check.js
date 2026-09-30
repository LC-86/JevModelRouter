// Runs inside the actual native webview, with the production React UI and IPC commands.
(async () => {
  if (window.__ISOLATION_CHECK_RUNNING__) return;
  window.__ISOLATION_CHECK_RUNNING__ = true;
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  const wait = async fn => {
    const until = Date.now() + 15000;
    while (Date.now() < until) { const value = await fn(); if (value) return value; await new Promise(r => setTimeout(r, 50)); }
    throw new Error('Desktop condition timed out');
  };
  const click = async selector => (await wait(() => { const e = document.querySelector(selector); return e && !e.disabled && e; })).click();
  const nav = async index => { (await wait(() => document.querySelectorAll('nav .nav-item')[index])).click(); };
  const setValue = (element, value) => {
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value').set.call(element, value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const report = { ok: false, layer: 'native-desktop-loopback', checks: [] };
  const passed = name => report.checks.push(name);
  const rejected = async (command, args, text) => {
    try { await invoke(command, args); throw new Error('Unexpected success'); }
    catch (error) { check(String(error).includes(text), `${command}: ${error}`); }
  };
  try {
    await wait(() => document.querySelector('.app-shell'));
    let snapshot = await invoke('get_snapshot');
    check(!snapshot.proxy.running && snapshot.proxy.port === 0, 'Isolation must not autostart');
    check(!(await invoke('get_model_performance')).settings.enabled, 'Automatic speed tests must be off');
    passed('no gateway autostart or automatic speed tests');
    if (window.__ISOLATION_CHECK__.reload) {
      check(snapshot.providers.find(p => p.id === 'openrouter').test_model === 'fixture-model', 'Saved provider must survive restart');
      check(snapshot.agent_catalogs.codex?.length > 0, 'Saved Agent catalog must survive restart');
      check(snapshot.agents.every(a => !a.connected), 'Saved Agent must not reconnect automatically');
      check((await invoke('get_model_performance')).models[snapshot.models[0].id].samples >= 3, 'Speed measurements must survive restart');
      await invoke('test_provider', { id: 'openrouter' });
      passed('DB restart, credential reuse, measurements and no Agent reconnect');
    } else {
      const provider = { ...snapshot.providers[0], kind: 'openai_compatible', name: 'Loopback fixture', base_url: window.__ISOLATION_CHECK__.base, api_type: 'chat_completions', test_model: 'fixture-model' };
      await rejected('test_provider_draft', { provider: { ...provider, base_url: 'https://example.invalid' }, apiKey: 'fixture-key' }, 'Connection failed');
      await rejected('test_provider_draft', { provider: { ...provider, base_url: 'http://127.0.0.1:11434' }, apiKey: 'fixture-key' }, 'Connection failed');
      await rejected('test_provider_draft', { provider: { ...provider, base_url: `${provider.base_url}/redirect` }, apiKey: 'fixture-key' }, '302');
      await rejected('save_provider', { provider: { ...provider, kind: 'codex_subscription' } }, 'cannot carry an API base URL');
      await rejected('save_provider', { provider: { ...provider, kind: 'some_future_subscription' } }, 'unknown variant');
      await rejected('save_provider', { provider: { ...provider, kind: 'codex_subscription', base_url: '', api_type: '', test_model: '' }, apiKey: 'fixture-key' }, 'not as an API key');
      await rejected('launch_agent', { id: 'codex' }, 'disabled in isolated');
      await rejected('save_custom_agent', { agent: { id: 'custom-outside', name: 'Fixture', command: 'grok', config_path: '/outside-isolation/config.toml', args: [] } }, 'inside the isolation');
      passed('remote and daily loopback targets, redirects, invalid subscription entries and external Agent actions rejected');
      await invoke('save_provider', { provider, apiKey: 'fixture-key' });
      const model = { ...snapshot.models[0], model_id: 'fixture-model', api_type: 'chat_completions', name: 'Fixture model' };
      await invoke('save_model', { model });
      for (const other of snapshot.models.slice(1)) await invoke('delete_model', { id: other.id });
      await invoke('save_policy', { policy: { ...snapshot.policy, use_jev_when_ambiguous: false } });
      await click('.sidebar [aria-busy]');
      snapshot = await wait(async () => { const s = await invoke('get_snapshot'); return s.proxy.running && s; });
      check(snapshot.proxy.port !== 9526 && snapshot.proxy.port !== 9527, 'Gateway must use an independent port');
      await nav(1);
      await wait(() => document.querySelector('tbody')?.textContent.includes('Loopback fixture'));
      await click('tbody tr .row-actions button:nth-child(2)');
      setValue(await wait(() => document.querySelector('.provider-dialog input[type="url"]')), window.__ISOLATION_CHECK__.base);
      setValue(document.querySelector('#provider-test-model'), 'fixture-model');
      await click('.provider-dialog-actions button[type="button"]');
      await wait(() => [...document.querySelectorAll('.provider-test-result')].some(e => /succeeded|成功/.test(e.textContent)));
      await click('.provider-dialog-actions button[type="submit"]');
      await wait(() => !document.querySelector('.provider-dialog'));
      passed('provider dialog test through native IPC');
      await invoke('test_provider', { id: provider.id });
      await nav(2);
      await click('tbody tr .row-actions button:first-child');
      await wait(() => [...document.querySelectorAll('[role="status"]')].some(e => /succeeded|成功/.test(e.textContent)));
      passed('saved provider and model row tests');
      await click('tbody input[type="checkbox"]');
      await click('.model-speed-actions button.ghost');
      const view = await wait(async () => { const v = await invoke('get_model_performance'); return !v.job.running && v.job.completed === 3 && v; });
      check(!view.job.error && view.models[model.id].samples >= 3, 'Manual speed tests must succeed');
      passed('manual speed test from model UI');
      // 订阅服务商：从服务商面板添加保存，行内显示真实连接状态；未验证生成一律拒绝且真实上游零派发。
      await nav(1);
      await click('.provider-actions .button.primary');
      await click('.provider-dialog .search-select-trigger');
      (await wait(() => [...document.querySelectorAll('.provider-dialog [role="option"]')].find(option => /Codex subscription|Codex 订阅/.test(option.textContent)))).click();
      setValue(await wait(() => document.querySelector('.provider-dialog input[pattern="[a-zA-Z0-9_-]+"]')), 'codex-subscription');
      await click('.provider-dialog-actions button[type="submit"]');
      await wait(() => !document.querySelector('.provider-dialog'));
      const subscriptionRow = await wait(() => [...document.querySelectorAll('tbody tr')].find(row => row.textContent.includes('codex-subscription')));
      check(/Not connected|未连接/.test(subscriptionRow.textContent), `Provider row must show the real connection state: ${subscriptionRow.textContent}`);
      const subscription = (await invoke('get_snapshot')).subscriptions.find(s => s.provider_id === 'codex-subscription');
      check(subscription && subscription.state === 'not_connected' && subscription.generation === 1, `Subscription connection must start unconnected: ${JSON.stringify(subscription)}`);
      check(subscription.adapter_available === false, 'Production must not install a subscription stub');
      check(subscription.denial && subscription.denial.code === 'not_connected', 'Unconnected subscription must stay denied');
      passed('subscription provider added from the provider panel with its real state');
      await invoke('save_model', { model: { ...model, id: 'codex-subscription-model', provider_id: 'codex-subscription', model_id: 'codex-fixture-model', name: 'Codex fixture' } });
      await rejected('test_provider_draft', { provider: { ...provider, id: 'codex-subscription', name: 'Codex subscription', kind: 'codex_subscription', base_url: '', api_type: '', test_model: 'codex-fixture-model' } }, 'not connected');
      await rejected('refresh_subscription', { providerId: 'codex-subscription' }, 'No subscription helper');
      await invoke('start_model_speed_tests', { ids: ['codex-subscription-model'] });
      const denied = await wait(async () => { const v = await invoke('get_model_performance'); return !v.job.running && v.job.completed === 3 && v; });
      check(/not connected/i.test(denied.job.error || ''), `Subscription speed test must report the reason: ${denied.job.error}`);
      passed('subscription generation denied without upstream work');
      await nav(5);
      for (const option of ['OpenAI Chat Completions', 'OpenAI Responses', 'Anthropic Messages']) {
        await click('.debug-settings .select-control button');
        (await wait(() => [...document.querySelectorAll('[role="option"]')].find(e => e.textContent === option))).click();
        const input = await wait(() => document.querySelector('.debug-composer textarea'));
        setValue(input, 'fictional desktop prompt');
        await click('.debug-composer button[type="submit"], .debug-composer button.primary');
        await wait(() => document.querySelector('.debug-messages')?.textContent.includes('OK') && !input.disabled);
      }
      passed('Debug UI streaming in all three downstream protocols');
      await invoke('connect_agent', { id: 'codex', routeId: `model/${model.id}` });
      await invoke('restore_agent', { id: 'codex' });
      // Leave a saved auto-connect preference; safe shutdown restores only the temporary files.
      await invoke('connect_agent', { id: 'codex', routeId: `model/${model.id}` });
      await invoke('save_performance_settings', { settings: { version: 1, enabled: true, interval_minutes: 5 } });
      passed('temporary Agent connect and restore');
    }
    report.ok = true;
  } catch (error) { report.error = String(error); report.screen = document.body.innerText.slice(-5000); }
  await invoke('isolation_check_report', { report });
})().catch(console.error);
