use super::*;
use axum::{
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn cpa_view_projects_saved_connection_name_without_changing_fixed_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::load(temp.path().join("named-view.db")).unwrap();
    let manager = Manager::default();
    let id = manager.create(&store, "codex", "团队 Codex").unwrap();
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.provider_id, id);
    assert_eq!(view.provider, "codex");
    assert_eq!(view.connection_name, "团队 Codex");
    let value = serde_json::to_value(view).unwrap();
    assert_eq!(value["connection_name"], "团队 Codex");
    assert_eq!(value["provider_id"], id);
    assert!(value.get("credential_ref").is_none());
}

struct AccountFixture {
    base: String,
    requests: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    account: Arc<std::sync::Mutex<Option<&'static str>>>,
    fail: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    delayed: Arc<std::sync::atomic::AtomicBool>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    cleanup_fail: Arc<std::sync::atomic::AtomicBool>,
    auth_delayed: Arc<std::sync::atomic::AtomicBool>,
    status_failed: Arc<std::sync::atomic::AtomicBool>,
    status_waiting: Arc<std::sync::atomic::AtomicBool>,
    status_delayed: Arc<std::sync::atomic::AtomicBool>,
    credentials_failed: Arc<std::sync::atomic::AtomicBool>,
    cancel_failed: Arc<std::sync::atomic::AtomicBool>,
    cancel_delayed: Arc<std::sync::atomic::AtomicBool>,
    cancel_entered: Arc<tokio::sync::Notify>,
    cancel_release: Arc<tokio::sync::Notify>,
    server: tokio::task::JoinHandle<()>,
}
impl AccountFixture {
    async fn paused_poll(&self, store: Arc<ConfigStore>, manager: Arc<Manager>, id: String)
        -> tokio::task::JoinHandle<Result<()>> {
        self.status_delayed.store(true, std::sync::atomic::Ordering::SeqCst);
        let operation = tokio::spawn(async move { manager.poll(&store, &id).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), self.entered.notified()).await.unwrap();
        operation
    }
    async fn start() -> Self {
        let account = Arc::new(std::sync::Mutex::new(None));
        let active = account.clone();
        let signed_in = account.clone();
        let status_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let expired = status_failed.clone();
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
        let status_waiting = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiting = status_waiting.clone();
        let status_delayed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let status_delay = status_delayed.clone();
        let status_entered = entered.clone();
        let status_release = release.clone();
        let credentials_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let credentials_error = credentials_failed.clone();
        let cancel_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_error = cancel_failed.clone();
        let cancel_delayed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_delay = cancel_delayed.clone();
        let cancel_entered = Arc::new(tokio::sync::Notify::new());
        let cancel_arrived = cancel_entered.clone();
        let cancel_release = Arc::new(tokio::sync::Notify::new());
        let cancel_unblock = cancel_release.clone();
        let router=Router::new()
            .route("/v8/management/oauth/auth-url",get(move || {let delay=auth_delay.clone();let entered=auth_entered.clone();let release=auth_release.clone();async move {if delay.load(std::sync::atomic::Ordering::SeqCst) {entered.notify_one();release.notified().await;} Json(json!({"state":"fictional-session","url":"https://auth.example.invalid/authorize"}))}}))
            .route("/v8/management/oauth/status",get(move || {let active=signed_in.clone();let expired=expired.clone();let waiting=waiting.clone();let delay=status_delay.clone();let entered=status_entered.clone();let release=status_release.clone();async move {if delay.load(std::sync::atomic::Ordering::SeqCst) {entered.notify_one();release.notified().await;} if waiting.load(std::sync::atomic::Ordering::SeqCst) {return Json(json!({"status":"wait"}));} if expired.load(std::sync::atomic::Ordering::SeqCst) {return Json(json!({"status":"error","error":"unknown or expired state"}));} *active.lock().unwrap()=Some("account-a");Json(json!({"status":"ok"}))}}))
            .route("/v8/management/credentials",get(move || {let active=active.clone();let failed=credentials_error.clone();async move {if failed.load(std::sync::atomic::Ordering::SeqCst) {return (axum::http::StatusCode::BAD_GATEWAY,Json(json!({})));} (axum::http::StatusCode::OK,Json(json!({"files":active.lock().unwrap().map(|account|json!({"name":"owned.json","provider":"codex","id_token":{"chatgpt_account_id":account,"plan_type":"plan-a"}})).into_iter().collect::<Vec<_>>()})))}}).delete(move ||{let deleted=deleted.clone();let failed=cleanup_failed.clone();async move {if failed.load(std::sync::atomic::Ordering::SeqCst) {return (axum::http::StatusCode::BAD_GATEWAY,Json(json!({})));} *deleted.lock().unwrap()=None;(axum::http::StatusCode::OK,Json(json!({"status":"ok"})))}}))
            .route("/v8/management/oauth/session",delete(move ||{let cancelled=cancellation.clone();let failed=cancel_error.clone();let delay=cancel_delay.clone();let entered=cancel_arrived.clone();let release=cancel_unblock.clone();async move {if delay.load(std::sync::atomic::Ordering::SeqCst) {entered.notify_one();release.notified().await;} if failed.load(std::sync::atomic::Ordering::SeqCst) {return (axum::http::StatusCode::BAD_GATEWAY,Json(json!({})));} (axum::http::StatusCode::OK,Json(json!({"status":"ok","cancelled":cancelled.load(std::sync::atomic::Ordering::SeqCst)})))}}))
            .route("/v8/management/credentials/models",get(move || {let failed=failed.clone();let delay=delay.clone();let arrival=arrival.clone();let unblock=unblock.clone();async move {
                if failed.load(std::sync::atomic::Ordering::SeqCst) {return (axum::http::StatusCode::BAD_GATEWAY,Json(json!({})));}
                if delay.load(std::sync::atomic::Ordering::SeqCst) {arrival.notify_one();unblock.notified().await;}
                (axum::http::StatusCode::OK,Json(json!({"models":[{"id":"same-model"}]})))
            }}))
            .layer(axum::middleware::from_fn(|req,next:axum::middleware::Next|async move {let mut response=next.run(req).await;response.headers_mut().insert("x-cpa-commit","e2bff0107bb307337aaa19018ccddd55f64253d5".parse().unwrap());response}));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let router = router.layer(axum::middleware::from_fn(move |request: axum::extract::Request, next: axum::middleware::Next| {
            let recorded = recorded.clone();
            async move {
                recorded.lock().unwrap().push((request.method().to_string(), request.uri().path().to_string()));
                next.run(request).await
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base,
            requests,
            account,
            fail,
            entered,
            release,
            delayed,
            cancelled,
            cleanup_fail,
            auth_delayed,
            status_failed,
            status_waiting,
            status_delayed,
            credentials_failed,
            cancel_failed,
            cancel_delayed,
            cancel_entered,
            cancel_release,
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
async fn an_expired_unclaimed_flow_can_be_removed_without_adopting_or_deleting_credentials() {
    for check_first in [false, true] {
        let fixture = AccountFixture::start().await;
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::load(temp.path().join("expired-unclaimed.db")).unwrap();
        let manager = Manager::owned_fixture(fixture.base.clone()).unwrap();
        let id = manager.create(&store, "codex", "Expired authorization").unwrap();
        manager.begin(&store, &id).await.unwrap();
        *fixture.account.lock().unwrap() = Some("unclaimed-account");
        fixture.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
        fixture.status_failed.store(true, std::sync::atomic::Ordering::SeqCst);
        if check_first {
            assert!(manager.poll(&store, &id).await.is_err());
        }
        manager.disconnect(&store, &id, !check_first).await.unwrap();
        let view = manager.views(&store.read()).remove(0);
        assert!(view.account.is_none() && view.plan.is_none());
        assert!(view.authorization_url.is_none() && !view.service_available);
        assert!(view.error.as_deref().unwrap().contains("isolated"));
        assert_eq!(*fixture.account.lock().unwrap(), Some("unclaimed-account"));
        assert!(manager.cleanup_finished(&id));
        assert!(manager.begin(&store, &id).await.is_err());
        manager.remove(&store, &id).unwrap();
        let replacement = manager.create(&store, "codex", "Fresh service required").unwrap();
        assert!(!manager.views(&store.read()).iter()
            .find(|c| c.provider_id == replacement).unwrap().service_available);
    }
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
    // Fixed CPA expires completed sessions after one minute. The cleanup reference
    // was already proven and persisted; a later retry must not need that session.
    fixture
        .status_failed
        .store(true, std::sync::atomic::Ordering::SeqCst);
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
    assert!(
        !manager.cleanup_finished(&id),
        "A pending poll must retain cleanup ownership after cancellation"
    );
    assert!(manager.remove(&store, &id).is_err());
    assert!(manager.begin(&store, &id).await.is_err());
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
    assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        !view.service_available,
        "A late credential must isolate the old service profile without restoring identity"
    );
    assert!(
        manager.begin(&store, &id).await.is_err(),
        "Never adopt a credential saved after a cancelled flow"
    );
    assert!(manager.cleanup_finished(&id));
    manager.remove(&store, &id).unwrap();
    let replacement = manager.create(&store, "codex", "New connection").unwrap();
    assert!(!manager.views(&store.read())[0].service_available);
    assert!(manager.begin(&store, &replacement).await.is_err());
    server.abort();
}

#[tokio::test]
async fn successful_cancel_with_a_late_waiting_poll_keeps_the_profile_retryable() {
    let fixture = AccountFixture::start().await;
    fixture.status_waiting.store(true, std::sync::atomic::Ordering::SeqCst);
    fixture.status_delayed.store(true, std::sync::atomic::Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("late-wait.db")).unwrap());
    let manager = Arc::new(Manager::owned_fixture(fixture.base.clone()).unwrap());
    let id = manager.create(&store, "codex", "Retryable cancellation").unwrap();
    manager.begin(&store, &id).await.unwrap();
    let operation = fixture.paused_poll(store.clone(), manager.clone(), id.clone()).await;
    manager.disconnect(&store, &id, true).await.unwrap();
    assert!(!manager.cleanup_finished(&id));
    assert!(manager.begin(&store, &id).await.is_err());
    assert!(manager.remove(&store, &id).is_err());
    fixture.release.notify_one();
    operation.await.unwrap().unwrap();
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.stage, Stage::Cancelled);
    assert!(view.account.is_none() && view.plan.is_none() && view.authorization_url.is_none());
    assert!(view.service_available, "Successful cancellation and late Waiting must preserve an empty profile");
    assert!(manager.cleanup_finished(&id));
    manager.begin(&store, &id).await.unwrap();
    assert_eq!(manager.views(&store.read())[0].stage, Stage::Waiting);
    manager.disconnect(&store, &id, true).await.unwrap();
}

#[tokio::test]
async fn a_cancelled_poll_with_completion_or_unproven_residue_isolates_the_profile() {
    for case in ["not_cancelled", "residue", "expired", "complete", "unreadable"] {
        let fixture = AccountFixture::start().await;
        fixture.status_waiting.store(!matches!(case, "expired" | "complete"), std::sync::atomic::Ordering::SeqCst);
        fixture.status_failed.store(case == "expired", std::sync::atomic::Ordering::SeqCst);
        fixture.cancelled.store(!matches!(case, "not_cancelled" | "complete"), std::sync::atomic::Ordering::SeqCst);
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(temp.path().join("uncertain.db")).unwrap());
        let manager = Arc::new(Manager::owned_fixture(fixture.base.clone()).unwrap());
        let id = manager.create(&store, "codex", "Uncertain cancellation").unwrap();
        manager.begin(&store, &id).await.unwrap();
        let operation = fixture.paused_poll(store.clone(), manager.clone(), id.clone()).await;
        manager.disconnect(&store, &id, true).await.unwrap();
        if case == "residue" { *fixture.account.lock().unwrap() = Some("unclaimed"); }
        fixture.credentials_failed.store(case == "unreadable", std::sync::atomic::Ordering::SeqCst);
        fixture.release.notify_one();
        assert_eq!(operation.await.unwrap().is_err(), matches!(case, "expired" | "complete"), "{case}");
        let view = manager.views(&store.read()).remove(0);
        assert_eq!(view.stage, Stage::Cancelled, "{case}");
        assert!(view.account.is_none() && view.plan.is_none() && !view.service_available, "{case}");
        assert!(manager.cleanup_finished(&id), "{case}");
        assert!(manager.begin(&store, &id).await.is_err(), "{case}");
        if case == "residue" { assert_eq!(*fixture.account.lock().unwrap(), Some("unclaimed")); }
        manager.remove(&store, &id).unwrap();
        let new_id = manager.create(&store, "codex", "Replacement").unwrap();
        assert!(manager.begin(&store, &new_id).await.is_err(), "{case}");
    }
}

#[tokio::test]
async fn a_waiting_poll_that_settles_before_cancel_ack_keeps_ownership_until_ack() {
    let fixture = AccountFixture::start().await;
    fixture.status_waiting.store(true, std::sync::atomic::Ordering::SeqCst);
    fixture.cancel_delayed.store(true, std::sync::atomic::Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("cancel-later.db")).unwrap());
    let manager = Arc::new(Manager::owned_fixture(fixture.base.clone()).unwrap());
    let id = manager.create(&store, "codex", "Cancel response later").unwrap();
    manager.begin(&store, &id).await.unwrap();
    let operation = fixture.paused_poll(store.clone(), manager.clone(), id.clone()).await;
    let cancellation = {
        let store = store.clone(); let manager = manager.clone(); let id = id.clone();
        tokio::spawn(async move { manager.disconnect(&store, &id, true).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), fixture.cancel_entered.notified()).await.unwrap();
    fixture.release.notify_one();
    operation.await.unwrap().unwrap();
    assert!(manager.views(&store.read())[0].service_available);
    assert!(!manager.cleanup_finished(&id));
    assert!(manager.begin(&store, &id).await.is_err());
    assert!(manager.remove(&store, &id).is_err());
    fixture.cancel_release.notify_one();
    cancellation.await.unwrap().unwrap();
    assert!(manager.cleanup_finished(&id));
    assert!(manager.views(&store.read())[0].service_available);
    manager.begin(&store, &id).await.unwrap();
    fixture.cancel_delayed.store(false, std::sync::atomic::Ordering::SeqCst);
    manager.disconnect(&store, &id, true).await.unwrap();
}

#[tokio::test]
async fn a_failed_cancel_keeps_a_late_waiting_poll_owned_for_explicit_retry() {
    let fixture = AccountFixture::start().await;
    fixture.status_waiting.store(true, std::sync::atomic::Ordering::SeqCst);
    fixture.cancel_failed.store(true, std::sync::atomic::Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(temp.path().join("cancel-failed.db")).unwrap());
    let manager = Arc::new(Manager::owned_fixture(fixture.base.clone()).unwrap());
    let id = manager.create(&store, "codex", "Cancellation retry").unwrap();
    manager.begin(&store, &id).await.unwrap();
    let operation = fixture.paused_poll(store.clone(), manager.clone(), id.clone()).await;
    assert!(manager.disconnect(&store, &id, true).await.is_err());
    fixture.release.notify_one();
    operation.await.unwrap().unwrap();
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.stage, Stage::Cancelled);
    assert!(view.account.is_none() && view.plan.is_none() && view.service_available);
    assert!(!manager.cleanup_finished(&id));
    assert!(manager.begin(&store, &id).await.is_err());
    assert!(manager.remove(&store, &id).is_err());
    fixture.cancel_failed.store(false, std::sync::atomic::Ordering::SeqCst);
    manager.disconnect(&store, &id, false).await.unwrap();
    assert!(manager.cleanup_finished(&id));
    manager.begin(&store, &id).await.unwrap();
    manager.disconnect(&store, &id, true).await.unwrap();
}

#[test]
fn concurrent_begin_disconnect_and_shutdown_finish_within_a_wall_clock_deadline() {
    // The receiver lives outside Tokio: a blocking mutex cycle cannot stall its deadline.
    // Always detach runtime shutdown on failure so a blocked worker cannot hang this test.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4).enable_all().build().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let fixture = runtime.block_on(AccountFixture::start());
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ConfigStore::load(temp.path().join("concurrent.db")).unwrap());
        let manager = Arc::new(Manager::owned_fixture(fixture.base.clone()).unwrap());
        let id = manager.create(&store, "codex", "Concurrent lifecycle").unwrap();
        let (sent, completed) = std::sync::mpsc::channel();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        runtime.block_on(manager.begin(&store, &id)).unwrap();
        fixture.cancel_delayed.store(true, std::sync::atomic::Ordering::SeqCst);
        {
            let sent = sent.clone(); let store = store.clone();
            let manager = manager.clone(); let id = id.clone();
            runtime.spawn(async move {
                let _ = sent.send(("held_cleanup", manager.disconnect(&store, &id, false).await));
            });
        }
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), fixture.cancel_entered.notified()).await.unwrap();
        });
        {
            let sent = sent.clone(); let store = store.clone();
            let manager = manager.clone(); let id = id.clone();
            runtime.spawn(async move {
                let _ = sent.send(("during_cleanup", manager.begin(&store, &id).await));
            });
        }
        let (operation, outcome) = completed.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("Begin blocked while only the logical cleanup lease was alive");
        assert_eq!(operation, "during_cleanup");
        assert!(outcome.unwrap_err().to_string().contains("previous owned authorization/cleanup"));
        fixture.cancel_release.notify_one();
        let (operation, outcome) = completed.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("Acknowledged cleanup did not finish");
        assert_eq!(operation, "held_cleanup"); outcome.unwrap();
        fixture.cancel_delayed.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(manager.cleanup_finished(&id));
        for round in 0..32 {
            let gate = Arc::new(tokio::sync::Barrier::new(2));
            {
                let gate = gate.clone(); let sent = sent.clone();
                let store = store.clone(); let manager = manager.clone(); let id = id.clone();
                runtime.spawn(async move {
                    gate.wait().await;
                    let outcome = manager.begin(&store, &id).await;
                    let _ = sent.send(("begin", outcome));
                });
            }
            {
                let gate = gate.clone(); let sent = sent.clone();
                let store = store.clone(); let manager = manager.clone(); let id = id.clone();
                runtime.spawn(async move {
                    gate.wait().await;
                    let outcome = manager.disconnect(&store, &id, false).await;
                    let _ = sent.send(("disconnect", outcome));
                });
            }
            for _ in 0..2 {
                let (operation, outcome) = completed.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .unwrap_or_else(|_| panic!("Concurrent begin/disconnect did not settle by the wall-clock deadline, round {round}"));
                if operation == "disconnect" { outcome.unwrap(); }
                // An overlapping begin may be refused or superseded; it must still finish.
            }
            {
                let sent = sent.clone(); let store = store.clone();
                let manager = manager.clone(); let id = id.clone();
                runtime.spawn(async move {
                    let _ = sent.send(("cleanup", manager.disconnect(&store, &id, false).await));
                });
            }
            let (operation, outcome) = completed.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("Owned cleanup did not settle by the wall-clock deadline");
            assert_eq!(operation, "cleanup"); outcome.unwrap();
            assert!(manager.cleanup_finished(&id));
        }
        {
            let sent = sent.clone(); let store = store.clone();
            let manager = manager.clone(); let id = id.clone();
            runtime.spawn(async move {
                let outcome = async {
                    manager.begin(&store, &id).await?;
                    manager.shutdown(&store).await
                }.await;
                let _ = sent.send(("shutdown", outcome));
            });
        }
        let (operation, outcome) = completed.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("Pending authorization shutdown did not settle by the wall-clock deadline");
        assert_eq!(operation, "shutdown"); outcome.unwrap();
        assert!(manager.cleanup_finished(&id));
        let view = manager.views(&store.read()).remove(0);
        assert_eq!(view.stage, Stage::Disconnected);
        assert!(view.account.is_none() && view.plan.is_none() && view.authorization_url.is_none());
        assert!(view.service_available);
    }));
    runtime.shutdown_background();
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn preflight_foreign_credentials_retire_the_profile_without_adoption_or_deletion() {
    let fixture = AccountFixture::start().await;
    *fixture.account.lock().unwrap() = Some("foreign-account");
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::load(temp.path().join("foreign-preflight.db")).unwrap();
    let manager = Manager::owned_fixture(fixture.base.clone()).unwrap();
    let id = manager.create(&store, "codex", "Foreign preflight").unwrap();
    let error = manager.begin(&store, &id).await.unwrap_err();
    assert!(error.to_string().contains("unclaimed credential"));
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.stage, Stage::Failed);
    assert!(view.account.is_none() && view.plan.is_none() && view.authorization_url.is_none());
    assert!(store.read().cpa_subscriptions[&id].credential_ref.is_none());
    assert!(!view.service_available, "A rejected foreign profile must not be reused");
    assert!(manager.cleanup_finished(&id));
    assert!(manager.begin(&store, &id).await.is_err());
    manager.disconnect(&store, &id, false).await.unwrap();
    manager.remove(&store, &id).unwrap();
    let replacement = manager.create(&store, "codex", "Replacement").unwrap();
    assert!(!manager.views(&store.read())[0].service_available);
    assert!(manager.begin(&store, &replacement).await.is_err());
    assert_eq!(*fixture.account.lock().unwrap(), Some("foreign-account"));
    assert_eq!(*fixture.requests.lock().unwrap(), vec![("GET".into(), "/v8/management/credentials".into())], "No OAuth start, credential deletion, or replacement-profile request");
}

