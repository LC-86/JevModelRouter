import type {
  ApiSourceDraft,
  RouteRule,
  AgentStatus,
  DashboardSnapshot,
  Model,
  Provider,
  RouteDecision,
  RoutePreviewInput,
  RoutingPolicy,
} from '../types';

const isTauri = () => '__TAURI_INTERNALS__' in window;

const MOCK: DashboardSnapshot = {
  routes: [],
  install_id: 'preview',
  subscriptions: [],
  subscription_auth: [],
  proxy: { running: true, port: 9526, base_url: 'http://127.0.0.1:9526' },
  policy: {
    mode: 'auto',
    prefer_local: false,
    use_jev_when_ambiguous: true,
    jev_endpoint: 'https://openrouter.ai/api/alpha/decisions',
    decision_provider: 'openrouter',
    jev_model: '~typesafe/jev-latest',
    has_autojev_key: false,
    savings_baseline_model_id: 'claude-sonnet-4',
  },
  providers: [
    {
      id: 'openrouter',
      name: 'OpenRouter',
      kind: 'openrouter',
      base_url: 'https://openrouter.ai/api',
      enabled: true,
      has_api_key: false,
    },
    {
      id: 'ollama',
      name: 'Ollama',
      kind: 'ollama',
      base_url: 'http://127.0.0.1:11434',
      enabled: false,
      has_api_key: false,
    },
  ],
  models: [
    {
      id: 'qwen-fast',
      provider_id: 'openrouter',
      model_id: 'qwen/qwen3-coder-flash',
      name: 'Qwen 3 Coder Flash',
      tier: 'fast',
      enabled: true,
      supports_tools: true,
      supports_vision: false,
      supports_reasoning: false,
      context_window: 262144,
      input_cost_per_million: 0.3,
      output_cost_per_million: 1.2,
    },
    {
      id: 'claude-sonnet-4',
      provider_id: 'openrouter',
      model_id: 'anthropic/claude-sonnet-4',
      name: 'Claude Sonnet 4',
      tier: 'strong',
      enabled: true,
      supports_tools: true,
      supports_vision: true,
      supports_reasoning: true,
      context_window: 200000,
      input_cost_per_million: 3,
      output_cost_per_million: 15,
    },
  ],
  agents: [
    {
      id: 'codex',
      name: 'Codex',
      installed: true,
      connected: false,
      config_path: '~/.codex/config.toml',
      detail: 'OpenAI Responses API',
    },
    {
      id: 'claude',
      name: 'Claude Code',
      installed: true,
      connected: false,
      config_path: '~/.claude/settings.json',
      detail: 'Anthropic Messages API',
    },
  ],
  events: [
    {
      id: 'demo-1',
      created_at: new Date(Date.now() - 1000 * 60 * 4).toISOString(),
      endpoint: 'responses',
      provider_name: 'OpenRouter',
      model_name: 'Qwen 3 Coder Flash',
      reason: 'Routine code edit; tools required, low ambiguity.',
      source: 'local',
      estimated_input_tokens: 1840,
      estimated_cost: 0.0006,
      estimated_savings: 0.0049,
      success: true,
    },
    {
      id: 'demo-2',
      created_at: new Date(Date.now() - 1000 * 60 * 19).toISOString(),
      endpoint: 'messages',
      provider_name: 'OpenRouter',
      model_name: 'Claude Sonnet 4',
      reason: 'Security-sensitive architecture review.',
      source: 'jev',
      estimated_input_tokens: 7340,
      estimated_cost: 0.024,
      estimated_savings: 0,
      success: true,
    },
  ],
};

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: tauriInvoke } = await import('@tauri-apps/api/core');
  return tauriInvoke<T>(command, args);
}

