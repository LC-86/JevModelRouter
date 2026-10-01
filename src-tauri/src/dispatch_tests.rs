//! Regression at the listening gateway, using fictional credentials and loopback only.
use crate::{
    config::ConfigStore,
    protocol::{
        tests::{request, response, wire},
        Protocol,
    },
};
use axum::{http::HeaderMap, response::IntoResponse, routing::post, Json, Router};
use std::sync::Arc;

#[test]
fn local_generation_requests_keep_an_explicit_local_target_and_protocol_headers() {
    for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
        let body = serde_json::json!({
            "model": "autojev/model/fixture",
            "stream": true,
            "messages": [{"role":"user","content":"fixture"}]
        });
        let request = crate::dispatch::local_gateway_request(9527, protocol, &body)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.url().as_str(), format!("http://127.0.0.1:9527{}", protocol.path()));
        assert_eq!(request.headers()["accept"], "text/event-stream");
        assert!(request.headers().get("authorization").is_none());
        assert!(request.headers().get("x-api-key").is_none());
        assert_eq!(request.headers().get("anthropic-version").is_some(), protocol == Protocol::Messages);
    }
    assert!(crate::dispatch::local_gateway_request(0, Protocol::Chat, &serde_json::json!({"model":"autojev/model/fixture"})).is_err());
    assert!(crate::dispatch::local_gateway_request(9527, Protocol::Chat, &serde_json::json!({"model":"upstream-model"})).is_err());
}

#[tokio::test]
async fn isolated_dispatch_preserves_deadlines_for_headers_and_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/headers",
                    post(|| async {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        "late headers"
                    }),
                )
                .route(
                    "/body",
                    post(|| async {
                        let stream = futures_util::stream::once(async {
                            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"first chunk"))
                        });
                        let stalled =
                            futures_util::StreamExt::chain(stream, futures_util::stream::pending());
                        axum::body::Body::from_stream(stalled)
                    }),
                ),
        )
        .await
        .unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::load_with_dispatcher(
        temp.path().join("deadline.db"),
        Arc::new(crate::dispatch::ApiDispatcher {
            loopback_only: true,
        }),
    )
    .unwrap();
    let config = store.read();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in ["headers", "body"] {
        let request = client
            .post(format!("http://{address}/{path}"))
            .timeout(std::time::Duration::from_millis(100));
        let operation = async {
            let response = store
                .dispatcher
                .send(
                    crate::dispatch::Target {
                        provider: &config.providers[0],
                        model_id: "fixture-model",
                        protocol: Protocol::Chat,
                    },
                    request,
                )
                .await?;
            response.bytes().await.map_err(anyhow::Error::from)
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), operation)
            .await
            .expect("The isolated request must expire without process termination");
        let error = result.unwrap_err();
        assert!(
            error.downcast_ref::<reqwest::Error>().unwrap().is_timeout(),
            "{path}: {error}"
        );
    }
    upstream.abort();
}

#[tokio::test]
async fn gateway_ephemeral_port_preserves_protocols_and_persistence() {
    for target in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    target.path(),
                    post(
                        move |headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                            assert_eq!(body["model"], "fixture-model");
                            assert!(body.to_string().contains("上海"));
                            if target == Protocol::Messages {
                                assert_eq!(headers["x-api-key"], "fixture-key");
                                assert_eq!(headers["anthropic-version"], "2023-06-01");
                                assert!(headers.get("authorization").is_none());
                            } else {
                                assert_eq!(headers["authorization"], "Bearer fixture-key");
                            }
                            if body["stream"] == true {
                                ([("content-type", "text/event-stream")], wire(target))
                                    .into_response()
                            } else {
                                Json(response(target)).into_response()
                            }
                        },
                    ),
                ),
            )
            .await
            .unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("gateway.db");
        let store = Arc::new(ConfigStore::load(path.clone()).unwrap());
        store
            .update(|c| {
                c.port = 0;
                c.models.truncate(1);
                c.models[0].model_id = "fixture-model".into();
                c.models[0].api_type = target.path().trim_start_matches("/v1/").into();
                c.providers[0].base_url = format!("http://{address}");
                c.policy.use_jev_when_ambiguous = false;
                c.gateway.proxy_mode = "direct".into();
            })
            .unwrap();
        store
            .write_secret("provider:openrouter", "fixture-key")
            .unwrap();
        let gateway = crate::proxy::start(store.clone()).await.unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for source in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            for stream in [false, true] {
                let mut body = request(source, stream);
                body["model"] = "autojev/auto".into();
                let reply = client
                    .post(format!(
                        "http://127.0.0.1:{}{}",
                        gateway.port,
                        source.path()
                    ))
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(reply.status(), 200);
                assert_eq!(reply.headers()["x-autojev-model"], "fixture-model");
                let text = reply.text().await.unwrap();
                assert!(
                    text.contains("你") && text.contains("好"),
                    "{source:?}: {text}"
                );
                if stream {
                    assert!(text.contains(match source {
                        Protocol::Chat => "[DONE]",
                        Protocol::Responses => "response.completed",
                        Protocol::Messages => "message_stop",
                    }));
                } else {
                    assert!(text.contains("call_2"));
                }
            }
        }
        for action in ["generateContent", "streamGenerateContent"] {
            let reply = client
                .post(format!(
                    "http://127.0.0.1:{}/v1beta/models/autojev/auto:{action}",
                    gateway.port
                ))
                .json(&serde_json::json!({"contents":[{"role":"user","parts":[{"text":"上海"}]}]}))
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), 200);
            let text = reply.text().await.unwrap();
            assert!(text.contains("你好"), "{text}");
            assert!(text.contains("candidates"));
        }
        gateway.stop().await;
        let saved = ConfigStore::load(path).unwrap();
        assert_eq!(saved.read().models[0].model_id, "fixture-model");
        assert_eq!(
            saved.read_secret("provider:openrouter").as_deref(),
            Some("fixture-key")
        );
        assert_eq!(saved.request_logs("").unwrap().len(), 8);
        upstream.abort();
    }
}

