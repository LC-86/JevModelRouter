//! 订阅服务商边界：每家一个活动连接、证据绑定连接世代，以及所有生成入口共用的 fail-closed 准入。
//!
//! 本模块不依赖 Tauri 或 HTTP 框架，连接、证据、准入与只读刷新都能直接单测。生产默认没有任何
//! 可用适配器（[`UnavailableAdapter`]），因此未验证的订阅生成一律拒绝；替身只能由测试构造，
//! 配置与界面都没有把它换成替身的开关。

use anyhow::{bail, Result};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{
    config::{AppConfig, ConfigStore, Model, Provider, ProviderKind},
    protocol::Protocol,
};

/// 订阅服务商与 API 服务商的身份是分开的：API 服务商继续走既有 HTTP 凭据，不进入本模块准入。
pub fn is_subscription(kind: &ProviderKind) -> bool {
    matches!(
        kind,
        ProviderKind::CodexSubscription | ProviderKind::GrokSubscription
    )
}

pub fn is_subscription_provider(provider: &Provider) -> bool {
    is_subscription(&provider.kind)
}

/// 面向界面与错误的服务商名称。
pub fn label(provider: &Provider) -> &'static str {
    match provider.kind {
        ProviderKind::CodexSubscription => "Codex",
        ProviderKind::GrokSubscription => "Grok",
        _ => "API",
    }
}

/// 连接的授权状态。`Unknown` 不在此列：拿不到状态时保留上一次已核实结果并另标证据陈旧。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    #[default]
    NotConnected,
    AuthorizationPending,
    Connected,
    Expired,
}

/// 单项模型能力：发现模型不等于该项能力已验证。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    #[default]
    Unverified,
    Verified,
    Unsupported,
}

/// 只读证据的可用性。展示字段缺失与调用依据缺失分别处理，但两者都不编造数值。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    #[default]
    Unknown,
    Available,
    Stale,
    Failed,
    Unsupported,
}

pub fn protocol_key(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Chat => "chat_completions",
        Protocol::Responses => "responses",
        Protocol::Messages => "messages",
    }
}

/// 服务商目录中的一项。发现不代表已选，也不代表任何协议能力已验证。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub model_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub eligible: bool,
}

/// 单项能力证据：按上游模型标识与客户端协议分别记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub model_id: String,
    pub protocol: String,
    #[serde(default)]
    pub status: CapabilityStatus,
}

/// 额度证据。首版只表达“是否有当前可用的额度依据”，桶与窗口由后续票据补齐。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaEvidence {
    #[serde(default)]
    pub state: EvidenceState,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
}

impl Default for QuotaEvidence {
    fn default() -> Self {
        Self { state: EvidenceState::Unknown, source: None, observed_at: None }
    }
}

/// 一次只读读取的完整结果，整体绑定连接世代：世代不符即整体作废，不逐字段沿用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub generation: u64,
    /// 已核实账号标识；与连接当前身份不一致时整份证据作废。
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub helper_version: Option<String>,
    #[serde(default)]
    pub account_path: Option<String>,
    #[serde(default)]
    pub models: Vec<DiscoveredModel>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub quota: QuotaEvidence,
}

/// 每家服务商一个活动连接。换号与退出递增 `generation`，旧世代的结果一律不采用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    #[serde(default = "first_generation")]
    pub generation: u64,
    #[serde(default)]
    pub state: ConnectionState,
    /// 已核实的账号标识；缺失即未知，不推断、不编造。
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub evidence: Option<Evidence>,
}

fn first_generation() -> u64 {
    1
}

impl Default for Connection {
    fn default() -> Self {
        Self { generation: 1, state: ConnectionState::NotConnected, identity: None, evidence: None }
    }
}

impl Connection {
    /// 只有同时绑定当前世代与已核实账号的证据才算数。
    pub fn current_evidence(&self) -> Option<&Evidence> {
        self.evidence.as_ref().filter(|evidence| {
            evidence.account.is_some()
                && evidence.account == self.identity
                && evidence.generation == self.generation
        })
    }
}

/// 只读连接状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    pub identity: Option<String>,
    /// 辅助进程版本与专用账号路径：证据据此绑定版本与账号路径。
    pub helper_version: Option<String>,
    pub account_path: Option<String>,
}

/// 拒绝类别。界面与客户端按类别给出恢复动作，不对所有 4xx 一律重试。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialFamily {
    Disabled,
    NotConnected,
    NotEligible,
    Capability,
    Quota,
}

/// 一次被拒绝的生成准入：稳定的机器码 + 原因 + 恢复动作。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Denial {
    pub code: String,
    pub family: DenialFamily,
    pub message: String,
    pub recovery: String,
}

impl Denial {
    fn new(code: &str, family: DenialFamily, message: String, recovery: String) -> Self {
        Self { code: code.to_owned(), family, message, recovery }
    }

    /// 展示与日志共用的单行原因：订阅拒绝只能靠用户动作恢复，重试本身不会改变结果。
    pub fn summary(&self) -> String {
        format!("{} {}", self.message, self.recovery)
    }

    pub fn status(&self) -> u16 {
        match self.family {
            DenialFamily::Disabled | DenialFamily::NotEligible | DenialFamily::Capability => 403,
            DenialFamily::NotConnected => 428,
            DenialFamily::Quota => 429,
        }
    }

    /// 客户端协议形状的错误体，附带稳定 code、类别与恢复动作。
    pub fn body(&self, protocol: Protocol) -> serde_json::Value {
        let mut body = protocol.error(&self.summary());
        if let Some(detail) = body.get_mut("error").and_then(|error| error.as_object_mut()) {
            detail.insert("code".into(), serde_json::json!(self.code));
            detail.insert("autojev_family".into(), serde_json::json!(self.family));
            detail.insert("autojev_recovery".into(), serde_json::json!(self.recovery));
        }
        body
    }
}

fn disabled(provider: &Provider, code: &str) -> Denial {
    Denial::new(
        code,
        DenialFamily::Disabled,
        format!("{} subscription is disabled.", label(provider)),
        "Enable the provider or model in AutoJev → Providers or Models.".into(),
    )
}

// CHUNK-4

/// 连接级检查：服务商停用、连接状态、身份核实。返回绑定当前世代的连接。
fn connection_check<'a>(config: &'a AppConfig, provider: &Provider) -> Result<&'a Connection, Denial> {
    if !provider.enabled {
        return Err(disabled(provider, "provider_disabled"));
    }
    let name = label(provider);
    let Some(connection) = config.subscriptions.get(&provider.id) else {
        return Err(Denial::new(
            "not_connected",
            DenialFamily::NotConnected,
            format!("{name} has no saved subscription connection."),
            "Add the subscription provider in AutoJev → Providers.".into(),
        ));
    };
    match connection.state {
        ConnectionState::NotConnected => {
            return Err(Denial::new(
                "not_connected",
                DenialFamily::NotConnected,
                format!("{name} is not connected."),
                "Connect the account in AutoJev → Providers.".into(),
            ))
        }
        ConnectionState::AuthorizationPending => {
            return Err(Denial::new(
                "authorization_pending",
                DenialFamily::NotConnected,
                format!("{name} authorization is still pending."),
                "Finish or cancel the authorization before sending work.".into(),
            ))
        }
        ConnectionState::Expired => {
            return Err(Denial::new(
                "authorization_expired",
                DenialFamily::NotConnected,
                format!("{name} authorization has expired."),
                "Reauthorize the account in AutoJev → Providers.".into(),
            ))
        }
        ConnectionState::Connected => {}
    }
    if connection.identity.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(Denial::new(
            "identity_unverified",
            DenialFamily::NotConnected,
            format!("{name} connection has no verified account identity."),
            "Reauthorize so AutoJev can verify the returned account identity.".into(),
        ));
    }
    Ok(connection)
}

/// 证据必须整体绑定当前世代；世代不符时按“没有证据”处理，不逐字段沿用旧结果。
fn evidence_check<'a>(provider: &Provider, connection: &'a Connection) -> Result<&'a Evidence, Denial> {
    connection.current_evidence().ok_or_else(|| {
        Denial::new(
            "evidence_missing",
            DenialFamily::NotConnected,
            format!(
                "{} has no read-only evidence for the current connection generation.",
                label(provider)
            ),
            "Refresh the connection in AutoJev → Providers.".into(),
        )
    })
}

fn capability_denial(provider: &Provider, model_id: &str, protocol: Protocol, code: &str, status: &str) -> Denial {
    Denial::new(
        code,
        DenialFamily::Capability,
        format!(
            "{} has no verified {} capability for {} ({status}).",
            label(provider),
            protocol_key(protocol),
            model_id
        ),
        "Keep using an API provider for this request, or verify this model and protocol first.".into(),
    )
}

fn quota_denial(provider: &Provider, code: &str, state: EvidenceState) -> Denial {
    let reason = match state {
        EvidenceState::Unknown => "no quota evidence has been read for this connection",
        EvidenceState::Stale => "the quota evidence does not belong to the current connection generation",
        EvidenceState::Failed => "the last quota read failed",
        EvidenceState::Unsupported => "the helper exposes no machine-readable quota",
        EvidenceState::Available => "quota is available",
    };
    Denial::new(
        code,
        DenialFamily::Quota,
        format!("{} cannot prove current in-subscription usage: {reason}.", label(provider)),
        "Refresh the connection in AutoJev → Providers once the upstream account is reachable.".into(),
    )
}

fn evaluate(
    config: &AppConfig,
    provider: &Provider,
    model: Option<&Model>,
    model_id: &str,
    protocol: Protocol,
) -> Result<(), Denial> {
    if !is_subscription_provider(provider) {
        return Ok(());
    }
    if model.is_some_and(|model| !model.enabled) {
        return Err(disabled(provider, "model_disabled"));
    }
    let connection = connection_check(config, provider)?;
    let evidence = evidence_check(provider, connection)?;
    if !evidence.models.iter().any(|entry| entry.model_id == model_id && entry.eligible) {
        return Err(Denial::new(
            "model_not_eligible",
            DenialFamily::NotEligible,
            format!("{} does not list {model_id} as eligible for this account.", label(provider)),
            "Refresh the directory, or pick a model this account can use.".into(),
        ));
    }
    match evidence.capabilities.iter().find(|entry| entry.model_id == model_id && entry.protocol == protocol_key(protocol)) {
        Some(capability) if capability.status == CapabilityStatus::Verified => {}
        Some(capability) if capability.status == CapabilityStatus::Unsupported => {
            return Err(capability_denial(provider, model_id, protocol, "capability_unsupported", "marked unsupported"))
        }
        _ => return Err(capability_denial(provider, model_id, protocol, "capability_unverified", "not verified yet")),
    }
    match evidence.quota.state {
        EvidenceState::Available => Ok(()),
        EvidenceState::Stale => Err(quota_denial(provider, "quota_stale", EvidenceState::Stale)),
        EvidenceState::Failed => Err(quota_denial(provider, "quota_failed", EvidenceState::Failed)),
        EvidenceState::Unsupported => Err(quota_denial(provider, "quota_unsupported", EvidenceState::Unsupported)),
        EvidenceState::Unknown => Err(quota_denial(provider, "quota_unknown", EvidenceState::Unknown)),
    }
}

