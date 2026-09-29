export type ProviderKind = 'openrouter' | 'ollama' | 'openai_compatible';
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