export async function createCpaSubscription(name:string,provider:'codex'|'xai'='codex'):Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('CPA 连接需要桌面应用；服务交付待 R6');
  return invoke('create_cpa_subscription',{provider,name});
}
export async function configureCpaService(providerId:string,binary:string,port:number):Promise<DashboardSnapshot> {
  return invoke('configure_cpa_service',{providerId,binary,port});
}
export async function stopCpaService(providerId:string):Promise<DashboardSnapshot> {
  return invoke('stop_cpa_service',{providerId});
}
export async function cpaSubscriptionAction(providerId:string,action:'begin'|'poll'|'refresh'|'cancel'|'disconnect'|'switch'):Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('CPA 连接需要桌面应用');
  return invoke('cpa_subscription_action',{providerId,action});
}
export async function selectCpaModel(providerId:string,modelId:string,selected:boolean):Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('CPA 模型选择需要桌面应用');
  return invoke('select_cpa_model',{providerId,modelId,selected});
}

export async function getSnapshot(): Promise<DashboardSnapshot> {
  if (!isTauri()) return structuredClone(MOCK);
  return invoke('get_snapshot');
}

export async function saveProvider(provider: Provider, apiKey?: string, addTestModel = false, originalId?: string, creating = false, source?: ApiSourceDraft): Promise<DashboardSnapshot> {
  if (source && creating && !apiKey?.trim()) throw new Error('Enter a generation API key before saving this source.');
  if (!isTauri()) {
    const oldId = originalId ?? provider.id;
    if ((creating || oldId !== provider.id) && MOCK.providers.some(p => p.id === provider.id)) throw new Error('Provider ID already exists');
    const i = MOCK.providers.findIndex((item) => item.id === oldId);
    if (originalId && i < 0) throw new Error('Provider no longer exists');
    for (const model of MOCK.models) if (model.provider_id === oldId) model.provider_id = provider.id;
    const next = { ...provider, has_api_key: Boolean(apiKey) || provider.has_api_key };
    if (i >= 0) MOCK.providers[i] = next;
    else MOCK.providers.push(next);
    if (source && !MOCK.api_sources?.[provider.id]) {
      MOCK.api_sources ??= {};
      const instanceId = crypto.randomUUID();
      MOCK.api_sources[provider.id] = { ...source, connection_instance_id: instanceId, generation: 1,
        endpoint: provider.base_url.replace(/\/+$/, ''), api_type: provider.api_type || 'chat_completions',
        credential_reference: `api-generation:${instanceId}`, retired: false, model_bindings: {}, account_state: source.account_label ? 'user_declared' : 'unknown', plan_state: source.plan_label ? 'user_declared' : 'unknown' };
    }
    const testedModelId = provider.test_model?.trim();
    if (addTestModel && testedModelId && !MOCK.models.some(m => m.provider_id === provider.id && m.model_id === testedModelId)) {
      MOCK.models.push({ id: crypto.randomUUID(), provider_id: provider.id, model_id: testedModelId, name: testedModelId, api_type: provider.api_type, enabled: true, tier: 'balanced', supports_tools: true, supports_vision: false, supports_reasoning: false, context_window: 1000000, input_cost_per_million: 0, output_cost_per_million: 0, cache_cost_per_million: 0 });
    }
    return structuredClone(MOCK);
  }
  return invoke('save_provider', { provider, apiKey: apiKey || null, addTestModel, originalId, creating, source });
}

export async function deleteProvider(id: string): Promise<DashboardSnapshot> {
  if (!isTauri()) {
    MOCK.providers = MOCK.providers.filter((provider) => provider.id !== id);
    MOCK.models = MOCK.models.filter((model) => model.provider_id !== id);
    return structuredClone(MOCK);
  }
  return invoke('delete_provider', { id });
}

export async function saveModel(model: Model): Promise<DashboardSnapshot> {
  model = { ...model, model_id: model.model_id.trim() };
  if (!isTauri()) {
    if (MOCK.models.some((item) => item.id !== model.id && item.provider_id === model.provider_id && item.model_id.trim() === model.model_id)) {
      throw new Error('This model ID already exists for this provider.');
    }
    const i = MOCK.models.findIndex((item) => item.id === model.id);
    if (i >= 0) MOCK.models[i] = model;
    else MOCK.models.push(model);
    return structuredClone(MOCK);
  }
  return invoke('save_model', { model });
}