/// 网关、Debug、服务商测试、模型测试与手动测速共用的准入；API 服务商保持原行为。
pub fn admit_model(config: &AppConfig, model: &Model, provider: &Provider, protocol: Protocol) -> Result<(), Denial> {
    evaluate(config, provider, Some(model), &model.model_id, protocol)
}

/// 没有模型记录的目标（例如服务商弹窗里的测试模型）使用同一套连接、资格、能力与额度规则。
pub fn admit_target(config: &AppConfig, provider: &Provider, model_id: &str, protocol: Protocol) -> Result<(), Denial> {
    evaluate(config, provider, None, model_id, protocol)
}

/// 自动选路用的过滤条件：被拒绝的订阅模型不进入候选。
pub fn generation_ready(config: &AppConfig, model: &Model, protocol: Protocol) -> bool {
    config
        .providers
        .iter()
        .find(|provider| provider.id == model.provider_id)
        .is_some_and(|provider| admit_model(config, model, provider, protocol).is_ok())
}

/// 连接级拒绝原因，供界面在服务商行上直接显示。
pub fn connection_denial(config: &AppConfig, provider: &Provider) -> Option<Denial> {
    if !is_subscription_provider(provider) {
        return None;
    }
    connection_check(config, provider)
        .and_then(|connection| evidence_check(provider, connection).map(|_| ()))
        .err()
}

/// 一次生成交接：绑定服务商、连接世代、模型与协议。适配器只负责外部辅助进程，
/// 准入在调用前完成，所以这里没有任何“跳过检查”的参数。
///
/// 生产构建里这些生成侧类型刻意不被调用：能力未验证前，准入会先拒绝，因此生成边界只在
/// 测试的隔离替身中被驱动。它们不是可选的绕过开关，也没有对应的配置项。
#[allow(dead_code)]
pub struct GenerationRequest<'a> {
    pub provider_id: &'a str,
    pub generation: u64,
    pub model_id: &'a str,
    pub protocol: Protocol,
    pub body: serde_json::Value,
    pub streaming: bool,
}

/// 生成事件：辅助进程产出的上游输出。协议转换、捕获与错误仍由调用方负责。
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum GenerationEvent {
    Started { generation: u64 },
    Chunk(String),
    Finished { status: u16 },
}

#[allow(dead_code)]
pub type GenerationStream<'a> =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = GenerationEvent> + Send + 'a>>;

/// 登录启动结果：登录 ID 与（可选的）授权链接或设备码。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LoginStart {
    pub login_id: String,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
}

/// 一次登录尝试的终态。身份来自完成通知，并再次用只读读数核实。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginResult {
    Completed { identity: String },
    /// 辅助进程显式报告取消；编排同样接受这个终态（替身与验收会驱动它）。
    #[allow(dead_code)]
    Cancelled,
    Failed(String),
}

/// 远端撤销结果；本地清理与远端撤销分开记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteRevocation {
    Revoked,
    Failed,
    Unsupported,
}

/// 一次退出的结果：本地是否已清、远端撤销到了哪一步。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogoutOutcome {
    pub local_cleared: bool,
    pub remote: RemoteRevocation,
}

/// 辅助进程自述状态。生产里解析不到官方 `codex` 时 `available:false`，不 panic。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HelperStatus {
    pub available: bool,
    pub version: Option<String>,
    pub auth_home: Option<String>,
}

/// 登录阶段。挂起的 URL、user code 与错误只存在于内存态，不写进配置文件。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStage {
    #[default]
    Idle,
    Pending,
    Completed,
    Failed,
    Cancelled,
}

/// 本地清理结果的展示值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogoutLocal {
    Cleared,
    Retained,
}

/// 远端撤销的展示值；`unknown` 表示这次没能观察到远端结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogoutRemote {
    Revoked,
    Failed,
    Unsupported,
    Unknown,
}

/// 登录会话的展示视图；挂起链接、user code 与错误只出现在这里，不写进配置文件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubscriptionLoginView {
    pub stage: LoginStage,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub attempt: u32,
    pub generation: u64,
    pub error: Option<String>,
}

/// 退出的展示视图：本地清理与远端撤销分别记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubscriptionLogoutView {
    pub local: LogoutLocal,
    pub remote: LogoutRemote,
    pub observed_at: Option<String>,
}

/// 辅助进程的展示视图。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubscriptionHelperView {
    pub available: bool,
    pub version: Option<String>,
    pub auth_home: Option<String>,
}

/// 一次退出的记录；`remote: None` 即展示为 `unknown`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogoutRecord {
    pub local_cleared: bool,
    pub remote: Option<RemoteRevocation>,
    pub observed_at: Option<String>,
}

impl LogoutRecord {
    fn view(&self) -> SubscriptionLogoutView {
        SubscriptionLogoutView {
            local: if self.local_cleared { LogoutLocal::Cleared } else { LogoutLocal::Retained },
            remote: match self.remote {
                Some(RemoteRevocation::Revoked) => LogoutRemote::Revoked,
                Some(RemoteRevocation::Failed) => LogoutRemote::Failed,
                Some(RemoteRevocation::Unsupported) => LogoutRemote::Unsupported,
                None => LogoutRemote::Unknown,
            },
            observed_at: self.observed_at.clone(),
        }
    }
}

/// 一家服务商的内存登录会话：世代与尝试序号决定迟到结果是否还能落盘。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionSession {
    pub stage: LoginStage,
    pub attempt: u32,
    /// 当前尝试绑定的连接世代；0 表示没有尝试，读取时回落到连接世代。
    pub generation: u64,
    pub login_id: Option<String>,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub error: Option<String>,
    pub last_logout: Option<LogoutRecord>,
}

/// 会话表 + 最近一次辅助进程自述；两者都只存在于内存，不落盘。
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub sessions: HashMap<String, SubscriptionSession>,
    pub helper: HelperStatus,
}

impl SessionState {
    pub fn session(&self, provider_id: &str) -> Option<&SubscriptionSession> {
        self.sessions.get(provider_id)
    }
}

fn require_subscription_provider(store: &ConfigStore, provider_id: &str) -> Result<(), String> {
    store
        .read()
        .providers
        .iter()
        .any(|provider| provider.id == provider_id && is_subscription_provider(provider))
        .then_some(())
        .ok_or_else(|| format!("Unknown subscription provider: {provider_id}"))
}

fn connection_generation(store: &ConfigStore, provider_id: &str) -> u64 {
    store
        .read()
        .subscriptions
        .get(provider_id)
        .map(|connection| connection.generation)
        .unwrap_or_default()
}

/// 适配器文本进入界面或日志前统一脱敏。
fn sanitize_error(message: &str) -> String {
    crate::codex_helper::redact(message)
}

/// 只在会话仍是同一尝试时改写其终态；否则说明结果已过期，调用方应整体丢弃。
fn finish_session(
    state: &mut SessionState,
    provider_id: &str,
    generation: u64,
    attempt: u32,
    stage: LoginStage,
    error: Option<String>,
) -> bool {
    let Some(session) = state.sessions.get_mut(provider_id) else {
        return false;
    };
    if session.stage != LoginStage::Pending || session.generation != generation || session.attempt != attempt
    {
        return false;
    }
    session.stage = stage;
    session.login_id = None;
    session.authorization_url = None;
    session.user_code = None;
    session.error = error;
    true
}

async fn pending_attempt(
    sessions: &tokio::sync::Mutex<SessionState>,
    provider_id: &str,
) -> Option<(u64, u32)> {
    let state = sessions.lock().await;
    state
        .session(provider_id)
        .filter(|session| session.stage == LoginStage::Pending)
        .map(|session| (session.generation, session.attempt))
}

/// 只在新世代仍是当前世代时绑定已核实身份；证据必须重新读取，不沿用旧账号。
fn bind_identity(store: &ConfigStore, provider_id: &str, generation: u64, identity: &str) -> Result<(), String> {
    store
        .update(|config| {
            let Some(connection) = config.subscriptions.get_mut(provider_id) else { return };
            if connection.generation != generation {
                return;
            }
            connection.state = ConnectionState::Connected;
            connection.identity = Some(identity.to_owned());
            connection.evidence = None;
        })
        .map_err(|error| error.to_string())
}

/// 失败/取消的终态：只在仍是挂起态时回到未连接，绝不复活更早的身份。
fn settle_not_connected(store: &ConfigStore, provider_id: &str, generation: u64) -> Result<(), String> {
    store
        .update(|config| {
            let Some(connection) = config.subscriptions.get_mut(provider_id) else { return };
            if connection.generation != generation {
                return;
            }
            if connection.state == ConnectionState::AuthorizationPending {
                connection.state = ConnectionState::NotConnected;
            }
        })
        .map_err(|error| error.to_string())
}

/// 立即发起登录并把 pending 写进内存态。调用方随后 spawn [`await_login`] 等完成通知，
/// 因此本函数不会阻塞在浏览器授权上，挂起链接只存在于内存。
pub async fn begin_login(
    store: &ConfigStore,
    provider_id: &str,
    sessions: &tokio::sync::Mutex<SessionState>,
) -> Result<LoginStart, String> {
    require_subscription_provider(store, provider_id)?;
    let generation = connection_generation(store, provider_id);
    let start = store
        .subscription
        .start_login(provider_id, generation)
        .await
        .map_err(|error| sanitize_error(&error.to_string()))?;
    // 进入非连接态：pending 期间生成准入与只读刷新都不得把连接当成已建立。
    store
        .update(|config| {
            if let Some(connection) = config.subscriptions.get_mut(provider_id) {
                connection.state = ConnectionState::AuthorizationPending;
            }
        })
        .map_err(|error| error.to_string())?;
    let mut state = sessions.lock().await;
    let session = state.sessions.entry(provider_id.to_owned()).or_default();
    session.stage = LoginStage::Pending;
    session.attempt += 1;
    session.generation = generation;
    session.login_id = Some(start.login_id.clone());
    session.authorization_url = start.authorization_url.clone();
    session.user_code = start.user_code.clone();
    session.error = None;
    Ok(start)
}

