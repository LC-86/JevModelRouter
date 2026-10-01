import { describe, expect, it } from 'vitest';
import { translate } from './preferences-context';
import {
  CATALOG_REFERENCE_URL, QUOTA_REFERENCE_URL, agentCatalogPendingSync, agentSelectableModels, authHomeLabel,
  availabilityLabel, capabilityLabel, catalogEntryEligible, catalogEntryInPool, catalogEntryStateLabel, catalogLabel,
  catalogModelRow, catalogRemovedLabel, catalogText, connectionStateLabel, connectionStateTone, creditsAxisText,
  denialLabel, eligibilityLabel, helperVersionLabel, identityLabel, isSubscriptionProvider, knownOrUnknown,
  localLogoutLabel, loginStageLabel, loginStageTone, modelCapability, modelListMembers, modelSelected, protocolKey,
  quotaEvidenceText, quotaHistoryLabel, quotaLabel, quotaPermissionLabel, quotaViewLabel, remainingPercent,
  remoteRevocationLabel, speedTestCandidates, subscriptionActions, subscriptionCatalog, subscriptionCatalogText,
  subscriptionQuotaText, subscriptionReason, subscriptionStatusText, subscriptionView, unselectedModels,
} from './subscription';
import type { DashboardSnapshot, Model, Provider, SubscriptionCatalogEntry, SubscriptionQuota, SubscriptionView } from '../types';

const t = (message: string, values?: Record<string, string | number>) => translate('zh-CN', message, values);

function view(patch: Partial<SubscriptionView> = {}): SubscriptionView {
  return {
    provider_id: 'codex', label: 'Codex', generation: 1, connection_instance_id: 'fixture-instance', state: 'not_connected', identity: null,
    models: [], capabilities: [], quota: { state: 'unknown' }, denial: null, adapter_available: false,
    ...patch,
  };
}

const snapshot = { subscriptions: [view()] } as unknown as DashboardSnapshot;

