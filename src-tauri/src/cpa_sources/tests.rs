use super::*;
use axum::{
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;

struct AccountFixture {
    base: String,
    account: Arc<std::sync::Mutex<Option<&'static str>>>,
    fail: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    delayed: Arc<std::sync::atomic::AtomicBool>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    cleanup_fail: Arc<std::sync::atomic::AtomicBool>,
    auth_delayed: Arc<std::sync::atomic::AtomicBool>,
    server: tokio::task::JoinHandle<()>,
}
impl AccountFixture {
    async fn start() -> Self {
        let account = Arc::new(std::sync::Mutex::new(None));
        let active = account.clone();
        let signed_in = account.clone();
        let deleted = account.clone();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let cancellation = cancelled.clone();
        let cleanup_fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cleanup_failed = cleanup_fail.clone();
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let failed = fail.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let arrival = entered.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        let unblock = release.clone();
        let delayed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let delay = delayed.clone();
        let auth_delayed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let auth_delay = auth_delayed.clone();
        let auth_entered = entered.clone();
        let auth_release = release.clone();
        let router=Router::new()
            .route("/v8/management/oauth/auth-url",get(move || {let delay=auth_delay.clone();let entered=auth_entered.clone();let release=auth_release.clone();async move {if delay.load(std::sync::atomic::Ordering::SeqCst) {entered.notify_one();release.notified().await;} Json(json!({"state":"fictional-session","url":"https://auth.example.invalid/authorize"}))}}))
            .route("/v8/management/oauth/status",get(move || {let active=signed_in.clone();async move {*active.lock().unwrap()=Some("account-a");Json(json!({"status":"ok"}))}}))
            .route("/v8/management/credentials",get(move || {let active=active.clone();async move {Json(json!({"files":active.lock().unwrap().map(|account|json!({"name":"owned.json","provider":"codex","id_token":{"chatgpt_account_id":account,"plan_type":"plan-a"}})).into_iter().collect::<Vec<_>>()}))}}).delete(move ||{let deleted=deleted.clone();let failed=cleanup_failed.clone();async move {if failed.load(std::sync::atomic::Ordering::SeqCst) {return (axum::http::StatusCode::BAD_GATEWAY,Json(json!({})));} *deleted.lock().unwrap()=None;(axum::http::StatusCode::OK,Json(json!({"status":"ok"})))}}))
            .route("/v8/management/oauth/session",delete(move ||{let cancelled=cancellation.clone();async move {Json(json!({"status":"ok","cancelled":cancelled.load(std::sync::atomic::Ordering::SeqCst)}))}}))
            .route("/v8/management/credentials/models",get(move || {let failed=failed.clone();let delay=delay.clone();let arrival=arrival.clone();let unblock=unblock.clone();async move {
                if failed.load(std::sync::atomic::Ordering::SeqCst) {return (axum::http::StatusCode::BAD_GATEWAY,Json(json!({})));}
                if delay.load(std::sync::atomic::Ordering::SeqCst) {arrival.notify_one();unblock.notified().await;}
                (axum::http::StatusCode::OK,Json(json!({"models":[{"id":"same-model"}]})))
            }}))
            .layer(axum::middleware::from_fn(|req,next:axum::middleware::Next|async move {let mut response=next.run(req).await;response.headers_mut().insert("x-cpa-commit","e2bff0107bb307337aaa19018ccddd55f64253d5".parse().unwrap());response}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base,
            account,
            fail,
            entered,
            release,
            delayed,
            cancelled,
            cleanup_fail,
            auth_delayed,
            server,
        }
    }
}
impl Drop for AccountFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn failed_reads_keep_history_but_changed_accounts_require_explicit_rebinding() {
    let fixture = AccountFixture::start().await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.db");
    let store = ConfigStore::load(path.clone()).unwrap();
    let manager = Manager::owned_fixture(fixture.base.clone()).unwrap();
    let id = manager
        .create(&store, "codex", "History connection")
        .unwrap();
    manager.begin(&store, &id).await.unwrap();
    manager.poll(&store, &id).await.unwrap();
    manager.refresh(&store, &id).await.unwrap();
    let model_id = manager.views(&store.read())[0].models[0].id.clone();
    let discovered = store
        .read()
        .models
        .into_iter()
        .find(|m| m.id == model_id)
        .unwrap();
    assert_eq!(
        discovered.context_window, 0,
        "Discovery must not invent a context limit"
    );
    assert_eq!(discovered.input_cost_per_million, 0.0);
    assert!(
        !discovered.supports_tools && !discovered.supports_vision && !discovered.supports_reasoning
    );
    manager.select(&store, &id, &model_id, true).unwrap();
    fixture
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(manager.refresh(&store, &id).await.is_err());
    let historical = manager.views(&store.read()).remove(0);
    assert_eq!(historical.catalog_state, EvidenceState::Stale);
    assert_eq!(historical.account.as_deref(), Some("account-a"));
    assert!(historical.models[0].selected && historical.models[0].bound);
    fixture
        .fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    *fixture.account.lock().unwrap() = Some("account-b");
    manager.refresh(&store, &id).await.unwrap();
    let changed = manager.views(&store.read()).remove(0);
    assert_eq!(changed.account.as_deref(), Some("account-b"));
    assert!(changed.generation > historical.generation);
    assert!(changed.models[0].selected && !changed.models[0].bound);
    assert_eq!(changed.models[0].id, model_id);
    let config = store.read();
    let model = config.models.iter().find(|m| m.id == model_id).unwrap();
    let provider = config.providers.iter().find(|p| p.id == id).unwrap();
    assert_eq!(
        crate::subscription::admit_model(&config, model, provider, crate::protocol::Protocol::Chat)
            .unwrap_err()
            .code,
        "cpa_target_identity_changed"
    );
    let reopened = ConfigStore::load(path).unwrap();
    assert!(
        !reopened
            .read()
            .models
            .iter()
            .find(|m| m.id == model_id)
            .unwrap()
            .supports_tools,
        "Reload must not invent a CPA protocol capability"
    );
    assert_eq!(
        Manager::default().views(&reopened.read())[0].models[0].id,
        model_id
    );
    manager.select(&store, &id, &model_id, true).unwrap();
    assert!(manager.views(&store.read())[0].models[0].bound);
}

#[tokio::test]
async fn completed_authorization_cancel_can_retry_only_its_owned_cleanup() {
    let fixture = AccountFixture::start().await;
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::load(temp.path().join("completed-cancel.db")).unwrap();
    let manager = Manager::owned_fixture(fixture.base.clone()).unwrap();
    let id = manager
        .create(&store, "codex", "Completed cancellation")
        .unwrap();
    manager.begin(&store, &id).await.unwrap();
    // CPA completed and stored this dedicated flow before the desktop polled it.
    *fixture.account.lock().unwrap() = Some("account-a");
    fixture
        .cancelled
        .store(false, std::sync::atomic::Ordering::SeqCst);
    fixture
        .cleanup_fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(manager.disconnect(&store, &id, true).await.is_err());
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.stage, Stage::Cancelled);
    assert!(view.account.is_none() && view.authorization_url.is_none());
    assert_eq!(
        store.read().cpa_subscriptions[&id]
            .credential_ref
            .as_deref(),
        Some("owned.json")
    );
    assert!(manager.begin(&store, &id).await.is_err());
    fixture
        .cleanup_fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    manager.disconnect(&store, &id, false).await.unwrap();
    assert!(fixture.account.lock().unwrap().is_none());
    manager.begin(&store, &id).await.unwrap();
    assert_eq!(manager.views(&store.read())[0].stage, Stage::Waiting);
    assert!(
        manager.remove(&store, &id).is_err(),
        "Pending owned tasks must block connection deletion"
    );
    manager.disconnect(&store, &id, true).await.unwrap();
    manager.remove(&store, &id).unwrap();
    assert!(manager.views(&store.read()).is_empty());
    assert!(!store
        .read()
        .providers
        .iter()
        .any(|provider| provider.id == id));
    let replacement = manager
        .create(&store, "codex", "Replacement connection")
        .unwrap();
    assert!(
        manager
            .views(&store.read())
            .iter()
            .find(|c| c.provider_id == replacement)
            .unwrap()
            .service_available
    );
}

#[tokio::test]
async fn cancelling_a_delayed_start_blocks_reuse_until_owned_cleanup_finishes() {
    let fixture = AccountFixture::start().await;
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("delayed-start.db")).unwrap());
    let manager = Arc::new(Manager::owned_fixture(fixture.base.clone()).unwrap());
    let id = manager.create(&store, "codex", "Delayed start").unwrap();
    fixture
        .auth_delayed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let starting = {
        let manager = manager.clone();
        let store = store.clone();
        let id = id.clone();
        tokio::spawn(async move { manager.begin(&store, &id).await })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.entered.notified(),
    )
    .await
    .unwrap();
    manager.disconnect(&store, &id, true).await.unwrap();
    let blocked = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        manager.begin(&store, &id),
    )
    .await;
    assert!(
        matches!(blocked, Ok(Err(_))),
        "A pending owned auth-url request must block another start immediately"
    );
    *fixture.account.lock().unwrap() = Some("account-a");
    fixture
        .cancelled
        .store(false, std::sync::atomic::Ordering::SeqCst);
    fixture
        .auth_delayed
        .store(false, std::sync::atomic::Ordering::SeqCst);
    fixture.release.notify_one();
    assert!(starting.await.unwrap().is_err());
    assert!(
        fixture.account.lock().unwrap().is_none(),
        "The superseded completed flow must clean only its own profile credential"
    );
    assert_eq!(manager.views(&store.read())[0].stage, Stage::Cancelled);
    manager.begin(&store, &id).await.unwrap();
}