/// 取消挂起登录：先把会话置为取消终态，迟到完成通知不可能再把它改回 completed。
/// 会话表里可能没有该服务商（挂起期间进程重启过）：此时不伪造会话，
/// 用连接的当前世代兜底把落盘的 `authorization_pending` 归位成未连接，幂等返回 `Ok`。
pub async fn cancel_login(
    store: &ConfigStore,
    provider_id: &str,
    sessions: &tokio::sync::Mutex<SessionState>,
) -> Result<(), String> {
    require_subscription_provider(store, provider_id)?;
    let recorded = {
        let mut state = sessions.lock().await;
        match state.sessions.get_mut(provider_id) {
            Some(session) => {
                if session.stage == LoginStage::Pending {
                    session.stage = LoginStage::Cancelled;
                    session.login_id = None;
                    session.authorization_url = None;
                    session.user_code = None;
                    session.error = None;
                }
                session.generation
            }
            None => 0,
        }
    };
    let generation = if recorded == 0 { connection_generation(store, provider_id) } else { recorded };
    let result = store.subscription.cancel_login(provider_id, generation).await;
    settle_not_connected(store, provider_id, generation)?;
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            let message = sanitize_error(&error.to_string());
            let mut state = sessions.lock().await;
            if let Some(session) = state.sessions.get_mut(provider_id) {
                session.error = Some(message.clone());
            }
            Err(message)
        }
    }
}

/// 进程启动时把无法继续的挂起登录归位：挂起链接与登录会话只存在于内存，
/// 重启后不可能还有活的挂起尝试。只把 `AuthorizationPending` 改回 `NotConnected`，
/// 不改变世代，也不清除已有身份。
pub fn reconcile_orphaned_pending(store: &ConfigStore) -> Result<(), String> {
    store
        .update(|config| {
            for connection in config.subscriptions.values_mut() {
                if connection.state == ConnectionState::AuthorizationPending {
                    connection.state = ConnectionState::NotConnected;
                }
            }
        })
        .map_err(|error| error.to_string())
}

/// 后台等待一次已启动登录的终态。结果整体绑定 `(provider_id, generation, attempt)`：
/// 世代或尝试在完成前变化时一律丢弃，不写入新世代、不复活旧账号。
pub async fn await_login(
    store: std::sync::Arc<ConfigStore>,
    provider_id: String,
    sessions: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
) -> Result<LoginResult, String> {
    let Some((generation, attempt)) = pending_attempt(&sessions, &provider_id).await else {
        return Err("No pending subscription login for this provider".to_owned());
    };
    let outcome = match store.subscription.login_result(&provider_id, generation).await {
        Ok(Some(outcome)) => outcome,
        Ok(None) => return Err("No login result was available; nothing was written".to_owned()),
        Err(error) => {
            let message = sanitize_error(&error.to_string());
            let mut state = sessions.lock().await;
            if finish_session(&mut state, &provider_id, generation, attempt, LoginStage::Failed, Some(message.clone())) {
                drop(state);
                settle_not_connected(&store, &provider_id, generation)?;
            }
            return Err(message);
        }
    };
    let mut state = sessions.lock().await;
    let bound = state.sessions.get(&provider_id).is_some_and(|session| {
        session.stage == LoginStage::Pending && session.generation == generation && session.attempt == attempt
    });
    if !bound || connection_generation(&store, &provider_id) != generation {
        return Err("The login attempt changed before the result arrived; the result was discarded".to_owned());
    }
    match outcome {
        LoginResult::Completed { identity } => {
            let identity = identity.trim().to_owned();
            if identity.is_empty() {
                finish_session(
                    &mut state,
                    &provider_id,
                    generation,
                    attempt,
                    LoginStage::Failed,
                    Some("The Codex helper returned an empty account identity".to_owned()),
                );
                drop(state);
                settle_not_connected(&store, &provider_id, generation)?;
                return Err("The Codex helper returned an empty account identity".to_owned());
            }
            finish_session(&mut state, &provider_id, generation, attempt, LoginStage::Completed, None);
            drop(state);
            bind_identity(&store, &provider_id, generation, &identity)?;
            Ok(LoginResult::Completed { identity })
        }
        LoginResult::Cancelled => {
            finish_session(&mut state, &provider_id, generation, attempt, LoginStage::Cancelled, None);
            drop(state);
            settle_not_connected(&store, &provider_id, generation)?;
            Ok(LoginResult::Cancelled)
        }
        LoginResult::Failed(message) => {
            let message = sanitize_error(&message);
            finish_session(&mut state, &provider_id, generation, attempt, LoginStage::Failed, Some(message.clone()));
            drop(state);
            settle_not_connected(&store, &provider_id, generation)?;
            Ok(LoginResult::Failed(message))
        }
    }
}

