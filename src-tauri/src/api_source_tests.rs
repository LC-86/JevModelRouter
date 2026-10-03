//! R2 public persistence and listening-gateway acceptance; fictional keys only.
use crate::config::{AppConfig, ConfigStore};
use axum::{
    extract::OriginalUri,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

async fn source_fixture() -> (
    tempfile::TempDir,
    Arc<ConfigStore>,
    Arc<Mutex<Vec<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let records = received.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(post(move |uri: OriginalUri, headers: HeaderMap, Json(body): Json<serde_json::Value>| {
            let records = records.clone();
            async move {
                let path = uri.0.path().to_string();
                records.lock().unwrap().push(json!({"path":path,"authorization":headers.get("authorization").and_then(|v|v.to_str().ok()),"model":body["model"]}));
                if body.pointer("/messages/0/content").and_then(|v| v.as_str()) == Some("split-secret") {
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let text = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"你好 {key} {key}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n");
                    let chunks: std::collections::VecDeque<_> = text.as_bytes().chunks(3).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move {
                        let chunk = chunks.pop_front()?;
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                        Some((Ok::<_, std::io::Error>(chunk), chunks))
                    });
                    return axum::response::Response::builder().header("content-type", "text/event-stream")
                        .header("x-request-id", key).body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if let Some(status) = body.pointer("/messages/0/content").and_then(|v|v.as_str()).and_then(|v| v.strip_prefix("fail-")).and_then(|v| v.parse::<u16>().ok()) {
                    return (StatusCode::from_u16(status).unwrap(), Json(json!({"error":{"message":headers["authorization"].to_str().unwrap()}}))).into_response();
                }
                if serde_json::to_string(&body).unwrap().contains("escaped-secret") {
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let escaped = format!("\\u0066{}", &key[1..]);
                    if body["stream"] == true {
                        return ([("content-type", "text/event-stream")], format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{escaped}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n")).into_response();
                    }
                    return ([("content-type", "application/json")], format!("{{\"error\":{{\"message\":\"{escaped}\"}}}}")).into_response();
                }
                if serde_json::to_string(&body).unwrap().contains("delta-secret") {
                    let key = headers.get("authorization").or_else(|| headers.get("x-api-key")).unwrap().to_str().unwrap().trim_start_matches("Bearer ");
                    let (mut text, terminal) = if path.ends_with("/messages") {
                        ("event: message_start\ndata: {\"message\":{\"usage\":{}}}\n\nevent: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_string(), "event: content_block_stop\ndata: {\"index\":0}\n\nevent: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {}\n\n")
                    } else if path.ends_with("/responses") {
                        (String::new(), "event: response.completed\ndata: {\"response\":{\"id\":\"fixture\",\"status\":\"completed\",\"output\":[]}}\n\n")
                    } else {
                        (String::new(), "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                    };
                    for piece in key.chars().map(|c| c.to_string()) {
                        let frame = if path.ends_with("/messages") { format!("event: content_block_delta\ndata: {}\n\n", json!({"index":0,"delta":{"type":"text_delta","text":piece}})) }
                        else if path.ends_with("/responses") { format!("event: response.output_text.delta\ndata: {}\n\n", json!({"output_index":0,"content_index":0,"delta":piece})) }
                        else { format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"content":piece},"finish_reason":null}]})) };
                        text.push_str(&frame);
                    }
                    text.push_str(terminal);
                    let chunks: std::collections::VecDeque<_> = text.as_bytes().chunks(71).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move { let chunk = chunks.pop_front()?; tokio::task::yield_now().await; Some((Ok::<_, std::io::Error>(chunk), chunks)) });
                    return axum::response::Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if serde_json::to_string(&body).unwrap().contains("nested-secret") {
                    let arguments = r#"{"note":"\u0066ictional-coding"}"#;
                    if path.ends_with("/messages") {
                        if body["stream"] == true {
                            let mut text = "event: message_start\ndata: {\"message\":{\"usage\":{}}}\n\nevent: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_fixture\",\"name\":\"echo\",\"input\":{}}}\n\n".to_string();
                            for piece in arguments.chars().map(|c| c.to_string()) { text.push_str(&format!("event: content_block_delta\ndata: {}\n\n", json!({"index":0,"delta":{"type":"input_json_delta","partial_json":piece}}))); }
                            text.push_str("event: content_block_stop\ndata: {\"index\":0}\n\nevent: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}\n\nevent: message_stop\ndata: {}\n\n");
                            return ([("content-type", "text/event-stream")], text).into_response();
                        }
                        return Json(json!({"id":"fixture","content":[{"type":"tool_use","id":"call_fixture","name":"echo","input":serde_json::from_str::<serde_json::Value>(arguments).unwrap()}],"stop_reason":"tool_use"})).into_response();
                    }
                    if path.ends_with("/responses") {
                        let item = json!({"type":"function_call","call_id":"call_fixture","name":"echo","arguments":arguments});
                        let response = json!({"id":"fixture","status":"completed","output":[item]});
                        if body["stream"] == true {
                            let mut text = format!("event: response.output_item.added\ndata: {}\n\n", json!({"output_index":0,"item":{"type":"function_call","call_id":"call_fixture","name":"echo","arguments":""}}));
                            for piece in arguments.chars().map(|c| c.to_string()) { text.push_str(&format!("event: response.function_call_arguments.delta\ndata: {}\n\n", json!({"output_index":0,"delta":piece}))); }
                            text.push_str(&format!("event: response.completed\ndata: {}\n\n", json!({"response":response})));
                            return ([("content-type", "text/event-stream")], text).into_response();
                        }
                        return Json(response).into_response();
                    }
                    if body["stream"] == true {
                        let mut text = format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_fixture","type":"function","function":{"name":"echo","arguments":""}}]},"finish_reason":null}]}));
                        for piece in arguments.chars().map(|c| c.to_string()) {
                            text.push_str(&format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":piece}}]},"finish_reason":null}]})));
                        }
                        text.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n");
                        return ([("content-type", "text/event-stream")], text).into_response();
                    }
                    return Json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_fixture","type":"function","function":{"name":"echo","arguments":arguments}}]},"finish_reason":"tool_calls"}]})).into_response();
                }
                if serde_json::to_string(&body).unwrap().contains("inflight-secret") { tokio::time::sleep(std::time::Duration::from_millis(150)).await; }
                if serde_json::to_string(&body).unwrap().contains("slow-json") {
                    let bytes = serde_json::to_vec(&json!({"choices":[{"message":{"role":"assistant","content":"fictional slow response"},"finish_reason":"stop"}]})).unwrap();
                    let chunks: std::collections::VecDeque<_> = bytes.chunks(16).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move {
                        let chunk = chunks.pop_front()?;
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        Some((Ok::<_, std::io::Error>(chunk), chunks))
                    });
                    return axum::response::Response::builder().header("content-type", "application/json").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if !["/official/v1/chat/completions","/third/api/v1/chat/completions","/coding/v4/chat/completions"].contains(&path.as_str()) {
                    return (StatusCode::NOT_FOUND, Json(json!({"error":{"message":"unsupported endpoint"}}))).into_response();
                }
                Json(json!({"id":"fixture","choices":[{"message":{"role":"assistant","content":path},"finish_reason":"stop"}]})).into_response()
            }
        }))).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(root.path().join("sources.db")).unwrap());
    store.update(|config| {
        let original_provider = config.providers[0].clone();
        let original_model = config.models[0].clone();
        config.providers.clear(); config.models.clear(); config.port = 0;
        config.gateway.proxy_mode = "direct".into();
        config.gateway.failure_threshold = 20;
        config.policy.use_jev_when_ambiguous = true;
        for (id, kind, endpoint) in [("official","official_api","/official/v1"),("third","third_party_api","/third/api/v1"),("coding","coding_plan","/coding/v4")] {
            let mut provider = original_provider.clone();
            provider.id = id.into(); provider.name = id.into(); provider.base_url = format!("http://{address}{endpoint}"); provider.api_type = "chat_completions".into();
            let mut model = original_model.clone(); model.id = format!("uuid-{id}"); model.provider_id = id.into(); model.model_id = "same-model".into();
            config.api_sources.insert(id.into(), serde_json::from_value(json!({"connection_instance_id":format!("instance-{id}"),"generation":1,"kind":kind,"endpoint":provider.base_url,"api_type":"chat_completions","credential_reference":format!("api-generation:instance-{id}"),"model_bindings":std::collections::HashMap::from([(model.id.clone(), model.model_id.clone())]),"account_label":null,"plan_label":null})).unwrap());
            config.providers.push(provider); config.models.push(model);
        }
    }).unwrap();
    for id in ["official", "third", "coding"] {
        store
            .write_secret(
                &format!("api-generation:instance-{id}"),
                &format!("fictional-{id}"),
            )
            .unwrap();
    }
    (root, store, received, task)
}

