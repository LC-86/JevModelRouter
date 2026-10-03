export type ProviderKind = 'openrouter' | 'ollama' | 'openai_compatible' | 'codex_subscription' | 'grok_subscription';
export type ModelTier = 'fast' | 'balanced' | 'strong';
export type RoutingMode = 'observe' | 'assist' | 'auto';
export type DecisionProvider = 'openrouter' | 'zenmux';

export type ApiSourceKind = 'official_api' | 'third_party_api' | 'coding_plan';
export interface ApiSourceDraft {
  kind: ApiSourceKind;
  account_label?: string | null;
  plan_label?: string | null;
}
/** Non-secret connection projection; labels do not verify account or plan eligibility. */
export interface ApiSourceConnection extends ApiSourceDraft {
  connection_instance_id: string;
  generation: number;
  endpoint: string;
  api_type: string;
  credential_reference: string;
  retired: boolean;
  model_bindings: Record<string, string>;
  account_state: 'unknown' | 'user_declared';
  plan_state: 'unknown' | 'user_declared';
}

export interface Provider {
  preset?: string;
  api_type?: string;
  test_model?: string;
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  enabled: boolean;
  has_api_key: boolean;
}

export interface Model {
  api_type?: string;
  input_price_known?: boolean | null;
  output_price_known?: boolean | null;
  cache_price_known?: boolean | null;
  cache_cost_per_million?: number;
  id: string;
  provider_id: string;
  model_id: string;
  name: string;
  tier: ModelTier;
  /** 用户停用：禁止该模型的所有调用（含按原 ID 直调）。 */
  enabled: boolean;
  /**
   * 用户选择：进入模型列表、自动候选与 Agent 可选列表。
   * 缺省视为 true（与后端 serde 默认一致，旧配置不因新增字段被清空）；
   * 取消选择不禁止按原模型标识的合规直调，只有停用才禁止调用。
   */
  selected?: boolean;
  supports_tools: boolean;
  supports_vision: boolean;
  supports_reasoning: boolean;
  context_window: number;
  input_cost_per_million: number;
  output_cost_per_million: number;
}

export interface RoutingPolicy {
  mode: RoutingMode;
  prefer_local: boolean;
  use_jev_when_ambiguous: boolean;
  jev_endpoint: string;
  decision_provider: DecisionProvider;
  jev_model?: string;
  decision_preference?: 'balanced' | 'cost' | 'quality' | 'speed';
  has_autojev_key: boolean;
  savings_baseline_model_id: string | null;
}

export interface ProxyStatus {
  running: boolean;
  paused?: boolean;
  port: number;
  base_url: string;
}

export type SubscriptionConnectionState = 'not_connected' | 'authorization_pending' | 'connected' | 'expired';
export type SubscriptionCapabilityStatus = 'unverified' | 'verified' | 'unsupported';
export type SubscriptionEvidenceState = 'unknown' | 'available' | 'stale' | 'failed' | 'unsupported' | 'denied';
/** 额度许可：缺失、null 或非布尔一律是 unknown，不得当成“已关闭”。 */
export type SubscriptionQuotaPermission = 'unknown' | 'allowed' | 'denied';
/** 额度来源视图：多桶优先，其次旧版单桶，都没有就是 unknown。 */
export type SubscriptionQuotaView = 'unknown' | 'rate_limits_by_limit_id' | 'rate_limits' | 'grok_cli_usage';

export type SubscriptionLoginStage = 'idle' | 'pending' | 'completed' | 'failed' | 'cancelled';
export type SubscriptionLocalClearing = 'cleared' | 'retained';
export type SubscriptionRemoteRevocation = 'revoked' | 'failed' | 'unsupported' | 'unknown';

/** 挂起登录的内存视图；绑定世代，不写进配置文件。 */
export interface SubscriptionLoginView {
  stage: SubscriptionLoginStage;
  authorization_url?: string | null;
  user_code?: string | null;
  attempt: number;
  generation: number;
  error?: string | null;
}

