//! DSH-shaped requests at the listening gateway; no DSH runtime or real accounts.
use crate::config::ConfigStore;
use axum::{http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn dsh_unmapped_reasoning_option_is_rejected_before_fixed_source_dispatch() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let records = received.clone();
    let upstream = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/messages", post(move |_headers: HeaderMap, Json(body): Json<Value>| {
            let records = records.clone();
            async move {
                records.lock().unwrap().push(body);
                Json(json!({"id":"fictional","type":"message","role":"assistant","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{}}))
            }
        }))).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(root.path().join("dsh.db")).unwrap());
    store
        .update(|c| {
            c.port = 0;
            c.providers.truncate(1);
            c.models.truncate(1);
            c.gateway.proxy_mode = "direct".into();
            c.providers[0].base_url = format!("http://{address}");
            c.providers[0].api_type = "messages".into();
            c.models[0].api_type = "messages".into();
            c.models[0].model_id = "same-model".into();
        })
        .unwrap();
    let config = store.read();
    store
        .write_secret(
            &format!("provider:{}", config.providers[0].id),
            "fictional-dsh-only",
        )
        .unwrap();
    let gateway = crate::proxy::start(store).await.unwrap();
    let response = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":format!("autojev/model/{}", config.models[0].id),"messages":[{"role":"user","content":"fictional reasoning request"}],"reasoning_effort":"high"}))
        .send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.text().await.unwrap().contains("reasoning_effort"));
    assert!(
        received.lock().unwrap().is_empty(),
        "Unsupported semantics must never reach any source"
    );
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn dsh_subscription_text_stream_and_tools_share_identity_and_entitlement_denials() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = received.clone();
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/chat/completions",
                post(move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({"paid":true}))
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(root.path().join("dsh-denied.db")).unwrap());
    let manager = crate::cpa_sources::Manager::default();
    let id = manager
        .create(&store, "codex", "Fictional subscription A")
        .unwrap();
    store
        .update(|c| {
            c.port = 0;
            c.gateway.proxy_mode = "direct".into();
            c.providers[0].base_url = format!("http://{address}");
            c.models[0].model_id = "same-model".into();
            let mut model = c.models[0].clone();
            model.id = "fictional-subscription-uuid".into();
            model.provider_id = id.clone();
            model.supports_tools = false;
            let connection = c.cpa_subscriptions.get_mut(&id).unwrap();
            connection.stage = crate::cpa_sources::Stage::Connected;
            connection.identity.generation = 1;
            connection.identity.identity = Some("fictional-account-a".into());
            connection.plan = Some("fictional-plan".into());
            c.cpa_model_bindings.insert(
                model.id.clone(),
                crate::cpa_sources::Binding {
                    identity: crate::cpa_sources::IdentityStamp {
                        provider_id: id.clone(),
                        connection_instance_id: connection.identity.connection_instance_id.clone(),
                        generation: 1,
                        account: connection.identity.identity.clone(),
                        plan: connection.plan.clone(),
                    },
                    model_id: model.model_id.clone(),
                },
            );
            c.models.push(model);
        })
        .unwrap();
    let original = store.read();
    store
        .write_secret(
            &format!("provider:{}", original.providers[0].id),
            "fictional-paid-fallback",
        )
        .unwrap();
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    for (state, expected) in [
        ("connected", "cpa_qualification_unknown"),
        ("disconnected", "cpa_not_connected"),
        ("account_changed", "cpa_target_identity_changed"),
        ("unbound", "cpa_target_identity_changed"),
        ("disabled", "provider_disabled"),
    ] {
        store
            .update(|c| {
                *c = original.clone();
                match state {
                    "disconnected" => {
                        c.cpa_subscriptions.get_mut(&id).unwrap().stage =
                            crate::cpa_sources::Stage::Idle
                    }
                    "account_changed" => {
                        c.cpa_subscriptions
                            .get_mut(&id)
                            .unwrap()
                            .identity
                            .generation += 1
                    }
                    "unbound" => {
                        c.cpa_model_bindings.clear();
                    }
                    "disabled" => {
                        c.providers.iter_mut().find(|p| p.id == id).unwrap().enabled = false
                    }
                    _ => {}
                }
            })
            .unwrap();
        for shape in ["text", "stream", "tools"] {
            let mut body = json!({"model":"autojev/model/fictional-subscription-uuid","messages":[{"role":"user","content":"fictional DSH input"}],"stream":shape == "stream"});
            if shape == "tools" {
                body["tools"] = json!([{"type":"function","function":{"name":"client_echo","parameters":{"type":"object","properties":{}}}}]);
            }
            let reply = reqwest::Client::new()
                .post(format!(
                    "http://127.0.0.1:{}/v1/chat/completions",
                    gateway.port
                ))
                .json(&body)
                .send()
                .await
                .unwrap();
            assert!(!reply.status().is_success(), "{state}/{shape}");
            assert_eq!(
                reply.json::<Value>().await.unwrap()["error"]["code"],
                expected,
                "{state}/{shape}"
            );
            assert_eq!(
                received.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "No same-name paid fallback for {state}/{shape}"
            );
        }
    }
    gateway.stop().await;
    upstream.abort();
}
