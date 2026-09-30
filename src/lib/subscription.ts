import type {
  DashboardSnapshot, Provider, ProviderKind, SubscriptionCapabilityStatus,
  SubscriptionCatalogEvidence, SubscriptionCatalogState, SubscriptionConnectionState, SubscriptionDenial,
  SubscriptionEvidenceState, SubscriptionLocalClearing, SubscriptionLoginStage, SubscriptionQuota,
  SubscriptionQuotaBucket, SubscriptionQuotaPermission, SubscriptionQuotaView, SubscriptionQuotaWindow,
  SubscriptionRemoteRevocation, SubscriptionView,
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
    denied: 'Quota access denied',
  }[state]);
}

/** 目录状态文案；未知就是未知，不用“已关闭”之类的推测替代。 */
export function catalogLabel(state: SubscriptionCatalogState, t: Translate): string {
  return t({
    unknown: 'Catalog unknown',
    available: 'Catalog available',
    stale: 'Catalog stale',
    failed: 'Catalog read failed',
  }[state]);
}

/** 额度许可文案：allowed/denied/unknown 三态分明，unknown 必须显示为未知。 */
export function quotaPermissionLabel(permission: SubscriptionQuotaPermission | undefined, t: Translate): string {
  return t({
    allowed: 'Quota usage allowed',
    denied: 'Quota usage denied',
    unknown: 'Quota permission unknown',
  }[permission ?? 'unknown']);
}

/** 额度视图文案：多桶 / 旧版单桶 / 未知。 */
export function quotaViewLabel(view: SubscriptionQuotaView | undefined, t: Translate): string {
  return t({
    rate_limits_by_limit_id: 'Multiple quota buckets',
    rate_limits: 'Legacy single quota bucket',
    unknown: 'Quota view unknown',
  }[view ?? 'unknown']);
}

/** history=true 时必须显示为历史数据与最后成功更新时间；否则不产生文案。 */
export function quotaHistoryLabel(quota: SubscriptionQuota | undefined, t: Translate): string {
  if (quota?.history !== true) return '';
  const observedAt = quota.observed_at?.trim();
  return t('Historical data · last successful update {time}', { time: observedAt && observedAt.length > 0 ? observedAt : t('Unknown') });
}

function stableNumber(value: number | null | undefined): string {
  return typeof value === 'number' && Number.isFinite(value) ? String(value) : 'Unknown';
}

function stableBoolean(value: boolean | null | undefined): string {
  return typeof value === 'boolean' ? String(value) : 'Unknown';
}

function stableList(values: (string | null | undefined)[] | undefined): string {
  return (values ?? []).filter((value): value is string => typeof value === 'string' && value.length > 0).join(',');
}

/** used 的有效区间：有限数字且 0..=100；其余（缺失、越界、NaN/Infinity）一律无效。 */
function validUsedPercent(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 100 ? value : null;
}

/**
 * 剩余百分比只能由 `100 - usedPercent` 推导，且只在 used 有效（有限且 0..=100）时给出；
 * used 未知/越界时返回 null，界面不得回退成 0。
 */
export function remainingPercent(usedPercent: number | null | undefined): number | null {
  const used = validUsedPercent(usedPercent);
  return used === null ? null : 100 - used;
}

function percentText(value: number): string {
  return String(Math.round(value * 10000) / 10000);
}

/** 单个额度窗口的稳定 token：`label:used=..,remaining=..,minutes=..,resets_at=..`。 */
export function quotaWindowToken(entry: SubscriptionQuotaWindow): string {
  const used = validUsedPercent(entry.used_percent);
  const remaining = remainingPercent(entry.used_percent);
  return [
    `${knownOrUnknown(entry.label)}:used=${used === null ? 'Unknown' : percentText(used)}`,
    `remaining=${remaining === null ? 'Unknown' : percentText(remaining)}`,
    `minutes=${stableNumber(entry.window_minutes)}`,
    `resets_at=${stableNumber(entry.resets_at)}`,
  ].join(',');
}

/** 桶级许可聚合：任一桶 denied 即 denied；全部 allowed 才 allowed；否则 unknown（与状态规则一致）。 */
function aggregatePermission(buckets: SubscriptionQuotaBucket[]): SubscriptionQuotaPermission {
  if (buckets.some(bucket => (bucket.permission ?? 'unknown') === 'denied')) return 'denied';
  if (buckets.length > 0 && buckets.every(bucket => bucket.permission === 'allowed')) return 'allowed';
  return 'unknown';
}

