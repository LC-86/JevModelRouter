export type ProviderKind = 'openrouter' | 'ollama' | 'openai_compatible' | 'codex_subscription' | 'grok_subscription';
export type ModelTier = 'fast' | 'balanced' | 'strong';
export type RoutingMode = 'observe' | 'assist' | 'auto';
export type DecisionProvider = 'openrouter' | 'zenmux';

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
  enabled: boolean;
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
export type SubscriptionEvidenceState = 'unknown' | 'available' | 'stale' | 'failed' | 'unsupported';

export interface SubscriptionModel { model_id: string; name?: string | null; eligible: boolean }
export interface SubscriptionCapability { model_id: string; protocol: string; status: SubscriptionCapabilityStatus }
export interface SubscriptionQuota { state: SubscriptionEvidenceState; source?: string | null; observed_at?: string | null }
/** 准入拒绝：稳定 code、类别、原因与恢复动作。 */
export interface SubscriptionDenial { code: string; family: string; message: string; recovery: string }
/** 一家订阅服务商的实际状态、只读证据与当前拒绝原因。 */
export interface SubscriptionView {
  provider_id: string;
  label: string;
  generation: number;
  state: SubscriptionConnectionState;
  identity?: string | null;
  helper_version?: string | null;
  account_path?: string | null;
  models: SubscriptionModel[];
  capabilities: SubscriptionCapability[];
  quota: SubscriptionQuota;
  denial?: SubscriptionDenial | null;
  adapter_available: boolean;
}

export type SubscriptionAuthPhase = 'idle' | 'pending' | 'succeeded' | 'failed' | 'cancelled';
export type SubscriptionLocalLogoutState = 'not_attempted' | 'cleared' | 'failed';
export type SubscriptionRemoteRevokeState = 'not_attempted' | 'failed' | 'verified' | 'unsupported';

export interface SubscriptionAuthChallenge { kind: string; instructions: string; verification_url?: string | null; user_code?: string | null }
export interface SubscriptionAuthError { code: string; message: string; recovery: string }
export interface SubscriptionHelperInfo { available: boolean; version?: string | null; program?: string | null; home?: string | null }
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
  job: { running: boolean; completed: number; total: number; completed_models: number; total_models: number; current_models: string[]; error: string | null };
}
