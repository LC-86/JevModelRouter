import type {
  DashboardSnapshot, Provider, ProviderKind, SubscriptionCapabilityStatus,
  SubscriptionConnectionState, SubscriptionDenial, SubscriptionEvidenceState, SubscriptionView,
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