#[tokio::test]
async fn gateway_denies_subscription_generation_without_reaching_any_upstream() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let counted = calls.clone();
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(post(move || {
                let counted = counted.clone();
                async move {
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"ok"}}]})).into_response()
                }
            })),
        )
        .await
        .unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("subscription.db")).unwrap());
    store
        .update(|c| {
            c.port = 0;
            c.models.truncate(1);
            c.models[0].model_id = "fixture-model".into();
            c.providers[0].base_url = format!("http://{address}");
            c.policy.use_jev_when_ambiguous = false;
            c.gateway.proxy_mode = "direct".into();
            // 订阅服务商与它的模型：身份、凭据与 API 服务商分开。
            let mut provider = c.providers[0].clone();
            provider.id = "codex".into();
            provider.name = "Codex".into();
            provider.kind = crate::config::ProviderKind::CodexSubscription;
            provider.base_url = String::new();
            provider.test_model = String::new();
            c.providers.push(provider.clone());
            let mut model = c.models[0].clone();
            model.id = "codex-subscription-model".into();
            model.provider_id = provider.id.clone();
            model.model_id = "codex-fixture-model".into();
            c.models.push(model);
            crate::subscription::sync_provider(c, &provider.id, &provider.kind);
        })
        .unwrap();
    store.write_secret("provider:openrouter", "fixture-key").unwrap();
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port);
    let body = |model: &str| {
        serde_json::json!({"model": model, "messages": [{"role": "user", "content": "fictional"}]})
    };

    // 未连接的订阅目标：网关、Debug 与测试入口共用的准入在派发前拒绝。
    for source in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
        let reply = client
            .post(format!("http://127.0.0.1:{}{}", gateway.port, source.path()))
            .json(&body("autojev/model/codex-subscription-model"))
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), 428, "{source:?}");
        assert_eq!(reply.headers()["x-autojev-subscription-denial"], "not_connected");
        assert_eq!(reply.headers()["x-should-retry"], "false");
        assert!(reply.text().await.unwrap().contains("not_connected"));
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0, "denied subscription work must never reach an upstream");

    // 停用该服务商后拒绝原因随之改变，仍然零派发。
    store.update(|c| c.providers.iter_mut().find(|p| p.id == "codex").unwrap().enabled = false).unwrap();
    let reply = client.post(&url).json(&body("autojev/model/codex-subscription-model")).send().await.unwrap();
    assert_eq!(reply.status(), 403);
    assert!(reply.text().await.unwrap().contains("provider_disabled"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    // 已连接但没有任何只读证据：按缺失证据拒绝，不是放行也不是笼统失败。
    store
        .update(|c| {
            c.providers.iter_mut().find(|p| p.id == "codex").unwrap().enabled = true;
            let connection = c.subscriptions.get_mut("codex").unwrap();
            connection.state = crate::subscription::ConnectionState::Connected;
            connection.identity = Some("fixture@example.invalid".into());
        })
        .unwrap();
    let reply = client.post(&url).json(&body("autojev/model/codex-subscription-model")).send().await.unwrap();
    assert_eq!(reply.status(), 428);
    assert!(reply.text().await.unwrap().contains("evidence_missing"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    // 自动选路不会选中被拒绝的订阅模型，API 目标仍然照常工作。
    let reply = client.post(&url).json(&body("autojev/auto")).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert_eq!(reply.headers()["x-autojev-model"], "fixture-model");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn isolated_gateway_rejects_remote_targets_and_never_follows_redirects() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/chat/completions",
                post(|| async {
                    (
                        axum::http::StatusCode::FOUND,
                        [("location", "https://example.invalid/forbidden")],
                        "redirect",
                    )
                }),
            ),
        )
        .await
        .unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ConfigStore::load_with_dispatcher(
            temp.path().join("isolation.db"),
            Arc::new(crate::dispatch::ApiDispatcher {
                loopback_only: true,
            }),
        )
        .unwrap(),
    );
    store
        .update(|c| {
            c.port = 0;
            c.models.truncate(1);
            c.policy.use_jev_when_ambiguous = false;
            c.providers[0].base_url = format!("http://{address}");
        })
        .unwrap();
    store
        .write_secret("provider:openrouter", "fixture-key")
        .unwrap();
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port);
    let body = serde_json::json!({"model":"autojev/auto","messages":[{"role":"user","content":"fictional"}]});
    let reply = client.post(&url).json(&body).send().await.unwrap();
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.text().await.unwrap(), "redirect");
    for forbidden in [
        "https://example.invalid",
        "http://localhost",
        "http://127.0.0.1.example.invalid",
    ] {
        store
            .update(|c| c.providers[0].base_url = forbidden.into())
            .unwrap();
        let reply = client.post(&url).json(&body).send().await.unwrap();
        assert_eq!(reply.status(), 502);
        assert!(reply
            .text()
            .await
            .unwrap()
            .contains("only literal loopback"));
    }
    gateway.stop().await;
    upstream.abort();
}