export async function deleteModel(id: string): Promise<DashboardSnapshot> {
  if (!isTauri()) {
    MOCK.models = MOCK.models.filter((model) => model.id !== id);
    for (const route of MOCK.routes) {
      route.model_ids = route.model_ids.filter(candidate => candidate !== id);
      if (route.model_settings) delete route.model_settings[id];
      if (route.automatic_policy?.savings_baseline_model_id === id) route.automatic_policy.savings_baseline_model_id = null;
    }
    return structuredClone(MOCK);
  }
  return invoke('delete_model', { id });
}

export async function savePolicy(policy: RoutingPolicy, autojevKey?: string): Promise<DashboardSnapshot> {
  if (!isTauri()) {
    const sameProvider = policy.decision_provider === MOCK.policy.decision_provider;
    if (!sameProvider && !autojevKey?.trim()) throw new Error('Enter a new decision API key after changing provider');
    MOCK.policy = { ...policy, has_autojev_key: Boolean(autojevKey?.trim()) || (sameProvider && MOCK.policy.has_autojev_key) };
    return structuredClone(MOCK);
  }
  return invoke('save_policy', { policy, autojevKey: autojevKey || null });
}

export async function toggleProxy(start: boolean): Promise<DashboardSnapshot> {
  if (!isTauri()) {
    MOCK.proxy.running = start;
    MOCK.proxy.paused = false;
    if (!start) MOCK.agents.forEach(agent => { agent.connected = false; });
    return structuredClone(MOCK);
  }
  return invoke(start ? 'start_proxy' : 'stop_proxy');
}

export async function pauseProxy(): Promise<DashboardSnapshot> {
  if (!isTauri()) { MOCK.proxy.paused = true; return structuredClone(MOCK); }
  return invoke('pause_proxy');
}

const previewCounts: Record<string, number> = {};

