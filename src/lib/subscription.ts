import type {
  DashboardSnapshot, Provider, ProviderKind, SubscriptionAuthError, SubscriptionAuthPhase, SubscriptionAuthView,
  SubscriptionCapabilityStatus, SubscriptionCatalogEvidence, SubscriptionCatalogState, SubscriptionConnectionState,
  SubscriptionDenial, SubscriptionEvidenceState, SubscriptionLocalClearing, SubscriptionLocalLogoutState,
  SubscriptionLoginStage, SubscriptionModel, SubscriptionQuota, SubscriptionQuotaBucket, SubscriptionQuotaCredits,
  SubscriptionQuotaPermission, SubscriptionQuotaView, SubscriptionQuotaWindow, SubscriptionRemoteRevocation,
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
    denied: 'Quota denied',
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

/** 官方查看入口：只供人工查看当前支持情况，绝不作为绕过准入的依据。 */
export const CATALOG_REFERENCE_URL = 'https://docs.x.ai/build/cli/reference';
export const QUOTA_REFERENCE_URL = 'https://docs.x.ai/grok/faq#usage--limits';

export function catalogLabel(state: SubscriptionCatalogState, t: Translate): string {
  return t({
    unknown: 'Catalog unknown',
    available: 'Catalog available',
    stale: 'Catalog evidence stale',
    failed: 'Catalog read failed',
    unsupported: 'No catalog interface',
  }[state]);
}

export function quotaPermissionLabel(permission: SubscriptionQuotaPermission, t: Translate): string {
  return t({
    unknown: 'Quota permission unknown',
    allowed: 'Quota permission allowed',
    denied: 'Quota permission denied',
  }[permission]);
}

export function quotaViewLabel(view: SubscriptionQuotaView, t: Translate): string {
  return t({
    unknown: 'Quota view unknown',
    rate_limits_by_limit_id: 'Rate limits by limit id',
    rate_limits: 'Rate limits',
    grok_cli_usage: 'Grok CLI usage',
  }[view]);
}

/** 历史数据必须显式标注；`history` 不是 `true` 就不出这句话，也不清零保留的数字。 */
export function quotaHistoryLabel(history: boolean | null | undefined, observedAt: string | null | undefined, t: Translate): string {
  if (history !== true) return '';
  return t('Historical data · last successful update {observed_at}', { observed_at: knownOrUnknown(observedAt) });
}

/** 剩余额度只由有效 `used_percent` 推导；缺失、NaN/Infinity、越界一律 null，绝不回退成 0。 */
export function remainingPercent(usedPercent: number | null | undefined): number | null {
  if (usedPercent == null || !Number.isFinite(usedPercent)) return null;
  if (usedPercent < 0 || usedPercent > 100) return null;
  return Number((100 - usedPercent).toPrecision(12));
}

/** 数值 token：缺失或非有限数值一律 `Unknown`，不用 0 顶替。 */
export function evidenceNumberText(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return 'Unknown';
  return String(value);
}

/** 布尔 token：非布尔（缺失/null）一律 `Unknown`，不把缺失当成 false。 */
export function booleanText(value: boolean | null | undefined): string {
  if (typeof value !== 'boolean') return 'Unknown';
  return value ? 'true' : 'false';
}

/** 百分比展示：未知不加 `%` 后缀，避免看起来像已核实的数字。 */
export function percentText(value: number | null | undefined): string {
  const text = evidenceNumberText(value);
  return text === 'Unknown' ? text : `${text}%`;
}

/**
 * 桶级许可聚合：任一桶 denied → denied；全部桶 allowed → allowed；其余（含空桶）unknown。
 * 与后端契约 B 的额度状态规则同向，且不把 `credits.permission` 混进这个计量轴。
 */
export function quotaPermission(quota: SubscriptionQuota | undefined): SubscriptionQuotaPermission {
  const buckets = quota?.buckets ?? [];
  if (buckets.some(bucket => (bucket.permission ?? 'unknown') === 'denied')) return 'denied';
  if (buckets.length > 0 && buckets.every(bucket => bucket.permission === 'allowed')) return 'allowed';
  return 'unknown';
}

/** 目录条目的资格文案：资格只来自后端证据；`eligible=false` 只说明不具备资格，绝不等同可调用。 */
export function modelEligibilityLabel(eligible: boolean, t: Translate): string {
  return eligible ? t('Eligible for this account') : t('Not eligible for this account');
}

function csv(fields: string[] | null | undefined): string {
  return (fields ?? []).join(',');
}

/**
 * 目录 node 的稳定机器文本（契约 §7）：token 顺序即契约，界面不得重排或插入本地化文案。
 * `models` 只输出 `model_id:eligible`，目录发现不等于可调用；`removed` 是这次整次替换后消失的模型。
 */
export function catalogText(catalog: SubscriptionCatalogEvidence | undefined, models: SubscriptionModel[]): string {
  const entries = models.map(model => `${model.model_id}:${model.eligible ? 'true' : 'false'}`);
  return [
    `catalog_state=${catalog?.state ?? 'unknown'}`,
    `source=${knownOrUnknown(catalog?.source)}`,
    `observed_at=${knownOrUnknown(catalog?.observed_at)}`,
    `models=${entries.join(',')}`,
    `removed=${csv(catalog?.removed_models)}`,
    `missing=${csv(catalog?.missing_fields)}`,
  ].join(' ');
}

/** 被移除的模型只说明「这次目录里没有了」，绝不写成不具备资格；为空时不出这一行。 */
export function catalogRemovedLabel(removed: string[] | null | undefined, t: Translate): string {
  const ids = (removed ?? []).map(id => id.trim()).filter(id => id.length > 0);
  if (ids.length === 0) return '';
  return t('Removed from the previous catalog: {models}', { models: ids.join(', ') });
}

export function subscriptionCatalogText(view: SubscriptionView | undefined): string {
  return catalogText(view?.catalog ?? undefined, view?.models ?? []);
}

function windowText(window: SubscriptionQuotaWindow): string {
  return [
    `${window.label}:used=${evidenceNumberText(window.used_percent)}`,
    `remaining=${evidenceNumberText(remainingPercent(window.used_percent))}`,
    `minutes=${evidenceNumberText(window.window_minutes)}`,
    `resets_at=${evidenceNumberText(window.resets_at)}`,
  ].join(',');
}

/** credits 两个轴分开：balance/unit 是上游原文，permission 只是该 credits 的许可，不参与额度状态判定。 */
function creditsText(credits: SubscriptionQuotaCredits | null | undefined): string {
  return [
    `has:${booleanText(credits?.has_credits)}`,
    `unlimited:${booleanText(credits?.unlimited)}`,
    `balance=${knownOrUnknown(credits?.balance)}`,
    `unit=${knownOrUnknown(credits?.unit)}`,
    `permission:${credits?.permission ?? 'unknown'}`,
  ].join(',');
}

/**
 * credits 轴的人类可读文本：后端完全没给 credits（null/undefined）时也要渲染一行诚实未知，
 * 绝不整段消失；未知字段一律 `Unknown`，不出现 0/100% 之类的伪造值。
 */
export function creditsAxisText(credits: SubscriptionQuotaCredits | null | undefined, t: Translate): string {
  const parts = [
    `${t('Has credits')}: ${booleanText(credits?.has_credits)}`,
    `${t('Unlimited')}: ${booleanText(credits?.unlimited)}`,
    `${t('Balance')}: ${knownOrUnknown(credits?.balance)}`,
    `${t('Unit')}: ${knownOrUnknown(credits?.unit)}`,
    quotaPermissionLabel(credits?.permission ?? 'unknown', t),
  ];
  const missing = credits?.missing_fields ?? [];
  const invalid = credits?.invalid_fields ?? [];
  if (missing.length > 0) parts.push(`${t('Missing fields')}: ${missing.join(', ')}`);
  if (invalid.length > 0) parts.push(`${t('Invalid fields')}: ${invalid.join(', ')}`);
  return `${t('Credits')} · ${parts.join(' · ')}`;
}

function bucketText(bucket: SubscriptionQuotaBucket): string {
  const windows = (bucket.windows ?? []).map(windowText).join(';');
  return [
    `bucket=${bucket.limit_id}`,
    `windows=<${windows}>`,
    `credits=${creditsText(bucket.credits)}`,
    `missing=${csv(bucket.missing_fields)}`,
    `invalid=${csv(bucket.invalid_fields)}`,
  ].join(' ');
}

/** 额度 node 的稳定机器文本（契约 §7）：未读取过时只到顶层 `missing=` 为止，没有 bucket 段。 */
export function quotaEvidenceText(quota: SubscriptionQuota | undefined): string {
  const parts = [
    `quota_state=${quota?.state ?? 'unknown'}`,
    `permission=${quotaPermission(quota)}`,
    `source=${knownOrUnknown(quota?.source)}`,
    `observed_at=${knownOrUnknown(quota?.observed_at)}`,
    `history=${quota?.history === true ? 'true' : 'false'}`,
    `missing=${csv(quota?.missing_fields)}`,
  ];
  for (const bucket of quota?.buckets ?? []) parts.push(bucketText(bucket));
  return parts.join(' ');
}

export function subscriptionQuotaText(view: SubscriptionView | undefined): string {
  return quotaEvidenceText(view?.quota);
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
    quota_denied: 'Quota basis denied',
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

/** 已知稳定 code 的本地化文案；新增 code 只在这里登记一次。 */
const AUTH_ERROR_LABELS: Record<string, string> = {
  already_connected: 'This subscription is already connected. Sign out before signing in again.',
  helper_isolated: 'Sign-in is disabled in the isolated verification environment.',
  helper_missing: 'The Grok helper was not found on this machine.',
  helper_unsupported: 'Only the Grok helper is managed in this build; sign-in, sign-out and account switching are not implemented for this provider.',
  logout_superseded: 'The connection changed while signing out; nothing was cleared. Refresh to review the current state, then retry.',
};

/** 按稳定 code 本地化；未知 code 回退后端原文，不编造文案。 */
export function authErrorLabel(error: SubscriptionAuthError | null | undefined, t: Translate): string {
  if (!error) return '';
  return t(AUTH_ERROR_LABELS[error.code] ?? (error.message || error.code));
}

export interface SubscriptionCommandErrorLabel { label: string; detail: string | null }
/** 后端命令 Err 形如 `<code>: <message>`（refusal()）。 */
const COMMAND_ERROR_PREFIX = /^([a-z][a-z0-9_]*)\s*:\s*([\s\S]*)$/;

/**
 * 命令错误文本的展示拆分：前缀是已知 code 时给出本地化 label，并把后端原文留在 detail 供核对；
 * 没有前缀或前缀不是已知 code 时原样返回（label = 原文，detail = null），不猜测。
 */
export function commandErrorLabel(text: string, t: Translate): SubscriptionCommandErrorLabel {
  const raw = text.trim();
  const match = COMMAND_ERROR_PREFIX.exec(raw);
  const code = match?.[1];
  const message = match?.[2] ?? '';
  if (!code || AUTH_ERROR_LABELS[code] === undefined) return { label: raw, detail: null };
  return { label: authErrorLabel({ code, message, recovery: '' }, t), detail: raw };
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
