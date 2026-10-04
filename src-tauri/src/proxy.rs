use std::{collections::HashMap, sync::Arc};

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{
    config::{ConfigStore, Model, Provider, ProviderKind, RouteEvent},
    protocol::{self, Protocol},
    router::{decide, ResolvedRoute, RoutePreviewInput},
};

#[derive(Clone)]
struct ProxyContext {
    store: Arc<ConfigStore>,
    client: Client,
    sessions: Arc<Mutex<HashMap<String, ResolvedRoute>>>,
    health: crate::resilience::Health,
}

pub struct ProxyHandle {
    pub port: u16,
    paused: Arc<std::sync::atomic::AtomicBool>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub health: crate::resilience::Health,
    task: tokio::task::JoinHandle<()>,
}

impl ProxyHandle {
    pub fn running(&self)->bool{!self.task.is_finished()}
    pub fn paused(&self) -> bool { self.paused.load(std::sync::atomic::Ordering::SeqCst) }
    pub fn set_paused(&self, paused: bool) { self.paused.store(paused, std::sync::atomic::Ordering::SeqCst); }
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.task).await.is_err() { self.task.abort(); }
    }
}

pub async fn start(store: Arc<ConfigStore>) -> anyhow::Result<ProxyHandle> {
    let port = store.read().port;
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    if crate::runtime::isolated() {
        crate::runtime::gateway_port(port);
        store.update(|c| c.port = port)?;
    }
    let circuit_health = crate::resilience::Health::default();
    let client = store.read().gateway.client()?;
    let context = ProxyContext {
        health: circuit_health.clone(),
        store,
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())) };
    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost".parse::<HeaderValue>()?,
            "tauri://localhost".parse::<HeaderValue>()?])
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(model_catalog))
        .route("/v1beta/models/{*operation}", post(gemini))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/responses", post(responses))
        .route("/v1/messages", post(messages))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(paused.clone(), pause_requests))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(context);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });
        if let Err(error) = server.await {
            eprintln!("AutoJev proxy stopped: {error}");
        }
    });
    Ok(ProxyHandle {
        port,
        health: circuit_health, task, paused,
        shutdown: Some(shutdown_tx) })
}

fn paused_response() -> Response {
    (StatusCode::NOT_FOUND, [("x-should-retry", "false")], Json(json!({"error":{"type":"not_found_error","code":"gateway_paused","message":"AutoJev 网关已暂停，请在 AutoJev 中恢复服务后重试。Gateway paused; resume AutoJev before sending another request."}}))).into_response()
}
async fn pause_requests(State(paused): State<Arc<std::sync::atomic::AtomicBool>>, request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if paused.load(std::sync::atomic::Ordering::SeqCst) && request.method() == Method::POST { return paused_response(); }
    next.run(request).await
}

async fn health(State(context): State<ProxyContext>) -> impl IntoResponse {
    let _=context.store.cpa.reconcile_owned(&context.store);
    let config = context.store.read();
    Json(json!({
        "status": "ok",
        "service": "autojev-local-router",
        "version": env!("CARGO_PKG_VERSION"),
        "models": config.models.iter().filter(|model| model.enabled).count()
    }))
}

fn rejected_request(context: &ProxyContext, headers: &HeaderMap, endpoint: &str,
    error: axum::extract::rejection::JsonRejection) -> Response {
    let capture = crate::traffic::Capture::new(endpoint, &json!({}), headers);
    capture.lock().unwrap().log.error = "Invalid JSON request body or content type".into();
    crate::traffic::response(error.into_response(), capture, context.store.clone())
}

async fn chat_completions(
    State(context): State<ProxyContext>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>) -> Response {
    let body = match body { Ok(Json(body)) => body, Err(error) => return rejected_request(&context, &headers, "chat/completions", error),
    };
    forward(
        context,
        headers,
        body,
        "/v1/chat/completions",
        "chat/completions")
    .await
}

async fn gemini(
    State(context): State<ProxyContext>,
    axum::extract::Path(operation): axum::extract::Path<String>,
    headers: HeaderMap, body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body { Ok(Json(body)) => body, Err(error) => return rejected_request(&context, &headers, &format!("v1beta/models/{operation}"), error),
    };
    let mut metadata = json!({});
    metadata["model"] = json!(operation.rsplit_once(':').map(|(m,_)|m).unwrap_or(""));
    metadata["stream"] = json!(operation.ends_with(":streamGenerateContent"));
    let capture = crate::traffic::Capture::new(&format!("v1beta/models/{operation}"), &metadata, &headers);
    let store = context.store.clone();
    let response = gemini_captured(context, operation, headers, body, capture.clone()).await;
    crate::traffic::response(response, capture, store)
}

async fn gemini_captured(context: ProxyContext, operation: String, headers: HeaderMap, body: Value,
    capture: crate::traffic::SharedCapture) -> Response {
    let Some((model, action)) = operation.rsplit_once(':') else {return error_response(StatusCode::BAD_REQUEST,"Invalid Gemini action");};
    if action == "countTokens" {
        return Json(json!({"totalTokens": (body.to_string().chars().count() / 4).max(1)})).into_response();
    }
    if !matches!(action,"generateContent"|"streamGenerateContent") {
        return error_response(StatusCode::NOT_IMPLEMENTED,"Unsupported Gemini action");
    }
    let mapped = match crate::gemini_bridge::request(model, &body) {
        Ok(v)=>v,Err(e)=>return error_response(StatusCode::BAD_REQUEST,&e.to_string()),
    };
    let response = forward_captured_with_subscription_policy(context, headers, mapped, "chat/completions", capture, false).await;
    if !response.status().is_success(){return response;}
    let bytes = match axum::body::to_bytes(response.into_body(),32*1024*1024).await {
        Ok(v)=>v,Err(_)=>return error_response(StatusCode::BAD_GATEWAY,"Upstream response too large"),
    };
    let converted = serde_json::from_slice(&bytes).map_err(anyhow::Error::from).and_then(|v|crate::gemini_bridge::response(&v));
    match converted {
        Ok(v) if action=="streamGenerateContent" => {
            (
            [(header::CONTENT_TYPE,"text/event-stream"),(header::CACHE_CONTROL,"no-cache")],
            format!("data: {}\n\n",v)
        ).into_response()
        }
        Ok(v)=>Json(v).into_response(),
        Err(_)=>error_response(StatusCode::BAD_GATEWAY,"Invalid upstream completion"),
    }
}

async fn responses(
    State(context): State<ProxyContext>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>) -> Response {
    let body = match body { Ok(Json(body)) => body, Err(error) => return rejected_request(&context, &headers, "responses", error),
    };
    forward(context, headers, body, "/v1/responses", "responses").await
}

async fn messages(
    State(context): State<ProxyContext>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>) -> Response {
    let body = match body { Ok(Json(body)) => body, Err(error) => return rejected_request(&context, &headers, "messages", error),
    };
    forward(context, headers, body, "/v1/messages", "messages").await
}

async fn forward(
    context: ProxyContext,
    headers: HeaderMap,
    body: Value,
    _upstream_path: &str,
    endpoint: &str) -> Response {
    let capture = crate::traffic::Capture::new(endpoint, &body, &headers);
    let store = context.store.clone();
    let response = forward_captured(context, headers, body, endpoint, capture.clone()).await;
    crate::traffic::response(response, capture, store)
}

