// Actual React controls, public Tauri IPC and the listening gateway; fictional sources only.
(async () => {
  if (window.__API_SOURCE_CHECK_RUNNING__) return;
  window.__API_SOURCE_CHECK_RUNNING__ = true;
  const { base, run_id, reload } = window.__API_SOURCE_CHECK__;
  const report = { ok: false, run_id, layer: 'native-desktop-api-sources-loopback', checks: [] };
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const check = (value, message) => { if (!value) throw new Error(message); };
  const wait = async (fn, label) => {
    const deadline = Date.now() + 15000;
    for (;;) { const value = await fn(); if (value) return value; if (Date.now() > deadline) throw new Error(`Timed out: ${label}`); await new Promise(r => setTimeout(r, 50)); }
  };
  const click = async selector => (await wait(() => { const e = document.querySelector(selector); return e && !e.disabled && e; }, selector)).click();
  const set = async (selector, value) => {
    const e = await wait(() => document.querySelector(selector), selector);
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e), 'value').set.call(e, value);
    e.dispatchEvent(new Event(e.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
    await new Promise(r => setTimeout(r, 0));
  };
  const control = async (path, payload = {}) => {
    const response = await fetch(`${base}${path}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(payload) });
    check(response.ok, `Fixture control failed: ${path}`); return response.json();
  };
  const rejected = async (command, args, expected) => {
    let error; try { await invoke(command, args); } catch (e) { error = String(e); }
    check(error && error.includes(expected), `${command} must reject ${expected}: ${error || 'accepted'}`);
  };
  try {
    const sources = [{ id: 'official', kind: 'official_api' }, { id: 'openrouter', kind: 'third_party_api' }, { id: 'zenmux', kind: 'third_party_api' }, { id: 'coding', kind: 'coding_plan' }];
    (await wait(() => document.querySelectorAll('nav .nav-item')[1], 'provider navigation')).click();
    if (!reload) {
      for (const source of sources) {
        await control('/__progress', { message: `R2 create ${source.id} via desktop form` });
        await click('.provider-actions .button.primary');
        await click('.provider-dialog .search-select-trigger');
        (await wait(() => [...document.querySelectorAll('.provider-dialog [role="option"]')].find(e => /OpenAI Compatible|OpenAI 兼容/.test(e.textContent)), 'compatible provider')).click();
        await set('[data-testid="api-source-kind"]', source.kind);
        await set('.provider-dialog input[pattern="[a-zA-Z0-9_-]+"]', `r2-${source.id}`);
        await set('.provider-dialog input[type="url"]', `${base}/${source.id}/${source.id === 'coding' ? 'v4' : 'v1'}`);
        await set('.provider-dialog input[type="password"]', `fictional-r2-${source.id}`);
        await set('#provider-test-model', 'same-model');
        check(document.querySelector('.provider-dialog-actions button[type="button"]').disabled, 'New explicit sources require save before testing');
        await click('.provider-dialog-actions button[type="submit"]');
        await wait(() => !document.querySelector('.provider-dialog'), 'source saved');
      }
      const before = await control('/__records');
      check(before.length === 0, 'Source save/selection must send zero generation requests');
      let snapshot = await invoke('get_snapshot');
      const legacy = { ...snapshot.providers.find(p => p.id === 'r2-official'), id: 'r2-legacy', name: 'legacy', base_url: `${base}/legacy/v1`, test_model: 'same-model' };
      snapshot = await invoke('save_provider', { provider: legacy, apiKey: 'fictional-r2-legacy', addTestModel: true, creating: true });
      check(!snapshot.api_sources['r2-legacy'], 'Legacy API remains unclassified and keeps its existing dispatch path');
    }
    let snapshot = await invoke('get_snapshot');
    if (reload) check(Object.entries(snapshot.api_sources).some(([id, source]) => id.startsWith('r2-retired-') && source.retired && Object.keys(source.model_bindings).length > 0), 'Retired connection/model identities must survive a new process');
    report.saved = [...sources.map(s => s.id), 'legacy'].map(id => {
      const provider = snapshot.providers.find(p => p.id === `r2-${id}`);
      const model = snapshot.models.find(m => m.provider_id === provider?.id && m.model_id === 'same-model');
      check(provider && model, `Missing saved target: ${id}`);
      const source = snapshot.api_sources[provider.id];
      if (id !== 'legacy') {
        check(source && source.kind === sources.find(s => s.id === id).kind && source.generation === 1, 'Source category or generation changed');
        check(source.plan_state === 'unknown' && source.account_state === 'unknown', 'Key/model entry cannot establish account or plan eligibility');
        check(model.input_price_known === false && model.output_price_known === false, 'Unknown fees must stay unknown');
        check(source.credential_reference === `api-generation:${source.connection_instance_id}`, 'Generation credential must be bound to this immutable connection instance');
      }
      return { id, provider_id: provider.id, model_uuid: model.id, source_instance: source?.connection_instance_id || null, endpoint: provider.base_url };
    });
    for (const id of [...sources.map(s => s.id), 'legacy']) check(!JSON.stringify(snapshot).includes(`fictional-r2-${id}`), 'Snapshot must never contain a generation credential');
    report.checks.push(reload ? 'new desktop process preserves source categories, unknown account/plan states and stable UUIDs' : 'desktop creates and saves four classified sources without generation; legacy API identity remains compatible');
    await invoke('save_gateway_settings', { gateway: { ...snapshot.gateway, proxy_mode: 'direct', failure_threshold: 20 } });
    snapshot = await invoke('start_proxy');
    const gateway = (target, prompt = 'R2 fictional request', endpoint = '/v1/chat/completions', stream = false) => control('/__gateway', {
      port: snapshot.proxy.port, endpoint, request: { model: target, messages: [{ role: 'user', content: `${prompt}@${run_id}` }], max_tokens: 8, stream },
    });
    for (const saved of report.saved) {
      const response = await gateway(`autojev/model/${saved.model_uuid}`);
      check(response.status === 200 && response.body.includes(`OK ${saved.id}`), `Fixed source failed: ${saved.id}`);
      check((await invoke('test_provider', { id: saved.provider_id })).includes('succeeded'), 'Saved provider test must use this same target');
    }
    report.checks.push('all same-name source models, saved provider tests and legacy API hit their own fictional credential and endpoint without a decision key');
    const coding = report.saved.find(s => s.id === 'coding');
    const target = `autojev/model/${coding.model_uuid}`;
    const recordsCount = async () => (await control('/__records')).length;
    let before = await recordsCount();
    const provider = snapshot.providers.find(p => p.id === coding.provider_id);
    await rejected('save_provider', { provider: { ...provider, base_url: `${base}/official/v1` }, originalId: provider.id }, 'Create a new source connection');
    await rejected('save_provider', { provider, originalId: provider.id, apiKey: 'fictional-r2-replacement' }, 'Create a new source connection');
    await rejected('save_model', { model: { ...snapshot.models.find(m => m.id === coding.model_uuid), provider_id: 'r2-official' } }, 'Create a new model');
    check(await recordsCount() === before, 'Identity edits must not generate');
    for (const status of [401, 429, 503, 404]) {
      await control('/__mode', { source: 'coding', status });
      await invoke('reset_gateway_health', { modelId: coding.model_uuid });
      before = await recordsCount();
      const failed = await gateway(target);
      check(failed.status === status && !failed.body.includes('fictional-r2-coding'), `Failure or credential redaction mismatch: ${status}`);
      const delta = (await control('/__records')).slice(before);
      check(delta.length === 1 && delta[0].source === 'coding', `Failure ${status} must not call any other paid source`);
    }
    await control('/__mode', { source: 'coding', status: 200 });
    await invoke('reset_gateway_health', { modelId: coding.model_uuid });
    // Deselect is list management; the original stable target still passes normal admission.
    const model = snapshot.models.find(m => m.id === coding.model_uuid);
    await invoke('save_model', { model: { ...model, selected: false } });
    check((await gateway(target)).status === 200, 'Deselect must preserve a valid original fixed target');
    await invoke('save_model', { model });
    await invoke('save_provider', { provider: { ...provider, enabled: false } });
    before = await recordsCount();
    check((await gateway(target)).body.includes('source_disabled'), 'Disabled source must return its own target error');
    await rejected('test_provider', { id: provider.id }, 'source_disabled');
    const channel = (() => { const id = window.__TAURI_INTERNALS__.transformCallback(() => {}, false); const serialize = () => `__CHANNEL__:${id}`; return { __TAURI_TO_IPC_KEY__: serialize, toJSON: serialize }; })();
    const debug = await invoke('debug_curl', { id: `r2-disabled-${run_id}`, endpoint: 'chat/completions', body: { model: target, messages: [{ role: 'user', content: 'fixture' }] }, headers: {}, onProgress: channel });
    check(debug.body.includes('source_disabled'), 'Debug must use the same source admission');
    await rejected('start_model_speed_tests', { ids: [coding.model_uuid] }, 'Select enabled');
    check(await recordsCount() === before, 'Disabled gateway/test/Debug/manual probe must dispatch zero requests');
    await invoke('save_provider', { provider });
    await invoke('save_model', { model: { ...model, api_type: 'responses' } });
    before = await recordsCount();
    check((await gateway(target)).body.includes('source_protocol_unsupported'), 'Protocol mismatch must not alter the endpoint');
    check(await recordsCount() === before, 'Unsupported endpoint protocol must dispatch zero requests');
    await invoke('save_model', { model });
    before = await recordsCount();
    check((await gateway('autojev/model/missing-r2-target')).status === 422, 'Missing fixed target must not become a fallback');
    check((await gateway('same-model')).body.includes('Ambiguous model'), 'A bare same-name model must not choose a source');
    check(await recordsCount() === before, 'Invalid/ambiguous targets dispatch zero requests');
    report.checks.push('identity edits rejected; 401/429/503/404 stay on the fixed source; deselection preserves UUID; disabled/invalid/unsupported/ambiguous targets dispatch zero through gateway, test, Debug and manual probe');
    const missingId = `r2-missing-${run_id}`;
    let next = await invoke('save_provider', { provider: { ...provider, id: missingId, name: 'missing fictional credential' }, creating: true, source: { kind: 'official_api' }, addTestModel: true });
    const missing = next.models.find(m => m.provider_id === missingId);
    before = await recordsCount();
    check((await gateway(`autojev/model/${missing.id}`)).body.includes('source_credential_missing'), 'Missing generation key must not use the decision key');
    await rejected('test_provider', { id: missingId }, 'source_credential_missing');
    check(await recordsCount() === before, 'Missing generation credential must dispatch zero');
    await invoke('delete_provider', { id: missingId });
    // Explicit manual probes and Debug go through the same normal gateway target.
    await invoke('start_model_speed_tests', { ids: [coding.model_uuid] });
    await wait(async () => !(await invoke('get_model_performance')).job.running, 'manual probe complete');
    const debugOk = await invoke('debug_curl', { id: `r2-ok-${run_id}`, endpoint: 'chat/completions', body: { model: target, messages: [{ role: 'user', content: 'fictional debug' }], stream: false }, headers: {}, onProgress: channel });
    check(debugOk.body.includes('OK coding'), 'Debug must reach the exact saved source');
    for (const endpoint of ['/v1/chat/completions', '/v1/responses', '/v1/messages']) {
      for (const stream of [false, true]) {
        const request = endpoint === '/v1/responses' ? { model: target, input: 'fictional protocol probe', max_output_tokens: 8, stream }
          : { model: target, messages: [{ role: 'user', content: 'fictional protocol probe' }], max_tokens: 8, stream };
        const result = await control('/__gateway', { port: snapshot.proxy.port, endpoint, request });
        check(result.status === 200 && result.body.includes('OK coding'), `Compatible protocol failed: ${endpoint}/${stream}`);
        if (stream) check(result.body.includes(endpoint.endsWith('responses') ? 'response.completed' : endpoint.endsWith('messages') ? 'message_stop' : '[DONE]'), 'Require explicit stream terminal');
      }
    }
    const retiredSnapshot = await invoke('save_provider', { provider: { ...provider, id: `r2-retired-${run_id}`, name: 'retired fictional source' }, apiKey: 'fictional-r2-coding', source: { kind: 'coding_plan' }, creating: true, addTestModel: true });
    const retiredId = `r2-retired-${run_id}`;
    const retiredModel = retiredSnapshot.models.find(m => m.provider_id === retiredId);
    before = await recordsCount();
    await invoke('delete_model', { id: retiredModel.id });
    await rejected('save_model', { model: { ...retiredModel, provider_id: 'r2-openrouter' } }, 'Create a new model');
    await invoke('save_model', { model: retiredModel });
    const inflight = gateway(`autojev/model/${retiredModel.id}`, 'inflight-r2');
    await wait(async () => await recordsCount() > before, 'fixed inflight target reached');
    await invoke('delete_provider', { id: retiredId });
    await rejected('save_provider', { provider: { ...provider, id: retiredId, base_url: `${base}/official/v1` }, apiKey: 'fictional-r2-official', source: { kind: 'official_api' }, creating: true }, 'Create a new source connection');
    check((await inflight).body.includes('OK coding'), 'An inflight request keeps its original source and credential after deletion');
    before = await recordsCount();
    check((await gateway(`autojev/model/${retiredModel.id}`)).status !== 200, 'A deleted fixed target must stay invalid');
    check((await gateway(`${retiredId}/same-model`)).status !== 200, 'A deleted provider alias must stay invalid');
    check(await recordsCount() === before, 'Deleted UUID/connection references must never dispatch to a new source');
    report.checks.push('deleted UUIDs/provider aliases cannot be rebound; an inflight request keeps its original source/credential');
    const catalog = await control('/__gateway', { port: snapshot.proxy.port, endpoint: '/v1/models' });
    const logs = await invoke('get_request_logs', { since: '2000-01-01T00:00:00Z' });
    for (const source of sources) check(!JSON.stringify([logs, catalog]).includes(`fictional-r2-${source.id}`), 'Directory and logs must not expose a generation key');
    report.checks.push('missing credential denied; manual probe and Debug hit saved source; three compatible downstream text/stream protocols preserve fixed source and terminal; directory/logs contain no generation secrets');
    check(await recordsCount() > 0, 'This run must have real receiver evidence');
    report.ok = true;
  } catch (error) { report.error = `${error?.message || error}\n${error?.stack || ''}`; }
  await invoke('isolation_check_report', { report });
})();
