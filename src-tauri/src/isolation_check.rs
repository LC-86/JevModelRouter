//! Native-webview acceptance driver, compiled only for the explicit check build.
use anyhow::Context;
use tauri::Manager;

/// 受控目录替身：只替换「上游目录与额度读取」这一项外部依赖（#17 第 5 条）。
/// 账号登录与退出、连接世代、只读状态与生成拒绝仍由真实适配器承担，准入、派发与快照
/// 全部走生产代码，因此替身不会成为真实入口的旁路——它只是把尚未接通的上游目录换成脚本可控内容。
struct CatalogStandInAdapter {
    inner: std::sync::Arc<dyn crate::subscription::SubscriptionAdapter>,
    fixture: std::path::PathBuf,
}

/// 替身按顺序读取的目录状态：每次 `models()` 前进一格，最后一格重复使用。
/// `fail` 表示同一账号下的目录读取失败（用于演练「保留已核实项并标陈旧」）。
#[derive(serde::Deserialize)]
struct CatalogFixture {
    /// 额度证据状态（available|stale|failed|unsupported|unknown），默认 unsupported。
    #[serde(default)]
    quota: Option<String>,
    #[serde(default)]
    reads: Vec<CatalogRead>,
}

#[derive(serde::Deserialize)]
struct CatalogRead {
    #[serde(default)]
    fail: bool,
    #[serde(default)]
    models: Vec<CatalogFixtureModel>,
}

#[derive(serde::Deserialize)]
struct CatalogFixtureModel {
    model_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "fixture_eligible")]
    eligible: bool,
}

fn fixture_eligible() -> bool {
    true
}

/// 目录读取游标：`models()` 每次调用前进一格；越界后重复最后一格，便于末态稳定。
static FIXTURE_CURSOR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl CatalogStandInAdapter {
    fn fixture(&self) -> anyhow::Result<CatalogFixture> {
        let bytes = std::fs::read(&self.fixture)
            .with_context(|| format!("read catalog fixture {}", self.fixture.display()))?;
        serde_json::from_slice(&bytes).context("parse catalog fixture")
    }

    fn next_read(&self) -> anyhow::Result<CatalogRead> {
        let fixture = self.fixture()?;
        let reads = fixture.reads;
        if reads.is_empty() {
            anyhow::bail!("the controlled directory has no read states");
        }
        let index = FIXTURE_CURSOR.fetch_add(1, std::sync::atomic::Ordering::SeqCst).min(reads.len() - 1);
        let mut read = reads.into_iter().nth(index).expect("the read index was clamped");
        if read.fail {
            anyhow::bail!("the controlled directory is unavailable for this account");
        }
        read.models.retain(|model| !model.model_id.trim().is_empty());
        Ok(read)
    }
}

impl crate::subscription::SubscriptionAdapter for CatalogStandInAdapter {
    fn available(&self) -> bool {
        self.inner.available()
    }

    fn supports(&self, kind: &crate::config::ProviderKind) -> bool {
        self.inner.supports(kind)
    }