describe('subscription views', () => {
  it('separates subscription providers from API providers', () => {
    expect(isSubscriptionProvider({ id: 'codex', name: 'Codex', kind: 'codex_subscription', base_url: '', enabled: true, has_api_key: false })).toBe(true);
    expect(isSubscriptionProvider({ id: 'openrouter', name: 'OpenRouter', kind: 'openrouter', base_url: 'https://openrouter.ai/api', enabled: true, has_api_key: true })).toBe(false);
    expect(isSubscriptionProvider(undefined)).toBe(false);
  });

  it('translates connection, capability and quota states', () => {
    expect(connectionStateLabel('not_connected', t)).toBe('未连接');
    expect(connectionStateLabel('authorization_pending', t)).toBe('等待授权');
    expect(connectionStateLabel('connected', t)).toBe('已连接');
    expect(connectionStateLabel('expired', t)).toBe('授权已过期');
    expect(connectionStateTone('connected')).toBe('success');
    expect(connectionStateTone('expired')).toBe('error');
    expect(capabilityLabel('unverified', t)).toBe('未验证');
    expect(capabilityLabel('unsupported', t)).toBe('不支持');
    expect(quotaLabel('unknown', t)).toBe('额度未知');
    expect(quotaLabel('unsupported', t)).toBe('无额度接口');
    expect(quotaLabel('denied', t)).toBe('额度访问被拒绝');
    expect(denialLabel({ code: 'quota_denied', family: 'quota', message: '', recovery: '' }, t)).toBe('额度访问被拒绝');
  });

  it('shows unknown identity instead of guessing it', () => {
    expect(identityLabel(view(), t)).toBe('未知');
    expect(identityLabel(view({ identity: 'fixture@example.invalid' }), t)).toBe('fixture@example.invalid');
    expect(identityLabel(undefined, t)).toBe('未知');
  });

  it('treats missing capability records as unverified and keeps protocols apart', () => {
    const withCapability = view({ capabilities: [{ model_id: 'fixture-model', protocol: 'responses', status: 'verified' }] });
    expect(modelCapability(withCapability, 'fixture-model', 'responses')).toBe('verified');
    expect(modelCapability(withCapability, 'fixture-model', 'chat_completions')).toBe('unverified');
    expect(modelCapability(undefined, 'fixture-model', 'responses')).toBe('unverified');
    expect(protocolKey('responses')).toBe('responses');
    expect(protocolKey('messages')).toBe('messages');
    expect(protocolKey('chat/completions')).toBe('chat_completions');
  });

  it('surfaces the denial reason before any secondary quota note', () => {
    const denied = view({ denial: { code: 'not_connected', family: 'not_connected', message: 'Codex is not connected.', recovery: 'Connect the account in AutoJev → Providers.' } });
    expect(subscriptionReason(denied, t)).toContain('Codex is not connected.');
    expect(subscriptionReason(denied, t)).toContain('Connect the account');
    expect(subscriptionReason(view({ quota: { state: 'stale' } }), t)).toBe('额度依据陈旧');
    expect(subscriptionReason(view({ quota: { state: 'available' } }), t)).toBe('');
    expect(subscriptionReason(undefined, t)).toBe('');
    expect(denialLabel(undefined, t)).toBe('');
    expect(denialLabel(denied.denial, t)).toBe('未连接');
    expect(denialLabel({ code: 'quota_failed', family: 'quota', message: '', recovery: '' }, t)).toBe('额度读取失败');
    expect(denialLabel({ code: 'extra_usage_allowed', family: 'quota', message: '', recovery: '' }, t)).toBe('上游当前允许使用额外 credits');
    expect(denialLabel({ code: 'extra_usage_permission_unknown', family: 'quota', message: '', recovery: '' }, t)).toBe('尚未核实上游是否禁止额外 credits');
    // #17 的四种账号资格拒绝码必须登记文案，否则界面只会显示裸 code。
    expect(denialLabel({ code: 'model_not_discovered', family: 'not_eligible', message: '', recovery: '' }, t)).toBe('当前账号目录中没有该模型');
    expect(denialLabel({ code: 'model_removed', family: 'not_eligible', message: '', recovery: '' }, t)).toBe('上游目录已移除该模型');
    expect(denialLabel({ code: 'model_revoked', family: 'not_eligible', message: '', recovery: '' }, t)).toBe('上游已撤销该账号的模型权限');
    expect(denialLabel({ code: 'model_unqualified', family: 'not_eligible', message: '', recovery: '' }, t)).toBe('该模型的账号资格尚未按当前连接重新核实');
    // 未知 code 原样显示，不编造文案。
    expect(denialLabel({ code: 'future_reason', family: 'quota', message: '', recovery: '' }, t)).toBe('future_reason');
    expect(subscriptionView(snapshot, 'codex')?.generation).toBe(1);
    expect(subscriptionView(snapshot, 'grok')).toBeUndefined();
  });
});

