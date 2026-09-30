import type {
  DashboardSnapshot, Model, Provider, ProviderKind, SubscriptionAuthError, SubscriptionAuthPhase, SubscriptionAuthView,
  SubscriptionCapabilityStatus, SubscriptionCatalogAvailability, SubscriptionCatalogEligibility, SubscriptionCatalogEntry,
  SubscriptionCatalogEvidence, SubscriptionCatalogState, SubscriptionConnectionState,
  SubscriptionDenial, SubscriptionEvidenceState, SubscriptionLocalClearing, SubscriptionLocalLogoutState,
  SubscriptionLoginStage, SubscriptionQuota, SubscriptionQuotaBucket, SubscriptionQuotaCredits, SubscriptionQuotaPermission,
  SubscriptionQuotaView,
  SubscriptionQuotaWindow, SubscriptionRemoteRevocation, SubscriptionRemoteRevokeState, SubscriptionView,
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

/** 已发现目录；缺失视为空目录，不用界面文案补造条目。 */
export function subscriptionCatalog(view: SubscriptionView | undefined): SubscriptionCatalogEntry[] {
  return view?.catalog_entries ?? [];
}

/** 上游可用性文案：读取失败、移除、撤销与未知分开呈现。 */
export function availabilityLabel(availability: SubscriptionCatalogAvailability | undefined, t: Translate): string {
  const key = {
    available: 'Available',
    stale: 'Stale (last confirmed read kept)',
    removed: 'Removed upstream',
    revoked: 'Revoked for this account',
    unknown: 'Availability unknown',
  }[availability as SubscriptionCatalogAvailability] ?? 'Availability unknown';
  return t(key);
}

/** 资格文案：不可用原因按稳定枚举分别说明，未知值回落为未知。 */
export function eligibilityLabel(eligibility: SubscriptionCatalogEligibility | undefined, t: Translate): string {
  const key = {
    eligible: 'Eligible',
    stale: 'Eligible (stale evidence)',
    not_discovered: 'Not discovered',
    removed: 'Removed from the upstream catalog',
    revoked: 'Access revoked',
    account_changed: 'Confirmed for another account',
    unknown: 'Eligibility unknown',
  }[eligibility as SubscriptionCatalogEligibility] ?? 'Eligibility unknown';
  return t(key);
}

/** 目录行的可用性 + 资格摘要。 */
export function catalogEntryStateLabel(entry: SubscriptionCatalogEntry, t: Translate): string {
  return `${availabilityLabel(entry.availability, t)} · ${eligibilityLabel(entry.eligibility, t)}`;
}

/** 资格是否成立：同账号同世代的陈旧资格仍成立；网络失败不等于被移除。 */
export function catalogEntryEligible(entry: Pick<SubscriptionCatalogEntry, 'eligibility'>): boolean {
  return entry.eligibility === 'eligible' || entry.eligibility === 'stale';
}

/** 目录行是否进入模型列表与自动候选：已选、未停用、资格成立。 */
export function catalogEntryInPool(entry: Pick<SubscriptionCatalogEntry, 'selected' | 'disabled' | 'eligibility'>): boolean {
  return entry.selected && !entry.disabled && catalogEntryEligible(entry);
}

/** 按稳定内部标识找本地模型行；显示名不参与匹配，改名不改变调用目标。 */
export function catalogModelRow(models: Model[], entry: Pick<SubscriptionCatalogEntry, 'internal_id'>): Model | undefined {
  return models.find(model => model.id === entry.internal_id);
}

/**
 * 用户是否选择了该模型。缺省视为已选：旧配置与 API 模型不因新增字段被移出列表。
 */
export function modelSelected(model: Pick<Model, 'selected'>): boolean {
  return model.selected !== false;
}

export function selectedModels<M extends { selected?: boolean }>(models: M[]): M[] {
  return models.filter(model => modelSelected(model));
}

/** 未选模型：仍可按原 ID 直调，但不进入列表、计数与任何候选。 */
export function unselectedModels<M extends { selected?: boolean }>(models: M[]): M[] {
  return models.filter(model => !modelSelected(model));
}

/** 模型列表成员：只由用户选择决定；停用模型仍在列表中出现并由行内标注停用。 */
export function modelListMembers(models: Model[]): Model[] {
  return selectedModels(models);
}

/** 测速候选：已选、未停用、服务商启用。 */
export function speedTestCandidates(models: Model[], providers: Provider[]): Model[] {
  return models.filter(model =>
    modelSelected(model) && model.enabled && providers.some(provider => provider.id === model.provider_id && provider.enabled));
}

/** Agent 可选模型与测速候选同一条规则：未选模型不得进入注入目录。 */
export function agentSelectableModels(models: Model[], providers: Provider[]): Model[] {
  return speedTestCandidates(models, providers);
}

export interface AgentCatalogSyncState {
  /** 已保存到磁盘的注入目录绑定（默认项在前）。 */
  saved: readonly string[];
  /** 当前应用内选择的绑定。 */
  chosen: readonly string[];
  /** 后端给出的待同步标记；缺省由绑定差异推断。 */
  pendingSync?: boolean;
}
/**
 * 保存目录与当前应用内选择是否不一致：不一致即待同步，需要重新连接才能刷新外部配置。
 * 这只是外部配置的同步状态；停用与撤销始终由后端立即生效，不等待这里。
 */
export function agentCatalogPendingSync(state: AgentCatalogSyncState): boolean {
  if (state.pendingSync === true) return true;
  const saved = [...new Set(state.saved)];
  const chosen = [...new Set(state.chosen)];
  return saved.length !== chosen.length || saved.some((binding, index) => binding !== chosen[index]);
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
    unsupported: 'No catalog interface',
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

/** Extra-credit permission is a separate billing axis from the included quota permit. */
export function extraUsagePermissionLabel(permission: SubscriptionQuotaPermission | undefined, t: Translate): string {
  return t({
    allowed: 'Extra credit use allowed',
    denied: 'Extra credit use prohibited',
    unknown: 'Extra credit restriction unknown',
  }[permission ?? 'unknown']);
}

/** 额度视图文案：多桶 / 旧版单桶 / 未知。 */
export function quotaViewLabel(view: SubscriptionQuotaView | undefined, t: Translate): string {
  return t({
    rate_limits_by_limit_id: 'Multiple quota buckets',
    rate_limits: 'Legacy single quota bucket',
    grok_cli_usage: 'Grok CLI usage',
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
    ...(credits?.invalid_fields ?? []),
  ]);
  return [
    `bucket=${knownOrUnknown(bucket.limit_id)}`,
    `windows=${windows.map(quotaWindowToken).join(',')}`,
    `credits=has:${stableBoolean(credits?.has_credits)},unlimited:${stableBoolean(credits?.unlimited)},balance:${knownOrUnknown(credits?.balance)},unit:${knownOrUnknown(credits?.unit)},permission:${credits?.permission ?? 'unknown'}`,
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

/** 已移除模型 token：上一次已核实目录里有、本次权威结果里已不存在的模型标识。 */
function catalogRemovedToken(catalog: SubscriptionCatalogEvidence | null | undefined): string {
  return `removed=${stableList(catalog?.removed_models)}`;
}

/**
 * 目录证据的机器可读稳定文本：
 * `catalog_state=<state> source=<..> observed_at=<..> missing=<..>`。
 */
export function catalogText(catalog: SubscriptionCatalogEvidence | null | undefined): string {
  return [catalogHead(catalog), catalogRemovedToken(catalog), catalogMissingToken(catalog)].join(' ');
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
  return [catalogHead(catalog), `models=${models}`, catalogRemovedToken(catalog), catalogMissingToken(catalog)].join(' ');
}

/** 已移除模型的用户文案；空列表不产生任何文案，不把「移除」混进「不具备资格」。 */
export function catalogRemovedLabel(removed: string[] | null | undefined, t: Translate): string {
  const ids = (removed ?? []).map(id => id.trim()).filter(id => id.length > 0);
  if (ids.length === 0) return '';
  return t('Removed from the previous catalog: {models}', { models: ids.join(', ') });
}

/** 官方查看入口：只供人工查看，绝不作为绕过准入的依据；没有可靠官方入口的服务商不新增链接。 */
export const CATALOG_REFERENCE_URL = 'https://docs.x.ai/build/cli/reference';
export const QUOTA_REFERENCE_URL = 'https://docs.x.ai/grok/faq#usage--limits';

/**
 * 额外 credits 轴的用户文案：未知一律 `Unknown`，单位与余额原样显示，缺失/越界字段照实列出。
 * `credits` 为 null/undefined 时也照实输出一整行未知，而不是让这一轴消失。
 */
export function creditsAxisText(credits: SubscriptionQuotaCredits | null | undefined, t: Translate): string {
  const parts = [
    `${t('Has credits')}: ${stableBoolean(credits?.has_credits)}`,
    `${t('Unlimited')}: ${stableBoolean(credits?.unlimited)}`,
    `${t('Balance')}: ${knownOrUnknown(credits?.balance)}`,
    `${t('Unit')}: ${knownOrUnknown(credits?.unit)}`,
    extraUsagePermissionLabel(credits?.permission, t),
  ];
  const missing = credits?.missing_fields ?? [];
  const invalid = credits?.invalid_fields ?? [];
  if (missing.length > 0) parts.push(`${t('Missing fields')}: ${missing.join(', ')}`);
  if (invalid.length > 0) parts.push(`${t('Invalid fields')}: ${invalid.join(', ')}`);
  return `${t('Credits')} · ${parts.join(' · ')}`;
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
  const denial = view.denial ?? view.admission_denial;
  if (denial) return `${denial.message} ${denial.recovery}`;
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
    model_not_discovered: 'Model is not in this account directory',
    model_removed: 'Model was removed upstream',
    model_revoked: 'Model access revoked for this account',
    model_unqualified: 'Model qualification stale for this account',
    capability_unverified: 'Capability not verified',
    capability_unsupported: 'Capability unsupported',
    quota_unknown: 'Quota basis unknown',
    quota_stale: 'Quota basis stale',
    quota_failed: 'Quota read failed',
    quota_denied: 'Quota access denied',
    quota_unsupported: 'No quota interface',
    extra_usage_allowed: 'Extra credits are currently allowed',
    extra_usage_permission_unknown: 'Extra credit restriction unverified',
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