#[cfg(test)]
pub(crate) async fn forward_test_request(
    store: Arc<ConfigStore>,
    body: Value,
    endpoint: &str) -> Response {
    let client = match store.read().gateway.client() {
        Ok(client) => client,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let context = ProxyContext {
        store,
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        health: crate::resilience::Health::default(),
    };
    forward(context, HeaderMap::new(), body, endpoint, endpoint).await
}

#[cfg(test)]
pub(crate) struct ForwardTestContext(ProxyContext);

#[cfg(test)]
impl ForwardTestContext {
    pub(crate) fn new(store: Arc<ConfigStore>) -> anyhow::Result<Self> {
        let client = store.read().gateway.client()?;
        Ok(Self(ProxyContext {
            store,
            client,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            health: crate::resilience::Health::default(),
        }))
    }

    pub(crate) async fn forward(&self, headers: HeaderMap, body: Value, endpoint: &str) -> Response {
        forward(self.0.clone(), headers, body, endpoint, endpoint).await
    }

    pub(crate) fn health_statuses(&self) -> Vec<crate::resilience::Status> {
        self.0.health.statuses()
    }

    pub(crate) fn expire_health_for_test(&self, id: &str) {
        self.0.health.expire_for_test(id);
    }
}

#[cfg(test)]
pub(crate) async fn gemini_test_request(
    store: Arc<ConfigStore>,
    operation: &str,
    body: Value) -> Response {
    let client = match store.read().gateway.client() {
        Ok(client) => client,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let context = ProxyContext {
        store: store.clone(),
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        health: crate::resilience::Health::default(),
    };
    let mut metadata = json!({});
    metadata["model"] = json!(operation.rsplit_once(':').map(|(model, _)| model).unwrap_or(""));
    metadata["stream"] = json!(operation.ends_with(":streamGenerateContent"));
    let endpoint = format!("v1beta/models/{operation}");
    let capture = crate::traffic::Capture::new(&endpoint, &metadata, &HeaderMap::new());
    let response = gemini_captured(context, operation.to_owned(), HeaderMap::new(), body, capture.clone()).await;
    crate::traffic::response(response, capture, store)
}

async fn forward_captured(context: ProxyContext, headers: HeaderMap, body: Value, endpoint: &str,
    capture: crate::traffic::SharedCapture) -> Response {
    forward_captured_with_subscription_policy(context, headers, body, endpoint, capture, true).await
}

async fn forward_captured_with_subscription_policy(context: ProxyContext, headers: HeaderMap, body: Value, endpoint: &str,
    capture: crate::traffic::SharedCapture, allow_subscription_protocol: bool,
) -> Response {
    let config = context.store.read();
    let canonical = crate::router::normalize_requested_model(&config, body["model"].as_str()).ok().flatten().unwrap_or_default();
    let mut binding = canonical.strip_prefix("autojev/").unwrap_or("").to_owned();
    if let Some(value) = headers.get("x-autojev-binding").and_then(|v|v.to_str().ok()) { binding = value.into(); }
    if let Some(agent) = headers.get("x-autojev-agent").and_then(|v|v.to_str().ok()) {
        if let Some(entry) = config.agent_catalogs.get(agent).and_then(|c|c.iter().find(|e| e.id == body["model"].as_str().unwrap_or(""))) {binding = entry.binding.clone();}
    }
    let attempts = config.routes.iter().find(|r| r.id == binding && r.enabled && matches!(r.strategy.as_str(), "round_robin" | "jev"))
        .map_or(1, |r| {
            config.models.iter().filter(|m| m.selected && crate::router::rule_includes_model(&config, r, m) && m.enabled && config.providers.iter().any(|p|p.id == m.provider_id && p.enabled)).count().max(1)
        });
    let attempts = attempts.min(config.gateway.max_attempts);
    let mut tried = std::collections::HashSet::new();
    let mut excluded_subscription_accounts = std::collections::HashSet::new();
    let mut last_response = None;
    for attempt in 0..attempts {
        let before = tried.len();
        let mut lease = None;
        let mut account_lease = None;
        let mut allow_retry = true;
        let started = std::time::Instant::now();
        let mut response = forward_attempt(context.clone(), headers.clone(), body.clone(), endpoint, capture.clone(), &mut tried, &mut excluded_subscription_accounts, &mut lease, &mut account_lease, &mut allow_retry, allow_subscription_protocol,
        ).await;
        if tried.len() == before && last_response.is_some() {return last_response.unwrap();}
        let status = response.status().as_u16();
        if response.status().is_success() {
            if let Some(lease) = lease.take() {
                response = observe_health(response, lease, account_lease.take(), config.gateway.clone(), capture.clone());
            } else if let Some(account_lease) = account_lease.take() {
                account_lease.complete(status, crate::resilience::retry_after(response.headers()), &config.gateway);
            }
        } else {
            if let Some(lease) = lease.take() {
                lease.complete(status, crate::resilience::retry_after(response.headers()), &config.gateway);
            }
            if let Some(account_lease) = account_lease.take() {
                account_lease.complete(status, crate::resilience::retry_after(response.headers()), &config.gateway);
            }
        }
        { let mut c = capture.lock().unwrap();
          let provider = c.log.provider_name.clone(); let model = c.log.model_id.clone();
          c.log.attempts.push(crate::traffic::Attempt {provider,model,status,duration_ms:started.elapsed().as_millis() as u64});
        }
        let retry = allow_retry && crate::resilience::retryable(status);
        if !retry || attempt + 1 == attempts {return response;}
        let failed_sample = {
            let mut c = capture.lock().unwrap();
            let mut log = c.log.clone();
            log.status = "error".into();
            log.upstream_duration_ms = started.elapsed().as_millis() as u64;
            c.log.performance_model_id.clear();
            log
        };
        if let Err(error) = crate::performance::record_log(&context.store,&failed_sample,false) {
            eprintln!("Could not persist failed attempt performance: {error}");
        }
        last_response = Some(response);
    }
    unreachable!()
}

fn request_compatible(body: &Value, source: Protocol, model: &crate::config::Model, provider: &crate::config::Provider) -> anyhow::Result<()> {
    Protocol::upstream(model,provider).and_then(|target| protocol::convert_request(body,source,target,&model.model_id).map(|_|()))
}

async fn forward_attempt(context: ProxyContext, headers: HeaderMap, body: Value, endpoint: &str,
    capture: crate::traffic::SharedCapture, tried: &mut std::collections::HashSet<String>, excluded_subscription_accounts: &mut std::collections::HashSet<String>, lease: &mut Option<crate::resilience::Lease>, account_lease: &mut Option<crate::resilience::Lease>, allow_retry: &mut bool, allow_subscription_protocol: bool,
) -> Response {
    let mut input = inspect_request(&body, endpoint);
    if let Some(binding) = headers.get("x-autojev-binding").and_then(|v| v.to_str().ok()) {
        input.requested_model = Some(format!("autojev/{binding}"));
    }
    let (mut config, decision_key) = context.store.read_with_decision_key();
    crate::traffic::resolve_requested_model(&mut capture.lock().unwrap().log, &config, input.requested_model.as_deref().unwrap_or(""));
    let session_config = serde_json::to_string(&(&config.routes, &config.models, &config.providers, &config.policy)).unwrap_or_default();
    let model_health_ids = config.models.iter().map(|model| (model.id.clone(), model_health_id(&config, model)))
        .collect::<HashMap<_, _>>();
    config.models.retain(|m| {
        !tried.contains(&m.id)
            && context.health.available(&model_health_ids[&m.id])
            && config.providers.iter().find(|provider| provider.id == m.provider_id).is_none_or(|provider| {
                !crate::subscription::is_subscription_provider(provider)
                    || config.subscriptions.get(&provider.id).is_none_or(|connection| {
                        let account_health = subscription_account_health_id(&provider.id, connection.generation);
                        !excluded_subscription_accounts.contains(&account_health)
                            && context.health.available(&account_health)
                    })
            })
    });
    if config.models.is_empty() {
        let mut response = error_response(StatusCode::SERVICE_UNAVAILABLE, "All candidate models are cooling down or unavailable. Retry shortly.");
        response.headers_mut().insert("retry-after", HeaderValue::from_static("5"));
        return response;
    }
    if let Some(agent) = headers.get("x-autojev-agent").and_then(|v| v.to_str().ok()) {
        let requested = body["model"].as_str().unwrap_or("");
        match config.agent_catalogs.get(agent).and_then(|items| items.iter().find(|entry| entry.id == requested)) {
            Some(entry) => input.requested_model = Some(format!("autojev/{}", entry.binding)),
            None => return error_response(StatusCode::UNPROCESSABLE_ENTITY, "Model is not in this agent's selected model list"),
        }
    }
    input.requested_model = match crate::router::normalize_requested_model(&context.store.read(), input.requested_model.as_deref()) {
        Ok(model) => model,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string()),
    };
    // 固定目标（原 ID 直调）先给出订阅准入的精确原因，而不是被候选过滤成笼统错误。
    if let Some(pinned) = input.requested_model.as_deref().and_then(|id| id.strip_prefix("autojev/model/")).map(str::to_owned) {
        let stored = context.store.read();
        if let Some(model) = stored.models.iter().find(|model| model.id == pinned) {
            if let Some(provider) = stored.providers.iter().find(|provider| provider.id == model.provider_id) {
                if let Ok(protocol) = Protocol::parse(endpoint) {
                    if let Err(denial) = crate::subscription::admit_model(&stored, model, provider, protocol) {
                        if stored.cpa_subscriptions.contains_key(&provider.id){let _=context.store.cpa.fail_hand_run(&context.store,&provider.id);}
                        return subscription_denial(denial, protocol);
                    }
                    if stored.api_sources.contains_key(&provider.id) {
                        let upstream = match Protocol::upstream(model, provider) {
                            Ok(upstream) => upstream,
                            Err(error) => return protocol_error_response(StatusCode::UNPROCESSABLE_ENTITY, protocol, &error.to_string()),
                        };
                        if let Err(error) = crate::api_sources::admit_generation_target(&context.store, &stored, provider, model, upstream) {
                            return protocol_error_response(StatusCode::PRECONDITION_REQUIRED, protocol, &error.to_string());
                        }
                    }
                }
            }
        }
    }
    capture.lock().unwrap().routing_rule(&config, input.requested_model.as_deref().unwrap_or(""));
    let requested=input.requested_model.as_deref().unwrap_or("").strip_prefix("autojev/").unwrap_or("");
    let unavailable = if let Some(id)=requested.strip_prefix("model/") {context.store.read().models.iter().any(|m|m.id==id) && !config.models.iter().any(|m|m.id==id)} else {config.routes.iter().find(|r|r.id==requested && r.enabled).is_some_and(|r|!config.models.iter().any(|m|crate::router::rule_includes_model(&config, r, m)))};
    if unavailable {let mut response=error_response(StatusCode::SERVICE_UNAVAILABLE,"All candidates for this route are cooling down or already attempted. Retry shortly.");response.headers_mut().insert("retry-after",HeaderValue::from_static("5"));return response;}
    // Validate the actual payload before selecting (or reusing) a model. Native
    // hosted tools cannot be implemented merely by translating the JSON schema.
    let source = match Protocol::parse(endpoint) {
        Ok(protocol) => protocol,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY,&error.to_string()),
    };
    let binding=input.requested_model.as_deref().unwrap_or("").strip_prefix("autojev/");
    // 被订阅准入拒绝的模型不参与候选与重试，但仍保留固定直调路径以返回具体原因。
    let pinned = binding.and_then(|id| id.strip_prefix("model/")).map(str::to_owned);
    let denied: std::collections::HashSet<String> = config.models.iter()
        .filter(|model| Some(&model.id) != pinned.as_ref())
        .filter(|model| {
            config.providers.iter().find(|provider| provider.id == model.provider_id)
            .is_some_and(|_provider| !crate::subscription::generation_ready(&config, model, source))
        })
        .map(|model| model.id.clone())
        .collect();
    config.models.retain(|model| !denied.contains(&model.id));
    let mut conversion_error=None;
    // 先按绑定定下作用域内的模型 ID，再改动 config.models：避免同时借用配置的冲突。
    let scoped_ids: Option<std::collections::HashSet<String>> = match binding {
        Some(id) if id.starts_with("model/") => Some(std::iter::once(id.trim_start_matches("model/").to_owned()).collect()),
        Some(id) => config.routes.iter().find(|r|r.id==id).map(|r| {
            config.models.iter()
            .filter(|m| crate::router::rule_includes_model(&config, r, m)).map(|m|m.id.clone()).collect()
        }),
        None => None,
    };
    config.models.retain(|model| {
        if scoped_ids.as_ref().is_some_and(|ids| !ids.contains(&model.id)) { return true; }
        let Some(provider)=config.providers.iter().find(|p|p.id==model.provider_id) else { return false; };
        match request_compatible(&body,source,model,provider) {
            Ok(()) => true,
            Err(error) => {conversion_error=Some(error.to_string());false}
        }
    });
    if let Some(error)=conversion_error {
        let scoped_available=config.models.iter().any(|model| match &scoped_ids {
            Some(ids) => ids.contains(&model.id),
            None => true,
        });
        if !scoped_available { return error_response(StatusCode::UNPROCESSABLE_ENTITY,&format!("No candidate supports this request. {error} For Codex through AutoJev, reconnect and restart Codex to apply web_search=disabled, or choose a native Responses model supporting this tool.")); }
    }
    use std::hash::{Hash, Hasher};
    let mut version = std::collections::hash_map::DefaultHasher::new();
    session_config.hash(&mut version);
    let session_id = session_id(&headers, &body).map(|id| format!("{}:{}:{}:{}", id, endpoint,
        input.requested_model.as_deref().unwrap_or(""), version.finish()));


    let resolved = if let Some(id) = session_id.as_ref() {
        let sessions = context.sessions.lock().await;
        sessions
            .get(id)
            .filter(|route| still_eligible(route, &input, &config))
            .cloned()
    } else {
        None
    };

    if crate::cost::is_cost_route(&config, &input) {
        let store = context.store.clone();
        config.cost_history = tauri::async_runtime::spawn_blocking(move || store.recent_cost_logs().unwrap_or_default()).await.unwrap_or_default();
        config.output_limit = body.get("max_completion_tokens").or_else(||body.get("max_output_tokens")).or_else(||body.get("max_tokens")).and_then(Value::as_u64);
        config.cost_incumbent = resolved.as_ref().map(|r|r.model.id.clone());
    }
    let resolved = if crate::cost::is_cost_route(&config, &input) { None } else { resolved };
    let resolved = match resolved {
        Some(mut route) => {
            route.decision.reason = "Kept the model selected for this agent session.".into();
            route
        }
        None => match decide(&config, &input, &context.client, decision_key.as_deref()).await {
            Ok(route) => {
                if let Some(id) = session_id {
                    let mut sessions = context.sessions.lock().await;
                    if sessions.len() >= 4096 { sessions.clear(); }
                    sessions.insert(id, route.clone());
                }
                route
            }
            Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string()),
        },
    };

    // Once an explicit source is selected, errors belong to that target alone.
    if config.api_sources.contains_key(&resolved.provider.id) {
        *allow_retry = false;
    }
    // 订阅模型与固定直调共用同一准入：未连接、未验证能力或缺额度依据时真实上游零派发。
    if crate::subscription::is_subscription_provider(&resolved.provider) {
        // An app-server turn may already have consumed work even when it returns an error. Never
        // retry it through another model or billing source, especially after partial text.
        *allow_retry = false;
        if !allow_subscription_protocol {
            return protocol_error_response(StatusCode::UNPROCESSABLE_ENTITY, source,
                "Gemini protocol is not supported by subscription providers.");
        }
        if resolved.provider.kind == ProviderKind::CodexSubscription
            && body.get("tools").and_then(Value::as_array).is_some_and(|tools| !tools.is_empty())
            && !resolved.model.supports_tools
        {
            return protocol_error_response(StatusCode::FORBIDDEN, source, "Codex client function tools have not been verified for this model; tool calls remain unavailable.");
        }
    }
    if let Err(denial) = crate::subscription::admit_model(&config, &resolved.model, &resolved.provider, source) {
        if config.cpa_subscriptions.contains_key(&resolved.provider.id){let _=context.store.cpa.fail_hand_run(&context.store,&resolved.provider.id);}
        return subscription_denial(denial, source);
    }
    let mut cpa_target = if config.cpa_subscriptions.contains_key(&resolved.provider.id) {
        *allow_retry=false;
        match context.store.cpa.generation_target(&context.store,&resolved.provider.id,&resolved.model,source,&body).await {
            Ok(target)=>Some(target),
            Err(error)=>return protocol_error_response(StatusCode::PRECONDITION_REQUIRED,source,&error.to_string()),
        }
    } else {None};
    let target_protocol = match Protocol::upstream(&resolved.model, &resolved.provider) {
        Ok(protocol) => protocol,
        Err(error) => {let _=crate::coding_hand_run::fail_active(&context.store,&resolved.provider.id);return protocol_error_response(StatusCode::UNPROCESSABLE_ENTITY, source, &error.to_string());},
    };
    let generation_key = match crate::api_sources::admit_generation_target(&context.store, &config, &resolved.provider, &resolved.model, target_protocol) {
        Ok(key) => key,
        Err(error) => {let _=crate::coding_hand_run::fail_active(&context.store,&resolved.provider.id);return protocol_error_response(StatusCode::PRECONDITION_REQUIRED, source, &error.to_string());},
    };

    let mut coding_lease=if config.api_sources.get(&resolved.provider.id).is_some_and(|s|s.kind==crate::api_sources::SourceKind::CodingPlan) {
        match crate::coding_hand_run::reserve(&context.store,&resolved.provider,&resolved.model,source,&body){Ok(lease)=>Some(lease),Err(error)=>return protocol_error_response(StatusCode::PRECONDITION_REQUIRED,source,&error.to_string())}
    }else{None};
    *lease = context.health.acquire(&model_health_id(&config, &resolved.model));
    if lease.is_none() {return error_response(StatusCode::SERVICE_UNAVAILABLE, "Candidate is being probed by another request. Retry shortly.");}
    tried.insert(resolved.model.id.clone());
    { let mut c=capture.lock().unwrap();c.route(&resolved);c.log.performance_context_tokens=input.estimated_context_tokens;c.log.performance_requires_vision=input.requires_vision; }
    let source = match Protocol::parse(endpoint) {
        Ok(protocol) => protocol,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let streaming = body["stream"].as_bool().unwrap_or(false);
    if cpa_target.is_none() && crate::subscription::is_subscription_provider(&resolved.provider) {
        if !matches!(resolved.provider.kind, ProviderKind::CodexSubscription | ProviderKind::GrokSubscription) {
            return protocol_error_response(StatusCode::NOT_IMPLEMENTED, source,
                "Generation through this subscription provider is not implemented.");
        }
        let Some(connection) = config.subscriptions.get(&resolved.provider.id) else {
            return protocol_error_response(StatusCode::PRECONDITION_REQUIRED, source, "The subscription connection changed before dispatch.");
        };
        let generation = connection.generation;
        let validation = match resolved.provider.kind {
            ProviderKind::CodexSubscription => crate::codex_helper::validate_generation_request(source, &body, generation),
            ProviderKind::GrokSubscription => crate::subscription::grok::validate_generation_request(source, &body, &resolved.model.model_id, generation),
            _ => unreachable!(),
        };
        if let Err(error) = validation {
            return protocol_error_response(StatusCode::UNPROCESSABLE_ENTITY, source, &error.to_string());
        }
        let identity = connection.identity.clone();
        let pre_dispatch_check = subscription_admission_check(
            resolved.provider.kind.clone(),
            context.store.clone(),
            resolved.provider.id.clone(),
            resolved.model.id.clone(),
            resolved.model.model_id.clone(),
            generation,
            identity.clone(),
            source,
        );
        capture.lock().unwrap().upstream(source, streaming);
        let request_metadata = capture.lock().unwrap().log.clone();
        let response = codex_subscription_response(
            &context, &resolved.provider, &resolved.model, generation, identity, source, body, streaming,
            &resolved.decision.source, pre_dispatch_check, capture, allow_retry, excluded_subscription_accounts, lease, account_lease).await;
        let _ = context.store.add_event(RouteEvent {
            id: request_metadata.id,
            created_at: request_metadata.created_at,
            endpoint: endpoint.into(),
            provider_name: resolved.provider.name.clone(),
            model_name: resolved.model.name.clone(),
            reason: resolved.decision.reason.clone(),
            source: resolved.decision.source.clone(),
            estimated_input_tokens: input.estimated_context_tokens,
            estimated_cost: resolved.decision.estimated_cost,
            estimated_savings: resolved.decision.estimated_savings,
            success: response.status().is_success(),
        });
        return response;
    }
    let target = match Protocol::upstream(&resolved.model, &resolved.provider) {
        Ok(protocol) => protocol,
        Err(error) => return (StatusCode::UNPROCESSABLE_ENTITY, Json(source.error(&error.to_string()))).into_response(),
    };
    let (mut body, tool_map) = match protocol::convert_request(&body, source, target, &resolved.model.model_id) {
        Ok(converted) => converted,
        Err(error) => return (StatusCode::UNPROCESSABLE_ENTITY, Json(source.error(&error.to_string()))).into_response(),
    };
    if streaming && target == Protocol::Chat {
        if !body["stream_options"].is_object() { body["stream_options"] = json!({}); }
        body["stream_options"]["include_usage"] = json!(true);
    }
    let url = cpa_target.as_ref().map(|t|format!("{}{}",t.base,target.path())).unwrap_or_else(||crate::api_sources::endpoint_url(&config, &resolved.provider, target));
    let mut request = context.client.post(url).json(&body)
        .header(header::ACCEPT, if streaming { "text/event-stream" } else { "application/json" });
    if target == Protocol::Messages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    if let Some(cpa)=&cpa_target { request=request.bearer_auth(&cpa.key); }
    else if resolved.provider.kind != ProviderKind::Ollama && !crate::subscription::is_subscription_provider(&resolved.provider) {
        let key = generation_key.as_ref().expect("admitted API generation credential");
        request = if target == Protocol::Messages { request.header("x-api-key", key) } else { request.bearer_auth(key) };
    }
    if let Some(value) = headers.get("user-agent") { request = request.header("user-agent", value); }
    // Provider-specific beta/version headers apply only to unchanged protocols.
    if source == target {
        for name in ["anthropic-version", "anthropic-beta", "openai-beta"] {
            if let Some(value) = headers.get(name) { request = request.header(name, value); }
        }
    }
    if resolved.provider.kind == ProviderKind::Openrouter {
        request = request
            .header("HTTP-Referer", "https://autojev.ai")
            .header("X-Title", "AutoJev");
    }

    let upstream = match tokio::time::timeout(std::time::Duration::from_secs(config.gateway.response_timeout_seconds), context.store.dispatcher.send(crate::dispatch::Target {
        provider: &resolved.provider, model_id: &resolved.model.model_id, protocol: target }, request),
    ).await {
        Ok(Ok(response)) => response,
        Err(_) => {if cpa_target.is_some(){let _=context.store.cpa.fail_hand_run(&context.store,&resolved.provider.id);}return error_response(StatusCode::GATEWAY_TIMEOUT, "Upstream response timed out");},
        Ok(Err(error)) => {if cpa_target.is_some(){let _=context.store.cpa.fail_hand_run(&context.store,&resolved.provider.id);}return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("Could not reach {}: {error}", resolved.provider.name));},
    };
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    if !status.is_success() {
        if let Some(lease)=cpa_target.as_mut().and_then(|t|t.lease.as_mut()){lease.reject();}
        if let Some(lease)=coding_lease.as_mut(){lease.reject();}
    }
    let redaction_key = cpa_target.as_ref().map(|t|t.key.clone()).or_else(||config.api_sources.contains_key(&resolved.provider.id).then(|| generation_key.clone()).flatten());
    let event_stream=response_headers.get(header::CONTENT_TYPE).and_then(|v|v.to_str().ok()).is_some_and(crate::protocol::is_event_stream);
    let stream:std::pin::Pin<Box<dyn futures_util::Stream<Item=Result<axum::body::Bytes,reqwest::Error>>+Send>> = if let Some(lease)=cpa_target.as_mut().and_then(|t|t.lease.take()).or_else(||coding_lease.take()){Box::pin(crate::cpa_sources::response::guarded(upstream.bytes_stream(),lease,event_stream))}else{Box::pin(upstream.bytes_stream())};
    let upstream_stream = crate::api_sources::redacted_stream(stream,redaction_key.clone(),event_stream);
    let success = status.is_success();
    capture.lock().unwrap().upstream(target, response_headers.get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok()).is_some_and(crate::protocol::is_event_stream),
    );

    let request_metadata = capture.lock().unwrap().log.clone();
    let _ = context.store.add_event(RouteEvent {
        id: request_metadata.id,
        created_at: request_metadata.created_at,
        endpoint: endpoint.into(),
        provider_name: resolved.provider.name.clone(),
        model_name: resolved.model.name.clone(),
        reason: resolved.decision.reason.clone(),
        source: resolved.decision.source.clone(),
        estimated_input_tokens: input.estimated_context_tokens,
        estimated_cost: resolved.decision.estimated_cost,
        estimated_savings: resolved.decision.estimated_savings,
        success,
    });

    let mut builder = Response::builder().status(status);
    for name in ["content-type", "cache-control", "retry-after", "x-request-id", "openai-request-id", "request-id"] {
        if let Some(value) = response_headers.get(name).filter(|value| !redaction_key.as_ref().is_some_and(|key| value.as_bytes().windows(key.len()).any(|part| part == key.as_bytes()))) {
            builder = builder.header(name, value);
        }
    }
    builder = builder
        .header("x-autojev-model", &resolved.model.model_id)
        .header("x-autojev-route-source", &resolved.decision.source);
    if source != target {
        if !success {
            let error = read_upstream_json(upstream_stream, capture.clone(), config.gateway.stream_idle_seconds).await.ok();
            let message = error.as_ref().and_then(|body| body.pointer("/error/message").and_then(Value::as_str))
                .unwrap_or("Upstream rejected the converted request");
            return builder.header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(source.error(message).to_string())).unwrap();
        }
        if streaming {
            let is_sse = response_headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok())
                .is_some_and(crate::protocol::is_event_stream);
            if !is_sse {
                return (StatusCode::BAD_GATEWAY, Json(source.error("Upstream did not return the requested event stream"))).into_response();
            }
            return builder.header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from_stream(protocol::converted_stream_observed(crate::traffic::observe(timed_stream(upstream_stream, config.gateway.stream_idle_seconds), capture.clone()), target, source, resolved.model.model_id, tool_map, { let capture = capture.clone(); move || capture.lock().unwrap().log.error = "Response conversion failed".into() },
                ))).unwrap();
        }
        let converted = match read_upstream_json(upstream_stream, capture.clone(), config.gateway.stream_idle_seconds).await.and_then(|body|
            protocol::convert_response(&body, target, source, &resolved.model.model_id, &tool_map)) {
            Ok(body) => body,
            Err(error) => return (StatusCode::BAD_GATEWAY, Json(source.error(&error.to_string()))).into_response(),
        };
        return builder.header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(converted.to_string())).unwrap();
    }
    if !streaming && success {
        return match read_upstream_json(upstream_stream,capture.clone(),config.gateway.stream_idle_seconds).await {
            Ok(body)=>{
                if body.get("error").is_some_and(|v|!v.is_null()) {
                    let code=body.pointer("/error/code").and_then(Value::as_u64).filter(|n|(400..600).contains(n)).unwrap_or(502) as u16;
                    builder=builder.status(StatusCode::from_u16(code).unwrap());
                }
                builder.header(header::CONTENT_TYPE,"application/json").body(Body::from(body.to_string())).unwrap()
            }
            Err(_)=>error_response(StatusCode::BAD_GATEWAY,"Upstream returned invalid JSON or the response body timed out"),
        };
    }
    let stream = crate::traffic::observe(timed_stream(upstream_stream, config.gateway.stream_idle_seconds), capture).map(|chunk| chunk.map_err(std::io::Error::other));
    builder.body(Body::from_stream(stream)).unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "Could not construct upstream response"))
}

