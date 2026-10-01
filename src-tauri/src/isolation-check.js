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
  const setSelectValue = (element, value) => {
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value').set.call(element, value);
    element.dispatchEvent(new Event('change', { bubbles: true }));
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
  // 登出后证据被整体清空：只接受 unknown/unsupported。denied 仍表示存在额度依据，
  // 不能算作已清空；残留 denied 必须让断言直接失败。
  const clearedQuota = quota => !quota || ['unknown', 'unsupported'].includes(quota.state) === true;
  const quotaNodeText = id => document.querySelector(`[data-testid="sub-quota-${id}"]`)?.textContent?.replace(/\s+/g, ' ').trim() || '';
  const catalogNodeText = id => document.querySelector(`[data-testid="sub-catalog-${id}"]`)?.textContent?.replace(/\s+/g, ' ').trim() || '';
  const tokenOf = (text, name) => { const match = new RegExp(`(?:^|\\s)${name}=([^\\s]*)`).exec(text); return match ? match[1] : null; };
  const bucketSegments = text => text.split(/\bbucket=/).slice(1).map(part => `bucket=${part}`);
  // 契约 D 的窗口格式：label:used=..,remaining=..,minutes=..,resets_at=..；Unknown 占位必须可读。
  const windowEntries = segment => [...segment.matchAll(/([a-z_]+):used=([^,\s]+),remaining=([^,\s]+),minutes=([^,\s]+),resets_at=([^,\s\]]+)/g)]
    .map(match => ({ label: match[1], used: match[2], remaining: match[3], minutes: match[4], resetsAt: match[5] }));
  const catalogModelTokens = text => (tokenOf(text, 'models') || '').split(',').filter(Boolean);
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
      // Bugbot #1：官方入口按服务商类型判定，不是名称或标识字符串。同一页面的 Codex 行
      // （同样处于未读取的 unknown 状态）不得出现 Grok 的入口。
      const codexRow = [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes('codex-subscription') && !item.textContent.includes(grokProviderId)) || null;
      check(codexRow !== null, 'A Codex subscription row must be present to compare the official references');
      check(!/docs\.x\.ai/.test(codexRow.textContent), `The Codex row must not offer Grok references: ${codexRow.textContent.slice(0, 240)}`);
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
      passed('the official references follow the provider kind and never appear on the Codex row');
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
      check(quota1.includes('credits=has:true,unlimited:false,balance:12.50,unit:USD,permission:denied'), `Round 1 credits must keep the raw balance and unit: ${quota1}`);
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
      check(quota3.includes('used=42.5') && quota3.includes('remaining=57.5') && quota3.includes('balance:12.50'), `Round 3 must retain the last observed numbers instead of zeroing them: ${quota3}`);
      check(quota3.includes('observed_at=2026-01-02T03:04:06Z'), `Round 3 must retain the last observed time: ${quota3}`);
      const view3 = await wait(async () => { const view = await grokView(); return view?.quota?.state === 'failed' ? view : null; }, 'round 3 snapshot');
      check(view3.catalog?.state === 'unsupported' && view3.models.length === 0, `Snapshot must report the unsupported catalog without models: ${JSON.stringify(view3.catalog)} ${JSON.stringify(view3.models)}`);
      check(view3.quota?.history === true && view3.quota?.observed_at === '2026-01-02T03:04:06Z' && (view3.quota?.buckets || []).length === 1, `Snapshot must retain the last quota evidence: ${JSON.stringify(view3.quota)}`);
      record('grokReadRound3', { catalogText: catalog3, quotaText: quota3, snapshot: { catalog: view3.catalog, models: view3.models, quota: view3.quota } });
      passed('round 3 reports unsupported/failed honestly and keeps the last observed numbers and time');
      // 4) 永不伪造：替身没给过的 0/100% 不许出现在任何抓到 token 或人读面上。
      const evidenceText = grokRowFor().querySelector('.provider-subscription-evidence')?.textContent || '';
      const captured = [catalog1, quota1, catalog2, quota2, catalog3, quota3, evidenceText];
      const fabricated = ['remaining=0', 'used=0', 'remaining=100', 'used=100', 'balance:0', '100%']
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
  // Issue #15 目录/额度只读验收：先完成一次成功登录，再通过真实界面刷新控件逐个排练目录与额度
  // 场景（一次运行只排练一组队列），断言同时取自界面稳定文本与后端 snapshot，不只看替身日志。
  const catalogLifecycle = async () => {
    const details = { mode: 'catalog', observations: [] };
    report.details = details;
    const record = (label, value) => details.observations.push({ label, value });
    const quotaText = () => quotaNodeText(providerId);
    const catalogText = () => catalogNodeText(providerId);
    const clickRefresh = async () => {
      const button = await wait(() => {
        const row = [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(providerId));
        const candidate = row && [...row.querySelectorAll('.row-actions .icon-action:not(.danger)')]
          .find(item => /refresh|刷新/i.test(`${item.getAttribute('title') || ''} ${item.getAttribute('aria-label') || ''}`));
        return candidate && !candidate.disabled ? candidate : null;
      }, 'subscription refresh control');
      button.click();
    };
    // 点真实控件的刷新按钮；等待「界面文本」与「后端 snapshot」同时满足该场景的判据。
    const refreshUntil = async (label, predicate) => {
      await clickRefresh();
      return await wait(async () => {
        const view = await subscriptionView();
        const dom = { quota: quotaText(), catalog: catalogText() };
        return predicate(view, dom) ? { view, dom } : null;
      }, label);
    };
    await nav(1);
    await wait(() => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(providerId)), 'subscription row');
    check(Boolean(document.querySelector(`[data-testid="sub-quota-${providerId}"]`)), 'The row must expose the stable quota node');
    check(Boolean(document.querySelector(`[data-testid="sub-catalog-${providerId}"]`)), 'The row must expose the stable catalog node');
    const started = await wait(async () => (await subscriptionView()) || null, 'subscription view');
    check(started.state !== 'connected', `Catalog rehearsal needs a disconnected subscription: ${started.state}`);
    // 目录与额度读取只对已核实账号生效：先完成一次成功登录。
    await click(`[data-testid="sub-login-${providerId}"]`);
    const pending = await wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'pending' ? item : null; }, 'pending login');
    const signedIn = await wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'completed' && item.identity ? item : null; }, 'completed login');
    check(signedIn.generation === pending.generation, `A completed sign-in must keep its generation: ${pending.generation} -> ${signedIn.generation}`);
    await waitStatus(text => text.includes(`identity=${signedIn.identity}`));
    record('signedIn', { generation: signedIn.generation, identity: signedIn.identity, state: signedIn.state });
    passed('a successful sign-in precedes the catalog and quota reads');
    // 1) 多桶 + 许可 true：字段映射、view、来源、时间、多桶优先于旧版单桶。
    const multi = await refreshUntil('multi-bucket quota with a discovered catalog', (view, dom) =>
      view?.quota?.buckets?.some(bucket => bucket.limit_id === 'fictional-primary')
      && dom.quota.includes('bucket=fictional-primary') && dom.catalog.includes('codex-fixture-model'));
    const multiQuota = multi.view.quota;
    check(multiQuota.state === 'available', `A multi-bucket allowed read must be available: ${JSON.stringify(multiQuota.state)}`);
    check(multiQuota.view === 'rate_limits_by_limit_id', `Two buckets must use the multi-bucket view: ${multiQuota.view}`);
    check(multiQuota.history === false, `A fresh read must not be marked historical: ${multiQuota.history}`);
    check(/^\d{4}-\d{2}-\d{2}T/.test(multiQuota.observed_at || ''), `A successful read must record an RFC3339 timestamp: ${multiQuota.observed_at}`);
    check(String(multiQuota.source).includes('account/rateLimits/read') && String(multiQuota.source).includes('rateLimitsByLimitId'), `The source must name the RPC and view: ${multiQuota.source}`);
    check(multiQuota.buckets.length === 2, `Both buckets must be exposed: ${multiQuota.buckets.map(bucket => bucket.limit_id)}`);
    check(!multiQuota.buckets.some(bucket => bucket.limit_id === 'fictional-legacy-decoy'), `rateLimitsByLimitId must win over the legacy single snapshot: ${multiQuota.buckets.map(bucket => bucket.limit_id)}`);
    const primary = multiQuota.buckets.find(bucket => bucket.limit_id === 'fictional-primary');
    const primaryWindows = Object.fromEntries((primary?.windows || []).map(window => [window.label, window]));
    check(primary?.permission === 'allowed', `An allowed root permission must allow every bucket: ${JSON.stringify(primary?.permission)}`);
    check(primary?.plan_type === 'fictional-plus' && primary?.name === 'Fictional primary window', `Bucket metadata must be mapped: ${JSON.stringify({ plan: primary?.plan_type, name: primary?.name })}`);
    check(primaryWindows.primary?.used_percent === 42 && primaryWindows.primary?.window_minutes === 300 && primaryWindows.primary?.resets_at === 1767225600, `usedPercent/windowDurationMins/resetsAt must map through: ${JSON.stringify(primaryWindows.primary)}`);
    check(primaryWindows.secondary?.used_percent === 7 && primaryWindows.secondary?.window_minutes === 10080 && primaryWindows.secondary?.resets_at === 1767830400, `The secondary window must map through: ${JSON.stringify(primaryWindows.secondary)}`);
    check(primary?.credits?.has_credits === true && primary?.credits?.unlimited === false && primary?.credits?.balance === '12.5 fictional credits', `Credits must be kept verbatim: ${JSON.stringify(primary?.credits)}`);
    const multiDomBuckets = bucketSegments(multi.dom.quota);
    check(tokenOf(multi.dom.quota, 'quota_state') === 'available' && tokenOf(multi.dom.quota, 'quota_view') === 'rate_limits_by_limit_id', `The DOM must show the multi-bucket view: ${multi.dom.quota}`);
    check(tokenOf(multi.dom.quota, 'permission') === 'allowed' && tokenOf(multi.dom.quota, 'history') === 'false', `The DOM must show the aggregated permission and freshness: ${multi.dom.quota}`);
    check(tokenOf(multi.dom.quota, 'observed_at') === multiQuota.observed_at, `The DOM timestamp must match the snapshot: ${multi.dom.quota}`);
    check(multi.dom.quota.includes(`source=${multiQuota.source}`), `The DOM must show the read source: ${multi.dom.quota}`);
    check(multiDomBuckets.length === 2 && !multi.dom.quota.includes('fictional-legacy-decoy'), `The DOM must show exactly the two buckets: ${multi.dom.quota}`);
    const domPrimaryWindows = Object.fromEntries(windowEntries(multiDomBuckets.find(segment => tokenOf(segment, 'bucket') === 'fictional-primary') || '').map(window => [window.label, window]));
    check(domPrimaryWindows.primary?.used === '42' && domPrimaryWindows.primary?.remaining === '58' && domPrimaryWindows.primary?.minutes === '300' && domPrimaryWindows.primary?.resetsAt === '1767225600', `The DOM must derive remaining = 100 - used and keep the window numbers: ${JSON.stringify(domPrimaryWindows.primary)}`);
    check(domPrimaryWindows.secondary?.remaining === '93' && domPrimaryWindows.secondary?.resetsAt === '1767830400', `The DOM secondary window is wrong: ${JSON.stringify(domPrimaryWindows.secondary)}`);
    // 目录：发现 ≠ 资格，eligible 一律 false。
    check(multi.view.catalog?.state === 'available', `A successful catalog read must be available: ${JSON.stringify(multi.view.catalog)}`);
    check(String(multi.view.catalog?.source).includes('model/list'), `The catalog source must name the RPC: ${multi.view.catalog?.source}`);
    check((multi.view.models || []).length === 2 && multi.view.models.every(model => model.eligible === false), `Discovered models must stay ineligible: ${JSON.stringify(multi.view.models)}`);
    check(multi.view.models.some(model => model.model_id === 'codex-fixture-model') && multi.view.models.some(model => model.model_id === 'codex-fixture-legacy-id'), `id/model fallback must be mapped: ${JSON.stringify(multi.view.models)}`);
    const multiDomModels = catalogModelTokens(multi.dom.catalog);
    check(multiDomModels.length === 2 && multiDomModels.every(entry => entry.endsWith(':false')), `The DOM catalog must list both models as ineligible: ${multi.dom.catalog}`);
    check(tokenOf(multi.dom.catalog, 'catalog_state') === 'available' && tokenOf(multi.dom.catalog, 'observed_at') === multi.view.catalog.observed_at, `The DOM catalog state and time must match: ${multi.dom.catalog}`);
    record('multi', { quota: multiQuota, catalog: multi.view.catalog, models: multi.view.models, domQuota: multi.dom.quota.slice(0, 800), domCatalog: multi.dom.catalog.slice(0, 400) });
    passed('multi-bucket quota maps usedPercent/windowDurationMins/resetsAt into the DOM and the snapshot, legacy single is not used');
    passed('discovered catalog models stay eligible=false in the DOM and the snapshot');
    // 生成准入仍被统一拒绝：即使额度可读、模型已发现，也不得派发任何生成请求。
    // 不能用「samples 计数变大」判断：summary 只保留最近 5 个样本，计数器会饱和。
    // 判据是这一批 3 次探测全部完成、任务结束且带拒绝原因（start 会先把 job 重置为 running）。
    await invoke('start_model_speed_tests', { ids: ['codex-subscription-model'] });
    const deniedWithCatalog = await wait(async () => {
      const value = await invoke('get_model_performance');
      return !value.job.running && value.job.completed === 3 && value.job.error ? value : null;
    }, 'denied speed test with a catalog');
    check(/eligible/i.test(deniedWithCatalog.job.error || ''), `Generation must be denied as ineligible even with an available quota read: ${JSON.stringify(deniedWithCatalog.job)}`);
    record('deniedWithCatalog', { error: deniedWithCatalog.job.error });
    passed('generation stays denied with a discovered catalog and an available quota read');
    // 2) 旧版单桶（仅 rateLimits，根层许可）+ 旧目录形状 `models`：整体替换为本次权威结果。
    const single = await refreshUntil('legacy single-bucket quota and models-array catalog', (view, dom) =>
      view?.quota?.view === 'rate_limits' && dom.quota.includes('bucket=fictional-single')
      && dom.catalog.includes('codex-fixture-legacy-shape'));
    check(single.view.quota.state === 'available' && single.view.quota.view === 'rate_limits', `A legacy single snapshot must be available in the legacy view: ${JSON.stringify({ state: single.view.quota.state, view: single.view.quota.view })}`);
    check(single.view.quota.buckets.length === 1 && single.view.quota.buckets[0].limit_id === 'fictional-single', `Exactly the legacy bucket must be used: ${JSON.stringify(single.view.quota.buckets.map(bucket => bucket.limit_id))}`);
    const singleWindow = single.view.quota.buckets[0].windows[0];
    check(singleWindow?.used_percent === 33 && singleWindow?.window_minutes === 120 && singleWindow?.resets_at === 1767232800, `The legacy window must map through: ${JSON.stringify(singleWindow)}`);
    check(single.view.quota.buckets[0].permission === 'allowed', `A root-level ordinaryUsageAllowed must permit the bucket: ${single.view.quota.buckets[0].permission}`);
    check(tokenOf(single.dom.quota, 'quota_view') === 'rate_limits', `The DOM must show the legacy view: ${single.dom.quota}`);
    const singleDomWindow = windowEntries(bucketSegments(single.dom.quota)[0])[0];
    check(singleDomWindow?.used === '33' && singleDomWindow?.remaining === '67' && singleDomWindow?.minutes === '120' && singleDomWindow?.resetsAt === '1767232800', `The DOM legacy window is wrong: ${JSON.stringify(singleDomWindow)}`);
    // 目录以本次权威结果整体替换：旧列表里的多桶模型必须被撤销。
    check((single.view.models || []).length === 1 && single.view.models[0].model_id === 'codex-fixture-legacy-shape', `A successful read must replace the catalog wholesale: ${JSON.stringify(single.view.models)}`);
    check(single.view.models.every(model => model.eligible === false), `The legacy-shaped model must stay ineligible: ${JSON.stringify(single.view.models)}`);
    check(tokenOf(single.dom.catalog, 'catalog_state') === 'available' && catalogModelTokens(single.dom.catalog).join(',') === 'codex-fixture-legacy-shape:false', `The DOM catalog must be replaced wholesale: ${single.dom.catalog}`);
    // #16 在 models= 之后固定增加 removed=（上一次已核实目录里已消失的模型）：旧模型只允许出现在
    // removed= 里，绝不能再出现在 models= 列表里；列表本身仍以本次权威结果整体替换。
    check(!catalogModelTokens(single.dom.catalog).some(token => token.startsWith('codex-fixture-model:')), `A model absent from the authoritative result must disappear from the model list: ${single.dom.catalog}`);
    check((tokenOf(single.dom.catalog, 'removed') || '').includes('codex-fixture-model'), `The retired model must be reported as removed instead of silently kept: ${single.dom.catalog}`);
    record('single', { quota: single.view.quota, catalog: single.view.catalog, models: single.view.models, domQuota: single.dom.quota.slice(0, 600), domCatalog: single.dom.catalog.slice(0, 400) });
    passed('the legacy single snapshot uses the legacy view and the DOM derives remaining there too');
    passed('a successful catalog read replaces the previous list wholesale');
    // 3) 缺字段：resetsAt 与 credits 必须进 missing_fields，且不得伪造数值。
    const missing = await refreshUntil('missing resetsAt and credits plus a skipped catalog entry', (view, dom) =>
      view?.quota?.buckets?.some(bucket => bucket.limit_id === 'fictional-missing')
      && dom.quota.includes('bucket=fictional-missing') && dom.catalog.includes('model.list[0].id'));
    const missingBucket = missing.view.quota.buckets.find(bucket => bucket.limit_id === 'fictional-missing');
    const missingWindow = missingBucket.windows[0];
    check(missingWindow.used_percent === 55 && missingWindow.window_minutes === 300, `Present fields must still map when others are missing: ${JSON.stringify(missingWindow)}`);
    check(missingWindow.resets_at == null, `A missing resetsAt must stay absent: ${missingWindow.resets_at}`);
    check((missingWindow.missing_fields || []).some(field => /resetsAt/i.test(field)), `The window must record the missing resetsAt: ${JSON.stringify(missingWindow.missing_fields)}`);
    check((missingBucket.missing_fields || []).some(field => /credits/i.test(field)), `The bucket must record the missing credits: ${JSON.stringify(missingBucket.missing_fields)}`);
    check((missingBucket.credits ? (missingBucket.credits.missing_fields || []) : (missingBucket.missing_fields || [])).length > 0, `Missing credits fields must be recorded individually: ${JSON.stringify(missingBucket.credits)}`);
    const missingDom = bucketSegments(missing.dom.quota).find(segment => tokenOf(segment, 'bucket') === 'fictional-missing') || '';
    check(/missing=[^\s]*resetsAt/i.test(missingDom), `The DOM must report the missing resetsAt: ${missingDom}`);
    check(/missing=[^\s]*credits|credits=[^\s]*Unknown/i.test(missingDom), `The DOM must report the missing credits fields: ${missingDom}`);
    check(!/resets_at=0\b/.test(missingDom), `A missing resetsAt must not be rendered as zero: ${missingDom}`);
    check(!/remaining=0\b/.test(missingDom), `A missing window field must not fabricate remaining=0: ${missingDom}`);
    // 条目没有可用标识 → 计入 missing_fields 并跳过该条，只保留合法条目。
    check((missing.view.models || []).length === 1 && missing.view.models[0].model_id === 'codex-fixture-model', `A catalog entry without an identifier must be skipped: ${JSON.stringify(missing.view.models)}`);
    check((missing.view.catalog?.missing_fields || []).some(field => field.includes('model.list[0].id')), `The skipped entry must be recorded as model.list[i].id: ${JSON.stringify(missing.view.catalog?.missing_fields)}`);
    check(tokenOf(missing.dom.catalog, 'missing')?.includes('model.list[0].id') === true, `The DOM catalog must show the skipped entry: ${missing.dom.catalog}`);
    record('missing', { window: missingWindow, bucketMissing: missingBucket.missing_fields, catalog: missing.view.catalog, models: missing.view.models, dom: missingDom.slice(0, 500), domCatalog: missing.dom.catalog.slice(0, 400) });
    passed('missing resetsAt/credits fields are recorded, never fabricated, and a catalog entry without an identifier is skipped');
    // 4) 越界/类型不符：值为 None 并原样记入 invalid_fields，绝不截断或改写成合法值。
    const invalid = await refreshUntil('out-of-range quota fields', (view, dom) =>
      view?.quota?.buckets?.some(bucket => bucket.limit_id === 'fictional-invalid') && dom.quota.includes('bucket=fictional-invalid'));
    const invalidBucket = invalid.view.quota.buckets.find(bucket => bucket.limit_id === 'fictional-invalid');
    const invalidPrimary = invalidBucket.windows.find(window => window.label === 'primary');
    const invalidSecondary = invalidBucket.windows.find(window => window.label === 'secondary');
    check(invalidPrimary.used_percent == null && invalidPrimary.resets_at == null && invalidPrimary.window_minutes == null, `Out-of-range numbers must not be adopted: ${JSON.stringify(invalidPrimary)}`);
    check(invalidSecondary.used_percent === 50 && invalidSecondary.resets_at === 1767232800, `A valid sibling window must be unaffected: ${JSON.stringify(invalidSecondary)}`);
    const invalidText = (invalidPrimary.invalid_fields || []).join(',');
    check(invalidText.includes('142') && /resetsAt|resets_at/i.test(invalidText) && /windowDurationMins|window_minutes/i.test(invalidText), `The invalid fields must keep the raw text: ${JSON.stringify(invalidPrimary.invalid_fields)}`);
    check((invalidPrimary.invalid_fields || []).length >= 3, `Every out-of-range/type-mismatched field must be recorded: ${JSON.stringify(invalidPrimary.invalid_fields)}`);
    const invalidDom = bucketSegments(invalid.dom.quota).find(segment => tokenOf(segment, 'bucket') === 'fictional-invalid') || '';
    check(!/used=142\b/.test(invalidDom), `The out-of-range usedPercent must not appear as a value: ${invalidDom}`);
    check(!/resets_at=0\b/.test(invalidDom), `The out-of-range resetsAt must not appear as a value: ${invalidDom}`);
    check(!/minutes=-1\b/.test(invalidDom), `The out-of-range windowDurationMins must not appear as a value: ${invalidDom}`);
    check(/used=Unknown/.test(invalidDom) && /remaining=Unknown/.test(invalidDom) && !/remaining=0\b/.test(invalidDom), `Invalid numbers must read Unknown, never zero: ${invalidDom}`);
    check(/invalid=[^\s]*142/.test(invalidDom), `The DOM must record the invalid field with its raw text: ${invalidDom}`);
    check(/used=50\b/.test(invalidDom) && /resets_at=1767232800\b/.test(invalidDom), `The valid sibling window must still render as numbers: ${invalidDom}`);
    record('invalid', { window: invalidPrimary, bucketInvalid: invalidBucket.invalid_fields, dom: invalidDom.slice(0, 600) });
    passed('out-of-range usedPercent=142/resetsAt=0/windowDurationMins=-1 are recorded as invalid, stay None and never render as values');
    // 5) 根层 ordinaryUsageAllowed=false → permission=denied。
    const denied = await refreshUntil('denied ordinary usage', (view, dom) =>
      view?.quota?.state === 'denied' && dom.quota.includes('bucket=fictional-denied'));
    check(denied.view.quota.view === 'rate_limits_by_limit_id' && denied.view.quota.buckets.every(bucket => bucket.permission === 'denied'), `A denied permission must deny every bucket: ${JSON.stringify(denied.view.quota.buckets.map(bucket => bucket.permission))}`);
    check(denied.view.quota.buckets[0].windows[0].used_percent === 12, `A denied bucket must still expose its window numbers: ${JSON.stringify(denied.view.quota.buckets[0].windows[0])}`);
    check(tokenOf(denied.dom.quota, 'quota_state') === 'denied' && tokenOf(denied.dom.quota, 'permission') === 'denied', `The DOM must show the aggregate denied state: ${denied.dom.quota}`);
    // `view.denial` 只承载连接级原因（not_connected/evidence_missing），额度拒绝码走 admit_model；
    // 本票 eligible 恒 false 时生成先停在 model_not_eligible，因此 quota_denied 端到端不可达，
    // 由 subscription.rs 单测覆盖；隔离验收只断言已展示的 denied 状态与本地化文案。
    check(denied.view.denial == null, `A connected, evidenced subscription must not carry a connection-level denial: ${JSON.stringify(denied.view.denial)}`);
    const deniedLabel = await wait(() => /额度访问被拒绝|[Qq]uota access denied/.test(rowText()) ? rowText() : null, 'denied quota label');
    check(deniedLabel, 'A denied permission must be labelled as denied in the row');
    record('denied', { state: denied.view.quota.state, permission: denied.view.quota.buckets[0].permission, denial: denied.view.denial, label: deniedLabel.slice(-200), dom: denied.dom.quota.slice(0, 500) });
    passed('ordinaryUsageAllowed=false maps to permission=denied and the denied quota label');
    // 6) 桶内显式 false 不得被根层 true 覆盖（fail-closed 防御）。
    const bucketDenied = await refreshUntil('bucket-level false is not overridden by root true', (view, dom) =>
      view?.quota?.state === 'denied' && dom.quota.includes('bucket=fictional-bucket-denied'));
    check(bucketDenied.view.quota.buckets.every(bucket => bucket.permission === 'denied'), `A bucket-level explicit false must deny, not be overridden by a root true: ${JSON.stringify(bucketDenied.view.quota.buckets.map(bucket => bucket.permission))}`);
    check(bucketDenied.view.quota.buckets[0].windows[0].used_percent === 15, `A bucket-level denial must still expose its window numbers: ${JSON.stringify(bucketDenied.view.quota.buckets[0].windows[0])}`);
    check(tokenOf(bucketDenied.dom.quota, 'quota_state') === 'denied' && tokenOf(bucketDenied.dom.quota, 'permission') === 'denied', `The DOM must show the bucket-level denial: ${bucketDenied.dom.quota}`);
    record('bucketDenied', { state: bucketDenied.view.quota.state, permission: bucketDenied.view.quota.buckets[0].permission, dom: bucketDenied.dom.quota.slice(0, 500) });
    passed('a bucket-level ordinaryUsageAllowed=false is not overridden by a root true (fail-closed)');
    // 7) 根层 ordinaryUsageAllowed 缺失与显式 null → unknown（不是 denied，也不是 0）。
    const noPermission = await refreshUntil('absent ordinaryUsageAllowed', (view, dom) =>
      view?.quota?.buckets?.some(bucket => bucket.limit_id === 'fictional-noperm') && dom.quota.includes('bucket=fictional-noperm'));
    check(noPermission.view.quota.buckets.every(bucket => bucket.permission === 'unknown'), `An absent permission must be unknown per bucket: ${JSON.stringify(noPermission.view.quota.buckets.map(bucket => bucket.permission))}`);
    check(noPermission.view.quota.state === 'unknown', `An absent permission must not be available: ${noPermission.view.quota.state}`);
    check(tokenOf(noPermission.dom.quota, 'permission') === 'unknown' && tokenOf(noPermission.dom.quota, 'quota_state') === 'unknown', `The DOM must show unknown, not denied: ${noPermission.dom.quota}`);
    check(!/permission=denied/.test(noPermission.dom.quota), `Unknown permission must not be rendered as denied: ${noPermission.dom.quota}`);
    check(/used=20\b/.test(bucketSegments(noPermission.dom.quota)[0] || ''), `The readable window number must still be shown: ${noPermission.dom.quota}`);
    const noPermissionMissing = [...(noPermission.view.quota.missing_fields || []), ...noPermission.view.quota.buckets.flatMap(bucket => bucket.missing_fields || [])].join(',');
    check(/ordinaryUsageAllowed/i.test(noPermissionMissing), `The missing permission must be recorded in missing_fields: ${noPermissionMissing}`);
    record('noPermission', { state: noPermission.view.quota.state, permission: noPermission.view.quota.buckets[0].permission, missing: noPermissionMissing, dom: noPermission.dom.quota.slice(0, 500) });
    passed('an absent ordinaryUsageAllowed is unknown, recorded in missing_fields and never rendered as denied or zero');
    const nullPermission = await refreshUntil('null ordinaryUsageAllowed', (view, dom) =>
      view?.quota?.buckets?.some(bucket => bucket.limit_id === 'fictional-nullperm') && dom.quota.includes('bucket=fictional-nullperm'));
    check(nullPermission.view.quota.buckets.every(bucket => bucket.permission === 'unknown') && nullPermission.view.quota.state === 'unknown', `A null permission must stay unknown: ${JSON.stringify({ state: nullPermission.view.quota.state, permissions: nullPermission.view.quota.buckets.map(bucket => bucket.permission) })}`);
    check(tokenOf(nullPermission.dom.quota, 'permission') === 'unknown', `The DOM must show unknown for a null permission: ${nullPermission.dom.quota}`);
    record('nullPermission', { state: nullPermission.view.quota.state, dom: nullPermission.dom.quota.slice(0, 400) });
    passed('a null ordinaryUsageAllowed is unknown, not denied');
    // 8) 额度读取失败：state=failed、history=true、observed_at 保持上一次成功值、桶数字不被清零。
    //    先等过一秒，确保「时间未被更新」这条断言不会被同一秒内的等值掩盖。
    const lastGoodObservedAt = nullPermission.view.quota.observed_at;
    const lastGoodBuckets = JSON.parse(JSON.stringify(nullPermission.view.quota.buckets));
    await new Promise(r => setTimeout(r, 1100));
    const failQuota = await refreshUntil('quota read failure retains history', (view, dom) =>
      view?.quota?.state === 'failed' && dom.quota.includes('quota_state=failed'));
    const failedQuota = failQuota.view.quota;
    check(failedQuota.history === true, `A failed quota read must be marked historical: ${JSON.stringify(failedQuota)}`);
    check(failedQuota.observed_at === lastGoodObservedAt, `A failed quota read must keep the last successful observed_at: ${lastGoodObservedAt} -> ${failedQuota.observed_at}`);
    check(JSON.stringify(failedQuota.buckets) === JSON.stringify(lastGoodBuckets), `A failed quota read must retain the previous buckets: ${JSON.stringify(failedQuota.buckets)}`);
    check(tokenOf(failQuota.dom.quota, 'history') === 'true' && tokenOf(failQuota.dom.quota, 'quota_state') === 'failed', `The DOM must show the historical failure: ${failQuota.dom.quota}`);
    check(failQuota.dom.quota.includes(`observed_at=${lastGoodObservedAt}`), `The DOM must keep the last successful timestamp: ${failQuota.dom.quota}`);
    const retainedDom = bucketSegments(failQuota.dom.quota).find(segment => tokenOf(segment, 'bucket') === 'fictional-nullperm') || '';
    check(/used=21\b/.test(retainedDom) && !/used=0\b/.test(retainedDom), `The retained bucket numbers must not be zeroed: ${retainedDom}`);
    const historyLabelled = await wait(() => /历史数据|[Hh]istorical/.test(rowText()) ? true : null, 'historical data label');
    check(historyLabelled, `A historical quota must be labelled as historical in the row`);
    record('failQuota', { state: failedQuota.state, history: failedQuota.history, observedAt: failedQuota.observed_at, previousObservedAt: lastGoodObservedAt, buckets: failedQuota.buckets, dom: failQuota.dom.quota.slice(0, 700) });
    passed('a failed quota read keeps the last numbers and timestamp, marks the quota historical and never zeroes the buckets');
    // 9) 目录读取失败：catalog.state=stale，已核实目录原样保留，observed_at 不更新。
    const lastGoodCatalogObservedAt = failQuota.view.catalog.observed_at;
    const lastGoodModelIds = (failQuota.view.models || []).map(model => model.model_id).sort().join(',');
    check(lastGoodModelIds.length > 0, `The stale rehearsal needs a previously verified catalog: ${lastGoodModelIds}`);
    await new Promise(r => setTimeout(r, 1100));
    const failCatalog = await refreshUntil('catalog read failure keeps the verified directory', (view, dom) =>
      view?.catalog?.state === 'stale' && dom.catalog.includes('catalog_state=stale'));
    check(failCatalog.view.catalog.state === 'stale', `A failed catalog read after a verified one must be stale: ${JSON.stringify(failCatalog.view.catalog)}`);
    check(failCatalog.view.catalog.observed_at === lastGoodCatalogObservedAt, `A failed catalog read must keep the last successful observed_at: ${lastGoodCatalogObservedAt} -> ${failCatalog.view.catalog.observed_at}`);
    check((failCatalog.view.models || []).map(model => model.model_id).sort().join(',') === lastGoodModelIds, `A stale catalog must retain the verified models: ${JSON.stringify(failCatalog.view.models)}`);
    check(tokenOf(failCatalog.dom.catalog, 'catalog_state') === 'stale' && failCatalog.dom.catalog.includes('codex-fixture-model'), `The DOM must keep listing the stale models: ${failCatalog.dom.catalog}`);
    check(tokenOf(failCatalog.dom.catalog, 'observed_at') === lastGoodCatalogObservedAt, `The DOM must keep the last successful catalog time: ${failCatalog.dom.catalog}`);
    check(failCatalog.view.quota.state === 'failed' && failCatalog.view.quota.history === true, `The independent quota failure must survive a catalog refresh: ${JSON.stringify({ state: failCatalog.view.quota.state, history: failCatalog.view.quota.history })}`);
    record('failCatalog', { catalog: failCatalog.view.catalog, models: failCatalog.view.models, previousObservedAt: lastGoodCatalogObservedAt, dom: failCatalog.dom.catalog.slice(0, 500) });
    passed('a failed catalog read keeps the verified directory, marks it stale and keeps the last successful timestamp');
    passed('a catalog failure does not rewrite the independently failed quota evidence');
    // Helper 自行改变状态：直接刷新，不先调用应用内 logout。
    const incomplete = await refreshUntil('incomplete identity suspends current authorization', (view, dom) =>
      view?.state === 'not_connected' && dom.quota.includes('history=true') && dom.catalog.includes('catalog_state=stale'));
    check(incomplete.view.identity === failCatalog.view.identity, 'Keep the last verified owner for history');
    check(incomplete.view.catalog.observed_at === failCatalog.view.catalog.observed_at, 'Incomplete identity must keep the catalog time');
    check(incomplete.view.quota.observed_at === failCatalog.view.quota.observed_at, 'Incomplete identity must keep the quota time');
    check(JSON.stringify(incomplete.view.models) === JSON.stringify(failCatalog.view.models), 'Incomplete identity must retain only the old models');
    check(JSON.stringify(incomplete.view.quota.buckets) === JSON.stringify(failCatalog.view.quota.buckets), 'Incomplete identity must retain only the old buckets');
    check(incomplete.view.denial?.code === 'not_connected', 'Incomplete identity must refuse admission');
    await waitStatus(text => text.includes('state=not_connected'));
    passed('incomplete identity suspends the connection and displays stale historical evidence with original timestamps');
    const recovered = await refreshUntil('same account recovery with failed evidence reads', (view, dom) =>
      view?.state === 'connected' && dom.catalog.includes('catalog_state=stale') && dom.quota.includes('quota_state=failed'));
    check(recovered.view.identity === failCatalog.view.identity, 'Recovery must confirm the original account');
    check(recovered.view.catalog.observed_at === failCatalog.view.catalog.observed_at, 'Recovery failure must keep historical catalog time');
    check(recovered.view.quota.observed_at === failCatalog.view.quota.observed_at && recovered.view.quota.history, 'Recovery failure must keep historical quota time');
    passed('same-account recovery retains the stale catalog and failed historical quota');
    // 10) 登出：目录与额度一并清空，生成仍被统一准入拒绝。
    const connectedGeneration = recovered.view.generation;
    await click(`[data-testid="sub-logout-${providerId}"]`);
    const after = await wait(async () => { const item = await subscriptionView(); return item?.logout && !item.identity ? item : null; }, 'logout outcome');
    check(after.generation > connectedGeneration, `Sign-out must advance the generation: ${connectedGeneration} -> ${after.generation}`);
    check((after.models || []).length === 0, `Sign-out must clear the discovered catalog: ${JSON.stringify(after.models)}`);
    check(after.catalog?.state === 'unknown' && !after.catalog?.source && !after.catalog?.observed_at, `Sign-out must clear the catalog evidence: ${JSON.stringify(after.catalog)}`);
    check((after.quota?.buckets || []).length === 0 && after.quota?.state === 'unknown' && after.quota?.history !== true && !after.quota?.observed_at, `Sign-out must clear the quota evidence: ${JSON.stringify(after.quota)}`);
    await wait(() => {
      const quota = quotaText(); const catalog = catalogText();
      return tokenOf(quota, 'quota_state') === 'unknown' && !quota.includes('bucket=')
        && tokenOf(catalog, 'catalog_state') === 'unknown' && !/models=[^\s]/.test(catalog) ? true : null;
    }, 'cleared catalog and quota nodes');
    check(!/codex-fixture/.test(catalogText()), `The cleared DOM node must not keep stale model ids: ${catalogText()}`);
    record('afterLogout', { generation: after.generation, identity: after.identity ?? null, catalog: after.catalog, models: after.models, quota: after.quota, domQuota: quotaText().slice(0, 400), domCatalog: catalogText().slice(0, 300) });
    passed('sign-out clears the catalog and quota evidence together');
    await invoke('start_model_speed_tests', { ids: ['codex-subscription-model'] });
    const deniedAfterLogout = await wait(async () => {
      const value = await invoke('get_model_performance');
      return !value.job.running && value.job.completed === 3 && value.job.error ? value : null;
    }, 'denied speed test after sign-out');
    check(/not connected/i.test(deniedAfterLogout.job.error || ''), `A signed-out subscription must stay denied with its reason: ${deniedAfterLogout.job.error}`);
    record('deniedAfterLogout', { error: deniedAfterLogout.job.error });
    passed('generation stays denied after the catalog and quota evidence is cleared');
    // 重新连接 A，留下有效证据，再排练 helper 自行退出（此前应用内退出不参与这次故障路径）。
    await click(`[data-testid="sub-login-${providerId}"]`);
    await wait(async () => { const item = await subscriptionView(); return item?.login?.stage === 'completed' && item?.state === 'connected' ? item : null; }, 'second account verification');
    const helperConnected = await refreshUntil('connected A before helper disconnect', (view, dom) =>
      view?.state === 'connected' && dom.catalog.includes('catalog_state=available') && dom.quota.includes('quota_state=available'));
    const helperOut = await refreshUntil('helper disconnect without app logout', (view, dom) =>
      view?.state === 'not_connected' && !view.identity && dom.catalog.includes('catalog_state=unknown') && !dom.quota.includes('bucket='));
    check(helperOut.view.generation > helperConnected.view.generation, 'Helper disconnect must invalidate the generation');
    check(!helperOut.view.models.length && !helperOut.view.quota.buckets.length, 'Helper disconnect must clear active evidence');
    check(helperOut.view.denial?.code === 'not_connected', 'Helper disconnect must refuse admission');
    await invoke('start_model_speed_tests', { ids: ['codex-subscription-model'] });
    const helperDenied = await wait(async () => {
      const value = await invoke('get_model_performance');
      return !value.job.running && value.job.completed === 3 && value.job.error ? value : null;
    }, 'helper disconnect admission refusal');
    check(/not connected/i.test(helperDenied.job.error), `Helper disconnect must refuse generation: ${helperDenied.job.error}`);
    record('helperDisconnect', { generation: helperOut.view.generation, denial: helperOut.view.denial, error: helperDenied.job.error });
    passed('helper disconnect clears evidence and refuses generation without app logout');
    // 全部故障断言完成后才调用退出，清理本次替身生成的专用授权文件。
    await invoke('logout_subscription', { providerId });

  };

  // #17 受控目录全过程：替身只把「上游目录与额度读取」换成脚本可控文件（--autojev-catalog-fixture），
  // 登录/退出、连接世代、准入、派发与快照仍走生产代码。步骤顺序与 scripts/check-isolated-desktop.mjs
  // 的目录读取队列一一对应：发现 → 未选 → 勾选 → 取消选择 → 停用 → 同账号失败 → 权威移除 → 换号 → 删行重建。
  const catalogModelId = 'codex-catalog-alpha';
  const catalogPublicId = `codex-subscription/${catalogModelId}`;
  const modelSelectionLifecycle = async () => {
    const details = { mode: 'model-selection', observations: [] };
    report.details = details;
    const record = (label, value) => details.observations.push({ label, value });
    const snapshot = () => invoke('get_snapshot');
    const view = async () => (await snapshot()).subscriptions.find(item => item.provider_id === providerId);
    // 目录视图按模型行组装；目录项缺失时（例如刚删掉模型行）等待下一次核对重建。
    const entry = async () => {
      const found = await wait(async () => {
        const item = await view();
        return item?.catalog_entries?.find(row => row.model_id === catalogModelId) ?? null;
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
    check((start.catalog_entries || []).every(item => item.model_id !== catalogModelId), `The controlled model must not exist before discovery: ${JSON.stringify(start.catalog_entries)}`);
    record('start', { state: start.state, generation: start.generation, identity: start.identity ?? null, catalog: start.catalog_entries });
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
    const subscriptionRow = await wait(() => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(providerId)), 'subscription provider row before catalog refresh');
    (await wait(() => subscriptionRow.querySelector(`[data-testid="sub-refresh-${providerId}"]`), 'subscription catalog refresh action')).click();
    const refreshed = await wait(async () => {
      const next = await snapshot();
      return next.subscriptions.find(item => item.provider_id === providerId)?.catalog_entries?.some(item => item.model_id === catalogModelId) ? next : null;
    }, 'UI-refreshed subscription catalog');
    const discovered = await entry();
    check(discovered.availability === 'available', `A successful directory read must confirm the entry: ${JSON.stringify(discovered)}`);
    check(discovered.eligibility === 'eligible', `A confirmed entry must be eligible: ${JSON.stringify(discovered)}`);
    check(discovered.selected === false && discovered.disabled === false, `A newly discovered model must start unselected and enabled: ${JSON.stringify(discovered)}`);
    const discoveredRow = await row();
    check(Boolean(discoveredRow), 'A discovered model must be materialized as a model row');
    check(discoveredRow.id === discovered.internal_id, `The internal ID must be the model row ID: ${JSON.stringify({ row: discoveredRow.id, entry: discovered.internal_id })}`);
    check(discoveredRow.selected === false && discoveredRow.enabled === true, `The materialized row must start unselected and enabled: ${JSON.stringify(discoveredRow)}`);
    // Provider list and edit dialog must both expose a real test command whose exact target is selected
    // from this verified directory. The native fixture supplies a fictional helper and account; no live
    // provider, OAuth session, subscription allowance, or model endpoint is contacted.
    await nav(1);
    const providerRow = await wait(() => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(providerId)), 'provider row for subscription test');
    (await wait(() => providerRow.querySelector(`[data-testid="provider-configure-${providerId}"]`), 'configure subscription provider')).click();
    const providerDialog = await wait(() => document.querySelector('.provider-dialog'), 'subscription provider dialog');
    const providerTarget = await wait(() => providerDialog.querySelector('[data-testid="provider-subscription-test-model"]'), 'subscription test target selector');
    check([...providerTarget.options].some(option => option.value === catalogModelId), `The provider dialog must offer the discovered target: ${JSON.stringify([...providerTarget.options].map(option => option.value))}`);
    check(/may consume subscription allowance|可能消耗订阅额度/i.test(providerDialog.textContent), `The provider dialog must warn about subscription allowance: ${providerDialog.textContent}`);
    setSelectValue(providerTarget, catalogModelId);
    await click('[data-testid="provider-subscription-test-action"]');
    const providerTestResult = await wait(() => providerDialog.querySelector('.provider-test-result')?.textContent?.trim(), 'provider dialog test result');
    check(providerTestResult.length > 0, 'The provider dialog test must return an explicit result');
    await click('.provider-dialog-actions button[type="submit"]');
    await wait(() => !document.querySelector('.provider-dialog'), 'saved subscription provider test target');
    const savedProvider = (await snapshot()).providers.find(item => item.id === providerId);
    check(savedProvider?.test_model === catalogModelId, `Saving must keep the selected directory target: ${JSON.stringify(savedProvider)}`);
    const savedProviderRow = await wait(() => [...document.querySelectorAll('tbody tr')].find(item => item.textContent.includes(providerId)), 'saved provider row');
    const providerRowTest = await wait(() => savedProviderRow.querySelector(`[data-testid="provider-test-${providerId}"]`), 'provider row test action');
    check(!providerRowTest.disabled, 'The saved provider row test action must be enabled for its selected target');
    const previousToast = document.querySelector('.toast')?.textContent?.trim() || '';
    providerRowTest.click();
    const listTestResult = await wait(() => {
      const message = document.querySelector('.toast')?.textContent?.trim() || '';
      return message && message !== previousToast ? message : null;
    }, 'provider row test result');
    check(listTestResult.length > 0, 'The provider row test must invoke the saved test target and report its result');
    await click(`[data-testid="provider-configure-${providerId}"]`);
    const changedProviderDialog = await wait(() => document.querySelector('.provider-dialog'), 'provider dialog for subscription kind switch');
    (await wait(() => changedProviderDialog.querySelector('.search-select-trigger'), 'provider type picker')).click();
    const typeSearch = await wait(() => changedProviderDialog.querySelector('[role="combobox"]'), 'provider type search');
    setValue(typeSearch, 'grok-subscription');
    const grokOption = await wait(() => [...changedProviderDialog.querySelectorAll('[role="option"]')].find(option => /grok subscription|grok 订阅/i.test(option.textContent || '')), 'Grok subscription provider type');
    grokOption.click();
    const kindChangeNote = await wait(() => {
      const text = changedProviderDialog.querySelector('[data-testid="provider-kind-change-warning"]')?.textContent?.replace(/\s+/g, ' ').trim() || '';
      return /save.*refresh.*catalog|先保存.*刷新.*目录/i.test(text) ? text : null;
    }, 'save-and-refresh guidance after provider type change');
    const staleTargetPicker = changedProviderDialog.querySelector('[data-testid="provider-subscription-test-model"]');
    check(staleTargetPicker?.disabled === true, 'A directory from the previously saved subscription type must be unavailable until refresh');
    check(![...(staleTargetPicker?.options || [])].some(option => option.value === catalogModelId), 'The old provider directory target must not be offered after switching subscription type');
    check(changedProviderDialog.querySelector('[data-testid="provider-subscription-test-action"]')?.disabled === true, 'Testing a new subscription type must wait until its own catalog is refreshed');
    passed('switching a saved subscription provider type blocks its stale directory and test action');
    (await wait(() => changedProviderDialog.querySelector('.provider-dialog-header button.icon-action'), 'provider dialog close action')).click();
    await wait(() => !document.querySelector('.provider-dialog'), 'close unsaved provider kind change');
    passed('subscription provider directory target is saved and tested from both the dialog and provider row');
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
    await click(`[data-testid="sub-select-${providerId}-${discovered.internal_id}"]`);
    const selected = await wait(async () => { const current = await entry(); return current.selected ? current : null; }, 'catalog selection persisted');
    check(selected.selected === true, `Selecting must reach the user configuration: ${JSON.stringify(selected)}`);
    const selectedList = await publicCatalog();
    check(selectedList.ids.includes(catalogPublicId), `A selected model must appear in the public catalog: ${JSON.stringify(selectedList)}`);
    await nav(2);
    const selectedModelRow = await wait(() => [...document.querySelectorAll('.models-table tbody tr')].find(item => item.textContent.includes(catalogModelId)), 'selected subscription model row');
    (await wait(() => selectedModelRow.querySelector(`[data-testid="model-configure-${selected.internal_id}"]`), 'configure selected subscription model')).click();
    const modelDialog = await wait(() => document.querySelector('.provider-dialog'), 'subscription model dialog');
    const protocolNote = await wait(() => modelDialog.querySelector('[data-testid="subscription-test-protocol-note"]')?.textContent?.trim(), 'subscription Chat-only test note');
    check(/locked to Chat Completions text|固定使用 Chat Completions 文本/i.test(protocolNote) && /not verified by this test|本测试不会验证该协议/i.test(protocolNote), `A visible routing API type must not imply protocol test coverage: ${protocolNote}`);
    await click('[data-testid="model-dialog-close"]');
    await wait(() => !document.querySelector('.provider-dialog'), 'closed subscription model dialog');
    await nav(1);
    const admittedWhileSelected = await admission('selected');
    record('selected', { publicCatalog: selectedList.ids, internal_id: selected.internal_id });
    passed('selecting the discovered model puts it into the public catalog');
    // 3. 取消选择：移出公共目录；显式原 ID 直调的准入结论必须与已选时完全一致。
    await nav(1);
    await click(`[data-testid="sub-select-${providerId}-${discovered.internal_id}"]`);
    const deselected = await wait(async () => { const current = await entry(); return !current.selected ? current : null; }, 'catalog deselection persisted');
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
    // #15 之后失败不再让 refresh 命令报错，而是如实写进证据状态（catalog=stale、保留模型与旧时间）；
    // 目录资格同样只标陈旧，界面必须能看到这两层，而不是靠命令异常。
    await setModel({ enabled: true, selected: true });
    const failure = await invoke('refresh_subscription', { providerId }).then(() => null, error => String(error));
    check(failure === null, `A failed directory read is reported in the evidence, not as a command error: ${failure}`);
    const failureView = await view();
    check(failureView?.catalog?.state === 'stale', `A failed read must keep the verified directory and mark it stale: ${JSON.stringify(failureView?.catalog)}`);
    check((failureView?.models || []).length > 0, `A failed read must keep the verified models: ${JSON.stringify(failureView?.models)}`);
    const stale = await entry();
    check(stale.availability === 'stale', `A failed read must keep the entry and mark it stale: ${JSON.stringify(stale)}`);
    check(stale.eligibility === 'stale', `A stale entry stays qualified for the same account: ${JSON.stringify(stale)}`);
    check(stale.selected === true && stale.disabled === false, `A failed read must not rewrite user configuration: ${JSON.stringify(stale)}`);
    const staleRow = await row();
    check(Boolean(staleRow), 'A failed read must keep the model row');
    const admittedWhileStale = await admission('stale');
    check(admittedWhileStale.code === admittedWhileSelected.code, `A stale entry must keep the direct-call admission result: ${JSON.stringify(admittedWhileStale)}`);
    record('stale', { evidence: failureView?.catalog?.state ?? null, error: failure, availability: stale.availability, eligibility: stale.eligibility, selected: stale.selected, disabled: stale.disabled, direct: admittedWhileStale });
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
    if (window.__ISOLATION_CHECK__.loginMode === 'catalog') {
      // 目录/额度只读验收：先成功登录，再按替身队列逐个排练目录与额度场景。
      try { await catalogLifecycle(); report.ok = true; }
      catch (error) { report.error = String(error); report.screen = document.body.innerText.slice(-5000); }
      await invoke('isolation_check_report', { report });
      return;
    }
    if (window.__ISOLATION_CHECK__.loginMode) {
      try {
        if (window.__ISOLATION_CHECK__.loginMode === 'catalog') await catalogLifecycle();
        else if (window.__ISOLATION_CHECK__.loginMode === 'model-selection') await modelSelectionLifecycle();
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