/** 上一次退出的实际结果：本地清除与远端撤销分开记录，不互相代替。 */
export interface SubscriptionLogoutView {
  local: SubscriptionLocalClearing;
  remote: SubscriptionRemoteRevocation;
  observed_at?: string | null;
}

/** 辅助进程的可用性、自述身份（未验证）、版本与专用授权目录。 */
export interface SubscriptionHelperView {
  available: boolean;
  user_agent?: string | null;
  version?: string | null;
  auth_home?: string | null;
}

export interface SubscriptionModel { model_id: string; name?: string | null; eligible: boolean }
export interface SubscriptionCapability { model_id: string; protocol: string; status: SubscriptionCapabilityStatus }
/** 单个额度窗口；越界或类型不符的字段是 null，并记入 invalid_fields，绝不改写成合法值。 */
export interface SubscriptionQuotaWindow {
  label: string;                // "primary" | "secondary" | "single"
  used_percent?: number | null;
  window_minutes?: number | null;
  resets_at?: number | null;    // Unix 秒
  missing_fields?: string[];
  invalid_fields?: string[];
}
/** 原始余额文本；不解析为金额、不推断单位，缺单位时必须由界面说明。 */
export interface SubscriptionQuotaCredits {
  has_credits?: boolean | null;
  unlimited?: boolean | null;
  balance?: string | null;
  /** 上游给出的原始单位文本；缺失即未知，界面不推断。 */
  unit?: string | null;
  /** credits 自身的许可轴（是否允许消耗额外 credits），与订阅内许可互不推导。 */
  permission?: SubscriptionQuotaPermission;
  missing_fields?: string[];
  invalid_fields?: string[];
}
export interface SubscriptionQuotaBucket {
  limit_id: string;
  name?: string | null;
  plan_type?: string | null;
  windows?: SubscriptionQuotaWindow[];
  credits?: SubscriptionQuotaCredits | null;
  permission?: SubscriptionQuotaPermission;
  missing_fields?: string[];
  invalid_fields?: string[];
}
/** 额度只读证据；history=true 表示 buckets 是失败后保留的历史数字。 */
export interface SubscriptionQuota {
  state: SubscriptionEvidenceState;
  source?: string | null;
  observed_at?: string | null;
  view?: SubscriptionQuotaView;
  buckets?: SubscriptionQuotaBucket[];
  missing_fields?: string[];
  history?: boolean;
}

/** Read-only Grok panel status; a blocked source gate is not account/quota evidence. */
export interface GrokReadOnlyStatus {
  state: 'blocked_unverified_source' | 'verified_source';
  sourceVerified: boolean;
  sourceVersion: string | null;
  sourceCommit: string | null;
  authInfo: 'unknown' | 'available';
  modelCatalog: 'unknown' | 'available';
  billing: 'unknown' | 'available';
  autoTopupRule: 'unknown' | 'available';
  extraUsagePermission: 'unknown' | 'available';
  realGenerationEnabled: boolean;
}
/** 模型目录只读证据；失败时保留上次已核实的目录并标记 stale。 */
export type SubscriptionCatalogState = 'unknown' | 'available' | 'stale' | 'failed' | 'unsupported';
export interface SubscriptionCatalogEvidence {
  state: SubscriptionCatalogState;
  source?: string | null;
  observed_at?: string | null;
  /** 上一次已核实目录里有、本次权威结果里已不存在的模型标识。 */
  removed_models?: string[];
  missing_fields?: string[];
}

/** 上游目录项的可用性：权威读取、读取失败、移除、撤销与未知互相区分。 */
export type SubscriptionCatalogAvailability = 'available' | 'stale' | 'removed' | 'revoked' | 'unknown';
/** 当前账号与连接世代下的资格；`eligible` 与 `stale` 仍构成资格，其余不可用。 */
export type SubscriptionCatalogEligibility = 'eligible' | 'stale' | 'not_discovered' | 'removed' | 'revoked' | 'account_changed' | 'unknown';