const MAX_CODEX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

fn subscription_admission_check(
    expected_kind: ProviderKind,
    store: Arc<ConfigStore>,
    provider_id: String,
    model_binding_id: String,
    model_id: String,
    generation: u64,
    identity: Option<String>,
    protocol: Protocol,
) -> Arc<dyn Fn() -> Result<(), String> + Send + Sync> {
    let reserved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    Arc::new(move || {
        let validate = |config: &crate::config::AppConfig,
                        call_already_reserved: bool|
         -> Result<(), String> {
            let provider = config
                .providers
                .iter()
                .find(|provider| provider.id == provider_id)
                .ok_or_else(|| {
                    "The subscription provider was removed before dispatch".to_owned()
                })?;
            if provider.kind != expected_kind {
                return Err("The subscription provider type changed before dispatch".into());
            }
            let model = config
                .models
                .iter()
                .find(|model| model.id == model_binding_id)
                .ok_or_else(|| {
                    "The subscription model binding was removed before dispatch".to_owned()
                })?;
            if model.provider_id != provider_id || model.model_id != model_id {
                return Err("The subscription model binding changed before dispatch".into());
            }
            let connection = config
                .subscriptions
                .get(&provider_id)
                .ok_or_else(|| "The subscription connection changed before dispatch".to_owned())?;
            if connection.generation != generation || connection.identity != identity {
                return Err("The subscription account changed before dispatch".into());
            }
            let admission =
                if matches!(&expected_kind, ProviderKind::CodexSubscription | ProviderKind::GrokSubscription) && call_already_reserved {
                    crate::subscription::admit_model_with_reserved_call(
                        config, model, provider, protocol,
                    )
                } else {
                    crate::subscription::admit_model(config, model, provider, protocol)
                };
            admission.map_err(|denial| denial.summary())
        };

        if !matches!(&expected_kind, ProviderKind::CodexSubscription | ProviderKind::GrokSubscription) {
            return validate(&store.read(), false);
        }
        if reserved.load(std::sync::atomic::Ordering::SeqCst) {
            return validate(&store.read(), true);
        }

        // The subscription helper invokes this callback more than once for one incoming API request.
        // Reserve once, atomically with all ordinary admission checks; failed and cancelled
        // requests still consume the confirmed attempt count.
        let result = store
            .update(|config| {
                validate(config, false)?;
                let result = match &expected_kind {
                    ProviderKind::CodexSubscription => crate::subscription::reserve_codex_real_generation_call(config, &provider_id),
                    ProviderKind::GrokSubscription => crate::subscription::reserve_grok_real_generation_call(config, &provider_id),
                    _ => unreachable!("only Codex and Grok subscriptions have real-generation controls"),
                };
                result.map_err(|denial| denial.summary())
            })
            .map_err(|error| error.to_string())?;
        result?;
        reserved.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    })
}

fn protocol_error_response(status: StatusCode, protocol: Protocol, message: &str) -> Response {
    let mut response = (status, Json(protocol.error(message))).into_response();
    response.headers_mut().insert("x-should-retry", HeaderValue::from_static("false"));
    response
}

fn codex_text_completion(
    protocol: Protocol,
    model: &str,
    output: &str,
    finish_reason: Option<crate::subscription::GenerationFinishReason>) -> Value {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let created = chrono::Utc::now().timestamp();
    match protocol {
        Protocol::Chat => json!({
            "id": format!("chatcmpl-{suffix}"),
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{"index":0,"message":{"role":"assistant","content":output},"finish_reason":if finish_reason == Some(crate::subscription::GenerationFinishReason::MaxTokens) {"length"} else {"stop"}}]
        }),
        Protocol::Responses => json!({
            "id": format!("resp_{suffix}"),
            "object": "response",
            "created_at": created,
            "status": if finish_reason == Some(crate::subscription::GenerationFinishReason::MaxTokens) {"incomplete"} else {"completed"},
            "error": null,
            "incomplete_details": if finish_reason == Some(crate::subscription::GenerationFinishReason::MaxTokens) {json!({"reason":"max_output_tokens"})} else {Value::Null},
            "model": model,
            "output": [{"id":format!("msg_{suffix}"),"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":output,"annotations":[]}]}]
        }),
        Protocol::Messages => json!({
            "id": format!("msg_{suffix}"),
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [{"type":"text","text":output}],
            "stop_reason": if finish_reason == Some(crate::subscription::GenerationFinishReason::MaxTokens) {"max_tokens"} else {"end_turn"},
            "stop_sequence": null
        }),
    }
}

fn codex_tool_completion(protocol: Protocol, model: &str, output: &str, calls: &[crate::codex_helper::ClientToolCall]) -> anyhow::Result<Value> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let created = chrono::Utc::now().timestamp();
    Ok(match protocol {
        Protocol::Chat => json!({
            "id":format!("chatcmpl-{suffix}"), "object":"chat.completion", "created":created, "model":model,
            "choices":[{"index":0,"message":{"role":"assistant","content":if output.is_empty(){Value::Null}else{output.into()},
                "tool_calls":calls.iter().map(|call|json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}})).collect::<Vec<_>>()},"finish_reason":"tool_calls"}]
        }),
        Protocol::Responses => {
            let mut items = Vec::new();
            if !output.is_empty() {
                items.push(json!({"id":format!("msg_{suffix}"),"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":output,"annotations":[]}]}));
            }
            for (index, call) in calls.iter().enumerate() {
                items.push(json!({"id":format!("fc_{suffix}_{index}"),"type":"function_call","status":"completed","call_id":call.id,"name":call.name,"arguments":call.arguments}));
            }
            json!({"id":format!("resp_{suffix}"),"object":"response","created_at":created,"status":"completed","error":null,"incomplete_details":null,"model":model,"output":items})
        }
        Protocol::Messages => {
            let mut content = Vec::new();
            if !output.is_empty() {
                content.push(json!({"type":"text","text":output}));
            }
            for call in calls {
                let input: Value = serde_json::from_str(&call.arguments)?;
                content.push(json!({"type":"tool_use","id":call.id,"name":call.name,"input":input}));
            }
            json!({"id":format!("msg_{suffix}"),"type":"message","role":"assistant","model":model,"content":content,"stop_reason":"tool_use","stop_sequence":null})
        }
    })
}

fn subscription_generation_label(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::CodexSubscription => "Codex",
        ProviderKind::GrokSubscription => "Grok",
        _ => "Subscription",
    }
}

struct CodexSseFrame {
    bytes: Bytes,
    terminal: bool,
}

struct PendingToolBodyGuard {
    adapter: Arc<dyn crate::subscription::SubscriptionAdapter>,
    provider_id: String,
    generation: u64,
    call_ids: Arc<std::sync::Mutex<Option<Vec<String>>>>,
    delivered: bool,
}

impl PendingToolBodyGuard {
    fn mark_delivered(&mut self) {
        self.delivered = true;
        self.call_ids.lock().unwrap().take();
    }
}

impl Drop for PendingToolBodyGuard {
    fn drop(&mut self) {
        if self.delivered {
            return;
        }
        if let Some(call_ids) = self.call_ids.lock().unwrap().take() {
            self.adapter.abandon_client_tool_calls(&self.provider_id, self.generation, &call_ids);
        }
    }
}

fn subscription_account_health_id(provider_id: &str, generation: u64) -> String {
    format!("subscription-account:{provider_id}:{generation}")
}

fn model_health_id(config: &crate::config::AppConfig, model: &Model) -> String {
    let Some(provider) = config.providers.iter().find(|provider| provider.id == model.provider_id) else {
        return model.id.clone();
    };
    if !crate::subscription::is_subscription_provider(provider) {
        return model.id.clone();
    }
    let generation = config.subscriptions.get(&provider.id).map_or(0, |connection| connection.generation);
    format!("subscription-model:{}:{generation}:{}", provider.id, model.id)
}

fn subscription_request_is_current(
    context: &ProxyContext,
    provider: &Provider,
    generation: u64,
    identity: &Option<String>,
) -> bool {
    let config = context.store.read();
    config.providers.iter().any(|current| current.id == provider.id && current.kind == provider.kind)
        && config.subscriptions.get(&provider.id).is_some_and(|connection| {
            connection.state == crate::subscription::ConnectionState::Connected
                && connection.generation == generation
                && &connection.identity == identity
        })
}

