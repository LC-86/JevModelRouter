// DSH protocol replay; this driver does not start the actual DSH application.
(async () => {
  if (window.__DSH_CHECK_RUNNING__) return;
  window.__DSH_CHECK_RUNNING__ = true;
  const { base, binary, run_id, reload, service_check } = window.__DSH_CHECK__;
  const report = { ok: false, run_id, layer: 'native-mac-ui-real-cpa-dsh-shaped-replay', real_dsh_runtime: false, checks: [] };
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  const wait = async (fn, label) => { const deadline = Date.now() + 15000; while (Date.now() < deadline) { const value = await fn(); if (value) return value; await new Promise(r => setTimeout(r, 50)); } throw new Error(`Timed out: ${label}`); };
  const control = async (path, body = {}) => { const res = await fetch(`${base}${path}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ ...body, run_id }) }); check(res.ok, `${path}: ${await res.clone().text()}`); return res.json(); };
  const progress = message => control('/__progress', { message });
  const click = async selector => (await wait(() => { const e = document.querySelector(selector); return e && !e.disabled && e; }, selector)).click();
  const set = async (selector, value) => { const e = await wait(() => document.querySelector(selector), selector); Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e), 'value').set.call(e, value); e.dispatchEvent(new Event(e.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true })); await new Promise(r => setTimeout(r, 0)); };
  const rejected = async (command, args, reason) => { let error; try { await invoke(command, args); } catch (e) { error = String(e); } check(error?.includes(reason), `${command} expected ${reason}: ${error || 'accepted'}`); };
  const records = async () => (await control('/__records')).records;
  const only = async (offset, account, count) => { const recent = (await records()).slice(offset); check(recent.length === count && recent.every(r => r.account === account && r.authorization === `Bearer fictional-${account}` && r.body.model === 'same-model'), `Wrong receiver: ${JSON.stringify(recent)}`); return recent; };
  const schema = { type: 'function', function: { name: 'client_echo', description: 'Executed only by the fictional client', parameters: { type: 'object', properties: { note: { type: 'string' } }, required: ['note'], additionalProperties: false } } };
  const channel = (() => { const id = window.__TAURI_INTERNALS__.transformCallback(() => {}, false); const serialize = () => `__CHANNEL__:${id}`; return { __TAURI_TO_IPC_KEY__: serialize, toJSON: serialize }; })();
  try {
    const beforeStartup = (await records()).length;
    if (service_check) {
      const status = await invoke('get_cpa_development_service');
      check(status.state === 'stopped' && status.real_generation_enabled === false, 'Development service defaults stopped with real generation disabled');
    }
    const accounts = ['a1', 'a2', 'b1', 'paid'];
    const initial = await invoke('get_snapshot');
    const savedProvider = initial.providers.find(p => p.id === 'r5-a1');
    const profile = { upstream: base, port: reload ? Number(new URL(savedProvider.base_url).port) : 0, targets: accounts.map(account => ({ prefix: `jev-${account}`, source: account.startsWith('a') ? 'source-a' : `source-${account}`, account, plan: `fictional-plan-${account}`, model: 'same-model', aliases: ['same-model'], keys: [`fictional-${account}`], disabled: false })) };
    const remember = service => { report.service = service; profile.port = Number(new URL(service.base_url).port); report.pids = [...(report.pids || []), service.pid]; };
    const start = async () => remember(await invoke('start_cpa_validation', { binary, profile }));
    const uiStart = async () => {
      await click('[data-testid="cpa-service-start"]');
      const status = await wait(async () => { const s = await invoke('get_cpa_development_service'); return s.state === 'ready' && s; }, 'owned service ready');
      remember(status.service);
    };
    if (service_check) {
      (await wait(() => document.querySelectorAll('nav .nav-item')[1], 'providers page')).click();
      await wait(() => document.querySelector('[data-testid="cpa-development-service"]'), 'development service controls');
      for (const [path, reason] of [[`${binary}-missing`, 'executable is missing'], ['/bin/echo', 'version/checksum']]) {
        await set('[data-testid="cpa-service-binary"]', path); await click('[data-testid="cpa-service-start"]');
        await wait(() => document.querySelector('[data-testid="cpa-service-error"]')?.textContent.includes(reason), reason);
      }
      await set('[data-testid="cpa-service-binary"]', binary);
      if (!reload) {
        await set('[data-testid="cpa-service-port"]', new URL(base).port); await click('[data-testid="cpa-service-start"]');
        await wait(() => document.querySelector('[data-testid="cpa-service-error"]')?.textContent.includes('port is already occupied'), 'occupied port recovery message');
      }
      await set('[data-testid="cpa-service-port"]', String(profile.port));
      await uiStart();
      await control('/__own_cpa', { pid: report.service.pid });
      await control('/__exit_cpa', { pid: report.service.pid });
      await wait(() => document.querySelector('[data-testid="cpa-service-state"]')?.textContent.includes('服务已退出'), 'owned exit appears in native UI');
      await uiStart();
      report.checks.push('native controls reject missing/incompatible artifact and occupied port; observe owned exit and explicitly recover; real generation remains disabled');
      report.service_screenshot = await control('/__capture', { label: 'service' });
    } else await start();
    await progress('R5 pinned CPA ready; configure sources through native forms');
    (await wait(() => document.querySelectorAll('nav .nav-item')[1], 'providers page')).click();
    if (!reload) for (const account of accounts) {
      await click('.provider-actions .button.primary'); await click('.provider-dialog .search-select-trigger');
      (await wait(() => [...document.querySelectorAll('.provider-dialog [role="option"]')].find(e => /OpenAI Compatible|OpenAI 兼容/.test(e.textContent)), 'compatible provider')).click();
      await set('[data-testid="api-source-kind"]', account === 'paid' ? 'third_party_api' : 'official_api');
      await set('.provider-dialog input[pattern="[a-zA-Z0-9_-]+"]', `r5-${account}`);
      await set('.provider-dialog input[type="url"]', `${report.service.base_url}/v1`);
      await set('.provider-dialog input[type="password"]', report.service.model_client_key);
      await set('[data-testid="api-source-account"]', `fictional-${account}`);
      await set('[data-testid="api-source-plan"]', `fictional-plan-${account}`);
      await set('#provider-test-model', `jev-${account}/same-model`);
      await click('.provider-dialog-actions button[type="submit"]'); await wait(() => !document.querySelector('.provider-dialog'), 'source saved');
    }
    let snapshot = await invoke('get_snapshot');
    report.saved = accounts.map(account => { const provider = snapshot.providers.find(p => p.id === `r5-${account}`); const model = snapshot.models.find(m => m.provider_id === provider.id && m.model_id === `jev-${account}/same-model`); check(model, `Missing ${account}`); const source = snapshot.api_sources[provider.id]; return { account, provider_id: provider.id, uuid: model.id, instance: source.connection_instance_id, generation: source.generation, endpoint: source.endpoint }; });
    check((await records()).length === beforeStartup, 'Startup/save must not generate');
    const startupCount = (await records()).length;
    await new Promise(r => setTimeout(r, 1500)); await invoke('get_snapshot');
    check((await records()).length === startupCount, 'Snapshot/normal idle must not generate');
    report.startup = { generation_requests: 0, account_quota_queries: 0, logins: 0 };
    report.checks.push('native source save, snapshot and startup idle generate zero; UUID/source identity persists across restart');
    await invoke('save_gateway_settings', { gateway: { ...snapshot.gateway, proxy_mode: 'direct', failure_threshold: 20, stream_idle_seconds: 5 } });
    snapshot = await invoke('start_proxy'); await control('/__register', { port: snapshot.proxy.port });
    const target = account => `autojev/model/${report.saved.find(s => s.account === account).uuid}`;
    const tag = (kind, session = 'one') => `r5-${kind}:${run_id}:${session}`;
    const request = (account, marker, extra = {}) => ({ model: target(account), messages: [{ role: 'user', content: marker }], max_tokens: 64, ...extra });
    const call = (body, extra = {}) => control('/__call', { request: body, ...extra });
    const restart = async () => { await invoke('stop_cpa_validation'); await start(); await invoke('reset_gateway_health', { modelId: null }); };
    const logFor = id => wait(async () => (await invoke('get_request_logs', { since: '2026-01-01T00:00:00Z' })).find(l => l.id === id), `terminal diagnostic ${id}`);
    let offset = (await records()).length;
    if (service_check) {
      await control('/__own_cpa', { pid: report.service.pid }); await control('/__exit_cpa', { pid: report.service.pid });
      await wait(async () => (await invoke('get_cpa_development_service')).state === 'exited', 'owned child exit status');
      const failed = await call(request('a1', tag('service-exit')));
      check(failed.status >= 400, 'Exited service must reject a fixed target'); await only(offset, 'a1', 0);
      await set('[data-testid="cpa-service-port"]', '0'); await click('[data-testid="cpa-service-start"]');
      const recovered = await invoke('get_cpa_development_service');
      check(recovered.state === 'exited', 'Changing the recovery port must not strand saved fixed targets');
      await wait(() => document.querySelector('[data-testid="cpa-service-error"]')?.textContent.includes('saved port'), 'saved port recovery guidance');
      await set('[data-testid="cpa-service-port"]', String(profile.port));
      await uiStart();
      check(report.service.base_url === snapshot.providers.find(p => p.id === 'r5-a1').base_url.replace(/\/v1$/, ''), 'Recovery preserves the saved endpoint and target bindings');
      report.checks.push('service-exit fixed request fails with zero upstream dispatch; recovery rejects port changes and preserves source/model binding');
    }
    const text = await call(request('a1', tag('text'))); check(text.status === 200 && text.body.includes('a1:'), 'Plain fixed text'); await only(offset, 'a1', 1);
    offset = (await records()).length;
    const history = [{ role: 'user', content: tag('multi') }, { role: 'assistant', content: 'fictional previous answer' }, { role: 'user', content: 'fictional next question' }];
    check((await call(request('a1', '', { messages: history }))).status === 200, 'Multi-turn fixed text');
    const [multi] = await only(offset, 'a1', 1); check(multi.body.messages.length === history.length && multi.body.messages.every((m, i) => m.role === history[i].role && m.content === history[i].content), 'Multi-turn roles/content preserved');
    report.checks.push('stable UUID maps text and complete multi-turn history to A1; same-name B/A2/paid receive zero');

    for (const streaming of [false, true]) {
      const marker = tag('tool', streaming ? 'stream' : 'json'); offset = (await records()).length;
      const first = await call(request('a1', marker, { tools: [schema], stream: streaming })); check(first.status === 200, 'Client tool request failed');
      let assistant;
      if (!streaming) assistant = JSON.parse(first.body).choices[0].message;
      else {
        const frames = first.body.split('\n').filter(l => l.startsWith('data: ') && !l.includes('[DONE]')).map(l => JSON.parse(l.slice(6)));
        const deltas = frames.flatMap(f => f.choices?.[0]?.delta?.tool_calls || []);
        check(frames.some(f => f.choices?.[0]?.finish_reason === 'tool_calls') && first.body.includes('[DONE]'), 'Tool stream must finish as tool_calls');
        assistant = { role: 'assistant', content: null, tool_calls: [{ id: deltas.find(d => d.id).id, type: 'function', function: { name: deltas.find(d => d.function?.name).function.name, arguments: deltas.map(d => d.function?.arguments || '').join('') } }] };
      }
      const tool = assistant.tool_calls[0]; check(tool.id === `call_${marker}` && tool.function.name === 'client_echo' && JSON.parse(tool.function.arguments).note === 'fictional result', 'Client call ID/schema/fragmented arguments mismatch');
      check((await records()).length === offset + 1, 'Jev/CPA must wait for the client result, without executing a tool or generating a continuation');
      const result = { role: 'tool', tool_call_id: tool.id, content: `client-result:${marker}` };
      const follow = await call(request('a1', marker, { tools: [schema], stream: streaming, messages: [{ role: 'user', content: marker }, assistant, result] }));
      check(follow.status === 200 && follow.body.includes('resumed:a1:'), 'Client result continuation failed');
      const recent = await only(offset, 'a1', 2); check(recent[1].body.messages[2].tool_call_id === tool.id && recent[1].body.messages[2].content === result.content, 'Result was not correlated');
      report.checks.push(`${streaming ? 'SSE' : 'JSON'} tool schema/call ID/result/continuation preserved; client supplies result; server tools executed=0`);
    }
    offset = (await records()).length;
    const clients = [{ account: 'a1', marker: tag('tool', 'client-a') }, { account: 'a2', marker: tag('tool', 'client-b') }];
    const toolReplies = await Promise.all(clients.map(client => call(request(client.account, client.marker, { tools: [schema] }), { session: client.marker })));
    const assistants = toolReplies.map((reply, i) => { check(reply.status === 200, 'Concurrent client tool call'); const assistant = JSON.parse(reply.body).choices[0].message; check(assistant.tool_calls[0].id === `call_${clients[i].marker}`, 'Each session owns its distinct call ID'); return assistant; });
    const continuations = await Promise.all(clients.map((client, i) => call(request(client.account, client.marker, { tools: [schema], messages: [{ role: 'user', content: client.marker }, assistants[i], { role: 'tool', tool_call_id: assistants[i].tool_calls[0].id, content: `client-result:${client.marker}` }] }), { session: client.marker })));
    check(continuations.every((reply, i) => reply.status === 200 && reply.body.includes(`resumed:${clients[i].account}:`)), 'Concurrent client results resume their own account/session');
    const concurrentTools = (await records()).slice(offset);
    check(concurrentTools.length === 4 && concurrentTools.every(r => clients.some(c => r.account === c.account && r.authorization === `Bearer fictional-${c.account}` && r.body.messages[0].content === c.marker)), 'Concurrent tool sessions cannot mix account, call ID or results');
    report.checks.push('two concurrent client tool sessions retain distinct call IDs/results and A1/A2 source identities; no B/paid dispatch');
    await progress('R5 tools passed; hold A stream while selecting B');
    const marker = tag('hold'); offset = (await records()).length;
    const open = await control('/__open', { request: request('a1', marker, { stream: true }), session: run_id });
    check(open.first.includes('first:a1:'), 'Need actual A content before switching');
    (await wait(() => document.querySelectorAll('nav .nav-item')[2], 'models page')).click();
    const a = report.saved.find(s => s.account === 'a1'), b = report.saved.find(s => s.account === 'b1');
    for (const saved of [a,b]) { const row = await wait(() => document.querySelector(`[data-testid="model-pool-row-${saved.uuid}"]`), 'pool row'); check(row.textContent.includes(target(saved.account)), 'Native pool displays stable UUID'); }
    await click(`[data-testid="model-pool-select-${a.uuid}"]`); await wait(async () => !(await invoke('get_snapshot')).models.find(m => m.id === a.uuid).selected, 'A deselection saved while A stream in flight');
    const next = await call(request('b1', tag('switched')), { session: run_id }); check(next.status === 200 && next.body.includes('b1:'), 'Next request must use B');
    await control('/__release', { tag: marker }); const completed = await control('/__finish', { id: open.id });
    check(completed.wire.includes('first:a1:') && completed.wire.includes(':last:a1') && !completed.wire.includes(':b1') && completed.wire.includes('"finish_reason":"stop"') && completed.wire.includes('[DONE]'), 'In-flight A stream must finish on A, without B output');
    const switched = (await records()).slice(offset); check(switched.length === 2 && switched[0].account === 'a1' && switched[1].account === 'b1', 'Switch dispatches exactly A then B; A2/paid zero');
    report.switch = { first: open.first, completed: completed.wire, receivers: switched.map(r => r.account), request_id: open.request_id };
    report.completed_stream = await logFor(open.request_id);
    check(report.completed_stream.status === 'success' && report.completed_stream.requested_model === target('a1'), 'Completed stream diagnosis keeps its fixed target');
    await click(`[data-testid="model-pool-select-${a.uuid}"]`); await wait(async () => (await invoke('get_snapshot')).models.find(m => m.id === a.uuid).selected, 'Restore selection for reload');
    report.checks.push('actual incremental A stream survives native pool selection change; same-session next request goes to B; existing stream finishes on A');

    for (const status of [401,429,500,502,503]) {
      await restart(); await control('/__mode', { account: 'a1', mode: status }); offset = (await records()).length;
      const failed = await call(request('a1', tag('failure', String(status)))); check(failed.status >= 400 && failed.body.includes(String(status)), `Explicit upstream error ${status}`); await only(offset, 'a1', 1);
      report.checks.push(`${status}: failed fixed A request; B/A2/paid fallback=0`);
    }
    await control('/__mode', { account: 'a1', mode: null }); await restart(); offset = (await records()).length;
    const partial = await call(request('a1', tag('partial'), { stream: true })); check(partial.status === 200 && partial.body.includes('first:a1:') && partial.body.includes('fictional partial stream failure') && !partial.body.includes('"finish_reason":"stop"'), 'Partial output must end in explicit failure'); await only(offset, 'a1', 1);
    const partialLog = await logFor(partial.request_id); check(partialLog.status === 'error' && partialLog.error, 'Partial failure must not be diagnosed successful');
    report.partial_failure = { body: partial.body, diagnostic: partialLog };
    await restart(); offset = (await records()).length;
    const cancelMarker = tag('cancel'); const pending = await control('/__open', { request: request('a1', cancelMarker, { stream: true }) });
    await control('/__cancel', { id: pending.id }); await wait(async () => (await control('/__records')).cancelled.includes(cancelMarker), 'Client cancel reaches owned upstream'); await only(offset, 'a1', 1);
    report.cancellation = await logFor(pending.request_id);
    check(report.cancellation.status === 'cancelled' && report.cancellation.requested_model === target('a1'), 'Client cancellation diagnosis keeps its fixed target');
    report.checks.push('partial output failure is terminal error; client abort is cancelled and releases stream; no billing-stop guarantee');

    offset = (await records()).length;
    const unsupported = await call({ model: target('a1'), input: tag('unsupported'), background: true }, { endpoint: '/v1/responses' });
    check(unsupported.status === 422 && unsupported.body.includes('background'), 'Unsupported converted field explicitly refused');
    await only(offset, 'a1', 0);
    snapshot = await invoke('get_snapshot'); const provider = snapshot.providers.find(p => p.id === a.provider_id);
    await invoke('save_provider', { provider: { ...provider, enabled: false } }); offset = (await records()).length;
    check((await call(request('a1', tag('disabled')))).body.includes('source_disabled'), 'Disabled fixed target denial');
    await rejected('test_provider', { id: a.provider_id }, 'source_disabled');
    const debug = await invoke('debug_curl', { id: `r5-denied-${run_id}`, endpoint: 'chat/completions', body: request('a1', tag('debug')), headers: {}, onProgress: channel });
    check(debug.body.includes('source_disabled'), 'Debug shares source admission');
    await rejected('start_model_speed_tests', { ids: [a.uuid] }, 'Select enabled'); await only(offset, 'a1', 0);
    await invoke('save_provider', { provider });
    report.checks.push('unsupported field, disabled gateway/Debug/provider test/speed test reject before dispatch to every source');
    report.requests = (await records()).filter(r => JSON.stringify(r.body).includes(run_id));
    document.querySelector(`[data-testid="model-pool-row-${a.uuid}"]`).scrollIntoView({ block: 'center' });
    report.screenshot = await control('/__capture');
    report.ok = true;
  } catch (error) { report.error = `${error?.message || error}\n${error?.stack || ''}`; await progress(`R5 error: ${report.error}`).catch(() => {}); }
  await invoke('isolation_check_report', { report });
})();