/**
 * 已发现订阅模型的界面投影：发现身份与用户配置分开。
 * `internal_id` 等于本地 `Model.id`，上游 ID 变化按新模型处理；
 * `selected` / `disabled` 来自用户配置，`availability` / `eligibility` 来自账号与上游。
 */
export interface SubscriptionCatalogEntry {
  /** 上游限定模型 ID：调用身份，不因显示名改变。 */
  model_id: string;
  /** 上游显示名：只用于展示。 */
  name?: string | null;
  /** 稳定内部标识，等于本地模型行的 `id`。 */
  internal_id: string;
  availability: SubscriptionCatalogAvailability;
  eligibility: SubscriptionCatalogEligibility;
  /** 用户是否把该模型列入模型列表与自动候选。 */
  selected: boolean;
  /** 用户是否停用该模型（停用禁止所有调用）。 */
  disabled: boolean;
}
/** 准入拒绝：稳定 code、类别、原因与恢复动作。 */
export interface SubscriptionDenial { code: string; family: string; message: string; recovery: string }
/** 一家订阅服务商的实际状态、只读证据与当前拒绝原因。 */
export interface SubscriptionView {
  provider_id: string;
  label: string;
  generation: number;
  /** Persistent unique identity for this stored connection instance; prevents delete/recreate aliasing. */
  connection_instance_id: string;
  state: SubscriptionConnectionState;
  /** Explicit, volatile real-generation opt-in for this subscription connection generation. */
  real_generation_enabled?: boolean;
  /** Finite per-confirmation AutoJev request ceiling and remaining attempts. */
  generation_call_limit?: number | null;
  generation_calls_remaining?: number | null;
  identity?: string | null;
  helper_version?: string | null;
  account_path?: string | null;
  models: SubscriptionModel[];
  /** 已发现模型的目录投影；目录失败时保留已核实项。API 服务商不出现。 */
  catalog_entries?: SubscriptionCatalogEntry[];
  capabilities: SubscriptionCapability[];
  quota: SubscriptionQuota;
  /** 目录证据；旧快照可能没有该字段，缺失即 unknown。 */
  catalog?: SubscriptionCatalogEvidence | null;
  /** 连接/证据级拒绝原因；额度准入原因单独记录。 */
  denial?: SubscriptionDenial | null;
  admission_denial?: SubscriptionDenial | null;
  adapter_available: boolean;
  login?: SubscriptionLoginView | null;
  logout?: SubscriptionLogoutView | null;
  helper?: SubscriptionHelperView | null;
}

export type SubscriptionAuthPhase = 'idle' | 'pending' | 'succeeded' | 'failed' | 'cancelled';
export type SubscriptionLocalLogoutState = 'not_attempted' | 'cleared' | 'failed';
export type SubscriptionRemoteRevokeState = 'not_attempted' | 'failed' | 'verified' | 'unsupported';

export interface SubscriptionAuthChallenge { kind: string; instructions: string; verification_url?: string | null; user_code?: string | null }
export interface SubscriptionAuthError { code: string; message: string; recovery: string }
export interface SubscriptionHelperInfo { available: boolean; login_supported: boolean; version?: string | null; program?: string | null; home?: string | null }
/** 退出证据：本地清除与远端撤销分开记录，二者互不推断。 */
export interface SubscriptionLogoutEvidence { local: SubscriptionLocalLogoutState; local_detail?: string | null; remote: SubscriptionRemoteRevokeState; remote_detail?: string | null }
/** 一次登录尝试的界面视图；不含任何凭据、token 或辅助进程输出原文。 */
export interface SubscriptionAuthView {
  provider_id: string;
  phase: SubscriptionAuthPhase;
  generation: number;
  attempt?: number | null;
  challenge?: SubscriptionAuthChallenge | null;
  identity?: string | null;
  error?: SubscriptionAuthError | null;
  helper: SubscriptionHelperInfo;
  logout: SubscriptionLogoutEvidence;
}

export interface AgentInjection { template: string; path: string; api: string }

