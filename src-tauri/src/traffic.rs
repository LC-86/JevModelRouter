//! Local request metadata. Bodies and credentials are never persisted.
use crate::{config::ConfigStore, protocol::Protocol};
use axum::{
    body::{Body, Bytes},
    response::Response,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

pub(crate) const SUBSCRIPTION_PROBE_RESPONSE_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestLog {
    pub performance_model_id: String,
    pub performance_fingerprint: String,
    pub performance_context_tokens: u64,
    pub performance_requires_vision: bool,
    pub first_content_ms: Option<u64>,
    pub upstream_duration_ms: u64,
    pub attempts: Vec<Attempt>,
    pub id: String,
    pub created_at: String,
    pub agent: String,
    pub endpoint: String,
    pub requested_model: String,
    pub route_id: String,
    pub route_strategy: String,
    pub route_preference: String,
    pub provider_name: String,
    pub model_name: String,
    pub model_id: String,
    pub source: String,
    pub reason: String,
    pub streaming: bool,
    pub status: String,
    pub status_code: u16,
    pub duration_ms: u64,
    pub first_byte_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub estimated_cost: Option<f64>,
    pub error: String,
}

// Resolve only a direct model binding. A rejected route has no selected upstream
// model, and historical metadata must never be overwritten with today's names.
pub fn resolve_requested_model(log: &mut RequestLog, config: &crate::config::AppConfig, requested: &str) {
    if !log.model_name.is_empty() || !log.provider_name.is_empty() || !log.performance_model_id.is_empty() { return; }
    let Some(id)=requested.strip_prefix("autojev/model/") else { return; };
    let Some(model)=config.models.iter().find(|m| m.id==id) else { return; };
    log.model_name=if model.name.is_empty() {model.model_id.clone()} else {model.name.clone()};
    if let Some(provider)=config.providers.iter().find(|p|p.id==model.provider_id) { log.provider_name=provider.name.clone(); }
    // Do not assign upstream IDs, pricing or performance attribution: this request
    // may have been rejected before contacting any provider.
}

pub struct Capture {
    pub log: RequestLog,
    performance_probe: bool,
    subscription_probe_over_budget: bool,
    subscription_probe_bytes: Option<usize>,
    start: Instant,
    upstream_start: Instant,
    protocol: Protocol,
    prices: Option<(f64, f64, f64)>,
    buffer: Vec<u8>,
    sse: bool,
    frame_data: Vec<u8>,
    discard_line: bool,
    pub terminal: bool,
    expected_choices: u64,
    finished_choices: std::collections::HashSet<u64>,
    pub usage_complete: bool,
}
pub type SharedCapture = Arc<Mutex<Capture>>;
// Local placeholders identify injected clients; they are not authentication credentials.
fn injected_agent(headers: &axum::http::HeaderMap) -> Option<&str> {
    let bearer = headers.get("authorization").and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token);
    [bearer, headers.get("x-api-key").and_then(|v| v.to_str().ok()),
        headers.get("x-goog-api-key").and_then(|v| v.to_str().ok())]
        .into_iter().flatten().find_map(|token| token.strip_prefix("autojev-local-")
            .filter(|id| !id.is_empty() && id.len() <= 80 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')))
}

impl Capture {
    pub fn new(endpoint: &str, body: &Value, headers: &axum::http::HeaderMap) -> SharedCapture {
        let ua = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let performance_probe = headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("AutoJev/ModelSpeedTest"));
        let agent = headers
            .get("x-autojev-agent")
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .or_else(|| injected_agent(headers))
            .map(|s| s.chars().take(80).collect())
            .unwrap_or_else(|| {
                [
                    ("autojev/", "AutoJev"),
                    ("codex", "Codex"),
                    ("claude", "Claude Code"),
                    ("gemini", "Gemini CLI"),
                    ("opencode", "OpenCode"),
                    ("openclaw", "OpenClaw"),
                    ("kimi", "Kimi"),
                    ("hermes", "Hermes"),
                    ("fastclaw", "FastClaw"),
                    ("oh-my-pi", "OMP"),
                    ("omp/", "OMP"),
                    ("grok", "Grok Build"),
                    ("cline", "Cline"),
                    ("aider", "Aider"),
                ]
                .into_iter()
                .find(|(key, _)| ua.contains(key))
                .map(|(_, v)| v)
                .unwrap_or("Unknown agent")
                .into()
            });
        Arc::new(Mutex::new(Self {
            log: RequestLog {
                id: uuid::Uuid::new_v4().to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                endpoint: endpoint.into(),
                requested_model: body["model"].as_str().unwrap_or("").into(),
                agent,
                streaming: body["stream"].as_bool().unwrap_or(false),
                status: "pending".into(),
                ..Default::default()
            },
            performance_probe,
            subscription_probe_over_budget: false,
            // The request's model string is user-controlled for API providers and
            // cannot establish that this probe belongs to a subscription provider.
            subscription_probe_bytes: None,
            frame_data: vec![],
            start: Instant::now(),
            upstream_start: Instant::now(),
            protocol: Protocol::Chat,
            prices: None,
            buffer: vec![],
            sse: false,
            discard_line: false,
            terminal: false,
            expected_choices: body["n"].as_u64().unwrap_or(1).max(1),
            finished_choices: Default::default(),
            usage_complete: false,
        }))
    }
    pub fn failed_body(&mut self)->bool {self.end_body();!self.log.error.is_empty() || (self.sse && !self.terminal)}
    pub fn routing_rule(&mut self, config: &crate::config::AppConfig, requested: &str) {
        resolve_requested_model(&mut self.log, config, requested);
        self.log.route_id.clear();
        self.log.route_strategy.clear();
        self.log.route_preference.clear();
        let Some(id) = requested.strip_prefix("autojev/").filter(|id| !id.starts_with("model/") && !id.is_empty()) else { return; };
        self.log.route_id = id.into();
        if let Some(rule) = config.routes.iter().find(|rule| rule.id == id) {
            self.log.route_strategy = rule.strategy.clone();
            if rule.strategy == "jev" {
                let policy = rule.automatic_policy.as_ref().unwrap_or(&config.policy);
                self.log.route_preference = if policy.decision_preference.is_empty() { "balanced".into() } else { policy.decision_preference.clone() };
            }
        } else if id == "auto" {
            self.log.route_strategy = "auto".into();
        }
    }
    pub fn measure(&mut self, model: &crate::config::Model, provider: &crate::config::Provider, context: u64) {
        self.upstream_start = Instant::now();
        self.log.first_content_ms = None;
        self.log.first_byte_ms = None;
        self.log.performance_model_id = model.id.clone();
        self.log.performance_fingerprint = crate::performance::fingerprint(model,provider);
        self.log.performance_context_tokens = context;
        self.subscription_probe_bytes = (self.performance_probe
            && crate::subscription::is_subscription_provider(provider))
            .then_some(0);
        self.subscription_probe_over_budget = false;
    }
    pub fn route(&mut self, route: &crate::router::ResolvedRoute) {
        self.measure(&route.model,&route.provider,0);
        self.log.error.clear();self.buffer.clear();self.frame_data.clear();self.discard_line=false;self.terminal=false;self.finished_choices.clear();self.usage_complete=false;
        self.log.input_tokens=None;self.log.output_tokens=None;self.log.cache_read_tokens=0;self.log.cache_write_tokens=0;
        self.log.provider_name = route.provider.name.clone();
        self.log.model_name = route.model.name.clone();
        self.log.model_id = route.model.model_id.clone();
        self.log.source = route.decision.source.clone();
        self.log.reason = route.decision.reason.clone();
        self.prices = crate::cost::known(&route.model).then_some((
            route.model.input_cost_per_million,
            route.model.output_cost_per_million,
            if crate::cost::cache_known(&route.model) { route.model.cache_cost_per_million } else { f64::NAN },
        ));
    }
    pub fn upstream(&mut self, protocol: Protocol, sse: bool) {
        self.protocol = protocol;
        self.sse = sse;
    }
    fn value(&mut self, value: &Value) {
        let kind = value["type"].as_str().unwrap_or("");
        let has_text = |v: Option<&Value>| v.and_then(Value::as_str).is_some_and(|s| !s.is_empty());
        let content = if self.sse {
            has_text(value.pointer("/choices/0/delta/content"))
                || value.pointer("/choices/0/delta/tool_calls").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
                || (kind == "content_block_delta" && (has_text(value.pointer("/delta/text")) || has_text(value.pointer("/delta/partial_json"))))
                || (matches!(kind,"response.output_text.delta" | "response.function_call_arguments.delta") && has_text(value.get("delta")))
        } else { false };
        if content && self.log.first_content_ms.is_none() {
            self.log.first_content_ms = Some(self.upstream_start.elapsed().as_millis() as u64);
        }
        if value.get("error").is_some_and(|e| !e.is_null())
            || matches!(kind, "error" | "response.failed")
        {
            // Provider error bodies can echo prompts or secrets. Keep only a safe category.
            self.log.error = "Upstream returned an API error".into();
        }
        if value.pointer("/response/status").and_then(Value::as_str) == Some("failed") {
            self.log.error = "Upstream response failed".into();
        }
        if matches!(
            kind,
            "message_stop" | "response.completed" | "response.incomplete" | "response.failed"
        ) {
            self.terminal = true;
        }
        if self.sse && self.protocol == Protocol::Chat {
            if let Some(choices) = value["choices"].as_array() {
                for choice in choices {
                    if choice["finish_reason"].as_str().is_some_and(|reason| !reason.is_empty()) {
                        let index = choice["index"].as_u64().unwrap_or(0);
                        if index < self.expected_choices { self.finished_choices.insert(index); }
                    }
                }
                if self.finished_choices.len() as u64 == self.expected_choices { self.terminal = true; }
            }
        }
        let usage = value
            .get("usage")
            .or_else(|| value.pointer("/message/usage"))
            .or_else(|| value.pointer("/response/usage"));
        let Some(u) = usage.filter(|v| v.is_object()) else {
            return;
        };
        let input = u
            .get("prompt_tokens")
            .or_else(|| u.get("input_tokens"))
            .and_then(Value::as_u64);
        let output = u
            .get("completion_tokens")
            .or_else(|| u.get("output_tokens"))
            .and_then(Value::as_u64);
        if let Some(n) = u
            .pointer("/prompt_tokens_details/cached_tokens")
            .or_else(|| u.pointer("/input_tokens_details/cached_tokens"))
            .or_else(|| u.get("cache_read_input_tokens"))
            .and_then(Value::as_u64)
        {
            self.log.cache_read_tokens = n;
        }
        if let Some(n) = u.get("cache_creation_input_tokens").and_then(Value::as_u64) {
            self.log.cache_write_tokens = n;
        }
        if let Some(n) = input {
            self.log.input_tokens = Some(if self.protocol == Protocol::Messages {
                n.saturating_add(self.log.cache_read_tokens)
                    .saturating_add(self.log.cache_write_tokens)
            } else {
                n
            });
        }
        if let Some(n) = output {
            self.log.output_tokens = Some(n);
        }
        // message_start includes output=0; it is not the final usage report.
        if !self.sse
            || (self.protocol == Protocol::Chat && input.is_some() && output.is_some())
            || kind == "message_delta"
            || matches!(kind, "response.completed" | "response.incomplete")
        {
            self.usage_complete = true;
        }
    }
    pub fn bytes(&mut self, bytes: &[u8]) {
        if let Some(seen) = &mut self.subscription_probe_bytes {
            *seen = (*seen).saturating_add(bytes.len());
            if *seen > SUBSCRIPTION_PROBE_RESPONSE_LIMIT_BYTES {
                self.subscription_probe_over_budget = true;
            }
        }
        if self.log.first_byte_ms.is_none() && !bytes.is_empty() {
            self.log.first_byte_ms = Some(self.start.elapsed().as_millis() as u64);
        }
        if !self.sse {
            if self.buffer.len() + bytes.len() <= 16 * 1024 * 1024 && !self.discard_line {
                self.buffer.extend_from_slice(bytes);
            } else {
                self.buffer.clear();
                self.discard_line = true;
            }
            return;
        }
        // Byte-oriented line assembly handles UTF-8 and CRLF split across arbitrary chunks.
        for &byte in bytes {
            if byte == b'\n' {
                if !self.discard_line {
                    let line = std::mem::take(&mut self.buffer);
                    let line = line.strip_suffix(b"\r").unwrap_or(&line);
                    if let Some(data) = line.strip_prefix(b"data:") {
                        let data = data.strip_prefix(b" ").unwrap_or(data);
                        if self.frame_data.len() + data.len() < 2 * 1024 * 1024 {
                            if !self.frame_data.is_empty() {
                                self.frame_data.push(b'\n');
                            }
                            self.frame_data.extend_from_slice(data);
                        }
                    } else if line.is_empty() {
                        let data = std::mem::take(&mut self.frame_data);
                        if data == b"[DONE]" {
                            self.terminal = true;
                        } else if let Ok(value) = serde_json::from_slice::<Value>(&data) {
                            self.value(&value);
                        }
                    }
                }
                self.buffer.clear();
                self.discard_line = false;
            } else if !self.discard_line {
                if self.buffer.len() < 1024 * 1024 {
                    self.buffer.push(byte);
                } else {
                    self.buffer.clear();
                    self.discard_line = true;
                }
            }
        }
    }
    pub fn end_body(&mut self) {
        if !self.sse {
            if let Ok(value) = serde_json::from_slice::<Value>(&self.buffer) {
                self.value(&value);
            }
            self.buffer.clear();
        }
    }
    pub(crate) fn finish(&mut self, completed: bool) -> RequestLog {
        self.end_body();
        self.log.duration_ms = self.start.elapsed().as_millis() as u64;
        self.log.upstream_duration_ms = self.upstream_start.elapsed().as_millis() as u64;
        if self.subscription_probe_over_budget {
            self.log.error = "Subscription speed test response exceeded the 64 KiB local output limit".into();
        }
        let ok = (200..300).contains(&self.log.status_code);
        if self.log.error.is_empty() {
            if !ok {
                self.log.error = format!("Request failed (HTTP {})", self.log.status_code);
            } else if !completed && !(self.sse && self.terminal) {
                self.log.error = "Client disconnected before completion".into();
            } else if self.sse && !self.terminal {
                self.log.error = "Upstream stream ended before completion".into();
            }
        }
        self.log.status = if self.log.error.is_empty() {
            "success"
        } else if !completed && ok && self.log.error == "Client disconnected before completion" {
            "cancelled"
        } else {
            "error"
        }
        .into();
        if self.usage_complete
            && self.log.input_tokens.is_some()
            && self.log.output_tokens.is_some()
        {
            if let (Some(input), Some(output), Some((ip, op, cp))) =
                (self.log.input_tokens, self.log.output_tokens, self.prices)
            {
                // Cache writes have no configured rate: omit cost instead of inventing a price.
                if self.log.cache_write_tokens == 0 && (self.log.cache_read_tokens == 0 || cp.is_finite()) {
                    let cached = self.log.cache_read_tokens.min(input);
                    self.log.estimated_cost = Some(
                        (input.saturating_sub(cached) as f64 * ip
                            + if cached > 0 { cached as f64 * cp } else { 0.0 }
                            + output as f64 * op)
                            / 1_000_000.0,
                    );
                }
            }
        } else {
            self.log.input_tokens = None;
            self.log.output_tokens = None;
            self.log.cache_read_tokens = 0;
            self.log.cache_write_tokens = 0;
        }
        self.log.clone()
    }
}

pub fn observe<S, E>(
    stream: S,
    capture: SharedCapture,
) -> impl Stream<Item = Result<Bytes, E>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send,
    E: Send,
{
    stream.map(move |chunk| {
        let mut c = capture.lock().unwrap();
        match &chunk {
            Ok(bytes) => c.bytes(bytes),
            Err(_) => c.log.error = "Upstream connection interrupted".into(),
        }
        chunk
    })
}

struct CompletionGuard {
    store: Arc<ConfigStore>,
    capture: SharedCapture,
    completed: bool,
}
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        let (log, probe) = {
            let mut capture = self.capture.lock().unwrap();
            let probe = capture.performance_probe;
            (capture.finish(self.completed), probe)
        };
        if let Err(error) = crate::performance::record_log(&self.store,&log,probe) {
            eprintln!("Could not persist performance sample: {error}");
        }
        if let Err(error) = self.store.save_request_log(&log) {
            eprintln!("Could not persist request log: {error}");
        }
    }
}
pub fn response(response: Response, capture: SharedCapture, store: Arc<ConfigStore>) -> Response {
    capture.lock().unwrap().log.status_code = response.status().as_u16();
    let (mut parts, body) = response.into_parts();
    if let Ok(id) = capture.lock().unwrap().log.id.parse() {
        parts.headers.insert("x-autojev-request-id", id);
    }
    let guard = CompletionGuard {
        store,
        capture,
        completed: false,
    };
    let stream = futures_util::stream::unfold(
        (body.into_data_stream(), guard),
        |(mut stream, mut guard)| async move {
            match stream.next().await {
                Some(chunk) => {
                    if chunk.is_err() {
                        guard.capture.lock().unwrap().log.error = "Response transfer failed".into();
                    }
                    Some((chunk, (stream, guard)))
                }
                None => {
                    guard.completed = true;
                    drop(guard);
                    None
                }
            }
        },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejected_direct_model_has_display_metadata_without_upstream_attribution() {
        let config=crate::config::AppConfig::default();
        let model=&config.models[0];
        let requested=format!("autojev/model/{}",model.id);
        let mut log=RequestLog {requested_model:requested.clone(),status:"error".into(),status_code:422,..Default::default()};
        resolve_requested_model(&mut log,&config,&requested);
        assert_eq!(log.model_name,model.name);
        assert!(!log.provider_name.is_empty());
        assert!(log.model_id.is_empty());
        assert!(log.performance_model_id.is_empty());
        assert!(log.estimated_cost.is_none());
        assert_eq!(log.requested_model,requested);
        log.model_name="Historical name".into();
        resolve_requested_model(&mut log,&config,&requested);
        assert_eq!(log.model_name,"Historical name");
        for requested in ["autojev/fast","autojev/model/deleted"] {
            let mut unknown=RequestLog::default();
            resolve_requested_model(&mut unknown,&config,requested);
            assert!(unknown.model_name.is_empty());
        }
    }

    #[test]
    fn captures_route_strategy_at_request_time() {
        let mut config = crate::config::AppConfig::default();
        let mut policy = config.policy.clone();
        policy.decision_preference = "speed".into();
        config.routes.push(crate::config::RouteRule { all_models: false,
            id: "fast".into(), name: "Fast".into(), strategy: "jev".into(), enabled: true,
            model_ids: vec![], model_settings: Default::default(), automatic_policy: Some(policy),
        });
        let c = capture(Protocol::Chat, false);
        let mut c = c.lock().unwrap();
        c.routing_rule(&config, "autojev/fast");
        assert_eq!(c.log.route_id, "fast");
        assert_eq!(c.log.route_strategy, "jev");
        assert_eq!(c.log.route_preference, "speed");
        config.routes[0].strategy = "round_robin".into();
        assert_eq!(c.log.route_strategy, "jev");
        c.routing_rule(&config, "autojev/fast");
        assert_eq!(c.log.route_strategy, "round_robin");
        assert!(c.log.route_preference.is_empty());
        c.routing_rule(&config, "autojev/model/direct");
        assert!(c.log.route_id.is_empty());
        assert!(c.log.route_strategy.is_empty());
    }
    #[test]
    fn injected_identity_supports_all_transports_and_prefers_explicit_headers() {
        for agent in ["codex", "claude", "gemini", "grok", "kimi", "openclaw", "fastclaw", "hermes", "opencode", "omp", "custom-test"] {
            for name in ["authorization", "x-api-key", "x-goog-api-key"] {
                let mut headers = axum::http::HeaderMap::new();
                let marker = format!("autojev-local-{agent}");
                headers.insert(name, (if name == "authorization" { format!("Bearer {marker}") } else { marker }).parse().unwrap());
                headers.insert("user-agent", "generic-sdk".parse().unwrap());
                let capture = Capture::new("chat/completions", &json!({}), &headers);
                assert_eq!(capture.lock().unwrap().log.agent, agent);
                headers.insert("x-autojev-agent", "explicit-client".parse().unwrap());
                let capture = Capture::new("chat/completions", &json!({}), &headers);
                assert_eq!(capture.lock().unwrap().log.agent, "explicit-client");
            }
        }
        for token in ["autojev-local", "autojev-local-", "unrelated-key", "autojev-local-bad/value"] {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert("x-api-key", token.parse().unwrap());
            assert!(injected_agent(&headers).is_none());
        }
    }
    #[test]
    fn kimi_local_marker_identifies_both_protocols_without_custom_headers() {
        for (name, value) in [("authorization", "Bearer autojev-local-kimi"), ("x-api-key", "autojev-local-kimi")] {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(name, value.parse().unwrap());
            let capture = Capture::new("chat/completions", &json!({}), &headers);
            assert_eq!(capture.lock().unwrap().log.agent, "kimi");
            headers.insert(name, "unrelated-key".parse().unwrap());
            let capture = Capture::new("chat/completions", &json!({}), &headers);
            assert_eq!(capture.lock().unwrap().log.agent, "Unknown agent");
        }
    }
    #[test]
    fn injected_omp_header_identifies_requests_with_generic_sdk_user_agent() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("user-agent", "OpenAI/JS".parse().unwrap());
        headers.insert("x-autojev-agent", "omp".parse().unwrap());
        for endpoint in ["chat/completions", "messages"] {
            let capture = Capture::new(endpoint, &json!({"model":"autojev/fast"}), &headers);
            assert_eq!(capture.lock().unwrap().log.agent, "omp");
        }
    }
    #[test]
    fn debug_agent_and_nonstreaming_first_byte_are_recorded() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("user-agent", "AutoJev/Debug".parse().unwrap());
        let capture = Capture::new("chat/completions", &json!({"model":"autojev/fast"}), &headers);
        let mut c = capture.lock().unwrap();
        assert_eq!(c.log.agent, "AutoJev");
        for ua in ["AutoJev/ProviderTest", "AutoJev/ModelTest"] {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert("user-agent", ua.parse().unwrap());
            let capture = Capture::new("chat/completions", &serde_json::json!({}), &headers);
            assert_eq!(capture.lock().unwrap().log.agent, "AutoJev");
        }

        assert!(!c.log.streaming);
        assert!(c.log.first_byte_ms.is_none());
        c.bytes(b"{}");
        assert!(c.log.first_byte_ms.is_some());
    }
    #[tokio::test]
    async fn response_id_matches_the_persisted_request() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(directory.path().join("debug.db")).unwrap());
        let c = capture(Protocol::Chat, false);
        let id = c.lock().unwrap().log.id.clone();
        let reply = response(Response::new(Body::from("{}")), c, store.clone());
        assert_eq!(reply.headers()["x-autojev-request-id"], id);
        axum::body::to_bytes(reply.into_body(), 1024).await.unwrap();
        assert_eq!(store.request_log(&id).unwrap().unwrap().id, id);
    }
    fn capture(protocol: Protocol, sse: bool) -> SharedCapture {
        let capture = Capture::new(
            "responses",
            &json!({"model":"auto", "stream":sse}),
            &Default::default(),
        );
        {
            let mut c = capture.lock().unwrap();
            c.upstream(protocol, sse);
            c.log.status_code = 200;
            c.prices = Some((2., 6., 0.5));
        }
        capture
    }
    #[test]
    fn reads_json_usage_without_double_counting_cache() {
        let c = capture(Protocol::Responses, false);
        let mut c = c.lock().unwrap();
        c.bytes(br#"{"usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":40}}}"#);
        let log = c.finish(true);
        assert_eq!(log.input_tokens, Some(100));
        assert_eq!(log.output_tokens, Some(20));
        assert_eq!(log.status, "success");
        assert!((log.estimated_cost.unwrap() - 0.00026).abs() < 1e-10);
    }
    #[test]
    fn fragmented_multiline_sse_preserves_anthropic_cache_and_partial_updates() {
        let c = capture(Protocol::Messages, true);
        let mut c = c.lock().unwrap();
        let wire = concat!("data: {\"type\":\"message_start\",\r\n", "data: \"message\":{\"usage\":{\"input_tokens\":100,\"cache_read_input_tokens\":40,\"cache_creation_input_tokens\":10,\"output_tokens\":0}}}\r\n\r\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":20}}\n\n", "data: {\"type\":\"message_stop\"}\n\n");
        for chunk in wire.as_bytes().chunks(3) {
            c.bytes(chunk);
        }
        let log = c.finish(true);
        assert_eq!(log.input_tokens, Some(150));
        assert_eq!(log.output_tokens, Some(20));
        assert_eq!(log.cache_read_tokens, 40);
        assert_eq!(log.cache_write_tokens, 10);
        assert_eq!(log.estimated_cost, None);
        assert_eq!(log.status, "success");
    }
    #[test]
    fn repeated_cumulative_usage_is_not_added_and_missing_usage_is_unknown() {
        let c = capture(Protocol::Chat, true);
        let mut c = c.lock().unwrap();
        let wire = b"data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n";
        c.bytes(wire);
        c.bytes(wire);
        c.bytes(b"data: [DONE]\n\n");
        let log = c.finish(true);
        assert_eq!(log.input_tokens, Some(10));
        assert_eq!(log.output_tokens, Some(5));
        let c = capture(Protocol::Chat, false);
        let log = c.lock().unwrap().finish(true);
        assert_eq!(log.input_tokens, None);
        assert_eq!(log.estimated_cost, None);
    }
    #[test]
    fn protocol_completion_survives_client_close_before_http_eof() {
        for (protocol, wire) in [
            (Protocol::Chat, "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n"),
            (Protocol::Chat, "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"tool_calls\"}]}\n\n"),
            (Protocol::Chat, "data: [DONE]\n\n"),
            (Protocol::Messages, "data: {\"type\":\"message_stop\"}\n\n"),
            (Protocol::Responses, "data: {\"type\":\"response.completed\"}\n\n"),
        ] {
            let c=capture(protocol,true); let mut c=c.lock().unwrap();
            for chunk in wire.as_bytes().chunks(3) { c.bytes(chunk); }
            assert_eq!(c.finish(false).status,"success");
        }
        let c=capture(Protocol::Chat,true);let mut c=c.lock().unwrap();
        c.bytes(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n");
        assert_eq!(c.finish(false).status,"cancelled");
        c.bytes(b"data: [DONE]\n\n");
        c.log.error="Response transfer failed".into();
        assert_eq!(c.finish(false).status,"error");
    }

    #[test]
    fn all_requested_choices_must_finish() {
        let c=capture(Protocol::Chat,true);let mut c=c.lock().unwrap();
        c.expected_choices=2;
        c.bytes(b"data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n");
        assert!(!c.terminal);
        c.bytes(b"data: {\"choices\":[{\"index\":1,\"finish_reason\":\"stop\"}]}\n\n");
        assert_eq!(c.finish(false).status,"success");
    }

    #[test]
    fn interrupted_stream_does_not_treat_start_usage_as_complete() {
        let c = capture(Protocol::Messages, true);
        let mut c = c.lock().unwrap();
        c.bytes(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n");
        let log = c.finish(true);
        assert_eq!(log.status, "error");
        assert_eq!(log.input_tokens, None);
        assert_eq!(log.estimated_cost, None);
    }
    #[tokio::test]
    async fn persists_completion_and_cancellation_once_without_bodies() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("traffic.db");
        let store = Arc::new(ConfigStore::load(path.clone()).unwrap());
        let c = capture(Protocol::Chat, false);
        let reply = response(
            Response::new(Body::from("private response text")),
            c,
            store.clone(),
        );
        axum::body::to_bytes(reply.into_body(), 1024).await.unwrap();
        let cancelled = response(
            Response::new(Body::from("unused")),
            capture(Protocol::Chat, true),
            store.clone(),
        );
        drop(cancelled);
        let records = ConfigStore::load(path).unwrap().request_logs("").unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.status == "cancelled"));
        assert!(records.iter().any(|r| r.status == "success"));
        assert!(!serde_json::to_string(&records)
            .unwrap()
            .contains("private response text"));
        assert!(store.request_logs("9999").unwrap().is_empty());
    }

    #[tokio::test]
    async fn oversized_manual_probe_persists_failure_for_both_sample_and_request_log() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(directory.path().join("oversized-probe.db")).unwrap());
        let config = store.read();
        let model = config.models.first().expect("default test model");
        let mut provider = config.providers.iter().find(|item| item.id == model.provider_id).expect("default test provider").clone();
        provider.kind = crate::config::ProviderKind::CodexSubscription;
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("user-agent", "AutoJev/ModelSpeedTest".parse().unwrap());
        let capture = Capture::new(
            "chat/completions",
            &json!({"model":"autojev/model/test", "stream":true}),
            &headers,
        );
        {
            let mut capture = capture.lock().unwrap();
            capture.measure(model, &provider, 32);
            capture.upstream(Protocol::Chat, true);
            capture.log.status_code = 200;
        }
        let oversized_content = "x".repeat(70 * 1024);
        let content_event = json!({"choices":[{"index":0,"delta":{"content":oversized_content},"finish_reason":null}]});
        let terminal_event = json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
        let wire = format!("data: {content_event}\n\ndata: {terminal_event}\n\ndata: [DONE]\n\n");
        let source = futures_util::stream::iter(vec![Ok::<Bytes, std::convert::Infallible>(Bytes::from(wire))]);
        let body = Body::from_stream(observe(source, capture.clone()));
        let reply = response(Response::new(body), capture, store.clone());
        axum::body::to_bytes(reply.into_body(), 128 * 1024).await.unwrap();

        let logs = store.request_logs("").unwrap();
        let log = logs.first().expect("persisted request log");
        assert_eq!(log.status, "error");
        assert!(log.error.contains("64 KiB"), "unexpected error: {}", log.error);
        let config = store.read();
        let sample = config.performance_samples[&model.id].last().expect("persisted speed sample");
        assert!(sample.probe);
        assert!(!sample.success, "an over-budget response must not be recorded as a success");
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {pub provider:String,pub model:String,pub status:u16,pub duration_ms:u64}

