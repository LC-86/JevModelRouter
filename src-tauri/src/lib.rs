mod debug_curl;
mod cost;
mod performance;
mod agents;
mod custom_agents;
mod lifecycle;
mod resilience;
mod ownership;
mod agent_catalog;
mod agent_adapters;
mod fastclaw_adapter;
mod gemini_bridge;
mod config;
mod proxy;
mod traffic;
mod protocol;
mod provider_import;
mod provider_test;
mod router;
mod dispatch;
mod subscription;
mod codex_helper;
mod runtime;
#[cfg(feature = "isolation-check")]
mod isolation_check;
#[cfg(test)]
mod dispatch_tests;

use std::sync::Arc;

use anyhow::{anyhow, Context};
use config::{
    AppConfig, ConfigStore, Model, Provider,
    ProviderKind, RoutingPolicy,
};
use proxy::ProxyHandle;
use reqwest::Client;
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tokio::sync::Mutex;

#[derive(Clone)]
struct AppState {
    performance: Arc<performance::Runner>,
    store: Arc<ConfigStore>,
    proxy: Arc<Mutex<Option<ProxyHandle>>>,
    /// 登录会话内存态：挂起链接、user code、错误与最近一次退出都不落盘。
    sessions: Arc<Mutex<subscription::SessionState>>,
}

#[derive(Serialize)]
struct ProxyStatus {
    running: bool,
    paused: bool,
    port: u16,
    base_url: String,
}

#[derive(Serialize)]
struct DashboardSnapshot {
    recovery_notice: Option<String>,
    gateway: resilience::Settings,
    health: Vec<resilience::Status>,
    agent_catalogs: std::collections::HashMap<String, Vec<agent_catalog::Entry>>,
    providers: Vec<Provider>,
    models: Vec<Model>,
    /// 订阅服务商的连接、只读证据与当前拒绝原因。API 服务商不出现在这里。
    subscriptions: Vec<subscription::SubscriptionView>,
    routes: Vec<config::RouteRule>,
    policy: RoutingPolicy,
    proxy: ProxyStatus,
    agents: Vec<agents::AgentStatus>,
    events: Vec<config::RouteEvent>,
    install_id: String,
}

async fn snapshot(state: &AppState) -> DashboardSnapshot {
    let (mut config, decision_key) = state.store.read_with_decision_key();
    for provider in &mut config.providers {
        provider.has_api_key = (provider.kind == ProviderKind::Ollama
            || state.store.read_secret(&format!("provider:{}", provider.id)).is_some())
            && !subscription::is_subscription_provider(provider);
    }
    config.policy.has_autojev_key = decision_key.is_some();
    let running = state.proxy.lock().await.as_ref().is_some_and(|p|p.running());
    let port = state.proxy.lock().await.as_ref().map_or(config.port, |p| p.port);
    let paused = state.proxy.lock().await.as_ref().is_some_and(|p| p.running() && p.paused());
    let detected = detected_agents_with_selection(&config);
    let helper = state.store.subscription.helper_status();
    // 只有适配器真正支持的订阅行才能带上 helper 版本与授权目录，其余行如实不可用。
    let supported: Vec<String> = config
        .providers
        .iter()
        .filter(|provider| subscription::is_subscription_provider(provider) && state.store.subscription.supports(&provider.kind))
        .map(|provider| provider.id.clone())
        .collect();
    let subscriptions = {
        let mut sessions = state.sessions.lock().await;
        sessions.helper = helper.clone();
        sessions.supported_providers = supported;
        subscription::views(&config, state.store.subscription.available(), &sessions)
    };
    DashboardSnapshot {
        recovery_notice: lifecycle::notice(config.port),
        gateway: config.gateway.clone(),
        health: state.proxy.lock().await.as_ref().map_or_else(Vec::new, |p|p.health.statuses()),
        agent_catalogs: config.agent_catalogs.clone(),
        providers: config.providers,
        models: config.models,
        subscriptions,
        routes: config.routes,
        policy: config.policy,
        proxy: ProxyStatus {
            running, paused,
            port,
            base_url: format!("http://127.0.0.1:{port}"),
        },
        agents: detected,
        events: config.events,
        install_id: config.install_id,
    }
}

fn detected_agents_with_selection(config: &config::AppConfig) -> Vec<agents::AgentStatus> {
    let mut detected = agents::detect(&config.custom_agents);
    for agent in &mut detected {
        agent.connected = agent.injection.as_ref().map(|i| custom_agents::owned(i, &crate::runtime::home_dir().unwrap_or_default(), config.port)).unwrap_or_else(|| agents::owned_by(&agent.id,config.port));
        if let Some(selected) = agent.route_id.as_ref() {
            if let Some(entry) = config.agent_catalogs.get(&agent.id).and_then(|entries| entries.iter().find(|e| &e.id == selected)) { agent.route_id = Some(entry.binding.clone()); }
        }
        if agent.route_id.is_none() {
            agent.route_id = config.agent_selections.get(&agent.id).cloned();
        }
    }
    detected
}

#[tauri::command]
async fn get_request_logs(state: State<'_, AppState>, since: String) -> Result<Vec<traffic::RequestLog>, String> {
    let since = chrono::DateTime::parse_from_rfc3339(&since).map_err(|_| "Invalid start date")?
        .with_timezone(&chrono::Utc).to_rfc3339();
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<Vec<traffic::RequestLog>> {
        let mut logs=store.request_logs(&since)?;
        let config=store.read();
        for log in &mut logs {
            let requested=log.requested_model.clone();
            traffic::resolve_requested_model(log,&config,&requested);
        }
        Ok(logs)
    })
        .await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())
}

#[tauri::command]
async fn get_model_performance(state: State<'_, AppState>) -> Result<performance::View,String> {
    Ok(state.performance.view(&state.store.read()))
}
#[tauri::command]
async fn start_model_speed_tests(state: State<'_, AppState>, ids: Vec<String>) -> Result<(),String> {
    state.performance.start(state.store.clone(),ids).map_err(|e|e.to_string())
}
#[tauri::command]
async fn cancel_model_speed_tests(state: State<'_, AppState>) -> Result<(),String> {
    state.performance.cancel(); Ok(())
}
#[tauri::command]
async fn save_performance_settings(state: State<'_, AppState>, mut settings: performance::Settings) -> Result<(),String> {
    if !(5..=120).contains(&settings.interval_minutes) {return Err("Test interval must be between 5 and 120 minutes".into());}
    settings.version = 1;
    state.store.update(|c|c.performance_settings=settings).map_err(|e|e.to_string())
}

#[tauri::command]
async fn get_snapshot(state: State<'_, AppState>) -> Result<DashboardSnapshot, String> {
    Ok(snapshot(&state).await)
}

/// 只读刷新订阅连接。生成被拒绝或网关暂停时仍需可用，且不产生任何生成请求。
/// 挂起登录期间不改写连接，避免把 pending 伪装成已连接或让旧身份复活。
#[tauri::command]
async fn refresh_subscription(state: State<'_, AppState>, provider_id: String) -> Result<DashboardSnapshot, String> {
    subscription::refresh(&state.store, &provider_id).await?;
    Ok(snapshot(&state).await)
}

/// 后台等待 account/login/completed 并写入内存态；世代或尝试已变时结果整体丢弃。
fn spawn_login_waiter(
    store: Arc<ConfigStore>,
    sessions: Arc<Mutex<subscription::SessionState>>,
    provider_id: String,
) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = subscription::await_login(store, provider_id, sessions).await {
            eprintln!("AutoJev subscription login: {error}");
        }
    });
}