#[tokio::test]
async fn an_old_directory_cannot_pollute_a_switched_account() {
    let fixture = AccountFixture::start().await;
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("late-directory.db")).unwrap());
    let manager = Arc::new(Manager::owned_fixture(fixture.base.clone()).unwrap());
    let id = manager
        .create(&store, "codex", "Switched connection")
        .unwrap();
    manager.begin(&store, &id).await.unwrap();
    manager.poll(&store, &id).await.unwrap();
    fixture
        .delayed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let old = {
        let manager = manager.clone();
        let store = store.clone();
        let id = id.clone();
        tokio::spawn(async move { manager.refresh(&store, &id).await })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.entered.notified(),
    )
    .await
    .unwrap();
    manager.disconnect(&store, &id, false).await.unwrap();
    manager.begin(&store, &id).await.unwrap();
    manager.poll(&store, &id).await.unwrap();
    *fixture.account.lock().unwrap() = Some("account-b");
    fixture
        .delayed
        .store(false, std::sync::atomic::Ordering::SeqCst);
    manager.refresh(&store, &id).await.unwrap();
    let before = serde_json::to_value(manager.views(&store.read())).unwrap();
    fixture.release.notify_one();
    assert!(old.await.unwrap().is_err());
    assert_eq!(
        serde_json::to_value(manager.views(&store.read())).unwrap(),
        before
    );
    assert_eq!(
        manager.views(&store.read())[0].account.as_deref(),
        Some("account-b")
    );
}