describe('subscription login and logout lifecycle', () => {
  it('maps login stages to labels and tones', () => {
    expect(loginStageLabel('idle', t)).toBe('未开始登录');
    expect(loginStageLabel('pending', t)).toBe('等待授权');
    expect(loginStageLabel('completed', t)).toBe('登录已完成');
    expect(loginStageLabel('failed', t)).toBe('登录失败');
    expect(loginStageLabel('cancelled', t)).toBe('登录已取消');
    expect(loginStageTone('completed')).toBe('success');
    expect(loginStageTone('failed')).toBe('error');
    expect(loginStageTone('pending')).toBe('warning');
  });

  it('keeps local clearing and remote revocation apart', () => {
    expect(localLogoutLabel('cleared', t)).toBe('本地凭据已清除');
    expect(localLogoutLabel('retained', t)).toBe('本地凭据仍保留');
    expect(remoteRevocationLabel('revoked', t)).toBe('远端授权已撤销');
    expect(remoteRevocationLabel('failed', t)).toBe('远端撤销失败');
    expect(remoteRevocationLabel('unsupported', t)).toBe('远端撤销不受支持');
    expect(remoteRevocationLabel('unknown', t)).toBe('远端撤销结果未知');
  });

  it('shows Unknown for every missing piece of evidence', () => {
    expect(knownOrUnknown(null)).toBe('Unknown');
    expect(knownOrUnknown('   ')).toBe('Unknown');
    expect(knownOrUnknown(' fixture ')).toBe('fixture');
    expect(helperVersionLabel(view())).toBe('Unknown');
    expect(authHomeLabel(view())).toBe('Unknown');
  });

  it('falls back to the retained helper fields when the new helper view is absent', () => {
    const legacy = view({ helper_version: '0.4.0', account_path: '/home/fixture/.autojev/helpers/codex/codex' });
    expect(helperVersionLabel(legacy)).toBe('0.4.0');
    expect(authHomeLabel(legacy)).toBe('/home/fixture/.autojev/helpers/codex/codex');
  });

  it('renders state, generation, identity, helper and auth home in the fixed status text', () => {
    const text = subscriptionStatusText(view({ generation: 7 }), t);
    expect(text).toContain('state=not_connected');
    expect(text).toContain('未连接');
    expect(text).toContain('generation=7');
    expect(text).toContain('identity=Unknown');
    expect(text).toContain('helper=Unknown');
    expect(text).toContain('auth_home=Unknown');

    const connected = subscriptionStatusText(view({
      generation: 12,
      state: 'connected',
      identity: 'fixture@example.invalid',
      helper: { available: true, version: '0.4.0', auth_home: '/home/fixture/.autojev/helpers/codex/codex' },
    }), t);
    expect(connected).toContain('state=connected');
    expect(connected).toContain('已连接');
    expect(connected).toContain('generation=12');
    expect(connected).toContain('identity=fixture@example.invalid');
    expect(connected).toContain('helper=0.4.0');
    expect(connected).toContain('auth_home=/home/fixture/.autojev/helpers/codex/codex');
    expect(connected).not.toContain('local=');
  });

  it('adds the login stage and the separate logout outcomes to the status text', () => {
    const pending = subscriptionStatusText(view({
      state: 'authorization_pending',
      generation: 3,
      login: { stage: 'pending', authorization_url: 'https://example.invalid/auth', user_code: 'ABCD-1234', attempt: 1, generation: 3 },
    }), t);
    expect(pending).toContain('login=pending');
    expect(pending).toContain('generation=3');
    // 挂起时授权链接与 user code 通过固定 selector 的文本可见。
    expect(pending).toContain('authorization_url=https://example.invalid/auth');
    expect(pending).toContain('user_code=ABCD-1234');
    expect(subscriptionStatusText(view({ login: { stage: 'completed', attempt: 1, generation: 3 } }), t)).not.toContain('authorization_url=');

    const loggedOut = subscriptionStatusText(view({
      generation: 4,
      logout: { local: 'cleared', remote: 'revoked', observed_at: '2026-09-30T00:00:00Z' },
    }), t);
    expect(loggedOut).toContain('local=cleared');
    expect(loggedOut).toContain('remote=revoked');
    // 远端失败或未知不得被折叠成已撤销。
    expect(subscriptionStatusText(view({ logout: { local: 'retained', remote: 'failed' } }), t)).toContain('remote=failed');
    expect(subscriptionStatusText(view({ logout: { local: 'retained', remote: 'unknown' } }), t)).toContain('remote=unknown');
    // 没有快照时不编造连接状态。
    expect(subscriptionStatusText(undefined, t)).toContain('state=not_connected');
    expect(subscriptionStatusText(undefined, t)).toContain('generation=0');
  });

  it('shows quota admission reasons without changing the connection denial axis', () => {
    const connected = view({
      state: 'connected',
      denial: null,
      admission_denial: {
        code: 'extra_usage_permission_unknown', family: 'quota',
        message: 'No evidence extra credits are prohibited.', recovery: 'Refresh read-only evidence.',
      },
    });
    expect(connectionStateLabel(connected.state, t)).toBe('已连接');
    expect(subscriptionReason(connected, t)).toBe('No evidence extra credits are prohibited. Refresh read-only evidence.');
    expect(denialLabel(connected.admission_denial, t)).toBe('尚未核实上游是否禁止额外 credits');
  });

  it('derives button availability from the connection state only', () => {
    expect(subscriptionActions('not_connected')).toEqual({ canLogin: true, canCancel: false, canLogout: false });
    expect(subscriptionActions('authorization_pending')).toEqual({ canLogin: false, canCancel: true, canLogout: false });
    expect(subscriptionActions('connected')).toEqual({ canLogin: false, canCancel: false, canLogout: true });
    expect(subscriptionActions('expired')).toEqual({ canLogin: true, canCancel: false, canLogout: false });
  });
});

