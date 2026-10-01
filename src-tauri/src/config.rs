use std::{fs, path::PathBuf, sync::RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

fn default_tools() -> bool { true }
/// 旧配置与 API 模型默认已选：迁移不得把既有模型变成未选。
fn default_selected() -> bool { true }

pub const DEFAULT_PORT: u16 = 9527;
pub const DEV_PORT: u16 = 9526;

/// A single, process-memory consent window for manual Codex generation checks.
/// `used_calls` counts AutoJev requests admitted through the subscription adapter,
/// including attempts that later fail or are cancelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexRealGenerationGrant {
    pub generation: u64,
    pub max_calls: u32,
    pub used_calls: u32,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provider {
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub api_type: String,
    #[serde(default)]
    pub test_model: String,
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    #[serde(default)]
    pub has_api_key: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Openrouter,
    Ollama,
    OpenaiCompatible,
    /// 订阅身份：由本应用管理的官方辅助进程承载授权，不与 API 凭据混用。
    CodexSubscription,
    GrokSubscription,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    #[serde(default)]
    pub input_price_known: Option<bool>,
    #[serde(default)]
    pub output_price_known: Option<bool>,
    #[serde(default)]
    pub cache_price_known: Option<bool>,
    #[serde(default)]
    pub api_type: String,
    #[serde(default)]
    pub cache_cost_per_million: f64,
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub name: String,
    pub tier: ModelTier,
    pub enabled: bool,
    /// 用户是否把该模型列入模型列表与自动候选。取消选择不禁止原模型标识的合规直调，停用才禁止调用。
    #[serde(default = "default_selected")]
    pub selected: bool,
    #[serde(default = "default_tools")]
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_reasoning: bool,
    pub context_window: u64,
    #[serde(default)]
    pub input_cost_per_million: f64,
    #[serde(default)]
    pub output_cost_per_million: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Fast,
    Balanced,
    Strong,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Observe,
    Assist,
    Auto,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingPolicy {
    pub mode: RoutingMode,
    pub prefer_local: bool,
    pub use_jev_when_ambiguous: bool,
    pub jev_endpoint: String,
    #[serde(default)]
    pub decision_provider: DecisionProvider,
    #[serde(default)]
    pub jev_model: String,
    #[serde(default)]
    pub decision_preference: String,
    #[serde(default)]
    pub has_autojev_key: bool,
    pub savings_baseline_model_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionProvider {
    #[default]
    Openrouter,
    Zenmux,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteEvent {
    pub id: String,
    pub created_at: String,
    pub endpoint: String,
    pub provider_name: String,
    pub model_name: String,
    pub reason: String,
    pub source: String,
    pub estimated_input_tokens: u64,
    pub estimated_cost: Option<f64>,
    pub estimated_savings: f64,
    pub success: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteRule {
    #[serde(default)]
    pub all_models: bool,
    #[serde(default)]
    pub automatic_policy: Option<RoutingPolicy>,
    #[serde(default)]
    pub model_settings: std::collections::HashMap<String, RouteModelSettings>,
    pub id: String,
    pub name: String,
    pub strategy: String,
    pub model_ids: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(skip)]
    pub cost_history: Vec<crate::traffic::RequestLog>,
    #[serde(skip)]
    pub output_limit: Option<u64>,
    #[serde(skip)]
    pub cost_incumbent: Option<String>,
    #[serde(default)]
    pub performance_settings: crate::performance::Settings,
    #[serde(default)]
    pub performance_samples: std::collections::HashMap<String, Vec<crate::performance::Sample>>,
    #[serde(default)]
    pub gateway: crate::resilience::Settings,
    #[serde(default)]
    pub agent_catalogs: std::collections::HashMap<String, Vec<crate::agent_catalog::Entry>>,
    #[serde(default)]
    pub agent_selections: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub agent_auto_connect: std::collections::HashMap<String, bool>,
    #[serde(default)]
    pub custom_agents: Vec<crate::agents::CustomAgent>,
    #[serde(default)]
    pub routes: Vec<RouteRule>,
    /// 订阅服务商的活动连接，按服务商标识索引：每家一个，证据绑定连接世代。
    #[serde(default)]
    pub subscriptions: std::collections::HashMap<String, crate::subscription::Connection>,
    /// 订阅目录：按服务商保存上游已核实模型目录与账号绑定资格。用户的选择／停用保存在 `models` 行上，
    /// 因此退出或换号只作废资格，不重建标识、也不丢失配置。
    #[serde(default)]
    pub subscription_catalogs: std::collections::HashMap<String, crate::subscription_catalog::ProviderCatalog>,
    /// 用户对当前连接世代的一次有限真实生成许可。仅驻留进程内存，重启、退出或换号后不会沿用。
    #[serde(skip)]
    pub codex_real_generation_grants: std::collections::HashMap<String, CodexRealGenerationGrant>,
    pub install_id: String,
    pub port: u16,
    pub providers: Vec<Provider>,
    pub models: Vec<Model>,
    pub policy: RoutingPolicy,
    #[serde(default)]
    pub events: Vec<RouteEvent>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            cost_history: Vec::new(), output_limit: None, cost_incumbent: None,
            performance_settings: Default::default(),
            performance_samples: Default::default(),
            gateway: Default::default(),
            agent_catalogs: Default::default(),
            agent_selections: Default::default(),
            agent_auto_connect: Default::default(),
            custom_agents: Vec::new(),
            routes: Vec::new(),
            subscriptions: Default::default(),
            subscription_catalogs: Default::default(),
            codex_real_generation_grants: Default::default(),
            install_id: Uuid::new_v4().to_string(),
            port: DEFAULT_PORT,
            providers: vec![
                Provider {
                    preset: String::new(), api_type: String::new(), test_model: String::new(),
                    id: "openrouter".into(),
                    name: "OpenRouter".into(),
                    kind: ProviderKind::Openrouter,
                    base_url: "https://openrouter.ai/api".into(),
                    enabled: true,
                    has_api_key: false,
                },
                Provider {
                    preset: String::new(), api_type: String::new(), test_model: String::new(),
                    id: "ollama".into(),
                    name: "Ollama".into(),
                    kind: ProviderKind::Ollama,
                    base_url: "http://127.0.0.1:11434".into(),
                    enabled: false,
                    has_api_key: false,
                },
            ],
            models: vec![
                Model { input_price_known: None, output_price_known: None, cache_price_known: None,
                    api_type: String::new(), cache_cost_per_million: 0.0,
                    id: "qwen-fast".into(),
                    provider_id: "openrouter".into(),
                    model_id: "qwen/qwen3-coder-flash".into(),
                    name: "Qwen 3 Coder Flash".into(),
                    tier: ModelTier::Fast,
                    enabled: true,
                    selected: true,
                    supports_tools: true,
                    supports_vision: false,
                    supports_reasoning: false,
                    context_window: 262_144,
                    input_cost_per_million: 0.3,
                    output_cost_per_million: 1.2,
                },
                Model { input_price_known: None, output_price_known: None, cache_price_known: None,
                    api_type: String::new(), cache_cost_per_million: 0.0,
                    id: "claude-sonnet-4".into(),
                    provider_id: "openrouter".into(),
                    model_id: "anthropic/claude-sonnet-4".into(),
                    name: "Claude Sonnet 4".into(),
                    tier: ModelTier::Strong,
                    enabled: true,
                    selected: true,
                    supports_tools: true,
                    supports_vision: true,
                    supports_reasoning: true,
                    context_window: 200_000,
                    input_cost_per_million: 3.0,
                    output_cost_per_million: 15.0,
                },
            ],
            policy: RoutingPolicy {
                mode: RoutingMode::Auto,
                prefer_local: false,
                use_jev_when_ambiguous: true,
                jev_model: "~typesafe/jev-latest".into(),
                decision_provider: DecisionProvider::Openrouter,
                decision_preference: "balanced".into(),
                jev_endpoint: "https://openrouter.ai/api/alpha/decisions".into(),
                has_autojev_key: false,
                savings_baseline_model_id: Some("claude-sonnet-4".into()),
            },
            events: Vec::new(),
        }
    }
}

pub struct ConfigStore {
    pub dispatcher: std::sync::Arc<dyn crate::dispatch::Dispatcher>,
    /// 订阅适配边界（按服务商类型分派的注册表）。默认构造只装
    /// [`crate::subscription::UnavailableAdapter`]（如实返回不可用）；生产在 `lib.rs` 构造
    /// `ConfigStore` 处注入真实的 Codex + Grok 适配器，代码内固定、无运行时替身开关。
    pub subscription: std::sync::Arc<crate::subscription::SubscriptionAdapters>,
    /// 订阅登录/退出边界。生产构造只注入 [`crate::subscription::auth::GrokCliAuth`]。
    pub auth: std::sync::Arc<dyn crate::subscription::auth::SubscriptionAuth>,
    path: PathBuf,
    value: RwLock<AppConfig>,
    /// 进程内、按服务商递增的刷新序号；不持久化、不跨异步读取持锁。
    subscription_refreshes: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Result<Self> {
        Self::load_with_dispatcher(
            path,
            std::sync::Arc::new(crate::dispatch::ApiDispatcher {
                loopback_only: crate::runtime::isolated(),
            }),
        )
    }

    pub fn load_with_dispatcher(path: PathBuf, dispatcher: std::sync::Arc<dyn crate::dispatch::Dispatcher>) -> Result<Self> {
        Self::load_with_adapters(path, dispatcher, std::sync::Arc::new(crate::subscription::UnavailableAdapter))
    }

    pub fn load_with_adapters(
        path: PathBuf,
        dispatcher: std::sync::Arc<dyn crate::dispatch::Dispatcher>,
        subscription: std::sync::Arc<dyn crate::subscription::SubscriptionAdapter>,
    ) -> Result<Self> {
        Self::load_with_adapters_and_auth(
            path,
            dispatcher,
            subscription,
            std::sync::Arc::new(crate::subscription::auth::GrokCliAuth::new()),
        )
    }

    /// 与 [`Self::load_with_adapters`] 相同，但允许测试注入授权替身。
    /// 生产构造路径只有 [`Self::load_with_adapters`]，配置与界面都没有替换替身的开关。
    pub fn load_with_adapters_and_auth(
        path: PathBuf,
        dispatcher: std::sync::Arc<dyn crate::dispatch::Dispatcher>,
        subscription: std::sync::Arc<dyn crate::subscription::SubscriptionAdapter>,
        auth: std::sync::Arc<dyn crate::subscription::auth::SubscriptionAuth>,
    ) -> Result<Self> {
        let subscription = std::sync::Arc::new(crate::subscription::SubscriptionAdapters::single(subscription));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("create AutoJev data directory")?;
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut db = Self::connect(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS credentials (account TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS request_logs (id TEXT PRIMARY KEY, created_at TEXT NOT NULL, data TEXT NOT NULL); CREATE INDEX IF NOT EXISTS request_logs_created ON request_logs(created_at);")?;
        let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let stored: Option<String> = transaction.query_row("SELECT value FROM app_meta WHERE key = 'config'", [], |row| row.get(0)).optional()?;
        let mut value: AppConfig = if let Some(data) = stored {
            serde_json::from_str(&data).context("parse AutoJev database configuration")?
        } else {
            let value = AppConfig::default();
            transaction.execute("INSERT INTO app_meta (key, value) VALUES ('config', ?1)", [serde_json::to_string(&value)?])?;
            value
        };
        if value.performance_settings.version == 0 {
            value.performance_settings = Default::default();
            transaction.execute("UPDATE app_meta SET value = ?1 WHERE key = 'config'", [serde_json::to_string(&value)?])?;
        }
        if value.models.iter().any(|model| !model.supports_tools) {
            for model in &mut value.models { model.supports_tools = true; }
            transaction.execute("UPDATE app_meta SET value = ?1 WHERE key = 'config'", [serde_json::to_string(&value)?])?;
        }
        // Import previously retained route events once; their token usage remains unknown.
        for event in &value.events {
            let log = crate::traffic::RequestLog {
                id: event.id.clone(), created_at: event.created_at.clone(), agent: "Unknown agent".into(),
                endpoint: event.endpoint.clone(), provider_name: event.provider_name.clone(),
                model_name: event.model_name.clone(), source: event.source.clone(), reason: event.reason.clone(),
                status: if event.success { "success" } else { "error" }.into(),
                ..Default::default()
            };
            transaction.execute("INSERT OR IGNORE INTO request_logs (id, created_at, data) VALUES (?1, ?2, ?3)",
                rusqlite::params![log.id, log.created_at, serde_json::to_string(&log)?])?;
        }
        // 登录会话是进程内的：重启后残留的授权中连接归位为未连接，pending 登录不会存活。
        let mut resumed: Vec<String> = Vec::new();
        for (provider_id, connection) in value.subscriptions.iter_mut() {
            if connection.state == crate::subscription::ConnectionState::AuthorizationPending {
                connection.state = crate::subscription::ConnectionState::NotConnected;
                resumed.push(provider_id.clone());
            }
        }
        if !resumed.is_empty() {
            transaction.execute("UPDATE app_meta SET value = ?1 WHERE key = 'config'", [serde_json::to_string(&value)?])?;
        }
        transaction.commit()?;
        // 这些登录会话已随进程消失：顺带清掉它们的专用 home，避免残留脏目录。
        // 清理失败不得阻止加载；状态归位已经完成，只把失败如实记到日志（错误必须脱敏）。
        for provider_id in &resumed {
            if let Err(error) = auth.dispose(provider_id) {
                eprintln!("AutoJev subscription cleanup for {provider_id}: {}", crate::subscription::helper::redact(&error));
            }
        }
        Ok(Self { path, value: RwLock::new(value), dispatcher, subscription, auth, subscription_refreshes: Default::default() })

    }

    fn connect(path: &PathBuf) -> Result<Connection> {
        let db = Connection::open(path).context("open AutoJev database")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(db)
    }

    pub fn read(&self) -> AppConfig {
        self.value.read().expect("config lock poisoned").clone()
    }

    pub fn read_with_decision_key(&self) -> (AppConfig, Option<String>) {
        // A provider switch holds the write lock through the config/key transaction.
        let config = self.value.read().expect("config lock poisoned");
        let key = self.read_secret("autojev-cloud");
        (config.clone(), key)
    }

    pub(crate) fn begin_subscription_refresh(&self, provider_id: &str) -> Result<u64, String> {
        let mut requests = self.subscription_refreshes.lock().expect("refresh lock poisoned");
        let request = requests.entry(provider_id.to_owned()).or_default();
        *request = request.checked_add(1).ok_or("The refresh request sequence was exhausted")?;
        Ok(*request)
    }

    pub(crate) fn update_subscription_refresh<T>(
        &self,
        provider_id: &str,
        request: u64,
        change: impl FnOnce(&mut AppConfig) -> Result<T, String>,
    ) -> Result<T, String> {
        // 登记与提交共享此短同步锁：原子配置校验/写回期间不能登记新序号。没有 await。
        let requests = self.subscription_refreshes.lock().expect("refresh lock poisoned");
        self.update(|config| {
            if requests.get(provider_id) != Some(&request) {
                return Err("The connection changed while refreshing; a newer refresh started and the read-only result was discarded".into());
            }
            change(config)
        }).map_err(|error| error.to_string())?
    }

    pub fn update<T>(&self, change: impl FnOnce(&mut AppConfig) -> T) -> Result<T> {
        let mut current = self.value.write().expect("config lock poisoned");
        let mut next = current.clone();
        let result = change(&mut next);
        let db = Self::connect(&self.path)?;
        db.execute("INSERT INTO app_meta (key, value) VALUES ('config', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [serde_json::to_string(&next)?])
            .context("save AutoJev configuration")?;
        *current = next;
        Ok(result)
    }

    pub fn update_checked(&self, change: impl FnOnce(&mut AppConfig) -> Result<()>, credential: Option<(&str, &str, Option<&str>)>) -> Result<()> {
        let mut current = self.value.write().expect("config lock poisoned");
        let mut next = current.clone();
        change(&mut next)?;
        let mut db = Self::connect(&self.path)?;
        let tx = db.transaction()?;
        if let Some((old, new, key)) = credential {
            if old != new {
                tx.execute("INSERT OR REPLACE INTO credentials (account,value) SELECT ?2,value FROM credentials WHERE account=?1", [old,new])?;
                tx.execute("DELETE FROM credentials WHERE account=?1", [old])?;
            }
            if let Some(key) = key {
                tx.execute("INSERT INTO credentials (account,value) VALUES (?1,?2) ON CONFLICT(account) DO UPDATE SET value=excluded.value", [new,key])?;
            }
        }
        tx.execute("INSERT INTO app_meta (key,value) VALUES ('config',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(&next)?])?;
        tx.commit()?;
        *current = next;
        Ok(())
    }

    pub fn save_request_log(&self, log: &crate::traffic::RequestLog) -> Result<()> {
        Self::connect(&self.path)?.execute("INSERT OR REPLACE INTO request_logs (id, created_at, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![log.id, log.created_at, serde_json::to_string(log)?])?;
        Ok(())
    }

    pub fn request_log(&self, id: &str) -> Result<Option<crate::traffic::RequestLog>> {
        use rusqlite::OptionalExtension;
        let data: Option<String> = Self::connect(&self.path)?.query_row(
            "SELECT data FROM request_logs WHERE id = ?1", [id], |row| row.get(0),
        ).optional()?;
        data.map(|value| serde_json::from_str(&value).map_err(Into::into)).transpose()
    }

    pub fn request_logs(&self, since: &str) -> Result<Vec<crate::traffic::RequestLog>> {
        let db = Self::connect(&self.path)?;
        let mut statement = db.prepare("SELECT data FROM request_logs WHERE created_at >= ?1 ORDER BY created_at DESC, id DESC")?;
        let rows = statement.query_map([since], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn recent_cost_logs(&self) -> Result<Vec<crate::traffic::RequestLog>> {
        let db = Self::connect(&self.path)?;
        let since = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
        let mut statement = db.prepare("SELECT data FROM request_logs WHERE created_at >= ?1 ORDER BY created_at DESC LIMIT 500")?;
        let rows = statement.query_map([since], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn add_event(&self, event: RouteEvent) -> Result<()> {
        self.update(|config| {
            config.events.insert(0, event);
            config.events.truncate(250);
        })
    }
}

impl ConfigStore {
    pub fn read_secret(&self, account: &str) -> Option<String> {
        Self::connect(&self.path).ok()?.query_row(
            "SELECT value FROM credentials WHERE account = ?1", [account], |row| row.get::<_, String>(0)
        ).optional().ok().flatten().filter(|value| !value.is_empty())
    }

    pub fn write_secret(&self, account: &str, value: &str) -> Result<()> {
        Self::connect(&self.path)?.execute(
            "INSERT INTO credentials (account, value) VALUES (?1, ?2) ON CONFLICT(account) DO UPDATE SET value = excluded.value", [account, value]
        ).context("save local credential")?;
        Ok(())
    }

    pub fn delete_secret(&self, account: &str) -> Result<()> {
        Self::connect(&self.path)?.execute("DELETE FROM credentials WHERE account = ?1", [account]).context("delete local credential")?;
        Ok(())
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;

    #[test]
    fn a_config_without_subscriptions_loads_and_keeps_api_identity_unchanged() {
        let mut legacy = serde_json::to_value(AppConfig::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("subscriptions");
        let restored: AppConfig = serde_json::from_value(legacy).unwrap();
        assert!(restored.subscriptions.is_empty());
        assert_eq!(restored.providers.len(), 2);
        assert_eq!(restored.providers[0].kind, ProviderKind::Openrouter);
        assert_eq!(restored.models[0].provider_id, "openrouter");
        assert!(restored.models[0].enabled);

        // 订阅服务商与连接可以往返保存，API 服务商不受影响。
        let mut saved = AppConfig::default();
        let mut provider = saved.providers[0].clone();
        provider.id = "codex".into();
        provider.name = "Codex".into();
        provider.kind = ProviderKind::CodexSubscription;
        provider.base_url = String::new();
        saved.providers.push(provider.clone());
        crate::subscription::sync_provider(&mut saved, &provider.id, &provider.kind);
        let round_trip: AppConfig = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(round_trip.subscriptions.len(), 1);
        assert_eq!(round_trip.subscriptions["codex"].generation, 1);
        assert_eq!(round_trip.subscriptions["codex"].state, crate::subscription::ConnectionState::NotConnected);
        assert_eq!(round_trip.providers[0].kind, ProviderKind::Openrouter);
        assert_eq!(round_trip.models.len(), saved.models.len());

        // 未知的服务商类型仍然整库拒绝，不会被静默降级成 API 服务商。
        let mut unknown = serde_json::to_value(AppConfig::default()).unwrap();
        unknown["providers"][0]["kind"] = serde_json::json!("some_future_subscription");
        assert!(serde_json::from_value::<AppConfig>(unknown).is_err());
    }

    #[test]
    fn codex_real_generation_opt_in_is_not_persisted_across_restart() {
        let mut config = AppConfig::default();
        config.codex_real_generation_grants.insert(
            "codex".into(),
            CodexRealGenerationGrant {
                generation: 7,
                max_calls: 3,
                used_calls: 1,
                enabled: true,
            },
        );
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(!serialized.contains("codex_real_generation_grants"));
        let restored: AppConfig = serde_json::from_str(&serialized).unwrap();
        assert!(restored.codex_real_generation_grants.is_empty());
    }

    #[test]
    fn a_restart_normalizes_pending_authorization_back_to_not_connected() {
        // 登录会话是进程内的：重启后残留的“授权中”不得被当成仍然有效的登录。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        store
            .update(|config| {
                let mut provider = config.providers[0].clone();
                provider.id = "grok".into();
                provider.name = "Grok".into();
                provider.kind = ProviderKind::GrokSubscription;
                provider.base_url = String::new();
                config.providers.push(provider.clone());
                crate::subscription::sync_provider(config, &provider.id, &provider.kind);
                config.subscriptions.get_mut("grok").unwrap().state = crate::subscription::ConnectionState::AuthorizationPending;
            })
            .unwrap();
        drop(store);
        let reopened = ConfigStore::load(path.clone()).unwrap();
        assert_eq!(
            reopened.read().subscriptions["grok"].state,
            crate::subscription::ConnectionState::NotConnected
        );
        // 归位会落库：再开一次也不会回到 AuthorizationPending。
        drop(reopened);
        let again = ConfigStore::load(path).unwrap();
        assert_eq!(again.read().subscriptions["grok"].state, crate::subscription::ConnectionState::NotConnected);
    }

    #[test]
    fn a_restart_disposes_the_pending_login_home() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending-clean.db");
        let program = PathBuf::from("/nonexistent-fixture-helper");
        let dispatcher = || std::sync::Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true });
        let auth = || std::sync::Arc::new(crate::subscription::auth::GrokCliAuth::with_test_helper(dir.path().to_path_buf(), program.clone()));
        let store = ConfigStore::load_with_adapters_and_auth(
            path.clone(),
            dispatcher(),
            std::sync::Arc::new(crate::subscription::UnavailableAdapter),
            auth(),
        )
        .unwrap();
        store
            .update(|config| {
                let mut provider = config.providers[0].clone();
                provider.id = "grok".into();
                provider.name = "Grok".into();
                provider.kind = ProviderKind::GrokSubscription;
                provider.base_url = String::new();
                config.providers.push(provider.clone());
                crate::subscription::sync_provider(config, &provider.id, &provider.kind);
                config.subscriptions.get_mut("grok").unwrap().state = crate::subscription::ConnectionState::AuthorizationPending;
            })
            .unwrap();
        // 模拟进程内登录留下的脏目录。
        let home = crate::subscription::helper::helper_home(&ProviderKind::GrokSubscription, "grok", dir.path()).unwrap();
        crate::subscription::helper::prepare_home(&home).unwrap();
        std::fs::write(home.join("session"), "fixture").unwrap();
        drop(store);
        let reopened = ConfigStore::load_with_adapters_and_auth(
            path,
            dispatcher(),
            std::sync::Arc::new(crate::subscription::UnavailableAdapter),
            auth(),
        )
        .unwrap();
        assert_eq!(reopened.read().subscriptions["grok"].state, crate::subscription::ConnectionState::NotConnected);
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "重启归位必须清空专用 home");
    }

    #[test]
    fn a_restart_still_loads_when_the_pending_home_cleanup_fails() {
        // home 是 symlink 时 cleanup_home 拒绝：清理失败不得阻止加载，状态仍须归位。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending-symlink.db");
        let program = PathBuf::from("/nonexistent-fixture-helper");
        let dispatcher = || std::sync::Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true });
        let auth = || std::sync::Arc::new(crate::subscription::auth::GrokCliAuth::with_test_helper(dir.path().to_path_buf(), program.clone()));
        let store = ConfigStore::load_with_adapters_and_auth(
            path.clone(),
            dispatcher(),
            std::sync::Arc::new(crate::subscription::UnavailableAdapter),
            auth(),
        )
        .unwrap();
        store
            .update(|config| {
                let mut provider = config.providers[0].clone();
                provider.id = "grok".into();
                provider.name = "Grok".into();
                provider.kind = ProviderKind::GrokSubscription;
                provider.base_url = String::new();
                config.providers.push(provider.clone());
                crate::subscription::sync_provider(config, &provider.id, &provider.kind);
                config.subscriptions.get_mut("grok").unwrap().state = crate::subscription::ConnectionState::AuthorizationPending;
            })
            .unwrap();
        let home = crate::subscription::helper::helper_home(&ProviderKind::GrokSubscription, "grok", dir.path()).unwrap();
        std::fs::create_dir_all(home.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(dir.path(), &home).unwrap();
        drop(store);
        let reopened = ConfigStore::load_with_adapters_and_auth(
            path,
            dispatcher(),
            std::sync::Arc::new(crate::subscription::UnavailableAdapter),
            auth(),
        )
        .unwrap();
        assert_eq!(reopened.read().subscriptions["grok"].state, crate::subscription::ConnectionState::NotConnected);
        assert!(home.is_symlink(), "清理失败时不得改动被拒绝的路径");
    }

    #[test]
    fn legacy_decision_provider_defaults_to_openrouter_and_unknown_values_fail() {
        let mut old = serde_json::to_value(AppConfig::default().policy).unwrap();
        old.as_object_mut().unwrap().remove("decision_provider");
        let restored: RoutingPolicy = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(restored.decision_provider, DecisionProvider::Openrouter);
        old["decision_provider"] = serde_json::json!("unknown");
        assert!(serde_json::from_value::<RoutingPolicy>(old).is_err());
    }

    #[test]
    fn concurrent_provider_changes_never_pair_a_config_with_the_other_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(ConfigStore::load(dir.path().join("decision.db")).unwrap());
        store.write_secret("autojev-cloud", "openrouter-key").unwrap();
        let writing = store.clone();
        let writer = std::thread::spawn(move || {
            for _ in 0..100 {
                for (provider, key) in [
                    (DecisionProvider::Zenmux, "zenmux-key"),
                    (DecisionProvider::Openrouter, "openrouter-key"),
                ] {
                    writing.update_checked(
                        |config| { config.policy.decision_provider = provider; Ok(()) },
                        Some(("autojev-cloud", "autojev-cloud", Some(key))),
                    ).unwrap();
                }
            }
        });
        for _ in 0..200 {
            let (config, key) = store.read_with_decision_key();
            let expected = match config.policy.decision_provider {
                DecisionProvider::Openrouter => "openrouter-key",
                DecisionProvider::Zenmux => "zenmux-key",
            };
            assert_eq!(key.as_deref(), Some(expected));
        }
        writer.join().unwrap();
    }

    #[test]
    fn initializes_database_and_persists_updates() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("autojev.db");
        let store = ConfigStore::load(db.clone()).unwrap();
        let install_id = store.read().install_id;
        store.update(|c| { c.port = 9999; c.providers.clear(); c.models.clear(); }).unwrap();
        drop(store);
        let reopened = ConfigStore::load(db).unwrap();
        assert_eq!(reopened.read().install_id, install_id);
        assert_eq!(reopened.read().port, 9999);
        assert!(reopened.read().providers.is_empty());
        assert!(reopened.read().models.is_empty());
    }

    #[test]
    fn failed_write_keeps_memory_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("autojev.db");
        let store = ConfigStore::load(db.clone()).unwrap();
        Connection::open(db).unwrap().execute_batch("DROP TABLE app_meta;").unwrap();
        let before = store.read().port;
        assert!(store.update(|c| c.port = 9999).is_err());
        assert_eq!(store.read().port, before);
    }
    #[test]
    fn checked_edit_moves_credentials_atomically_and_rolls_back_on_failure() {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("autojev.db");
        let store=ConfigStore::load(path.clone()).unwrap();
        store.write_secret("provider:old","key").unwrap();
        let before=store.read().port;
        assert!(store.update_checked(|c| {c.port=1; anyhow::bail!("conflict")},Some(("provider:old","provider:new",None))).is_err());
        assert_eq!(store.read().port,before);
        assert_eq!(store.read_secret("provider:old").as_deref(),Some("key"));
        assert!(store.read_secret("provider:new").is_none());
        store.update_checked(|c| {c.port=9999;Ok(())},Some(("provider:old","provider:new",None))).unwrap();
        assert!(store.read_secret("provider:old").is_none());
        assert_eq!(store.read_secret("provider:new").as_deref(),Some("key"));
        Connection::open(&path).unwrap().execute_batch("DROP TABLE app_meta;").unwrap();
        assert!(store.update_checked(|c| {c.port=2;Ok(())},Some(("provider:new","provider:failed",Some("new-key")))).is_err());
        assert_eq!(store.read().port,9999);
        assert_eq!(store.read_secret("provider:new").as_deref(),Some("key"));
        assert!(store.read_secret("provider:failed").is_none());
    }

    #[test]
    fn credentials_persist_and_stay_out_of_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autojev.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        store.write_secret("provider:test", "fixture-key").unwrap();
        assert!(!serde_json::to_string(&store.read()).unwrap().contains("fixture-key"));
        drop(store);
        let store = ConfigStore::load(path).unwrap();
        assert_eq!(store.read_secret("provider:test").as_deref(), Some("fixture-key"));
        store.write_secret("provider:test", "replacement-key").unwrap();
        assert_eq!(store.read_secret("provider:test").as_deref(), Some("replacement-key"));
        let other = ConfigStore::load(dir.path().join("other.db")).unwrap();
        assert!(other.read_secret("provider:test").is_none());
        store.delete_secret("provider:test").unwrap();
        assert!(store.read_secret("provider:test").is_none());
    }

    #[test]
    fn model_protocol_and_cache_price_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        store.update(|config| {
            config.models[0].api_type = "messages".into();
            config.models[0].cache_cost_per_million = 0.025;
        }).unwrap();
        let model = ConfigStore::load(path).unwrap().read().models[0].clone();
        assert_eq!(model.api_type, "messages");
        assert_eq!(model.cache_cost_per_million, 0.025);
    }

}

#[cfg(test)]
mod request_log_tests {
    use super::*;
    #[test]
    fn request_lookup_is_exact_and_missing_metadata_stays_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::load(dir.path().join("debug.db")).unwrap();
        for (id, reason) in [("request-a", "Jev choice"), ("request-b", "Session reuse")] {
            store.save_request_log(&crate::traffic::RequestLog {
                id: id.into(), reason: reason.into(), ..Default::default()
            }).unwrap();
        }
        assert_eq!(store.request_log("request-a").unwrap().unwrap().reason, "Jev choice");
        assert_eq!(store.request_log("request-b").unwrap().unwrap().reason, "Session reuse");
        assert!(store.request_log("missing").unwrap().is_none());
    }
    #[test]
    fn migrates_legacy_events_without_duplicates_or_invented_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let store = ConfigStore::load(path.clone()).unwrap();
        store.add_event(RouteEvent { id: "legacy".into(), created_at: "2026-09-20T00:00:00+00:00".into(),
            endpoint: "responses".into(), provider_name: "Provider".into(), model_name: "Model".into(),
            reason: "Legacy route".into(), source: "local".into(), estimated_input_tokens: 500,
            estimated_cost: Some(0.1), estimated_savings: 0., success: true }).unwrap();
        let reopened = ConfigStore::load(path.clone()).unwrap();
        let mut logs = reopened.request_logs("").unwrap();
        assert_eq!(logs.len(), 1); assert_eq!(logs[0].input_tokens, None);
        assert_eq!(logs[0].estimated_cost, None);
        logs[0].input_tokens = Some(42);
        reopened.save_request_log(&logs[0]).unwrap();
        let latest = ConfigStore::load(path).unwrap().request_logs("").unwrap();
        assert_eq!(latest.len(), 1); assert_eq!(latest[0].input_tokens, Some(42));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteModelSettings { pub priority: u32, pub weight: u32 }

#[cfg(test)]
mod legacy_config_storage_tests {
    use super::*;

    /// 模拟本票之前的落库内容：每个模型没有 `selected` 字段，根上没有 `subscription_catalogs`。
    /// 返回按旧库内容解析出的期望值，供加载后逐项比对。
    fn write_legacy_config(path: &std::path::Path) -> AppConfig {
        let mut legacy: serde_json::Value = serde_json::to_value(AppConfig::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("subscription_catalogs");
        for model in legacy["models"].as_array_mut().unwrap() {
            model.as_object_mut().unwrap().remove("selected");
        }
        // 旧库中用户停用过其中一个 API 模型：迁移不得把它重新启用。
        legacy["models"][1]["enabled"] = serde_json::json!(false);

        let db = Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO app_meta (key, value) VALUES ('config', ?1)", [legacy.to_string()]).unwrap();
        drop(db);

        serde_json::from_value(legacy).unwrap()
    }

    #[test]
    fn legacy_database_config_keeps_api_models_selected_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let expected = write_legacy_config(&path);

        let store = ConfigStore::load(path.clone()).unwrap();
        let loaded = store.read();
        assert_eq!(loaded.models.len(), expected.models.len(), "迁移不增删模型行");
        assert!(loaded.subscription_catalogs.is_empty(), "旧库没有目录时为空");
        for (actual, original) in loaded.models.iter().zip(expected.models.iter()) {
            assert!(actual.selected, "旧 API 模型迁移后必须保持已选：{}", actual.model_id);
            assert_eq!(actual.enabled, original.enabled, "迁移不得改写停用状态：{}", actual.model_id);
            assert_eq!(actual.id, original.id, "迁移不得重建内部标识：{}", actual.model_id);
            assert_eq!(actual.model_id, original.model_id);
        }
        assert!(!loaded.models[1].enabled, "被停用的模型在旧库中保持停用");

        // 选择与停用写回后再重读，仍然保持。
        store
            .update(|config| {
                config.models[0].selected = true;
                config.models[1].selected = false;
                config.models[1].enabled = true;
            })
            .unwrap();
        let reopened = ConfigStore::load(path).unwrap().read();
        assert_eq!(reopened.models.len(), expected.models.len());
        assert!(reopened.models[0].selected, "已选状态重读后保持");
        assert!(!reopened.models[1].selected, "取消选择必须持久化");
        assert!(reopened.models[1].enabled, "重新启用必须持久化");
        assert!(reopened.subscription_catalogs.is_empty());
    }
}
