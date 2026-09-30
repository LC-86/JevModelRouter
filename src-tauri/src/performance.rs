//! Bounded, local performance samples; no request bodies or credentials are retained.
use crate::{
    config::{AppConfig, ConfigStore, Model, Provider, ProviderKind},
    protocol::Protocol,
    traffic::{Capture, RequestLog},
};
use anyhow::{ensure, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub const FRESH_MS: i64 = 30 * 60 * 1000;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(default)]
    pub version: u8,
    pub enabled: bool,
    pub interval_minutes: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            interval_minutes: 30,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub at: i64,
    pub fingerprint: String,
    pub success: bool,
    pub first_content_ms: Option<u64>,
    pub duration_ms: u64,
    pub output_tokens: Option<u64>,
    pub context_tokens: u64,
    pub probe: bool,
    #[serde(default)]
    pub requires_vision: bool,
}
#[derive(Clone, Debug, Serialize, Default)]
pub struct Summary {
    pub first_content_ms: Option<f64>,
    pub duration_ms: Option<f64>,
    pub tokens_per_second: Option<f64>,
    pub success_rate: f64,
    pub samples: usize,
    pub last_test_at: Option<i64>,
    pub stale: bool,
}
#[derive(Clone, Serialize, Default)]
pub struct Job {
    pub running: bool,
    pub completed: usize,
    pub completed_models: usize,
    pub total_models: usize,
    pub total: usize,
    pub current_models: Vec<String>,
    pub error: Option<String>,
}
#[derive(Default)]
pub struct Runner {
    busy: AtomicBool,
    cancel: AtomicBool,
    wake: tokio::sync::Notify,
    job: Mutex<Job>,
}
#[derive(Serialize)]
pub struct View {
    pub settings: Settings,
    pub models: HashMap<String, Summary>,
    pub job: Job,
}

pub fn fingerprint(model: &Model, provider: &Provider) -> String {
    // Endpoint/protocol edits invalidate prior measurements, even if the internal ID is unchanged.
    json!([
        model.provider_id,
        model.model_id,
        provider.base_url,
        provider.api_type,
        model.api_type
    ])
    .to_string()
}
fn band(tokens: u64) -> u8 {
    if tokens < 4096 {
        0
    } else if tokens < 32768 {
        1
    } else {
        2
    }
}
fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    Some((values[(n - 1) / 2] + values[n / 2]) / 2.0)
}
fn throughput(sample: &Sample) -> Option<f64> {
    let output = sample.output_tokens?;
    let elapsed = sample.duration_ms.checked_sub(sample.first_content_ms?)?;
    (output > 1 && elapsed > 0).then(|| (output - 1) as f64 * 1000.0 / elapsed as f64)
}
pub fn summary(
    config: &AppConfig,
    model: &Model,
    provider: &Provider,
    context: Option<u64>,
    now: i64,
) -> Summary {
    summary_for(config, model, provider, context, None, now)
}
fn summary_for(config: &AppConfig, model: &Model, provider: &Provider, context: Option<u64>, vision: Option<bool>, now: i64) -> Summary {
    let signature = fingerprint(model, provider);
    let all: Vec<_> = config
        .performance_samples
        .get(&model.id)
        .into_iter()
        .flatten()
        .filter(|s| s.fingerprint == signature && vision.is_none_or(|v| s.requires_vision == v))
        .collect();
    let last_test_at = all.iter().map(|s| s.at).max();
    let fresh: Vec<_> = all
        .iter()
        .copied()
        .filter(|s| now - s.at >= 0 && now - s.at <= FRESH_MS)
        .collect();
    let matching: Vec<_> = fresh
        .iter()
        .copied()
        .filter(|s| context.is_none_or(|n| !s.probe && band(n) == band(s.context_tokens)))
        .collect();
    // Synthetic probes provide a cold-start estimate; unrelated real workloads are not mixed in.
    let source = if context.is_some() && matching.is_empty() {
        fresh
            .iter()
            .copied()
            .filter(|s| s.probe)
            .collect::<Vec<_>>()
    } else {
        matching
    };
    let recent: Vec<_> = source.into_iter().rev().take(5).collect();
    let successful: Vec<_> = recent.iter().copied().filter(|s| s.success).collect();
    Summary {
        duration_ms: median(successful.iter().map(|s| s.duration_ms as f64).collect()),
        first_content_ms: median(
            successful
                .iter()
                .filter_map(|s| s.first_content_ms.map(|n| n as f64))
                .collect(),
        ),
        tokens_per_second: median(successful.iter().filter_map(|s| throughput(s)).collect()),
        success_rate: if recent.is_empty() {
            0.0
        } else {
            successful.len() as f64 / recent.len() as f64
        },
        samples: recent.len(),
        last_test_at,
        stale: fresh.is_empty(),
    }
}
pub fn request_summary(config: &AppConfig, model: &Model, provider: &Provider, input: &crate::router::RoutePreviewInput) -> Summary {
    summary_for(config, model, provider, Some(input.estimated_context_tokens), Some(input.requires_vision), chrono::Utc::now().timestamp_millis())
}