export async function previewRoute(input: RoutePreviewInput): Promise<RouteDecision> {
  if (!isTauri()) {
    const complex = /architect|security|migration|incident|review/i.test(input.prompt) || input.estimated_context_tokens > 24000;
    const routeId = input.requested_model?.replace(/^autojev\//, '');
    const route = routeId ? MOCK.routes.find(r => r.id === routeId && r.enabled) : undefined;
    const directModel = routeId?.startsWith('model/') ? routeId.slice(6) : undefined;
    if (routeId && !route && !directModel) throw new Error('Route is unavailable');
    let eligible = MOCK.models.filter(m => m.enabled && (!directModel || m.id === directModel) && (!route || (route.strategy === 'jev' && route.all_models) || route.model_ids.includes(m.id))
      && MOCK.providers.some(p => p.id === m.provider_id && p.enabled
        && ((input.endpoint === 'messages') === ((m.api_type || p.api_type) === 'messages')))
);
    let index = 0;
    if (route?.strategy === 'round_robin' && eligible.length) {
      const priority = Math.max(...eligible.map(m => route.model_settings?.[m.id]?.priority ?? 0));
      eligible = eligible.filter(m => (route.model_settings?.[m.id]?.priority ?? 0) === priority);
    }
    if ((route?.strategy === 'round_robin' || route?.strategy === 'jev') && eligible.length) {
      const total = eligible.reduce((sum, m) => sum + (route.strategy === 'jev' ? 1 : (route.model_settings?.[m.id]?.weight ?? 1)), 0);
      const count = previewCounts[route.id] ?? 0;
      let slot = count % total;
      index = eligible.findIndex(m => { const weight = route.strategy === 'jev' ? 1 : (route.model_settings?.[m.id]?.weight ?? 1); if (slot < weight) return true; slot -= weight; return false; });
      previewCounts[route.id] = count + 1;
    }
    const model = route ? eligible[index]
      : eligible.find(m => m.tier === (complex ? 'strong' : 'fast')) ?? eligible[0];
    if (!model) throw new Error('No compatible enabled candidate for this route');
    const provider = MOCK.providers.find((p) => p.id === model.provider_id)!;
    return {
      model_id: model.id,
      model_name: model.name,
      provider_name: provider.name,
      tier: model.tier,
      source: !route && complex && MOCK.policy.use_jev_when_ambiguous ? 'jev' : 'local',
      confidence: complex ? 0.74 : 0.92,
      reason: route?.strategy === 'jev' ? ((route.automatic_policy ?? MOCK.policy).decision_preference === 'speed' ? 'No fresh comparable speed measurements; fell back to equal load balancing among eligible candidates.' : 'Jev unavailable in web preview; fell back to equal load balancing among eligible candidates.') : complex ? 'High-stakes or complex task; selected the strongest eligible model.' : 'Routine task; selected the lowest-cost eligible model.',
      estimated_cost: complex ? 0.028 : 0.0012,
      estimated_savings: complex ? 0 : 0.014,
    };
  }
  return invoke('preview_route', { input });
}

export async function detectAgents(): Promise<AgentStatus[]> {
  if (!isTauri()) return structuredClone(MOCK.agents);
  return invoke('detect_agents');
}

export async function connectAgent(id: string, routeId: string, routeIds: string[] = [routeId], onlyConnected = false): Promise<DashboardSnapshot> {
  if (!isTauri()) {
    const agent = MOCK.agents.find((item) => item.id === id);
    if (onlyConnected && !agent?.connected) throw new Error('Agent disconnected; configuration update skipped.');
    if (agent) { agent.connected = true; agent.route_id = routeId; }
    MOCK.agent_catalogs ??= {};
    MOCK.agent_catalogs[id] = routeIds.map(binding => ({ binding, id: binding, name: binding }));
    return structuredClone(MOCK);
  }
  return invoke('connect_agent', { id, routeId, routeIds, onlyConnected });
}

export async function restoreAgent(id: string): Promise<DashboardSnapshot> {
  if (!isTauri()) {
    const agent = MOCK.agents.find((item) => item.id === id);
    if (agent) agent.connected = false;
    return structuredClone(MOCK);
  }
  return invoke('restore_agent', { id });
}

export async function testProvider(id: string): Promise<string> {
  if (!isTauri()) throw new Error('Open the desktop app to test provider connections.');
  return invoke('test_provider', { id });
}

export type ImportSource = 'ccswitch' | 'termany';
export async function importProviders(source: ImportSource): Promise<{ snapshot: DashboardSnapshot; imported: number; skipped: number }> {
  if (!isTauri()) throw new Error('Open the desktop app to import local providers.');
  return invoke('import_providers', { source });
}

export async function testProviderDraft(provider: Provider, apiKey?: string, source?: ApiSourceDraft): Promise<string> {
  if (!isTauri()) throw new Error('Open the desktop app to test provider connections.');
  return invoke('test_provider_draft', { provider, apiKey: apiKey || null, source });
}

/** 只读刷新订阅连接：生成被拒绝或网关暂停时仍然可用，且不派发任何生成请求。 */
export async function refreshSubscription(providerId: string): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to refresh a subscription connection.');
  return invoke('refresh_subscription', { providerId });
}

/** Arm or disarm real Codex generation for the current connection only; ordinary admission remains mandatory. */
export async function setCodexRealGenerationEnabled(
  providerId: string,
  enabled: boolean,
  maxCalls: number,
  expectedConnectionInstanceId: string,
  expectedGeneration: number,
  expectedIdentity: string,
): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to manage real Codex generation.');
  return invoke('set_codex_real_generation_enabled', {
    providerId,
    enabled,
    maxCalls: enabled ? maxCalls : null,
    expectedConnectionInstanceId,
    expectedGeneration,
    expectedIdentity,
  });
}

/** Arm or disarm real Grok generation for the current connection only; all admission gates remain mandatory. */
export async function setGrokRealGenerationEnabled(
  providerId: string,
  enabled: boolean,
  maxCalls: number,
  expectedConnectionInstanceId: string,
  expectedGeneration: number,
  expectedIdentity: string,
): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to manage real Grok generation.');
  return invoke('set_grok_real_generation_enabled', {
    providerId,
    enabled,
    maxCalls: enabled ? maxCalls : null,
    expectedConnectionInstanceId,
    expectedGeneration,
    expectedIdentity,
  });
}

