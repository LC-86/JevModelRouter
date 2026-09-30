// Runs inside the actual native webview, with the production React UI and IPC commands.
(async () => {
  if (window.__ISOLATION_CHECK_RUNNING__) return;
  window.__ISOLATION_CHECK_RUNNING__ = true;
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  const wait = async (fn, label) => {
    const until = Date.now() + 15000;
    while (Date.now() < until) { const value = await fn(); if (value) return value; await new Promise(r => setTimeout(r, 50)); }
    throw new Error(`Desktop condition timed out${label ? `: ${label}` : ''}`);
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
  // 接受多个可接受的错误关键词，但“意外成功”永远判失败。
  const rejectedAny = async (command, args, texts) => {
    try { await invoke(command, args); throw new Error('Unexpected success'); }
    catch (error) {
      if (String(error) === 'Unexpected success') throw error;
      check(texts.some(text => String(error).includes(text)), `${command}: ${error}`);
    }
  };

  const providerId = 'codex-subscription';
  const grokProviderId = 'grok-subscription';
  const statusTextFor = id => document.querySelector(`[data-testid="sub-status-${id}"]`)?.textContent || '';
  const statusText = () => statusTextFor(providerId);
  const rowText = () => document.querySelector('tbody')?.textContent || '';
  // 界面按自己的轮询渲染：读取状态文本前必须等它反映后端快照，不能只读一次。
  const waitStatusFor = (id, predicate) => wait(() => { const text = statusTextFor(id); return predicate(text) ? text : null; }, `row status ${id}`);
  const waitStatus = predicate => waitStatusFor(providerId, predicate);
  const subscriptionView = async () => (await invoke('get_snapshot')).subscriptions.find(item => item.provider_id === providerId);
  const grokView = async () => (await invoke('get_snapshot')).subscriptions.find(item => item.provider_id === grokProviderId);
  const clearedQuota = quota => !quota || ['unknown', 'unsupported'].includes(quota.state) === true;
  // Issue #13 登录/退出/换号闭环：每次桌面运行只排练一个场景，替身场景队列与之对应。
  const loginLifecycle = async mode => {
    const details = { mode, observations: [] };
    report.details = details;
    const record = (label, value) => details.observations.push({ label, value });
    await nav(1);
    await wait(() => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(providerId)), 'subscription row');
    const started = await wait(async () => (await subscriptionView()) || null, 'subscription view');
    record('start', { state: started.state, generation: started.generation, identity: started.identity ?? null });
    const startLogin = async () => {
      await click(`[data-testid="sub-login-${providerId}"]`);
      return wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'pending' ? item : null; }, 'pending login');
    };
    if (mode === 'success') {
      check(started.state !== 'connected', `Success rehearsal needs a disconnected subscription: ${started.state}`);
      const pending = await startLogin();
      const pendingText = await waitStatus(text => text.includes('login=pending'));
      const advertised = [pending.login.authorization_url, pending.login.user_code].filter(Boolean);
      check(advertised.length > 0, `Pending login must advertise a link or code: ${JSON.stringify(pending.login)}`);
      if (pending.login.authorization_url) {
        const link = await wait(() => document.querySelector('.provider-subscription-auth-link')?.getAttribute('href') ?? null, 'authorization link');
        check(link === pending.login.authorization_url, `Authorization link must be on screen: ${link} vs ${pending.login.authorization_url}`);
      }
      if (pending.login.user_code) await wait(() => rowText().includes(pending.login.user_code) ? true : null, 'user code on screen');
      record('pending', { generation: pending.generation, login: pending.login, status: pendingText });
      passed('sign-in start reaches pending and shows the authorization link or code');
      const completed = await wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'completed' ? item : null; }, 'completed login');
      check(Boolean(completed.identity), `Completed login must verify an identity: ${JSON.stringify(completed.login)}`);
      const completedText = await waitStatus(text => text.includes(`identity=${completed.identity}`));
      check(completedText.includes(`generation=${completed.generation}`), `Status must show generation=${completed.generation}: ${completedText}`);
      check(completedText.includes(`helper=${completed.helper?.version || 'Unknown'}`), `Status must show the helper version: ${completedText}`);
      check(completedText.includes(`auth_home=${completed.helper?.auth_home || 'Unknown'}`), `Status must show the helper auth home: ${completedText}`);
      const home = completed.helper?.auth_home || '';
      check(home.startsWith(`${window.__ISOLATION_CHECK__.root}/`), `Helper home must live under the isolation root ${window.__ISOLATION_CHECK__.root}: ${home}`);
      check(completed.generation === pending.generation, `A completed login must keep its own generation: ${pending.generation} -> ${completed.generation}`);
      record('pendingToCompleted', { from: pending.login.stage, to: completed.login.stage });
      record('completed', { generation: completed.generation, identity: completed.identity, home, login: completed.login, status: completedText });
      passed('verified identity, generation, helper version and isolated auth home in the row');
      // Codex helper 已在本进程自述；同一时刻 Grok 行必须仍显示 Unknown，不得借用这份自述。
      const grokDuring = await wait(async () => (await grokView()) || null, 'grok view during codex sign-in');
      check(grokDuring.helper?.available !== true && !grokDuring.helper?.version && !grokDuring.helper?.auth_home, `Grok must not borrow the Codex helper report: ${JSON.stringify(grokDuring.helper)}`);
      const grokDuringText = await waitStatusFor(grokProviderId, text => text.includes('helper=Unknown') && text.includes('auth_home=Unknown'));
      check(!grokDuringText.includes(completed.helper.version) && !grokDuringText.includes(home), `The Grok row must not show the Codex helper values: ${grokDuringText}`);
      record('grokBorrowCheck', { codex: { version: completed.helper?.version, home }, grok: { helper: grokDuring.helper ?? null, status: grokDuringText } });
      passed('the Grok row never borrows the Codex helper version or auth home');
      // 登录成功不得放行真实生成：订阅模型仍被准入拒绝，替身也不会收到生成请求。
      const samplesBefore = ((await invoke('get_model_performance')).models['codex-subscription-model']?.samples) ?? 0;
      await invoke('start_model_speed_tests', { ids: ['codex-subscription-model'] });
      const denied = await wait(async () => {
        const v = await invoke('get_model_performance');
        return !v.job.running && (v.models['codex-subscription-model']?.samples ?? 0) > samplesBefore ? v : null;
      }, 'denied speed test');
      check(Boolean(denied.job.error), `Subscription generation must stay denied after sign-in: ${JSON.stringify(denied.job)}`);
      record('deniedAfterLogin', { error: denied.job.error });
      passed('real generation stays denied after a successful sign-in');
      // 换号 = 退出 + 重新登录：世代递增，旧身份立刻消失，新登录仍绑定新世代。
      await click(`[data-testid="sub-switch-${providerId}"]`);
      const switched = await wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'pending' ? item : null; }, 'switched pending login');
      check(!switched.identity, `Switching accounts must clear the previous identity immediately: ${switched.identity}`);
      check(switched.generation > completed.generation, `Switching accounts must advance the generation: ${completed.generation} -> ${switched.generation}`);
      const switchedText = await waitStatus(text => text.includes('identity=Unknown') && text.includes(`generation=${switched.generation}`));
      check(!switchedText.includes(completed.identity), `Switching accounts must not keep showing the previous identity: ${switchedText}`);
      record('switch', { from: { generation: completed.generation, identity: completed.identity }, to: { generation: switched.generation, state: switched.state, identity: switched.identity ?? null, login: switched.login }, status: switchedText });
      passed('switching accounts advances the generation and drops the previous identity');
      const reconnected = await wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'completed' && item.identity ? item : null; }, 'second sign-in');
      check(reconnected.generation === switched.generation, `The switched sign-in must stay on its new generation: ${switched.generation} -> ${reconnected.generation}`);
      check(reconnected.identity === completed.identity, `The switched sign-in must verify the same stand-in account: ${reconnected.identity}`);
      const reconnectedText = await waitStatus(text => text.includes(`identity=${reconnected.identity}`) && text.includes('login=completed'));
      record('reconnected', { generation: reconnected.generation, identity: reconnected.identity, login: reconnected.login, status: reconnectedText });
      passed('the switched account signs in again on the new generation');
      await click(`[data-testid="sub-logout-${providerId}"]`);
      const after = await wait(async () => { const item = await subscriptionView(); return item?.logout && !item.identity ? item : null; }, 'logout outcome');
      check(after.generation > reconnected.generation, `Sign-out must advance the generation: ${reconnected.generation} -> ${after.generation}`);
      check(!after.identity, `Sign-out must clear the verified identity: ${after.identity}`);
      check((after.models || []).length === 0, `Sign-out must clear the discovered catalog: ${(after.models || []).length}`);
      check((after.capabilities || []).length === 0, `Sign-out must clear verified capabilities: ${(after.capabilities || []).length}`);
      check(clearedQuota(after.quota), `Sign-out must clear the quota evidence: ${JSON.stringify(after.quota)}`);
      const afterText = await waitStatus(text => /local=(cleared|retained)/.test(text) && /remote=(revoked|failed|unsupported|unknown)/.test(text));
      check(/local=(cleared|retained)/.test(afterText), `Status must report the local clearing result: ${afterText}`);
      check(/remote=(revoked|failed|unsupported|unknown)/.test(afterText), `Status must report the remote revocation result: ${afterText}`);
      const kept = await invoke('get_snapshot');
      check(kept.providers.some(item => item.id === providerId && item.kind === 'codex_subscription'), 'Sign-out must keep the subscription provider configuration');
      check(kept.models.some(item => item.id === 'codex-subscription-model' && item.provider_id === providerId), 'Sign-out must keep the subscription model configuration');
      record('afterLogout', { generation: after.generation, state: after.state, identity: after.identity ?? null, logout: after.logout, quota: after.quota, status: afterText });
      passed('sign-out advances the generation, clears caches and keeps provider/model configuration');
    }
    if (mode === 'late') {
      check(started.state !== 'connected', `Late rehearsal needs a disconnected subscription: ${started.state}`);
      // 先开始一次登录，再取消它；替身只会在收到 cancel 之后补发一个迟到的成功结果。
      const pending = await startLogin();
      await click(`[data-testid="sub-cancel-${providerId}"]`);
      const settled = await wait(async () => { const item = await subscriptionView(); return item?.login && item.login.stage !== 'pending' ? item : null; }, 'cancelled login');
      const cancelledText = await waitStatus(text => !text.includes('login=pending'));
      record('cancelled', { stage: settled.login.stage, state: settled.state, generation: settled.generation, identity: settled.identity ?? null, status: cancelledText });
      await new Promise(r => setTimeout(r, 900));
      const afterLate = await subscriptionView();
      check(afterLate.login.stage !== 'completed', `A late completion must not complete the cancelled login: ${afterLate.login.stage}`);
      check(!afterLate.identity, `A late completion must not revive an identity: ${afterLate.identity}`);
      check(afterLate.generation >= pending.generation, `A late completion must not roll the generation back: ${pending.generation} -> ${afterLate.generation}`);
      const lateText = await waitStatus(text => text.includes('identity=Unknown') && !text.includes('login=completed'));
      record('lateCompletion', { stage: afterLate.login.stage, generation: afterLate.generation, identity: afterLate.identity ?? null, status: lateText, pendingGeneration: pending.generation });
      passed('a late completion is discarded: no revived identity and no generation rollback');
      // 取消后状态回到未连接；随后再留一个挂起登录到桌面退出，用来验证下次启动会把
      // 落在盘上的 authorization_pending 归位成未连接，并且可以重新登录。
      const settledState = (await subscriptionView()).state;
      check(settledState === 'not_connected', `Cancelling must settle back to not_connected: ${settledState}`);
      const stillPending = await startLogin();
      const pendingText = await waitStatus(text => text.includes('login=pending'));
      check(!stillPending.identity, `A pending sign-in must not carry an identity: ${stillPending.identity}`);
      record('exitPending', { generation: stillPending.generation, state: stillPending.state, login: stillPending.login, status: pendingText });
      passed('desktop exits with a sign-in still pending');
    }
    if (mode === 'failed') {
      // 上一次运行在挂起登录中退出：重启后必须归位成未连接（不动世代与身份），否则登录按钮会被禁用。
      check(started.state === 'not_connected', `A restarted desktop must reconcile an orphaned pending sign-in: ${JSON.stringify({ state: started.state, generation: started.generation })}`);
      const reconciledText = await waitStatus(text => text.includes('login=idle'));
      record('reconciled', { state: started.state, generation: started.generation, identity: started.identity ?? null, status: reconciledText });
      const pending = await startLogin();
      const failed = await wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'failed' ? item : null; }, 'failed login');
      check(Boolean(failed.login.error), `A failed login must report an error: ${JSON.stringify(failed.login)}`);
      check(!failed.identity, `A failed login must not verify an identity: ${failed.identity}`);
      check(failed.generation >= pending.generation, `A failed login must not roll the generation back: ${pending.generation} -> ${failed.generation}`);
      const failedText = await waitStatus(text => text.includes('login=failed'));
      const failedRow = await wait(() => rowText().includes(failed.login.error) ? rowText() : null, 'failure reason in row');
      const snapshot = await invoke('get_snapshot');
      const view = snapshot.subscriptions.find(item => item.provider_id === providerId);
      check(view.login.stage === 'failed', `The backend snapshot must report the failure: ${JSON.stringify(view.login)}`);
      record('failed', { stage: failed.login.stage, error: failed.login.error, generation: failed.generation, status: failedText, row: failedRow.slice(-600), snapshot: JSON.stringify(view) });
      passed('a failed sign-in is reported in the row and the backend snapshot');
      passed('a restarted desktop reconciles the orphaned pending sign-in and can sign in again');
    }
    if (mode === 'grok') {
      // #12 边界：Grok 登录未实现，必须得到明确错误，且不拉起 Codex 替身、不改动任何连接。
      const grok = await wait(async () => (await grokView()) || null, 'grok view');
      const codexBefore = await subscriptionView();
      check(grok.state === 'not_connected', `Grok rehearsal needs an unconnected Grok row: ${grok.state}`);
      check(grok.helper?.available !== true && !grok.helper?.version && !grok.helper?.auth_home, `The Grok row must report an unavailable helper: ${JSON.stringify(grok.helper)}`);
      const grokText = await waitStatusFor(grokProviderId, text => text.includes('helper=Unknown') && text.includes('auth_home=Unknown'));
      record('grokStart', { state: grok.state, generation: grok.generation, login: grok.login, helper: grok.helper ?? null, status: grokText, codexBefore: { state: codexBefore.state, generation: codexBefore.generation, identity: codexBefore.identity ?? null } });
      await click(`[data-testid="sub-login-${grokProviderId}"]`);
      const rowError = await wait(() => {
        const row = [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(grokProviderId));
        const text = row?.textContent || '';
        return /not implemented|unsupported|not supported/i.test(text) ? text : null;
      }, 'grok sign-in error in row');
      const directError = await invoke('begin_subscription_login', { providerId: grokProviderId }).then(() => { throw new Error('Grok sign-in must be rejected'); }, error => String(error));
      check(/grok/i.test(directError) && /not implemented|unsupported|not supported/i.test(directError), `Grok sign-in must fail with a clear message: ${directError}`);
      const grokAfter = await wait(async () => (await grokView()) || null, 'grok view after rejection');
      check(grokAfter.state === 'not_connected' && grokAfter.generation === grok.generation, `A rejected Grok sign-in must not change its connection: ${JSON.stringify({ state: grokAfter.state, generation: grokAfter.generation })}`);
      check(grokAfter.login.stage === 'idle' && !grokAfter.identity, `A rejected Grok sign-in must not create a session: ${JSON.stringify(grokAfter.login)}`);
      const codexAfter = await subscriptionView();
      check(codexAfter.state === codexBefore.state && codexAfter.generation === codexBefore.generation && (codexAfter.identity ?? null) === (codexBefore.identity ?? null), `The Codex row must not be affected by the Grok action: ${JSON.stringify({ before: { state: codexBefore.state, generation: codexBefore.generation }, after: { state: codexAfter.state, generation: codexAfter.generation } })}`);
      record('grokRejected', { rowError: rowError.slice(-500), directError, after: { state: grokAfter.state, generation: grokAfter.generation, login: grokAfter.login }, codex: { state: codexAfter.state, generation: codexAfter.generation } });
      passed('an unsupported Grok sign-in is rejected with a clear error and changes nothing');
      passed('the Codex row is unaffected by the rejected Grok sign-in');
    }
  };
  try {
    await wait(() => document.querySelector('.app-shell'));
    if (window.__ISOLATION_CHECK__.loginMode) {
      try { await loginLifecycle(window.__ISOLATION_CHECK__.loginMode); report.ok = true; }
      catch (error) { report.error = String(error); report.screen = document.body.innerText.slice(-5000); }
      await invoke('isolation_check_report', { report });
      return;
    }
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
      check(subscription.adapter_available === true, 'The isolation build must install the real Codex helper adapter (no production stub)');
      check(subscription.denial && subscription.denial.code === 'not_connected', 'Unconnected subscription must stay denied');
      passed('subscription provider added from the provider panel with its real state');
      // #12 边界：再加一家 Grok 订阅服务商。它不得借用 Codex 的适配器与 helper 自述。
      await click('.provider-actions .button.primary');
      await click('.provider-dialog .search-select-trigger');
      (await wait(() => [...document.querySelectorAll('.provider-dialog [role="option"]')].find(option => /Grok subscription|Grok 订阅/.test(option.textContent)))).click();
      setValue(await wait(() => document.querySelector('.provider-dialog input[pattern="[a-zA-Z0-9_-]+"]')), grokProviderId);
      await click('.provider-dialog-actions button[type="submit"]');
      await wait(() => !document.querySelector('.provider-dialog'));
      await wait(() => [...document.querySelectorAll('tbody tr')].find(row => row.textContent.includes(grokProviderId)), 'grok row');
      const grokStart = await wait(async () => (await grokView()) || null, 'grok view');
      check(grokStart.state === 'not_connected', `Grok subscription must start unconnected: ${JSON.stringify(grokStart)}`);
      check(grokStart.login.stage === 'idle', `Grok must start without a sign-in session: ${JSON.stringify(grokStart.login)}`);
      check(grokStart.helper?.available !== true && !grokStart.helper?.version && !grokStart.helper?.auth_home, `Grok must not borrow the Codex helper report: ${JSON.stringify(grokStart.helper)}`);
      passed('grok subscription provider added without borrowing the Codex helper');
      await invoke('save_model', { model: { ...model, id: 'codex-subscription-model', provider_id: 'codex-subscription', model_id: 'codex-fixture-model', name: 'Codex fixture' } });
      await rejected('test_provider_draft', { provider: { ...provider, id: 'codex-subscription', name: 'Codex subscription', kind: 'codex_subscription', base_url: '', api_type: '', test_model: 'codex-fixture-model' } }, 'not connected');
      // 只读刷新不得把未连接伪装成已连接；替身的目录/额度读取尚未实现时必须如实报错。
      const refreshError = await invoke('refresh_subscription', { providerId: 'codex-subscription' }).then(() => null, error => String(error));
      const afterRefresh = (await invoke('get_snapshot')).subscriptions.find(s => s.provider_id === 'codex-subscription');
      check(afterRefresh.state !== 'connected', `Read-only refresh must not fake a connection: ${afterRefresh.state} (${refreshError})`);
      check(afterRefresh.denial && afterRefresh.denial.code === 'not_connected', `Unconnected subscription must stay denied: ${JSON.stringify(afterRefresh.denial)}`);
      check(afterRefresh.identity == null, `Read-only refresh must not invent an identity: ${afterRefresh.identity}`);
      await invoke('start_model_speed_tests', { ids: ['codex-subscription-model'] });
      const denied = await wait(async () => { const v = await invoke('get_model_performance'); return !v.job.running && v.job.completed === 3 && v; });
      check(/not connected/i.test(denied.job.error || ''), `Subscription speed test must report the reason: ${denied.job.error}`);
      passed('subscription generation denied without upstream work');
      // 订阅授权视图：不得借用适配器 helper 的自述、不得伪造远端撤销或本地清除。
      // Codex 的登录由 #13 的适配器替身在隔离环境里承载，因此这里只断言该视图保持中立。
      const subscriptionAuth = providerId => wait(async () => {
        const s = await invoke('get_snapshot');
        const auth = (s.subscription_auth ?? []).find(v => v.provider_id === providerId);
        const connection = (s.subscriptions ?? []).find(v => v.provider_id === providerId);
        return auth && connection ? { auth, connection } : null;
      });
      const authInitial = (await subscriptionAuth('codex-subscription')).auth;
      check(authInitial.helper.available === false, `Isolation must not report an available helper: ${JSON.stringify(authInitial.helper)}`);
      check(authInitial.phase === 'idle', `Isolation must not start a sign-in attempt: ${authInitial.phase}`);
      check(authInitial.logout.remote === 'not_attempted', `Remote revoke must stay unattempted: ${authInitial.logout.remote}`);
      check(authInitial.logout.local !== 'cleared', 'Isolation must not claim a local clear before any sign-out');
      passed('subscription authorization reports no helper and no revoke');
      // Grok 分支的端到端隔离证据：必须由 isolated() 拒绝，而不是被 helper_missing/helper_unsupported 挡下。
      const grokProvider = { ...provider, id: 'grok-subscription', kind: 'grok_subscription', name: 'Grok subscription', base_url: '', api_type: '', test_model: '' };
      await invoke('save_provider', { provider: grokProvider, apiKey: null });
      await invoke('save_model', { model: { ...model, id: 'grok-subscription-model', provider_id: 'grok-subscription', model_id: 'grok-fixture-model', name: 'Grok fixture' } });
      const grokInitial = await subscriptionAuth('grok-subscription');
      check(grokInitial.auth.helper.available === false, `Isolation must not report an available Grok helper: ${JSON.stringify(grokInitial.auth.helper)}`);
      check(grokInitial.auth.phase === 'idle', `Isolation must not start a Grok sign-in attempt: ${grokInitial.auth.phase}`);
      check(grokInitial.auth.logout.remote === 'not_attempted' && grokInitial.auth.logout.local !== 'cleared', `Grok sign-out evidence must stay unattempted: ${JSON.stringify(grokInitial.auth.logout)}`);
      check(grokInitial.connection.state === 'not_connected', `Grok subscription must start unconnected: ${JSON.stringify(grokInitial.connection)}`);
      await rejectedAny('begin_subscription_login', { providerId: 'grok-subscription' }, ['isolated', 'helper']);
      const grokAfterBegin = await subscriptionAuth('grok-subscription');
      check(grokAfterBegin.auth.phase === 'idle' && grokAfterBegin.connection.state === 'not_connected', `Rejected Grok sign-in must not change state: ${grokAfterBegin.auth.phase}/${grokAfterBegin.connection.state}`);
      await rejectedAny('logout_subscription', { providerId: 'grok-subscription' }, ['isolated', 'helper']);
      const grokAfterLogout = await subscriptionAuth('grok-subscription');
      check(grokAfterLogout.auth.logout.local === 'not_attempted' && grokAfterLogout.auth.logout.remote === 'not_attempted', `Rejected Grok sign-out must not claim a clear: ${JSON.stringify(grokAfterLogout.auth.logout)}`);
      check(grokAfterLogout.auth.phase === 'idle', `Rejected Grok sign-out must not start a sign-in: ${grokAfterLogout.auth.phase}`);
      passed('grok subscription authorization rejected by isolation, not by helper detection');
      // 界面侧：登录入口必须显示诚实失败原因，不得渲染凭据，也不得伪造远端撤销。
      // 两套 UI 并存：入口与对话框都必须锁定到 Grok 行自己的那一个（Codex 行也有同款入口）。
      await nav(1);
      const grokEntry = await wait(() => {
        const row = [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(grokProviderId));
        return row?.querySelector('.subscription-auth-entry:not([disabled])') ?? null;
      }, 'grok subscription sign-in entry');
      grokEntry.click();
      const grokDialog = () => {
        const dialog = document.querySelector('.subscription-auth-dialog');
        return dialog && dialog.textContent.includes(grokProviderId) ? dialog : null;
      };
      await wait(grokDialog, 'grok subscription auth dialog');
      // 界面显示的世代必须与后端快照一致，手测可据此逐步核对 generation 不变／attempt 加一。
      const grokGeneration = (await invoke('get_snapshot')).subscriptions.find(v => v.provider_id === grokProviderId).generation;
      const generationText = await wait(() => grokDialog()?.querySelector('.subscription-auth-generation')?.textContent?.trim() || null, 'grok dialog generation');
      check(generationText.includes(String(grokGeneration)), `Dialog generation must match the backend: ${generationText} vs ${grokGeneration}`);
      await click('.subscription-auth-dialog .subscription-auth-begin');
      const authError = await wait(() => grokDialog()?.querySelector('.subscription-auth-error')?.textContent?.trim() || null, 'grok dialog sign-in error');
      check(authError.length > 0, 'The sign-in entry must show why sign-in failed');
      const authDialogText = grokDialog()?.textContent ?? '';
      check(authDialogText.includes(grokProviderId), `The sign-in dialog must belong to the Grok row: ${authDialogText.slice(0, 200)}`);
      check(/未尝试远端撤销|Remote revoke not attempted/.test(authDialogText), `Sign-out evidence must keep the remote revoke unattempted: ${authDialogText}`);
      check(!/远端撤销已验证|Remote revoke verified/.test(authDialogText), 'Isolation must not claim a verified remote revoke');
      check(!/sk-[A-Za-z0-9]{4,}|Bearer\s[A-Za-z0-9]|api[_-]?key\s*[:=]/i.test(document.body.innerText), 'The sign-in dialog must not render credentials');
      await click('.subscription-auth-dialog .subscription-auth-footer button.primary');
      await wait(() => !grokDialog(), 'grok subscription auth dialog closed');
      passed('subscription sign-in UI shows an honest failure without credentials');
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
      await loginLifecycle('success');
    }
    report.ok = true;
  } catch (error) { report.error = String(error); report.screen = document.body.innerText.slice(-5000); }
  await invoke('isolation_check_report', { report });
})().catch(console.error);