fn confirmed_pre_start_failure(
    context: &ProxyContext,
    provider: &Provider,
    model: &Model,
    generation: u64,
    identity: &Option<String>,
    protocol: Protocol,
    route_source: &str,
    status: u16,
    scope: crate::subscription::GenerationFailureScope,
    retry_after_seconds: Option<u64>,
    message: &str,
    capture: crate::traffic::SharedCapture,
    allow_retry: &mut bool,
    excluded_subscription_accounts: &mut std::collections::HashSet<String>,
    account_lease: &mut Option<crate::resilience::Lease>,
) -> Response {
    if !subscription_request_is_current(context, provider, generation, identity) {
        *allow_retry = false;
        account_lease.take();
        return protocol_error_response(
            StatusCode::CONFLICT,
            protocol,
            "The subscription connection changed before its generation result was applied.",
        );
    }
    let status_is_retryable = crate::resilience::retryable(status);
    *allow_retry = status_is_retryable
        && scope != crate::subscription::GenerationFailureScope::Unknown;
    if status_is_retryable && matches!(scope,
        crate::subscription::GenerationFailureScope::Account
            | crate::subscription::GenerationFailureScope::Unknown
    ) {
        let account_health = subscription_account_health_id(&provider.id, generation);
        excluded_subscription_accounts.insert(account_health.clone());
        let health_lease = account_lease.take().or_else(|| context.health.acquire(&account_health));
        if let Some(lease) = health_lease {
            if scope == crate::subscription::GenerationFailureScope::Unknown {
                lease.complete_conservatively(status, retry_after_seconds, &context.store.read().gateway);
            } else {
                lease.complete(status, retry_after_seconds, &context.store.read().gateway);
            }
        }
    } else if let Some(lease) = account_lease.take() {
        let account_status = if scope == crate::subscription::GenerationFailureScope::Model { 200 } else { status };
        lease.complete(account_status, None, &context.store.read().gateway);
    }
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let detail = if message.trim().is_empty() {
        format!("{} rejected generation before it started.", subscription_generation_label(&provider.kind))
    } else {
        format!("{} rejected generation before it started: {}", subscription_generation_label(&provider.kind), message.trim())
    };
    let mut response = codex_json_response(status, protocol, model, route_source, protocol.error(&detail), capture);
    if status_is_retryable {
        if let Some(seconds) = retry_after_seconds.and_then(|seconds| HeaderValue::try_from(seconds.to_string()).ok()) {
            response.headers_mut().insert(header::RETRY_AFTER, seconds);
        }
    }
    response
}

async fn codex_subscription_response(
    context: &ProxyContext,
    provider: &Provider,
    model: &Model,
    generation: u64,
    identity: Option<String>,
    protocol: Protocol,
    body: Value,
    streaming: bool,
    route_source: &str,
    pre_dispatch_check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    capture: crate::traffic::SharedCapture,
    allow_retry: &mut bool,
    excluded_subscription_accounts: &mut std::collections::HashSet<String>,
    lease: &mut Option<crate::resilience::Lease>,
    account_lease: &mut Option<crate::resilience::Lease>,
) -> Response {
    let provider_label = subscription_generation_label(&provider.kind);
    let Some(adapter) = context.store.subscription.for_kind(&provider.kind).cloned() else {
        return protocol_error_response(StatusCode::SERVICE_UNAVAILABLE, protocol,
            "The subscription adapter is unavailable.");
    };
    if !adapter.supports(&provider.kind) {
        return protocol_error_response(StatusCode::SERVICE_UNAVAILABLE, protocol,
            "The subscription adapter is unavailable.");
    }
    let account_health = subscription_account_health_id(&provider.id, generation);
    *account_lease = context.health.acquire_recovery_probe(&account_health);
    if account_lease.is_none() && !context.health.available(&account_health) {
        *allow_retry = false;
        account_lease.take();
        lease.take();
        return protocol_error_response(StatusCode::SERVICE_UNAVAILABLE, protocol,
            "The subscription account is being checked after its cooldown. Retry shortly.");
    }

    if !streaming {
        let response_deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(context.store.read().gateway.response_timeout_seconds);
        let mut events = match tokio::time::timeout_at(
            response_deadline,
            adapter.generate(crate::subscription::GenerationRequest {
                provider_id: &provider.id,
                generation,
                model_id: &model.model_id,
                protocol,
                body,
                pre_dispatch_check: pre_dispatch_check.clone(),
            }),
        ).await {
            Ok(Ok(events)) => events,
            Ok(Err(_)) => {
                return codex_json_response(StatusCode::BAD_GATEWAY, protocol,
                model, route_source, protocol.error(&format!("Could not start {provider_label} generation.")), capture,
                )
            }
            Err(_) => {
                return codex_json_response(StatusCode::GATEWAY_TIMEOUT, protocol,
                model, route_source, protocol.error(&format!("{provider_label} did not start generation before the gateway timeout.")), capture,
                )
            }
        };
        let mut output = String::new();
        let mut generation_started = false;
        loop {
            let event = match tokio::time::timeout_at(response_deadline, events.next()).await {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(_) => {
                    return codex_json_response(StatusCode::GATEWAY_TIMEOUT, protocol, model,
                    route_source, protocol.error(&format!("{provider_label} generation exceeded the gateway response timeout.")), capture,
                    )
                }
            };
            match event {
                crate::subscription::GenerationEvent::RejectedBeforeStart { status, scope, retry_after_seconds, message }
                    if !generation_started && output.is_empty() => {
                        return confirmed_pre_start_failure(
                            context, provider, model, generation, &identity, protocol, route_source, status, scope,
                            retry_after_seconds, &message, capture, allow_retry, excluded_subscription_accounts,
                            account_lease,
                        );
                    }
                crate::subscription::GenerationEvent::RejectedBeforeStart { message, .. } => {
                    return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model, route_source,
                        protocol.error(&format!("{provider_label} reported a pre-start rejection after generation state became ambiguous: {message}")), capture);
                }
                crate::subscription::GenerationEvent::Chunk(delta) => {
                    generation_started = true;
                    if output.len().saturating_add(delta.len()) > MAX_CODEX_OUTPUT_BYTES {
                        return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model, route_source,
                            protocol.error(&format!("{provider_label} text output exceeded the gateway response limit.")), capture,
                        );
                    }
                    output.push_str(&delta);
                }
                crate::subscription::GenerationEvent::ToolCalls { calls } => {
                    let call_ids: Vec<_> = calls.iter().map(|call| call.id.clone()).collect();
                    let call_bytes = calls
                        .iter()
                        .map(|call| call.id.len().saturating_add(call.name.len()).saturating_add(call.arguments.len()))
                        .fold(0usize, usize::saturating_add);
                    if output.len().saturating_add(call_bytes) > MAX_CODEX_OUTPUT_BYTES {
                        adapter.abandon_client_tool_calls(&provider.id, generation, &call_ids);
                        return codex_json_response(
                            StatusCode::BAD_GATEWAY,
                            protocol,
                            model,
                            route_source,
                            protocol.error(&format!("{provider_label} tool-call output exceeded the gateway response limit.")),
                            capture,
                        );
                    }
                    let body = match codex_tool_completion(protocol, &model.model_id, &output, &calls) {
                        Ok(body) => body,
                        Err(_) => {
                            adapter.abandon_client_tool_calls(&provider.id, generation, &call_ids);
                            return codex_json_response(
                                StatusCode::BAD_GATEWAY,
                                protocol,
                                model,
                                route_source,
                                protocol.error(&format!("{provider_label} returned an invalid function-call payload.")),
                                capture,
                            )
                        }
                    };
                    return codex_json_tool_response(
                        protocol, model, route_source, body, capture, adapter.clone(),
                        provider.id.clone(), generation, call_ids,
                    );
                }
                crate::subscription::GenerationEvent::Failed { .. } => {
                    return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model, route_source,
                        protocol.error(&format!("{provider_label} generation failed.")), capture);
                }
                crate::subscription::GenerationEvent::Cancelled => {
                    let status = StatusCode::from_u16(499).unwrap_or(StatusCode::REQUEST_TIMEOUT);
                    return codex_json_response(status, protocol, model, route_source,
                        protocol.error(&format!("{provider_label} generation was interrupted.")), capture);
                }
                crate::subscription::GenerationEvent::Finished { status } if status == 200 => {
                    return codex_json_response(StatusCode::OK, protocol, model, route_source,
                        codex_text_completion(protocol, &model.model_id, &output, None), capture);
                }
                crate::subscription::GenerationEvent::FinishedWithReason { status: 200, reason } => {
                    return codex_json_response(StatusCode::OK, protocol, model, route_source,
                        codex_text_completion(protocol, &model.model_id, &output, Some(reason)), capture);
                }
                crate::subscription::GenerationEvent::Finished { .. } => {
                    return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model, route_source,
                        protocol.error(&format!("{provider_label} generation did not complete.")), capture,
                    );
                }
                crate::subscription::GenerationEvent::FinishedWithReason { .. } => {
                    return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model, route_source,
                        protocol.error(&format!("{provider_label} generation did not complete.")), capture,
                    );
                }
                crate::subscription::GenerationEvent::Started { .. } => generation_started = true,
            }
        }
        return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model, route_source,
            protocol.error(&format!("{provider_label} ended the turn without a terminal status.")), capture,
        );
    }

    let provider_id = provider.id.clone();
    let model_id = model.model_id.clone();
    let output_limit = MAX_CODEX_OUTPUT_BYTES;
    let stream_idle_seconds = context.store.read().gateway.stream_idle_seconds;
    let adapter_for_task = adapter.clone();
    let capture_for_task = capture.clone();
    let pre_dispatch_check_for_task = pre_dispatch_check;
    let pending_call_ids = Arc::new(std::sync::Mutex::new(None::<Vec<String>>));
    let pending_call_ids_for_task = pending_call_ids.clone();
    let (tx, rx) = tokio::sync::mpsc::channel::<CodexSseFrame>(8);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<crate::subscription::GenerationEvent, String>>();
    let task = tokio::spawn(async move {
        let request = crate::subscription::GenerationRequest {
            provider_id: &provider_id,
            generation,
            model_id: &model_id,
            protocol,
            body,
            pre_dispatch_check: pre_dispatch_check_for_task,
        };
        let mut events = match adapter_for_task.generate(request).await {
            Ok(events) => events,
            Err(_) => {
                let _ = ready_tx.send(Err(format!("Could not start {provider_label} generation.")));
                return;
            }
        };
        let first_event = tokio::select! {
            _ = tx.closed() => return,
            first = events.next() => first.unwrap_or_else(|| crate::subscription::GenerationEvent::Failed {
                message: format!("{provider_label} ended the turn without a terminal status."),
            }),
        };
        let rejected_before_start = matches!(first_event, crate::subscription::GenerationEvent::RejectedBeforeStart { .. });
        if ready_tx.send(Ok(first_event.clone())).is_err() { return; }
        if rejected_before_start { return; }
        let mut first_event = Some(first_event);
        let mut encoder = CodexSseEncoder::new_for_provider(protocol, &model_id, provider_label);
        let mut output_bytes = 0usize;
        loop {
            let next = if first_event.is_some() {
                first_event.take()
            } else {
                tokio::select! {
                    _ = tx.closed() => break,
                    next = events.next() => next,
                }
            };
            let Some(event) = next else {
                let frames = encoder.frames(
                    crate::subscription::GenerationEvent::Failed { message: format!("{provider_label} ended the turn without a terminal status.") },
                    &capture_for_task,
                );
                let final_frame = frames.len().saturating_sub(1);
                for (index, frame) in frames.into_iter().enumerate() {
                    if tx.send(CodexSseFrame { bytes: frame, terminal: index == final_frame }).await.is_err() { return; }
                }
                break;
            };
            let additional_bytes = match &event {
                crate::subscription::GenerationEvent::Chunk(delta) => delta.len(),
                crate::subscription::GenerationEvent::ToolCalls { calls } => calls
                    .iter()
                    .map(|call| call.id.len().saturating_add(call.name.len()).saturating_add(call.arguments.len()))
                    .fold(0usize, usize::saturating_add),
                _ => 0,
            };
            output_bytes = output_bytes.saturating_add(additional_bytes);
                if output_bytes > output_limit {
                if let crate::subscription::GenerationEvent::ToolCalls { calls } = &event {
                    let call_ids = calls.iter().map(|call| call.id.clone()).collect::<Vec<_>>();
                    adapter_for_task.abandon_client_tool_calls(&provider_id, generation, &call_ids);
                }
                let kind = if matches!(event, crate::subscription::GenerationEvent::ToolCalls { .. }) {
                    "tool-call"
                } else {
                    "text"
                };
                let frames = encoder.frames(
                        crate::subscription::GenerationEvent::Failed { message: format!("{provider_label} {kind} output exceeded the gateway response limit.") },
                        &capture_for_task,
                    );
                    let final_frame = frames.len().saturating_sub(1);
                    for (index, frame) in frames.into_iter().enumerate() {
                        if tx.send(CodexSseFrame { bytes: frame, terminal: index == final_frame }).await.is_err() { return; }
                    }
                    break;
                }
            let terminal = matches!(event,
                crate::subscription::GenerationEvent::ToolCalls { .. }
                    | crate::subscription::GenerationEvent::Finished { .. }
                | crate::subscription::GenerationEvent::FinishedWithReason { .. }
                | crate::subscription::GenerationEvent::Failed { .. }
                | crate::subscription::GenerationEvent::Cancelled
            );
            let client_tool_call_ids = if let crate::subscription::GenerationEvent::ToolCalls { calls } = &event {
                Some(calls.iter().map(|call| call.id.clone()).collect::<Vec<_>>())
            } else {
                None
            };
            if let crate::subscription::GenerationEvent::ToolCalls { calls } = &event {
                if calls.iter().all(|call| serde_json::from_str::<Value>(&call.arguments).is_ok()) {
                    *pending_call_ids_for_task.lock().unwrap() = Some(calls.iter().map(|call| call.id.clone()).collect());
                } else {
                    let call_ids = calls.iter().map(|call| call.id.clone()).collect::<Vec<_>>();
                    adapter_for_task.abandon_client_tool_calls(&provider_id, generation, &call_ids);
                }
            }
            let frames = encoder.frames(event, &capture_for_task);
            let final_frame = frames.len().saturating_sub(1);
            for (index, frame) in frames.into_iter().enumerate() {
                if tx.send(CodexSseFrame { bytes: frame, terminal: terminal && index == final_frame }).await.is_err() {
                    if let Some(call_ids) = client_tool_call_ids.as_ref() {
                        adapter_for_task.abandon_client_tool_calls(&provider_id, generation, call_ids);
                        pending_call_ids_for_task.lock().unwrap().take();
                    }
                    return;
                }
            }
            if terminal { break; }
        }
    });

    match tokio::time::timeout(
        std::time::Duration::from_secs(context.store.read().gateway.response_timeout_seconds), ready_rx).await {
        Ok(Ok(Ok(crate::subscription::GenerationEvent::RejectedBeforeStart { status, scope, retry_after_seconds, message }))) => {
            return confirmed_pre_start_failure(
                context, provider, model, generation, &identity, protocol, route_source, status, scope,
                retry_after_seconds, &message, capture, allow_retry, excluded_subscription_accounts,
                account_lease,
            );
        }
        Ok(Ok(Ok(_))) => {}
        Ok(Ok(Err(message))) => return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model,
            route_source, protocol.error(&message), capture),
        Ok(Err(_)) => {
            return codex_json_response(StatusCode::BAD_GATEWAY, protocol, model,
            route_source, protocol.error(&format!("{provider_label} generation stopped before it became ready.")), capture,
            )
        }
        Err(_) => {
            task.abort();
            return codex_json_response(StatusCode::GATEWAY_TIMEOUT, protocol, model, route_source,
                protocol.error(&format!("{provider_label} did not start generation before the gateway timeout.")), capture,
            );
        }
    }

    let guard = PendingToolBodyGuard {
        adapter: adapter.clone(),
        provider_id: provider.id.clone(),
        generation,
        call_ids: pending_call_ids,
        delivered: false,
    };
    let stream = futures_util::stream::unfold((rx, guard), |(mut rx, mut guard)| async move {
        match rx.recv().await {
            Some(frame) => {
                if frame.terminal {
                    guard.mark_delivered();
                }
                Some((Ok::<Bytes, std::io::Error>(frame.bytes), (rx, guard)))
            }
            None => {
                guard.mark_delivered();
                None
            }
        }
    });
    let stream = timed_stream(stream, stream_idle_seconds);
    let stream = crate::traffic::observe(stream, capture.clone());
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert("x-should-retry", HeaderValue::from_static("false"));
    if let Ok(value) = HeaderValue::from_str(&model.model_id) { response.headers_mut().insert("x-autojev-model", value); }
    if let Ok(value) = HeaderValue::from_str(route_source) { response.headers_mut().insert("x-autojev-route-source", value); }
    response
}