pub fn estimated_duration(summary: &Summary, context: u64) -> Option<f64> {
    match (summary.first_content_ms, summary.tokens_per_second) {
        (Some(first), Some(rate)) if rate > 0.0 => Some(first + (context as f64 * 0.18).clamp(16.0, 4096.0) / rate * 1000.0),
        _ => summary.duration_ms.or(summary.first_content_ms),
    }
}

pub fn fastest(config: &AppConfig, eligible: &[(Model, Provider)], context: u64, requires_vision: bool) -> Option<String> {
    let now = chrono::Utc::now().timestamp_millis();
    let measured: Vec<_> = eligible
        .iter()
        .map(|(m, p)| (m, summary_for(config, m, p, Some(context), Some(requires_vision), now)))
        .filter(|(_, s)| s.first_content_ms.is_some() && s.success_rate >= 0.5)
        .collect();
    // Compare the same metric for all candidates. Without complete token usage, use first content latency.
    let has_rates = measured.iter().all(|(_, s)| s.tokens_per_second.is_some());
    let output = (context as f64 * 0.18).clamp(16.0, 4096.0);
    measured
        .into_iter()
        .min_by(|(_, a), (_, b)| {
            let score = |s: &Summary| {
                (s.first_content_ms.unwrap()
                    + if has_rates {
                        output / s.tokens_per_second.unwrap() * 1000.0
                    } else {
                        0.0
                    })
                    / s.success_rate
            };
            score(a).total_cmp(&score(b))
        })
        .map(|(m, _)| m.id.clone())
}
pub fn record(store: &ConfigStore, id: &str, sample: Sample) -> Result<()> {
    store.update(|config| {
        let valid = config.models.iter().find(|m| m.id == id).and_then(|m| {
            config
                .providers
                .iter()
                .find(|p| p.id == m.provider_id)
                .map(|p| fingerprint(m, p))
        }) == Some(sample.fingerprint.clone());
        if !valid {
            return;
        }
        config
            .performance_samples
            .retain(|id, _| config.models.iter().any(|m| &m.id == id));
        let samples = config.performance_samples.entry(id.into()).or_default();
        samples.push(sample);
        if samples.len() > 30 {
            samples.drain(..samples.len() - 30);
        }
    })
}
pub fn record_log(store: &ConfigStore, log: &RequestLog, probe: bool) -> Result<()> {
    if log.performance_model_id.is_empty()
        || log.status == "cancelled"

    {
        return Ok(());
    }
    record(
        store,
        &log.performance_model_id,
        Sample {
            at: chrono::Utc::now().timestamp_millis(),
            fingerprint: log.performance_fingerprint.clone(),
            success: log.status == "success" && (!probe || log.first_content_ms.is_some()),
            first_content_ms: if log.streaming { log.first_content_ms } else { None },
            duration_ms: log.upstream_duration_ms,
            output_tokens: log.output_tokens,
            context_tokens: log.performance_context_tokens,
            requires_vision: log.performance_requires_vision,
            probe,
        },
    )
}
impl Runner {
    pub fn view(&self, config: &AppConfig) -> View {
        let now = chrono::Utc::now().timestamp_millis();
        View {
            settings: config.performance_settings.clone(),
            job: self.job.lock().unwrap().clone(),
            models: config
                .models
                .iter()
                .filter_map(|m| {
                    config
                        .providers
                        .iter()
                        .find(|p| p.id == m.provider_id)
                        .map(|p| (m.id.clone(), summary(config, m, p, None, now)))
                })
                .collect(),
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
    pub fn start(self: &Arc<Self>, store: Arc<ConfigStore>, ids: Vec<String>) -> Result<()> {
        let config = store.read();
        let ids: Vec<_> = ids
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        ensure!(
            !ids.is_empty()
                && ids.iter().all(|id| config.models.iter().any(|m| &m.id == id
                    && m.enabled
                    && config
                        .providers
                        .iter()
                        .any(|p| p.id == m.provider_id && p.enabled))),
            "Select enabled models with enabled providers"
        );
        ensure!(
            !self.busy.swap(true, Ordering::SeqCst),
            "A speed test is already running"
        );
        self.cancel.store(false, Ordering::SeqCst);
        // Clear any old cancellation notification before starting a new batch.
        let _ = self.wake.notified().now_or_never();
        *self.job.lock().unwrap() = Job {
            running: true,
            total: ids.len() * 3,
            total_models: ids.len(),
            ..Default::default()
        };
        let runner = self.clone();
        tauri::async_runtime::spawn(async move {
            let probes = futures_util::future::join_all(ids.into_iter().map(|id| {
                let runner = runner.clone();
                let store = store.clone();
                async move {
                    if runner.cancel.load(Ordering::SeqCst) { return; }
                    runner.job.lock().unwrap().current_models.push(id.clone());
                    for _ in 0..3 {
                        if runner.cancel.load(Ordering::SeqCst) { break; }
                        let result = probe(&store, &id).await;
                        let mut job = runner.job.lock().unwrap();
                        job.completed += 1;
                        if result.is_err() {
                            job.error = Some("Some speed tests failed. Check model availability and credentials.".into());
                        }
                    }
                    let mut job = runner.job.lock().unwrap();
                    job.current_models.retain(|active| active != &id);
                    if !runner.cancel.load(Ordering::SeqCst) { job.completed_models += 1; }
                }
            }));
            // Dropping the batch cancels every in-flight HTTP request together.
            tokio::select! {
                _ = runner.wake.notified() => {},
                _ = probes => {},
            }
            let mut job = runner.job.lock().unwrap();
            job.running = false;
            job.current_models.clear();
            runner.busy.store(false, Ordering::SeqCst);
        });
        Ok(())
    }
}
use futures_util::FutureExt;
async fn probe(store: &ConfigStore, id: &str) -> Result<()> {
    let config = store.read();
    let model = config
        .models
        .iter()
        .find(|m| m.id == id && m.enabled)
        .ok_or_else(|| anyhow::anyhow!("Model unavailable"))?;
    let provider = config
        .providers
        .iter()
        .find(|p| p.id == model.provider_id && p.enabled)
        .ok_or_else(|| anyhow::anyhow!("Provider unavailable"))?;
    let protocol = Protocol::upstream(model, provider)?;
    let prompt = "Count from 1 to 40, separated by spaces. Do not explain.";
    let body = match protocol {
        Protocol::Responses => {
            json!({"model":model.model_id,"input":prompt,"max_output_tokens":128,"stream":true})
        }
        Protocol::Messages => {
            json!({"model":model.model_id,"messages":[{"role":"user","content":prompt}],"max_tokens":128,"stream":true})
        }
        Protocol::Chat => {
            json!({"model":model.model_id,"messages":[{"role":"user","content":prompt}],"max_tokens":128,"stream":true,"stream_options":{"include_usage":true}})
        }
    };
    let mut test_headers = axum::http::HeaderMap::new();
    test_headers.insert("user-agent", "AutoJev/ModelTest".parse().unwrap());
    let capture = Capture::new(
        protocol.path().trim_start_matches("/v1/"),
        &body,
        &test_headers,
    );
    {
        let mut c = capture.lock().unwrap();
        c.measure(model, provider, 32);
        c.upstream(protocol, true);
    }
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let client = config.gateway.client()?;
        let mut request = client
            .post(crate::proxy::endpoint_url(
                &provider.base_url,
                protocol.path(),
            ))
            .json(&body)
            .header("accept", "text/event-stream")
            .header("user-agent", "AutoJev/ModelTest")
            .header("HTTP-Referer", "https://autojev.ai")
            .header("X-Title", "AutoJev");
        if protocol == Protocol::Messages {
            request = request.header("anthropic-version", "2023-06-01");
        }
        if provider.kind != ProviderKind::Ollama {
            let key = store
                .read_secret(&format!("provider:{}", provider.id))
                .ok_or_else(|| anyhow::anyhow!("Missing API key"))?;
            request = if protocol == Protocol::Messages {
                request.header("x-api-key", key)
            } else {
                request.bearer_auth(key)
            };
        }
        let response = store.dispatcher.send(crate::dispatch::Target { provider, model_id: &model.model_id, protocol }, request).await?;
        capture.lock().unwrap().log.status_code = response.status().as_u16();
        ensure!(response.status().is_success(), "Speed test HTTP failure");
        let sse = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| s.contains("text/event-stream"));
        capture.lock().unwrap().upstream(protocol, sse);
        let mut stream = response.bytes_stream();
        let mut size = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            size += chunk.len();
            ensure!(size <= 1024 * 1024, "Speed test response too large");
            capture.lock().unwrap().bytes(&chunk);
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    let succeeded = matches!(result, Ok(Ok(())));
    let log = {
        let mut c = capture.lock().unwrap();
        if !succeeded {
            c.log.error = "Speed test failed or timed out".into();
        }
        c.finish(true)
    };
    record_log(store, &log, true)?;
    ensure!(
        log.status == "success" && log.first_content_ms.is_some(),
        "Speed test did not return measurable content"
    );
    Ok(())
}
pub async fn schedule(store: Arc<ConfigStore>, runner: Arc<Runner>) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let config = store.read();
        if !config.performance_settings.enabled {
            continue;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let age = (config.performance_settings.interval_minutes.clamp(5, 120) * 60 * 1000) as i64;
        let ids = config
            .models
            .iter()
            .filter(|m| m.enabled)
            .filter_map(|m| {
                let p = config
                    .providers
                    .iter()
                    .find(|p| p.id == m.provider_id && p.enabled)?;
                let s = summary(&config, m, p, None, now);
                s.last_test_at
                    .is_none_or(|at| now - at >= age)
                    .then(|| m.id.clone())
            })
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            let _ = runner.start(store.clone(), ids);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(config: &AppConfig, index: usize, latency: u64) -> Sample {
        let m = &config.models[index];
        let p = config
            .providers
            .iter()
            .find(|p| p.id == m.provider_id)
            .unwrap();
        Sample {
            at: chrono::Utc::now().timestamp_millis(),
            fingerprint: fingerprint(m, p),
            success: true,
            first_content_ms: Some(latency),
            duration_ms: latency + 1000,
            output_tokens: Some(101),
            context_tokens: 100,
            probe: true,
            requires_vision: false,
        }
    }
    #[test]
    fn image_workloads_never_use_text_probe_measurements() {
        let mut config = AppConfig::default();
        let id = config.models[0].id.clone();
        config.performance_samples.insert(id.clone(), vec![sample(&config, 0, 10)]);
        let eligible = vec![(config.models[0].clone(), config.providers.iter().find(|p| p.id == config.models[0].provider_id).unwrap().clone())];
        assert!(fastest(&config, &eligible, 100, false).is_some());
        assert!(fastest(&config, &eligible, 100, true).is_none());
        let mut image = sample(&config, 0, 500);
        image.requires_vision = true;
        image.probe = false;
        config.performance_samples.get_mut(&id).unwrap().push(image);
        let summary = summary_for(&config, &eligible[0].0, &eligible[0].1, Some(100), Some(true), chrono::Utc::now().timestamp_millis());
        assert_eq!(summary.first_content_ms, Some(500.0));
        assert_eq!(summary.samples, 1);
    }

    #[test]
    fn medians_expiry_context_and_endpoint_invalidation() {
        let mut config = AppConfig::default();
        let id = config.models[0].id.clone();
        let samples = [100, 120, 110, 10000, 130]
            .map(|n| sample(&config, 0, n))
            .to_vec();
        config.performance_samples.insert(id.clone(), samples);
        let m = &config.models[0];
        let p = &config.providers[0];
        let now = chrono::Utc::now().timestamp_millis();
        let s = summary(&config, m, p, Some(100), now);
        assert_eq!(s.first_content_ms, Some(120.0));
        assert_eq!(s.tokens_per_second, Some(100.0));
        assert_eq!(s.samples, 5);
        assert!(summary(&config, m, p, None, now + FRESH_MS + 1).stale);
        config.providers[0].base_url.push_str("/changed");
        assert_eq!(
            summary(&config, &config.models[0], &config.providers[0], None, now).samples,
            0
        );
    }
    #[test]
    fn comparable_real_samples_override_probes_and_failures_affect_ranking() {
        let mut config = AppConfig::default();
        let id = config.models[0].id.clone();
        let probe = sample(&config, 0, 100);
        let mut real = sample(&config, 0, 900);
        real.probe = false;
        real.context_tokens = 50000;
        config.performance_samples.insert(id, vec![probe, real]);
        let now = chrono::Utc::now().timestamp_millis();
        assert_eq!(
            summary(
                &config,
                &config.models[0],
                &config.providers[0],
                Some(50000),
                now
            )
            .first_content_ms,
            Some(900.0)
        );
        assert_eq!(
            summary(
                &config,
                &config.models[0],
                &config.providers[0],
                Some(100),
                now
            )
            .first_content_ms,
            Some(100.0)
        );
        let a = sample(&config, 0, 100);
        let mut failed = a.clone();
        failed.success = false;
        let b = sample(&config, 1, 200);
        config
            .performance_samples
            .insert(config.models[0].id.clone(), vec![a, failed.clone(), failed]);
        config
            .performance_samples
            .insert(config.models[1].id.clone(), vec![b]);
        let eligible = config
            .models
            .iter()
            .map(|m| {
                (
                    m.clone(),
                    config
                        .providers
                        .iter()
                        .find(|p| p.id == m.provider_id)
                        .unwrap()
                        .clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            fastest(&config, &eligible, 100, false),
            Some(config.models[1].id.clone())
        );
    }
    #[tokio::test]
    async fn speed_routing_ignores_legacy_priority_and_respects_capabilities_and_expiry() {
        let mut config = AppConfig::default();
        config.models.truncate(2);
        for m in &mut config.models {
            m.enabled = true;
            m.tier = crate::config::ModelTier::Strong;
            m.context_window = 100000;
        }
        config.policy.decision_preference = "speed".into();
        config.policy.jev_endpoint = "http://127.0.0.1:1/must-not-be-called".into();
        config.routes = vec![crate::config::RouteRule { all_models: false,
            id: "speed".into(),
            name: "Speed".into(),
            strategy: "jev".into(),
            enabled: true,
            automatic_policy: None,
            model_settings: Default::default(),
            model_ids: config.models.iter().map(|m| m.id.clone()).collect(),
        }];
        let input = crate::router::RoutePreviewInput {
            prompt: "hello".into(),
            endpoint: "chat/completions".into(),
            requires_tools: false,
            requires_vision: false,
            estimated_context_tokens: 100,
            requested_model: Some("autojev/speed".into()),
        };
        for (i, n) in [(0, 900), (1, 100)] {
            config
                .performance_samples
                .insert(config.models[i].id.clone(), vec![sample(&config, i, n)]);
        }
        let client = reqwest::Client::new();
        let result = crate::router::decide(&config, &input, &client, Some("test"))
            .await
            .unwrap();
        assert_eq!(result.model.id, config.models[1].id);
        assert!(result.decision.reason.contains("recent measured"));
        assert_eq!(result.decision.source, "local");
        config.routes[0].model_settings.insert(
            config.models[0].id.clone(),
            crate::config::RouteModelSettings {
                priority: 10,
                weight: 1,
            },
        );
        assert_eq!(
            crate::router::decide(&config, &input, &client, None)
                .await
                .unwrap()
                .model
                .id,
            config.models[1].id
        );
        config.routes[0].model_settings.clear();
        config.models[1].context_window = 50;
        assert_eq!(
            crate::router::decide(&config, &input, &client, None)
                .await
                .unwrap()
                .model
                .id,
            config.models[1].id
        );
        config.models[1].context_window = 100000;
        for samples in config.performance_samples.values_mut() {
            for s in samples {
                s.at -= FRESH_MS + 1;
            }
        }
        let a = crate::router::decide(&config, &input, &client, Some("test"))
            .await
            .unwrap();
        let b = crate::router::decide(&config, &input, &client, Some("test"))
            .await
            .unwrap();
        assert_ne!(a.model.id, b.model.id);
        assert!(a.decision.reason.contains("load balancing"));
    }
    #[tokio::test]
    async fn probes_streams_all_protocols_persist_and_never_store_prompts_or_keys() {
        use axum::{routing::post, Router};
        for (protocol,payload) in [
            ("chat_completions","data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"1 2 3\"}}]}\n\ndata: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":3}}\n\ndata: [DONE]\n\n"),
            ("messages","data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"1 2 3\"}}\n\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":3}}\n\ndata: {\"type\":\"message_stop\"}\n\n"),
            ("responses","data: {\"type\":\"response.output_text.delta\",\"delta\":\"1 2 3\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":3}}}\n\n")
        ] {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
            let path=Protocol::parse(protocol).unwrap().path();
            let server=tokio::spawn(async move {axum::serve(listener,Router::new().route(path,post(move || async move {([("content-type","text/event-stream")],payload)}))).await.unwrap();});
            let dir=tempfile::tempdir().unwrap();let path=dir.path().join("test.db");let store=ConfigStore::load(path.clone()).unwrap();
            store.update(|c| {c.models.truncate(1);c.providers[0].base_url=format!("http://{address}");c.providers[0].api_type=protocol.into();c.gateway.proxy_mode="direct".into();}).unwrap();
            store.write_secret("provider:openrouter","not-for-logs").unwrap();
            let id=store.read().models[0].id.clone();probe(&store,&id).await.unwrap();
            let reopened=ConfigStore::load(path).unwrap().read();let samples=&reopened.performance_samples[&id];
            assert_eq!(samples.len(),1);assert!(samples[0].success);assert!(samples[0].first_content_ms.is_some());
            assert_eq!(samples[0].output_tokens,Some(3));
            let saved=serde_json::to_string(samples).unwrap();assert!(!saved.contains("not-for-logs"));assert!(!saved.contains("Count from"));
            server.abort();
        }
    }
    #[tokio::test]
    async fn batch_cancellation_is_prompt_and_prevents_overlapping_jobs() {
        use axum::{routing::post, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/v1/chat/completions",
                    post(|| async {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        ""
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(dir.path().join("test.db")).unwrap());
        store
            .update(|c| {
                c.providers[0].base_url = format!("http://{address}");
                c.gateway.proxy_mode = "direct".into();
            })
            .unwrap();
        store.write_secret("provider:openrouter", "test").unwrap();
        let id = store.read().models[0].id.clone();
        let runner = Arc::new(Runner::default());
        runner.start(store.clone(), vec![id.clone()]).unwrap();
        assert!(runner.start(store.clone(), vec![id]).is_err());
        tokio::time::sleep(Duration::from_millis(20)).await;
        runner.cancel();
        tokio::time::timeout(Duration::from_secs(1), async {
            while runner.view(&store.read()).job.running {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(store.read().performance_samples.is_empty());
        server.abort();
    }
    #[test]
    fn automatic_tests_default_on_and_migrate_once_preserving_later_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        assert!(store.read().performance_settings.enabled);
        assert_eq!(store.read().performance_settings.interval_minutes, 30);
        let mut old = serde_json::to_value(store.read()).unwrap();
        old["performance_settings"] = json!({"enabled":false,"interval_minutes":30});
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE app_meta SET value = ?1 WHERE key = 'config'",
                [old.to_string()],
            )
            .unwrap();
        let migrated = ConfigStore::load(path.clone()).unwrap();
        assert!(migrated.read().performance_settings.enabled);
        migrated
            .update(|c| {
                c.performance_settings.enabled = false;
                c.performance_settings.interval_minutes = 15;
            })
            .unwrap();
        let reloaded = ConfigStore::load(path).unwrap();
        assert!(!reloaded.read().performance_settings.enabled);
        assert_eq!(reloaded.read().performance_settings.interval_minutes, 15);
    }

    #[tokio::test]
    async fn selected_models_run_in_parallel_and_unselected_models_are_not_called() {
        use axum::{routing::post, Json, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener,Router::new().route("/v1/chat/completions",post(move |Json(body):Json<serde_json::Value>| {
                let barrier=barrier.clone();let calls=observed.clone();
                async move {
                    calls.lock().unwrap().push(body["model"].as_str().unwrap().to_owned());
                    barrier.wait().await;
                    ([("content-type","text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n")
                }
            }))).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(dir.path().join("test.db")).unwrap());
        store
            .update(|c| {
                c.models.truncate(1);
                c.providers[0].base_url = format!("http://{address}");
                c.gateway.proxy_mode = "direct".into();
                let first = c.models[0].clone();
                for id in ["selected-second", "unselected"] {
                    let mut m = first.clone();
                    m.id = id.into();
                    m.model_id = id.into();
                    c.models.push(m);
                }
            })
            .unwrap();
        store.write_secret("provider:openrouter", "test").unwrap();
        let first = store.read().models[0].id.clone();
        let upstream = store.read().models[0].model_id.clone();
        let runner = Arc::new(Runner::default());
        assert!(runner.start(store.clone(), vec![]).is_err());
        runner
            .start(
                store.clone(),
                vec![first.clone(), "selected-second".into(), first.clone()],
            )
            .unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(5), async {
            while runner.view(&store.read()).job.running {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if completed.is_err() {
            runner.cancel();
        }
        server.abort();
        completed.unwrap();
        let job = runner.view(&store.read()).job;
        assert_eq!(job.total, 6);
        assert_eq!(job.completed, 6);
        assert_eq!(job.total_models, 2);
        assert_eq!(job.completed_models, 2);
        assert!(job.current_models.is_empty());
        let calls = calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|id| *id == &upstream).count(), 3);
        assert_eq!(
            calls
                .iter()
                .filter(|id| id.as_str() == "selected-second")
                .count(),
            3
        );
        assert!(!calls.iter().any(|id| id == "unselected"));
    }
}
