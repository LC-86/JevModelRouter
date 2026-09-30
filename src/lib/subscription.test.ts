import { describe, expect, it } from 'vitest';
import { translate } from './preferences-context';
import {
  authHomeLabel, booleanText, catalogLabel, catalogRemovedLabel, CATALOG_REFERENCE_URL, catalogText, capabilityLabel,
  connectionStateLabel, connectionStateTone, creditsAxisText, denialLabel, evidenceNumberText, helperVersionLabel,
  identityLabel, isSubscriptionProvider, knownOrUnknown, localLogoutLabel, loginStageLabel, loginStageTone,
  modelCapability, modelEligibilityLabel, percentText, protocolKey, quotaEvidenceText, quotaHistoryLabel, quotaLabel,
  quotaPermission, quotaPermissionLabel, quotaViewLabel, QUOTA_REFERENCE_URL, remainingPercent, remoteRevocationLabel,
  subscriptionActions, subscriptionCatalogText, subscriptionQuotaText, subscriptionReason, subscriptionStatusText,
  subscriptionView,
} from './subscription';
import type { DashboardSnapshot, SubscriptionQuota, SubscriptionView } from '../types';

const t = (message: string, values?: Record<string, string | number>) => translate('zh-CN', message, values);

function view(patch: Partial<SubscriptionView> = {}): SubscriptionView {
  return {
    provider_id: 'codex', label: 'Codex', generation: 1, state: 'not_connected', identity: null,
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

  it('derives button availability from the connection state only', () => {
    expect(subscriptionActions('not_connected')).toEqual({ canLogin: true, canCancel: false, canLogout: false });
    expect(subscriptionActions('authorization_pending')).toEqual({ canLogin: false, canCancel: true, canLogout: false });
    expect(subscriptionActions('connected')).toEqual({ canLogin: false, canCancel: false, canLogout: true });
    expect(subscriptionActions('expired')).toEqual({ canLogin: true, canCancel: false, canLogout: false });
  });
});

