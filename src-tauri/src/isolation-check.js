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
      // Grok 由本票的授权对话框承载（订阅行按 kind 分派后，Grok 行不再有 #13 的动作按钮）：
      // 入口在 Grok 行内，拒绝原因只出现在该行的对话框里，行内不再显示错误文本。
      const grok = await wait(async () => (await grokView()) || null, 'grok view');
      const codexBefore = await subscriptionView();
      check(grok.state === 'not_connected', `Grok rehearsal needs an unconnected Grok row: ${grok.state}`);
      check(grok.helper?.available !== true && !grok.helper?.version && !grok.helper?.auth_home, `The Grok row must report an unavailable helper: ${JSON.stringify(grok.helper)}`);
      const grokText = await waitStatusFor(grokProviderId, text => text.includes('helper=Unknown') && text.includes('auth_home=Unknown'));
      record('grokStart', { state: grok.state, generation: grok.generation, login: grok.login, helper: grok.helper ?? null, status: grokText, codexBefore: { state: codexBefore.state, generation: codexBefore.generation, identity: codexBefore.identity ?? null } });
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
      await click('.subscription-auth-dialog .subscription-auth-begin');
      const rowError = await wait(() => {
        const text = grokDialog()?.querySelector('.subscription-auth-error')?.textContent?.trim();
        return text && /not implemented|unsupported|not supported/i.test(text) ? text : null;
      }, 'grok sign-in error in dialog');
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
      await click('.subscription-auth-dialog .subscription-auth-footer button.primary');
      await wait(() => !grokDialog(), 'grok subscription auth dialog closed');
    }
  };
  // #17 受控目录全过程：替身只把「上游目录与额度读取」换成脚本可控文件（--autojev-catalog-fixture），
  // 登录/退出、连接世代、准入、派发与快照仍走生产代码。步骤顺序与 scripts/check-isolated-desktop.mjs
  // 的目录读取队列一一对应：发现 → 未选 → 勾选 → 取消选择 → 停用 → 同账号失败 → 权威移除 → 换号 → 删行重建。
  const catalogModelId = 'codex-catalog-alpha';
  const catalogPublicId = `codex-subscription/${catalogModelId}`;
  const catalogLifecycle = async () => {
    const details = { mode: 'catalog', observations: [] };
    report.details = details;
    const record = (label, value) => details.observations.push({ label, value });
    const snapshot = () => invoke('get_snapshot');
    const view = async () => (await snapshot()).subscriptions.find(item => item.provider_id === providerId);
    // 目录视图按模型行组装；目录项缺失时（例如刚删掉模型行）等待下一次核对重建。
    const entry = async () => {
      const found = await wait(async () => {
        const item = await view();
        return item?.catalog?.find(row => row.model_id === catalogModelId) ?? null;
      }, 'controlled catalog entry');
      return { ...found };
    };
    const row = async () => (await snapshot()).models.find(model => model.model_id === catalogModelId);
    const setModel = async changes => {
      const current = await row();
      check(Boolean(current), 'The discovered model row must exist before changing it');
      await invoke('save_model', { model: { ...current, ...changes } });
    };
    // 显式原 ID 直调走真实网关：Debug 入口与客户端请求共用同一准入，返回稳定的拒绝 code。
    const admission = async label => {
      const proxy = (await snapshot()).proxy;
      check(proxy.running, 'The local gateway must be running for the direct-call probe');
      const onProgress = (() => {
        const id = window.__TAURI_INTERNALS__.transformCallback(() => {}, false);
        const serialize = () => `__CHANNEL__:${id}`;
        return { __TAURI_TO_IPC_KEY__: serialize, toJSON: serialize };
      })();
      const result = await invoke('debug_curl', {
        id: `catalog-${label}`,
        endpoint: 'chat/completions',
        body: { model: catalogPublicId, messages: [{ role: 'user', content: 'fictional catalog probe' }], stream: false },
        headers: {},
        onProgress,
      });
      const headers = Object.fromEntries(Object.entries(result.headers || {}).map(([name, value]) => [name.toLowerCase(), value]));
      const parsed = (() => { try { return JSON.parse(result.body); } catch { return {}; } })();
      const observation = {
        label,
        status: result.status,
        code: headers['x-autojev-subscription-denial'] || parsed?.error?.code || null,
        message: parsed?.error?.message ?? null,
      };
      record(`direct:${label}`, observation);
      return observation;
    };
    // 公共目录（无 Agent 头）由验收替身的 Node 侧代取：应用内 fetch 受同源策略限制，读取入口仍是真实网关。
    const publicCatalog = async () => {
      const proxy = (await snapshot()).proxy;
      check(proxy.running, 'The local gateway must be running for the public catalog probe');
      const response = await fetch(`${window.__ISOLATION_CHECK__.base}/__catalog-list`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ port: proxy.port }),
      });
      check(response.ok, `Public catalog probe failed: ${response.status}`);
      const listing = await response.json();
      check(Array.isArray(listing.ids), `Public catalog probe returned no ids: ${JSON.stringify(listing)}`);
      return listing;
    };
    const start = await wait(async () => (await view()) || null, 'subscription view');
    check(start.state !== 'connected', `The catalog rehearsal needs a disconnected subscription: ${start.state}`);
    check((start.catalog || []).every(item => item.model_id !== catalogModelId), `The controlled model must not exist before discovery: ${JSON.stringify(start.catalog)}`);
    record('start', { state: start.state, generation: start.generation, identity: start.identity ?? null, catalog: start.catalog });
    // 1. 真实登录（替身辅助进程承载官方握手），随后启动网关并做一次目录核对。
    await nav(1);
    await wait(() => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(providerId)), 'subscription row');
    await click(`[data-testid="sub-login-${providerId}"]`);
    const signedIn = await wait(async () => {
      const item = await view();
      return item?.login?.stage === 'completed' ? item : null;
    }, 'catalog sign-in');
    check(signedIn.identity === 'standin-success@example.invalid', `The controlled directory needs a verified stand-in account: ${signedIn.identity}`);
    await click('.sidebar [aria-busy]');
    const gateway = await wait(async () => { const current = await snapshot(); return current.proxy.running ? current.proxy : null; }, 'local gateway');
    check(gateway.port !== 9526 && gateway.port !== 9527 && gateway.port !== 0, `The gateway must use an independent port: ${gateway.port}`);
    const refreshed = await invoke('refresh_subscription', { providerId });
    const discovered = await entry();
    check(discovered.availability === 'available', `A successful directory read must confirm the entry: ${JSON.stringify(discovered)}`);
    check(discovered.eligibility === 'eligible', `A confirmed entry must be eligible: ${JSON.stringify(discovered)}`);
    check(discovered.selected === false && discovered.disabled === false, `A newly discovered model must start unselected and enabled: ${JSON.stringify(discovered)}`);
    const discoveredRow = await row();
    check(Boolean(discoveredRow), 'A discovered model must be materialized as a model row');
    check(discoveredRow.id === discovered.internal_id, `The internal ID must be the model row ID: ${JSON.stringify({ row: discoveredRow.id, entry: discovered.internal_id })}`);
    check(discoveredRow.selected === false && discoveredRow.enabled === true, `The materialized row must start unselected and enabled: ${JSON.stringify(discoveredRow)}`);
    const unselectedList = await publicCatalog();
    check(!unselectedList.ids.includes(catalogPublicId), `An unselected model must stay out of the public catalog: ${JSON.stringify(unselectedList)}`);
    record('discovered', {
      generation: signedIn.generation, identity: signedIn.identity, port: gateway.port,
      internal_id: discovered.internal_id, availability: discovered.availability, eligibility: discovered.eligibility,
      selected: discovered.selected, disabled: discovered.disabled,
      upstream: (await view()).models, publicCatalog: unselectedList.ids,
      refreshState: refreshed.subscriptions.find(item => item.provider_id === providerId)?.state ?? null,
    });
    record('unselected', { publicCatalog: unselectedList.ids, containsDiscovered: unselectedList.ids.includes(catalogPublicId) });
    passed('a newly discovered subscription model stays unselected and out of the public catalog');
    // 2. 勾选：进入公共目录；直调准入结论不变。
    await setModel({ selected: true });
    const selected = await entry();
    check(selected.selected === true, `Selecting must reach the user configuration: ${JSON.stringify(selected)}`);
    const selectedList = await publicCatalog();
    check(selectedList.ids.includes(catalogPublicId), `A selected model must appear in the public catalog: ${JSON.stringify(selectedList)}`);
    const admittedWhileSelected = await admission('selected');
    record('selected', { publicCatalog: selectedList.ids, internal_id: selected.internal_id });
    passed('selecting the discovered model puts it into the public catalog');
    // 3. 取消选择：移出公共目录；显式原 ID 直调的准入结论必须与已选时完全一致。
    await setModel({ selected: false });
    const deselected = await entry();
    check(deselected.selected === false && deselected.disabled === false, `Deselecting must only change the selection flag: ${JSON.stringify(deselected)}`);
    const deselectedList = await publicCatalog();
    check(!deselectedList.ids.includes(catalogPublicId), `Deselecting must remove the model from the public catalog: ${JSON.stringify(deselectedList)}`);
    const admittedWhileDeselected = await admission('deselected');
    check(
      admittedWhileDeselected.code === admittedWhileSelected.code,
      `Deselection must not change the direct-call admission: ${JSON.stringify({ selected: admittedWhileSelected, deselected: admittedWhileDeselected })}`,
    );
    check(admittedWhileDeselected.code !== 'model_disabled', `Deselection must not disable the model: ${JSON.stringify(admittedWhileDeselected)}`);
    record('deselected', {
      publicCatalog: deselectedList.ids, publicCatalog_contains: deselectedList.ids.includes(catalogPublicId),
      admissionWhileSelected: admittedWhileSelected, admissionWhileDeselected: admittedWhileDeselected,
    });
    passed('deselecting removes the model from the public catalog and keeps the direct-call admission result');
    // 4. 停用：后端禁止一切调用（含原 ID 直调），公共目录与手动测速都不放行。
    await setModel({ enabled: false, selected: true });
    const disabledEntry = await entry();
    check(disabledEntry.disabled === true && disabledEntry.selected === true, `Disabling must be reported separately from selection: ${JSON.stringify(disabledEntry)}`);
    const disabledList = await publicCatalog();
    check(!disabledList.ids.includes(catalogPublicId), `A disabled model must stay out of the public catalog: ${JSON.stringify(disabledList)}`);
    const deniedWhileDisabled = await admission('disabled');
    check(deniedWhileDisabled.code === 'model_disabled', `Disabling must deny every direct call: ${JSON.stringify(deniedWhileDisabled)}`);
    // 手动测速入口：停用的模型连任务都不该起（拒绝启动或任务以拒绝原因结束，两种都算禁止派发）。
    const started = await invoke('start_model_speed_tests', { ids: [discoveredRow.id] }).then(() => null, error => String(error));
    const measured = started === null
      ? await wait(async () => { const value = await invoke('get_model_performance'); return !value.job.running ? value : null; }, 'disabled speed test')
      : null;
    const speedTestError = started ?? measured?.job?.error ?? null;
    check(Boolean(speedTestError), 'A disabled model must not be measured');
    record('disabled', {
      publicCatalog: disabledList.ids, direct: deniedWhileDisabled, internal_id: disabledEntry.internal_id,
      speedTestError,
    });
    passed('disabling denies the direct call, the public catalog and the manual speed test');
    // 5. 同账号目录失败：保留已核实项、只标陈旧，直调资格仍成立（网络失败 ≠ 被移除）。
    await setModel({ enabled: true, selected: true });
    const failure = await invoke('refresh_subscription', { providerId }).then(() => null, error => String(error));
    check(Boolean(failure), 'A failed directory read must be reported instead of hidden');
    const stale = await entry();
    check(stale.availability === 'stale', `A failed read must keep the entry and mark it stale: ${JSON.stringify(stale)}`);
    check(stale.eligibility === 'stale', `A stale entry stays qualified for the same account: ${JSON.stringify(stale)}`);
    check(stale.selected === true && stale.disabled === false, `A failed read must not rewrite user configuration: ${JSON.stringify(stale)}`);
    const staleRow = await row();
    check(Boolean(staleRow), 'A failed read must keep the model row');
    const admittedWhileStale = await admission('stale');
    check(admittedWhileStale.code === admittedWhileSelected.code, `A stale entry must keep the direct-call admission result: ${JSON.stringify(admittedWhileStale)}`);
    record('stale', { error: failure, availability: stale.availability, eligibility: stale.eligibility, selected: stale.selected, disabled: stale.disabled, direct: admittedWhileStale });
    passed('a same-account directory failure keeps the verified entry, marks it stale and keeps it qualified');
    // 6. 权威移除：目录中不再出现 → 不可用，但配置、选择与稳定标识都保留。
    await invoke('refresh_subscription', { providerId });
    const removed = await entry();
    check(removed.availability === 'removed', `An authoritatively removed model must be marked removed: ${JSON.stringify(removed)}`);
    check(removed.eligibility === 'removed', `A removed model must be ineligible: ${JSON.stringify(removed)}`);
    check(removed.internal_id === discovered.internal_id, `An authoritative removal must not change the internal ID: ${JSON.stringify(removed)}`);
    check(removed.selected === true && removed.disabled === false, `An authoritative removal must not rewrite user configuration: ${JSON.stringify(removed)}`);
    const removedList = await publicCatalog();
    check(!removedList.ids.includes(catalogPublicId), `A removed model must stay out of the public catalog: ${JSON.stringify(removedList)}`);
    const deniedWhileRemoved = await admission('removed');
    check(deniedWhileRemoved.code === 'model_removed', `A removed model must be denied with its own code: ${JSON.stringify(deniedWhileRemoved)}`);
    record('removed', {
      publicCatalog: removedList.ids, internal_id: removed.internal_id,
      availability: removed.availability, eligibility: removed.eligibility,
      selected: removed.selected, disabled: removed.disabled, direct: deniedWhileRemoved,
    });
    passed('an authoritative removal denies the model with model_removed and keeps its configuration');
    // 7. 换号：资格整体作废（保留选择/停用与标识），重新登录并核对后恢复可用。
    await setModel({ selected: true, enabled: false });
    const beforeSwitch = await view();
    await click(`[data-testid="sub-switch-${providerId}"]`);
    const switching = await wait(async () => {
      const item = await view();
      return item?.login?.stage === 'pending' ? item : null;
    }, 'catalog account switch');
    check(!switching.identity, `Switching accounts must clear the previous identity: ${switching.identity}`);
    check(switching.generation > beforeSwitch.generation, `Switching accounts must advance the generation: ${beforeSwitch.generation} -> ${switching.generation}`);
    const invalidated = await entry();
    check(invalidated.availability === 'unknown', `Switching accounts must invalidate the confirmed catalogue: ${JSON.stringify(invalidated)}`);
    check(invalidated.eligibility === 'account_changed', `Switching accounts must bind eligibility to the new account: ${JSON.stringify(invalidated)}`);
    check(invalidated.selected === true && invalidated.disabled === true, `Switching accounts must keep selection and disable state: ${JSON.stringify(invalidated)}`);
    const reconnected = await wait(async () => {
      const item = await view();
      return item?.login?.stage === 'completed' && item.identity ? item : null;
    }, 'catalog re-sign-in');
    check(reconnected.identity === signedIn.identity, `The switched sign-in must verify the stand-in account: ${reconnected.identity}`);
    await invoke('refresh_subscription', { providerId });
    const reverified = await entry();
    check(reverified.availability === 'available' && reverified.eligibility === 'eligible', `A re-verified catalogue must be available again: ${JSON.stringify(reverified)}`);
    check(reverified.internal_id === discovered.internal_id, `Re-verifying must reuse the stable internal ID: ${JSON.stringify(reverified)}`);
    check(reverified.selected === true && reverified.disabled === true, `Selection and disable state must survive the account switch: ${JSON.stringify(reverified)}`);
    const deniedWhileSwitched = await admission('switched');
    check(deniedWhileSwitched.code === 'model_disabled', `The preserved disable state must still deny calls: ${JSON.stringify(deniedWhileSwitched)}`);
    record('switched', {
      from: { generation: beforeSwitch.generation }, to: { generation: reconnected.generation, identity: reconnected.identity },
      invalidated: { availability: invalidated.availability, eligibility: invalidated.eligibility, selected: invalidated.selected, disabled: invalidated.disabled },
      reverified: { availability: reverified.availability, eligibility: reverified.eligibility, internal_id: reverified.internal_id, selected: reverified.selected, disabled: reverified.disabled },
      direct: deniedWhileSwitched,
    });
    passed('switching accounts invalidates the catalogue, then re-verifies it with selection and disable kept');
    // 8. 删除模型行：目录项与稳定标识不被删除，下一次核对用同一个 internal_id 重建行。
    await setModel({ selected: true, enabled: true });
    const stableId = (await row()).id;
    await invoke('delete_model', { id: stableId });
    check(!(await row()), 'Deleting must remove the model row');
    await invoke('refresh_subscription', { providerId });
    const restored = await wait(async () => (await row()) ?? null, 're-materialized model row');
    check(restored.id === stableId, `The re-materialized row must reuse the stable internal ID: ${JSON.stringify({ stableId, restored: restored.id })}`);
    const restoredEntry = await entry();
    check(restoredEntry.internal_id === stableId, `The catalog entry must keep the stable internal ID: ${JSON.stringify(restoredEntry)}`);
    record('deletedRow', { internal_id: stableId, restored: restored.id, selected: restored.selected, availability: restoredEntry.availability });
    passed('deleting a model row keeps the catalogue entry and rebuilds the row with the same stable ID');
    // 9. Agent 保存目录：与当前选择不一致时显示待同步；后端撤销不因外部待同步而推迟。
    await setModel({ selected: true, enabled: true });
    await invoke('connect_agent', { id: 'codex', routeId: `model/${stableId}` });
    const agent = async () => (await snapshot()).agents.find(item => item.id === 'codex');
    const connected = await wait(async () => {
      const item = await agent();
      return item?.route_id === `model/${stableId}` ? item : null;
    }, 'agent saved catalog');
    check(connected.catalog_pending_sync === false, `A freshly saved Agent catalog must not be pending: ${JSON.stringify(connected)}`);
    check((((await snapshot()).agent_catalogs || {}).codex || []).length === 1, `The Agent catalog must be saved: ${JSON.stringify((await snapshot()).agent_catalogs)}`);
    await setModel({ selected: false });
    const pendingSync = await wait(async () => {
      const item = await agent();
      return item?.catalog_pending_sync === true ? item : null;
    }, 'agent pending sync');
    const admittedWhilePending = await admission('pending-sync');
    check(admittedWhilePending.code === admittedWhileSelected.code, `An external pending sync must not change backend admission: ${JSON.stringify(admittedWhilePending)}`);
    await setModel({ enabled: false });
    const pendingWhileDisabled = await wait(async () => {
      const item = await agent();
      return item?.catalog_pending_sync === true ? item : null;
    }, 'agent pending sync while disabled');
    const revokedWhilePending = await admission('pending-sync-disabled');
    check(revokedWhilePending.code === 'model_disabled', `The backend revocation must not wait for the external configuration: ${JSON.stringify(revokedWhilePending)}`);
    record('pendingSync', {
      connected: { route_id: connected.route_id, pending: connected.catalog_pending_sync },
      pending: pendingSync.catalog_pending_sync,
      pendingWhileDisabled: pendingWhileDisabled.catalog_pending_sync,
      admissionWhilePending: admittedWhilePending,
      admissionAfterDisable: revokedWhilePending,
    });
    passed('an out-of-date saved Agent catalogue is reported as pending while the backend revokes immediately');
    // 10. 收起会话：注销走生产路径，专用授权目录随退出清空，替身凭据不留盘（保持既有清理断言）。
    await click(`[data-testid="sub-logout-${providerId}"]`);
    const closedOut = await wait(async () => {
      const item = await view();
      return item?.logout && !item.identity ? item : null;
    }, 'catalog sign-out');
    check(clearedQuota(closedOut.quota), `Sign-out must clear the quota evidence: ${JSON.stringify(closedOut.quota)}`);
    check(closedOut.generation > reconnected.generation, `Sign-out must advance the generation: ${reconnected.generation} -> ${closedOut.generation}`);
    const invalidatedByLogout = await entry();
    check(invalidatedByLogout.eligibility === 'account_changed', `Sign-out must invalidate the catalogue eligibility: ${JSON.stringify(invalidatedByLogout)}`);
    record('closedOut', {
      generation: closedOut.generation, state: closedOut.state, identity: closedOut.identity ?? null, logout: closedOut.logout,
      availability: invalidatedByLogout.availability, eligibility: invalidatedByLogout.eligibility,
    });
    passed('signing out invalidates the catalogue eligibility and clears the dedicated helper home');
  };
  try {
    await wait(() => document.querySelector('.app-shell'));
    if (window.__ISOLATION_CHECK__.loginMode) {
      try {
        if (window.__ISOLATION_CHECK__.loginMode === 'catalog') await catalogLifecycle();
        else await loginLifecycle(window.__ISOLATION_CHECK__.loginMode);
        report.ok = true;
      }
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
      // 订阅行按 kind 分派后 Grok 行只渲染本票入口：仍要锁定 Grok 行自己的入口与含 grok-subscription 的对话框。
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