    fn rename<'a>(&'a self, old_id: &'a str, new_id: &'a str) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
        self.inner.rename(old_id, new_id)
    }

    fn helper_status(&self) -> crate::subscription::HelperStatus {
        self.inner.helper_status()
    }

    fn status<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, anyhow::Result<crate::subscription::ConnectionStatus>> {
        self.inner.status(provider_id, generation)
    }

    /// 目录内容来自受控文件，其余证据仍然来自真实适配器。
    /// 返回 #15 的 `CatalogRead`：失败（`fail: true`）时必须返回 `Err`，让调用方走「保留已核实项并标陈旧」，
    /// 不得用不完整的列表冒充权威目录。受控文件给出的列表即本次权威结果（含空列表＝权威移除），
    /// 因此 `state` 固定为 `Available`，并且不记 `missing_fields`（#16 的不可信列表另有 Grok 侧分支）。
    fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, anyhow::Result<crate::subscription::CatalogRead>> {
        Box::pin(async move {
            let read = self.next_read()?;
            Ok(crate::subscription::CatalogRead {
                state: crate::subscription::EvidenceState::Available,
                models: read
                    .models
                    .into_iter()
                    .map(|model| crate::subscription::DiscoveredModel { model_id: model.model_id, name: model.name, eligible: model.eligible })
                    .collect(),
                source: Some("isolation-catalog-fixture:model/list".into()),
                observed_at: Some(chrono::Utc::now().to_rfc3339()),
                missing_fields: Vec::new(),
            })
        })
    }

    /// 额度证据同样由受控文件给出：替身不编造真实额度，默认如实报「无可机读额度」。
    fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, anyhow::Result<crate::subscription::QuotaEvidence>> {
        Box::pin(async move {
            let state = match self.fixture()?.quota.as_deref() {
                Some("available") => crate::subscription::EvidenceState::Available,
                Some("stale") => crate::subscription::EvidenceState::Stale,
                Some("failed") => crate::subscription::EvidenceState::Failed,
                Some("unknown") => crate::subscription::EvidenceState::Unknown,
                Some("unsupported") | None => crate::subscription::EvidenceState::Unsupported,
                Some(other) => anyhow::bail!("Unknown catalog fixture quota state: {other}"),
            };
            Ok(crate::subscription::QuotaEvidence {
                state,
                source: Some("isolation-catalog-fixture".into()),
                observed_at: None,
                ..crate::subscription::QuotaEvidence::default()
            })
        })
    }

    fn generate<'a>(&'a self, request: crate::subscription::GenerationRequest<'a>) -> futures_util::future::BoxFuture<'a, anyhow::Result<crate::subscription::GenerationStream<'a>>> {
        self.inner.generate(request)
    }

    fn start_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, anyhow::Result<crate::subscription::LoginStart>> {
        self.inner.start_login(provider_id, generation)
    }

    fn login_result<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, anyhow::Result<Option<crate::subscription::LoginResult>>> {
        self.inner.login_result(provider_id, generation)
    }

    fn cancel_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
        self.inner.cancel_login(provider_id, generation)
    }

    fn logout<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, anyhow::Result<crate::subscription::LogoutOutcome>> {
        self.inner.logout(provider_id, generation)
    }
}

/// 受控目录替身只在隔离模式且显式给出 `--autojev-catalog-fixture <absolute path>` 时装配。
/// 路径必须位于隔离根目录内；生产构建没有这个开关，也没有这个模块。
pub fn catalog_adapter() -> anyhow::Result<Option<std::sync::Arc<dyn crate::subscription::SubscriptionAdapter>>> {
    let args: Vec<_> = std::env::args_os().collect();
    let Some(index) = args.iter().position(|arg| arg == "--autojev-catalog-fixture") else {
        return Ok(None);
    };
    anyhow::ensure!(crate::runtime::isolated(), "--autojev-catalog-fixture requires --autojev-isolated");
    let fixture = std::path::PathBuf::from(
        args.get(index + 1)
            .context("--autojev-catalog-fixture requires an absolute path")?,
    );
    crate::runtime::check_path(&fixture).context("Catalog fixture must stay inside the isolation directory")?;
    anyhow::ensure!(fixture.is_file(), "--autojev-catalog-fixture must point at an existing file");
    Ok(Some(std::sync::Arc::new(CatalogStandInAdapter {
        inner: std::sync::Arc::new(crate::codex_helper::CodexAdapter::new()),
        fixture,
    })))
}