#[tokio::test]
async fn observed_account_change_invalidates_the_old_binding_even_if_directory_fails() {
    let fixture = AccountFixture::start().await;
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::load(temp.path().join("changed-failed.db")).unwrap();
    let manager = Manager::owned_fixture(fixture.base.clone()).unwrap();
    let id = manager.create(&store, "codex", "Changed identity").unwrap();
    manager.begin(&store, &id).await.unwrap();
    manager.poll(&store, &id).await.unwrap();
    manager.refresh(&store, &id).await.unwrap();
    let old = manager.views(&store.read()).remove(0);
    manager
        .select(&store, &id, &old.models[0].id, true)
        .unwrap();
    *fixture.account.lock().unwrap() = Some("account-b");
    fixture
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(manager.refresh(&store, &id).await.is_err());
    let changed = manager.views(&store.read()).remove(0);
    assert_eq!(changed.account.as_deref(), Some("account-b"));
    assert!(changed.generation > old.generation && !changed.models[0].bound);
    assert_eq!(changed.catalog_state, EvidenceState::Stale);
}

#[tokio::test]
async fn process_restart_invalidates_a_pending_cpa_identity_generation() {
    let fixture = AccountFixture::start().await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("restart.db");
    let store = ConfigStore::load(path.clone()).unwrap();
    let manager = Manager::owned_fixture(fixture.base.clone()).unwrap();
    let id = manager
        .create(&store, "codex", "Pending connection")
        .unwrap();
    manager.begin(&store, &id).await.unwrap();
    let pending = manager.views(&store.read()).remove(0);
    let reopened = ConfigStore::load(path).unwrap();
    let view = Manager::default().views(&reopened.read()).remove(0);
    assert_eq!(view.stage, Stage::Disconnected);
    assert!(view.generation > pending.generation);
    assert!(!view.service_available && view.authorization_url.is_none());
}

