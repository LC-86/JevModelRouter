import type {
  DashboardSnapshot, Provider, ProviderKind, SubscriptionAuthError, SubscriptionAuthPhase, SubscriptionAuthView,
  SubscriptionCapabilityStatus, SubscriptionConnectionState, SubscriptionDenial, SubscriptionEvidenceState,
  SubscriptionLocalClearing, SubscriptionLocalLogoutState, SubscriptionLoginStage, SubscriptionRemoteRevocation,
  SubscriptionRemoteRevokeState, SubscriptionView,
} from '../types';
import type { Translate } from './preferences-context';

const SUBSCRIPTION_KINDS: ProviderKind[] = ['codex_subscription', 'grok_subscription'];

export function isSubscriptionKind(kind: ProviderKind): boolean {
  return SUBSCRIPTION_KINDS.includes(kind);
}

export function isSubscriptionProvider(provider: Provider | undefined): boolean {
  return Boolean(provider && isSubscriptionKind(provider.kind));
}

export function subscriptionView(snapshot: DashboardSnapshot, providerId: string): SubscriptionView | undefined {
  return (snapshot.subscriptions ?? []).find(view => view.provider_id === providerId);
}

export function connectionStateLabel(state: SubscriptionConnectionState, t: Translate): string {
  return t({
    not_connected: 'Not connected',
    authorization_pending: 'Waiting for authorization',
    connected: 'Subscription connected',
    expired: 'Subscription expired',
  }[state]);
}

export function connectionStateTone(state: SubscriptionConnectionState): 'success' | 'warning' | 'error' {
  if (state === 'connected') return 'success';
  if (state === 'authorization_pending') return 'warning';
  return state === 'expired' ? 'error' : 'warning';
}

export function capabilityLabel(status: SubscriptionCapabilityStatus, t: Translate): string {
  return t({ unverified: 'Unverified', verified: 'Verified', unsupported: 'Unsupported' }[status]);
}

export function quotaLabel(state: SubscriptionEvidenceState, t: Translate): string {
  return t({
    unknown: 'Quota unknown',
    available: 'Quota evidence available',
    stale: 'Quota evidence stale',
    failed: 'Quota read failed',
    unsupported: 'No quota interface',
  }[state]);
}

/** 协议能力按模型与客户端协议分别记录；没有记录就是未验证，不推断。 */
export function modelCapability(view: SubscriptionView | undefined, modelId: string, protocol: string): SubscriptionCapabilityStatus {
  return view?.capabilities?.find(entry => entry.model_id === modelId && entry.protocol === protocol)?.status ?? 'unverified';
}

export function protocolKey(endpoint: string): string {
  if (endpoint === 'responses') return 'responses';
  if (endpoint === 'messages') return 'messages';
  return 'chat_completions';
}

/** 完整原因文本（后端英文原文），用作提示与日志详情。 */
export function subscriptionReason(view: SubscriptionView | undefined, t: Translate): string {
  if (!view) return '';
  if (view.denial) return `${view.denial.message} ${view.denial.recovery}`;
  if (view.quota.state !== 'available') return quotaLabel(view.quota.state, t);
  return '';
}

/** 行内摘要：按稳定 code 本地化，完整原文仍保留在提示里。 */
export function denialLabel(denial: SubscriptionDenial | null | undefined, t: Translate): string {
  if (!denial) return '';
  return t({
    provider_disabled: 'Provider disabled',
    model_disabled: 'Model disabled',
    not_connected: 'Not connected',
    authorization_pending: 'Waiting for authorization',
    authorization_expired: 'Subscription expired',
    identity_unverified: 'Identity not verified',
    evidence_missing: 'No read-only evidence',
    model_not_eligible: 'Model not eligible for this account',
    capability_unverified: 'Capability not verified',
    capability_unsupported: 'Capability unsupported',
    quota_unknown: 'Quota basis unknown',
    quota_stale: 'Quota basis stale',
    quota_failed: 'Quota read failed',
    quota_unsupported: 'No quota interface',
  }[denial.code] ?? denial.code);
}

export function identityLabel(view: SubscriptionView | undefined, t: Translate): string {
  const identity = view?.identity?.trim();
  return identity && identity.length > 0 ? identity : t('Unknown');
}

export function subscriptionAuthView(snapshot: DashboardSnapshot, providerId: string): SubscriptionAuthView | undefined {
  return (snapshot.subscription_auth ?? []).find(view => view.provider_id === providerId);
}

export function authPhaseLabel(phase: SubscriptionAuthPhase, t: Translate): string {
  return t({
    idle: 'No sign-in in progress',
    pending: 'Waiting for sign-in',
    succeeded: 'Sign-in succeeded',
    failed: 'Sign-in failed',
    cancelled: 'Sign-in cancelled',
  }[phase]);
}

/** 按稳定 code 本地化；未知 code 回退后端原文，不编造文案。 */
export function authErrorLabel(error: SubscriptionAuthError | null | undefined, t: Translate): string {
  if (!error) return '';
  return t({
    already_connected: 'This subscription is already connected. Sign out before signing in again.',
    helper_isolated: 'Sign-in is disabled in the isolated verification environment.',
    helper_missing: 'The Grok helper was not found on this machine.',
    helper_unsupported: 'Only the Grok helper is managed in this build; sign-in, sign-out and account switching are not implemented for this provider.',
    logout_superseded: 'The connection changed while signing out; nothing was cleared. Refresh to review the current state, then retry.',
  }[error.code] ?? (error.message || error.code));
}

