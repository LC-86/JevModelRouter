import { describe, expect, it } from 'vitest';
import { translate } from './preferences-context';
import {
  authHomeLabel, capabilityLabel, connectionStateLabel, connectionStateTone, denialLabel, helperVersionLabel,
  identityLabel, isSubscriptionProvider, knownOrUnknown, localLogoutLabel, loginStageLabel, loginStageTone,
  modelCapability, protocolKey, quotaLabel, remoteRevocationLabel, subscriptionActions, subscriptionReason,
  subscriptionStatusText, subscriptionView,
} from './subscription';
import type { DashboardSnapshot, SubscriptionView } from '../types';

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
