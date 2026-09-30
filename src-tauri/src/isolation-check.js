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
      // Issue #16：隔离验证环境里不得拉起任何 Grok 辅助进程。只读 refresh 必须如实失败，
      // 且一个连接字段都不改写；从未读取过的目录/额度证据必须保持 unknown，不得伪造 0/100%。
      const grokReadBefore = await wait(async () => (await grokView()) || null, 'grok view before read-only refresh');
      const grokReadBeforeJson = JSON.stringify(grokReadBefore);
      const refreshError = await invoke('refresh_subscription', { providerId: grokProviderId }).then(() => null, error => String(error));
      check(refreshError !== null, 'A read-only Grok refresh must not succeed while isolated validation provides no Grok helper');
      check(/isolated|helper|not implemented|unsupported|not supported/i.test(refreshError), `A read-only Grok refresh must fail honestly: ${refreshError}`);
      const grokReadAfter = await wait(async () => (await grokView()) || null, 'grok view after read-only refresh');
      check(JSON.stringify(grokReadAfter) === grokReadBeforeJson, `A failed isolated Grok refresh must not rewrite any connection field: before=${grokReadBeforeJson} after=${JSON.stringify(grokReadAfter)}`);
      check(grokReadAfter.state === 'not_connected', `An isolated Grok refresh must leave the connection unconnected: ${grokReadAfter.state}`);
      check(grokReadAfter.generation === grokReadBefore.generation, `An isolated Grok refresh must not advance the generation: ${grokReadBefore.generation} -> ${grokReadAfter.generation}`);
      check(!grokReadAfter.identity, `An isolated Grok refresh must not invent an identity: ${grokReadAfter.identity}`);
      check(grokReadAfter.catalog?.state === 'unknown', `Grok catalog evidence must stay unknown instead of being fabricated: ${JSON.stringify(grokReadAfter.catalog)}`);
      check(grokReadAfter.quota?.state === 'unknown', `Grok quota evidence must stay unknown instead of being fabricated: ${JSON.stringify(grokReadAfter.quota)}`);
      check((grokReadAfter.quota?.buckets || []).length === 0, `An unread Grok quota must keep its buckets empty: ${JSON.stringify(grokReadAfter.quota?.buckets)}`);
      check((grokReadAfter.models || []).length === 0, `An unread Grok catalog must keep its models empty: ${(grokReadAfter.models || []).length}`);
      check(grokReadAfter.helper?.available !== true && !grokReadAfter.helper?.version && !grokReadAfter.helper?.auth_home, `The Grok row must report an unavailable helper: ${JSON.stringify(grokReadAfter.helper)}`);
      const catalogText = await wait(() => document.querySelector(`[data-testid="sub-catalog-${grokProviderId}"]`)?.textContent || null, 'grok catalog read-only text');
      const quotaText = await wait(() => document.querySelector(`[data-testid="sub-quota-${grokProviderId}"]`)?.textContent || null, 'grok quota read-only text');
      check(/catalog_state=unknown\b/.test(catalogText), `Grok catalog text must read catalog_state=unknown: ${catalogText}`);
      check(/quota_state=unknown\b/.test(quotaText), `Grok quota text must read quota_state=unknown: ${quotaText}`);
      check(!/\bbucket=/.test(quotaText), `An unread Grok quota must not render a bucket segment: ${quotaText}`);
      check(!/(used|remaining|minutes|resets_at|balance|has_credits|unlimited|credit)=/.test(quotaText), `An unread Grok quota must not render window or credit values: ${quotaText}`);
      check(!/\d/.test(quotaText), `An unread Grok quota must not fabricate numbers (0/100%): ${quotaText}`);
      check(!/\d/.test(catalogText), `An unread Grok catalog must not fabricate numbers: ${catalogText}`);
      check(!/models=\S/.test(catalogText), `An unread Grok catalog must not list models: ${catalogText}`);
      // 只读块：unknown 状态下必须给出官方查看入口（只看，不作为绕过准入的依据）。
      const grokRow = await wait(() => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(grokProviderId)) || null, 'grok row for the read-only block');
      const readOnlyBlock = (() => {
        const catalog = grokRow.querySelector(`[data-testid="sub-catalog-${grokProviderId}"]`);
        const quota = grokRow.querySelector(`[data-testid="sub-quota-${grokProviderId}"]`);
        if (!catalog || !quota) return grokRow;
        let node = catalog;
        while (node && node !== grokRow && !node.contains(quota)) node = node.parentElement;
        return node && node.contains(quota) ? node : grokRow;
      })();
      const officialLinks = [...readOnlyBlock.querySelectorAll('a[href]')].map(anchor => anchor.getAttribute('href') || '');
      check(officialLinks.some(href => href.includes('docs.x.ai/build/cli/reference')), `An unknown Grok catalog must offer the official reference: ${JSON.stringify(officialLinks)}`);
      check(officialLinks.some(href => href.includes('docs.x.ai/grok/faq')), `An unknown Grok quota must offer the official reference: ${JSON.stringify(officialLinks)}`);
      // #12 边界：#16 的只读块不得让 Grok 行借用 Codex 适配器的 helper 版本或授权目录。
      const codexDuringGrok = await subscriptionView();
      const codexVersion = codexDuringGrok?.helper?.version || '';
      const codexHome = codexDuringGrok?.helper?.auth_home || '';
      const grokStatusText = await waitStatusFor(grokProviderId, text => text.includes('helper=Unknown') && text.includes('auth_home=Unknown'));
      if (codexVersion) check(!grokStatusText.includes(codexVersion), `The Grok row must not show the Codex helper version: ${grokStatusText}`);
      if (codexHome) check(!grokStatusText.includes(codexHome), `The Grok row must not show the Codex auth home: ${grokStatusText}`);
      // #16：生成准入保持拒绝。测速请求必须如实失败、样本不增加，绝不到达任何真实上游
      //（驱动侧再用 fixture 请求账本复核 Grok 订阅模型零派发）。
      const grokModelId = 'grok-subscription-model';
      const grokSnapshot = await invoke('get_snapshot');
      check(grokSnapshot.models.some(item => item.id === grokModelId && item.provider_id === grokProviderId), `The Grok subscription model configuration must survive read-only refreshes: ${JSON.stringify(grokSnapshot.models.filter(item => item.provider_id === grokProviderId))}`);
      const samplesBefore = ((await invoke('get_model_performance')).models[grokModelId]?.samples) ?? 0;
      const startError = await invoke('start_model_speed_tests', { ids: [grokModelId] }).then(() => null, error => String(error));
      const denied = await wait(async () => {
        const value = await invoke('get_model_performance');
        return !value.job.running && (value.job.error || startError) ? value : null;
      }, 'denied grok speed test');
      const deniedReason = String(startError || denied.job.error || '');
      check(deniedReason.length > 0, `A denied Grok subscription test must report a reason: ${JSON.stringify(denied.job)}`);
      check(/not connected|not_connected|not implemented|unsupported|not supported|denied|未连接/i.test(deniedReason), `A denied Grok subscription test must report an honest reason: ${deniedReason}`);
      record('grokReadOnly', {
        refreshError,
        unchanged: JSON.stringify(grokReadAfter) === grokReadBeforeJson,
        before: { state: grokReadBefore.state, generation: grokReadBefore.generation, identity: grokReadBefore.identity ?? null, catalogState: grokReadBefore.catalog?.state ?? null, quotaState: grokReadBefore.quota?.state ?? null, buckets: (grokReadBefore.quota?.buckets || []).length, models: (grokReadBefore.models || []).length },
        after: { state: grokReadAfter.state, generation: grokReadAfter.generation, identity: grokReadAfter.identity ?? null, catalogState: grokReadAfter.catalog?.state ?? null, quotaState: grokReadAfter.quota?.state ?? null, buckets: (grokReadAfter.quota?.buckets || []).length, models: (grokReadAfter.models || []).length, helper: grokReadAfter.helper ?? null },
        catalogText, quotaText, links: officialLinks, status: grokStatusText,
        codexHelper: { version: codexVersion || null, auth_home: codexHome || null },
      });
      passed('an isolated Grok read-only refresh fails honestly without spawning a helper or rewriting any field');
      passed('the unread Grok catalog and quota stay unknown with no bucket segment and no fabricated numbers');
      passed('the unknown Grok read-only block offers the official catalog and quota references');
      passed('the Grok row never borrows the Codex helper version or auth home');
      // `samples` 只是尝试计数（失败的尝试也会记录），所以「零派发」由驱动侧的 fixture 请求账本判定；
      // 这里断言本次拒绝没有被记成任何成功测量（success_rate 必须为 0），不得伪造成可用。
      const deniedSummary = denied.models[grokModelId] ?? {};
      check((deniedSummary.success_rate ?? 0) === 0, `A denied Grok subscription test must never be recorded as a success: ${JSON.stringify(deniedSummary)}`);
      record('grokDenied', {
        reason: deniedReason.slice(-400),
        samplesBefore,
        samplesAfter: deniedSummary.samples ?? null,
        successRate: deniedSummary.success_rate ?? null,
      });
      passed('a denied Grok subscription generation reports a reason without a fabricated success or upstream dispatch');
    }
    if (mode === 'grok-read') {
      // Issue #16：隔离替身真的回复目录/额度。走真实界面路径（行内 Refresh 按钮 → IPC → 适配器 → 替身），
      // 再用 get_snapshot 交叉核对同一批数值。替身场景队列一次调用消费一项，刷新顺序 account → models → usage。
      await nav(1);
      const grokRowFor = () => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(grokProviderId)) || null;
      await wait(grokRowFor, 'grok row for the read-only stand-in');
      // 边界（先于任何只读读取）：只读替身绝不代表登录被放行。连接尚未建立时登录仍必须被隔离如实拒绝。
      const authBefore = await wait(async () => {
        const snapshot = await invoke('get_snapshot');
        return (snapshot.subscription_auth ?? []).find(item => item.provider_id === grokProviderId) || null;
      }, 'grok subscription auth view before the stand-in reads');
      check(authBefore.helper.available === false, `The read-only stand-in must not make the sign-in helper available: ${JSON.stringify(authBefore.helper)}`);
      await rejectedAny('begin_subscription_login', { providerId: grokProviderId }, ['isolated', 'helper']);
      passed('the pinned read-only helper never unlocks Grok sign-in in isolation');
      const textOf = testid => document.querySelector(`[data-testid="${testid}"]`)?.textContent || '';
      const modelsToken = text => (text.match(/\bmodels=(\S*)/) || ['', ''])[1];
      const removedToken = text => (text.match(/\bremoved=(\S*)/) || ['', ''])[1];
      const clickRefresh = async label => {
        const button = await wait(() => {
          const row = grokRowFor();
          return [...(row?.querySelectorAll('.row-actions button') ?? [])].find(item =>
            !item.classList.contains('subscription-auth-entry') &&
            /refresh|刷新/i.test(`${item.getAttribute('aria-label') || ''} ${item.getAttribute('title') || ''}`) &&
            !item.disabled) || null;
        }, `grok refresh button ${label}`);
        button.click();
      };
      // 1) account/models/usage 全部成功：目录两个模型（eligible=false）、额度 available、两个许可轴分开。
      await clickRefresh('#1');
      const catalog1 = await wait(() => { const text = textOf(`sub-catalog-${grokProviderId}`); return /catalog_state=available\b/.test(text) ? text : null; }, 'round 1 catalog available');
      const quota1 = await wait(() => { const text = textOf(`sub-quota-${grokProviderId}`); return /quota_state=available\b/.test(text) ? text : null; }, 'round 1 quota available');
      check(modelsToken(catalog1) === 'grok-build:false,grok-mini:false', `Round 1 catalog must list both stand-in models as not eligible: ${catalog1}`);
      check(catalog1.includes('source=grok-cli:models') && catalog1.includes('observed_at=2026-01-02T03:04:05Z'), `Round 1 catalog source/time must come from the stand-in: ${catalog1}`);
      check(/permission=allowed\b/.test(quota1) && /permission:denied\b/.test(quota1), `The pool permission and the credits permission must stay separate axes: ${quota1}`);
      check(quota1.includes('used=42.5') && quota1.includes('remaining=57.5'), `Round 1 quota must show the stand-in usage and its derived remainder: ${quota1}`);
      check(quota1.includes('credits=has:true,unlimited:false,balance=12.50,unit=USD,permission:denied'), `Round 1 credits must keep the raw balance and unit: ${quota1}`);
      check(quota1.includes('source=grok-cli:usage') && quota1.includes('observed_at=2026-01-02T03:04:06Z'), `Round 1 quota source/time must come from the stand-in: ${quota1}`);
      const view1 = await wait(async () => { const view = await grokView(); return view?.catalog?.state === 'available' && view?.quota?.state === 'available' ? view : null; }, 'round 1 snapshot');
      const bucket1 = view1.quota?.buckets?.[0] ?? {};
      const window1 = bucket1.windows?.[0] ?? {};
      check(view1.models.map(model => model.model_id).join(',') === 'grok-build,grok-mini', `Snapshot catalog must carry both stand-in models: ${JSON.stringify(view1.models)}`);
      check(view1.models.every(model => model.eligible === false), `Discovered models must stay not eligible: ${JSON.stringify(view1.models)}`);
      check(view1.state === 'connected' && view1.identity === 'standin-grok@example.invalid', `Snapshot must bind the stand-in identity: ${JSON.stringify({ state: view1.state, identity: view1.identity })}`);
      check(window1.used_percent === 42.5 && bucket1.permission === 'allowed', `Snapshot must carry the stand-in pool mapping: ${JSON.stringify(bucket1)}`);
      check(bucket1.credits?.balance === '12.50' && bucket1.credits?.unit === 'USD' && bucket1.credits?.permission === 'denied', `Snapshot credits must stay raw and on their own axis: ${JSON.stringify(bucket1.credits)}`);
      check(typeof window1.resets_at === 'number', `The stand-in reset time must be mapped to Unix seconds: ${JSON.stringify(window1)}`);
      record('grokReadRound1', { catalogText: catalog1, quotaText: quota1, snapshot: { state: view1.state, identity: view1.identity, models: view1.models, bucket: bucket1 } });
      passed('round 1 maps the stand-in catalog and quota into the row and the snapshot');
      // 2) models=removed + usage=denied：退场模型只说明「这次目录里没有了」，存活模型仍在。
      await clickRefresh('#2');
      const catalog2 = await wait(() => { const text = textOf(`sub-catalog-${grokProviderId}`); return /removed=grok-mini\b/.test(text) ? text : null; }, 'round 2 removed token');
      const quota2 = await wait(() => { const text = textOf(`sub-quota-${grokProviderId}`); return /quota_state=denied\b/.test(text) ? text : null; }, 'round 2 denied quota');
      check(modelsToken(catalog2) === 'grok-build:false' && removedToken(catalog2) === 'grok-mini', `Round 2 catalog must keep the survivor and report the retired id: ${catalog2}`);
      check(/permission=denied\b/.test(quota2), `Round 2 quota must report the denied pool permission: ${quota2}`);
      check(quota2.includes('used=42.5') && quota2.includes('remaining=57.5'), `Round 2 must keep the stand-in numbers on the denied axis: ${quota2}`);
      const view2 = await wait(async () => { const view = await grokView(); return view?.quota?.state === 'denied' ? view : null; }, 'round 2 snapshot');
      check((view2.catalog?.removed_models || []).join(',') === 'grok-mini', `Snapshot must report the retired model: ${JSON.stringify(view2.catalog)}`);
      check(view2.models.map(model => model.model_id).join(',') === 'grok-build', `The surviving model must stay listed: ${JSON.stringify(view2.models)}`);
      check(view2.quota?.state === 'denied' && view2.quota?.buckets?.[0]?.permission === 'denied', `Snapshot must report the denied quota: ${JSON.stringify(view2.quota)}`);
      record('grokReadRound2', { catalogText: catalog2, quotaText: quota2, snapshot: { catalog: view2.catalog, models: view2.models, quota: view2.quota } });
      passed('round 2 reports the retired model while the quota denial stays on its own axis');
      // 3) models=unsupported-catalog + usage=fail-quota：如实 unsupported/failed，保留上一次数字与时间，不给 0。
      await clickRefresh('#3');
      const catalog3 = await wait(() => { const text = textOf(`sub-catalog-${grokProviderId}`); return /catalog_state=unsupported\b/.test(text) ? text : null; }, 'round 3 unsupported catalog');
      const quota3 = await wait(() => { const text = textOf(`sub-quota-${grokProviderId}`); return /quota_state=failed\b/.test(text) ? text : null; }, 'round 3 failed quota');
      check(modelsToken(catalog3) === '', `An unsupported catalog must not list models: ${catalog3}`);
      const readOnlyRowText = grokRowFor().textContent || '';
      check((/历史数据|Historical data/.test(readOnlyRowText)) && readOnlyRowText.includes('2026-01-02T03:04:06Z'), `The retained history must be labelled with its last successful time: ${readOnlyRowText.slice(-600)}`);
      check(quota3.includes('history=true'), `Round 3 must flag the retained history: ${quota3}`);
      check(quota3.includes('used=42.5') && quota3.includes('remaining=57.5') && quota3.includes('balance=12.50'), `Round 3 must retain the last observed numbers instead of zeroing them: ${quota3}`);
      check(quota3.includes('observed_at=2026-01-02T03:04:06Z'), `Round 3 must retain the last observed time: ${quota3}`);
      const view3 = await wait(async () => { const view = await grokView(); return view?.quota?.state === 'failed' ? view : null; }, 'round 3 snapshot');
      check(view3.catalog?.state === 'unsupported' && view3.models.length === 0, `Snapshot must report the unsupported catalog without models: ${JSON.stringify(view3.catalog)} ${JSON.stringify(view3.models)}`);
      check(view3.quota?.history === true && view3.quota?.observed_at === '2026-01-02T03:04:06Z' && (view3.quota?.buckets || []).length === 1, `Snapshot must retain the last quota evidence: ${JSON.stringify(view3.quota)}`);
      record('grokReadRound3', { catalogText: catalog3, quotaText: quota3, snapshot: { catalog: view3.catalog, models: view3.models, quota: view3.quota } });
      passed('round 3 reports unsupported/failed honestly and keeps the last observed numbers and time');
      // 4) 永不伪造：替身没给过的 0/100% 不许出现在任何抓到 token 或人读面上。
      const evidenceText = grokRowFor().querySelector('.provider-subscription-evidence')?.textContent || '';
      const captured = [catalog1, quota1, catalog2, quota2, catalog3, quota3, evidenceText];
      const fabricated = ['remaining=0', 'used=0', 'remaining=100', 'used=100', 'balance=0', '100%']
        .filter(pattern => captured.some(text => text.includes(pattern)));
      check(fabricated.length === 0, `The stand-in never supplied a zero or full value, so none may appear: ${JSON.stringify(fabricated)}`);
      // 5) 只读替身不得解锁登录：已连接时拒绝矩阵优先 already_connected；只要被如实拒绝、
      //    且只读连接与证据不被扰动即可（未连接时的隔离拒绝已在本分支开头断言）。
      await rejectedAny('begin_subscription_login', { providerId: grokProviderId }, ['isolated', 'helper', 'already_connected']);
      const viewAfterSignIn = await grokView();
      check(viewAfterSignIn.state === 'connected' && viewAfterSignIn.identity === 'standin-grok@example.invalid', `A rejected sign-in must not disturb the read-only connection: ${JSON.stringify({ state: viewAfterSignIn.state, identity: viewAfterSignIn.identity })}`);
      record('grokReadFabrication', { captured: captured.map(text => text.slice(0, 400)), fabricated, rejectedSignIn: true });
      passed('no fabricated zero or 100% token appears and the read-only helper never unlocks sign-in');
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