export function logoutLocalLabel(state: SubscriptionLocalLogoutState, t: Translate): string {
  return t({
    not_attempted: 'Local clearing not attempted',
    cleared: 'Local credentials cleared',
    failed: 'Local clearing failed',
  }[state]);
}

/** 远端撤销只由远端结论决定：本地 cleared 绝不推断成远端已撤销。 */
export function remoteRevokeLabel(state: SubscriptionRemoteRevokeState, t: Translate): string {
  return t({
    not_attempted: 'Remote revoke not attempted',
    failed: 'Remote revoke failed',
    verified: 'Remote revoke verified',
    unsupported: 'Remote revoke unsupported',
  }[state]);
}

export interface SubscriptionPollGuard { phase: SubscriptionAuthPhase; busy: boolean; inFlight: boolean; stopped: boolean }
/** 轮询 tick 仅在仍处于 pending、没有其它命令在途、上次 poll 已返回且未被停止时才允许发起。 */
export function pollTickAllowed(guard: SubscriptionPollGuard): boolean {
  return guard.phase === 'pending' && !guard.busy && !guard.inFlight && !guard.stopped;
}

export interface SubscriptionCancelGuard { phase: SubscriptionAuthPhase; busy: boolean; pollInFlight: boolean }
/** poll 在途时禁止取消：两个请求交叉写回会互相覆盖刚写入的连接状态。 */
export function cancelAllowed(guard: SubscriptionCancelGuard): boolean {
  return guard.phase === 'pending' && !guard.busy && !guard.pollInFlight;
}

export interface SubscriptionPollResultGuard { phase: SubscriptionAuthPhase; requestEpoch: number; currentEpoch: number; stopped: boolean }
/** 迟到的 poll 响应必须丢弃：只有发起时的序号仍是当前序号、未被停止且仍处于 pending 才允许写快照。 */
export function pollResultAllowed(guard: SubscriptionPollResultGuard): boolean {
  return guard.phase === 'pending' && !guard.stopped && guard.requestEpoch === guard.currentEpoch;
}

/** 缺失或空白的只读证据一律显示 `Unknown`，不用界面文案猜测。 */
export function knownOrUnknown(value: string | null | undefined): string {
  const text = value?.trim();
  return text && text.length > 0 ? text : 'Unknown';
}

/** 官方辅助进程版本；缺失显示 `Unknown`。 */
export function helperVersionLabel(view: SubscriptionView | undefined): string {
  return knownOrUnknown(view?.helper?.version ?? view?.helper_version);
}

/** 专用授权目录（辅助进程的 CODEX_HOME）；缺失显示 `Unknown`。 */
export function authHomeLabel(view: SubscriptionView | undefined): string {
  return knownOrUnknown(view?.helper?.auth_home ?? view?.account_path);
}

export function loginStageLabel(stage: SubscriptionLoginStage, t: Translate): string {
  return t({
    idle: 'Sign-in idle',
    pending: 'Waiting for authorization',
    completed: 'Sign-in completed',
    failed: 'Sign-in failed',
    cancelled: 'Sign-in cancelled',
  }[stage]);
}

export function loginStageTone(stage: SubscriptionLoginStage): 'success' | 'warning' | 'error' {
  if (stage === 'completed') return 'success';
  if (stage === 'failed') return 'error';
  return 'warning';
}

/** 本地凭据清除结果；与远端撤销严格分开，不互相代替。 */
export function localLogoutLabel(local: SubscriptionLocalClearing, t: Translate): string {
  return t({ cleared: 'Local credentials cleared', retained: 'Local credentials retained' }[local]);
}

/** 远端撤销结果；失败、不支持与未知都不得显示成已撤销。 */
export function remoteRevocationLabel(remote: SubscriptionRemoteRevocation, t: Translate): string {
  return t({
    revoked: 'Remote authorization revoked',
    failed: 'Remote revocation failed',
    unsupported: 'Remote revocation unsupported',
    unknown: 'Remote revocation unknown',
  }[remote]);
}

/**
 * 订阅行固定状态文本：连接状态、世代、已核实身份、辅助进程版本与授权目录；
 * 有退出记录时追加本地清除与远端撤销结果。机器可读 token 与本地化文案同时保留。
 */
export function subscriptionStatusText(view: SubscriptionView | undefined, t: Translate): string {
  const state = view?.state ?? 'not_connected';
  const parts = [
    `state=${state}`,
    `(${connectionStateLabel(state, t)})`,
    `generation=${view?.generation ?? 0}`,
    `identity=${knownOrUnknown(view?.identity)}`,
    `helper=${helperVersionLabel(view)}`,
    `auth_home=${authHomeLabel(view)}`,
  ];
  if (view?.login) {
    parts.push(`login=${view.login.stage}`);
    // 挂起时必须能看到授权链接或 user code；两者都放进固定 selector 的文本里。
    if (view.login.stage === 'pending') {
      if (view.login.authorization_url?.trim()) parts.push(`authorization_url=${view.login.authorization_url.trim()}`);
      if (view.login.user_code?.trim()) parts.push(`user_code=${view.login.user_code.trim()}`);
    }
  }
  if (view?.logout) parts.push(`local=${view.logout.local}`, `remote=${view.logout.remote}`);
  return parts.join(' ');
}

/** 界面准入：真实生成保持后端默认拒绝，这里只决定登录/退出按钮可用性。 */
export function subscriptionActions(state: SubscriptionConnectionState): { canLogin: boolean; canCancel: boolean; canLogout: boolean } {
  return {
    canLogin: state === 'not_connected' || state === 'expired',
    canCancel: state === 'authorization_pending',
    canLogout: state === 'connected',
  };
}