#[tokio::test]
async fn api_sources_same_model_hits_only_the_explicit_endpoint_and_generation_key() {
    let (_root, store, received, upstream) = source_fixture().await;
    // A decision key exists but must never be used for generation.
    store
        .write_secret("autojev-cloud", "fictional-decision-only")
        .unwrap();
    let gateway = crate::proxy::start(store).await.unwrap();
    for id in ["official", "third", "coding"] {
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
            .json(&json!({"model":format!("autojev/model/uuid-{id}"),"messages":[{"role":"user","content":"fictional request"}]})).send().await.unwrap();
        assert_eq!(reply.status(), 200, "{id}: {}", reply.text().await.unwrap());
    }
    let records = received.lock().unwrap().clone();
    assert_eq!(records.len(), 3);
    for (record, id) in records.iter().zip(["official", "third", "coding"]) {
        assert_eq!(record["authorization"], format!("Bearer fictional-{id}"));
        assert_eq!(record["model"], "same-model");
    }
    gateway.stop().await;
    upstream.abort();
}

#[test]
fn api_source_identity_survives_reopen_without_projecting_a_secret() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sources.db");
    let store = ConfigStore::load(path.clone()).unwrap();
    let mut value = serde_json::to_value(store.read()).unwrap();
    value["api_sources"] = json!({"openrouter": {
        "connection_instance_id":"fixture-connection", "generation":1,
        "kind":"third_party_api", "endpoint":"https://example.invalid/api/v1",
        "api_type":"chat_completions", "credential_reference":"api-generation:fixture-connection",
        "account_label":null, "plan_label":null,
        "account_state":"unknown", "plan_state":"unknown"
    }});
    store
        .update(|config| *config = serde_json::from_value::<AppConfig>(value).unwrap())
        .unwrap();
    store
        .write_secret(
            "api-generation:fixture-connection",
            "fictional-generation-key",
        )
        .unwrap();
    drop(store);
    let reopened = ConfigStore::load(path).unwrap();
    let projection = serde_json::to_value(reopened.read()).unwrap();
    assert_eq!(
        projection["api_sources"]["openrouter"]["connection_instance_id"],
        "fixture-connection"
    );
    assert_eq!(
        projection["api_sources"]["openrouter"]["plan_state"],
        "unknown"
    );
    assert!(!projection.to_string().contains("fictional-generation-key"));
}