function model(patch: Partial<Model> = {}): Model {
  return {
    id: 'internal-1', provider_id: 'codex', model_id: 'gpt-5-codex', name: 'GPT-5 Codex', tier: 'balanced',
    enabled: true, supports_tools: true, supports_vision: false, supports_reasoning: true, context_window: 200000,
    input_cost_per_million: 0, output_cost_per_million: 0, ...patch,
  };
}

function entry(patch: Partial<SubscriptionCatalogEntry> = {}): SubscriptionCatalogEntry {
  return {
    model_id: 'gpt-5-codex', internal_id: 'internal-1', availability: 'available', eligibility: 'eligible',
    selected: true, disabled: false, ...patch,
  };
}

function provider(patch: Partial<Provider> = {}): Provider {
  return { id: 'codex', name: 'Codex', kind: 'codex_subscription', base_url: '', enabled: true, has_api_key: false, ...patch };
}

describe('subscription catalog selection and eligibility', () => {
  it('treats a missing selected flag as selected so legacy configs keep their models', () => {
    expect(modelSelected(model())).toBe(true);
    expect(modelSelected(model({ selected: undefined }))).toBe(true);
    expect(modelSelected(model({ selected: false }))).toBe(false);
  });

  it('keeps only selected models in the model list and its count', () => {
    const models = [model({ id: 'a' }), model({ id: 'b', selected: false }), model({ id: 'c', selected: true })];
    expect(modelListMembers(models).map(m => m.id)).toEqual(['a', 'c']);
    expect(unselectedModels(models).map(m => m.id)).toEqual(['b']);
    // 停用模型仍是列表成员（由行内标注停用），但不进入任何候选。
    expect(modelListMembers([model({ id: 'd', enabled: false })]).map(m => m.id)).toEqual(['d']);
  });

  it('excludes unselected and disabled models from speed tests and agent catalogs alike', () => {
    const providers = [provider(), provider({ id: 'grok', enabled: false })];
    const models = [
      model({ id: 'selected' }),
      model({ id: 'unselected', selected: false }),
      model({ id: 'disabled', enabled: false }),
      model({ id: 'grok-model', provider_id: 'grok' }),
    ];
    expect(speedTestCandidates(models, providers).map(m => m.id)).toEqual(['selected']);
    expect(agentSelectableModels(models, providers).map(m => m.id)).toEqual(['selected']);
  });

  it('maps availability and eligibility to localised labels', () => {
    expect(availabilityLabel('available', t)).toBe('可用');
    expect(availabilityLabel('stale', t)).toBe('陈旧（保留上次已核实结果）');
    expect(availabilityLabel('removed', t)).toBe('上游已移除');
    expect(availabilityLabel('revoked', t)).toBe('该账号权限已撤销');
    expect(availabilityLabel('unknown', t)).toBe('可用性未知');
    // 未知枚举值回落为「未知」，不编造可用性。
    expect(availabilityLabel('future' as never, t)).toBe('可用性未知');
    expect(eligibilityLabel('eligible', t)).toBe('资格有效');
    expect(eligibilityLabel('stale', t)).toBe('资格有效（证据陈旧）');
    expect(eligibilityLabel('not_discovered', t)).toBe('尚未发现');
    expect(eligibilityLabel('removed', t)).toBe('已从上游目录移除');
    expect(eligibilityLabel('revoked', t)).toBe('权限已撤销');
    expect(eligibilityLabel('account_changed', t)).toBe('已在其它账号下核实');
    expect(eligibilityLabel('unknown', t)).toBe('资格未知');
    expect(eligibilityLabel('future' as never, t)).toBe('资格未知');
  });

  it('keeps stale eligibility usable but never treats removal or revocation as eligible', () => {
    expect(catalogEntryEligible(entry({ eligibility: 'eligible' }))).toBe(true);
    expect(catalogEntryEligible(entry({ eligibility: 'stale' }))).toBe(true);
    for (const eligibility of ['not_discovered', 'removed', 'revoked', 'account_changed', 'unknown'] as const) {
      expect(catalogEntryEligible(entry({ eligibility }))).toBe(false);
      expect(catalogEntryInPool(entry({ eligibility }))).toBe(false);
    }
    expect(catalogEntryInPool(entry())).toBe(true);
    expect(catalogEntryInPool(entry({ selected: false }))).toBe(false);
    expect(catalogEntryInPool(entry({ disabled: true }))).toBe(false);
    expect(catalogEntryStateLabel(entry(), t)).toBe('可用 · 资格有效');
  });

  it('resolves the local model row by stable internal id, never by display name', () => {
    const models = [model({ id: 'internal-1', name: 'Old name' }), model({ id: 'other', model_id: 'gpt-5-codex' })];
    expect(catalogModelRow(models, entry({ name: 'Renamed upstream' }))?.id).toBe('internal-1');
    expect(catalogModelRow(models, entry({ internal_id: 'missing' }))).toBeUndefined();
  });

  it('lists catalog entries from the subscription view and treats a missing catalog as empty', () => {
    expect(subscriptionCatalog(view({ catalog_entries: [entry()] }))).toHaveLength(1);
    expect(subscriptionCatalog(view())).toEqual([]);
    expect(subscriptionCatalog(undefined)).toEqual([]);
  });

  it('flags pending sync when the saved catalog differs from the current in-app selection', () => {
    expect(agentCatalogPendingSync({ saved: ['model/a'], chosen: ['model/a'] })).toBe(false);
    expect(agentCatalogPendingSync({ saved: [], chosen: [] })).toBe(false);
    expect(agentCatalogPendingSync({ saved: ['model/a'], chosen: [] })).toBe(true);
    expect(agentCatalogPendingSync({ saved: [], chosen: ['model/a'] })).toBe(true);
    expect(agentCatalogPendingSync({ saved: ['model/a', 'model/b'], chosen: ['model/b', 'model/a'] })).toBe(true);
    expect(agentCatalogPendingSync({ saved: ['model/a'], chosen: ['model/a', 'model/a'] })).toBe(false);
  });

  it('flags pending sync whenever the backend marks the catalog as pending', () => {
    expect(agentCatalogPendingSync({ saved: [], chosen: [], pendingSync: true })).toBe(true);
    expect(agentCatalogPendingSync({ saved: ['model/a'], chosen: ['model/a'], pendingSync: false })).toBe(false);
  });
});