export interface AgentStatus {
  args?: string[];
  injection?: AgentInjection | null;
  icon?: string | null;
  route_id?: string | null;
  id: string;
  executable_path?: string | null;
  command?: string;
  custom?: boolean;
  can_connect?: boolean;
  connection_note?: string | null;
  name: string;
  installed: boolean;
  connected: boolean;
  config_path: string;
  detail: string;
  /**
   * 注入目录待同步：保存的目录与当前应用内选择不一致。
   * 缺省视为 false。待同步只影响外部配置刷新，不推迟后端的撤销与停用生效。
   */
  catalog_pending_sync?: boolean;
}

export interface RouteEvent {
  id: string;
  created_at: string;
  endpoint: string;
  provider_name: string;
  model_name: string;
  reason: string;
  source: 'local' | 'jev' | 'observe' | 'assist';
  estimated_input_tokens: number;
  estimated_cost: number | null;
  estimated_savings: number;
  success: boolean;
}

export interface RouteRule {
  all_models?: boolean;
  id: string; name: string; strategy: "fixed" | "round_robin" | "jev"; automatic_policy?: RoutingPolicy | null; model_settings?: Record<string, { priority: number; weight: number }>; model_ids: string[]; enabled: boolean;
}

export interface GatewaySettings {
  connect_timeout_seconds: number; response_timeout_seconds: number; stream_idle_seconds: number;
  max_attempts: number; failure_threshold: number; cooldown_seconds: number;
  proxy_mode: 'system' | 'direct' | 'custom'; proxy_url: string;
}
export interface GatewayHealth { model_id: string; failures: number; state: string; retry_after_seconds: number; last_status: number }
export interface DashboardSnapshot {
  cpa_subscriptions?: import('./lib/cpa').CpaView[];
  api_sources?: Record<string, ApiSourceConnection>;
  recovery_notice?: string | null;
  gateway?: GatewaySettings;
  health?: GatewayHealth[];
  agent_catalogs?: Record<string, { binding: string; id: string; name: string }[]>;
  routes: RouteRule[];
  providers: Provider[];
  models: Model[];
  /** 订阅服务商的连接与证据；API 服务商不出现在这里。 */
  subscriptions: SubscriptionView[];
  /** 订阅登录/退出/换号的当前视图；API 服务商不出现在这里。 */
  subscription_auth: SubscriptionAuthView[];
  policy: RoutingPolicy;
  proxy: ProxyStatus;
  agents: AgentStatus[];
  events: RouteEvent[];
  install_id: string;
}

export interface RoutePreviewInput {
  requested_model?: string;
  prompt: string;
  endpoint: 'chat/completions' | 'responses' | 'messages';
  requires_tools: boolean;
  requires_vision: boolean;
  estimated_context_tokens: number;
}

export interface RouteDecision {
  model_id: string;
  model_name: string;
  provider_name: string;
  tier: ModelTier;
  source: 'local' | 'jev' | 'observe' | 'assist';
  confidence: number;
  reason: string;
  estimated_cost: number | null;
  estimated_savings: number;
}

export interface RequestLog {
  route_id?: string; route_strategy?: string; route_preference?: string;
  attempts?: { provider: string; model: string; status: number; duration_ms: number }[];
  id: string; created_at: string; agent: string; endpoint: string; requested_model: string;
  provider_name: string; model_name: string; model_id: string; source: string; reason: string;
  streaming: boolean; status: 'success' | 'error' | 'cancelled'; status_code: number;
  duration_ms: number; first_byte_ms: number | null; input_tokens: number | null;
  output_tokens: number | null; cache_read_tokens: number; cache_write_tokens: number;
  estimated_cost: number | null; error: string;
}

export interface PerformanceSettings { enabled: boolean; interval_minutes: number }
export interface ModelPerformance {
  first_content_ms: number | null; tokens_per_second: number | null;
  success_rate: number; samples: number; last_test_at: number | null; stale: boolean;
}
export interface PerformanceView {
  settings: PerformanceSettings; models: Record<string, ModelPerformance>;
  job: { running: boolean; cancelled: boolean; completed: number; total: number; completed_models: number; total_models: number; current_models: string[]; error: string | null };
}