describe('read-only catalog and quota evidence', () => {
  const quota = (patch: Partial<SubscriptionQuota> = {}): SubscriptionQuota => ({
    state: 'unknown', source: null, observed_at: null, view: 'unknown', buckets: [], missing_fields: [], history: false,
    ...patch,
  });
  const poolBucket = {
    limit_id: 'subscription_pool', name: 'Subscription pool', plan_type: 'pro', permission: 'allowed' as const,
    windows: [{ label: 'weekly', used_percent: 42, window_minutes: null, resets_at: 1799999999, missing_fields: [], invalid_fields: [] }],
    credits: { has_credits: true, unlimited: false, balance: '12.50', unit: 'USD', permission: 'denied' as const },
    missing_fields: [], invalid_fields: [],
  };

  it('keeps the catalog token order and never marks an ineligible model callable', () => {
    const models = [
      { model_id: 'grok-4', name: null, eligible: true },
      { model_id: 'grok-4-fast', name: 'Grok 4 Fast', eligible: false },
    ];
    expect(catalogText({ state: 'available', source: 'grok-cli:models', observed_at: '2026-09-30T12:00:00Z', missing_fields: ['models[1].id'] }, models))
      .toBe('catalog_state=available source=grok-cli:models observed_at=2026-09-30T12:00:00Z models=grok-4:true,grok-4-fast:false removed= missing=models[1].id');
    // 未读取过：状态未知、来源未知、列表为空、无被移除条目，且不编造条目。
    expect(catalogText(undefined, [])).toBe('catalog_state=unknown source=Unknown observed_at=Unknown models= removed= missing=');
    expect(catalogText({ state: 'stale' }, models)).toContain('catalog_state=stale');
    expect(catalogText({ state: 'unsupported' }, [])).toContain('models= removed= missing=');
    expect(subscriptionCatalogText(undefined)).toBe('catalog_state=unknown source=Unknown observed_at=Unknown models= removed= missing=');
    // 发现 ≠ 准入：eligible=false 只显示不具备资格。
    expect(modelEligibilityLabel(false, t)).toBe('账号不具备该模型资格');
    expect(modelEligibilityLabel(true, t)).toBe('账号具备该模型资格');
  });

  it('lists removed models between models and missing, and never as ineligible', () => {
    // removed= 空时为空串，位置固定在 models= 与 missing= 之间。
    expect(catalogText({ state: 'available', removed_models: [] }, [{ model_id: 'grok-4', eligible: true }]))
      .toBe('catalog_state=available source=Unknown observed_at=Unknown models=grok-4:true removed= missing=');
    expect(catalogText({ state: 'available', removed_models: ['grok-3', 'grok-3-mini'] }, []))
      .toBe('catalog_state=available source=Unknown observed_at=Unknown models= removed=grok-3,grok-3-mini missing=');
    // 被移除 ≠ 不具备资格，也绝不写成可调用。
    expect(catalogRemovedLabel(['grok-3', 'grok-3-mini'], t)).toBe('已从上一次目录移除：grok-3, grok-3-mini');
    expect(catalogRemovedLabel([], t)).toBe('');
    expect(catalogRemovedLabel(undefined, t)).toBe('');
    expect(catalogRemovedLabel(['  '], t)).toBe('');
    expect(catalogRemovedLabel(['grok-3'], t)).not.toContain('资格');
  });

  it('renders the quota token order with one segment per bucket', () => {
    const evidence = quota({ state: 'available', source: 'grok-cli:usage', observed_at: '2026-09-30T12:00:00Z', view: 'grok_cli_usage', buckets: [poolBucket] });
    expect(quotaEvidenceText(evidence)).toBe(
      'quota_state=available permission=allowed source=grok-cli:usage observed_at=2026-09-30T12:00:00Z history=false missing= '
      + 'bucket=subscription_pool windows=<weekly:used=42,remaining=58,minutes=Unknown,resets_at=1799999999> '
      + 'credits=has:true,unlimited:false,balance=12.50,unit=USD,permission:denied missing= invalid=');
    expect(subscriptionQuotaText(view({ quota: evidence }))).toBe(quotaEvidenceText(evidence));
    // 未读取过时只到顶层 missing= 为止，没有 bucket 段。
    const empty = quotaEvidenceText(undefined);
    expect(empty).toBe('quota_state=unknown permission=unknown source=Unknown observed_at=Unknown history=false missing=');
    expect(empty).not.toContain('bucket=');
    expect(quotaEvidenceText(quota({ state: 'unsupported' }))).toBe('quota_state=unsupported permission=unknown source=Unknown observed_at=Unknown history=false missing=');
  });

  it('separates the pool permission axis from the credits permission', () => {
    // credits.permission=denied 不得把订阅池许可改写成 denied。
    expect(quotaPermission(quota({ buckets: [poolBucket] }))).toBe('allowed');
    expect(quotaPermission(quota({ buckets: [{ ...poolBucket, permission: 'denied' }] }))).toBe('denied');
    expect(quotaPermission(quota({ buckets: [{ ...poolBucket, permission: 'unknown' }] }))).toBe('unknown');
    expect(quotaPermission(quota())).toBe('unknown');
    expect(quotaPermission(undefined)).toBe('unknown');
    expect(quotaPermissionLabel('denied', t)).toBe('额度许可：拒绝');
    expect(quotaLabel('denied', t)).toBe('额度已被拒绝');
    expect(denialLabel({ code: 'quota_denied', family: 'quota', message: '', recovery: '' }, t)).toBe('额度依据被拒绝');
    expect(catalogLabel('unsupported', t)).toBe('无目录接口');
    expect(quotaViewLabel('grok_cli_usage', t)).toBe('Grok CLI 用量');
    expect(CATALOG_REFERENCE_URL).toBe('https://docs.x.ai/build/cli/reference');
    expect(QUOTA_REFERENCE_URL).toBe('https://docs.x.ai/grok/faq#usage--limits');
  });

  it('derives remaining only from a valid used_percent and never fabricates 0', () => {
    expect(remainingPercent(0)).toBe(100);
    expect(remainingPercent(100)).toBe(0);
    expect(remainingPercent(42.5)).toBe(57.5);
    expect(remainingPercent(33.3)).toBe(66.7);
    for (const invalid of [null, undefined, NaN, Infinity, -Infinity, -1, 101, 150]) {
      expect(remainingPercent(invalid as number | null | undefined)).toBeNull();
    }
    expect(evidenceNumberText(undefined)).toBe('Unknown');
    expect(evidenceNumberText(NaN)).toBe('Unknown');
    expect(evidenceNumberText(0)).toBe('0');
    expect(percentText(null)).toBe('Unknown');
    expect(percentText(12)).toBe('12%');
    expect(booleanText(undefined)).toBe('Unknown');
    expect(booleanText(false)).toBe('false');
    expect(booleanText(true)).toBe('true');
  });

  it('never writes a fabricated remaining=0 for an invalid window', () => {
    const invalid = quota({ buckets: [{ limit_id: 'subscription_pool', windows: [
      { label: 'weekly', used_percent: 150 }, { label: 'daily' }, { label: 'monthly', used_percent: Number.NaN },
    ] }] });
    const text = quotaEvidenceText(invalid);
    expect(text).toContain('windows=<weekly:used=150,remaining=Unknown,minutes=Unknown,resets_at=Unknown'
      + ';daily:used=Unknown,remaining=Unknown,minutes=Unknown,resets_at=Unknown'
      + ';monthly:used=Unknown,remaining=Unknown,minutes=Unknown,resets_at=Unknown>');
    expect(text).toContain('daily:used=Unknown,remaining=Unknown');
    expect(text).toContain('monthly:used=Unknown,remaining=Unknown');
    expect(text).not.toContain('remaining=0');
    // 多个窗口用 `;` 分隔，窗口内字段顺序不变。
    expect(text).toContain('resets_at=Unknown;daily:used=Unknown');
  });

  it('passes credits balance and unit through as raw upstream text', () => {
    expect(quotaEvidenceText(quota({ state: 'available', buckets: [poolBucket] }))).toContain('balance=12.50,unit=USD');
    // 缺单位不推断单位，缺余额不用 0 顶替。
    const bare = quotaEvidenceText(quota({ state: 'available', buckets: [{ ...poolBucket, credits: { has_credits: true, permission: 'unknown' } }] }));
    expect(bare).toContain('credits=has:true,unlimited:Unknown,balance=Unknown,unit=Unknown,permission:unknown');
    const noCredits = quotaEvidenceText(quota({ state: 'available', buckets: [{ ...poolBucket, credits: null }] }));
    expect(noCredits).toContain('credits=has:Unknown,unlimited:Unknown,balance=Unknown,unit=Unknown,permission:unknown');
  });

  it('always renders the credits axis, even when the backend sent no credits at all', () => {
    // 后端完全没给 credits：token 仍是诚实的 Unknown，该轴不消失，也不是 0/100%。
    expect(quotaEvidenceText(quota({ state: 'available', buckets: [{ limit_id: 'subscription_pool' }] })))
      .toContain('credits=has:Unknown,unlimited:Unknown,balance=Unknown,unit=Unknown,permission:unknown');
    // 后端给了 credits 但字段全为 null：同样 Unknown，并显式列出缺字段。
    const unknownCredits = { has_credits: null, unlimited: null, balance: null, unit: null, permission: 'unknown' as const, missing_fields: ['has_credits', 'unlimited', 'balance', 'unit'], invalid_fields: [] };
    expect(quotaEvidenceText(quota({ state: 'available', buckets: [{ ...poolBucket, credits: unknownCredits }] })))
      .toContain('credits=has:Unknown,unlimited:Unknown,balance=Unknown,unit=Unknown,permission:unknown');
    // 人类可读面：credits 轴始终有一行，缺失也渲染 Unknown，绝不出现 0/100%。
    const axis = creditsAxisText(unknownCredits, t);
    expect(axis).toContain('额外 credits');
    expect(axis).toContain('余额: Unknown');
    expect(axis).toContain('单位: Unknown');
    expect(axis).toContain('缺字段: has_credits, unlimited, balance, unit');
    const missingAxis = creditsAxisText(null, t);
    expect(missingAxis).toContain('额外 credits');
    expect(missingAxis).toContain('有 credits: Unknown');
    expect(missingAxis).toContain('余额: Unknown');
    expect(missingAxis).not.toContain('缺字段');
    expect([axis, missingAxis].join(' ')).not.toMatch(/0%|100%|余额: 0|单位: 0/);
  });

  it('marks historical quota only when history is true and keeps the retained numbers', () => {
    expect(quotaHistoryLabel(false, '2026-09-30T12:00:00Z', t)).toBe('');
    expect(quotaHistoryLabel(undefined, '2026-09-30T12:00:00Z', t)).toBe('');
    expect(quotaHistoryLabel(true, '2026-09-30T12:00:00Z', t)).toBe('历史数据 · 最后成功更新 2026-09-30T12:00:00Z');
    expect(quotaHistoryLabel(true, null, t)).toBe('历史数据 · 最后成功更新 Unknown');
    const stale = quota({ state: 'failed', history: true, observed_at: '2026-09-30T12:00:00Z', buckets: [poolBucket] });
    const text = quotaEvidenceText(stale);
    expect(text).toContain('quota_state=failed');
    expect(text).toContain('history=true');
    // 历史数字仍然原样保留，不清零。
    expect(text).toContain('used=42,remaining=58');
    expect(subscriptionQuotaText(view({ quota: stale }))).toBe(text);
    expect(subscriptionQuotaText(undefined)).toBe(quotaEvidenceText(undefined));
  });
});