#[tokio::test]
async fn connected_directory_and_selection_do_not_authorize_generation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let files = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let read_files = files.clone();
    let logged_in = files.clone();
    let paid_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let received = paid_calls.clone();
    let app = Router::new()
        .route("/v1/chat/completions",post(move || {let received=received.clone();async move {received.fetch_add(1,std::sync::atomic::Ordering::SeqCst);Json(json!({"paid":true}))}}))
        .route("/v8/management/oauth/auth-url", get(|| async { Json(json!({"status":"ok","state":"fictional-attempt","url":"https://auth.example.invalid/authorize"})) }))
        .route("/v8/management/oauth/status", get(move || { let files = logged_in.clone(); async move { files.store(true, std::sync::atomic::Ordering::SeqCst); Json(json!({"status":"ok"})) } }))
        .route("/v8/management/credentials", get(move || { let files = read_files.clone(); async move { Json(json!({"files":if files.load(std::sync::atomic::Ordering::SeqCst) { vec![json!({"name":"fictional-codex.json","provider":"codex","account":"fictional-a","id_token":{"chatgpt_account_id":"fictional-a","plan_type":"fictional-plan"},"access_token":"must-not-leak"})] } else {vec![]} })) } }))
        .route("/v8/management/credentials/models", get(|| async { Json(json!({"models":[{"id":"same-model","display_name":"Same model"}]})) }))
        .route("/v8/management/oauth/session", delete(|| async { Json(json!({"status":"ok","cancelled":true})) }))
        .layer(axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
            let mut response = next.run(req).await;
            response.headers_mut().insert("x-cpa-commit", "e2bff0107bb307337aaa19018ccddd55f64253d5".parse().unwrap());
            response
        }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("cpa.db")).unwrap());
    store
        .update(|c| {
            c.port = 0;
            c.gateway.proxy_mode = "direct".into();
            c.providers[0].base_url = base.clone();
            c.models[0].model_id = "same-model".into();
        })
        .unwrap();
    let manager = Manager::owned_fixture(base).unwrap();
    let id = manager
        .create(&store, "codex", "Work subscription")
        .unwrap();
    manager.begin(&store, &id).await.unwrap();
    assert_eq!(manager.views(&store.read())[0].stage, Stage::Waiting);
    manager.poll(&store, &id).await.unwrap();
    manager.refresh(&store, &id).await.unwrap();
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.stage, Stage::Connected);
    assert_eq!(view.account.as_deref(), Some("fictional-a"));
    assert_eq!(view.plan.as_deref(), Some("fictional-plan"));
    let model = &store.read().models.last().unwrap().clone();
    assert!(!model.selected);
    manager.select(&store, &id, &model.id, true).unwrap();
    let config = store.read();
    let provider = config.providers.iter().find(|p| p.id == id).unwrap();
    let denial =
        crate::subscription::admit_model(&config, model, provider, crate::protocol::Protocol::Chat)
            .unwrap_err();
    assert_eq!(denial.code, "cpa_qualification_unknown");
    let projection = serde_json::to_string(&view).unwrap();
    assert!(!projection.contains("must-not-leak"));
    assert!(!projection.contains("fictional-management-r3"));
    assert!(!projection.contains("fictional-attempt"));
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let reply=reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions",gateway.port)).json(&json!({"model":format!("autojev/model/{}",model.id),"messages":[{"role":"user","content":"fictional input"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 429);
    assert_eq!(
        reply.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "cpa_qualification_unknown"
    );
    assert_eq!(
        paid_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "Same-name API must not receive this fixed subscription request"
    );
    gateway.stop().await;
    server.abort();
}