/// 发起登录：立即返回 pending，授权链接只存在于内存态，由 `get_snapshot` 轮询观察。
#[tauri::command]
async fn begin_subscription_login(state: State<'_, AppState>, provider_id: String) -> Result<DashboardSnapshot, String> {
    subscription::begin_login(&state.store, &provider_id, &state.sessions).await?;
    spawn_login_waiter(state.store.clone(), state.sessions.clone(), provider_id);
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn cancel_subscription_login(state: State<'_, AppState>, provider_id: String) -> Result<DashboardSnapshot, String> {
    subscription::cancel_login(&state.store, &provider_id, &state.sessions).await?;
    Ok(snapshot(&state).await)
}

/// 退出：世代递增、身份/额度/目录/能力缓存清空；服务商、模型与路由配置保留。
#[tauri::command]
async fn logout_subscription(state: State<'_, AppState>, provider_id: String) -> Result<DashboardSnapshot, String> {
    subscription::logout(&state.store, &provider_id, &state.sessions).await?;
    Ok(snapshot(&state).await)
}

/// 换号 = 退出 + 重新登录；换号失败不会恢复旧账号。
#[tauri::command]
async fn switch_subscription_account(state: State<'_, AppState>, provider_id: String) -> Result<DashboardSnapshot, String> {
    subscription::switch_account(&state.store, &provider_id, &state.sessions).await?;
    spawn_login_waiter(state.store.clone(), state.sessions.clone(), provider_id);
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn save_provider(
    state: State<'_, AppState>,
    mut provider: Provider,
    api_key: Option<String>,
    add_test_model: Option<bool>,
    original_id: Option<String>,
    creating: Option<bool>,
) -> Result<DashboardSnapshot, String> {
    provider.id = provider.id.trim().to_owned();
    validate_provider(&provider).map_err(|error| error.to_string())?;
    if subscription::is_subscription_provider(&provider) && api_key.as_deref().is_some_and(|key| !key.trim().is_empty()) {
        return Err("Subscription providers keep their authorization in the provider helper, not as an API key".into());
    }
    provider.has_api_key = false;
    let old_id = original_id.as_deref().unwrap_or(&provider.id).to_owned();
    let old_account = format!("provider:{old_id}");
    // 校验先行、副作用在后：目标标识被占用或旧 provider 已不存在时直接失败，
    // helper、内存会话与专用授权目录零改动；校验通过后才迁移或释放订阅资源。
    prepare_subscription_change(&state.store, &state.sessions, &provider, original_id.as_deref(), creating.unwrap_or(false)).await?;
    let new_account = format!("provider:{}", provider.id);
    state.store.update_checked(|config| apply_provider_edit(config, provider, original_id.as_deref(), creating.unwrap_or(false), add_test_model.unwrap_or(false)), Some((&old_account, &new_account, api_key.as_deref().map(str::trim).filter(|key|!key.is_empty())))).map_err(|e|e.to_string())?;
    Ok(snapshot(&state).await)
}

/// 保存服务商前的订阅资源处置。先做与 [`apply_provider_edit`] 完全相同的只读校验
/// （失败零副作用），再按 kind 与标识变化决定：
/// - 同一订阅类型且改名 → [`subscription::migrate_helper`] 迁移适配器自有状态；
/// - kind 变化（订阅→API、Codex↔Grok 等，适配器的 supports 面随之改变）→ 先
///   [`subscription::dispose`] 释放旧标识的辅助进程、专用授权目录与会话。
///
/// 返回 Ok 才允许改配置；任何一步失败都整体中止，绝不半清理。
async fn prepare_subscription_change(
    store: &ConfigStore,
    sessions: &tokio::sync::Mutex<subscription::SessionState>,
    provider: &Provider,
    original_id: Option<&str>,
    creating: bool,
) -> Result<(), String> {
    let Some(old_id) = original_id else { return Ok(()); };
    let old_kind = {
        let config = store.read();
        validate_provider_edit(&config, provider, original_id, creating).map_err(|error| error.to_string())?;
        config.providers.iter().find(|existing| existing.id == old_id).map(|existing| existing.kind.clone())
    };
    let Some(old_kind) = old_kind.filter(|kind| subscription::is_subscription(kind)) else {
        return Ok(());
    };
    if provider.kind == old_kind {
        // 同一订阅类型：只有改名需要迁移适配器自有状态（辅助进程、专用目录、挂起尝试）。
        if old_id != provider.id {
            subscription::migrate_helper(store, old_id, &provider.id, sessions).await?;
        }
        return Ok(());
    }
    subscription::dispose(store, old_id, sessions).await
}

/// 存在性与新标识唯一性校验。改配置（[`apply_provider_edit`]）与订阅资源处置
/// （[`prepare_subscription_change`]）共用同一份，避免两处漂移。
fn validate_provider_edit(config: &AppConfig, provider: &Provider, original_id: Option<&str>, creating: bool) -> anyhow::Result<()> {
    let old_id = original_id.unwrap_or(provider.id.as_str());
    if original_id.is_some() && !config.providers.iter().any(|p| p.id == old_id) {
        return Err(anyhow!("Provider no longer exists"));
    }
    if (creating || old_id != provider.id) && config.providers.iter().any(|p| p.id == provider.id) {
        return Err(anyhow!("Provider ID already exists"));
    }
    Ok(())
}

fn apply_provider_edit(config: &mut AppConfig, provider: Provider, original_id: Option<&str>, creating: bool, add_test_model: bool) -> anyhow::Result<()> {
    let old_id = original_id.unwrap_or(&provider.id).to_owned();

    validate_provider_edit(config, &provider, original_id, creating)?;
    for model in &mut config.models { if model.provider_id==old_id { model.provider_id=provider.id.clone(); } }
    if add_test_model { add_provider_test_model(config, &provider); }
    subscription::rename_provider(config, &old_id, &provider.id);
    subscription::sync_provider(config, &provider.id, &provider.kind);
    if let Some(existing)=config.providers.iter_mut().find(|p|p.id==old_id) { *existing=provider; } else { config.providers.push(provider); }
    Ok(())
}

fn add_provider_test_model(config: &mut config::AppConfig, provider: &Provider) {
    let model_id = provider.test_model.trim();
    if model_id.is_empty() || config.models.iter().any(|m| m.provider_id == provider.id && m.model_id == model_id) { return; }
    config.models.push(config::Model { input_price_known: Some(false), output_price_known: Some(false), cache_price_known: Some(false),
        id: uuid::Uuid::new_v4().to_string(), provider_id: provider.id.clone(),
        model_id: model_id.into(), name: model_id.into(), api_type: provider.api_type.clone(),
        tier: config::ModelTier::Balanced, enabled: true,
        supports_tools: true, supports_vision: false, supports_reasoning: false,
        context_window: 1000000, input_cost_per_million: 0.0,
        output_cost_per_million: 0.0, cache_cost_per_million: 0.0,
    });
}

#[derive(Serialize)]
struct ImportResult { snapshot: DashboardSnapshot, imported: usize, skipped: usize }

#[tauri::command]
async fn import_providers(state: State<'_, AppState>, source: String) -> Result<ImportResult, String> {
    let (candidates, mut skipped) = tauri::async_runtime::spawn_blocking(move || provider_import::read(&source))
        .await.map_err(|_| "Could not read import source".to_string())?
        .map_err(|error| error.to_string())?;
    let mut imported = 0;
    for item in candidates {
        if provider_import::already_imported(&state.store.read().providers, &item.provider) {
            skipped += 1;
            continue;
        }
        if !item.key.is_empty() {
            state.store.write_secret(&format!("provider:{}", item.provider.id), &item.key)
                .map_err(|_| format!("Could not store credentials. {imported} providers imported before this error."))?;
        }
        let id = item.provider.id.clone();
        if state.store.update(|config| config.providers.push(item.provider)).is_err() {
            let _ = state.store.delete_secret(&format!("provider:{id}"));
            return Err(format!("Could not save provider. {imported} providers imported before this error."));
        }
        imported += 1;
    }
    Ok(ImportResult { snapshot: snapshot(&state).await, imported, skipped })
}

#[tauri::command]
async fn delete_provider(
    state: State<'_, AppState>,
    id: String,
) -> Result<DashboardSnapshot, String> {
    // 删除订阅服务商前先释放自有资源（辅助进程、专用授权目录、内存会话）；
    // 失败即中止，不留下半清理状态。非订阅服务商此调用是空操作。
    subscription::dispose(&state.store, &id, &state.sessions).await?;
    state
        .store
        .update(|config| {
            config.providers.retain(|provider| provider.id != id);
            config.models.retain(|model| model.provider_id != id);
            subscription::forget_provider(config, &id);
        })
        .map_err(|error| error.to_string())?;
    state.store.delete_secret(&format!("provider:{id}")).map_err(|error| error.to_string())?;
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn save_route(state: State<'_, AppState>, mut route: config::RouteRule, creating: bool, api_key: Option<String>, original_id: Option<String>) -> Result<DashboardSnapshot, String> {
    route.id=route.id.trim().to_owned();
    state.store.update_checked(|config| apply_route_edit(config, route, original_id.as_deref(), creating), None).map_err(|e| e.to_string())?;
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        state.store.write_secret("autojev-cloud", key.trim()).map_err(|e| e.to_string())?;
    }
    Ok(snapshot(&state).await)
}

fn apply_route_edit(config: &mut AppConfig, mut route: config::RouteRule, original_id: Option<&str>, creating: bool) -> anyhow::Result<()> {

    route.model_ids.retain(|id| config.models.iter().any(|m| &m.id == id));
    route.model_settings.retain(|id,_| route.model_ids.contains(id));
    router::validate_rule(config, &route)?;
    let old_id=original_id.unwrap_or(&route.id);
    if original_id.is_some() && !config.routes.iter().any(|r|r.id==old_id) { return Err(anyhow!("Route no longer exists")); }
    if (creating || old_id != route.id) && config.routes.iter().any(|r| r.id == route.id) {
        return Err(anyhow!("Route ID already exists"));
    }
    for binding in config.agent_selections.values_mut() { if binding==old_id { *binding=route.id.clone(); } }
    for entries in config.agent_catalogs.values_mut() { for entry in entries { if entry.binding==old_id { entry.binding=route.id.clone(); } } }
    if let Some(existing) = config.routes.iter_mut().find(|r| r.id == old_id) {
        *existing = route;
    } else { config.routes.push(route); }
    Ok(())
}

#[tauri::command]
async fn delete_route(state: State<'_, AppState>, id: String) -> Result<DashboardSnapshot, String> {
    state.store.update(|config| config.routes.retain(|r| r.id != id)).map_err(|e| e.to_string())?;
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn save_model(state: State<'_, AppState>, mut model: Model) -> Result<DashboardSnapshot, String> {
    model.supports_tools = true;
    model.model_id = model.model_id.trim().to_owned();
    state.store.update(|config| -> anyhow::Result<()> {
        validate_model(config, &model)?;
        if let Some(existing) = config.models.iter_mut().find(|item| item.id == model.id) {
            *existing = model;
        } else { config.models.push(model); }
        Ok(())
    }).map_err(|error| error.to_string())?.map_err(|error| error.to_string())?;
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn delete_model(state: State<'_, AppState>, id: String) -> Result<DashboardSnapshot, String> {
    state
        .store
        .update(|config| {
            config.models.retain(|model| model.id != id);
            for route in &mut config.routes {
                route.model_ids.retain(|candidate| candidate != &id);
                route.model_settings.remove(&id);
                if let Some(policy) = &mut route.automatic_policy {
                    if policy.savings_baseline_model_id.as_deref() == Some(&id) { policy.savings_baseline_model_id = None; }
                }
            }
            if config.policy.savings_baseline_model_id.as_deref() == Some(&id) {
                config.policy.savings_baseline_model_id = None;
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(snapshot(&state).await)
}

fn validate_jev_policy(policy:&RoutingPolicy)->Result<(),String>{
    let url=reqwest::Url::parse(&policy.jev_endpoint).map_err(|_|"Invalid Jev endpoint")?;
    if !(url.scheme()=="https" || (url.scheme()=="http" && matches!(url.host_str(),Some("127.0.0.1"|"localhost"|"[::1]")))) || !url.username().is_empty() || url.password().is_some() {return Err("Jev endpoint must use HTTPS or loopback HTTP without embedded credentials".into());}
    Ok(())
}

fn require_new_key_after_provider_change(saved: &RoutingPolicy, draft: &RoutingPolicy, key: Option<&str>) -> anyhow::Result<()> {
    if saved.decision_provider != draft.decision_provider && key.is_none() {
        return Err(anyhow!("Enter a new decision API key after changing provider"));
    }
    Ok(())
}

fn save_decision_policy(store: &ConfigStore, mut policy: RoutingPolicy, api_key: Option<&str>) -> Result<(), String> {
    validate_jev_policy(&policy)?;
    policy.jev_model = policy.jev_model.trim().to_owned();
    policy.has_autojev_key = false;
    let key = api_key.map(str::trim).filter(|value| !value.is_empty());
    store.update_checked(
        |config| {
            require_new_key_after_provider_change(&config.policy, &policy, key)?;
            config.policy = policy;
            Ok(())
        },
        Some(("autojev-cloud", "autojev-cloud", key)),
    ).map_err(|error| error.to_string())
}

async fn probe_decision_settings(store: &ConfigStore, policy: RoutingPolicy, api_key: Option<&str>) -> Result<String, String> {
    validate_jev_policy(&policy)?;
    let (mut config, stored) = store.read_with_decision_key();
    let supplied = api_key.map(str::trim).filter(|value| !value.is_empty());
    require_new_key_after_provider_change(&config.policy, &policy, supplied).map_err(|error| error.to_string())?;
    let key = supplied.or(stored.as_deref()).ok_or("Enter a Jev access key")?;
    config.policy = policy;
    let client = config.gateway.client().map_err(|error| error.to_string())?;
    router::probe_jev(&config, &client, key).await.map_err(|error| {
        if error.to_string().contains("Add an enabled model") { error.to_string() }
        else { "Jev test failed: check the endpoint, model ID, credentials and returned candidate ID".into() }
    })
}

#[tauri::command]
async fn test_jev_settings(state:State<'_,AppState>,policy:RoutingPolicy,api_key:Option<String>)->Result<String,String>{
    probe_decision_settings(&state.store, policy, api_key.as_deref()).await
}

#[tauri::command]
async fn save_policy(
    state: State<'_, AppState>,
    policy: RoutingPolicy,
    autojev_key: Option<String>,
) -> Result<DashboardSnapshot, String> {
    save_decision_policy(&state.store, policy, autojev_key.as_deref())?;
    Ok(snapshot(&state).await)
}

async fn safe_stop(state:&AppState)->Result<(),String> {
    let mut handle=state.proxy.lock().await;
    agents::restore_gateway(state.store.read().port).map_err(|e|e.to_string())?;
    if let Some(proxy)=handle.take(){proxy.stop().await;}Ok(())
}
#[tauri::command]
async fn save_gateway_settings(state:State<'_,AppState>,gateway:resilience::Settings)->Result<DashboardSnapshot,String> {
    gateway.validate().map_err(|e|e.to_string())?;
    let service=state.proxy.lock().await;
    let old=state.store.read().gateway;
    if service.is_some() && (old.proxy_mode!=gateway.proxy_mode || old.proxy_url!=gateway.proxy_url || old.connect_timeout_seconds!=gateway.connect_timeout_seconds) {return Err("Stop the gateway before changing outbound proxy or connection timeout".into());}
    state.store.update(|c|c.gateway=gateway).map_err(|e|e.to_string())?;
    drop(service);
    Ok(snapshot(&state).await)
}
#[tauri::command]
async fn get_gateway_health(state:State<'_,AppState>)->Result<Vec<resilience::Status>,String>{
    let handle=state.proxy.lock().await;let mut result=handle.as_ref().map_or_else(Vec::new,|p|p.health.statuses());result.sort_by(|a,b|a.model_id.cmp(&b.model_id));Ok(result)
}
#[tauri::command]
async fn reset_gateway_health(state:State<'_,AppState>, model_id: Option<String>)->Result<DashboardSnapshot,String>{
    if let Some(p)=state.proxy.lock().await.as_ref(){if let Some(id)=model_id { p.health.reset_model(&id); } else { p.health.reset(); }}Ok(snapshot(&state).await)
}

#[tauri::command]
async fn start_proxy(state: State<'_, AppState>) -> Result<DashboardSnapshot, String> {
    let mut handle = state.proxy.lock().await;
    let starting = !handle.as_ref().is_some_and(|p|p.running());
    if starting {
        *handle = Some(
            proxy::start(state.store.clone())
                .await
                .map_err(|error| error.to_string())?,
        );
    }
    if let Some(proxy) = handle.as_ref() { proxy.set_paused(false); }
    if starting { reconnect_saved_agents(&state.store).map_err(|error| error.to_string())?; }
    drop(handle);
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn pause_proxy(state: State<'_, AppState>) -> Result<DashboardSnapshot, String> {
    let handle = state.proxy.lock().await;
    let proxy = handle.as_ref().filter(|p| p.running()).ok_or("Start the local proxy before pausing")?;
    proxy.set_paused(true);
    drop(handle);
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn stop_proxy(state: State<'_, AppState>) -> Result<DashboardSnapshot, String> {
    safe_stop(&state).await?;
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn preview_route(
    state: State<'_, AppState>,
    input: router::RoutePreviewInput,
) -> Result<router::RouteDecision, String> {
    let client = Client::new();
    let (mut config, decision_key) = state.store.read_with_decision_key();
    config.install_id = format!("preview:{}", config.install_id);
    if cost::is_cost_route(&config, &input) {
        let store=state.store.clone();
        config.cost_history=tauri::async_runtime::spawn_blocking(move || store.recent_cost_logs().unwrap_or_default()).await.unwrap_or_default();
    }
    router::decide(&config, &input, &client, decision_key.as_deref())
        .await
        .map(|route| route.decision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn debug_request(state: State<'_, AppState>, target: String, endpoint: String, prompt: serde_json::Value, history: Option<Vec<serde_json::Value>>, parameters: Option<serde_json::Value>, session_id: Option<String>, on_progress: tauri::ipc::Channel<serde_json::Value>) -> Result<serde_json::Value, String> {
    if !target.starts_with("autojev/") || (!prompt.is_string() && !prompt.is_array()) || prompt.as_str().is_some_and(|text| text.trim().is_empty() || text.len() > 32000) || prompt.as_array().is_some_and(|parts| parts.is_empty()) { return Err("Select a target and enter a prompt (maximum 32,000 bytes)".into()); }
    if !matches!(endpoint.as_str(), "chat/completions" | "messages" | "responses") { return Err("Invalid API format".into()); }
    if state.proxy.lock().await.is_none() { return Err("Start the local proxy before testing".into()); }
    let mut messages = history.unwrap_or_default();
    if messages.len() > 100 || messages.iter().any(|m| !matches!(m["role"].as_str(), Some("user" | "assistant")) || (!m["content"].is_string() && !m["content"].is_array())) || serde_json::to_vec(&messages).map_err(|e|e.to_string())?.len() > 24 * 1024 * 1024 { return Err("Invalid debug conversation".into()); }
    messages.push(serde_json::json!({"role":"user","content":prompt}));
    if serde_json::to_vec(&messages).map_err(|e| e.to_string())?.len() > 24 * 1024 * 1024 { return Err("Debug conversation exceeds 24 MB".into()); }
    let mut body = if endpoint == "responses" { serde_json::json!({"model": target, "input": messages}) }
        else { serde_json::json!({"model": target, "messages": messages}) };
    if let Some(params) = parameters {
        let params = params.as_object().ok_or("Parameters must be an object")?;
        for (key, value) in params {
            let target_key = if key == "max_tokens" && endpoint == "responses" { "max_output_tokens" } else if key == "max_output_tokens" && endpoint != "responses" { "max_tokens" } else { key };
            body[target_key] = value.clone();
        }
    }
    let start = std::time::Instant::now();
    let mut response = dispatch::send_http(Client::builder().build().map_err(|e|e.to_string())?
        .post(format!("http://127.0.0.1:{}/v1/{endpoint}", state.store.read().port))
        .timeout(std::time::Duration::from_secs(90))
        .header("x-autojev-session-id", session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()))
        .header("user-agent", "AutoJev/Debug")
        .header("anthropic-version", "2023-06-01").json(&body), runtime::isolated()).await.map_err(|_| "Debug request failed or timed out".to_string())?;
    let is_sse = response.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("text/event-stream"));
    let status = response.status().as_u16();
    let request_id = response.headers().get("x-autojev-request-id").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let model = response.headers().get("x-autojev-model").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let source = response.headers().get("x-autojev-route-source").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let mut bytes = Vec::new();
    let mut progress = protocol::DebugProgress::default();
    let format = protocol::Protocol::parse(&endpoint).map_err(|e| e.to_string())?;
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > 1024 * 1024 { return Err("Debug response exceeds 1 MB".into()); }
        if is_sse {
            if let Ok(events) = progress.push(&chunk, format) {
                for event in events { let _ = on_progress.send(event); }
            }
        }
        bytes.extend_from_slice(&chunk);
    }
    let elapsed_ms = start.elapsed().as_millis();
    let parsed_body = if is_sse { Some(protocol::collect_debug_stream(&bytes, protocol::Protocol::parse(&endpoint).map_err(|e| e.to_string())?, &target).map_err(|e| e.to_string())?) } else { None };
    // Correlate the completed request, never the latest global log (other agents
    // may be using the gateway concurrently). Telemetry must not fail the reply.
    let store = state.store.clone();
    let telemetry = tauri::async_runtime::spawn_blocking(move || {
        if request_id.is_empty() { return None; }
        for _ in 0..5 {
            if let Ok(Some(log)) = store.request_log(&request_id) { return Some(log); }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
    }).await.unwrap_or(None);
    Ok(serde_json::json!({"status":status,"model":model,"source":source,"elapsed_ms":elapsed_ms,"body":String::from_utf8_lossy(&bytes),"telemetry":telemetry,"request_body":body,"parsed_body":parsed_body}))
}

#[tauri::command]
async fn test_custom_agent(agent: agents::CustomAgent) -> Result<String, String> {
    agents::test_custom(agent).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_custom_agent(state: State<'_, AppState>, mut agent: agents::CustomAgent) -> Result<DashboardSnapshot, String> {
    agent.name = agent.name.trim().to_owned();
    agent.command = agent.command.trim().to_owned();
    let home = crate::runtime::home_dir().ok_or("Cannot locate home directory")?;
    let requested_path = agent.config_path.as_deref().or_else(|| agent.injection.as_ref().map(|i| i.path.as_str())).unwrap_or("").to_owned();
    agent.injection = custom_agents::infer(&agent.command, &requested_path, &home).map_err(|e| e.to_string())?;
    agent.config_path = Some(agent.injection.as_ref().map(|i| i.path.clone()).unwrap_or_else(|| requested_path.trim().to_owned()));
    agents::validate_custom(&agent).map_err(|e| e.to_string())?;
    let lock = ownership::config_lock(&home).map_err(|e| e.to_string())?;
    let config = state.store.read();
    if let Some(old) = config.custom_agents.iter().find(|a| a.id == agent.id).and_then(|a| a.injection.as_ref()) {
        if ownership::owner(&old.resolve(&home).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?.is_some() {
            return Err("Disconnect the agent before editing its configuration".into());
        }
    }
    if let Some(injection) = &agent.injection {
        let path = injection.resolve(&home).map_err(|e| e.to_string())?;
        let duplicate = config.custom_agents.iter().filter(|a| a.id != agent.id).filter_map(|a| a.injection.as_ref()).any(|i| i.resolve(&home).ok().as_ref() == Some(&path));
        let builtin = ["codex", "claude", "gemini", "grok", "kimi", "openclaw", "opencode", "hermes", "omp", "fastclaw", "cursor"].into_iter().flat_map(agents::configuration_paths).any(|p| p.canonicalize().unwrap_or(p) == path);
        if duplicate || builtin { return Err("Configuration path is already managed by another agent".into()); }
    }
    state.store.update(|config| {
        config.custom_agents.retain(|a| a.id != agent.id);
        config.custom_agents.push(agent);
    }).map_err(|e| e.to_string())?;
    drop(lock);
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn launch_agent(state: State<'_, AppState>, id: String) -> Result<(), String> {
    runtime::external_action().map_err(|e| e.to_string())?;
    let custom = state.store.read().custom_agents;
    tauri::async_runtime::spawn_blocking(move || agents::launch(&id, &custom))
        .await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_agent_config(app: tauri::AppHandle, state: State<'_, AppState>, id: String, index: usize) -> Result<(), String> {
    runtime::external_action().map_err(|e| e.to_string())?;
    if !agents::detect(&state.store.read().custom_agents).iter().any(|agent| agent.id == id) { return Err("Unknown agent".into()); }
    let custom = state.store.read().custom_agents;
    let paths = if let Some(injection) = custom.iter().find(|a| a.id == id).and_then(|a| a.injection.as_ref()) {
        vec![injection.resolve(&crate::runtime::home_dir().unwrap_or_default()).map_err(|e| e.to_string())?]
    } else { agents::configuration_paths(&id) };
    let path = paths.get(index).cloned().ok_or("Configuration file is unavailable")?;
    if !path.is_file() { return Err("Configuration file does not exist yet".into()); }
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use tauri_plugin_opener::OpenerExt;
        if path.extension().is_some_and(|ext| ext == "db") {
            return app.opener().reveal_item_in_dir(&path).map_err(|e|e.to_string());
        }
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("/usr/bin/open").arg("-t").arg(&path).status().map_err(|e|e.to_string())?;
            if status.success() { Ok(()) } else { Err("Could not open configuration file".into()) }
        }
        #[cfg(not(target_os = "macos"))]
        { app.opener().open_path(path.to_string_lossy().into_owned(), None::<&str>).map_err(|e|e.to_string()) }
    }).await.map_err(|e|e.to_string())?
}

#[tauri::command]
async fn detect_agents(state: State<'_, AppState>) -> Result<Vec<agents::AgentStatus>, String> {
    let config = state.store.read();
    tauri::async_runtime::spawn_blocking(move || detected_agents_with_selection(&config)).await.map_err(|e| e.to_string())
}

fn inject_agent(custom: &[agents::CustomAgent], id: &str, port: u16, binding: &str, catalog: &[agent_catalog::Entry]) -> anyhow::Result<()> {
    if let Some(agent) = custom.iter().find(|agent| agent.id == id) {
        let injection = agent.injection.as_ref().context("Custom agent requires manual configuration")?;
        let public = &catalog.iter().find(|entry| entry.binding == binding).context("Default model must be selected")?.id;
        return custom_agents::connect(injection, &crate::runtime::home_dir().context("Cannot locate home directory")?, port, public, id);
    }
    let api = if id == "claude" { "messages" } else if id == "codex" { "responses" } else { "chat_completions" };
    if id == "codex" {
        agents::connect_codex_catalog(port, binding, catalog, &crate::runtime::home_dir().context("Cannot locate home directory")?)
    } else if id == "claude" {
        agents::connect_claude_catalog(port, binding, catalog, &crate::runtime::home_dir().context("Cannot locate home directory")?)
    } else if agent_catalog::supported(id) {
        agent_adapters::connect_catalog(id, port, binding, api, &crate::runtime::home_dir().unwrap_or_default(), catalog)
    } else {
        let public = &catalog.iter().find(|entry| entry.binding == binding).context("Default model must be selected")?.id;
        agents::connect(id, port, public, api)
    }
}

fn reconnect_saved_agents(store: &ConfigStore) -> anyhow::Result<()> {
    if runtime::isolated() { return Ok(()); }
    let _lock = ownership::config_lock(&crate::runtime::home_dir().context("Cannot locate home directory")?)?;
    agents::repair_orphan_models(&crate::runtime::home_dir().context("Cannot locate home directory")?)?;
    let config = store.read();
    let mut errors = Vec::new();
    for (id, binding) in agent_catalog::reconnect_targets(&config) {
        let result = (|| -> anyhow::Result<()> {
            // Existing saved bindings predate the explicit auto-connect preference.
            let bindings = config.agent_catalogs.get(id).filter(|items| !items.is_empty())
                .map(|items| items.iter().map(|entry| entry.binding.clone()).collect::<Vec<_>>())
                .unwrap_or_else(|| vec![binding.to_owned()]);
            if !bindings.iter().any(|value| value == binding) { return Err(anyhow!("Default model must be selected")); }
            let catalog = agent_catalog::build(&config, &bindings)?;
            inject_agent(&config.custom_agents, id, config.port, binding, &catalog)?;
            store.update(|config| { config.agent_auto_connect.insert(id.to_owned(), true); config.agent_catalogs.insert(id.to_owned(), catalog); })?;
            Ok(())
        })();
        if let Err(error) = result { errors.push(format!("{id}: {error}")); }
    }
    if errors.is_empty() { Ok(()) } else { Err(anyhow!("Could not reconnect agents:\n{}", errors.join("\n"))) }
}

#[tauri::command]
async fn connect_agent(
    state: State<'_, AppState>,
    id: String,
    route_id: String,
    route_ids: Option<Vec<String>>,
    only_connected: Option<bool>,
) -> Result<DashboardSnapshot, String> {
    let config = state.store.read();
    let bindings = route_ids.unwrap_or_else(|| vec![route_id.clone()]);
    if !bindings.contains(&route_id) { return Err("Default model must be selected".into()); }
    let catalog = agent_catalog::build(&config, &bindings).map_err(|e| e.to_string())?;
    if let Some(model_id) = route_id.strip_prefix("model/") {
        if !config.models.iter().any(|m| m.id == model_id && m.enabled
            && config.providers.iter().any(|p| p.id == m.provider_id && p.enabled)) {
            return Err("Model is unavailable".into());
        }
    } else {
        let route = config.routes.iter().find(|r| r.id == route_id && r.enabled)
            .ok_or("Route is unavailable")?;
        router::validate_available_rule(&config, route).map_err(|e| e.to_string())?;
    }
    let candidates: Vec<_> = if let Some(model_id) = route_id.strip_prefix("model/") {
        config.models.iter().filter(|m| m.id == model_id).collect()
    } else {
        let route = config.routes.iter().find(|r| r.id == route_id).ok_or("Route is unavailable")?;
        config.models.iter().filter(|m| route.includes_model(&m.id)).collect()
    };
    let has_candidate = candidates.iter().filter(|m| m.enabled).any(|model| {
        config.providers.iter().any(|provider| provider.id == model.provider_id && provider.enabled
            && protocol::Protocol::upstream(model, provider).is_ok())
    });
    if !has_candidate { return Err("No compatible enabled candidate for this route".into()); }
    let mut service=state.proxy.lock().await;
    let config_lock=ownership::config_lock(&crate::runtime::home_dir().unwrap_or_default()).map_err(|e|e.to_string())?;
    if only_connected.unwrap_or(false) {
        let current = state.store.read();
        if current.agent_auto_connect.get(&id) == Some(&false)
            || !detected_agents_with_selection(&current).iter().any(|agent| agent.id == id && agent.connected) {
            return Err("Agent disconnected; configuration update skipped.".into());
        }
    } else if !service.as_ref().is_some_and(|p|p.running()) {
        *service=Some(proxy::start(state.store.clone()).await.map_err(|e|e.to_string())?);
    }
    inject_agent(&config.custom_agents, &id, state.store.read().port, &route_id, &catalog).map_err(|error| error.to_string())?;
    state.store.update(|config| { config.agent_selections.insert(id.clone(), route_id.clone()); config.agent_catalogs.insert(id.clone(), catalog); config.agent_auto_connect.insert(id.clone(), true); }).map_err(|e| e.to_string())?;
    drop(config_lock);
    drop(service);
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn restore_agent(
    state: State<'_, AppState>,
    id: String,
) -> Result<DashboardSnapshot, String> {
    let service=state.proxy.lock().await;
    let config_lock=ownership::config_lock(&crate::runtime::home_dir().unwrap_or_default()).map_err(|e|e.to_string())?;
    let config = state.store.read();
    let custom_injection = config.custom_agents.iter().find(|a| a.id == id).and_then(|a| a.injection.as_ref());
    let owned = custom_injection.map(|i| custom_agents::owned(i, &crate::runtime::home_dir().unwrap_or_default(), config.port)).unwrap_or_else(|| agents::owned_by(&id,config.port));
    if !owned {return Err("This agent is not connected to this gateway instance".into());}
    if let Some(binding) = detected_agents_with_selection(&config).into_iter().find(|a| a.id == id).and_then(|a| a.route_id) {
        state.store.update(|config| { config.agent_selections.insert(id.clone(), binding); }).map_err(|e| e.to_string())?;
    }
    if let Some(injection) = custom_injection { custom_agents::restore(injection, &crate::runtime::home_dir().unwrap_or_default(), config.port).map_err(|e| e.to_string())?; }
    else { agents::restore_for_gateway(&id,config.port).map_err(|error| error.to_string())?; }
    state.store.update(|config| { config.agent_auto_connect.insert(id.clone(), false); }).map_err(|e| e.to_string())?;
    drop(config_lock);drop(service);
    Ok(snapshot(&state).await)
}

#[tauri::command]
async fn test_provider_draft(state: State<'_, AppState>, provider: Provider, api_key: Option<String>) -> Result<String, String> {
    validate_provider(&provider).map_err(|e| e.to_string())?;
    if provider.test_model.trim().is_empty() { return Err("Enter a test model".into()); }
    // 订阅服务商的测试入口共用订阅准入，且不经过 API Key 与 base_url 路径。
    if subscription::is_subscription_provider(&provider) {
        // 订阅条目不带 API 类型，测试按首版唯一的客户端协议语义走 Chat Completions 准入。
        let config = state.store.read();
        return Err(match subscription::admit_target(&config, &provider, provider.test_model.trim(), protocol::Protocol::Chat) {
            Err(denial) => denial.summary(),
            Ok(()) => "Subscription generation requires the native helper, which this build does not provide.".into(),
        });
    }
    let key = api_key.filter(|k| !k.trim().is_empty()).or_else(|| {
        state.store.read().providers.iter().find(|p| p.id == provider.id)
            .and_then(|_| state.store.read_secret(&format!("provider:{}", provider.id)))
    });
    let responses = match provider.api_type.as_str() {
        "" | "chat_completions" | "messages" => false,
        "responses" => true,
        _ => return Err("Unsupported API type".into()),
    };
    let base = provider.base_url.trim_end_matches('/');
    let messages = provider.api_type == "messages";
    let base = if base.ends_with("/v1") { base.to_string() } else { format!("{base}/v1") };
    let endpoint = if messages { "messages" } else if responses { "responses" } else { "chat/completions" };
    let payload = if responses { serde_json::json!({"model":provider.test_model,"input":"Say OK","max_output_tokens":16,"stream":false}) }
        else { serde_json::json!({"model":provider.test_model,"messages":[{"role":"user","content":"Say OK"}],"max_tokens":16,"stream":false}) };
    let mut request = state.store.read().gateway.client().map_err(|e|e.to_string())?.post(format!("{base}/{endpoint}")).header("user-agent", "AutoJev/ProviderTest").header("HTTP-Referer", "https://autojev.ai").header("X-Title", "AutoJev").timeout(std::time::Duration::from_secs(30)).json(&payload);
    if messages { request = request.header("anthropic-version", "2023-06-01"); }
    if let Some(key) = key.as_ref() { request = if messages { request.header("x-api-key", key.trim()) } else { request.bearer_auth(key.trim()) }; }
    let protocol = protocol::Protocol::parse(endpoint).map_err(|e| e.to_string())?;
    let response = match state.store.dispatcher.send(dispatch::Target { provider: &provider, model_id: &provider.test_model, protocol }, request).await {
        Ok(response) => response,
        Err(_) => {
            return Err("Connection failed or timed out. Check the base URL and network.".into());
        }
    };
    if let Err(error) = provider_test::check_response(response, key.as_deref()).await {
        return Err(error);
    }
    Ok("Test request succeeded.".into())
}

#[tauri::command]
async fn test_provider(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let provider = state.store.read().providers.into_iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| "Provider not found".to_string())?;
    test_provider_draft(state, provider, None).await
}

fn validate_provider(provider: &Provider) -> anyhow::Result<()> {
    if provider.id.trim().is_empty() || provider.name.trim().is_empty() {
        return Err(anyhow!("Provider ID and name are required"));
    }
    if !provider
        .id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(anyhow!(
            "Provider ID may contain only letters, numbers, hyphens and underscores"
        ));
    }
    // 订阅服务商由官方辅助进程承载身份，没有 base_url 与 API key 可填。
    if subscription::is_subscription_provider(provider) {
        if !provider.base_url.trim().is_empty() || !provider.api_type.trim().is_empty() {
            return Err(anyhow!(
                "Subscription providers cannot carry an API base URL or API type"
            ));
        }
        return Ok(());
    }
    if !(provider.base_url.starts_with("https://")
        || provider.base_url.starts_with("http://127.0.0.1")
        || provider.base_url.starts_with("http://localhost"))
    {
        return Err(anyhow!("Provider URL must use HTTPS or loopback HTTP"));
    }
    Ok(())
}

fn validate_model(config: &AppConfig, model: &Model) -> anyhow::Result<()> {
    if model.id.trim().is_empty()
        || model.name.trim().is_empty()
        || model.model_id.trim().is_empty()
    {
        return Err(anyhow!("Model ID, provider model ID and name are required"));
    }
    if !config
        .providers
        .iter()
        .any(|provider| provider.id == model.provider_id)
    {
        return Err(anyhow!("Select an existing provider"));
    }
    if config.models.iter().any(|existing| existing.id != model.id
        && existing.provider_id == model.provider_id
        && existing.model_id.trim() == model.model_id.trim()) {
        return Err(anyhow!("This model ID already exists for this provider."));
    }
    if [model.input_cost_per_million, model.output_cost_per_million, model.cache_cost_per_million].iter().any(|cost| !cost.is_finite() || *cost < 0.0) {
        return Err(anyhow!("Model pricing cannot be negative"));
    }
    Ok(())
}

static EXIT_READY: std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
static EXIT_PENDING: std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
fn show_main(app:&tauri::AppHandle){if let Some(w)=app.get_webview_window("main"){let _=w.show();let _=w.set_focus();}}
fn request_safe_exit(app:tauri::AppHandle){
    use std::sync::atomic::Ordering;
    if EXIT_PENDING.swap(true,Ordering::SeqCst){return;}
    tauri::async_runtime::spawn(async move{
        let state=app.state::<AppState>().inner().clone();
        match safe_stop(&state).await {
            Ok(())=>{EXIT_READY.store(true,Ordering::SeqCst);app.exit(0);},
            Err(error)=>{EXIT_PENDING.store(false,Ordering::SeqCst);show_main(&app);let _=app.emit("gateway-lifecycle-error",error);}
        }
    });
}
pub fn run() {
    if let Err(error) = runtime::init() { eprintln!("AutoJev isolation: {error}"); std::process::exit(2); }
    if lifecycle::watchdog_entry(){return;}
    let mut context = tauri::generate_context!();
    if runtime::isolated() {
        context.config_mut().identifier = "ai.autojev.client.isolated".into();
        let args: Vec<_> = std::env::args().collect();
        if let Some(index) = args.iter().position(|s| s == "--autojev-ui-url") {
            let url = args.get(index + 1).and_then(|s| reqwest::Url::parse(s).ok())
                .filter(|url| dispatch::ensure_loopback(url).is_ok()).expect("Isolation UI URL must be literal loopback HTTP");
            context.config_mut().build.dev_url = Some(url);
        }
        for window in &mut context.config_mut().app.windows {
            window.incognito = true;
            window.data_directory = Some(runtime::home_dir().unwrap().join("webview"));
        }
    }
    let builder = tauri::Builder::default();
    let builder = if runtime::isolated() { builder } else { builder.plugin(tauri_plugin_updater::Builder::new().build()) };
    builder
        .on_page_load(|_window, _payload| {
            #[cfg(feature = "isolation-check")]
            if _payload.event() == tauri::webview::PageLoadEvent::Finished {
                match isolation_check::script() {
                    Ok(Some(script)) => { let _ = _window.eval(&script); },
                    Ok(None) => {},
                    Err(error) => eprintln!("Desktop isolation check: {error}"),
                }
            }
        })
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window,event|{if let tauri::WindowEvent::CloseRequested{api,..}=event{api.prevent_close();let _=window.hide();}})
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                window.set_decorations(false)?;
                window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)))?;
            }
            let root = crate::runtime::home_dir().context("find home directory")?.join(".autojev");
            let database = if app.config().identifier.ends_with(".dev") { "autojev-dev.db" } else { "autojev.db" };
            // #13：生产唯一的订阅适配器注入点就是这里——ConfigStore 构造处，代码内固定。
            // 生产实际装的是官方 Codex 适配器；默认构造（`ConfigStore::load`，测试用）仍是不提供
            // 任何辅助进程的 UnavailableAdapter。没有任何配置/环境/界面开关能把它换成替身。
            let mut store = ConfigStore::load(root.join(database))?;
            store.subscription = Arc::new(codex_helper::CodexAdapter::new());
            let store = Arc::new(store);
            // 挂起登录只存在于内存：上次进程退出时留下的 authorization_pending 无法继续，
            // 启动时归位成未连接，否则界面只允许取消、而取消又无会话可 settle。
            subscription::reconcile_orphaned_pending(&store).map_err(|error| anyhow!(error))?;
            let port = if runtime::isolated() { 0 } else if app.config().identifier.ends_with(".dev") { config::DEV_PORT } else { config::DEFAULT_PORT };
            app.manage(lifecycle::lock(port)?);
            if !runtime::isolated() { lifecycle::spawn_watchdog(port)?; }
            let port = if runtime::isolated() { 0 } else { port };
            if store.read().port != port { store.update(|config| config.port = port)?; }
            let state = AppState {
                performance: Arc::new(performance::Runner::default()),
                store: store.clone(),
                proxy: Arc::new(Mutex::new(None)),
                sessions: Arc::new(Mutex::new(subscription::SessionState::default())),
            };
            if runtime::isolated() {
                store.update(|c| c.performance_settings.enabled = false)?;
            } else {
                tauri::async_runtime::spawn(performance::schedule(state.store.clone(),state.performance.clone()));
            }
            let proxy_state = state.clone();
            let startup_app = app.handle().clone();
            if !runtime::isolated() { tauri::async_runtime::spawn(async move {
                let mut service=proxy_state.proxy.lock().await;
                if service.is_some(){return;}
                match proxy::start(store).await {
                    Ok(handle) => {
                        *service = Some(handle);
                        if let Err(error) = reconnect_saved_agents(&proxy_state.store) {
                            eprintln!("AutoJev agent reconnection: {error}");
                            let _ = startup_app.emit("gateway-lifecycle-error", error.to_string());
                        }
                    },
                    Err(error) => eprintln!("AutoJev proxy did not start automatically: {error}"),
                }
            }); }
            app.manage(state);
            let show=tauri::menu::MenuItem::with_id(app,"show","Open AutoJev",true,None::<&str>)?;
            let quit=tauri::menu::MenuItem::with_id(app,"safe-quit","Quit AutoJev (restore agents)",true,None::<&str>)?;
            let menu=tauri::menu::Menu::with_items(app,&[&show,&quit])?;
            let mut tray=tauri::tray::TrayIconBuilder::new().tooltip("AutoJev").menu(&menu).on_menu_event(|app,event|match event.id.as_ref(){"show"=>show_main(app),"safe-quit"=>request_safe_exit(app.clone()),_=>{}});
            #[cfg(target_os = "macos")]
            {
                tray = tray.icon(tauri::include_image!("icons-tray/44x44.png")).icon_as_template(true);
            }
            #[cfg(not(target_os = "macos"))]
            if let Some(icon)=app.default_window_icon(){tray=tray.icon(icon.clone());}
            tray.build(app)?;
            Ok(())
        })
        .invoke_handler(|invoke: tauri::ipc::Invoke<tauri::Wry>| {
            #[cfg(feature = "isolation-check")]
            if invoke.message.command() == "isolation_check_report" {
                let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![isolation_check::isolation_check_report];
                return handler(invoke);
            }
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
            get_snapshot,
            refresh_subscription,
            begin_subscription_login,
            cancel_subscription_login,
            logout_subscription,
            switch_subscription_account,
            get_model_performance,
            start_model_speed_tests,
            cancel_model_speed_tests,
            save_performance_settings,
            get_request_logs,
            save_provider,
            import_providers,
            delete_provider,
            save_model,
            save_route,
            delete_route,
            delete_model,
            save_policy,
            test_jev_settings,
            save_gateway_settings,
            reset_gateway_health,
            get_gateway_health,
            start_proxy,
            stop_proxy,
            pause_proxy,
            preview_route,
            debug_request,
            debug_curl::debug_curl,
            debug_curl::cancel_debug_curl,
            detect_agents,
            open_agent_config,
            launch_agent,
            save_custom_agent,
            test_custom_agent,
            connect_agent,
            restore_agent,
            test_provider,
            test_provider_draft
        ];
            handler(invoke)
        })
        .build(context)
        .expect("error while building AutoJev")
        .run(|app,event|{match event{
            tauri::RunEvent::ExitRequested{api,code,..} if code!=Some(tauri::RESTART_EXIT_CODE) && !EXIT_READY.load(std::sync::atomic::Ordering::SeqCst)=>{api.prevent_exit();request_safe_exit(app.clone());},
            #[cfg(target_os="macos")]
            tauri::RunEvent::Reopen{..}=>show_main(app),
            _=>{}
        }});
}

#[cfg(test)]
mod decision_policy_tests {
    use super::*;
    use config::DecisionProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use axum::{Router, routing::post, Json};

    #[test]
    fn changing_decision_provider_requires_a_new_key_and_keeps_saved_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decision.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        store.write_secret("autojev-cloud", "old-fixture-key").unwrap();
        let mut draft = store.read().policy;
        draft.decision_provider = DecisionProvider::Zenmux;
        draft.jev_endpoint = "https://zenmux.ai/api/v1/systemone".into();
        draft.jev_model = "typesafe/jev-1.13".into();
        assert!(save_decision_policy(&store, draft, None).is_err());
        let reopened = ConfigStore::load(path).unwrap();
        assert_eq!(reopened.read().policy.decision_provider, DecisionProvider::Openrouter);
        assert_eq!(reopened.read_secret("autojev-cloud").as_deref(), Some("old-fixture-key"));
    }

    #[test]
    fn zenmux_save_roundtrips_and_keeps_key_on_same_provider_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decision.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        store.write_secret("autojev-cloud", "old-fixture-key").unwrap();
        let mut draft = store.read().policy;
        draft.decision_provider = DecisionProvider::Zenmux;
        draft.jev_endpoint = "https://zenmux.ai/api/v1/systemone".into();
        draft.jev_model = "typesafe/jev-1.13".into();
        save_decision_policy(&store, draft, Some("new-fixture-key")).unwrap();
        let reopened = ConfigStore::load(path.clone()).unwrap();
        let mut saved = reopened.read().policy;
        assert_eq!(saved.decision_provider, DecisionProvider::Zenmux);
        assert_eq!(saved.jev_endpoint, "https://zenmux.ai/api/v1/systemone");
        assert_eq!(saved.jev_model, "typesafe/jev-1.13");
        assert_eq!(reopened.read_secret("autojev-cloud").as_deref(), Some("new-fixture-key"));
        saved.jev_model = "typesafe/jev-custom".into();
        save_decision_policy(&reopened, saved, None).unwrap();
        let again = ConfigStore::load(path).unwrap();
        assert_eq!(again.read().policy.jev_model, "typesafe/jev-custom");
        assert_eq!(again.read_secret("autojev-cloud").as_deref(), Some("new-fixture-key"));
    }

    #[test]
    fn failed_decision_save_rolls_back_new_key_and_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decision.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        store.write_secret("autojev-cloud", "old-fixture-key").unwrap();
        let mut draft = store.read().policy;
        draft.decision_provider = DecisionProvider::Zenmux;
        draft.jev_endpoint = "https://zenmux.ai/api/v1/systemone".into();
        draft.jev_model = "typesafe/jev-1.13".into();
        rusqlite::Connection::open(&path).unwrap().execute_batch("DROP TABLE app_meta;").unwrap();
        assert!(save_decision_policy(&store, draft, Some("new-fixture-key")).is_err());
        assert_eq!(store.read().policy.decision_provider, DecisionProvider::Openrouter);
        assert_eq!(store.read_secret("autojev-cloud").as_deref(), Some("old-fixture-key"));
    }

    #[tokio::test]
    async fn draft_test_never_sends_the_saved_key_to_a_new_decision_provider() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::load(dir.path().join("decision.db")).unwrap();
        store.write_secret("autojev-cloud", "old-fixture-key").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let selected = store.read().models[0].clone();
        let selected_id = selected.id.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/api/v1/systemone", post(move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| {
                let seen = seen.clone();
                let selected_id = selected_id.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(headers["authorization"], "Bearer new-fixture-key");
                    assert_eq!(body["model"], "typesafe/jev-1.13");
                    Json(serde_json::json!({"answers":{"route":{"type":"choice","choice":selected_id,"confidence":0.9}}}))
                }
            }))).await.unwrap();
        });
        let mut draft = store.read().policy;
        draft.decision_provider = DecisionProvider::Zenmux;
        draft.jev_endpoint = format!("http://127.0.0.1:{port}/api/v1/systemone");
        draft.jev_model = "typesafe/jev-1.13".into();
        assert!(probe_decision_settings(&store, draft.clone(), None).await.unwrap_err().contains("new decision API key"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(probe_decision_settings(&store, draft.clone(), Some("new-fixture-key")).await.unwrap(), selected.name);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        save_decision_policy(&store, draft.clone(), Some("new-fixture-key")).unwrap();
        assert_eq!(probe_decision_settings(&store, draft.clone(), None).await.unwrap(), selected.name);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        store.update(|config| config.models.iter_mut().for_each(|model| model.enabled = false)).unwrap();
        assert!(probe_decision_settings(&store, draft, None).await.unwrap_err().contains("Add an enabled model"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }
}

#[cfg(test)]
mod model_uniqueness_tests {
    use super::*;
    #[test]
    fn model_id_is_unique_within_each_provider() {
        let config = AppConfig::default();
        let mut model = config.models[0].clone();
        assert!(validate_model(&config, &model).is_ok());
        model.id = "new-id".into();
        model.model_id = format!(" {} ", model.model_id);
        assert!(validate_model(&config, &model).is_err());
        model.provider_id = config.providers[1].id.clone();
        assert!(validate_model(&config, &model).is_ok());
    }
}

#[cfg(test)]
mod provider_model_tests {
    #[test]
    fn tested_model_is_enabled_and_deduplicated_per_provider() {
        let mut config = crate::config::AppConfig::default();
        let mut provider = config.providers[0].clone();
        provider.test_model = "  tested-model  ".into();
        super::add_provider_test_model(&mut config, &provider);
        super::add_provider_test_model(&mut config, &provider);
        let models: Vec<_> = config.models.iter().filter(|m| m.model_id == "tested-model").collect();
        assert_eq!(models.len(), 1);
        assert!(models[0].enabled);
        assert_eq!(models[0].api_type, provider.api_type);
        provider.id = "another-provider".into();
        super::add_provider_test_model(&mut config, &provider);
        assert_eq!(config.models.iter().filter(|m| m.model_id == "tested-model").count(), 2);
    }
}

#[cfg(test)]
mod identifier_edit_tests {
    use super::*;
    #[test]
    fn provider_rename_preserves_model_references_and_rejects_conflicts() {
        let mut config=AppConfig::default();
        let mut provider=config.providers[0].clone();
        let old=provider.id.clone();
        provider.id="renamed-provider".into();
        apply_provider_edit(&mut config,provider.clone(),Some(&old),false,false).unwrap();
        assert!(!config.providers.iter().any(|p|p.id==old));
        assert!(!config.models.iter().any(|m|m.provider_id==old));
        assert!(config.models.iter().any(|m|m.provider_id==provider.id));
        let before=serde_json::to_value(&config).unwrap();
        assert!(apply_provider_edit(&mut config,provider.clone(),None,true,false).is_err());
        assert_eq!(serde_json::to_value(&config).unwrap(),before);
        provider.id=config.providers[1].id.clone();
        assert!(apply_provider_edit(&mut config,provider,Some("renamed-provider"),false,false).is_err());
        assert_eq!(serde_json::to_value(&config).unwrap(),before);
    }
    #[test]
    fn route_rename_updates_bindings_without_replacing_other_routes() {
        let mut config=AppConfig::default();
        let route=config::RouteRule {id:"old".into(),name:"Route".into(),strategy:"jev".into(),all_models:true,enabled:true,model_ids:vec![],model_settings:Default::default(),automatic_policy:None};
        config.routes=vec![route.clone()];
        config.agent_selections.insert("agent".into(),"old".into());
        config.agent_catalogs.insert("agent".into(),vec![crate::agent_catalog::Entry {id:"autojev/old".into(),name:"Route".into(),binding:"old".into()}]);
        let mut renamed=route; renamed.id="new".into();
        apply_route_edit(&mut config,renamed.clone(),Some("old"),false).unwrap();
        assert_eq!(config.routes.len(),1);
        assert_eq!(config.agent_selections["agent"],"new");
        assert_eq!(config.agent_catalogs["agent"][0].binding,"new");
        assert_eq!(config.agent_catalogs["agent"][0].id,"autojev/old");
        assert!(apply_route_edit(&mut config,renamed,None,true).is_err());
    }
}

#[cfg(test)]
mod subscription_provider_tests {
    use super::*;

    fn subscription(id: &str, kind: ProviderKind) -> Provider {
        Provider {
            preset: String::new(), api_type: String::new(), test_model: String::new(),
            id: id.into(), name: id.into(), kind, base_url: String::new(), enabled: true, has_api_key: false,
        }
    }

    #[test]
    fn subscription_providers_carry_no_api_identity() {
        let codex = subscription("codex", ProviderKind::CodexSubscription);
        validate_provider(&codex).unwrap();
        let mut with_url = codex.clone();
        with_url.base_url = "https://example.invalid".into();
        assert!(validate_provider(&with_url).is_err());
        let mut with_type = codex.clone();
        with_type.api_type = "responses".into();
        assert!(validate_provider(&with_type).is_err());
        // API 服务商的既有校验不变：仍然要求 HTTPS 或回环 base URL。
        let mut api = codex.clone();
        api.kind = ProviderKind::OpenaiCompatible;
        assert!(validate_provider(&api).is_err());
        api.base_url = "https://api.example.invalid/v1".into();
        validate_provider(&api).unwrap();
        assert!(validate_provider(&AppConfig::default().providers[0]).is_ok());
    }

    #[test]
    fn one_active_connection_per_subscription_provider() {
        let mut config = AppConfig::default();
        let codex = subscription("codex", ProviderKind::CodexSubscription);
        apply_provider_edit(&mut config, codex.clone(), None, true, false).unwrap();
        let grok = subscription("grok", ProviderKind::GrokSubscription);
        apply_provider_edit(&mut config, grok, None, true, false).unwrap();
        assert_eq!(config.subscriptions.len(), 2);
        assert_eq!(config.subscriptions["codex"].generation, 1);
        assert_eq!(config.subscriptions["grok"].generation, 1);

        // 标识重命名保留世代与已核实身份，不新建第二个连接。
        config.subscriptions.get_mut("codex").unwrap().identity = Some("fixture@example.invalid".into());
        let mut renamed = codex;
        renamed.id = "codex-work".into();
        apply_provider_edit(&mut config, renamed, Some("codex"), false, false).unwrap();
        assert!(!config.subscriptions.contains_key("codex"));
        assert_eq!(config.subscriptions["codex-work"].identity.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(config.subscriptions["codex-work"].generation, 1);
        assert!(config.subscriptions.contains_key("grok"));

        // 改成 API 服务商即放弃订阅身份，另一家不受影响。
        let mut converted = config.providers.iter().find(|p| p.id == "codex-work").unwrap().clone();
        converted.kind = ProviderKind::OpenaiCompatible;
        converted.base_url = "https://api.example.invalid/v1".into();
        apply_provider_edit(&mut config, converted, Some("codex-work"), false, false).unwrap();
        assert!(!config.subscriptions.contains_key("codex-work"));
        assert!(config.subscriptions.contains_key("grok"));
    }

    #[test]
    fn api_providers_never_gain_a_subscription_connection() {
        let mut config = AppConfig::default();
        let api = config.providers[0].clone();
        apply_provider_edit(&mut config, api, None, false, false).unwrap();
        assert!(config.subscriptions.is_empty());
    }

    /// 订阅处置测试装置：真实 `CodexAdapter`（专用目录根在临时目录）+ 已连接的 Codex 订阅服务商
    /// + 一个挂起会话 + 专用授权目录里的虚构凭据文件；另有一个已占用标识 `taken`。
    async fn dispose_fixture() -> (Arc<ConfigStore>, Arc<codex_helper::CodexAdapter>, Mutex<subscription::SessionState>, tempfile::TempDir, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let adapter = Arc::new(codex_helper::CodexAdapter::with_launch(
            home.path().to_path_buf(),
            codex_helper::HelperLaunch { program: std::path::PathBuf::from("/autojev-test-no-such-codex"), args: Vec::new() },
        ));
        let store = Arc::new(
            ConfigStore::load_with_adapters(
                directory.path().join("autojev.db"),
                Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true }),
                adapter.clone(),
            )
            .unwrap(),
        );
        store
            .update(|config| {
                let codex = subscription("codex", ProviderKind::CodexSubscription);
                config.providers.push(codex.clone());
                config.providers.push(subscription("taken", ProviderKind::CodexSubscription));
                subscription::sync_provider(config, &codex.id, &codex.kind);
                let connection = config.subscriptions.get_mut("codex").unwrap();
                connection.state = subscription::ConnectionState::Connected;
                connection.identity = Some("old@example.invalid".into());
            })
            .unwrap();
        let sessions = Mutex::new(subscription::SessionState::default());
        sessions.lock().await.sessions.insert(
            "codex".into(),
            subscription::SubscriptionSession {
                stage: subscription::LoginStage::Pending,
                attempt: 1,
                generation: 1,
                ..Default::default()
            },
        );
        let auth_home = adapter.auth_home_for("codex");
        std::fs::create_dir_all(&auth_home).unwrap();
        std::fs::write(auth_home.join("fictional-auth.json"), "{}").unwrap();
        (store, adapter, sessions, home, directory)
    }

    #[tokio::test]
    async fn a_conflicting_save_never_touches_helper_session_or_home() {
        let (store, adapter, sessions, _home, _directory) = dispose_fixture().await;
        let auth_home = adapter.auth_home_for("codex");

        // 目标标识已被占用：与 apply_provider_edit 相同的校验必须先失败，且不搬助手、不清会话。
        let renamed = subscription("taken", ProviderKind::CodexSubscription);
        let error = prepare_subscription_change(&store, &sessions, &renamed, Some("codex"), false).await.unwrap_err();
        assert_eq!(error, "Provider ID already exists");
        assert!(auth_home.join("fictional-auth.json").is_file(), "helper home must be untouched");
        assert_eq!(sessions.lock().await.session("codex").unwrap().stage, subscription::LoginStage::Pending);
        {
            let config = store.read();
            assert_eq!(config.subscriptions["codex"].state, subscription::ConnectionState::Connected);
            assert_eq!(config.subscriptions["codex"].identity.as_deref(), Some("old@example.invalid"));
            assert_eq!(config.providers.iter().find(|p| p.id == "codex").unwrap().kind, ProviderKind::CodexSubscription);
        }

        // 旧 provider 已不存在：同样先失败，零副作用。
        let ghost = subscription("codex", ProviderKind::CodexSubscription);
        let error = prepare_subscription_change(&store, &sessions, &ghost, Some("missing"), false).await.unwrap_err();
        assert_eq!(error, "Provider no longer exists");
        assert!(auth_home.join("fictional-auth.json").is_file());
        assert_eq!(sessions.lock().await.session("codex").unwrap().stage, subscription::LoginStage::Pending);
    }

    #[tokio::test]
    async fn converting_between_subscription_kinds_disposes_the_old_helper() {
        let (store, adapter, sessions, _home, _directory) = dispose_fixture().await;
        let auth_home = adapter.auth_home_for("codex");

        // Codex → Grok（同标识）：不是「同 kind 改名」，必须先释放旧 Codex 资源再改配置。
        let converted = subscription("codex", ProviderKind::GrokSubscription);
        prepare_subscription_change(&store, &sessions, &converted, Some("codex"), false).await.unwrap();

        assert!(!auth_home.exists(), "the dedicated Codex auth home must be removed");
        assert!(sessions.lock().await.session("codex").is_none(), "the in-memory session must be dropped");
        let config = store.read();
        // 连接按新类型的起点处理：未连接、无 Codex 身份与证据，等待重新登录。
        let connection = &config.subscriptions["codex"];
        assert_eq!(connection.state, subscription::ConnectionState::NotConnected);
        assert!(connection.identity.is_none() && connection.evidence.is_none());
        // 配置 kind 由随后的 apply_provider_edit 改写；处置阶段不得提前改配置。
        assert_eq!(config.providers.iter().find(|p| p.id == "codex").unwrap().kind, ProviderKind::CodexSubscription);
    }

    #[tokio::test]
    async fn renaming_within_the_same_subscription_kind_migrates_the_helper_home() {
        let (store, adapter, sessions, _home, _directory) = dispose_fixture().await;
        let old_home = adapter.auth_home_for("codex");
        let new_home = adapter.auth_home_for("codex-work");

        // 同 kind 且改名：迁移而不是释放，凭据目录跟随新标识。
        let renamed = subscription("codex-work", ProviderKind::CodexSubscription);
        prepare_subscription_change(&store, &sessions, &renamed, Some("codex"), false).await.unwrap();

        assert!(!old_home.exists());
        assert!(new_home.join("fictional-auth.json").is_file(), "the dedicated home moves with the identifier");
        // 会话键随标识迁移；在途尝试作废，新标识不得停在 Pending。
        assert!(sessions.lock().await.session("codex").is_none());
        assert_eq!(sessions.lock().await.session("codex-work").unwrap().stage, subscription::LoginStage::Idle);
        // 身份与世代保留，等 apply_provider_edit 完成配置键迁移。
        let config = store.read();
        assert_eq!(config.subscriptions["codex"].identity.as_deref(), Some("old@example.invalid"));
        assert_eq!(config.subscriptions["codex"].generation, 1);
    }
}