fn codex_json_response(status: StatusCode, protocol: Protocol, model: &Model, route_source: &str,
    body: Value, capture: crate::traffic::SharedCapture) -> Response {
    let encoded = body.to_string();
    capture.lock().unwrap().bytes(encoded.as_bytes());
    let mut response = Response::new(Body::from(encoded));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert("x-should-retry", HeaderValue::from_static("false"));
    if let Ok(value) = HeaderValue::from_str(&model.model_id) { response.headers_mut().insert("x-autojev-model", value); }
    if let Ok(value) = HeaderValue::from_str(route_source) { response.headers_mut().insert("x-autojev-route-source", value); }
    let _ = protocol;
    response
}

fn codex_json_tool_response(
    protocol: Protocol,
    model: &Model,
    route_source: &str,
    body: Value,
    capture: crate::traffic::SharedCapture,
    adapter: Arc<dyn crate::subscription::SubscriptionAdapter>,
    provider_id: String,
    generation: u64,
    call_ids: Vec<String>,
) -> Response {
    let encoded = body.to_string();
    capture.lock().unwrap().bytes(encoded.as_bytes());
    let guard = PendingToolBodyGuard { adapter, provider_id, generation, call_ids: Arc::new(std::sync::Mutex::new(Some(call_ids))), delivered: false };
    let stream = futures_util::stream::once(async move {
        let mut guard = guard;
        guard.mark_delivered();
        Ok::<Bytes, std::io::Error>(Bytes::from(encoded))
    });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert("x-should-retry", HeaderValue::from_static("false"));
    if let Ok(value) = HeaderValue::from_str(&model.model_id) { response.headers_mut().insert("x-autojev-model", value); }
    if let Ok(value) = HeaderValue::from_str(route_source) { response.headers_mut().insert("x-autojev-route-source", value); }
    let _ = protocol;
    response
}

struct CodexSseEncoder {
    protocol: Protocol,
    provider_label: &'static str,
    id: String,
    suffix: String,
    model: String,
    created: i64,
    output: String,
    tool_calls: Vec<crate::codex_helper::ClientToolCall>,
    started: bool,
    content_started: bool,
    terminal: bool,
}

impl CodexSseEncoder {
    #[cfg(test)]
    fn new(protocol: Protocol, model: &str) -> Self {
        Self::new_for_provider(protocol, model, "Codex")
    }

