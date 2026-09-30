import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { PreferencesProvider, translate } from './preferences-context';
import { authErrorLabel, authPhaseLabel, cancelAllowed, logoutLocalLabel, pollResultAllowed, pollTickAllowed, remoteRevokeLabel, subscriptionAuthView } from './subscription';
import { SubscriptionAuthDialog } from '../components/subscription-auth-dialog';
import type {
  DashboardSnapshot, Provider, SubscriptionAuthView, SubscriptionLocalLogoutState, SubscriptionRemoteRevokeState,
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

// 渲染级用例：PreferencesProvider 需要 matchMedia 与 navigator，测试环境（node）里补齐这两个全局。
beforeAll(() => {
  vi.stubGlobal('matchMedia', () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  vi.stubGlobal('navigator', { language: 'zh-CN' });
});
afterAll(() => vi.unstubAllGlobals());

const provider: Provider = { id: 'grok-subscription', name: 'Grok subscription', kind: 'grok_subscription', base_url: '', enabled: true, has_api_key: false };

function renderDialog(view: SubscriptionAuthView): string {
  const authSnapshot = { subscription_auth: [view] } as unknown as DashboardSnapshot;
  return renderToStaticMarkup(createElement(
    PreferencesProvider, null,
    createElement(SubscriptionAuthDialog, { provider, snapshot: authSnapshot, onSnapshot: () => {}, onClose: () => {} }),
  ));
}

describe('subscription authorization dialog', () => {
  it('shows a local clear without ever claiming a remote revoke', () => {
    const html = renderDialog(authView({ generation: 1, attempt: 2, logout: { local: 'cleared', local_detail: 'helper home removed', remote: 'not_attempted' } }));
    expect(html).toContain('本地凭据已清除');
    expect(html).toContain('未尝试远端撤销');
    // 本地 cleared 绝不能渲染出远端已验证对应的任何文案。
    expect(html).not.toContain('远端撤销已验证');
    expect(html).not.toContain(t('Remote revoke verified'));
    // 世代与尝试编号必须与后端视图一致，手测据此核对 generation 不变／attempt 加一。
    expect(html).toContain('世代 1');
    expect(html).toContain('尝试 2');
  });

  it('shows the verified remote revoke only when the backend reports it', () => {
    const html = renderDialog(authView({ attempt: null, logout: { local: 'cleared', remote: 'verified' } }));
    expect(html).toContain('远端撤销已验证');
    expect(html).toContain('本地凭据已清除');
    expect(html).not.toContain('尝试');
    expect(html).toContain('世代 3');
  });
});

// 轮询与取消重叠的前端守卫：全部为纯函数，不用 jsdom／timer。
describe('subscription poll and cancel guards', () => {
  const tick = { phase: 'pending' as const, busy: false, inFlight: false, stopped: false };

  it('allows a poll tick only while pending, idle and not stopped', () => {
    expect(pollTickAllowed(tick)).toBe(true);
    expect(pollTickAllowed({ ...tick, busy: true })).toBe(false);
    expect(pollTickAllowed({ ...tick, inFlight: true })).toBe(false);
    expect(pollTickAllowed({ ...tick, stopped: true })).toBe(false);
    for (const phase of ['idle', 'succeeded', 'failed', 'cancelled'] as const) {
      expect(pollTickAllowed({ ...tick, phase })).toBe(false);
    }
  });

  it('blocks a cancel while busy or while a poll is in flight', () => {
    const cancel = { phase: 'pending' as const, busy: false, pollInFlight: false };
    expect(cancelAllowed(cancel)).toBe(true);
    expect(cancelAllowed({ ...cancel, busy: true })).toBe(false);
    expect(cancelAllowed({ ...cancel, pollInFlight: true })).toBe(false);
    for (const phase of ['idle', 'succeeded', 'failed', 'cancelled'] as const) {
      expect(cancelAllowed({ ...cancel, phase })).toBe(false);
    }
  });

  it('drops a poll response that arrives after a cancel superseded its epoch', () => {
    const result = { phase: 'pending' as const, requestEpoch: 4, currentEpoch: 4, stopped: false };
    expect(pollResultAllowed(result)).toBe(true);
    // cancel／logout 推进序号后，迟到的响应不得覆盖新状态。
    expect(pollResultAllowed({ ...result, currentEpoch: 5 })).toBe(false);
    expect(pollResultAllowed({ ...result, stopped: true })).toBe(false);
    expect(pollResultAllowed({ ...result, phase: 'cancelled' })).toBe(false);
  });
});