#[tokio::test]
async fn cancel_discards_a_late_success_and_only_cancels_its_owned_session() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let arrived = entered.clone();
    let unblock = release.clone();
    let cancels = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let recorded = cancels.clone();
    let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let completed_status = completed.clone();
    let file_state = completed.clone();
    let router=Router::new()
        .route("/v8/management/oauth/auth-url",get(||async {Json(json!({"status":"ok","state":"owned-old-session","url":"https://auth.example.invalid/authorize"}))}))
        .route("/v8/management/oauth/status",get(move || {let entered=arrived.clone();let release=unblock.clone();let completed=completed_status.clone();async move {entered.notify_one();release.notified().await;completed.store(true,std::sync::atomic::Ordering::SeqCst);Json(json!({"status":"ok"}))}}))
        .route("/v8/management/credentials",get(move || {let completed=file_state.clone();async move {Json(json!({"files":if completed.load(std::sync::atomic::Ordering::SeqCst) {vec![json!({"name":"late.json","provider":"codex","id_token":{"chatgpt_account_id":"old-a","plan_type":"old-plan"}})]} else {vec![]}}))}}))
        .route("/v8/management/oauth/session",delete(move |axum::extract::Query(query):axum::extract::Query<std::collections::HashMap<String,String>>| {let recorded=recorded.clone();async move {recorded.lock().unwrap().push(query["state"].clone());Json(json!({"status":"ok","cancelled":true}))}}))
        .layer(axum::middleware::from_fn(|req,next:axum::middleware::Next|async move {let mut response=next.run(req).await;response.headers_mut().insert("x-cpa-commit","e2bff0107bb307337aaa19018ccddd55f64253d5".parse().unwrap());response}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("race.db")).unwrap());
    let manager = Arc::new(Manager::owned_fixture(base).unwrap());
    let id = manager.create(&store, "codex", "Race connection").unwrap();
    manager.begin(&store, &id).await.unwrap();
    let old_generation = manager.views(&store.read())[0].generation;
    let operation = {
        let store = store.clone();
        let manager = manager.clone();
        let id = id.clone();
        tokio::spawn(async move { manager.poll(&store, &id).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    manager.disconnect(&store, &id, true).await.unwrap();
    release.notify_one();
    assert!(operation
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("late result discarded"));
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.stage, Stage::Cancelled);
    assert!(view.generation > old_generation);
    assert!(view.account.is_none() && view.plan.is_none() && view.authorization_url.is_none());
    assert_eq!(*cancels.lock().unwrap(), vec!["owned-old-session"]);
    assert!(
        manager.begin(&store, &id).await.is_err(),
        "Never adopt a credential saved after a cancelled flow"
    );
    server.abort();
}