#[tokio::test]
async fn api_source_changed_endpoint_is_rejected_without_any_upstream_request() {
    let (_root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| config.providers[2].base_url = config.providers[0].base_url.clone())
        .unwrap();
    let gateway = crate::proxy::start(store).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"fixture"}]})).send().await.unwrap();
    assert!(!reply.status().is_success());
    assert!(reply
        .text()
        .await
        .unwrap()
        .contains("source_target_changed"));
    assert!(received.lock().unwrap().is_empty());
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_upstream_failure_never_projects_its_generation_secret() {
    let (_root, store, received, upstream) = source_fixture().await;
    for status in [200, 401, 429, 500] {
        // Each error starts with a healthy fixed target; 401 otherwise cools down the next request.
        let gateway = crate::proxy::start(store.clone()).await.unwrap();
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
            .json(&json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":format!("fail-{status}")}]})).send().await.unwrap();
        assert_eq!(
            reply.status().as_u16(),
            if status == 200 { 502 } else { status }
        );
        let body = reply.text().await.unwrap();
        assert!(!body.contains("fictional-coding"));
        assert!(body.contains("[REDACTED]"));
        gateway.stop().await;
    }
    assert_eq!(received.lock().unwrap().len(), 4);
    assert!(received
        .lock()
        .unwrap()
        .iter()
        .all(|record| record["path"] == "/coding/v4/chat/completions"));
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    upstream.abort();
}

#[tokio::test]
async fn api_sources_all_refused_targets_dispatch_zero_to_every_other_source() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let original = store.read();
    for id in ["official", "third", "coding"] {
        for condition in ["missing", "disabled", "removed", "protocol"] {
            store
                .update(|config| {
                    *config = original.clone();
                    if condition == "disabled" {
                        config
                            .providers
                            .iter_mut()
                            .find(|p| p.id == id)
                            .unwrap()
                            .enabled = false;
                    }
                    if condition == "removed" {
                        config.models.retain(|m| m.provider_id != id);
                    }
                    if condition == "protocol" {
                        config
                            .models
                            .iter_mut()
                            .find(|m| m.provider_id == id)
                            .unwrap()
                            .api_type = "responses".into();
                    }
                })
                .unwrap();
            if condition == "missing" {
                store
                    .delete_secret(&format!("api-generation:instance-{id}"))
                    .unwrap();
            }
            let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
                .json(&json!({"model":format!("autojev/model/uuid-{id}"),"messages":[{"role":"user","content":"fixture"}]})).send().await.unwrap();
            assert!(!reply.status().is_success(), "{id}/{condition}");
            let body = reply.text().await.unwrap();
            assert!(
                body.contains(match condition {
                    "missing" => "source_credential_missing",
                    "disabled" => "source_disabled",
                    "removed" => "source_model_retired",
                    _ => "source_protocol_unsupported",
                }),
                "{id}/{condition}: {body}"
            );
            assert!(
                received.lock().unwrap().is_empty(),
                "{id}/{condition} dispatched"
            );
            store
                .write_secret(
                    &format!("api-generation:instance-{id}"),
                    &format!("fictional-{id}"),
                )
                .unwrap();
        }
    }
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_split_stream_secret_never_reaches_output_headers_or_logs() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":"autojev/model/uuid-coding","stream":true,"messages":[{"role":"user","content":"split-secret"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert!(!reply.headers().contains_key("x-request-id"));
    let body = reply.text().await.unwrap();
    assert!(body.contains("你好 [REDACTED] [REDACTED]"), "{body}");
    assert!(body.contains("data: [DONE]"));
    assert!(!body.contains("fictional-coding"));
    assert_eq!(received.lock().unwrap().len(), 1);
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_escaped_json_secret_never_reaches_text_or_stream_output() {
    let (_root, store, _received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    for endpoint in ["chat/completions", "responses", "messages"] {
        for streaming in [false, true] {
            let mut body = json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"escaped-secret"}],"max_tokens":8,"stream":streaming});
            if endpoint == "responses" {
                body["input"] = json!("escaped-secret");
                body.as_object_mut().unwrap().remove("messages");
            }
            let reply = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/v1/{endpoint}", gateway.port))
                .json(&body)
                .send()
                .await
                .unwrap();
            let output = reply.text().await.unwrap();
            assert!(!output.contains("fictional-coding"), "{endpoint}: {output}");
            for data in output
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
            {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                    assert!(
                        !value.to_string().contains("fictional-coding"),
                        "Decoded SSE leaked a credential"
                    );
                }
            }
            assert!(output.contains("[REDACTED]"), "{endpoint}: {output}");
        }
    }
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_legacy_database_copy_preserves_existing_uuid_and_credential_reference() {
    let (root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| {
            config.api_sources.clear();
            config.providers.truncate(1);
            config.models.truncate(1);
        })
        .unwrap();
    store
        .write_secret("provider:official", "fictional-official")
        .unwrap();
    let expected = store.read().models[0].clone();
    let mut legacy = serde_json::to_value(store.read()).unwrap();
    legacy.as_object_mut().unwrap().remove("api_sources");
    drop(store);
    let original = root.path().join("sources.db");
    let database = rusqlite::Connection::open(&original).unwrap();
    database
        .execute(
            "UPDATE app_meta SET value = ?1 WHERE key = 'config'",
            [legacy.to_string()],
        )
        .unwrap();
    drop(database);
    let original_bytes = std::fs::read(&original).unwrap();
    let copy = root.path().join("isolated-copy.db");
    std::fs::copy(&original, &copy).unwrap();
    let reopened = Arc::new(ConfigStore::load(copy).unwrap());
    let config = reopened.read();
    assert!(config.api_sources.is_empty());
    assert_eq!(config.models[0].id, expected.id);
    assert_eq!(config.models[0].provider_id, expected.provider_id);
    let gateway = crate::proxy::start(reopened).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":format!("autojev/model/{}", expected.id),"messages":[{"role":"user","content":"fixture"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert_eq!(
        received.lock().unwrap()[0]["authorization"],
        "Bearer fictional-official"
    );
    assert_eq!(
        std::fs::read(original).unwrap(),
        original_bytes,
        "Only the isolated copy may be changed"
    );
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_nested_tool_json_secret_never_reappears_after_conversion() {
    let (_root, store, _received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let mut failures = Vec::new();
    for source_protocol in ["chat_completions", "responses", "messages"] {
        store
            .update(|config| {
                config
                    .providers
                    .iter_mut()
                    .find(|p| p.id == "coding")
                    .unwrap()
                    .api_type = source_protocol.into();
                config
                    .models
                    .iter_mut()
                    .find(|m| m.id == "uuid-coding")
                    .unwrap()
                    .api_type = String::new();
                config.api_sources.get_mut("coding").unwrap().api_type = source_protocol.into();
            })
            .unwrap();
        for endpoint in ["chat/completions", "responses", "messages"] {
            for stream in [false, true] {
                let schema = json!({"type":"object","properties":{"note":{"type":"string"}}});
                let tool = match endpoint {
                    "messages" => json!({"name":"echo","input_schema":schema}),
                    "responses" => json!({"type":"function","name":"echo","parameters":schema}),
                    _ => json!({"type":"function","function":{"name":"echo","parameters":schema}}),
                };
                let mut request = json!({"model":"autojev/model/uuid-coding","max_tokens":16,"stream":stream,"tools":[tool]});
                if endpoint == "responses" {
                    request["input"] = "nested-secret".into();
                } else {
                    request["messages"] = json!([{"role":"user","content":"nested-secret"}]);
                }
                let reply = reqwest::Client::new()
                    .post(format!("http://127.0.0.1:{}/v1/{endpoint}", gateway.port))
                    .json(&request)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(reply.status(), 200);
                let body = reply.bytes().await.unwrap();
                let value = if stream {
                    crate::protocol::collect_debug_stream(
                        &body,
                        crate::protocol::Protocol::parse(endpoint).unwrap(),
                        "fixture",
                    )
                    .unwrap()
                } else {
                    serde_json::from_slice(&body).unwrap()
                };
                let input = match endpoint {
                    "messages" => value["content"][0]["input"].clone(),
                    "responses" => {
                        serde_json::from_str(value["output"][0]["arguments"].as_str().unwrap())
                            .unwrap()
                    }
                    _ => serde_json::from_str(
                        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap(),
                };
                if input["note"] != "[REDACTED]" {
                    failures.push(format!(
                        "{source_protocol}/{endpoint}/stream={stream}: {value}"
                    ));
                }
            }
        }
    }
    gateway.stop().await;
    upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
}

#[tokio::test]
async fn api_source_redaction_preserves_incomplete_prefix_and_sse_block_order() {
    use futures_util::StreamExt;
    let frames = [
        ": keepalive\nprovider-extension: retained\n\n",
        "event: message_start\ndata: {\"message\":{\"usage\":{}}}\n\n",
        "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你好 fictional-\"}}\n\n",
        "event: content_block_stop\ndata: {\"index\":0}\n\n",
        "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
        "event: message_stop\ndata: {}\n\n",
    ];
    let input =
        futures_util::stream::iter(frames.into_iter().map(|text| {
            Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(text.as_bytes()))
        }));
    let safe = crate::api_sources::redacted_stream(input, Some("fictional-coding".into()), true);
    futures_util::pin_mut!(safe);
    let mut bytes = Vec::new();
    while let Some(chunk) = safe.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains(": keepalive\nprovider-extension: retained"));
    assert!(
        text.find("你好 fictional-").unwrap() < text.find("event: content_block_stop").unwrap()
    );
    let collected = crate::protocol::collect_debug_stream(
        &bytes,
        crate::protocol::Protocol::Messages,
        "fixture",
    )
    .unwrap();
    assert!(
        collected.to_string().contains("你好 fictional-"),
        "An incomplete prefix is legitimate output"
    );
}