/// 退出：先阻止新请求并递增世代、清空身份/额度/目录/能力缓存，再调辅助进程退出。
/// 服务商、模型、路由与 API 配置原样保留；本地与远端结果分别记录。
pub async fn logout(
    store: &ConfigStore,
    provider_id: &str,
    sessions: &tokio::sync::Mutex<SessionState>,
) -> Result<LogoutOutcome, String> {
    require_subscription_provider(store, provider_id)?;
    let generation = store
        .update(|config| {
            if !config.subscriptions.contains_key(provider_id) {
                config.subscriptions.insert(provider_id.to_owned(), Connection::default());
                return 1;
            }
            let connection = config.subscriptions.get_mut(provider_id).expect("checked above");
            connection.generation += 1;
            let generation = connection.generation;
            connection.state = ConnectionState::NotConnected;
            connection.identity = None;
            connection.evidence = None;
            generation
        })
        .map_err(|error| error.to_string())?;
    let previous_generation = generation.saturating_sub(1);
    let adapter = store.subscription.logout(provider_id, previous_generation).await;
    let (local_cleared, remote) = match adapter {
        Ok(outcome) => (outcome.local_cleared, Some(outcome.remote)),
        // 远端结果无法观察：本地连接已清，但远端撤销结果如实标记为未知。
        Err(_) => (true, None),
    };
    let record = LogoutRecord {
        local_cleared,
        remote,
        observed_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    {
        let mut state = sessions.lock().await;
        let session = state.sessions.entry(provider_id.to_owned()).or_default();
        // 挂起登录在退出时一并作废：迟到完成通知不能再写入连接。
        session.stage = LoginStage::Idle;
        session.generation = generation;
        session.login_id = None;
        session.authorization_url = None;
        session.user_code = None;
        session.error = None;
        session.last_logout = Some(record);
    }
    Ok(LogoutOutcome { local_cleared, remote: remote.unwrap_or(RemoteRevocation::Failed) })
}

/// 换号 = 退出 + 立即重新登录。退出已清掉旧身份；换号失败不会自动恢复旧账号。
pub async fn switch_account(
    store: &ConfigStore,
    provider_id: &str,
    sessions: &tokio::sync::Mutex<SessionState>,
) -> Result<LoginStart, String> {
    logout(store, provider_id, sessions).await?;
    begin_login(store, provider_id, sessions).await
}

/// 订阅适配边界：只读状态、模型、额度，以及绑定账号与连接世代的生成交接。
/// 实现只在构造 `ConfigStore` 时注入，配置与界面都无法替换它。
pub trait SubscriptionAdapter: Send + Sync {
    /// 是否存在可用的官方辅助进程。缺少它不影响只读刷新本身，只影响能读到什么。
    fn available(&self) -> bool;
    /// 辅助进程自述状态：解析不到官方 `codex` 时如实报不可用，绝不 panic。
    fn helper_status(&self) -> HelperStatus;
    fn status<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>>;
    fn models<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<Vec<DiscoveredModel>>>;
    fn quota<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>>;
    #[allow(dead_code)]
    fn generate<'a>(&'a self, request: GenerationRequest<'a>) -> BoxFuture<'a, Result<GenerationStream<'a>>>;
    /// 发起一次浏览器登录；世代与尝试一起绑定，结果迟到即整体丢弃。
    fn start_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<LoginStart>>;
    /// 等待该登录尝试的终态；世代或尝试已变时返回 `Ok(None)`。
    fn login_result<'a>(&'a self, provider_id: &'a str, generation: u64)
        -> BoxFuture<'a, Result<Option<LoginResult>>>;
    fn cancel_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<()>>;
    fn logout<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<LogoutOutcome>>;
}

const UNAVAILABLE: &str = "No subscription helper is available in this build";

/// 生产默认实现：尚未管理任何官方辅助进程，只读查询如实报不可用，生成一律拒绝。
/// 没有任何配置、环境变量或界面开关可以把它换成替身；替身只能由测试注入。
pub struct UnavailableAdapter;

impl SubscriptionAdapter for UnavailableAdapter {
    fn available(&self) -> bool {
        false
    }

    fn helper_status(&self) -> HelperStatus {
        HelperStatus::default()
    }

    fn status<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Vec<DiscoveredModel>>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn generate<'a>(&'a self, _request: GenerationRequest<'a>) -> BoxFuture<'a, Result<GenerationStream<'a>>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn start_login<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LoginStart>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn login_result<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Option<LoginResult>>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn cancel_login<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn logout<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LogoutOutcome>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }
}

/// 服务商保存后同步连接记录：订阅服务商得到一个活动连接，API 服务商不保留订阅身份。
pub fn sync_provider(config: &mut AppConfig, provider_id: &str, kind: &ProviderKind) {
    if is_subscription(kind) {
        config.subscriptions.entry(provider_id.to_owned()).or_default();
    } else {
        config.subscriptions.remove(provider_id);
    }
}

/// 服务商标识重命名时迁移连接，保留世代与已核实身份。
pub fn rename_provider(config: &mut AppConfig, old_id: &str, new_id: &str) {
    if old_id == new_id {
        return;
    }
    if let Some(connection) = config.subscriptions.remove(old_id) {
        config.subscriptions.insert(new_id.to_owned(), connection);
    }
}

/// 删除服务商时一并丢弃其连接与证据。
pub fn forget_provider(config: &mut AppConfig, provider_id: &str) {
    config.subscriptions.remove(provider_id);
}

// CHUNK-6

/// 只读刷新：按当前连接世代读取状态、模型与额度并写回证据。
/// 它与生成准入无关，因此生成被拒绝、网关暂停或服务商停用时仍可执行。
///
/// 读取失败时保留上一次已核实的证据，只把额度依据标为失败并如实返回错误；
/// 临时失败不会被伪装成可用，也不会被伪装成余额为零。
pub async fn refresh(store: &ConfigStore, provider_id: &str) -> Result<Connection, String> {
    store
        .read()
        .providers
        .iter()
        .find(|provider| provider.id == provider_id && is_subscription_provider(provider))
        .ok_or_else(|| format!("Unknown subscription provider: {provider_id}"))?;
    let generation = store
        .read()
        .subscriptions
        .get(provider_id)
        .map(|connection| connection.generation)
        .unwrap_or_default();
    let pending = store
        .read()
        .subscriptions
        .get(provider_id)
        .is_some_and(|connection| connection.state == ConnectionState::AuthorizationPending);
    if pending {
        // 挂起登录期间只读刷新不得伪装成已连接，也不得让旧身份复活：一个连接字段都不改写。
        return Ok(store.read().subscriptions.get(provider_id).cloned().unwrap_or_default());
    }
    let reads = async {
        let status = store.subscription.status(provider_id, generation).await.map_err(|error| error.to_string())?;
        let models = store.subscription.models(provider_id, generation).await.map_err(|error| error.to_string())?;
        let quota = store.subscription.quota(provider_id, generation).await.map_err(|error| error.to_string())?;
        Ok::<_, String>((status, models, quota))
    }
    .await;
    let (status, models, quota) = match reads {
        Ok(reads) => reads,
        Err(error) => {
            store
                .update(|config| {
                    let Some(connection) = config.subscriptions.get_mut(provider_id) else { return };
                    if connection.generation != generation {
                        return;
                    }
                    if let Some(evidence) = connection.evidence.as_mut().filter(|evidence| evidence.generation == generation) {
                        evidence.quota = QuotaEvidence { state: EvidenceState::Failed, source: None, observed_at: None };
                    }
                })
                .map_err(|save| save.to_string())?;
            return Err(error);
        }
    };
    store.update(|config| -> Result<Connection, String> {
        // 服务商可能在读取期间被删除或换号：迟到的只读结果不得复活或污染连接。
        if !config.providers.iter().any(|provider| provider.id == provider_id && is_subscription_provider(provider)) {
            return Err("The subscription provider was removed while refreshing".into());
        }
        let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
        if connection.generation != generation {
            return Err("The connection changed while refreshing; the read-only result was discarded".into());
        }
        let identity = status.identity.filter(|identity| !identity.trim().is_empty());
        let capabilities = connection
            .current_evidence()
            .map(|evidence| evidence.capabilities.clone())
            .unwrap_or_default();
        connection.state = status.state;
        connection.identity = identity.clone();
        connection.evidence = Some(Evidence {
            generation,
            account: identity,
            helper_version: status.helper_version,
            account_path: status.account_path,
            models,
            capabilities,
            quota,
        });
        Ok(connection.clone())
    })
    .map_err(|error| error.to_string())?
}

/// 界面视图：每家订阅服务商的实际状态、只读证据与当前拒绝原因。
#[derive(Clone, Debug, Serialize)]
pub struct SubscriptionView {
    pub provider_id: String,
    pub label: String,
    pub generation: u64,
    pub state: ConnectionState,
    pub identity: Option<String>,
    pub helper_version: Option<String>,
    pub account_path: Option<String>,
    pub models: Vec<DiscoveredModel>,
    pub capabilities: Vec<Capability>,
    pub quota: QuotaEvidence,
    pub denial: Option<Denial>,
    pub adapter_available: bool,
    /// 登录会话（内存态）：阶段、挂起链接、尝试序号与绑定世代。
    pub login: SubscriptionLoginView,
    /// 最近一次退出：本地与远端结果分别记录；从未退出过时为 `None`。
    pub logout: Option<SubscriptionLogoutView>,
    /// 辅助进程自述状态。
    pub helper: SubscriptionHelperView,
}

pub fn views(config: &AppConfig, adapter_available: bool, sessions: &SessionState) -> Vec<SubscriptionView> {
    let mut items: Vec<_> = config
        .providers
        .iter()
        .filter(|provider| is_subscription_provider(provider))
        .map(|provider| {
            let connection = config.subscriptions.get(&provider.id).cloned().unwrap_or_default();
            let evidence = connection.current_evidence();
            let session = sessions.session(&provider.id);
            SubscriptionView {
                provider_id: provider.id.clone(),
                label: label(provider).to_owned(),
                generation: connection.generation,
                state: connection.state,
                identity: connection.identity.clone(),
                helper_version: evidence.and_then(|evidence| evidence.helper_version.clone()),
                account_path: evidence.and_then(|evidence| evidence.account_path.clone()),
                models: evidence.map(|evidence| evidence.models.clone()).unwrap_or_default(),
                capabilities: evidence.map(|evidence| evidence.capabilities.clone()).unwrap_or_default(),
                quota: evidence.map(|evidence| evidence.quota.clone()).unwrap_or_default(),
                denial: connection_denial(config, provider),
                adapter_available,
                login: SubscriptionLoginView {
                    stage: session.map(|session| session.stage).unwrap_or_default(),
                    authorization_url: session.and_then(|session| session.authorization_url.clone()),
                    user_code: session.and_then(|session| session.user_code.clone()),
                    attempt: session.map(|session| session.attempt).unwrap_or_default(),
                    generation: session
                        .map(|session| session.generation)
                        .filter(|generation| *generation != 0)
                        .unwrap_or(connection.generation),
                    error: session.and_then(|session| session.error.clone()),
                },
                logout: session.and_then(|session| session.last_logout.as_ref()).map(LogoutRecord::view),
                helper: SubscriptionHelperView {
                    available: sessions.helper.available,
                    version: sessions.helper.version.clone(),
                    auth_home: sessions.helper.auth_home.clone(),
                },
            }
        })
        .collect();
    items.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
    items
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::config::{Model, ModelTier, ProviderKind};

    const FIXTURE_PROVIDER: &str = "fixture-subscription";

    fn provider(id: &str, kind: ProviderKind) -> Provider {
        Provider {
            preset: String::new(),
            api_type: String::new(),
            test_model: String::new(),
            id: id.into(),
            name: format!("{id} subscription"),
            kind,
            base_url: String::new(),
            enabled: true,
            has_api_key: false,
        }
    }

    fn model(id: &str, provider_id: &str) -> Model {
        Model {
            input_price_known: None,
            output_price_known: None,
            cache_price_known: None,
            api_type: String::new(),
            cache_cost_per_million: 0.0,
            id: id.into(),
            provider_id: provider_id.into(),
            model_id: "fixture-model".into(),
            name: "Fixture".into(),
            tier: ModelTier::Balanced,
            enabled: true,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: false,
            context_window: 1000,
            input_cost_per_million: 0.0,
            output_cost_per_million: 0.0,
        }
    }

    /// 一个已核实到可派发程度的 Codex 连接；各用例只改动其中一项。
    fn connected(config: &mut AppConfig) {
        let connection = config.subscriptions.entry(FIXTURE_PROVIDER.to_owned()).or_default();
        let generation = connection.generation;
        connection.state = ConnectionState::Connected;
        connection.identity = Some("fixture@example.invalid".into());
        connection.evidence = Some(Evidence {
            generation,
            account: connection.identity.clone(),
            helper_version: Some("fixture-helper-1.0".into()),
            account_path: Some("/tmp/fixture-account".into()),
            models: vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
            capabilities: vec![Capability {
                model_id: "fixture-model".into(),
                protocol: protocol_key(Protocol::Chat).into(),
                status: CapabilityStatus::Verified,
            }],
            quota: QuotaEvidence {
                state: EvidenceState::Available,
                source: Some("fixture".into()),
                observed_at: Some("2026-09-30T00:00:00Z".into()),
            },
        });
    }

    fn fixture(kind: ProviderKind) -> (AppConfig, Provider, Model) {
        let mut config = AppConfig::default();
        let provider = provider("fixture-subscription", kind);
        let model = model("fixture-model-record", &provider.id);
        config.providers.push(provider.clone());
        config.models.push(model.clone());
        sync_provider(&mut config, &provider.id, &provider.kind);
        (config, provider, model)
    }

    fn denial(config: &AppConfig, model: &Model, provider: &Provider) -> Denial {
        admit_model(config, model, provider, Protocol::Chat).expect_err("subscription generation must stay denied")
    }

    #[test]
    fn api_providers_keep_their_existing_behaviour() {
        let mut config = AppConfig::default();
        let api_provider = config.providers[0].clone();
        let api_model = config.models[0].clone();
        assert!(!is_subscription_provider(&api_provider));
        assert!(admit_model(&config, &api_model, &api_provider, Protocol::Chat).is_ok());
        assert!(admit_target(&config, &api_provider, "unlisted-model", Protocol::Chat).is_ok());
        assert!(generation_ready(&config, &api_model, Protocol::Chat));
        // 复制出一份未连接的订阅服务商也不影响 API 服务商本身。
        config.providers.push(provider("codex", ProviderKind::CodexSubscription));
        assert!(admit_model(&config, &api_model, &api_provider, Protocol::Chat).is_ok());
    }

    #[test]
    fn every_denial_carries_a_stable_code_reason_and_recovery() {
        let (mut config, _, _) = fixture(ProviderKind::CodexSubscription);
        let cases: Vec<(&str, Box<dyn Fn(&mut AppConfig)>)> = vec![
            ("provider_disabled", Box::new(|config: &mut AppConfig| config.providers[2].enabled = false)),
            ("model_disabled", Box::new(|config: &mut AppConfig| config.models[2].enabled = false)),
            ("not_connected", Box::new(|_| {})),
            ("authorization_pending", Box::new(|config: &mut AppConfig| { config.subscriptions.get_mut("fixture-subscription").unwrap().state = ConnectionState::AuthorizationPending; })),
            ("authorization_expired", Box::new(|config: &mut AppConfig| { config.subscriptions.get_mut("fixture-subscription").unwrap().state = ConnectionState::Expired; })),
            ("identity_unverified", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut(FIXTURE_PROVIDER).unwrap().identity = None;
            })),
            ("model_not_eligible", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().models.clear();
            })),
            ("capability_unverified", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().capabilities.clear();
            })),
            ("capability_unsupported", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().capabilities[0].status = CapabilityStatus::Unsupported;
            })),
            ("quota_unknown", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().quota.state = EvidenceState::Unknown;
            })),
            ("quota_stale", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().quota.state = EvidenceState::Stale;
            })),
            ("quota_failed", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().quota.state = EvidenceState::Failed;
            })),
            ("quota_unsupported", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().quota.state = EvidenceState::Unsupported;
            })),
        ];
        for (expected, change) in cases {
            change(&mut config);
            let provider = config.providers.iter().find(|entry| entry.id == FIXTURE_PROVIDER).unwrap().clone();
            let model = config.models.iter().find(|entry| entry.provider_id == FIXTURE_PROVIDER).unwrap().clone();
            let denial = denial(&config, &model, &provider);
            assert_eq!(denial.code, expected, "unexpected code for {expected}");
            assert!(!denial.message.is_empty() && !denial.recovery.is_empty());
            assert!((403..=503).contains(&denial.status()));
            let body = denial.body(Protocol::Chat);
            assert_eq!(body["error"]["code"], expected);
            assert!(body["error"]["message"].as_str().unwrap().contains(&denial.recovery));
            // 撤销本次改动，回到“未连接”基线。
            config = fixture(ProviderKind::CodexSubscription).0;
        }
    }

    #[test]
    fn a_fully_verified_connection_is_the_only_admitted_case() {
        let (mut config, provider, model) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        assert!(admit_model(&config, &model, &provider, Protocol::Chat).is_ok());
        assert!(generation_ready(&config, &model, Protocol::Chat));
        // 未验证的第二协议仍然被拒绝，能力按协议分别判定。
        let denial = admit_model(&config, &model, &provider, Protocol::Responses).unwrap_err();
        assert_eq!(denial.code, "capability_unverified");
    }

    #[test]
    fn evidence_from_another_generation_never_admits_generation() {
        let (mut config, provider, model) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        assert!(admit_model(&config, &model, &provider, Protocol::Chat).is_ok());
        let connection = config.subscriptions.get_mut("fixture-subscription").unwrap();
        connection.generation += 1; // 换号：旧证据整体作废。
        assert!(connection.current_evidence().is_none());
        let denial = denial(&config, &model, &provider);
        assert_eq!(denial.code, "evidence_missing");
    }

    #[test]
    fn evidence_from_another_account_never_admits_generation() {
        let (mut config, provider, model) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        assert!(admit_model(&config, &model, &provider, Protocol::Chat).is_ok());
        // 身份变了但证据仍属于旧账号：整份证据作废，不会被当成新账号已核实。
        let connection = config.subscriptions.get_mut(FIXTURE_PROVIDER).unwrap();
        connection.identity = Some("other@example.invalid".into());
        assert!(connection.current_evidence().is_none());
        assert_eq!(denial(&config, &model, &provider).code, "evidence_missing");
    }

    #[test]
    fn each_provider_keeps_its_own_connection_and_evidence() {
        let (mut config, codex, codex_model) = fixture(ProviderKind::CodexSubscription);
        let grok = provider("grok-fixture", ProviderKind::GrokSubscription);
        config.providers.push(grok.clone());
        config.models.push(model("grok-model-record", &grok.id));
        sync_provider(&mut config, &grok.id, &grok.kind);
        connected(&mut config);
        let grok_model = config.models.last().unwrap().clone();
        assert!(admit_model(&config, &codex_model, &codex, Protocol::Chat).is_ok());
        // Grok 没有自己的连接与证据，不会借用 Codex 的结果。
        assert_eq!(denial(&config, &grok_model, &grok).code, "not_connected");
        let views = views(&config, false, &SessionState::default());
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].provider_id, "fixture-subscription");
        assert_eq!(views[0].identity.as_deref(), Some("fixture@example.invalid"));
        assert!(views[1].identity.is_none() && views[1].denial.is_some());
        assert!(views.iter().all(|view| !view.adapter_available));
    }

    #[test]
    fn removing_or_converting_a_provider_drops_its_connection() {
        let (mut config, provider, _) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        rename_provider(&mut config, &provider.id, "codex-renamed");
        assert!(config.subscriptions.contains_key("codex-renamed"));
        assert!(!config.subscriptions.contains_key(&provider.id));
        sync_provider(&mut config, "codex-renamed", &ProviderKind::OpenaiCompatible);
        assert!(config.subscriptions.is_empty());
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use crate::config::ProviderKind;
    use futures_util::StreamExt;
    use std::sync::{Arc, Mutex};

    /// 只读替身：只在测试代码里存在，用来驱动适配边界本身。
    /// 它没有任何生产入口，配置与界面都无法把它装进应用。
    struct StubAdapter {
        status: ConnectionStatus,
        models: Vec<DiscoveredModel>,
        quota: QuotaEvidence,
        reads: Mutex<Vec<u64>>,
        store: Mutex<Option<Arc<ConfigStore>>>,
        switch_during_read: std::sync::atomic::AtomicBool,
        fail_reads: std::sync::atomic::AtomicBool,
    }

    impl StubAdapter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                status: ConnectionStatus {
                    state: ConnectionState::Connected,
                    identity: Some("fixture@example.invalid".into()),
                    helper_version: Some("fixture-helper-1.0".into()),
                    account_path: Some("/tmp/fixture-account".into()),
                },
                models: vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
                quota: QuotaEvidence {
                    state: EvidenceState::Available,
                    source: Some("fixture".into()),
                    observed_at: Some("2026-09-30T00:00:00Z".into()),
                },
                reads: Mutex::new(Vec::new()),
                store: Mutex::new(None),
                switch_during_read: std::sync::atomic::AtomicBool::new(false),
                fail_reads: std::sync::atomic::AtomicBool::new(false),
            })
        }
    }

    impl SubscriptionAdapter for StubAdapter {
        fn available(&self) -> bool {
            true
        }

        fn status<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
            Box::pin(async move {
                self.reads.lock().unwrap().push(generation);
                if self.fail_reads.load(std::sync::atomic::Ordering::SeqCst) {
                    bail!("fixture helper read failed");
                }
                if self.switch_during_read.load(std::sync::atomic::Ordering::SeqCst) {
                    if let Some(store) = self.store.lock().unwrap().clone() {
                        store.update(|config| {
                            if let Some(connection) = config.subscriptions.get_mut(provider_id) {
                                connection.generation += 1;
                            }
                        })?;
                    }
                }
                Ok(self.status.clone())
            })
        }

        fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Vec<DiscoveredModel>>> {
            Box::pin(async move { Ok(self.models.clone()) })
        }

        fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
            Box::pin(async move { Ok(self.quota.clone()) })
        }

        fn generate<'a>(&'a self, request: GenerationRequest<'a>) -> BoxFuture<'a, Result<GenerationStream<'a>>> {
            Box::pin(async move {
                let events = vec![
                    GenerationEvent::Started { generation: request.generation },
                    GenerationEvent::Chunk("fixture".into()),
                    GenerationEvent::Finished { status: 200 },
                ];
                let stream: GenerationStream<'a> = Box::pin(futures_util::stream::iter(events));
                Ok(stream)
            })
        }

        fn helper_status(&self) -> HelperStatus {
            HelperStatus {
                available: true,
                version: Some("fixture-helper-1.0".into()),
                auth_home: Some("/tmp/fixture-account".into()),
            }
        }

        fn start_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<LoginStart>> {
            Box::pin(async move {
                Ok(LoginStart {
                    login_id: format!("{provider_id}-{generation}"),
                    authorization_url: Some("https://example.invalid/auth".into()),
                    user_code: None,
                })
            })
        }

        fn login_result<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Option<LoginResult>>> {
            Box::pin(async { Ok(None) })
        }

        fn cancel_login<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn logout<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LogoutOutcome>> {
            Box::pin(async { Ok(LogoutOutcome { local_cleared: true, remote: RemoteRevocation::Revoked }) })
        }
    }

    fn fixture_store(adapter: Arc<StubAdapter>) -> (Arc<ConfigStore>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            ConfigStore::load_with_adapters(
                directory.path().join("autojev.db"),
                Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true }),
                adapter.clone(),
            )
            .unwrap(),
        );
        store
            .update(|config| {
                config.port = 0;
                let provider = Provider {
                    preset: String::new(),
                    api_type: String::new(),
                    test_model: String::new(),
                    id: "codex".into(),
                    name: "Codex".into(),
                    kind: ProviderKind::CodexSubscription,
                    base_url: String::new(),
                    enabled: true,
                    has_api_key: false,
                };
                config.providers.push(provider.clone());
                sync_provider(config, &provider.id, &provider.kind);
                let mut model = config.models[0].clone();
                model.id = "codex-model".into();
                model.provider_id = "codex".into();
                model.model_id = "fixture-model".into();
                config.models.push(model);
            })
            .unwrap();
        *adapter.store.lock().unwrap() = Some(store.clone());
        (store, directory)
    }

    fn target(config: &AppConfig) -> (Model, Provider) {
        let model = config.models.iter().find(|model| model.provider_id == "codex").unwrap().clone();
        let provider = config.providers.iter().find(|provider| provider.id == "codex").unwrap().clone();
        (model, provider)
    }

    #[tokio::test]
    async fn read_only_refresh_binds_evidence_to_the_current_generation() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        // 未连接时生成一律拒绝。
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "not_connected");
        let connection = refresh(&store, "codex").await.unwrap();
        assert_eq!(connection.state, ConnectionState::Connected);
        assert_eq!(connection.identity.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(connection.generation, 1);
        assert_eq!(adapter.reads.lock().unwrap().clone(), vec![1]);
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.generation, 1);
        assert_eq!(evidence.account.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(evidence.helper_version.as_deref(), Some("fixture-helper-1.0"));
        assert_eq!(evidence.account_path.as_deref(), Some("/tmp/fixture-account"));
        assert_eq!(evidence.models.len(), 1);
        assert_eq!(evidence.quota.state, EvidenceState::Available);
        // 证据齐备后仍然缺能力记录：未验证能力不会因为一次只读刷新而被视为已验证。
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "capability_unverified");
        let view = views(&store.read(), true, &SessionState::default()).pop().unwrap();
        assert_eq!(view.identity.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(view.generation, 1);
        assert!(view.adapter_available);
        // 连接级检查此时已通过；缺能力仍会挡在生成准入上。
        assert!(view.denial.is_none());
    }

    #[tokio::test]
    async fn read_only_refresh_keeps_working_while_the_gateway_is_paused() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        let gateway = crate::proxy::start(store.clone()).await.unwrap();
        gateway.set_paused(true);
        assert!(gateway.paused());
        // 生成暂停只影响生成：只读刷新照常读取并写回证据。
        let connection = refresh(&store, "codex").await.unwrap();
        assert_eq!(connection.current_evidence().unwrap().generation, 1);
        assert_eq!(adapter.reads.lock().unwrap().len(), 1);
        gateway.stop().await;
    }

    #[tokio::test]
    async fn a_failed_read_keeps_verified_evidence_and_marks_the_quota_basis_failed() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        // 能力验证由后续票据记录；这里先记一项已验证能力，好让本次断言落在额度依据上。
        store
            .update(|config| {
                let evidence = config.subscriptions.get_mut("codex").unwrap().evidence.as_mut().unwrap();
                evidence.capabilities.push(Capability {
                    model_id: "fixture-model".into(),
                    protocol: protocol_key(Protocol::Chat).into(),
                    status: CapabilityStatus::Verified,
                });
            })
            .unwrap();
        adapter.fail_reads.store(true, std::sync::atomic::Ordering::SeqCst);
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("fixture helper read failed"), "{error}");
        // 临时失败保留已核实的模型目录，只把额度依据标为失败：既不伪装可用，也不伪装成空余额。
        let connection = store.read().subscriptions.get("codex").cloned().unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.models.len(), 1);
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "quota_failed");
    }

    #[tokio::test]
    async fn a_late_result_from_another_generation_is_discarded() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        // 读取期间换号：迟到的只读结果不得写入新世代。
        adapter.switch_during_read.store(true, std::sync::atomic::Ordering::SeqCst);
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("changed while refreshing"), "{error}");
        let connection = store.read().subscriptions.get("codex").cloned().unwrap();
        assert_eq!(connection.generation, 2);
        assert!(connection.evidence.is_none());
    }

    #[tokio::test]
    async fn the_default_adapter_denies_reads_and_generation() {
        let adapter = UnavailableAdapter;
        assert!(!adapter.available());
        assert!(adapter.status("codex", 1).await.is_err());
        assert!(adapter.models("codex", 1).await.is_err());
        assert!(adapter.quota("codex", 1).await.is_err());
        assert!(adapter
            .generate(GenerationRequest {
                provider_id: "codex",
                generation: 1,
                model_id: "fixture-model",
                protocol: Protocol::Chat,
                body: serde_json::json!({}),
                streaming: false,
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_test_only_stub_drives_the_generation_boundary() {
        let adapter = StubAdapter::new();
        assert!(adapter.available());
        let mut stream = adapter
            .generate(GenerationRequest {
                provider_id: "codex",
                generation: 7,
                model_id: "fixture-model",
                protocol: Protocol::Chat,
                body: serde_json::json!({"model": "fixture-model"}),
                streaming: true,
            })
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        assert_eq!(
            events,
            vec![
                GenerationEvent::Started { generation: 7 },
                GenerationEvent::Chunk("fixture".into()),
                GenerationEvent::Finished { status: 200 },
            ]
        );
    }

    #[test]
    fn production_has_no_switch_to_install_a_stub() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::load(directory.path().join("autojev.db")).unwrap();
        assert!(!store.subscription.available());
        store
            .update(|config| {
                for (id, kind) in [("codex", ProviderKind::CodexSubscription), ("grok", ProviderKind::GrokSubscription)] {
                    let mut provider = config.providers[0].clone();
                    provider.id = id.into();
                    provider.name = id.into();
                    provider.base_url = String::new();
                    provider.kind = kind.clone();
                    config.providers.push(provider.clone());
                    sync_provider(config, &provider.id, &provider.kind);
                }
            })
            .unwrap();
        // 配置里没有任何字段能把它换成替身。
        assert!(!store.subscription.available());
        let views = views(&store.read(), store.subscription.available(), &SessionState::default());
        assert_eq!(views.len(), 2);
        assert!(views.iter().all(|view| !view.adapter_available && view.denial.is_some()));
    }
}






#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::config::{AppConfig, Model, Provider, ProviderKind};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// 生命周期替身：完成结果由测试显式投递，用来驱动成功/失败/取消/迟到结果与换号。
    /// 它只存在于测试代码里，生产构造没有任何入口能装入它。
    struct LifecycleStub {
        helper: HelperStatus,
        status: ConnectionStatus,
        start_error: Mutex<Option<String>>,
        started: Mutex<Vec<(String, u64)>>,
        cancels: Mutex<Vec<(String, u64)>>,
        logouts: Mutex<Vec<(String, u64)>>,
        logout_outcome: Mutex<std::result::Result<LogoutOutcome, String>>,
        completions: tokio::sync::Mutex<VecDeque<(String, u64, Option<LoginResult>)>>,
        notify: tokio::sync::Notify,
        late_completion_on_cancel: AtomicBool,
    }

    impl LifecycleStub {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                helper: HelperStatus {
                    available: true,
                    version: Some("fixture-helper-1.0".into()),
                    auth_home: Some("/tmp/fixture-helper-home".into()),
                },
                status: ConnectionStatus {
                    state: ConnectionState::Connected,
                    identity: Some("helper@example.invalid".into()),
                    helper_version: Some("fixture-helper-1.0".into()),
                    account_path: Some("/tmp/fixture-helper-home".into()),
                },
                start_error: Mutex::new(None),
                started: Mutex::new(Vec::new()),
                cancels: Mutex::new(Vec::new()),
                logouts: Mutex::new(Vec::new()),
                logout_outcome: Mutex::new(Ok(LogoutOutcome {
                    local_cleared: true,
                    remote: RemoteRevocation::Revoked,
                })),
                completions: tokio::sync::Mutex::new(VecDeque::new()),
                notify: tokio::sync::Notify::new(),
                late_completion_on_cancel: AtomicBool::new(false),
            })
        }

        async fn push_completion(&self, provider_id: &str, generation: u64, result: LoginResult) {
            self.completions
                .lock()
                .await
                .push_back((provider_id.to_owned(), generation, Some(result)));
            self.notify.notify_one();
        }
    }

    impl SubscriptionAdapter for LifecycleStub {
        fn available(&self) -> bool {
            true
        }

        fn helper_status(&self) -> HelperStatus {
            self.helper.clone()
        }

        fn status<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
            Box::pin(async move { Ok(self.status.clone()) })
        }

        fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Vec<DiscoveredModel>>> {
            Box::pin(async { Ok(vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }]) })
        }

        fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
            Box::pin(async {
                Ok(QuotaEvidence {
                    state: EvidenceState::Available,
                    source: Some("fixture".into()),
                    observed_at: Some("2026-09-30T00:00:00Z".into()),
                })
            })
        }

        fn generate<'a>(&'a self, _request: GenerationRequest<'a>) -> BoxFuture<'a, Result<GenerationStream<'a>>> {
            Box::pin(async { bail!("fixture generation is never allowed") })
        }

        fn start_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<LoginStart>> {
            Box::pin(async move {
                if let Some(error) = self.start_error.lock().unwrap().clone() {
                    bail!("{error}");
                }
                self.started.lock().unwrap().push((provider_id.to_owned(), generation));
                Ok(LoginStart {
                    login_id: format!("{provider_id}-{generation}"),
                    authorization_url: Some("https://example.invalid/authorize?state=fixture".into()),
                    user_code: None,
                })
            })
        }

        fn login_result<'a>(&'a self, provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Option<LoginResult>>> {
            Box::pin(async move {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    {
                        let mut queue = self.completions.lock().await;
                        if let Some(index) = queue.iter().position(|entry| entry.0 == provider_id) {
                            let entry = queue.remove(index).expect("found above");
                            return Ok(entry.2);
                        }
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Ok(None);
                    }
                    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), self.notify.notified()).await;
                }
            })
        }

        fn cancel_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.cancels.lock().unwrap().push((provider_id.to_owned(), generation));
                if self.late_completion_on_cancel.load(Ordering::SeqCst) {
                    self.push_completion(provider_id, generation, LoginResult::Completed { identity: "late@example.invalid".into() }).await;
                }
                Ok(())
            })
        }

        fn logout<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<LogoutOutcome>> {
            Box::pin(async move {
                self.logouts.lock().unwrap().push((provider_id.to_owned(), generation));
                let recorded = self.logout_outcome.lock().unwrap().clone();
                match recorded {
                    Ok(outcome) => Ok(outcome),
                    Err(message) => bail!("{message}"),
                }
            })
        }
    }

    async fn fixture(adapter: Arc<LifecycleStub>) -> (Arc<ConfigStore>, tempfile::TempDir, Arc<tokio::sync::Mutex<SessionState>>) {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            ConfigStore::load_with_adapters(
                directory.path().join("autojev.db"),
                Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true }),
                adapter,
            )
            .unwrap(),
        );
        store
            .update(|config| {
                let provider = Provider {
                    preset: String::new(),
                    api_type: String::new(),
                    test_model: String::new(),
                    id: "codex".into(),
                    name: "Codex".into(),
                    kind: ProviderKind::CodexSubscription,
                    base_url: String::new(),
                    enabled: true,
                    has_api_key: false,
                };
                config.providers.push(provider.clone());
                sync_provider(config, &provider.id, &provider.kind);
                let mut model = config.models[0].clone();
                model.id = "codex-model".into();
                model.provider_id = "codex".into();
                model.model_id = "fixture-model".into();
                config.models.push(model);
            })
            .unwrap();
        (store, directory, Arc::new(tokio::sync::Mutex::new(SessionState::default())))
    }

    fn target(config: &AppConfig) -> (Model, Provider) {
        let model = config.models.iter().find(|model| model.provider_id == "codex").unwrap().clone();
        let provider = config.providers.iter().find(|provider| provider.id == "codex").unwrap().clone();
        (model, provider)
    }

    async fn session(sessions: &Arc<tokio::sync::Mutex<SessionState>>, provider_id: &str) -> SubscriptionSession {
        sessions.lock().await.session(provider_id).cloned().unwrap_or_default()
    }

    /// 与 lib.rs 的 snapshot 一样：先把辅助进程自述写进会话表，再取视图。
    async fn view_of(
        store: &ConfigStore,
        adapter_available: bool,
        sessions: &Arc<tokio::sync::Mutex<SessionState>>,
    ) -> SubscriptionView {
        let mut guard = sessions.lock().await;
        guard.helper = store.subscription.helper_status();
        views(&store.read(), adapter_available, &guard).pop().unwrap()
    }

    fn connection(store: &ConfigStore, provider_id: &str) -> Connection {
        store.read().subscriptions.get(provider_id).cloned().unwrap_or_default()
    }

    #[tokio::test]
    async fn login_success_binds_identity_to_the_current_generation() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        assert!(store.subscription.helper_status().available);
        let start = begin_login(&store, "codex", &sessions).await.unwrap();
        assert_eq!(start.login_id, "codex-1");
        assert_eq!(start.authorization_url.as_deref(), Some("https://example.invalid/authorize?state=fixture"));
        assert_eq!(adapter.started.lock().unwrap().clone(), vec![("codex".into(), 1)]);
        let pending = session(&sessions, "codex").await;
        assert_eq!(pending.stage, LoginStage::Pending);
        assert_eq!((pending.attempt, pending.generation), (1, 1));
        assert_eq!(connection(&store, "codex").state, ConnectionState::AuthorizationPending);
        // pending 期间生成一律拒绝，刷新也不得伪装成已连接。
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "authorization_pending");
        assert_eq!(admit_target(&store.read(), &provider, "fixture-model", Protocol::Chat).unwrap_err().code, "authorization_pending");
        assert!(!generation_ready(&store.read(), &model, Protocol::Chat));
        let refreshed = refresh(&store, "codex").await.unwrap();
        assert_eq!(refreshed.state, ConnectionState::AuthorizationPending);
        assert!(refreshed.identity.is_none() && refreshed.evidence.is_none());
        // 完成通知 + 身份核实之后才绑定，且绑定在当前世代。
        adapter.push_completion("codex", 1, LoginResult::Completed { identity: "new@example.invalid".into() }).await;
        let result = await_login(store.clone(), "codex".into(), sessions.clone()).await.unwrap();
        assert_eq!(result, LoginResult::Completed { identity: "new@example.invalid".into() });
        let bound = connection(&store, "codex");
        assert_eq!(bound.generation, 1);
        assert_eq!(bound.state, ConnectionState::Connected);
        assert_eq!(bound.identity.as_deref(), Some("new@example.invalid"));
        assert!(bound.evidence.is_none(), "a new account must not inherit old evidence");
        // 登录成功不放行生成：还没有当前世代的只读证据。
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "evidence_missing");
        assert!(!generation_ready(&store.read(), &model, Protocol::Chat));
        let view = view_of(&store, store.subscription.available(), &sessions).await;
        assert_eq!(view.login.stage, LoginStage::Completed);
        assert_eq!(view.login.generation, 1);
        assert!(view.helper.available);
        assert_eq!(view.helper.version.as_deref(), Some("fixture-helper-1.0"));
        assert_eq!(view.logout, None);
    }

    #[tokio::test]
    async fn login_failure_and_cancel_have_their_own_terminal_stages() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        begin_login(&store, "codex", &sessions).await.unwrap();
        adapter
            .push_completion("codex", 1, LoginResult::Failed("sign-in failed: authorization_code=SUPERSECRET1234567890".into()))
            .await;
        let failed = await_login(store.clone(), "codex".into(), sessions.clone()).await.unwrap();
        let LoginResult::Failed(message) = failed else { panic!("expected a failed login") };
        assert!(!message.contains("SUPERSECRET"), "the error must be redacted: {message}");
        assert!(message.contains("[redacted]"), "{message}");
        let ended = session(&sessions, "codex").await;
        assert_eq!(ended.stage, LoginStage::Failed);
        assert!(ended.error.as_deref().is_some_and(|error| !error.contains("SUPERSECRET")));
        let settled = connection(&store, "codex");
        assert_eq!(settled.state, ConnectionState::NotConnected);
        assert!(settled.identity.is_none());
        // 取消：终态 cancelled，且不会留下挂起链接。
        begin_login(&store, "codex", &sessions).await.unwrap();
        cancel_login(&store, "codex", &sessions).await.unwrap();
        let cancelled = session(&sessions, "codex").await;
        assert_eq!(cancelled.stage, LoginStage::Cancelled);
        assert!(cancelled.authorization_url.is_none());
        assert_eq!(connection(&store, "codex").state, ConnectionState::NotConnected);
        assert_eq!(adapter.cancels.lock().unwrap().clone(), vec![("codex".into(), 1)]);
    }

    #[tokio::test]
    async fn a_late_completion_never_revives_the_account_after_logout() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        begin_login(&store, "codex", &sessions).await.unwrap();
        let waiting = tokio::spawn(await_login(store.clone(), "codex".into(), sessions.clone()));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let old_generation = connection(&store, "codex").generation;
        logout(&store, "codex", &sessions).await.unwrap();
        assert_eq!(connection(&store, "codex").generation, old_generation + 1);
        adapter
            .push_completion("codex", old_generation, LoginResult::Completed { identity: "late@example.invalid".into() })
            .await;
        let outcome = waiting.await.unwrap();
        assert!(outcome.is_err(), "a late result must be discarded: {outcome:?}");
        let after = connection(&store, "codex");
        assert_eq!(after.generation, old_generation + 1);
        assert!(after.identity.is_none(), "the old account must not be revived");
        assert_eq!(after.state, ConnectionState::NotConnected);
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Idle);
    }

    #[tokio::test]
    async fn a_late_completion_after_cancel_is_discarded() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        begin_login(&store, "codex", &sessions).await.unwrap();
        let waiting = tokio::spawn(await_login(store.clone(), "codex".into(), sessions.clone()));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        adapter.late_completion_on_cancel.store(true, Ordering::SeqCst);
        cancel_login(&store, "codex", &sessions).await.unwrap();
        let outcome = waiting.await.unwrap();
        assert!(outcome.is_err(), "a completion after cancel must be discarded: {outcome:?}");
        let after = connection(&store, "codex");
        assert_eq!(after.generation, 1);
        assert!(after.identity.is_none());
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Cancelled);
    }

    #[tokio::test]
    async fn logout_clears_caches_keeps_configuration_and_records_both_results() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        store
            .update(|config| {
                let generation = config.subscriptions["codex"].generation;
                let connection = config.subscriptions.get_mut("codex").unwrap();
                connection.state = ConnectionState::Connected;
                connection.identity = Some("old@example.invalid".into());
                connection.evidence = Some(Evidence {
                    generation,
                    account: Some("old@example.invalid".into()),
                    helper_version: Some("fixture-helper-1.0".into()),
                    account_path: Some("/tmp/fixture-helper-home".into()),
                    models: vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
                    capabilities: vec![Capability {
                        model_id: "fixture-model".into(),
                        protocol: protocol_key(Protocol::Chat).into(),
                        status: CapabilityStatus::Verified,
                    }],
                    quota: QuotaEvidence {
                        state: EvidenceState::Available,
                        source: Some("fixture".into()),
                        observed_at: Some("2026-09-30T00:00:00Z".into()),
                    },
                });
            })
            .unwrap();
        begin_login(&store, "codex", &sessions).await.unwrap();
        let before = store.read();
        let providers = before.providers.len();
        let models = before.models.len();
        let routes = serde_json::to_value(&before.routes).unwrap();
        let policy = serde_json::to_value(&before.policy).unwrap();
        let outcome = logout(&store, "codex", &sessions).await.unwrap();
        assert!(outcome.local_cleared);
        assert_eq!(outcome.remote, RemoteRevocation::Revoked);
        assert_eq!(adapter.logouts.lock().unwrap().clone(), vec![("codex".into(), 1)]);
        let after = store.read();
        let connection = after.subscriptions.get("codex").unwrap();
        assert_eq!(connection.generation, 2, "logout must advance the generation");
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert!(connection.identity.is_none());
        assert!(connection.evidence.is_none(), "identity, quota, directory and capability caches must be cleared");
        // 服务商、模型、路由与 API 配置原样保留。
        assert_eq!(after.providers.len(), providers);
        assert_eq!(after.models.len(), models);
        assert_eq!(serde_json::to_value(&after.routes).unwrap(), routes);
        assert_eq!(serde_json::to_value(&after.policy).unwrap(), policy);
        // 挂起 URL、user code 与错误只在内存态，不落盘。
        assert!(!serde_json::to_string(&after).unwrap().contains("example.invalid/authorize"));
        let ended = session(&sessions, "codex").await;
        assert_eq!(ended.stage, LoginStage::Idle);
        assert!(ended.authorization_url.is_none() && ended.login_id.is_none());
        let record = ended.last_logout.unwrap();
        assert!(record.local_cleared);
        assert_eq!(record.remote, Some(RemoteRevocation::Revoked));
        assert!(record.observed_at.is_some());
        let view = view_of(&store, true, &sessions).await;
        let logout_view = view.logout.unwrap();
        assert_eq!(logout_view.local, LogoutLocal::Cleared);
        assert_eq!(logout_view.remote, LogoutRemote::Revoked);
        assert!(logout_view.observed_at.is_some());
        assert_eq!(view.login.stage, LoginStage::Idle);
        assert_eq!(view.login.generation, 2);
    }

    #[tokio::test]
    async fn logout_without_an_observable_helper_records_an_unknown_remote_result() {
        let adapter = LifecycleStub::new();
        *adapter.logout_outcome.lock().unwrap() = Err("No subscription helper is available in this build".into());
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        let outcome = logout(&store, "codex", &sessions).await.unwrap();
        assert!(outcome.local_cleared);
        let record = session(&sessions, "codex").await.last_logout.unwrap();
        assert_eq!(record.remote, None);
        let view = view_of(&store, true, &sessions).await;
        assert_eq!(view.logout.unwrap().remote, LogoutRemote::Unknown);
    }

    #[tokio::test]
    async fn switch_account_failure_never_restores_the_old_account() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        store
            .update(|config| {
                let connection = config.subscriptions.get_mut("codex").unwrap();
                connection.state = ConnectionState::Connected;
                connection.identity = Some("old@example.invalid".into());
            })
            .unwrap();
        *adapter.start_error.lock().unwrap() = Some("The official Codex executable was not found".into());
        let error = switch_account(&store, "codex", &sessions).await.unwrap_err();
        assert!(error.contains("official Codex"), "{error}");
        let after = connection(&store, "codex");
        assert_eq!(after.generation, 2);
        assert_eq!(after.state, ConnectionState::NotConnected);
        assert!(after.identity.is_none() && after.evidence.is_none());
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Idle);
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "not_connected");
    }

    #[tokio::test]
    async fn switch_account_success_binds_the_new_identity_to_a_new_generation() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        store
            .update(|config| {
                let connection = config.subscriptions.get_mut("codex").unwrap();
                connection.state = ConnectionState::Connected;
                connection.identity = Some("old@example.invalid".into());
            })
            .unwrap();
        switch_account(&store, "codex", &sessions).await.unwrap();
        assert_eq!(connection(&store, "codex").generation, 2);
        assert!(connection(&store, "codex").identity.is_none());
        adapter
            .push_completion("codex", 2, LoginResult::Completed { identity: "new@example.invalid".into() })
            .await;
        let result = await_login(store.clone(), "codex".into(), sessions.clone()).await.unwrap();
        assert_eq!(result, LoginResult::Completed { identity: "new@example.invalid".into() });
        let bound = connection(&store, "codex");
        assert_eq!(bound.generation, 2);
        assert_eq!(bound.identity.as_deref(), Some("new@example.invalid"));
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Completed);
    }

    #[tokio::test]
    async fn the_default_adapter_denies_the_whole_login_lifecycle() {
        let adapter = UnavailableAdapter;
        let status = adapter.helper_status();
        assert!(!status.available && status.version.is_none() && status.auth_home.is_none());
        assert!(adapter.start_login("codex", 1).await.is_err());
        assert!(adapter.login_result("codex", 1).await.is_err());
        assert!(adapter.cancel_login("codex", 1).await.is_err());
        assert!(adapter.logout("codex", 1).await.is_err());
        let message = adapter.start_login("codex", 1).await.unwrap_err().to_string();
        assert!(message.contains("No subscription helper"), "{message}");
    }

    #[tokio::test]
    async fn unknown_providers_cannot_start_or_end_a_login() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter).await;
        assert!(begin_login(&store, "missing", &sessions).await.is_err());
        assert!(await_login(store.clone(), "missing".into(), sessions.clone()).await.is_err());
        assert!(cancel_login(&store, "missing", &sessions).await.is_err());
        assert!(logout(&store, "missing", &sessions).await.is_err());
        assert!(switch_account(&store, "missing", &sessions).await.is_err());
    }

    #[test]
    fn subscription_view_json_matches_the_frozen_shape() {
        let mut config = AppConfig::default();
        let provider = Provider {
            preset: String::new(),
            api_type: String::new(),
            test_model: String::new(),
            id: "codex".into(),
            name: "Codex".into(),
            kind: ProviderKind::CodexSubscription,
            base_url: String::new(),
            enabled: true,
            has_api_key: false,
        };
        config.providers.push(provider.clone());
        sync_provider(&mut config, &provider.id, &provider.kind);
        // 没有会话时：idle、世代回落到连接世代、logout 为 null、helper 不可用。
        let idle = views(&config, false, &SessionState::default()).pop().unwrap();
        let json = serde_json::to_value(&idle).unwrap();
        assert_eq!(json["login"]["stage"], "idle");
        assert_eq!(json["login"]["attempt"], 0);
        assert_eq!(json["login"]["generation"], 1);
        assert!(json["login"]["authorization_url"].is_null());
        assert!(json["login"]["user_code"].is_null());
        assert!(json["login"]["error"].is_null());
        assert!(json["logout"].is_null());
        assert_eq!(json["helper"]["available"], false);
        assert_eq!(json["adapter_available"], false);
        assert_eq!(json["state"], "not_connected");
        // 挂起登录 + 一次退出记录：字段名与取值都是冻结契约里的 snake_case。
        let mut sessions = SessionState::default();
        sessions.helper = HelperStatus {
            available: true,
            version: Some("1.2.3".into()),
            auth_home: Some("/tmp/fixture-helper-home".into()),
        };
        sessions.sessions.insert(
            "codex".into(),
            SubscriptionSession {
                stage: LoginStage::Pending,
                attempt: 3,
                generation: 4,
                login_id: Some("login-1".into()),
                authorization_url: Some("https://example.invalid/auth".into()),
                user_code: Some("ABCD-EFGH".into()),
                error: None,
                last_logout: Some(LogoutRecord {
                    local_cleared: true,
                    remote: Some(RemoteRevocation::Revoked),
                    observed_at: Some("2026-09-30T00:00:00Z".into()),
                }),
            },
        );
        let pending = views(&config, true, &sessions).pop().unwrap();
        let json = serde_json::to_value(&pending).unwrap();
        assert_eq!(json["login"]["stage"], "pending");
        assert_eq!(json["login"]["authorization_url"], "https://example.invalid/auth");
        assert_eq!(json["login"]["user_code"], "ABCD-EFGH");
        assert_eq!(json["login"]["attempt"], 3);
        assert_eq!(json["login"]["generation"], 4);
        assert_eq!(json["logout"]["local"], "cleared");
        assert_eq!(json["logout"]["remote"], "revoked");
        assert_eq!(json["logout"]["observed_at"], "2026-09-30T00:00:00Z");
        assert_eq!(json["helper"]["available"], true);
        assert_eq!(json["helper"]["version"], "1.2.3");
        assert_eq!(json["helper"]["auth_home"], "/tmp/fixture-helper-home");
        // 远端不可观察时展示 unknown。
        sessions.sessions.get_mut("codex").unwrap().last_logout = Some(LogoutRecord {
            local_cleared: false,
            remote: None,
            observed_at: None,
        });
        let json = serde_json::to_value(&views(&config, true, &sessions).pop().unwrap()).unwrap();
        assert_eq!(json["logout"]["local"], "retained");
        assert_eq!(json["logout"]["remote"], "unknown");
        assert!(json["logout"]["observed_at"].is_null());
    }

    #[tokio::test]
    async fn reconcile_orphaned_pending_recovers_a_restart_that_left_a_pending_state() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        // 挂起期间进程退出：配置里只剩 authorization_pending，会话表是空的。
        store
            .update(|config| {
                let connection = config.subscriptions.get_mut("codex").unwrap();
                connection.state = ConnectionState::AuthorizationPending;
                connection.identity = Some("old@example.invalid".into());
            })
            .unwrap();
        assert!(sessions.lock().await.session("codex").is_none());
        reconcile_orphaned_pending(&store).unwrap();
        let recovered = connection(&store, "codex");
        assert_eq!(recovered.state, ConnectionState::NotConnected);
        // 归位不改变世代，也不清除已有身份。
        assert_eq!(recovered.generation, 1);
        assert_eq!(recovered.identity.as_deref(), Some("old@example.invalid"));
        // 归位后可以重新发起登录，不再卡在“只能取消”。
        let start = begin_login(&store, "codex", &sessions).await.unwrap();
        assert_eq!(start.login_id, "codex-1");
        assert_eq!(connection(&store, "codex").state, ConnectionState::AuthorizationPending);
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Pending);
    }

    #[tokio::test]
    async fn cancel_login_settles_an_orphaned_pending_without_a_session() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        // 第二家订阅服务商同样落盘为挂起：取消 codex 不得波及它。
        store
            .update(|config| {
                let grok = Provider {
                    preset: String::new(),
                    api_type: String::new(),
                    test_model: String::new(),
                    id: "grok".into(),
                    name: "Grok".into(),
                    kind: ProviderKind::GrokSubscription,
                    base_url: String::new(),
                    enabled: true,
                    has_api_key: false,
                };
                config.providers.push(grok.clone());
                sync_provider(config, &grok.id, &grok.kind);
                config.subscriptions.get_mut("codex").unwrap().state = ConnectionState::AuthorizationPending;
                config.subscriptions.get_mut("grok").unwrap().state = ConnectionState::AuthorizationPending;
            })
            .unwrap();
        // 模拟重启后的空会话表：cancel 用连接的当前世代兜底 settle。
        cancel_login(&store, "codex", &sessions).await.unwrap();
        assert_eq!(connection(&store, "codex").state, ConnectionState::NotConnected);
        assert_eq!(connection(&store, "grok").state, ConnectionState::AuthorizationPending);
        assert_eq!(adapter.cancels.lock().unwrap().clone(), vec![("codex".into(), 1)]);
        assert!(sessions.lock().await.session("codex").is_none(), "cancel must not fabricate a session");
        // 幂等：重复取消仍然 Ok，也不会凭空造出会话。
        cancel_login(&store, "codex", &sessions).await.unwrap();
        assert_eq!(connection(&store, "codex").state, ConnectionState::NotConnected);
        assert!(sessions.lock().await.session("codex").is_none());
    }

    #[tokio::test]
    async fn a_normal_pending_session_still_cancels_with_its_own_generation() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        begin_login(&store, "codex", &sessions).await.unwrap();
        cancel_login(&store, "codex", &sessions).await.unwrap();
        assert_eq!(adapter.cancels.lock().unwrap().clone(), vec![("codex".into(), 1)]);
        let cancelled = session(&sessions, "codex").await;
        assert_eq!(cancelled.stage, LoginStage::Cancelled);
        assert!(cancelled.authorization_url.is_none());
        assert_eq!(connection(&store, "codex").state, ConnectionState::NotConnected);
        // 回归：正常挂起会话被取消后，迟到的完成通知仍然整体丢弃。
        adapter.late_completion_on_cancel.store(true, Ordering::SeqCst);
        begin_login(&store, "codex", &sessions).await.unwrap();
        let waiting = tokio::spawn(await_login(store.clone(), "codex".into(), sessions.clone()));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_login(&store, "codex", &sessions).await.unwrap();
        let outcome = waiting.await.unwrap();
        assert!(outcome.is_err(), "a late completion must be discarded: {outcome:?}");
        assert!(connection(&store, "codex").identity.is_none());
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Cancelled);
    }
}
