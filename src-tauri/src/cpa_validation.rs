//! R1 selection experiment, compiled only into the explicit isolation-check build.
//! CPA owns auth and protocol execution; Jev owns one fictional loopback process.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, path::PathBuf, process::Stdio, time::Duration};
use tauri::State;
use tokio::process::{Child, Command};

const MODEL_KEY: &str = "fictional-model-client-r1";
const POLICY: &str = "(version 1) (allow default) (deny network*) (allow network-bind (local ip \"localhost:*\")) (allow network-inbound (local ip \"localhost:*\")) (allow network-outbound (remote ip \"localhost:*\"))";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Target {
    pub prefix: String,
    pub source: String,
    pub account: String,
    pub plan: String,
    pub model: String,
    pub aliases: Vec<String>,
    pub keys: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
}

impl Target {
    fn namespace_binding(&self) -> Self {
        Self {
            disabled: false,
            ..self.clone()
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct Profile {
    pub upstream: String,
    pub port: u16,
    pub targets: Vec<Target>,
}

#[derive(Clone, Serialize)]
pub struct View {
    pub running: bool,
    pub pid: Option<u32>,
    pub base_url: String,
    pub model_client_key: &'static str,
    pub artifact: Value,
    pub targets: Vec<Value>,
}

pub struct Service {
    child: Child,
    root: OwnedDirectory,
    profile: Profile,
    bindings: HashMap<String, Target>,
    bindings_path: PathBuf,
    management_key: String,
    client: reqwest::Client,
}

// Once created, even a failed spawn/readiness path owns and removes this directory.
struct OwnedDirectory(PathBuf);

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Profile {
    fn validate(&self) -> Result<()> {
        ensure!(
            cfg!(debug_assertions) && crate::runtime::isolated(),
            "CPA validation requires an isolated debug build"
        );
        ensure!(
            cfg!(target_os = "macos"),
            "R1 CPA validation requires the macOS process network sandbox"
        );
        let url = reqwest::Url::parse(&self.upstream)?;
        crate::dispatch::ensure_loopback(&url)?;
        crate::runtime::check_cpa_upstream(&url)?;
        ensure!(
            url.path() == "/" && url.query().is_none() && url.fragment().is_none(),
            "Use the owned loopback fixture origin"
        );
        ensure!(
            self.port == 0 || (self.port >= 1024 && !matches!(self.port, 9526 | 9527 | 11434)),
            "Use an independent CPA port"
        );
        ensure!(
            !self.targets.is_empty() && self.targets.len() <= 16,
            "Select 1 to 16 fictional namespaces"
        );
        let mut prefixes = std::collections::HashSet::new();
        let mut identities = std::collections::HashSet::new();
        for t in &self.targets {
            ensure!(
                identifier(&t.prefix)
                    && identifier(&t.source)
                    && identifier(&t.account)
                    && identifier(&t.plan)
                    && identifier(&t.model),
                "Use explicit fictional source, account, plan and bare model identifiers"
            );
            ensure!(
                t.prefix != t.model,
                "Prefix must differ from the bare model ID"
            );
            ensure!(prefixes.insert(&t.prefix), "Conflicting CPA prefix");
            ensure!(
                identities.insert((&t.source, &t.account, &t.plan)),
                "Duplicate source/account/plan binding"
            );
            ensure!(
                t.keys.len() == 1,
                "Each CPA namespace requires exactly one credential"
            );
            ensure!(
                t.keys[0] == format!("fictional-{}", t.account),
                "R1 accepts only the declared fictional credential"
            );
            ensure!(
                t.aliases.len() == 1 && t.aliases[0] == t.model,
                "Shared alias pools are not allowed"
            );
        }
        Ok(())
    }
}

impl Service {
    async fn start(binary: String, mut profile: Profile) -> Result<Self> {
        profile.validate()?;
        let bindings_path = crate::runtime::home_dir()
            .context("Missing isolation home")?
            .join(".autojev/cpa-bindings-r1.json");
        let bindings: HashMap<String, Target> = if bindings_path.exists() {
            serde_json::from_slice(&fs::read(&bindings_path)?)
                .context("Read preserved R1 namespace bindings")?
        } else {
            HashMap::new()
        };
        Self::validate_bindings(&bindings, &profile)?;
        let binary = PathBuf::from(binary);
        ensure!(
            binary.is_absolute() && binary.is_file(),
            "Pinned CPA service executable is missing"
        );
        let artifact: Value =
            serde_json::from_str(include_str!("../../scripts/cpa-artifact.json"))?;
        let hash = format!(
            "{:x}",
            Sha256::digest(fs::read(&binary).context("Read pinned CPA executable")?)
        );
        ensure!(
            hash == artifact["binary_sha256"],
            "CPA version/checksum does not match the pinned R1 artifact"
        );
        let listener = std::net::TcpListener::bind(("127.0.0.1", profile.port))
            .context("CPA port is already occupied")?;
        profile.port = listener.local_addr()?.port();
        let root = crate::runtime::home_dir()
            .context("Missing isolation home")?
            .join(".autojev")
            .join(format!("cpa-r1-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root)?;
        let root = OwnedDirectory(root);
        fs::create_dir(root.0.join("auth"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(root.0.join("auth"), fs::Permissions::from_mode(0o700))?;
        }
        let management_key = uuid::Uuid::new_v4().to_string();
        Self::write_config(&root.0, &profile, &management_key)?;
        let log = fs::File::create(root.0.join("service.log"))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(1))
            .build()?;
        drop(listener);
        let child = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", POLICY])
            .arg(binary)
            .arg("--config")
            .arg(root.0.join("config.json"))
            .arg("--local-model")
            .current_dir(&root.0)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .context("Start owned pinned CPA service")?;
        let mut service = Self {
            child,
            root,
            bindings,
            bindings_path,
            profile,
            management_key,
            client,
        };
        let ready = async {
            service.wait_ready(true).await?;
            service.remember_bindings()
        }
        .await;
        if let Err(error) = ready {
            service
                .stop()
                .await
                .context(format!("CPA startup failed: {error:#}"))?;
            return Err(error);
        }
        crate::runtime::cpa_validation_port(service.profile.port);
        Ok(service)
    }

    fn write_config(root: &std::path::Path, profile: &Profile, management_key: &str) -> Result<()> {
        let config = json!({
            "config-version":8,
            "server":{"host":"127.0.0.1","port":profile.port,"discovery":{"enabled":false}},
            "management":{"allow-remote":false,"secret-key":management_key,"disable-control-panel":true,"disable-auto-update-panel":true},
            "access":{"api-keys":[MODEL_KEY]}, "oauth":{"auth-dir":root.join("auth")},
            "routing":{"force-model-prefix":true,"session-affinity":false,"retry":{"request-retry":0,"max-retry-interval":0},"cooldown":{"disable-cooling":false}},
            "requests":{"streaming":{"bootstrap-retries":0}},
            "api-keys":{"openai-compatibility":profile.targets.iter().map(|t| json!({
                "name":t.prefix,"prefix":t.prefix,"disabled":t.disabled,
                "base-url":format!("{}/{}/v1",profile.upstream.trim_end_matches('/'),t.account),
                "keys":[{"api-key":t.keys[0],"proxy-url":"direct"}],"models":[{"name":t.model,"alias":t.model}]
            })).collect::<Vec<_>>()}
        });
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(root.join("config.json"))?
            .write_all(&serde_json::to_vec_pretty(&config)?)?;
        Ok(())
    }

    async fn wait_ready(&mut self, startup: bool) -> Result<()> {
        let artifact: Value =
            serde_json::from_str(include_str!("../../scripts/cpa-artifact.json"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            ensure!(
                self.child.try_wait()?.is_none(),
                "CPA exited before readiness (port conflict or startup failure)"
            );
            let ready = async {
                let health = self
                    .client
                    .get(format!("{}/healthz", self.base()))
                    .send()
                    .await?;
                ensure!(health.status().is_success(), "CPA health is unavailable");
                let management = self
                    .client
                    .get(format!("{}/v8/management/config", self.base()))
                    .bearer_auth(&self.management_key)
                    .send()
                    .await?;
                ensure!(
                    management.status().is_success(),
                    "CPA v8 management API is unavailable"
                );
                ensure!(
                    management
                        .headers()
                        .get("X-CPA-COMMIT")
                        .and_then(|h| h.to_str().ok())
                        == artifact["commit"].as_str()
                        && management
                            .headers()
                            .get("X-CPA-VERSION")
                            .and_then(|h| h.to_str().ok())
                            == artifact["version"].as_str(),
                    "CPA service version is incompatible"
                );
                let models: Value = self
                    .client
                    .get(format!("{}/v1/models", self.base()))
                    .bearer_auth(MODEL_KEY)
                    .send()
                    .await?
                    .json()
                    .await?;
                let mut found: Vec<&str> = models["data"]
                    .as_array()
                    .context("CPA model list is missing")?
                    .iter()
                    .filter_map(|m| m["id"].as_str())
                    .collect();
                let mut expected: Vec<String> = self
                    .profile
                    .targets
                    .iter()
                    .filter(|t| !t.disabled)
                    .map(|t| format!("{}/{}", t.prefix, t.model))
                    .collect();
                found.sort();
                expected.sort();
                ensure!(found == expected, "CPA namespace reload is pending");
                if startup {
                    ensure!(
                        fs::read_to_string(self.root.0.join("service.log"))?
                            .contains("file watcher started for config"),
                        "CPA watcher is starting"
                    );
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if ready.is_ok() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return ready.context("CPA readiness timed out");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn remember_bindings(&mut self) -> Result<()> {
        for target in &self.profile.targets {
            self.bindings
                .insert(target.prefix.clone(), target.namespace_binding());
        }
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&self.bindings_path)?
            .write_all(&serde_json::to_vec_pretty(&self.bindings)?)?;
        Ok(())
    }

    fn validate_bindings(bindings: &HashMap<String, Target>, profile: &Profile) -> Result<()> {
        for target in &profile.targets {
            ensure!(
                bindings
                    .get(&target.prefix)
                    .is_none_or(|old| old == &target.namespace_binding()),
                "Cannot rebind a namespace to a different source, account, plan or model"
            );
        }
        Ok(())
    }

    fn validate_reload(&self, profile: &Profile) -> Result<()> {
        profile.validate()?;
        ensure!(
            profile.upstream == self.profile.upstream
                && (profile.port == 0 || profile.port == self.profile.port),
            "Reload cannot change the owned endpoint"
        );
        Self::validate_bindings(&self.bindings, profile)
    }

    async fn reload(&mut self, mut profile: Profile) -> Result<()> {
        profile.port = self.profile.port;
        Self::write_config(&self.root.0, &profile, &self.management_key)?;
        self.profile = profile;
        self.wait_ready(false).await?;
        self.remember_bindings()
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.profile.port)
    }

    fn view(&mut self) -> Result<View> {
        let running = self.child.try_wait()?.is_none();
        if !running { crate::runtime::cpa_validation_port(0); }
        Ok(View { running, pid: self.child.id(), base_url: self.base(), model_client_key: MODEL_KEY,
            artifact: serde_json::from_str(include_str!("../../scripts/cpa-artifact.json")).expect("Bundled artifact manifest"),
            targets: self.profile.targets.iter().map(|t|json!({"prefix":t.prefix,"source":t.source,"account":t.account,"plan":t.plan,"model":t.model,"disabled":t.disabled})).collect() })
    }

    pub async fn stop(mut self) -> Result<()> {
        crate::runtime::cpa_validation_port(0);
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
            tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .context("Timed out reaping owned CPA process")?
                .context("Reap owned CPA process")?;
        }
        fs::remove_dir_all(&self.root.0).context("Remove owned CPA config and empty auth space")?;
        Ok(())
    }
}

#[tauri::command]
pub async fn start_cpa_validation(
    state: State<'_, crate::AppState>,
    binary: String,
    profile: Profile,
) -> Result<View, String> {
    let mut owned = state.cpa_validation.lock().await;
    let exited = match owned.as_mut() {
        Some(service) => service.child.try_wait().map_err(|e| e.to_string())?.is_some(),
        None => false,
    };
    if exited {
        if let Some(service) = owned.take() {
            service.stop().await.map_err(|e| format!("{e:#}"))?;
        }
    }
    if owned.is_some() {
        return Err("Stop the owned CPA before starting another profile".into());
    }
    let mut service = Service::start(binary, profile)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let view = service.view().map_err(|e| format!("{e:#}"))?;
    *owned = Some(service);
    Ok(view)
}

#[tauri::command]
pub async fn reload_cpa_validation(
    state: State<'_, crate::AppState>,
    profile: Profile,
) -> Result<View, String> {
    let mut owned = state.cpa_validation.lock().await;
    let service = owned
        .as_mut()
        .context("Owned CPA service is not running")
        .map_err(|e| e.to_string())?;
    service
        .validate_reload(&profile)
        .map_err(|e| format!("{e:#}"))?;
    if let Err(error) = service.reload(profile).await {
        // A failed applied reload is unproved: revoke its endpoint and reap it.
        if let Some(service) = owned.take() {
            service.stop().await.map_err(|cleanup| {
                format!("CPA reload failed: {error:#}; cleanup failed: {cleanup:#}")
            })?;
        }
        return Err(format!(
            "CPA reload failed; owned service stopped: {error:#}"
        ));
    }
    service.view().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn stop_cpa_validation(state: State<'_, crate::AppState>) -> Result<(), String> {
    if let Some(service) = state.cpa_validation.lock().await.take() {
        service.stop().await.map_err(|e| format!("{e:#}"))?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct DevelopmentView {
    state: &'static str,
    binary: String,
    port: u16,
    service: Option<View>,
    real_generation_enabled: bool,
    artifact: Value,
}

// This entry is deliberately absent from ordinary builds. It never provisions
// OAuth or real credentials, and shares the R1 single-credential fixture rules.
fn development_profile() -> Result<Option<(String, Profile)>> {
    if !crate::runtime::isolated() || !cfg!(debug_assertions) { return Ok(None); }
    let args: Vec<_> = std::env::args().collect();
    let Some(index) = args.iter().position(|s| s == "--autojev-cpa-service") else { return Ok(None); };
    let binary = args.get(index + 1).context("Missing pinned CPA artifact path")?.clone();
    let upstream = args.iter().position(|s| s == "--autojev-upstream")
        .and_then(|i| args.get(i + 1)).context("Missing loopback fixture")?.clone();
    let path = crate::runtime::home_dir().context("Missing isolation home")?.join(".autojev/cpa-development-port.json");
    let port = if path.exists() { serde_json::from_slice(&fs::read(path)?)? } else { 0 };
    let profile = Profile { upstream, port, targets: ["a1", "a2", "b1", "paid"].iter().map(|account| Target {
        prefix: format!("jev-{account}"),
        source: if account.starts_with('a') { "source-a".into() } else { format!("source-{account}") },
        account: (*account).into(), plan: format!("fictional-plan-{account}"), model: "same-model".into(),
        aliases: vec!["same-model".into()], keys: vec![format!("fictional-{account}")], disabled: false,
    }).collect() };
    profile.validate()?;
    Ok(Some((binary, profile)))
}

#[tauri::command]
pub async fn get_cpa_development_service(state: State<'_, crate::AppState>) -> Result<Option<DevelopmentView>, String> {
    let Some((binary, profile)) = development_profile().map_err(|e| format!("{e:#}"))? else { return Ok(None); };
    let mut owned = state.cpa_validation.lock().await;
    let service = owned.as_mut().map(Service::view).transpose().map_err(|e| format!("{e:#}"))?;
    let phase = match &service {
        Some(view) if view.running => "ready",
        Some(_) => "exited",
        None if !std::path::Path::new(&binary).is_file() => "missing",
        None => "stopped",
    };
    Ok(Some(DevelopmentView { state: phase, binary, port: profile.port, service,
        real_generation_enabled: false,
        artifact: serde_json::from_str(include_str!("../../scripts/cpa-artifact.json")).map_err(|e|e.to_string())?,
    }))
}

#[tauri::command]
pub async fn start_cpa_development_service(state: State<'_, crate::AppState>, binary: String, port: u16) -> Result<View, String> {
    let Some((_, mut profile)) = development_profile().map_err(|e| format!("{e:#}"))? else {
        return Err("CPA development service requires the explicit isolated debug profile".into());
    };
    if profile.port != 0 && port != profile.port {
        return Err("Use the saved port to preserve fixed targets; release its occupant or create a new isolated home and new source references".into());
    }
    profile.port = port;
    let view = start_cpa_validation(state.clone(), binary, profile).await?;
    let path = crate::runtime::home_dir().expect("Validated isolation home").join(".autojev/cpa-development-port.json");
    if let Err(error) = fs::write(path, serde_json::to_vec(&reqwest::Url::parse(&view.base_url).expect("Owned endpoint").port().expect("Owned port")).expect("Port JSON")) {
        stop_cpa_validation(state).await?;
        return Err(format!("CPA stopped because its recovery port could not be saved: {error}"));
    }
    Ok(view)
}