#[tokio::test]
async fn api_source_cross_delta_secret_cannot_reappear_in_converted_output_or_logs() {
    let (_root, store, _received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    for source_protocol in ["chat_completions", "responses", "messages"] {
        store
            .update(|config| {
                config
                    .providers
                    .iter_mut()
                    .find(|p| p.id == "coding")
                    .unwrap()
                    .api_type = source_protocol.into();
                config
                    .models
                    .iter_mut()
                    .find(|m| m.id == "uuid-coding")
                    .unwrap()
                    .api_type = String::new();
                config.api_sources.get_mut("coding").unwrap().api_type = source_protocol.into();
            })
            .unwrap();
        for (endpoint, terminal) in [
            ("chat/completions", "[DONE]"),
            ("responses", "response.completed"),
            ("messages", "message_stop"),
        ] {
            let request = if endpoint == "responses" {
                json!({"model":"autojev/model/uuid-coding","input":"delta-secret","stream":true})
            } else {
                json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"delta-secret"}],"max_tokens":16,"stream":true})
            };
            let reply = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/v1/{endpoint}", gateway.port))
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), 200);
            let body = reply.text().await.unwrap();
            assert!(
                body.contains(terminal),
                "{source_protocol}/{endpoint}: {body}"
            );
            assert!(
                !body.contains("fictional-coding"),
                "{source_protocol}/{endpoint}: {body}"
            );
            assert!(
                body.contains("[REDACTED]"),
                "{source_protocol}/{endpoint}: {body}"
            );
        }
    }
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_retired_alias_and_uuid_cannot_be_captured_as_another_sources_bare_id() {
    let (_root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| {
            config.api_sources.get_mut("official").unwrap().retired = true;
            config.providers.retain(|p| p.id != "official");
            config.models.retain(|m| m.provider_id != "official");
            config.models[0].model_id = "official/same-model".into();
            config
                .api_sources
                .get_mut("third")
                .unwrap()
                .model_bindings
                .insert("uuid-third".into(), "official/same-model".into());
        })
        .unwrap();
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    for reference in ["official/same-model", "uuid-official"] {
        if reference == "uuid-official" {
            store
                .update(|config| {
                    config.models[0].model_id = reference.into();
                    config
                        .api_sources
                        .get_mut("third")
                        .unwrap()
                        .model_bindings
                        .insert("uuid-third".into(), reference.into());
                })
                .unwrap();
        }
        let reply = reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/v1/chat/completions",
                gateway.port
            ))
            .json(&json!({"model":reference,"messages":[{"role":"user","content":"fixture"}]}))
            .send()
            .await
            .unwrap();
        assert!(
            !reply.status().is_success(),
            "Retired reference dispatched: {reference}"
        );
        assert!(
            received.lock().unwrap().is_empty(),
            "Retired reference hit another source"
        );
    }
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_redaction_preserves_json_progress_for_idle_timeout() {
    let (_root, store, _received, upstream) = source_fixture().await;
    store
        .update(|config| config.gateway.stream_idle_seconds = 1)
        .unwrap();
    let gateway = crate::proxy::start(store).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"slow-json"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert!(reply
        .text()
        .await
        .unwrap()
        .contains("fictional slow response"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_inflight_generation_never_uses_replaced_legacy_or_decision_credentials() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let request = json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"inflight-secret"}]});
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port);
    let inflight = tokio::spawn(client.post(&url).json(&request).send());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while received.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    store
        .write_secret("provider:coding", "fictional-replacement-account")
        .unwrap();
    store
        .write_secret("autojev-cloud", "fictional-replacement-decision")
        .unwrap();
    let second = client.post(url).json(&request).send().await.unwrap();
    assert_eq!(second.status(), 200);
    assert_eq!(inflight.await.unwrap().unwrap().status(), 200);
    let records = received.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(
        |record| record["authorization"] == "Bearer fictional-coding"
            && record["path"] == "/coding/v4/chat/completions"
    ));
    drop(records);
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_sources_background_schedule_never_generates_or_speed_tests() {
    let (_root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| config.performance_settings.enabled = true)
        .unwrap();
    let task = tokio::spawn(crate::performance::schedule(
        store,
        Arc::new(crate::performance::Runner::default()),
    ));
    tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    assert!(received.lock().unwrap().is_empty());
    task.abort();
    upstream.abort();
}
