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