#[tokio::test]
async fn preflight_http_failure_keeps_the_profile_available_for_explicit_retry() {
    let fixture = AccountFixture::start().await;
    fixture.credentials_failed.store(true, std::sync::atomic::Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::load(temp.path().join("retry-preflight.db")).unwrap();
    let manager = Manager::owned_fixture(fixture.base.clone()).unwrap();
    let id = manager.create(&store, "codex", "Temporary preflight failure").unwrap();
    assert!(manager.begin(&store, &id).await.is_err());
    let view = manager.views(&store.read()).remove(0);
    assert_eq!(view.stage, Stage::Failed);
    assert!(view.service_available && view.account.is_none() && view.plan.is_none());
    assert!(manager.cleanup_finished(&id));
    fixture.credentials_failed.store(false, std::sync::atomic::Ordering::SeqCst);
    manager.begin(&store, &id).await.unwrap();
    assert_eq!(manager.views(&store.read())[0].stage, Stage::Waiting);
    manager.disconnect(&store, &id, false).await.unwrap();
    manager.remove(&store, &id).unwrap();
    let replacement = manager.create(&store, "codex", "Replacement after retry").unwrap();
    assert!(manager.views(&store.read())[0].service_available);
    manager.begin(&store, &replacement).await.unwrap();
    manager.disconnect(&store, &replacement, false).await.unwrap();
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|(method, path)| method == "GET" && path.ends_with("/oauth/auth-url")).count(), 2);
    assert!(!requests.iter().any(|(method, path)| method == "DELETE" && path.ends_with("/credentials")));
    assert!(fixture.account.lock().unwrap().is_none());
}
