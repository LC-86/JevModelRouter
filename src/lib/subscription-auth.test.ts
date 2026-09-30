import { describe, expect, it } from 'vitest';
import { translate } from './preferences-context';
import { authErrorLabel, authPhaseLabel, logoutLocalLabel, remoteRevokeLabel, subscriptionAuthView } from './subscription';
import type {
  DashboardSnapshot, SubscriptionAuthView, SubscriptionLocalLogoutState, SubscriptionRemoteRevokeState,
} from '../types';

const t = (message: string) => translate('zh-CN', message);

function authView(patch: Partial<SubscriptionAuthView> = {}): SubscriptionAuthView {
  return {
    provider_id: 'grok-subscription', phase: 'idle', generation: 3, attempt: null, challenge: null, identity: null, error: null,
    helper: { available: false, version: null, program: null, home: null },
    logout: { local: 'not_attempted', remote: 'not_attempted' },
    ...patch,
  };
}

const snapshot = { subscription_auth: [authView()] } as unknown as DashboardSnapshot;

describe('subscription authorization views', () => {
  it('labels every sign-in phase', () => {
    expect(authPhaseLabel('idle', t)).toBe('没有进行中的登录');
    expect(authPhaseLabel('pending', t)).toBe('等待登录');
    expect(authPhaseLabel('succeeded', t)).toBe('登录成功');
    expect(authPhaseLabel('failed', t)).toBe('登录失败');
    expect(authPhaseLabel('cancelled', t)).toBe('登录已取消');
  });

  it('localizes known error codes and falls back to the backend text', () => {
    expect(authErrorLabel({ code: 'helper_missing', message: 'grok not found on PATH', recovery: 'Install the official CLI.' }, t)).toBe('本机未找到 Grok 辅助进程。');
    expect(authErrorLabel({ code: 'helper_isolated', message: '', recovery: '' }, t)).toBe('隔离验证环境中禁用登录。');
    expect(authErrorLabel({ code: 'already_connected', message: '', recovery: '' }, t)).toBe('该订阅已连接，请先退出登录再重新登录。');
    // 非 Grok 订阅（如 Codex）在本构建未实现：已知 code 走本地化映射，而不是后端英文原文。
    expect(authErrorLabel({ code: 'helper_unsupported', message: 'helper_unsupported: Codex subscription authorization is not implemented in this build; only the Grok CLI helper is managed', recovery: '' }, t)).toBe('本构建只管理 Grok 辅助进程；该订阅服务商的登录、退出与换号尚未实现。');
    // 未知 code 原样显示后端原文，不编造文案。
    expect(authErrorLabel({ code: 'future_failure', message: 'Upstream said no.', recovery: '' }, t)).toBe('Upstream said no.');
    expect(authErrorLabel({ code: 'future_failure', message: '', recovery: '' }, t)).toBe('future_failure');
    expect(authErrorLabel(null, t)).toBe('');
  });

  it('never infers a remote revoke from a local clear', () => {
    const locals: SubscriptionLocalLogoutState[] = ['not_attempted', 'cleared', 'failed'];
    const remotes: SubscriptionRemoteRevokeState[] = ['not_attempted', 'failed', 'verified', 'unsupported'];
    const localLabels = new Set(locals.map(state => logoutLocalLabel(state, t)));
    const remoteLabels = remotes.map(state => remoteRevokeLabel(state, t));
    expect(localLabels.size).toBe(3);
    expect(new Set(remoteLabels).size).toBe(4);
    for (const label of remoteLabels) expect(localLabels.has(label)).toBe(false);
    expect(logoutLocalLabel('cleared', t)).toBe('本地凭据已清除');
    expect(remoteRevokeLabel('not_attempted', t)).toBe('未尝试远端撤销');
    expect(remoteRevokeLabel('verified', t)).toBe('远端撤销已验证');
    // 本地已清除时，远端结论仍必须是它自己的状态。
    const cleared = authView({ logout: { local: 'cleared', remote: 'not_attempted' } });
    expect(logoutLocalLabel(cleared.logout.local, t)).toBe('本地凭据已清除');
    expect(remoteRevokeLabel(cleared.logout.remote, t)).toBe('未尝试远端撤销');
  });

  it('reads only the authorization view of the requested provider', () => {
    expect(subscriptionAuthView(snapshot, 'grok-subscription')?.generation).toBe(3);
    expect(subscriptionAuthView(snapshot, 'codex-subscription')).toBeUndefined();
    expect(subscriptionAuthView({} as DashboardSnapshot, 'grok-subscription')).toBeUndefined();
  });

  it('keeps local clearing and remote revoke as separate evidence', () => {
    const view = authView({ logout: { local: 'cleared', local_detail: 'home removed', remote: 'failed', remote_detail: 'revoke endpoint refused' } });
    expect(view.logout.local).toBe('cleared');
    expect(view.logout.remote).toBe('failed');
    expect(logoutLocalLabel(view.logout.local, t)).not.toBe(remoteRevokeLabel(view.logout.remote, t));
    expect(view.helper.available).toBe(false);
  });
});