/** 开始订阅登录：立即返回 pending，完成结果由前端轮询 get_snapshot 观察。 */
export async function beginSubscriptionLogin(providerId: string): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to sign in to a subscription.');
  return invoke('begin_subscription_login', { providerId });
}

/** 取消挂起的订阅登录；迟到结果按世代丢弃。 */
export async function cancelSubscriptionLogin(providerId: string): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to cancel a subscription sign-in.');
  return invoke('cancel_subscription_login', { providerId });
}

/** 退出订阅：本地清除与远端撤销结果分别返回。 */
export async function logoutSubscription(providerId: string): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to sign out of a subscription.');
  return invoke('logout_subscription', { providerId });
}

/** 换号等价于退出加登录；失败时不恢复旧账号。 */
export async function switchSubscriptionAccount(providerId: string): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to switch a subscription account.');
  return invoke('switch_subscription_account', { providerId });
}

/** 读取当前登录尝试的进度；迟到结果由后端判定为 Superseded，不写入任何状态。 */
export async function pollSubscriptionLogin(providerId: string): Promise<DashboardSnapshot> {
  if (!isTauri()) throw new Error('Open the desktop app to manage subscription authorization.');
  return invoke('poll_subscription_login', { providerId });
}


export async function saveRoute(route: RouteRule, creating = false, apiKey?: string, originalId?: string): Promise<DashboardSnapshot> {
  if (isTauri()) return invoke('save_route', { route, creating, apiKey, originalId });
  if ((creating || (originalId && originalId !== route.id)) && MOCK.routes.some(r => r.id === route.id)) throw new Error('Route ID already exists');
  MOCK.routes = [...MOCK.routes.filter(r => r.id !== (originalId ?? route.id)), route];
  return structuredClone(MOCK);
}
export async function deleteRoute(id: string): Promise<DashboardSnapshot> {
  if (isTauri()) return invoke('delete_route', { id });
  MOCK.routes = MOCK.routes.filter(r => r.id !== id);
  return structuredClone(MOCK);
}

export interface CustomAgentInput { config_path?: string; injection?: import('../types').AgentInjection | null; id: string; name: string; command: string; args: string[]; icon?: string | null }
export async function testCustomAgent(agent: CustomAgentInput): Promise<string> {
  if (!isTauri()) throw new Error("Testing agents requires the desktop app");
  return invoke("test_custom_agent", { agent });
}
export async function saveCustomAgent(agent: CustomAgentInput): Promise<DashboardSnapshot> {
  if (isTauri()) return invoke('save_custom_agent', { agent });
  MOCK.agents = [...MOCK.agents.filter(a => a.id !== agent.id), { ...agent, installed: false, connected: false, config_path: agent.config_path ?? agent.injection?.path ?? '', detail: agent.command, custom: true, can_connect: !!agent.injection }];
  return structuredClone(MOCK);
}

export async function launchAgent(id: string): Promise<void> {
  if (!isTauri()) throw new Error('Launching agents requires the desktop app');
  return invoke('launch_agent', { id });
}

export async function getRequestLogs(since: string): Promise<import('../types').RequestLog[]> {
  if (!isTauri()) return []; // Browser preview must not pretend demo requests are real traffic.
  return invoke('get_request_logs', { since });
}

export interface DebugResult { status: number; model: string; source: string; elapsed_ms: number; body: string; parsed_body?: unknown; request_body?: Record<string, unknown>; telemetry?: import('../types').RequestLog | null }
export async function debugRequest(target: string, endpoint: string, prompt: import('./debug-message').DebugContent, history: {role: string; content: import('./debug-message').DebugContent}[] = [], parameters: Record<string, unknown> = {}, sessionId?: string, onProgress?: (event: {text: string; reasoning: boolean}) => void): Promise<DebugResult> {
  if (!isTauri()) throw new Error('Real requests require the desktop app');
  const { Channel } = await import('@tauri-apps/api/core');
  const onProgressChannel = new Channel<{text: string; reasoning: boolean}>();
  onProgressChannel.onmessage = event => onProgress?.(event);
  return invoke('debug_request', { target, endpoint, prompt, history, parameters, sessionId, onProgress: onProgressChannel });
}