#[cfg(test)]
mod performance_capture_tests {
    use super::*;
    #[test]
    fn first_content_ignores_headers_roles_and_usage_and_resets_on_retry() {
        let config=crate::config::AppConfig::default();
        let capture=Capture::new("chat/completions",&serde_json::json!({"stream":true}),&Default::default());
        let mut c=capture.lock().unwrap();c.measure(&config.models[0],&config.providers[0],100);
        c.upstream(Protocol::Chat,true);
        c.bytes(b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n");
        assert!(c.log.first_byte_ms.is_some());assert!(c.log.first_content_ms.is_none());
        c.bytes(b"data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":0}}\n\n");
        assert!(c.log.first_content_ms.is_none());
        for byte in b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n" {c.bytes(&[*byte]);}
        assert!(c.log.first_content_ms.is_some());
        c.measure(&config.models[1],&config.providers[0],100);
        assert!(c.log.first_content_ms.is_none());assert!(c.log.first_byte_ms.is_none());
    }

    #[test]
    fn api_speed_probe_with_subscription_style_model_id_keeps_api_response_limit() {
        let config = crate::config::AppConfig::default();
        let mut model = config.models[0].clone();
        model.model_id = "autojev/model/chained-fixture".into();
        let provider = &config.providers[0];
        assert!(!crate::subscription::is_subscription_provider(provider));

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("user-agent", "AutoJev/ModelSpeedTest".parse().unwrap());
        let capture = Capture::new(
            "chat/completions",
            &serde_json::json!({"model":model.model_id, "stream":true}),
            &headers,
        );
        let event = serde_json::json!({"choices":[{"index":0,"delta":{"content":"OK"},"finish_reason":null}]});
        let terminal = serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":32,"completion_tokens":2}});
        let wire = format!(
            ": {}\n\ndata: {event}\n\ndata: {terminal}\n\ndata: [DONE]\n\n",
            "p".repeat(70 * 1024),
        );
        let mut capture = capture.lock().unwrap();
        capture.measure(&model, provider, 32);
        capture.upstream(Protocol::Chat, true);
        capture.log.status_code = 200;
        capture.bytes(wire.as_bytes());

        let log = capture.finish(true);
        assert_eq!(log.status, "success", "API response size must not inherit the subscription limit: {}", log.error);
        assert_eq!(log.output_tokens, Some(2));
    }
}
