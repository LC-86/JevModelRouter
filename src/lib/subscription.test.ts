import { describe, expect, it } from 'vitest';
import { translate } from './preferences-context';
import {
  agentCatalogPendingSync, agentSelectableModels, authHomeLabel, availabilityLabel, capabilityLabel,
  catalogEntryEligible, catalogEntryInPool, catalogEntryStateLabel, catalogModelRow, connectionStateLabel,
  connectionStateTone, denialLabel, eligibilityLabel, helperVersionLabel,
  identityLabel, isSubscriptionProvider, knownOrUnknown, localLogoutLabel, loginStageLabel, loginStageTone,
  modelCapability, modelListMembers, modelSelected, protocolKey, quotaLabel, remoteRevocationLabel,
  speedTestCandidates, subscriptionActions, subscriptionCatalog, subscriptionReason,
  subscriptionStatusText, subscriptionView, unselectedModels,
} from './subscription';
import type { DashboardSnapshot, Model, Provider, SubscriptionCatalogEntry, SubscriptionView } from '../types';

const t = (message: string) => translate('zh-CN', message);

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
    expect(subscriptionCatalog(view({ catalog: [entry()] }))).toHaveLength(1);
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