    fn new_for_provider(protocol: Protocol, model: &str, provider_label: &'static str) -> Self {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let id = match protocol {
            Protocol::Chat => format!("chatcmpl-{suffix}"),
            Protocol::Responses => format!("resp_{suffix}"),
            Protocol::Messages => format!("msg_{suffix}"),
        };
        Self { protocol, provider_label, id, suffix, model: model.into(), created: chrono::Utc::now().timestamp(), output: String::new(),
            tool_calls: Vec::new(),
            started: false, content_started: false, terminal: false,
        }
    }

    fn frame(&self, event: &str, mut value: Value) -> Bytes {
        let text = if self.protocol == Protocol::Chat {
            format!("data: {value}\n\n")
        } else {
            value["type"] = event.into();
            format!("event: {event}\ndata: {value}\n\n")
        };
        Bytes::from(text)
    }

    fn response_value(&self, status: &str, error: Option<Value>) -> Value {
        let output_completed = matches!(status, "completed" | "incomplete");
        let mut output = Vec::new();
        if self.content_started {
            output.push(json!({"id":format!("msg_{}",self.suffix),"type":"message","status":if output_completed {"completed"} else {"in_progress"},"role":"assistant","content":[{"type":"output_text","text":self.output,"annotations":[]}]}));
        }
        for (index, call) in self.tool_calls.iter().enumerate() {
            output.push(json!({"id":format!("fc_{}_{}",self.suffix,index),"type":"function_call","status":if output_completed {"completed"} else {"in_progress"},"call_id":call.id,"name":call.name,"arguments":call.arguments}));
        }
        json!({"id":self.id,"object":"response","created_at":self.created,"status":status,"error":error,"incomplete_details":null,"model":self.model,"output":output})
    }

    fn start_frames(&mut self) -> Vec<Bytes> {
        if self.started { return Vec::new(); }
        self.started = true;
        match self.protocol {
            Protocol::Chat => vec![self.frame("", json!({"id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}))],
            Protocol::Responses => vec![
                self.frame("response.created", json!({"response":self.response_value("in_progress",None)})),
                self.frame("response.in_progress", json!({"response":self.response_value("in_progress",None)})),
            ],
            Protocol::Messages => vec![self.frame("message_start", json!({"message":{"id":self.id,"type":"message","role":"assistant","model":self.model,"content":[],"stop_reason":null,"stop_sequence":null}}))],
        }
    }

    fn error_frames(&mut self, message: &str, cancelled: bool, capture: &crate::traffic::SharedCapture) -> Vec<Bytes> {
        let mut frames = self.start_frames();
        self.terminal = true;
        capture.lock().unwrap().log.error = if cancelled {
            format!("{} generation was interrupted", self.provider_label)
        } else {
            format!("{} generation failed", self.provider_label)
        };
        frames.push(match self.protocol {
            Protocol::Chat => self.frame("", json!({"error":{"message":message,"type":"server_error","code":if cancelled {"generation_cancelled"} else {"generation_failed"}}})),
            Protocol::Responses => self.frame(if cancelled {"response.cancelled"} else {"response.failed"}, json!({"response":self.response_value(if cancelled {"cancelled"} else {"failed"},Some(json!({"code":if cancelled {"generation_cancelled"} else {"server_error"},"message":message})))})),
            Protocol::Messages => self.frame("error", json!({"type":"error","error":{"type":"api_error","message":message}})),
        });
        if cancelled && self.protocol == Protocol::Messages {
            frames.push(self.frame("message_stop", json!({"type":"message_stop"})));
        }
        frames
    }

    fn finish_frames(&mut self, reason: Option<crate::subscription::GenerationFinishReason>) -> Vec<Bytes> {
        let limited = reason == Some(crate::subscription::GenerationFinishReason::MaxTokens);
        let mut frames = self.start_frames();
        self.terminal = true;
        match self.protocol {
            Protocol::Chat => {
                frames.push(self.frame("", json!({"id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":{},"finish_reason":if limited {"length"} else if self.tool_calls.is_empty() {"stop"} else {"tool_calls"}}]})));
                frames.push(Bytes::from_static(b"data: [DONE]\n\n"));
            }
            Protocol::Responses => {
                if self.content_started {
                    let item_id = format!("msg_{}",self.suffix);
                    let part = json!({"type":"output_text","text":self.output,"annotations":[]});
                    frames.push(self.frame("response.output_text.done", json!({"item_id":item_id,"output_index":0,"content_index":0,"text":self.output})));
                    frames.push(self.frame("response.content_part.done", json!({"item_id":item_id,"output_index":0,"content_index":0,"part":part})));
                    frames.push(self.frame("response.output_item.done", json!({"output_index":0,"item":{"id":item_id,"type":"message","status":"completed","role":"assistant","content":[part]}}),
                    ));
                }
                for (index, call) in self.tool_calls.iter().enumerate() {
                    let output_index = usize::from(self.content_started) + index;
                    let item = json!({"id":format!("fc_{}_{}",self.suffix,index),"type":"function_call","status":"completed","call_id":call.id,"name":call.name,"arguments":call.arguments});
                    frames.push(self.frame("response.function_call_arguments.done", json!({"item_id":item["id"],"output_index":output_index,"arguments":call.arguments})));
                    frames.push(self.frame("response.output_item.done", json!({"output_index":output_index,"item":item})));
                }
                if limited {
                    let mut response = self.response_value("incomplete", None);
                    response["incomplete_details"] = json!({"reason":"max_output_tokens"});
                    frames.push(self.frame("response.incomplete", json!({"response":response})));
                } else {
                    frames.push(self.frame("response.completed", json!({"response":self.response_value("completed",None)})));
                }
            }
            Protocol::Messages => {
                if self.content_started { frames.push(self.frame("content_block_stop", json!({"index":0}))); }
                for index in 0..self.tool_calls.len() {
                    frames.push(self.frame("content_block_stop", json!({"index":usize::from(self.content_started)+index})));
                }
                frames.push(self.frame("message_delta", json!({"delta":{"stop_reason":if limited {"max_tokens"} else if self.tool_calls.is_empty() {"end_turn"} else {"tool_use"},"stop_sequence":null}}),
                ));
                frames.push(self.frame("message_stop", json!({"type":"message_stop"})));
            }
        }
        frames
    }

    fn frames(&mut self, event: crate::subscription::GenerationEvent, capture: &crate::traffic::SharedCapture) -> Vec<Bytes> {
        use crate::subscription::GenerationEvent as Event;
        match event {
            Event::RejectedBeforeStart { message, .. } => self.error_frames(&message, false, capture),
            Event::Started { .. } => self.start_frames(),
            Event::Chunk(delta) => {
                if self.terminal || delta.is_empty() { return Vec::new(); }
                let mut frames = self.start_frames();
                self.output.push_str(&delta);
                match self.protocol {
                    Protocol::Chat => frames.push(self.frame("", json!({"id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":{"content":delta},"finish_reason":null}]}))),
                    Protocol::Responses => {
                        if !self.content_started {
                            self.content_started = true;
                            let item = json!({"id":format!("msg_{}",self.suffix),"type":"message","status":"in_progress","role":"assistant","content":[]});
                            frames.push(self.frame("response.output_item.added", json!({"output_index":0,"item":item})));
                            frames.push(self.frame("response.content_part.added", json!({"item_id":format!("msg_{}",self.suffix),"output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})));
                        }
                        frames.push(self.frame("response.output_text.delta", json!({"item_id":format!("msg_{}",self.suffix),"output_index":0,"content_index":0,"delta":delta})));
                    }
                    Protocol::Messages => {
                        if !self.content_started {
                            self.content_started = true;
                            frames.push(self.frame("content_block_start", json!({"index":0,"content_block":{"type":"text","text":""}})));
                        }
                        frames.push(self.frame("content_block_delta", json!({"index":0,"delta":{"type":"text_delta","text":delta}})));
                    }
                }
                frames
            }
            Event::ToolCalls { calls } => {
                if self.terminal || calls.is_empty() {
                    return Vec::new();
                }
                let mut frames = self.start_frames();
                for call in calls {
                    let index = self.tool_calls.len();
                    let Ok(arguments_value) = serde_json::from_str::<Value>(&call.arguments) else {
                        return self.error_frames("Codex returned invalid function-call arguments.", false, capture);
                    };
                    self.tool_calls.push(call);
                    let call = &self.tool_calls[index];
                    let output_index = usize::from(self.content_started) + index;
                    match self.protocol {
                        Protocol::Chat => frames.push(self.frame("", json!({"id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}}]},"finish_reason":null}]}))),
                        Protocol::Responses => {
                            let item = json!({"id":format!("fc_{}_{}",self.suffix,index),"type":"function_call","status":"in_progress","call_id":call.id,"name":call.name,"arguments":""});
                            frames.push(self.frame("response.output_item.added", json!({"output_index":output_index,"item":item})));
                            frames.push(self.frame("response.function_call_arguments.delta", json!({"item_id":item["id"],"output_index":output_index,"delta":call.arguments})));
                        }
                        Protocol::Messages => {
                            let block_index = usize::from(self.content_started) + index;
                            frames.push(self.frame("content_block_start", json!({"index":block_index,"content_block":{"type":"tool_use","id":call.id,"name":call.name,"input":{}}})));
                            frames.push(self.frame("content_block_delta", json!({"index":block_index,"delta":{"type":"input_json_delta","partial_json":call.arguments}})));
                        }
                    }
                    drop(arguments_value);
                }
                frames.extend(self.finish_frames(None));
                frames
            }
            Event::Finished { status: 200 } => self.finish_frames(None),
            Event::FinishedWithReason { status: 200, reason } => self.finish_frames(Some(reason)),
            Event::Finished { status } => self.error_frames(&format!("{} turn ended with status {status}.", self.provider_label), false, capture),
            Event::FinishedWithReason { status, .. } => self.error_frames(&format!("{} turn ended with status {status}.", self.provider_label), false, capture),
            Event::Failed { message } => self.error_frames(&message, false, capture),
            Event::Cancelled => self.error_frames(&format!("{} generation was interrupted.", self.provider_label), true, capture),
        }
    }
}

#[cfg(test)]
mod codex_sse_tests {
    use super::*;
    use crate::subscription::GenerationEvent;

    fn captured(protocol: Protocol) -> crate::traffic::SharedCapture {
        let capture = crate::traffic::Capture::new(
            crate::subscription::protocol_key(protocol),
            &json!({"stream":true}),
            &HeaderMap::new());
        {
            let mut capture = capture.lock().unwrap();
            capture.upstream(protocol, true);
            capture.log.status_code = StatusCode::OK.as_u16();
        }
        capture
    }

    fn collect(encoder: &mut CodexSseEncoder, event: GenerationEvent, capture: &crate::traffic::SharedCapture) -> String {
        let frames = encoder.frames(event, capture);
        let mut bytes = Vec::new();
        for frame in frames {
            capture.lock().unwrap().bytes(&frame);
            bytes.extend_from_slice(&frame);
        }
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn all_codex_stream_protocols_mark_success_failure_and_cancellation_terminally() {
        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            let success_capture = captured(protocol);
            let mut success = CodexSseEncoder::new(protocol, "fixture-model");
            let wire = collect(&mut success, GenerationEvent::Started { generation: 1 }, &success_capture)
                + &collect(&mut success, GenerationEvent::Chunk("partial".into()), &success_capture)
                + &collect(&mut success, GenerationEvent::Finished { status: 200 }, &success_capture);
            assert!(wire.contains("partial"), "{protocol:?}: {wire}");
            match protocol {
                Protocol::Chat => assert!(wire.contains("data: [DONE]"), "{wire}"),
                Protocol::Responses => {
                    assert!(wire.contains("\"type\":\"response.completed\""), "{wire}")
                }
                Protocol::Messages => assert!(wire.contains("\"type\":\"message_stop\""), "{wire}"),
            }
            assert_eq!(success_capture.lock().unwrap().finish(true).status, "success", "{protocol:?}");

            let failed_capture = captured(protocol);
            let mut failed = CodexSseEncoder::new(protocol, "fixture-model");
            let wire = collect(&mut failed, GenerationEvent::Started { generation: 2 }, &failed_capture)
                + &collect(&mut failed, GenerationEvent::Chunk("partial".into()), &failed_capture)
                + &collect(&mut failed, GenerationEvent::Failed { message: "fixture failure".into() }, &failed_capture);
            assert!(wire.contains("partial") && wire.contains("fixture failure"), "{protocol:?}: {wire}");
            match protocol {
                Protocol::Chat => assert!(!wire.contains("data: [DONE]"), "{wire}"),
                Protocol::Responses => assert!(wire.contains("event: response.failed") && !wire.contains("event: response.completed"), "{wire}"),
                Protocol::Messages => assert!(wire.contains("event: error") && !wire.contains("event: message_stop"), "{wire}"),
            }
            assert_eq!(failed_capture.lock().unwrap().finish(true).status, "error", "{protocol:?}");

            let cancelled_capture = captured(protocol);
            let mut cancelled = CodexSseEncoder::new(protocol, "fixture-model");
            let wire = collect(&mut cancelled, GenerationEvent::Started { generation: 3 }, &cancelled_capture)
                + &collect(&mut cancelled, GenerationEvent::Cancelled, &cancelled_capture);
            match protocol {
                Protocol::Chat => assert!(wire.contains("generation_cancelled") && !wire.contains("data: [DONE]"), "{wire}"),
                Protocol::Responses => assert!(wire.contains("event: response.cancelled") && !wire.contains("event: response.completed"), "{wire}"),
                Protocol::Messages => assert!(wire.contains("event: error") && wire.contains("event: message_stop"), "{wire}"),
            }
            assert_eq!(cancelled_capture.lock().unwrap().finish(true).status, "error", "{protocol:?}");
        }
    }

    #[test]
    fn all_codex_protocols_return_client_tool_calls_as_terminal_tool_use() {
        let calls = vec![
            crate::codex_helper::ClientToolCall { id: "call_ajv1_4_one".into(), name: "lookup".into(), arguments: r#"{"key":"one"}"#.into() },
            crate::codex_helper::ClientToolCall { id: "call_ajv1_4_two".into(), name: "lookup".into(), arguments: r#"{"key":"two"}"#.into() },
        ];
        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            let body = codex_tool_completion(protocol, "fixture-model", "", &calls).unwrap();
            match protocol {
                Protocol::Chat => {
                    assert_eq!(body.pointer("/choices/0/finish_reason"), Some(&json!("tool_calls")));
                    assert_eq!(body.pointer("/choices/0/message/tool_calls/1/id"), Some(&json!(calls[1].id)));
                }
                Protocol::Responses => {
                    assert_eq!(body.pointer("/output/1/type"), Some(&json!("function_call")));
                    assert_eq!(body.pointer("/output/1/call_id"), Some(&json!(calls[1].id)));
                }
                Protocol::Messages => {
                    assert_eq!(body.pointer("/stop_reason"), Some(&json!("tool_use")));
                    assert_eq!(body.pointer("/content/1/id"), Some(&json!(calls[1].id)));
                }
            }

            let capture = captured(protocol);
            let mut encoder = CodexSseEncoder::new(protocol, "fixture-model");
            let wire =
                collect(&mut encoder, GenerationEvent::Started { generation: 4 }, &capture) + &collect(&mut encoder, GenerationEvent::ToolCalls { calls: calls.clone() }, &capture);
            assert!(encoder.terminal, "{protocol:?} tool-call event must terminate the HTTP stream");
            assert!(wire.contains("call_ajv1_4_one") && wire.contains("call_ajv1_4_two"), "{protocol:?}: {wire}");
            match protocol {
                Protocol::Chat => {
                    assert!(wire.contains("\"finish_reason\":\"tool_calls\""), "{wire}")
                }
                Protocol::Responses => assert!(wire.contains("response.function_call_arguments.done") && wire.contains("response.completed"), "{wire}"),
                Protocol::Messages => assert!(wire.contains("\"stop_reason\":\"tool_use\"") && wire.contains("message_stop"), "{wire}"),
            }
            assert_eq!(capture.lock().unwrap().finish(true).status, "success", "{protocol:?}");
        }
    }
}

async fn read_upstream_json(response: impl futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send, capture: crate::traffic::SharedCapture, idle: u64) -> anyhow::Result<Value> {
    let mut body = Vec::new();
    let stream = crate::traffic::observe(response, capture.clone());
    futures_util::pin_mut!(stream);
    while let Some(chunk) = tokio::time::timeout(std::time::Duration::from_secs(idle), stream.next()).await? {
        let chunk = chunk?;
        if body.len() + chunk.len() > 16 * 1024 * 1024 { anyhow::bail!("Upstream response exceeds the 16 MiB conversion limit"); }
        body.extend_from_slice(&chunk);
    }
    capture.lock().unwrap().end_body();
    serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("Upstream returned invalid JSON"))

}

// Advisory estimate: do not tokenize image URLs/base64 as text. Image tokenization
// varies by provider/resolution; use a bounded 1024-token allowance per image.
fn estimate_request_tokens(body: &Value) -> u64 {
    fn units(value: &Value) -> u64 {
        match value {
            Value::Object(map) if matches!(map.get("type").and_then(Value::as_str), Some("image" | "image_url" | "input_image")) => 4096,
            Value::Object(map) => map.iter().map(|(key, value)| key.len() as u64 + units(value)).sum(),
            Value::Array(items) => items.iter().map(units).sum(),
            Value::String(text) => text.chars().map(|c| if c.is_ascii() { 1 } else { 4 }).sum(),
            _ => 4,
        }
    }
    ["messages", "input", "instructions", "system", "tools", "prompt"].iter()
        .filter_map(|key| body.get(key)).map(units).sum::<u64>().div_ceil(4).max(1)
}

fn inspect_request(body: &Value, endpoint: &str) -> RoutePreviewInput {

    let prompt = routing_prompt(body);
    let requires_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty());
    let requires_vision = ["input", "messages"]
        .iter()
        .filter_map(|key| body.get(key))
        .any(has_image_content);
    RoutePreviewInput {
        prompt,
        endpoint: endpoint.into(),
        requires_tools,
        requires_vision,
        estimated_context_tokens: estimate_request_tokens(body),
        requested_model: body.get("model").and_then(Value::as_str).map(str::to_owned),
    }
}

// Inspect actual content parts, never words in prompts, instructions or tool schemas.
fn has_image_content(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(has_image_content),
        Value::Object(object) => {
            if matches!(object.get("type").and_then(Value::as_str),
                Some("input_image" | "image_url" | "image")) {
                return true;
            }
            object.get("content").is_some_and(has_image_content)
        }
        _ => false,
    }
}

// Classify the latest user task, not system prompts, tool schemas or old turns.
// Only this locally derived metadata is sent to the decision service.
fn routing_prompt(body: &Value) -> String {
    for key in ["messages", "input"] {
        if let Some(items) = body.get(key).and_then(Value::as_array) {
            for item in items.iter().rev() {
                if item["role"] != "user" { continue; }
                let mut text = String::new();
                let mut remaining = 4_000;
                collect_text(&item["content"], &mut text, &mut remaining, 0);
                // Anthropic tool results also have role=user; skip them.
                if !text.trim().is_empty() { return text; }
            }
        }
    }
    let mut text = String::new();
    let mut remaining = 4_000;
    if let Some(input) = body.get("input").filter(|v| v.is_string()) {
        collect_text(input, &mut text, &mut remaining, 0);
    } else if let Some(prompt) = body.get("prompt") {
        collect_text(prompt, &mut text, &mut remaining, 0);
    }
    text
}

fn collect_text(value: &Value, result: &mut String, remaining: &mut usize, depth: usize) {
    if depth > 8 || *remaining == 0 {
        return;
    }
    match value {
        Value::String(text) => {
            if !result.is_empty() {
                result.push(' ');
                *remaining -= 1;
            }
            for c in text.chars().take(*remaining) {
                result.push(c);
                *remaining -= 1;
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_text(item, result, remaining, depth + 1);
                if *remaining == 0 { break; }
            }
        }
        Value::Object(object) if matches!(object.get("type").and_then(Value::as_str), Some("text" | "input_text")) => {
            if let Some(text) = object.get("text") {
                collect_text(text, result, remaining, depth + 1);
            }
        }
        _ => {}
    }
}

fn session_id(headers: &HeaderMap, body: &Value) -> Option<String> {
    headers
        .get("x-autojev-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| body.pointer("/metadata/user_id")
                .and_then(Value::as_str)
                .map(str::to_owned))
        .or_else(|| body.get("conversation")
                .and_then(Value::as_str)
                .map(str::to_owned))
        .or_else(|| body.get("prompt_cache_key")
                .and_then(Value::as_str)
                .map(str::to_owned))
        .or_else(|| body.get("previous_response_id")
                .and_then(Value::as_str)
                .map(str::to_owned))
}

fn still_eligible(
    route: &ResolvedRoute,
    input: &RoutePreviewInput,
    config: &crate::config::AppConfig) -> bool {
    let speed_route = input.requested_model.as_deref().and_then(|id|id.strip_prefix("autojev/"))
        .and_then(|id|config.routes.iter().find(|r|r.id==id && r.strategy=="jev"))
        .is_some_and(|r|r.automatic_policy.as_ref().unwrap_or(&config.policy).decision_preference=="speed");
    !crate::router::balanced_session_is_slow(config,input,&route.model)
        && (!speed_route || crate::router::speed_capable(&route.model,input))
        && route.model.enabled
        && config.models.iter().any(|m| m.id == route.model.id && m.enabled && m.selected && (!input.requires_vision || m.supports_vision))
        && crate::router::protocol_matches(&route.model, &route.provider, &input.endpoint)
        && Protocol::parse(&input.endpoint).is_ok_and(|protocol| crate::subscription::generation_ready(config, &route.model, protocol))
        && config
            .providers
            .iter()
            .any(|provider| provider.id == route.provider.id && provider.enabled)
}

pub(crate) fn endpoint_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") && path.starts_with("/v1/") {
        format!("{}{}", base, &path[3..])
    } else {
        format!("{base}{path}")
    }
}

/// 订阅准入被拒绝：按客户端协议返回稳定 code、原因与恢复动作，并明确标注是否可重试。
fn subscription_denial(denial: crate::subscription::Denial, protocol: Protocol) -> Response {
    let status = StatusCode::from_u16(denial.status()).unwrap_or(StatusCode::FORBIDDEN);
    let mut response = (status, Json(denial.body(protocol))).into_response();
    let headers = response.headers_mut();
    // 订阅拒绝只能靠用户动作恢复（连接、刷新、核实能力），客户端重试不会改变结果。
    headers.insert("x-should-retry", HeaderValue::from_static("false"));
    headers.insert("x-autojev-subscription-denial", HeaderValue::from_str(&denial.code).unwrap_or_else(|_| HeaderValue::from_static("denied")));
    response
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let payload = json!({
        "error": {
            "message": message,
            "type": "autojev_proxy_error",
            "code": status.as_u16()
        }
    });
    (status, Json(payload)).into_response()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn hosted_search_requires_native_responses_before_model_selection() {
        let config=crate::config::AppConfig::default();
        let mut model=config.models[0].clone();
        let provider=config.providers.iter().find(|p|p.id==model.provider_id).unwrap();
        let body=json!({"model":"autojev/fast","input":"hello","tools":[{"type":"web_search"}]});
        model.api_type="chat_completions".into();
        assert!(request_compatible(&body,Protocol::Responses,&model,provider).is_err());
        model.api_type="messages".into();
        assert!(request_compatible(&body,Protocol::Responses,&model,provider).is_err());
        model.api_type="responses".into();
        assert!(request_compatible(&body,Protocol::Responses,&model,provider).is_ok());
        model.api_type="chat_completions".into();
        assert!(request_compatible(&json!({"input":"hello"}),Protocol::Responses,&model,provider).is_ok());
    }

    #[test]
    fn image_payload_size_does_not_inflate_token_estimate() {
        for kind in ["image_url", "input_image", "image"] {
            let make = |data: String| json!({"messages":[{"role":"user","content":[{"type":"text","text":"what is this"},{"type":kind,"image_url":data.clone(),"source":{"type":"base64","data":data}}]}]});
            let small = inspect_request(&make("a".into()), "chat/completions");
            let large = inspect_request(&make("a".repeat(1_000_000)), "chat/completions");
            assert_eq!(small.estimated_context_tokens, large.estimated_context_tokens);
            assert!(large.estimated_context_tokens >= 1024 && large.estimated_context_tokens < 1100);
            assert!(large.requires_vision);
        }
        assert!(estimate_request_tokens(&json!({"messages":[{"role":"user","content":"a".repeat(40_000)}]})) >= 10000);
    }

    #[tokio::test]
    async fn image_request_invalidates_text_only_session_model() {
        let mut config = crate::config::AppConfig::default();
        config.models[0].supports_vision = false;
        let mut input = inspect_request(&json!({"model":format!("autojev/model/{}",config.models[0].id),"messages":[{"role":"user","content":"hello"}]}), "chat/completions");
        let route = decide(&config, &input, &Client::new(), None).await.unwrap();
        assert!(still_eligible(&route, &input, &config));
        input.requires_vision = true;
        assert!(!still_eligible(&route, &input, &config));
        config.models[0].supports_vision = true;
        assert!(still_eligible(&route, &input, &config));
    }

    #[tokio::test]
    async fn public_catalog_lists_only_selected_models_and_keeps_saved_agent_catalogs() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(directory.path().join("catalog.db")).unwrap());
        // 由系统分配端口：并行用例不得争抢默认端口。
        store.update(|config| { config.port = 0; config.models.truncate(1); }).unwrap();
        let public_id = {
            let config = store.read();
            format!("autojev/model/{}", config.models[0].id)
        };
        let mut same_name_cpa_model = store.read().models[0].clone();
        same_name_cpa_model.id = "cpa-stable-uuid".into();
        same_name_cpa_model.provider_id = "cpa-fictional".into();
        store.update(|config| {
            config.providers.push(Provider {
                preset: String::new(), api_type: String::new(), test_model: String::new(),
                id: "cpa-fictional".into(), name: "同名 Codex 连接".into(),
                kind: ProviderKind::CodexSubscription, base_url: String::new(), enabled: true, has_api_key: false,
            });
            same_name_cpa_model.name = config.models[0].name.clone();
            config.models.push(same_name_cpa_model);
        }).unwrap();
        store
            .update(|config| {
                config.agent_catalogs.insert(
                    "hermes".into(),
                    vec![crate::agent_catalog::Entry {
                        binding: "model/kept".into(),
                        id: "kept/frozen-model".into(),
                        name: "Frozen".into() }]);
            })
            .unwrap();
        let gateway = start(store.clone()).await.unwrap();
        let client = Client::new();
        let missing_credential: Value = client
            .get(format!("http://127.0.0.1:{}/v1/models", gateway.port))
            .send().await.unwrap().json().await.unwrap();
        assert!(missing_credential["data"].as_array().unwrap().is_empty(), "{}", missing_credential);
        store.write_secret("provider:openrouter", "fictional-only-key").unwrap();
        let catalog: Value = client
            .get(format!("http://127.0.0.1:{}/v1/models", gateway.port))
            .send().await.unwrap().json().await.unwrap();
        let ids: Vec<String> = catalog["data"]
            .as_array().unwrap().iter()
            .map(|entry| entry["id"].as_str().unwrap().to_owned()).collect();
        assert!(ids.iter().any(|id| id == &public_id), "{ids:?}");
        assert_eq!(ids.len(), 1, "unverified same-name CPA targets must not enter the callable directory: {ids:?}");
        // Agent 保存目录按保存内容原样返回：它是注入清单，不因本地目录状态改写。
        let agent: Value = client
            .get(format!("http://127.0.0.1:{}/v1/models", gateway.port))
            .header("x-autojev-agent", "hermes").send().await.unwrap().json().await.unwrap();
        assert_eq!(agent["data"][0]["id"], "kept/frozen-model");
        // 取消选择：移出公共目录，但 Agent 保存目录与直调都不受影响。
        store.update(|config| config.models[0].selected = false).unwrap();
        let catalog: Value = client
            .get(format!("http://127.0.0.1:{}/v1/models", gateway.port))
            .send().await.unwrap().json().await.unwrap();
        assert!(catalog["data"].as_array().unwrap().is_empty(), "{}", catalog);
        let agent: Value = client
            .get(format!("http://127.0.0.1:{}/v1/models", gateway.port))
            .header("x-autojev-agent", "hermes").send().await.unwrap().json().await.unwrap();
        assert_eq!(agent["data"][0]["id"], "kept/frozen-model");
        gateway.stop().await;
    }

    #[test]
    fn routing_reads_latest_user_task_in_each_protocol() {
        for endpoint in ["chat/completions", "messages", "responses"] {
            let key = if endpoint == "responses" { "input" } else { "messages" };
            let body = json!({key: [
                {"role":"system", "content":"security migration architecture"},
                {"role":"user", "content":"Investigate production authentication"},
                {"role":"assistant", "content":"security vulnerabilities"},
                {"role":"user", "content":[{"type":"text","text":"翻译"}, {"type":"text","text":"你好"}]},
                {"role":"user", "content":[{"type":"tool_result","content":"migration delete payment"}]},
                {"role":"tool", "content":"security incident"}
            ]});
            assert_eq!(inspect_request(&body, endpoint).prompt, "翻译 你好");
        }
        assert_eq!(inspect_request(&json!({"input":"Hi"}), "responses").prompt, "Hi");
        let body = json!({"messages":[{"role":"user","content":"界".repeat(10_000)}]});
        assert_eq!(inspect_request(&body, "messages").prompt.chars().count(), 4_000);
    }

    #[test]
    fn text_and_tool_schemas_do_not_require_vision() {
        let body = json!({
            "model": "autojev/auto",
            "instructions": "Use input_image and image_url when working with images.",
            "tools": [{"type":"function", "name":"view_image", "description":"Return input_image", "parameters":{"properties":{"image_url":{"type":"string"}}}}],
            "input": [{"role":"user", "content":[{"type":"input_text", "text":"Explain image_url and input_image"}]}]
        });
        let input = inspect_request(&body, "responses");
        assert!(!input.requires_vision);
        assert!(input.requires_tools);
    }

    #[test]
    fn actual_image_parts_require_vision_in_all_supported_formats() {
        for (endpoint, body) in [
            ("responses", json!({"input":[{"role":"user","content":[{"type":"input_image","image_url":"https://example.com/a.png"}]}]})),
            ("chat/completions", json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.com/a.png"}}]}]})),
            ("messages", json!({"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","data":"abc"}}]}]})),
            ("messages", json!({"messages":[{"role":"user","content":[{"type":"tool_result","content":[{"type":"image","source":{"type":"base64","data":"abc"}}]}]}]}),
            ),
        ] { assert!(inspect_request(&body, endpoint).requires_vision, "{endpoint}"); }
    }

    #[test]
    fn builds_provider_urls_without_duplicate_v1() {
        assert_eq!(
            endpoint_url("https://example.com/v1", "/v1/responses"),
            "https://example.com/v1/responses"
        );
        assert_eq!(
            endpoint_url("https://openrouter.ai/api", "/v1/messages"),
            "https://openrouter.ai/api/v1/messages"
        );
    }

    #[tokio::test]
    async fn serves_health_on_loopback() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(directory.path().join("autojev.db")).unwrap());
        let reserved=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=reserved.local_addr().unwrap().port();drop(reserved);
        store.update(|config| config.port = port).unwrap();
        store.write_secret("provider:openrouter", "fictional-only-key").unwrap();
        let handle = start(store.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        let response: Value = reqwest::get(format!("http://127.0.0.1:{port}/health"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["status"], "ok");
        let catalog:Value=reqwest::get(format!("http://127.0.0.1:{port}/v1/models")).await.unwrap().json().await.unwrap();
        assert_eq!(catalog["object"],"list");
        // Fixed model call IDs use the stable local UUID; routes retain their route ID.
        let config=store.read();let ids=catalog["data"].as_array().unwrap();assert!(!ids.is_empty());
        assert_eq!(ids[0]["id"],format!("autojev/model/{}",config.models[0].id));
        assert!(ids.iter().all(|v|crate::router::normalize_requested_model(&config,v["id"].as_str()).unwrap().is_some_and(|m|m.starts_with("autojev/"))));
        handle.stop().await;
        assert!(TcpListener::bind(("127.0.0.1",port)).await.is_ok());
    }
}