export const gatewayDefaults: import('../types').GatewaySettings = { connect_timeout_seconds:10,response_timeout_seconds:60,stream_idle_seconds:120,max_attempts:4,failure_threshold:3,cooldown_seconds:30,proxy_mode:'system',proxy_url:'' };
export async function saveGatewaySettings(gateway: import('../types').GatewaySettings): Promise<DashboardSnapshot> {
  if (!isTauri()) { MOCK.gateway = gateway; return structuredClone(MOCK); }
  return invoke('save_gateway_settings', { gateway });
}
export async function resetGatewayHealth(modelId?: string): Promise<DashboardSnapshot> {
  if (!isTauri()) { MOCK.health=modelId ? (MOCK.health ?? []).filter(h => h.model_id !== modelId) : [];return structuredClone(MOCK); }
  return invoke('reset_gateway_health', { modelId });
}

export async function getGatewayHealth(): Promise<import('../types').GatewayHealth[]> {
  return isTauri() ? invoke('get_gateway_health') : structuredClone(MOCK.health ?? []);
}

export async function testJevSettings(policy: RoutingPolicy, apiKey?: string): Promise<string> {
  if (!isTauri()) throw new Error('Open the desktop app to test Jev.');
  return invoke('test_jev_settings', { policy, apiKey: apiKey || null });
}

export async function getModelPerformance(): Promise<import('../types').PerformanceView> {
  if (!isTauri()) return { settings: { enabled: true, interval_minutes: 30 }, models: {}, job: { running: false, cancelled: false, completed: 0, total: 0, completed_models: 0, total_models: 0, current_models: [], error: null } };
  return invoke('get_model_performance');
}
export async function startModelSpeedTests(ids: string[]): Promise<void> {
  if (!isTauri()) throw new Error('Speed tests are available in the desktop app.');
  return invoke('start_model_speed_tests', { ids });
}
export async function cancelModelSpeedTests(): Promise<void> {
  if (!isTauri()) return;
  return invoke('cancel_model_speed_tests');
}
export async function savePerformanceSettings(settings: import('../types').PerformanceSettings): Promise<void> {
  if (!isTauri()) throw new Error('Speed tests are available in the desktop app.');
  return invoke('save_performance_settings', { settings });
}

export interface CurlResult { status: number; elapsed_ms: number; body: string; headers: Record<string, string>; telemetry?: import('../types').RequestLog | null }
export interface CurlProgress { started?: boolean; status?: number; headers?: Record<string, string>; bytes?: number[] }
export async function executeDebugCurl(id: string, request: { endpoint: string; body: Record<string, unknown>; headers: Record<string, string> }, progress: (event: CurlProgress) => void): Promise<CurlResult> {
  if (!isTauri()) throw new Error('Real requests require the desktop app');
  const { Channel } = await import('@tauri-apps/api/core');
  const onProgress = new Channel<CurlProgress>();
  onProgress.onmessage = progress;
  return invoke('debug_curl', { id, ...request, onProgress });
}
export async function cancelDebugCurl(id: string) { return invoke('cancel_debug_curl', { id }); }

export async function openAgentConfig(id: string, index: number): Promise<void> {
  if (!isTauri()) throw new Error('Opening configuration files requires the desktop app');
  return invoke('open_agent_config', { id, index });
}

export async function setCpaHandRun(providerId:string,enabled:boolean):Promise<DashboardSnapshot> {return invoke('set_cpa_hand_run',{providerId,enabled});}
export async function setCodingPlanHandRun(providerId:string,enabled:boolean):Promise<DashboardSnapshot> {return invoke('set_coding_plan_hand_run',{providerId,enabled});}

export async function openCpaAuthorization(providerId:string):Promise<void> {return invoke('open_cpa_authorization',{providerId});}