function quotaBucketText(bucket: SubscriptionQuotaBucket): string {
  const windows = bucket.windows ?? [];
  const credits = bucket.credits;
  const missing = stableList([
    ...(bucket.missing_fields ?? []),
    ...(credits?.missing_fields ?? []),
    ...windows.flatMap(window => window.missing_fields ?? []),
  ]);
  const invalid = stableList([
    ...(bucket.invalid_fields ?? []),
    ...windows.flatMap(window => window.invalid_fields ?? []),
  ]);
  return [
    `bucket=${knownOrUnknown(bucket.limit_id)}`,
    `windows=${windows.map(quotaWindowToken).join(',')}`,
    `credits=has:${stableBoolean(credits?.has_credits)},unlimited:${stableBoolean(credits?.unlimited)},balance:${knownOrUnknown(credits?.balance)}`,
    `missing=${missing}`,
    `invalid=${invalid}`,
  ].join(' ');
}

/**
 * 额度证据的机器可读稳定文本（隔离检查用）。单行输出：
 * `quota_state=<state> quota_view=<view> permission=<allowed|denied|unknown> source=<..> observed_at=<..> history=<bool> missing=<..>`
 * 以及每个桶 `bucket=<limit_id> windows=<label:used=..,remaining=..,minutes=..,resets_at=..>[,...] credits=has:..,unlimited:..,balance:.. missing=.. invalid=..`。
 */
export function quotaEvidenceText(quota: SubscriptionQuota | null | undefined): string {
  const buckets = quota?.buckets ?? [];
  const header = [
    `quota_state=${quota?.state ?? 'unknown'}`,
    `quota_view=${quota?.view ?? 'unknown'}`,
    `permission=${aggregatePermission(buckets)}`,
    `source=${knownOrUnknown(quota?.source)}`,
    `observed_at=${knownOrUnknown(quota?.observed_at)}`,
    `history=${quota?.history === true ? 'true' : 'false'}`,
    `missing=${stableList(quota?.missing_fields)}`,
  ].join(' ');
  return [header, ...buckets.map(quotaBucketText)].join(' ');
}

/** 订阅视图的额度稳定文本；视图或额度缺失时按全 unknown 输出。 */
export function subscriptionQuotaText(view: SubscriptionView | undefined): string {
  return quotaEvidenceText(view?.quota);
}

/** 目录稳定文本的公共头部 token；`catalogText` 与 `subscriptionCatalogText` 共用，避免两处漂移。 */
function catalogHead(catalog: SubscriptionCatalogEvidence | null | undefined): string {
  return [
    `catalog_state=${catalog?.state ?? 'unknown'}`,
    `source=${knownOrUnknown(catalog?.source)}`,
    `observed_at=${knownOrUnknown(catalog?.observed_at)}`,
  ].join(' ');
}

function catalogMissingToken(catalog: SubscriptionCatalogEvidence | null | undefined): string {
  return `missing=${stableList(catalog?.missing_fields)}`;
}

/**
 * 目录证据的机器可读稳定文本：
 * `catalog_state=<state> source=<..> observed_at=<..> missing=<..>`。
 */
export function catalogText(catalog: SubscriptionCatalogEvidence | null | undefined): string {
  return [catalogHead(catalog), catalogMissingToken(catalog)].join(' ');
}

/**
 * 订阅视图的目录稳定文本：
 * `catalog_state=<state> source=<..> observed_at=<..> models=<id:eligible,...> missing=<..>`。
 * 与 `catalogText` 组合同一份头部与 missing token，`models=` 必须插在 missing 之前以保持冻结顺序。
 * `eligible` 原样反映后端判断，本票不把它标成可调用。
 */
export function subscriptionCatalogText(view: SubscriptionView | undefined): string {
  const catalog = view?.catalog;
  const models = (view?.models ?? []).map(model => `${model.model_id}:${model.eligible === true ? 'true' : 'false'}`).join(',');
  return [catalogHead(catalog), `models=${models}`, catalogMissingToken(catalog)].join(' ');
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
  if ((view.quota?.state ?? 'unknown') !== 'available') return quotaLabel(view.quota?.state ?? 'unknown', t);
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
    quota_denied: 'Quota access denied',
    quota_unsupported: 'No quota interface',
  }[denial.code] ?? denial.code);
}

export function identityLabel(view: SubscriptionView | undefined, t: Translate): string {
  const identity = view?.identity?.trim();
  return identity && identity.length > 0 ? identity : t('Unknown');
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