describe('subscription catalog and quota evidence', () => {
  it('labels denied quota permission and keeps unknown as unknown', () => {
    expect(quotaPermissionLabel('allowed', t)).toBe('允许使用额度');
    expect(quotaPermissionLabel('denied', t)).toBe('额度使用被拒绝');
    expect(quotaPermissionLabel('unknown', t)).toBe('额度许可未知');
    expect(quotaPermissionLabel(undefined, t)).toBe('额度许可未知');
    // 未知绝不能被折叠成“已关闭”。
    expect(quotaPermissionLabel(undefined, t)).not.toBe('已关闭');
    expect(quotaViewLabel('rate_limits_by_limit_id', t)).toBe('多桶额度');
    expect(quotaViewLabel('rate_limits', t)).toBe('旧版单桶额度');
    expect(quotaViewLabel('unknown', t)).toBe('额度视图未知');
    expect(quotaViewLabel(undefined, t)).toBe('额度视图未知');
    expect(catalogLabel('unknown', t)).toBe('目录未知');
    expect(catalogLabel('available', t)).toBe('目录可用');
    expect(catalogLabel('stale', t)).toBe('目录已陈旧');
    expect(catalogLabel('failed', t)).toBe('目录读取失败');
  });

  it('only shows a history note for retained quota numbers', () => {
    expect(quotaHistoryLabel(undefined, t)).toBe('');
    expect(quotaHistoryLabel({ state: 'failed' }, t)).toBe('');
    expect(quotaHistoryLabel({ state: 'failed', history: false }, t)).toBe('');
    expect(quotaHistoryLabel({ state: 'failed', history: true, observed_at: '2026-09-30T04:05:06Z' }, t))
      .toBe('历史数据 · 最后成功更新 2026-09-30T04:05:06Z');
    // 历史标记但没有时间时也必须说明更新时间未知。
    expect(quotaHistoryLabel({ state: 'failed', history: true }, t)).toBe('历史数据 · 最后成功更新 未知');
  });

  it('displays an unconfirmed account as disconnected with stale historical evidence', () => {
    const historical = view({
      state: 'not_connected', identity: 'A@example.invalid',
      catalog: { state: 'stale', observed_at: '2026-09-30T04:05:06Z' },
      models: [{ model_id: 'old-model', eligible: false }],
      quota: { state: 'failed', history: true, observed_at: '2026-09-30T04:05:06Z' },
      denial: { code: 'not_connected', family: 'not_connected', message: 'Codex is not connected.', recovery: 'Connect account.' },
    });
    expect(connectionStateLabel(historical.state, t)).toBe('未连接');
    expect(subscriptionStatusText(historical, t)).toContain('state=not_connected');
    expect(subscriptionReason(historical, t)).toBe('Codex is not connected. Connect account.');
    expect(denialLabel(historical.denial, t)).toBe('未连接');
    expect(subscriptionCatalogText(historical)).toContain('catalog_state=stale');
    expect(subscriptionCatalogText(historical)).toContain('old-model:false');
    expect(subscriptionQuotaText(historical)).toContain('quota_state=failed');
    expect(quotaHistoryLabel(historical.quota, t)).toBe('历史数据 · 最后成功更新 2026-09-30T04:05:06Z');
  });

  it('derives the remaining percent from usedPercent only when it is valid', () => {
    expect(remainingPercent(0)).toBe(100);
    expect(remainingPercent(33.3)).toBeCloseTo(66.7, 4);
    expect(remainingPercent(100)).toBe(0);
    expect(remainingPercent(null)).toBeNull();
    expect(remainingPercent(undefined)).toBeNull();
    expect(remainingPercent(142)).toBeNull();
    expect(remainingPercent(-1)).toBeNull();
    expect(remainingPercent(Number.NaN)).toBeNull();
    expect(remainingPercent(Number.POSITIVE_INFINITY)).toBeNull();
  });

  it('renders machine-readable quota text with buckets, credits and remaining', () => {
    const quota: SubscriptionQuota = {
      state: 'available', view: 'rate_limits_by_limit_id', history: false, missing_fields: [],
      source: 'codex-app-server:account/rateLimits/read#rateLimitsByLimitId',
      observed_at: '2026-09-30T04:05:06Z',
      buckets: [{
        limit_id: 'fixture-limit', permission: 'allowed',
        windows: [{ label: 'primary', used_percent: 33.3, window_minutes: 300, resets_at: 1790000000 }],
        credits: { has_credits: false, unlimited: false, balance: '12.5', missing_fields: ['balance_unit'] },
      }],
    };
    const text = quotaEvidenceText(quota);
    expect(text).toContain('quota_state=available');
    expect(text).toContain('quota_view=rate_limits_by_limit_id');
    expect(text).toContain('permission=allowed');
    expect(text).toContain('source=codex-app-server:account/rateLimits/read#rateLimitsByLimitId');
    expect(text).toContain('observed_at=2026-09-30T04:05:06Z');
    expect(text).toContain('history=false');
    expect(text).toContain('bucket=fixture-limit');
    // remaining 必须由 100 - usedPercent 得来，并标为 remaining。
    expect(text).toContain('primary:used=33.3,remaining=66.7,minutes=300,resets_at=1790000000');
    expect(text).toContain('credits=has:false,unlimited:false,balance:12.5');
    expect(text).toContain('missing=balance_unit');
    // 视图级包装与证据级文本一致。
    expect(subscriptionQuotaText(view({ quota }))).toBe(text);
  });

  it('never turns unknown quota facts into zero or a closed state', () => {
    const text = subscriptionQuotaText(view());
    expect(text).toContain('quota_state=unknown');
    expect(text).toContain('quota_view=unknown');
    expect(text).toContain('permission=unknown');
    expect(text).toContain('history=false');
    expect(text).not.toContain('used=0');
    expect(text).not.toContain('remaining=0');
    expect(text).not.toContain('已关闭');
    expect(subscriptionQuotaText(undefined)).toBe(quotaEvidenceText(undefined));
    expect(subscriptionQuotaText(undefined)).toContain('permission=unknown');
  });

  it('keeps invalid and missing window fields distinguishable', () => {
    const text = subscriptionQuotaText(view({
      quota: {
        state: 'unknown', view: 'rate_limits', history: false,
        missing_fields: ['rateLimitsByLimitId'],
        buckets: [{
          limit_id: 'legacy', permission: 'unknown',
          windows: [{ label: 'single', used_percent: null, window_minutes: 60, resets_at: null, invalid_fields: ['usedPercent', 'resetsAt'] }],
          missing_fields: ['credits'],
        }],
      },
    }));
    // used 无效时不给剩余值，也绝不写成 0。
    expect(text).toContain('single:used=Unknown,remaining=Unknown,minutes=60,resets_at=Unknown');
    expect(text).not.toContain('remaining=0');
    // 缺字段与越界字段都必须各自可见：配额级与桶级缺失分开记录。
    expect(text).toContain('missing=rateLimitsByLimitId');
    expect(text).toContain('missing=credits');
    expect(text).toContain('invalid=usedPercent,resetsAt');
    expect(text).toContain('credits=has:Unknown,unlimited:Unknown,balance:Unknown');
  });

  it('marks denied permission from any bucket in the aggregated quota text', () => {
    const text = subscriptionQuotaText(view({
      quota: {
        state: 'denied', view: 'rate_limits_by_limit_id', history: false,
        buckets: [
          { limit_id: 'first', permission: 'allowed', windows: [{ label: 'primary', used_percent: 10, window_minutes: 60, resets_at: 1790000000 }] },
          { limit_id: 'second', permission: 'denied', windows: [] },
        ],
      },
    }));
    expect(text).toContain('quota_state=denied');
    expect(text).toContain('permission=denied');
    expect(text).toContain('bucket=first');
    expect(text).toContain('bucket=second');
  });

  it('renders machine-readable catalog text with eligibility untouched', () => {
    const withCatalog = view({
      catalog: { state: 'stale', source: 'codex-app-server:model/list', observed_at: '2026-09-30T04:05:06Z', missing_fields: ['model.list[3].id'] },
      models: [{ model_id: 'gpt-5-codex', name: 'GPT-5 Codex', eligible: false }],
    });
    const text = subscriptionCatalogText(withCatalog);
    expect(text).toContain('catalog_state=stale');
    expect(text).toContain('source=codex-app-server:model/list');
    expect(text).toContain('observed_at=2026-09-30T04:05:06Z');
    // 发现不等于资格：eligible 必须原样显示，不得标成可调用。
    expect(text).toContain('models=gpt-5-codex:false');
    expect(text).toContain('missing=model.list[3].id');
    // 目录证据级文本不含模型列表，但也必须给出同样的状态与来源。
    expect(catalogText(withCatalog.catalog)).toContain('catalog_state=stale');
    expect(catalogText(withCatalog.catalog)).toContain('missing=model.list[3].id');
    // #16 在 models= 与 missing= 之间固定插入 removed=（上一次已核实目录里已消失的模型）。
    expect(catalogText(undefined)).toBe('catalog_state=unknown source=Unknown observed_at=Unknown removed= missing=');
  });

  it('always renders the credits axis, even when the backend sent no credits at all', () => {
    // 后端完全没给 credits：token 仍是诚实的 Unknown，该轴不消失，也不是 0/100%。
    expect(quotaEvidenceText({ state: 'available', buckets: [{ limit_id: 'subscription_pool' }] }))
      .toContain('credits=has:Unknown,unlimited:Unknown,balance:Unknown,unit:Unknown,permission:unknown');
    const unknown = { has_credits: null, unlimited: null, balance: null, unit: null, permission: 'unknown' as const, missing_fields: ['has_credits'], invalid_fields: [] };
    const axis = creditsAxisText(unknown, t);
    expect(axis).toContain('额外 credits');
    expect(axis).toContain('额外 credits 许可未知');
    expect(axis).toContain('余额: Unknown');
    expect(axis).toContain('单位: Unknown');
    expect(axis).toContain('缺字段: has_credits');
    const missingAxis = creditsAxisText(null, t);
    expect(missingAxis).toContain('额外 credits');
    expect(missingAxis).toContain('有 credits: Unknown');
    expect(creditsAxisText({ permission: 'allowed' }, t)).toContain('允许使用额外 credits');
    expect(creditsAxisText({ permission: 'denied' }, t)).toContain('禁止使用额外 credits');
    expect([axis, missingAxis].join(' ')).not.toMatch(/0%|100%|余额: 0|单位: 0/);
  });

  it('lists removed models between models and missing, and never as ineligible', () => {
    // removed= 空时为空串，位置固定在 models= 与 missing= 之间。
    expect(subscriptionCatalogText(view({ catalog: { state: 'available', removed_models: [] }, models: [{ model_id: 'grok-4', eligible: true }] })))
      .toBe('catalog_state=available source=Unknown observed_at=Unknown models=grok-4:true removed= missing=');
    expect(catalogText({ state: 'available', removed_models: ['grok-3', 'grok-3-mini'] }))
      .toBe('catalog_state=available source=Unknown observed_at=Unknown removed=grok-3,grok-3-mini missing=');
    // 被移除 ≠ 不具备资格，也绝不写成可调用。
    expect(catalogRemovedLabel(['grok-3', 'grok-3-mini'], t)).toBe('已从上一次目录移除：grok-3, grok-3-mini');
    expect(catalogRemovedLabel([], t)).toBe('');
    expect(catalogRemovedLabel(undefined, t)).toBe('');
    expect(catalogRemovedLabel(['  '], t)).toBe('');
    expect(catalogRemovedLabel(['grok-3'], t)).not.toContain('资格');
  });

  it('passes the credits unit through as raw upstream text and keeps the permission axes apart', () => {
    const withUnit: SubscriptionQuota = {
      state: 'available', buckets: [{
        limit_id: 'subscription_pool', permission: 'allowed',
        credits: { has_credits: true, unlimited: false, balance: '12.50', unit: 'USD', permission: 'denied', missing_fields: [] },
      }],
    };
    const text = quotaEvidenceText(withUnit);
    expect(text).toContain('credits=has:true,unlimited:false,balance:12.50,unit:USD,permission:denied');
    // 池许可仍是 allowed：credits 轴不参与聚合。
    expect(text).toContain('permission=allowed');
    // 缺单位不推断单位，缺余额不用 0 顶替。
    expect(quotaEvidenceText({ state: 'available', buckets: [{ limit_id: 'subscription_pool', credits: { has_credits: true, permission: 'unknown' } }] }))
      .toContain('credits=has:true,unlimited:Unknown,balance:Unknown,unit:Unknown,permission:unknown');
    expect(quotaEvidenceText({ state: 'available', buckets: [{ limit_id: 'subscription_pool', credits: null }] }))
      .toContain('credits=has:Unknown,unlimited:Unknown,balance:Unknown,unit:Unknown,permission:unknown');
    // Grok 只读契约的视图标签与官方入口是 Grok 专属事实，Codex 没有自己的入口时不新增。
    expect(catalogLabel('unsupported', t)).toBe('无目录接口');
    expect(quotaViewLabel('grok_cli_usage', t)).toBe('Grok CLI 用量');
    expect(CATALOG_REFERENCE_URL).toBe('https://docs.x.ai/build/cli/reference');
    expect(QUOTA_REFERENCE_URL).toBe('https://docs.x.ai/grok/faq#usage--limits');
  });

  it('tolerates legacy snapshots without catalog or extended quota fields', () => {
    expect(subscriptionCatalogText(undefined)).toContain('catalog_state=unknown');
    expect(subscriptionCatalogText(undefined)).toContain('models=');
    // 旧快照没有 catalog，且 quota 只有 state/source/observed_at。
    const legacy = view({ quota: { state: 'unknown', source: null, observed_at: null } });
    expect(subscriptionCatalogText(legacy)).toContain('catalog_state=unknown');
    expect(subscriptionQuotaText(legacy)).toContain('permission=unknown');
    expect(quotaHistoryLabel(legacy.quota, t)).toBe('');
  });
});