#[cfg(test)]
mod route_forward_tests {
    use super::*;
    #[tokio::test]
    async fn load_balancing_fails_over_on_rate_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let observed = calls.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
                let calls = observed.clone();
                async move {
                    let model = body["model"].as_str().unwrap().to_owned();
                    calls.lock().unwrap().push(model.clone());
                    if model == "primary" { (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error":{"message":"busy"}}))).into_response() }
                    else {Json(json!({"model":model,"choices":[]})).into_response()}
                }
            }),
                ),
            ).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(dir.path().join("test.db")).unwrap());
        store.update(|config| {
            config.models.truncate(1);
            config.providers[0].base_url = format!("http://127.0.0.1:{port}");
            config.models[0].model_id = "primary".into();
            let mut backup = config.models[0].clone(); backup.id = "backup".into(); backup.model_id = "backup".into();
            config.models.push(backup);
            config.routes = vec![crate::config::RouteRule { all_models: false, automatic_policy: None,id:"fallback".into(),name:"Fallback".into(),strategy:"round_robin".into(),enabled:true,
                model_ids:config.models.iter().map(|m|m.id.clone()).collect(),
                model_settings: std::collections::HashMap::from([(config.models[0].id.clone(),crate::config::RouteModelSettings{priority:10,weight:1})]),
                }];
        }).unwrap();
        store.write_secret("provider:openrouter", "test-only-key").unwrap();
        let context = ProxyContext {store,client:Client::new(),sessions:Default::default(),health:Default::default()};
        let response = forward(context,HeaderMap::new(),json!({"model":"autojev/fallback","messages":[{"role":"user","content":"hello"}]}),"/v1/chat/completions","chat/completions",
        ).await;
        assert_eq!(response.status(),StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(),4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["model"],"backup");
        assert_eq!(*calls.lock().unwrap(),vec!["primary","backup"]);
        server.abort();
    }
    #[tokio::test]
    async fn hermes_switches_models_in_same_session_and_rejects_unselected_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v1/chat/completions", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert!(headers.get("x-autojev-agent").is_none());
                Json(json!({"model":body["model"],"choices":[]}))
            }),
                ),
            ).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(dir.path().join("test.db")).unwrap());
        store.update(|config| {
            config.models.truncate(1);
            config.providers[0].base_url = format!("http://127.0.0.1:{port}");
            config.models[0].model_id = "first".into();
            let mut second = config.models[0].clone();
            second.id = "second-id".into(); second.model_id = "second".into();
            config.models.push(second);
            let bindings = config.models.iter().map(|m| format!("model/{}",m.id)).collect::<Vec<_>>();
            config.agent_catalogs.insert("hermes".into(), crate::agent_catalog::build(config, &bindings).unwrap());
        }).unwrap();
        store.write_secret("provider:openrouter", "test-only-key").unwrap();
        let context = ProxyContext {store:store.clone(),client:Client::new(),sessions:Default::default(),health:Default::default(),
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-autojev-agent", HeaderValue::from_static("hermes"));
        headers.insert("x-autojev-session-id", HeaderValue::from_static("same-session"));
        let models=store.read().models;
        for (public, upstream) in [(format!("autojev/model/{}",models[0].id), "first"), (format!("autojev/model/{}",models[1].id), "second"), ("not-selected".into(), "")] {
            let body = json!({"model":public,"messages":[{"role":"user","content":"hello"}]});
            let response = forward(context.clone(), headers.clone(), body, "/v1/chat/completions", "chat/completions").await;
            if upstream.is_empty() {assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);}
            else {
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
                assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["model"], upstream);
            }
        }
        upstream.abort();
    }
    #[tokio::test]
    async fn forwards_named_route_and_rejects_disabled_route_in_same_session() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v1/chat/completions", post(
                |headers: HeaderMap, Json(body): Json<Value>| async move {
                    assert_eq!(headers.get("authorization").unwrap(), "Bearer test-only-key");
                    assert!(headers.get("x-autojev-binding").is_none());
                    Json(json!({"model": body["model"], "choices": []}))
                }),
                ),
            ).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routes.db");
        let store = Arc::new(ConfigStore::load(path.clone()).unwrap());
        store.update(|config| {
            config.providers[0].base_url = format!("http://127.0.0.1:{port}");
            config.models.truncate(1);
            config.routes = vec![crate::config::RouteRule { all_models: false, automatic_policy: None, model_settings: Default::default(), id: "daily".into(), name: "Daily".into(),
                enabled: true, strategy: "fixed".into(), model_ids: vec![config.models[0].id.clone()],
                }];
        }).unwrap();
        store.write_secret("provider:openrouter", "test-only-key").unwrap();
        assert_eq!(ConfigStore::load(path).unwrap().read().routes[0].id, "daily");
        let context = ProxyContext { store: store.clone(), client: Client::new(), sessions: Default::default(), health: Default::default(),
        };
        let body = json!({"model":"public-model-id", "messages":[{"role":"user","content":"hello"}]});
        let mut headers = HeaderMap::new();
        headers.insert("x-autojev-binding", HeaderValue::from_static("daily"));
        headers.insert("x-autojev-session-id", HeaderValue::from_static("test-session"));
        let response = forward(context.clone(), headers.clone(), body.clone(), "/v1/chat/completions", "chat/completions").await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        let payload: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["model"], store.read().models[0].model_id);
        assert_eq!(store.request_logs("").unwrap()[0].status, "success");
        store.update(|config| config.routes[0].enabled = false).unwrap();
        let response = forward(context, headers, body, "/v1/chat/completions", "chat/completions").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let logs = store.request_logs("").unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].status, "error");
        assert_eq!(logs[0].status_code, 422);
        assert_eq!(logs[0].input_tokens, None);
        upstream.abort();
    }
}

#[cfg(test)]
mod protocol_forward_tests {
    use super::*;
    use crate::protocol::tests::{request, response, wire};