pub fn script() -> anyhow::Result<Option<String>> {
    let args: Vec<_> = std::env::args().collect();
    let Some(index) = args.iter().position(|s| s == "--autojev-ui-check") else {
        return Ok(None);
    };
    anyhow::ensure!(
        crate::runtime::isolated(),
        "Desktop check requires --autojev-isolated"
    );
    let url = reqwest::Url::parse(
        args.get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("Missing loopback fixture URL"))?,
    )?;
    crate::dispatch::ensure_loopback(&url)?;
    if args.iter().any(|s|s=="--autojev-cpa-auth-check") {
        let run_id=args.iter().position(|s|s=="--autojev-cpa-auth-run").and_then(|i|args.get(i+1)).context("Missing CPA auth run ID")?;
        let config=serde_json::json!({"base":url.to_string().trim_end_matches('/'),"run_id":run_id,"reload":args.iter().any(|s|s=="--autojev-check-reload")});
        return Ok(Some(format!("window.__CPA_AUTH_CHECK__={config};\n{}",include_str!("cpa-auth-check.js"))));
    }
    if let Some(index) = args.iter().position(|s| s == "--autojev-api-source-check") {
        let run_id = args.get(index + 1).context("Missing R2 acceptance run ID")?;
        let config = serde_json::json!({"base":url.to_string().trim_end_matches('/'),"run_id":run_id,"reload":args.iter().any(|s|s == "--autojev-check-reload")});
        return Ok(Some(format!("window.__API_SOURCE_CHECK__ = {config};\n{}", include_str!("api-sources-check.js"))));
    }
    if let Some(cpa_index) = args.iter().position(|s| s == "--autojev-cpa-check") {
        let binary = args.get(cpa_index + 1).context("Missing pinned CPA binary path")?;
        let run_id = args.iter().position(|s| s == "--autojev-cpa-run").and_then(|i|args.get(i + 1)).context("Missing CPA acceptance run ID")?;
        let config = serde_json::json!({"base": url.to_string().trim_end_matches('/'), "binary": binary, "run_id": run_id, "reload": args.iter().any(|s| s == "--autojev-check-reload")});
        return Ok(Some(format!("window.__CPA_CHECK__ = {config};\n{}", include_str!("cpa-validation-check.js"))));
    }
    // Optional login-lifecycle mode: the acceptance driver only rehearses one scenario per run.
    // `catalog` is the #15 read-only directory/quota acceptance path; it signs in first and then
    // rehearses one catalog + quota scenario queue per refresh.
    let login_mode = args
        .iter()
        .position(|s| s == "--autojev-login-check")
        .and_then(|index| args.get(index + 1))
        .cloned();
    if let Some(mode) = &login_mode {
        anyhow::ensure!(
            matches!(mode.as_str(), "success" | "late" | "failed" | "grok" | "grok-read" | "catalog" | "model-selection" | "grok-readonly"),
            "Unknown login check mode: {mode}"
        );
    }
    let root = crate::runtime::home_dir()
        .context("Isolation requires a home directory")?
        .to_string_lossy()
        .to_string();
    let capture_hold_ms = if args.iter().any(|s| s == "--autojev-native-screenshot") { 15_000 } else { 0 };
    let config = serde_json::json!({"base": url.to_string().trim_end_matches('/'), "reload": args.iter().any(|s| s == "--autojev-check-reload"), "loginMode": login_mode, "root": root, "captureHoldMs": capture_hold_ms});
    Ok(Some(format!(
        "window.__ISOLATION_CHECK__ = {config}; window.dispatchEvent(new Event('autojev-isolation-ready'));\n{}",
        include_str!("isolation-check.js")
    )))
}

#[tauri::command]
pub async fn isolation_check_report(
    app: tauri::AppHandle,
    report: serde_json::Value,
) -> Result<(), String> {
    if !crate::runtime::isolated() {
        return Err("Desktop check requires isolation".into());
    }
    let root = crate::runtime::home_dir().unwrap();
    let state = app.state::<crate::AppState>();
    crate::safe_stop(&state).await?;
    std::fs::write(
        root.join("isolation-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    crate::EXIT_READY.store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(if report["ok"] == true { 0 } else { 1 });
    Ok(())
}