    #[tokio::test]
    async fn six_directions_use_upstream_path_and_auth_for_json_and_streams() {
        let protocols = [Protocol::Chat, Protocol::Responses, Protocol::Messages];
        for source in protocols {
            for target in protocols {
                if source == target { continue; }
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = listener.local_addr().unwrap().port();
                let server = tokio::spawn(async move {
                    axum::serve(listener, Router::new().route(target.path(), post(move |headers: HeaderMap, Json(body): Json<Value>| async move {
                        assert_eq!(body["model"], "test-upstream-model");
                        assert!(body.to_string().contains("call_1"));
                        assert!(headers.get("anthropic-beta").is_none());
                        if target == Protocol::Messages {
                            assert_eq!(headers["x-api-key"], "test-only-key");
                            assert!(headers.get("authorization").is_none());
                            assert_eq!(headers["anthropic-version"], "2023-06-01");
                            assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "call_1");
                        } else {
                            assert_eq!(headers["authorization"], "Bearer test-only-key");
                            assert!(headers.get("x-api-key").is_none());
                            assert!(headers.get("anthropic-version").is_none());
                        }
                        if body["stream"] == true {
                            let chunks = wire(target).as_bytes().chunks(7).map(|bytes| Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(bytes))).collect::<Vec<_>>();
                            ([("content-type", "text/event-stream")], Body::from_stream(futures_util::stream::iter(chunks))).into_response()
                        } else { Json(response(target)).into_response() }
                    }),
                        ),
                    ).await.unwrap();
                });
                let dir = tempfile::tempdir().unwrap();
                let store = Arc::new(ConfigStore::load(dir.path().join("protocol.db")).unwrap());
                store.update(|config| {
                    config.models.truncate(1);
                    config.models[0].model_id = "test-upstream-model".into();
                    config.models[0].api_type = target.path().trim_start_matches("/v1/").into();
                    config.providers[0].base_url = format!("http://127.0.0.1:{port}/v1");
                    config.providers[0].kind = ProviderKind::OpenaiCompatible;
                    config.policy.use_jev_when_ambiguous = false;
                }).unwrap();
                store.write_secret("provider:openrouter", "test-only-key").unwrap();
                let context = ProxyContext { store, client: Client::new(), sessions: Default::default(), health: Default::default() };
                for streaming in [false, true] {
                    let mut body = request(source, streaming);
                    body["model"] = "autojev/auto".into();
                    let mut headers = HeaderMap::new();
                    headers.insert("anthropic-beta", HeaderValue::from_static("client-only-feature"));
                    let reply = forward(context.clone(), headers, body, source.path(), source.path().trim_start_matches("/v1/")).await;
                    assert_eq!(reply.status(), StatusCode::OK, "{source:?} → {target:?}");
                    assert_eq!(reply.headers()["x-autojev-model"], "test-upstream-model");
                    let content_type = reply.headers()[header::CONTENT_TYPE].to_str().unwrap().to_owned();
                    let bytes = axum::body::to_bytes(reply.into_body(), 64 * 1024).await.unwrap();
                    if streaming {
                        assert!(content_type.starts_with("text/event-stream"));
                        let text = String::from_utf8(bytes.to_vec()).unwrap();
                        assert!(text.contains(match source { Protocol::Chat => "[DONE]", Protocol::Responses => "response.completed", Protocol::Messages => "message_stop",
                            }), "{text}");
                    } else {
                        let body: Value = serde_json::from_slice(&bytes).unwrap();
                        assert!(body.to_string().contains("call_2"));
                        assert!(body.to_string().contains("你好"));
                        assert!(content_type.starts_with("application/json"));
                    }
                }
                let logs = context.store.request_logs("").unwrap();
                assert_eq!(logs.len(), 2);
                for log in logs {
                    assert_eq!(log.status, "success", "{source:?} → {target:?}: {}", log.error);
                    assert_eq!(log.input_tokens, Some(12));
                    assert_eq!(log.output_tokens, Some(7));
                    assert!(log.estimated_cost.is_some());
                }
                server.abort();
            }
        }
    }

    #[tokio::test]
    async fn preserves_upstream_http_errors_in_client_protocol() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v1/chat/completions", post(|| async {
                (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error":{"message":"rate limited"}})))
            })),
            ).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(dir.path().join("errors.db")).unwrap());
        store.update(|config| { config.models.truncate(1); config.providers[0].base_url = format!("http://127.0.0.1:{port}"); }).unwrap();
        store.write_secret("provider:openrouter", "test-only-key").unwrap();
        let mut body = request(Protocol::Messages, false);
        body["model"] = "auto".into();
        let reply = forward(ProxyContext { store, client: Client::new(), sessions: Default::default(), health: Default::default() }, HeaderMap::new(), body, "/v1/messages", "messages",
        ).await;
        assert_eq!(reply.status(), StatusCode::TOO_MANY_REQUESTS);
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(reply.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["message"], "rate limited");
        server.abort();
    }
}

fn timed_stream<S,E>(stream:S,seconds:u64)->impl futures_util::Stream<Item=Result<axum::body::Bytes,std::io::Error>>+Send
where S:futures_util::Stream<Item=Result<axum::body::Bytes,E>>+Send,E:std::error::Error+Send+Sync+'static,
{
    futures_util::stream::unfold((Box::pin(stream),false),move |(mut stream,done)|async move{
        if done{return None;}
        match tokio::time::timeout(std::time::Duration::from_secs(seconds),stream.next()).await {
            Ok(Some(chunk))=>{let failed=chunk.is_err();Some((chunk.map_err(std::io::Error::other),(stream,failed)))}
            Ok(None)=>None,
            Err(_)=>Some((Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"Upstream idle timeout")),(stream,true))),
        }
    })
}
struct AccountProbeCompletion {
    lease: Option<crate::resilience::Lease>,
    settings: crate::resilience::Settings,
    capture: crate::traffic::SharedCapture,
}

impl AccountProbeCompletion {
    fn complete(&mut self, status: u16) {
        if let Some(lease) = self.lease.take() {
            lease.complete(status, None, &self.settings);
        }
    }
}

impl Drop for AccountProbeCompletion {
    fn drop(&mut self) {
        // A dropped body is healthy only after the response observer has parsed a
        // successful terminal frame. Disconnects before completion and terminal
        // error/cancellation frames remain failures.
        if self.lease.is_some() {
            let successful_terminal = {
                let mut capture = self.capture.lock().unwrap();
                let terminal = capture.terminal;
                terminal && !capture.failed_body()
            };
            let status = if successful_terminal { 200 } else { 502 };
            self.complete(status);
        }
    }
}

fn observe_health(
    response: Response,
    lease: crate::resilience::Lease,
    account_lease: Option<crate::resilience::Lease>,
    settings: crate::resilience::Settings,
    capture: crate::traffic::SharedCapture,
) -> Response {
    let(parts,body)=response.into_parts();
    let account_probe = AccountProbeCompletion {
        lease: account_lease,
        settings: settings.clone(),
        capture: capture.clone(),
    };
    let stream=futures_util::stream::unfold((body.into_data_stream(),Some(lease),account_probe,settings,capture),|(mut stream,mut lease,mut account_probe,settings,capture)|async move{
        match stream.next().await{
            Some(chunk)=>{if chunk.is_err(){if let Some(l)=lease.take(){l.complete(502,None,&settings);}account_probe.complete(502);}Some((chunk,(stream,lease,account_probe,settings,capture)))}
            None=>{let status=if capture.lock().unwrap().failed_body(){502}else{200};if let Some(l)=lease.take(){l.complete(status,None,&settings);}account_probe.complete(status);None}
        }
    });Response::from_parts(parts,Body::from_stream(stream))
}

async fn model_catalog(State(context):State<ProxyContext>,headers:HeaderMap)->Response {
    let _=context.store.cpa.reconcile_owned(&context.store);
    let config=context.store.read();
    if let Some(agent)=headers.get("x-autojev-agent").and_then(|v|v.to_str().ok()) {
        // Agent 保存目录按保存内容原样返回：它是注入清单，不是准入依据，外部待同步不在此判定。
        let entries=config.agent_catalogs.get(agent).cloned().unwrap_or_default();
        return Json(json!({"object":"list","data":entries.iter().map(|e|json!({"id":e.id,"name":e.name,"object":"model","created":0,"owned_by":"autojev"})).collect::<Vec<_>>()})).into_response();
    }
    // 公共目录：只列已选、已启用、服务商启用且当前账号资格合格的模型；
    // 路由只有在至少有一个这样的候选时才出现。取消选择与资格不可用都不在这里展示。
    let bindings=config.models.iter().filter(|m|crate::subscription::callable_catalog_listed(&context.store,&config,m)).map(|m|format!("model/{}",m.id))
        .chain(config.routes.iter().filter(|r|r.enabled&&route_catalog_listed(&context.store,&config,r)).map(|r|r.id.clone())).collect::<Vec<_>>();
    let data=bindings.iter().filter_map(|b|crate::agent_catalog::build(&config,std::slice::from_ref(b)).ok()).flatten()
        .map(|e|json!({"id":e.id,"name":e.name,"object":"model","created":0,"owned_by":"autojev"})).collect::<Vec<_>>();
    Json(json!({"object":"list","data":data})).into_response()
}

/// 路由进入公共目录的条件：至少有一个已选、启用、服务商启用且资格合格的候选。
fn route_catalog_listed(store:&ConfigStore, config:&crate::config::AppConfig, rule:&crate::config::RouteRule) -> bool {
    config.models.iter().any(|model| crate::router::rule_includes_model(config, rule, model)
        && crate::subscription::callable_catalog_listed(store, config, model))
}

#[cfg(test)]
mod availability_tests {
    use super::*;
    #[tokio::test]
    async fn retry_matrix_limits_and_cooldown() {
        for (status,max,expected) in [(400,4,1),(401,4,2),(402,4,2),(403,4,2),(401,1,1),(422,4,1),(429,4,2),(500,4,2),(503,4,2),(429,1,1)] {
            let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();
            let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=calls.clone();
            let server=tokio::spawn(async move{axum::serve(listener,Router::new().route("/v1/chat/completions",post(move|Json(body):Json<Value>|{let calls=observed.clone();async move{
                calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                if body["model"]=="primary" {(StatusCode::from_u16(status).unwrap(),[("retry-after","60")],Json(json!({"error":{"message":"test failure"}}))).into_response()}
                else{Json(json!({"choices":[{"message":{"content":"ok"}}]})).into_response()}
            }}),
                    ),
                ).await.unwrap();});
            let dir=tempfile::tempdir().unwrap();let store=Arc::new(ConfigStore::load(dir.path().join("test.db")).unwrap());
            store.update(|c|{c.models.truncate(1);c.models[0].model_id="primary".into();c.providers[0].base_url=format!("http://127.0.0.1:{port}");c.gateway.max_attempts=max;
                let mut backup=c.models[0].clone();backup.id="backup".into();backup.model_id="backup".into();c.models.push(backup);
                c.routes=vec![crate::config::RouteRule{ all_models: false,id:"test".into(),name:"test".into(),enabled:true,strategy:"round_robin".into(),automatic_policy:None,model_ids:c.models.iter().map(|m|m.id.clone()).collect(),model_settings:HashMap::from([(c.models[0].id.clone(),crate::config::RouteModelSettings{priority:10,weight:1})]),
                    }];}).unwrap();
            store.write_secret("provider:openrouter","test").unwrap();let primary=store.read().models[0].id.clone();
            let context=ProxyContext{store:store.clone(),client:Client::new(),sessions:Default::default(),health:Default::default(),
            };
            let response=forward(context.clone(),HeaderMap::new(),json!({"model":"autojev/test","messages":[]}),"/v1/chat/completions","chat/completions").await;
            assert_eq!(response.status().as_u16(),if expected==2{200}else{status});
            axum::body::to_bytes(response.into_body(),4096).await.unwrap();
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),expected);
            let logs=store.request_logs("").unwrap();assert_eq!(logs[0].attempts.len(),expected);
            if expected==2{assert_eq!(logs[0].status,"success");assert!(logs[0].error.is_empty());}
            if matches!(status,401|402|403|429) && expected==2{
                assert!(!context.health.available(&primary));
                let again=forward(context.clone(),HeaderMap::new(),json!({"model":"autojev/test","messages":[]}),"/v1/chat/completions","chat/completions").await;
                assert_eq!(again.status(),StatusCode::OK);axum::body::to_bytes(again.into_body(),4096).await.unwrap();
                assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),expected+1);
                context.health.acquire("backup").unwrap().complete(429,None,&store.read().gateway);
                let unavailable=forward(context.clone(),HeaderMap::new(),json!({"model":"autojev/test","messages":[]}),"/v1/chat/completions","chat/completions").await;
                assert_eq!(unavailable.status(),StatusCode::SERVICE_UNAVAILABLE);assert!(unavailable.headers().contains_key("retry-after"));
                assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),expected+1);
            }
            server.abort();
        }
    }
    #[tokio::test]
    async fn idle_timeout_terminates_stream_without_replay(){
        let stream=futures_util::stream::pending::<Result<axum::body::Bytes,std::io::Error>>();
        let timed=timed_stream(stream,1);futures_util::pin_mut!(timed);
        assert_eq!(timed.next().await.unwrap().unwrap_err().kind(),std::io::ErrorKind::TimedOut);
        assert!(timed.next().await.is_none());
    }
    #[tokio::test]
    async fn successful_headers_do_not_mark_failed_body_healthy(){
        let h=crate::resilience::Health::default();let mut settings=crate::resilience::Settings::default();settings.failure_threshold=1;
        let lease=h.acquire("model").unwrap();let capture=crate::traffic::Capture::new("chat/completions",&json!({}),&HeaderMap::new());
        let response=Response::new(Body::from_stream(futures_util::stream::iter([Err::<axum::body::Bytes,_>(std::io::Error::other("disconnected"))])));
        let response=observe_health(response,lease,None,settings,capture);
        assert!(axum::body::to_bytes(response.into_body(),1024).await.is_err());assert!(!h.available("model"));
    }

    #[tokio::test]
    async fn dropping_a_nonstreaming_account_probe_without_eof_remains_a_failure() {
        let health = crate::resilience::Health::default();
        let settings = crate::resilience::Settings::default();
        health.acquire("subscription-account:codex:1").unwrap()
            .complete(429, Some(60), &settings);
        health.expire_for_test("subscription-account:codex:1");
        let account_lease = health.acquire_recovery_probe("subscription-account:codex:1").unwrap();
        let model_lease = health.acquire("subscription-model:codex:1:fixture").unwrap();
        let capture = crate::traffic::Capture::new("chat/completions", &json!({"stream":false}), &HeaderMap::new());
        let source = futures_util::stream::iter([Ok::<_, std::io::Error>(
            axum::body::Bytes::from_static(b"{\"choices\":[]}"),
        )]);
        let source = crate::traffic::observe(source, capture.clone());
        let response = Response::new(Body::from_stream(source));
        let response = observe_health(response, model_lease, Some(account_lease), settings, capture);

        let mut body = response.into_body().into_data_stream();
        assert!(body.next().await.unwrap().is_ok());
        drop(body);

        let account = health.statuses().into_iter()
            .find(|status| status.model_id == "subscription-account:codex:1").unwrap();
        assert_eq!(account.state, "cooldown");
        assert_eq!(account.last_status, 502, "an unread nonstreaming body has no successful terminal frame");
    }
}

#[cfg(test)]
mod pause_tests {
    use super::*;
    #[tokio::test]
    async fn pause_retains_listener_state_and_returns_not_found_with_retry_hint() {
        let task = tokio::spawn(std::future::pending::<()>());
        let handle = ProxyHandle { port: 0, shutdown: None, health: Default::default(), task, paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        handle.set_paused(true);
        assert!(handle.running());
        assert!(handle.paused());
        let response = paused_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["x-should-retry"], "false");
        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "gateway_paused");
        assert_eq!(body["error"]["type"], "not_found_error");
        handle.set_paused(false);
        assert!(!handle.paused());
        assert!(handle.running());
        handle.task.abort();
    }
}
