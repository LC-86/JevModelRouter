//! 订阅服务商边界：每家一个活动连接、证据绑定连接世代，以及所有生成入口共用的 fail-closed 准入。
//!
//! 本模块不依赖 Tauri 或 HTTP 框架，连接、证据、准入与只读刷新都能直接单测。适配器只在构造
//! `ConfigStore` 时注入：默认构造（含测试）装的是不提供任何辅助进程的 [`UnavailableAdapter`]，
//! 生产在 `lib.rs` 唯一的 ConfigStore 构造处注入官方 Codex 与 Grok 适配器组成的
//! [`SubscriptionAdapters`] 注册表（按 `provider.kind` 分派，两家互不冒用）。两者都由代码固定，
//! 配置、环境与界面都没有把它换成替身的开关；未验证的订阅生成一律拒绝。

pub mod auth;
pub mod grok;
pub mod helper;

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
    kind_label(&provider.kind)
}

/// 订阅类型对应的展示名；非订阅类型统一回落为 `API`。
fn kind_label(kind: &ProviderKind) -> &'static str {
    match kind {
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
    /// 读取成功但上游账号被明确拒绝（订阅内许可为 `denied`）；界面不得把它写成「未知」。
    Denied,
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

/// 额度许可：上游许可字段的只读映射，缺失、null 或非布尔一律视为未知。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPermission {
    #[default]
    Unknown,
    Allowed,
    Denied,
}

/// 额度结果的来源视图。它标明本次额度消费的是哪种上游契约，不同视图的桶语义不可混用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaView {
    #[default]
    Unknown,
    RateLimitsByLimitId,
    RateLimits,
    /// 本应用的 Grok 只读额度事件（`usage --json`）。
    GrokCliUsage,
}

/// 一个额度窗口。`used_percent` 是上游原始百分比，剩余百分比由界面用 `100 - used` 推导，
/// 后端不预先换算、不截断、不补齐越界值。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub label: String,
    #[serde(default)]
    pub used_percent: Option<f64>,
    #[serde(default)]
    pub window_minutes: Option<i64>,
    /// Unix 秒。
    #[serde(default)]
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub missing_fields: Vec<String>,
    /// 类型不符或越界的原始文本；对应取值一律为 `None`，绝不截断成合法值。
    #[serde(default)]
    pub invalid_fields: Vec<String>,
}

impl QuotaWindow {
    pub(crate) fn new(label: &str) -> Self {
        Self { label: label.to_owned(), ..Self::default() }
    }
}

/// 上游额外 credits 的只读映射：`balance`/`unit` 原样保留字符串，不解析金额、不推断单位。
/// `permission` 是 credits 自身的计量轴，与订阅内许可互不推导。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaCredits {
    #[serde(default)]
    pub has_credits: Option<bool>,
    #[serde(default)]
    pub unlimited: Option<bool>,
    #[serde(default)]
    pub balance: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub permission: QuotaPermission,
    #[serde(default)]
    pub missing_fields: Vec<String>,
    #[serde(default)]
    pub invalid_fields: Vec<String>,
}

/// 一个额度桶。Grok 只读契约里是单桶（`limit_id = subscription_pool`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuotaBucket {
    #[serde(default)]
    pub limit_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub windows: Vec<QuotaWindow>,
    #[serde(default)]
    pub credits: Option<QuotaCredits>,
    #[serde(default)]
    pub permission: QuotaPermission,
    #[serde(default)]
    pub missing_fields: Vec<String>,
    #[serde(default)]
    pub invalid_fields: Vec<String>,
}

impl QuotaBucket {
    pub(crate) fn new(limit_id: &str) -> Self {
        Self { limit_id: limit_id.to_owned(), ..Self::default() }
    }
}

/// 额度证据：来源、时间、视图、桶与解析时记下的缺字段/越界字段。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuotaEvidence {
    #[serde(default)]
    pub state: EvidenceState,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
    #[serde(default)]
    pub view: QuotaView,
    #[serde(default)]
    pub buckets: Vec<QuotaBucket>,
    #[serde(default)]
    pub missing_fields: Vec<String>,
    /// true 表示 `buckets` 是失败后保留的历史数字，界面必须标「历史数据 / 最后成功更新时间」。
    #[serde(default)]
    pub history: bool,
}

/// 一次目录读取的完整结果：状态 + 条目 + 解析时记下的缺失字段 + 来源与时间。
/// 状态只有 `Available`（读到了目录）与 `Unsupported`（固定版本没有该机器接口）两种成功形态；
/// 失败一律用 `Err` 表达，绝不返回空列表来伪装成功。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRead {
    #[serde(default)]
    pub state: EvidenceState,
    #[serde(default)]
    pub models: Vec<DiscoveredModel>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
    #[serde(default)]
    pub missing_fields: Vec<String>,
}

/// 目录证据的元数据；目录条目本身是 `Evidence::models`。
/// 从未读取 → Unknown；读取成功 → Available；失败但有已核实目录 → Stale；失败且无目录 → Failed。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEvidence {
    #[serde(default)]
    pub state: EvidenceState,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
    #[serde(default)]
    pub missing_fields: Vec<String>,
    /// 上一次已核实目录里有、本次成功读取里没有的模型标识（顺序稳定、去重）。
    /// 只有成功读取才重算；stale/unsupported 保留上一次的值，failed 为空。
    #[serde(default)]
    pub removed_models: Vec<String>,
}

/// 额度状态规则（契约 B）：至少一个桶时，任一桶 denied → Denied；
/// 全部桶 allowed 且至少一个窗口有有效 `used_percent` → Available；否则 Unknown。桶为空 → Unknown。
pub(crate) fn quota_state(buckets: &[QuotaBucket]) -> EvidenceState {
    if buckets.is_empty() {
        return EvidenceState::Unknown;
    }
    if buckets.iter().any(|bucket| bucket.permission == QuotaPermission::Denied) {
        return EvidenceState::Denied;
    }
    let all_allowed = buckets.iter().all(|bucket| bucket.permission == QuotaPermission::Allowed);
    let measured = buckets
        .iter()
        .flat_map(|bucket| bucket.windows.iter())
        .any(|window| window.used_percent.is_some());
    if all_allowed && measured {
        EvidenceState::Available
    } else {
        EvidenceState::Unknown
    }
}

/// 一次只读读取的完整结果，整体绑定连接世代：世代不符即整体作废，不逐字段沿用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// 目录证据的来源/时间/缺失字段；旧配置没有该字段时按「从未读取」读取。
    #[serde(default)]
    pub catalog: CatalogEvidence,
}

/// 每家服务商一个活动连接。换号与退出递增 `generation`，旧世代的结果一律不采用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
        EvidenceState::Denied => "the upstream account is not permitted in-subscription ordinary usage",
        EvidenceState::Available => "quota is available",
    };
    // denied 是上游对订阅内普通用量的明确拒绝，刷新并不能恢复，不能给误导性的重试建议。
    let recovery = match state {
        EvidenceState::Denied => {
            "Use an API provider for this request, or wait until the upstream account restores included usage."
        }
        _ => "Refresh the connection in AutoJev → Providers once the upstream account is reachable.",
    };
    Denial::new(
        code,
        DenialFamily::Quota,
        format!("{} cannot prove current in-subscription usage: {reason}.", label(provider)),
        recovery.into(),
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
        EvidenceState::Denied => Err(quota_denial(provider, "quota_denied", EvidenceState::Denied)),
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
    /// 全局回退的辅助进程自述；仅在 [`Self::helpers`] 没有该服务商条目时使用（既有测试注入）。
    pub helper: HelperStatus,
    /// per-kind（按服务商标识）的辅助进程自述：Grok 行不得显示 Codex 适配器的版本或授权目录。
    pub helpers: HashMap<String, HelperStatus>,
    /// per-kind（按服务商标识）的适配器可用性：Grok 行不得借用 Codex 适配器的 availability。
    pub adapters_available: HashMap<String, bool>,
    /// 当前适配器真正支持登录的服务商标识（来自 `SubscriptionAdapter::supports`）。
    /// 未列出的订阅行不得借用全局 helper 的版本与授权目录。
    pub supported_providers: Vec<String>,
}

impl SessionState {
    pub fn session(&self, provider_id: &str) -> Option<&SubscriptionSession> {
        self.sessions.get(provider_id)
    }

    pub fn supports(&self, provider_id: &str) -> bool {
        self.supported_providers.iter().any(|supported| supported == provider_id)
    }
}

fn require_subscription_provider(store: &ConfigStore, provider_id: &str) -> Result<Provider, String> {
    store
        .read()
        .providers
        .into_iter()
        .find(|provider| provider.id == provider_id && is_subscription_provider(provider))
        .ok_or_else(|| format!("Unknown subscription provider: {provider_id}"))
}

/// 只有适配器声明支持该服务商类型时才允许动连接或拉起辅助进程。
fn require_supported(store: &ConfigStore, provider: &Provider, action: &str) -> Result<(), String> {
    if store.subscription.supports(&provider.kind) {
        return Ok(());
    }
    Err(format!("{} subscription {action} is not implemented yet", label(provider)))
}

/// 按服务商类型解析适配器：解析不到即明确拒绝，绝不让一家借用别家的辅助进程或身份。
fn adapter_for_kind(
    store: &ConfigStore,
    kind: &ProviderKind,
) -> Result<std::sync::Arc<dyn SubscriptionAdapter>, String> {
    store
        .subscription
        .for_kind(kind)
        .cloned()
        .ok_or_else(|| format!("{} subscription reads are not implemented yet", kind_label(kind)))
}

/// 按服务商标识解析适配器：先确认它确实是订阅服务商。
fn adapter_for(store: &ConfigStore, provider_id: &str) -> Result<std::sync::Arc<dyn SubscriptionAdapter>, String> {
    let provider = require_subscription_provider(store, provider_id)?;
    adapter_for_kind(store, &provider.kind)
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
    let provider = require_subscription_provider(store, provider_id)?;
    // 不支持的订阅类型不得拉起任何辅助进程，也不得改动连接状态。
    require_supported(store, &provider, "sign-in")?;
    let generation = connection_generation(store, provider_id);
    let start = adapter_for_kind(store, &provider.kind)?
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
    let provider = require_subscription_provider(store, provider_id)?;
    require_supported(store, &provider, "sign-in cancellation")?;
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
    let result = adapter_for(store, provider_id)?.cancel_login(provider_id, generation).await;
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
    let outcome = match adapter_for(&store, &provider_id)?.login_result(&provider_id, generation).await {
        Ok(Some(outcome)) => outcome,
        // 辅助进程中途退出（stdout 关闭或尝试已不在跟踪）与出错同等处理：结束挂起态并如实报错，
        // 否则会话会永远停在 Pending，用户既不能登录也不能取消。
        Ok(None) => {
            let message = "The Codex helper exited before the sign-in finished; sign in again".to_owned();
            let mut state = sessions.lock().await;
            if finish_session(&mut state, &provider_id, generation, attempt, LoginStage::Failed, Some(message.clone())) {
                drop(state);
                settle_not_connected(&store, &provider_id, generation)?;
            }
            return Err(message);
        }
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
    let provider = require_subscription_provider(store, provider_id)?;
    require_supported(store, &provider, "sign-out")?;
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
    let adapter = adapter_for(store, provider_id)?.logout(provider_id, previous_generation).await;
    let (local_cleared, remote) = match adapter {
        Ok(outcome) => (outcome.local_cleared, Some(outcome.remote)),
        // 没拿到“专用授权目录已清”的确认，就不能声称本地已清除；远端结果同样未知。
        Err(_) => (false, None),
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

/// 该服务商当前的订阅种类；不存在或不是订阅服务商时返回 `None`。
fn subscription_kind(store: &ConfigStore, provider_id: &str) -> Option<ProviderKind> {
    store
        .read()
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| provider.kind.clone())
        .filter(is_subscription)
}

/// 删除服务商或把它转成 API 服务商之前，先释放它占用的订阅资源：
/// 停掉辅助进程、删除专用授权目录、清空内存会话。
/// 失败即返回错误，调用方不得继续改配置——不做半清理。
pub async fn dispose(
    store: &ConfigStore,
    provider_id: &str,
    sessions: &tokio::sync::Mutex<SessionState>,
) -> Result<(), String> {
    let Some(kind) = subscription_kind(store, provider_id) else {
        return Ok(());
    };
    if store.subscription.supports(&kind) {
        logout(store, provider_id, sessions).await.map(|_| ())?;
    }
    let mut state = sessions.lock().await;
    state.sessions.remove(provider_id);
    Ok(())
}

/// 服务商在保留订阅类型的前提下改名：把适配器自有状态（辅助进程、挂起尝试、专用授权目录）
/// 与内存会话迁到新标识。迁移失败时对旧标识走完整退出，配置改名后新标识为未连接，逼用户重新登录；
/// 绝不只改配置而不碰辅助进程。
pub async fn migrate_helper(
    store: &ConfigStore,
    old_id: &str,
    new_id: &str,
    sessions: &tokio::sync::Mutex<SessionState>,
) -> Result<(), String> {
    if old_id == new_id {
        return Ok(());
    }
    let Some(kind) = subscription_kind(store, old_id) else {
        return Ok(());
    };
    if let Ok(adapter) = adapter_for_kind(store, &kind) {
        if let Err(error) = adapter.rename(old_id, new_id).await {
            let message = sanitize_error(&error.to_string());
            logout(store, old_id, sessions).await.map(|_| ())?;
            eprintln!("AutoJev subscription rename fell back to sign-out: {message}");
        }
    }
    let mut state = sessions.lock().await;
    if let Some(mut session) = state.sessions.remove(old_id) {
        // 在途登录绑定旧标识，改名后不可能再完成：作废它，别让新标识停在 Pending。
        if session.stage == LoginStage::Pending {
            session.stage = LoginStage::Idle;
            session.login_id = None;
            session.authorization_url = None;
            session.user_code = None;
            session.error = None;
        }
        state.sessions.insert(new_id.to_owned(), session);
    }
    Ok(())
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
    /// 这个适配器是否能承载该类型的订阅服务商；不支持的 kind 必须被明确拒绝，
    /// 不得借用其它服务商的辅助进程或身份。
    fn supports(&self, kind: &ProviderKind) -> bool;
    /// 服务商标识重命名：适配器要把自有状态（辅助进程、挂起尝试、专用授权目录）整体迁到新标识。
    /// 默认实现表示该适配器没有自有状态可迁；绝不静默失败，迁不动必须返回错误。
    fn rename<'a>(&'a self, _old_id: &'a str, _new_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    /// 辅助进程自述状态：解析不到官方 `codex` 时如实报不可用，绝不 panic。
    fn helper_status(&self) -> HelperStatus;
    fn status<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>>;
    fn models<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<CatalogRead>>;
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

/// 默认实现（含测试构造）：尚未管理任何官方辅助进程，只读查询如实报不可用，生成一律拒绝。
/// 它也不支持任何订阅服务商类型；没有任何配置、环境变量或界面开关能把它换成替身。
pub struct UnavailableAdapter;

impl SubscriptionAdapter for UnavailableAdapter {
    fn available(&self) -> bool {
        false
    }

    fn supports(&self, _kind: &ProviderKind) -> bool {
        false
    }

    fn helper_status(&self) -> HelperStatus {
        HelperStatus::default()
    }

    fn status<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
        Box::pin(async { bail!(UNAVAILABLE) })
    }

    fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<CatalogRead>> {
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

/// 按服务商类型分派的适配器注册表：同时装官方 Codex 与 Grok 适配器，按 `provider.kind` 解析。
/// 解析不到即明确拒绝（沿用 [`require_supported`] 语义），**绝不**让一家借用另一家的辅助进程、
/// 版本或授权目录。`--autojev-helper`、`AUTOJEV_GROK_HELPER` 等覆盖入口都不改变这条边界。
#[derive(Clone)]
pub struct SubscriptionAdapters {
    adapters: Vec<std::sync::Arc<dyn SubscriptionAdapter>>,
}

impl SubscriptionAdapters {
    pub fn new(adapters: Vec<std::sync::Arc<dyn SubscriptionAdapter>>) -> Self {
        Self { adapters }
    }

    /// 只装一个适配器：兼容既有测试注入（默认构造也用它装 [`UnavailableAdapter`]）。
    pub fn single(adapter: std::sync::Arc<dyn SubscriptionAdapter>) -> Self {
        Self { adapters: vec![adapter] }
    }

    /// 按服务商类型解析适配器；同一类型出现多次时取第一个，绝不回落到别家的实现。
    pub fn for_kind(&self, kind: &ProviderKind) -> Option<&std::sync::Arc<dyn SubscriptionAdapter>> {
        self.adapters.iter().find(|adapter| adapter.supports(kind))
    }

    pub fn supports(&self, kind: &ProviderKind) -> bool {
        self.for_kind(kind).is_some()
    }

    /// 该类型是否有可用的官方辅助进程；没有适配器即不可用。
    pub fn available(&self, kind: &ProviderKind) -> bool {
        self.for_kind(kind).is_some_and(|adapter| adapter.available())
    }

    /// per-kind helper 自述状态：Grok 行永远拿不到 Codex 适配器的版本或授权目录。
    pub fn helper_status(&self, kind: &ProviderKind) -> HelperStatus {
        self.for_kind(kind).map(|adapter| adapter.helper_status()).unwrap_or_default()
    }
}

impl Default for SubscriptionAdapters {
    fn default() -> Self {
        Self::single(std::sync::Arc::new(UnavailableAdapter))
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
    if let Some(mut connection) = config.subscriptions.remove(old_id) {
        // 在途登录绑定旧标识，重命名后不可能再完成：回到未连接，逼用户在新标识下重新登录。
        if connection.state == ConnectionState::AuthorizationPending {
            connection.state = ConnectionState::NotConnected;
        }
        config.subscriptions.insert(new_id.to_owned(), connection);
    }
}

/// 删除服务商时一并丢弃其连接与证据。
pub fn forget_provider(config: &mut AppConfig, provider_id: &str) {
    config.subscriptions.remove(provider_id);
}

// CHUNK-6

/// 此前是否有一份已核实的目录：成功读过（可能是空目录），或保留了非空模型列表。
/// 只有它成立时，读取失败才标 `Stale` 并保留旧目录；否则如实标 `Failed`。
fn had_verified_catalog(previous: Option<&Evidence>) -> bool {
    previous.is_some_and(|evidence| {
        !evidence.models.is_empty()
            || matches!(evidence.catalog.state, EvidenceState::Available | EvidenceState::Stale)
    })
}

/// 一次成功目录读取相对上一次已核实目录「被移除」的模型：旧目录里有、本次没有的标识，
/// 按旧目录顺序去重。没有上一次目录（或账号/世代不同导致证据作废）时为空。
fn removed_models(previous: Option<&Evidence>, current: &[DiscoveredModel]) -> Vec<String> {
    let Some(previous) = previous else { return Vec::new() };
    let mut removed: Vec<String> = Vec::new();
    for model in &previous.models {
        let still_present = current.iter().any(|entry| entry.model_id == model.model_id);
        if !still_present && !removed.iter().any(|entry| entry == &model.model_id) {
            removed.push(model.model_id.clone());
        }
    }
    removed
}

/// 本次目录列表是否可信：只认本次读取自己的缺失记账，不做任何猜测。
/// - `models`：目录事件根本没有可用的 `models` 数组（键缺失或类型不符）；
/// - `models[i].id`：第 i 个条目缺可用标识、已被跳过。
/// 两者都意味着「我们不知道上游这次到底给了哪些模型」，此时不能据此判定任何模型被移除。
fn catalog_list_is_trustworthy(missing_fields: &[String]) -> bool {
    !missing_fields
        .iter()
        .any(|field| field == "models" || (field.starts_with("models[") && field.ends_with("].id")))
}

/// 只读刷新：按当前连接世代读取状态、目录与额度并写回证据。
/// 它与生成准入无关，因此生成被拒绝、网关暂停或服务商停用时仍可执行。
///
/// 三项读取彼此独立：状态读不到就返回错误且不改写任何证据；目录或额度失败时按契约
/// 写回（保留已核实的历史数字与时间，只把对应依据标为陈旧/失败），命令仍返回 `Ok`。
///
/// 本次状态读不到有效账号身份时无法确认数据归属：既不改写已核实身份（覆盖成 `None` 会让
/// `current_evidence` 绑不上账号，连历史目录与额度都会在下次刷新时丢掉），也不读取本次的
/// 目录与额度，直接返回错误。明确退出与换号不经过这条守卫：logout/switch 发生时已递增
/// 世代并清空身份与证据。
pub async fn refresh(store: &ConfigStore, provider_id: &str) -> Result<Connection, String> {
    let provider = require_subscription_provider(store, provider_id)?;
    // 不支持的订阅类型不得借用其它服务商的辅助进程读取证据（#12「两家互不冒用」）。
    require_supported(store, &provider, "read-only status")?;
    let before = store.read().subscriptions.get(provider_id).cloned().unwrap_or_default();
    if before.state == ConnectionState::AuthorizationPending {
        // 挂起登录期间只读刷新不得伪装成已连接，也不得让旧身份复活：一个连接字段都不改写。
        return Ok(before);
    }
    let generation = before.generation;
    // 1) 连接状态：读不到状态就是「连接状态未知」，返回错误且不改写任何证据。
    let adapter = store
        .subscription
        .for_kind(&provider.kind)
        .cloned()
        .ok_or_else(|| format!("{} subscription reads are not implemented yet", label(&provider)))?;
    let status = adapter.status(provider_id, generation).await.map_err(|error| error.to_string())?;
    let verified_identity = before.identity.clone().filter(|identity| !identity.trim().is_empty());
    // 证据必须绑定账号：本次读到空身份就无法归属，无论此前是否已核实过身份，
    // 都一个字段都不改写（幂等），也不发起本次目录与额度读取。
    // 这同时保证「已核实身份」永远不会被覆盖成 None：覆盖了 current_evidence 就再也绑不上账号，
    // 连历史目录与额度都会在下次刷新时丢掉。
    let Some(read_identity) = status.identity.clone().filter(|identity| !identity.trim().is_empty()) else {
        return Err(format!(
            "The {} helper did not report an account identity; the read-only refresh could not \
             confirm which account the data belongs to, so nothing was changed",
            label(&provider)
        ));
    };
    // 换号期间的旧响应不得污染新账号：读取到的身份与连接里已核实的身份不符时整体丢弃。
    if verified_identity.as_deref().is_some_and(|verified| verified != read_identity) {
        return Err("The account changed while refreshing; the read-only result was discarded".into());
    }
    // 2) 目录与额度独立读取：任一失败都不影响另一项写回。
    let catalog_read = adapter.models(provider_id, generation).await;
    let quota_read = adapter.quota(provider_id, generation).await;
    store.update(|config| -> Result<Connection, String> {
        // 服务商可能在读取期间被删除或换号：迟到的只读结果不得复活或污染连接。
        if !config.providers.iter().any(|provider| provider.id == provider_id && is_subscription_provider(provider)) {
            return Err("The subscription provider was removed while refreshing".into());
        }
        let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
        if connection.generation != generation || connection.identity != before.identity {
            return Err("The connection changed while refreshing; the read-only result was discarded".into());
        }
        let previous = connection.current_evidence().cloned();
        // 目录：成功即以本次权威结果整体替换（旧列表中不再出现的模型即被撤销）；
        // `unsupported` 是「固定版本没有这个机器接口」的如实结果，不是失败：列表为空，
        // 来源与观测时间只取本次事件，缺失即 None——绝不沿用上一次已核实目录的时间，
        // 否则「unsupported」旁边挂着一个历史时间会被读成「在那个时间点观测到了当前状态」。
        // （额度失败保留历史数字 + history=true 是另一回事：那里确实有历史数字可保留。）
        let (models, catalog) = match catalog_read {
            Ok(read) if read.state == EvidenceState::Unsupported => (
                Vec::new(),
                CatalogEvidence {
                    state: EvidenceState::Unsupported,
                    source: read.source,
                    observed_at: read.observed_at,
                    missing_fields: read.missing_fields,
                    // 本次什么都没读到，上一次的「移除」记录原样保留。
                    removed_models: previous
                        .as_ref()
                        .map(|evidence| evidence.catalog.removed_models.clone())
                        .unwrap_or_default(),
                },
            ),
            // 本次列表自身不可信（缺 `models` 数组，或有条目因缺可用 id 被跳过）：整份目录读取都
            // 不算权威结果——不能把已核实目录清空成「可用 + 0 个模型」，也不能据此判定任何模型被移除。
            // missing_fields 用本次的记账，说明这次为什么没能确认。
            Ok(read) if !catalog_list_is_trustworthy(&read.missing_fields) => {
                let missing_fields = read.missing_fields;
                if had_verified_catalog(previous.as_ref()) {
                    let kept = previous.as_ref().map(|evidence| evidence.catalog.clone()).unwrap_or_default();
                    (
                        previous.as_ref().map(|evidence| evidence.models.clone()).unwrap_or_default(),
                        CatalogEvidence { state: EvidenceState::Stale, missing_fields, ..kept },
                    )
                } else {
                    (Vec::new(), CatalogEvidence { state: EvidenceState::Failed, missing_fields, ..CatalogEvidence::default() })
                }
            }
            Ok(read) => {
                // 列表可信：旧目录里有、本次没有的模型就是被上游移除了。
                // 顺序按上一次已核实目录的顺序稳定去重，绝不排序或补造标识。
                let removed_models = removed_models(previous.as_ref(), &read.models);
                (
                    read.models,
                    CatalogEvidence {
                        state: EvidenceState::Available,
                        source: read.source,
                        observed_at: read.observed_at,
                        missing_fields: read.missing_fields,
                        removed_models,
                    },
                )
            }
            Err(_) if had_verified_catalog(previous.as_ref()) => {
                let kept = previous.as_ref().map(|evidence| evidence.catalog.clone()).unwrap_or_default();
                (
                    previous.as_ref().map(|evidence| evidence.models.clone()).unwrap_or_default(),
                    CatalogEvidence { state: EvidenceState::Stale, ..kept },
                )
            }
            Err(_) => (Vec::new(), CatalogEvidence { state: EvidenceState::Failed, ..CatalogEvidence::default() }),
        };
        // 额度：失败保留旧桶与旧 observed_at，绝不刷新时间或编造 0。
        let quota = match quota_read {
            Ok(quota) => quota,
            Err(_) => {
                let kept = previous.as_ref().map(|evidence| evidence.quota.clone()).unwrap_or_default();
                // 只有确实保留了历史数字/时间才标 history：从未成功读过时没有「最后成功更新」可言。
                let history = kept.observed_at.is_some() || !kept.buckets.is_empty();
                QuotaEvidence { state: EvidenceState::Failed, history, ..kept }
            }
        };
        let capabilities = previous.as_ref().map(|evidence| evidence.capabilities.clone()).unwrap_or_default();
        connection.state = status.state;
        connection.identity = Some(read_identity.clone());
        connection.evidence = Some(Evidence {
            generation,
            account: Some(read_identity),
            helper_version: status.helper_version,
            account_path: status.account_path,
            models,
            capabilities,
            quota,
            catalog,
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
    pub catalog: CatalogEvidence,
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

/// 界面视图。`adapter_available` 是**回退值**：优先按服务商标识取
/// [`SessionState::adapters_available`] 里的 per-kind 结果，缺失时才用它（既有测试与单适配器注入）。
/// 生产 `snapshot` 会为每家订阅服务商写入 per-kind 结果，因此 Grok 行不会借用 Codex 适配器的 availability。
pub fn views(config: &AppConfig, adapter_available: bool, sessions: &SessionState) -> Vec<SubscriptionView> {
    let mut items: Vec<_> = config
        .providers
        .iter()
        .filter(|provider| is_subscription_provider(provider))
        .map(|provider| {
            let connection = config.subscriptions.get(&provider.id).cloned().unwrap_or_default();
            let evidence = connection.current_evidence();
            let session = sessions.session(&provider.id);
            // 适配器不支持这类服务商时，如实报 helper 不可用：不借用其它服务商的进程信息。
            // 支持的订阅行取该服务商自己的 helper 自述；没有 per-kind 条目才回落到全局值。
            let supported = sessions.supports(&provider.id);
            let own_helper = sessions
                .helpers
                .get(&provider.id)
                .cloned()
                .unwrap_or_else(|| sessions.helper.clone());
            let helper = if supported { own_helper } else { HelperStatus::default() };
            // availability 同样按服务商取自己的适配器结果，绝不把别家的可用性算到这一行。
            let adapter_available = sessions
                .adapters_available
                .get(&provider.id)
                .copied()
                .unwrap_or(adapter_available);
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
                catalog: evidence.map(|evidence| evidence.catalog.clone()).unwrap_or_default(),
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
                    available: helper.available,
                    version: helper.version,
                    auth_home: helper.auth_home,
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
                ..QuotaEvidence::default()
            },
            catalog: CatalogEvidence {
                state: EvidenceState::Available,
                source: Some("fixture".into()),
                observed_at: Some("2026-09-30T00:00:00Z".into()),
                missing_fields: Vec::new(),
                removed_models: Vec::new(),
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
            // denied 是上游对订阅内普通用量的明确拒绝：不能回落成「未知」，也不能建议无效重试。
            ("quota_denied", Box::new(|config: &mut AppConfig| {
                connected(config);
                config.subscriptions.get_mut("fixture-subscription").unwrap().evidence.as_mut().unwrap().quota.state = EvidenceState::Denied;
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// 替身写回的固定读取时间：失败后必须原样保留，不能被刷新成「现在」。
    const STUB_CATALOG_OBSERVED_AT: &str = "2026-09-30T00:00:00Z";
    const STUB_QUOTA_OBSERVED_AT: &str = "2026-09-30T01:00:00Z";

    /// 只读替身：只在测试代码里存在，用来驱动适配边界本身。
    /// 它没有任何生产入口，配置与界面都无法把它装进应用。
    struct StubAdapter {
        status: Mutex<ConnectionStatus>,
        catalog: Mutex<CatalogRead>,
        quota: Mutex<QuotaEvidence>,
        reads: Mutex<Vec<u64>>,
        /// 目录与额度读取的调用次数：用来证明「无法确认身份时本次刷新根本不发起这两项读取」。
        catalog_reads: AtomicUsize,
        quota_reads: AtomicUsize,
        store: Mutex<Option<Arc<ConfigStore>>>,
        switch_during_read: AtomicBool,
        fail_status: AtomicBool,
        fail_models: AtomicBool,
        fail_quota: AtomicBool,
    }

    impl StubAdapter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                status: Mutex::new(ConnectionStatus {
                    state: ConnectionState::Connected,
                    identity: Some("fixture@example.invalid".into()),
                    helper_version: Some("fixture-helper-1.0".into()),
                    account_path: Some("/tmp/fixture-account".into()),
                }),
                catalog: Mutex::new(CatalogRead {
                    state: EvidenceState::Available,
                    models: vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
                    source: Some("fixture:models".into()),
                    observed_at: Some(STUB_CATALOG_OBSERVED_AT.into()),
                    missing_fields: Vec::new(),
                }),
                quota: Mutex::new(QuotaEvidence {
                    state: EvidenceState::Available,
                    source: Some("fixture:usage".into()),
                    observed_at: Some(STUB_QUOTA_OBSERVED_AT.into()),
                    view: QuotaView::RateLimitsByLimitId,
                    buckets: vec![QuotaBucket {
                        limit_id: "fixture-limit".into(),
                        permission: QuotaPermission::Allowed,
                        windows: vec![QuotaWindow {
                            label: "primary".into(),
                            used_percent: Some(42.0),
                            window_minutes: Some(300),
                            resets_at: Some(1_800_000_000),
                            ..QuotaWindow::default()
                        }],
                        ..QuotaBucket::default()
                    }],
                    ..QuotaEvidence::default()
                }),
                reads: Mutex::new(Vec::new()),
                catalog_reads: AtomicUsize::new(0),
                quota_reads: AtomicUsize::new(0),
                store: Mutex::new(None),
                switch_during_read: AtomicBool::new(false),
                fail_status: AtomicBool::new(false),
                fail_models: AtomicBool::new(false),
                fail_quota: AtomicBool::new(false),
            })
        }
    }

    impl SubscriptionAdapter for StubAdapter {
        fn available(&self) -> bool {
            true
        }

        /// 只读替身只承载 Codex 订阅；Grok 一行不得借它读取。
        fn supports(&self, kind: &ProviderKind) -> bool {
            matches!(kind, ProviderKind::CodexSubscription)
        }

        fn status<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
            Box::pin(async move {
                self.reads.lock().unwrap().push(generation);
                if self.fail_status.load(Ordering::SeqCst) {
                    bail!("fixture helper status read failed");
                }
                if self.switch_during_read.load(Ordering::SeqCst) {
                    if let Some(store) = self.store.lock().unwrap().clone() {
                        store.update(|config| {
                            if let Some(connection) = config.subscriptions.get_mut(provider_id) {
                                connection.generation += 1;
                            }
                        })?;
                    }
                }
                Ok(self.status.lock().unwrap().clone())
            })
        }

        /// 目录读取：成功返回本次权威结果；`unsupported` 是如实结果而不是失败。
        fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<CatalogRead>> {
            Box::pin(async move {
                self.catalog_reads.fetch_add(1, Ordering::SeqCst);
                if self.fail_models.load(Ordering::SeqCst) {
                    bail!("fixture helper catalog read failed");
                }
                Ok(self.catalog.lock().unwrap().clone())
            })
        }

        fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
            Box::pin(async move {
                self.quota_reads.fetch_add(1, Ordering::SeqCst);
                if self.fail_quota.load(Ordering::SeqCst) {
                    bail!("fixture helper quota read failed");
                }
                Ok(self.quota.lock().unwrap().clone())
            })
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

    /// 状态读失败 = 连接状态未知：返回错误且一个证据字段都不改写，也不发起目录/额度读取。
    #[tokio::test]
    async fn a_failed_status_read_changes_nothing_and_stops_the_other_reads() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let before = serde_json::to_value(store.read().subscriptions.get("codex").unwrap()).unwrap();
        adapter.fail_status.store(true, Ordering::SeqCst);
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("fixture helper status read failed"), "{error}");
        let after = serde_json::to_value(store.read().subscriptions.get("codex").unwrap()).unwrap();
        assert_eq!(after, before, "状态读失败不得改写任何连接字段");
        assert_eq!(adapter.catalog_reads.load(Ordering::SeqCst), 1, "第二次刷新不得再发起目录读取");
        assert_eq!(adapter.quota_reads.load(Ordering::SeqCst), 1, "第二次刷新不得再发起额度读取");
    }

    /// 目录与额度失败是相互独立的写回：目录保留并标陈旧，额度保留历史数字与时间并标失败。
    #[tokio::test]
    async fn a_failed_catalog_or_quota_read_keeps_verified_evidence_with_history() {
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
        adapter.fail_models.store(true, Ordering::SeqCst);
        adapter.fail_quota.store(true, Ordering::SeqCst);
        // 目录与额度都失败：命令仍返回 Ok，写回的是「陈旧 + 历史」而不是实时假象。
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.models.len(), 1, "已核实的目录必须原样保留");
        assert_eq!(evidence.catalog.observed_at.as_deref(), Some(STUB_CATALOG_OBSERVED_AT));
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        assert!(evidence.quota.history);
        assert_eq!(evidence.quota.observed_at.as_deref(), Some(STUB_QUOTA_OBSERVED_AT), "失败不得刷新时间");
        assert_eq!(evidence.quota.buckets.len(), 1);
        assert_eq!(evidence.capabilities.len(), 1, "能力证据不因只读刷新失败而丢失");
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "quota_failed");
    }

    /// 目录返回 `unsupported` 不是失败：如实标 unsupported、模型列表为空、来源保留。
    #[tokio::test]
    async fn an_unsupported_catalog_read_is_not_a_failure() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        // 此前有一次成功读取（source/observed_at 都有值），用来钉死「unsupported 不沿用上一次时间」。
        let first = refresh(&store, "codex").await.unwrap();
        assert_eq!(first.current_evidence().unwrap().catalog.observed_at.as_deref(), Some(STUB_CATALOG_OBSERVED_AT));
        *adapter.catalog.lock().unwrap() = CatalogRead {
            state: EvidenceState::Unsupported,
            models: Vec::new(),
            source: Some("fixture:models:v2".into()),
            observed_at: None,
            missing_fields: Vec::new(),
        };
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Unsupported);
        assert!(evidence.models.is_empty(), "固定版本没有该接口即撤销旧条目");
        assert_eq!(evidence.catalog.source.as_deref(), Some("fixture:models:v2"), "source 只取本次事件");
        assert!(evidence.catalog.observed_at.is_none(), "unsupported 不得沿用上一次已核实的观测时间");
        // 本次事件连 source 都没给：同样不得回落到上一次的 source。
        *adapter.catalog.lock().unwrap() = CatalogRead {
            state: EvidenceState::Unsupported,
            models: Vec::new(),
            source: None,
            observed_at: None,
            missing_fields: Vec::new(),
        };
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Unsupported);
        assert!(evidence.catalog.source.is_none(), "缺失即 None，不臆造也不沿用旧来源");
        assert!(evidence.catalog.observed_at.is_none());
        // 额度读取仍然独立成功，不受目录 unsupported 影响。
        assert_eq!(evidence.quota.state, EvidenceState::Available);
    }

    /// 已有已核实身份、本次身份读取不完整：不改写任何字段，也不发起本次目录/额度读取。
    #[tokio::test]
    async fn a_missing_identity_never_overwrites_a_verified_account() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let before = serde_json::to_value(store.read().subscriptions.get("codex").unwrap()).unwrap();
        adapter.status.lock().unwrap().identity = None;
        adapter.status.lock().unwrap().state = ConnectionState::NotConnected;
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("did not report an account identity"), "{error}");
        let after = serde_json::to_value(store.read().subscriptions.get("codex").unwrap()).unwrap();
        assert_eq!(after, before);
        assert_eq!(adapter.catalog_reads.load(Ordering::SeqCst), 1);
        assert_eq!(adapter.quota_reads.load(Ordering::SeqCst), 1);
    }

    /// 读到的身份与已核实身份不符：整体丢弃并报错，绝不把结果挂到旧账号名下。
    #[tokio::test]
    async fn an_identity_mismatch_discards_the_whole_read() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let before = serde_json::to_value(store.read().subscriptions.get("codex").unwrap()).unwrap();
        adapter.status.lock().unwrap().identity = Some("other@example.invalid".into());
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("account changed"), "{error}");
        let after = serde_json::to_value(store.read().subscriptions.get("codex").unwrap()).unwrap();
        assert_eq!(after, before);
    }

    /// 注册表按 kind 分派：只装 Codex 替身时，Grok 行既拿不到适配器也拿不到别家的 helper 自述。
    #[test]
    fn the_registry_dispatch_never_lends_one_provider_the_other_adapter() {
        let registry = SubscriptionAdapters::single(StubAdapter::new());
        assert!(registry.supports(&ProviderKind::CodexSubscription));
        assert!(registry.for_kind(&ProviderKind::GrokSubscription).is_none());
        assert!(!registry.supports(&ProviderKind::GrokSubscription));
        assert!(!registry.available(&ProviderKind::GrokSubscription));
        let grok = registry.helper_status(&ProviderKind::GrokSubscription);
        assert!(!grok.available && grok.version.is_none() && grok.auth_home.is_none());
        let codex = registry.helper_status(&ProviderKind::CodexSubscription);
        assert!(codex.available && codex.version.is_some() && codex.auth_home.is_some());
    }

    /// 视图的 availability 按服务商取自己适配器的结果：一家可用不得算到另一家头上。
    #[test]
    fn the_view_availability_is_taken_per_provider_kind() {
        let mut config = AppConfig::default();
        for (id, kind) in [("codex", ProviderKind::CodexSubscription), ("grok", ProviderKind::GrokSubscription)] {
            let provider = Provider {
                preset: String::new(),
                api_type: String::new(),
                test_model: String::new(),
                id: id.into(),
                name: id.into(),
                kind: kind.clone(),
                base_url: String::new(),
                enabled: true,
                has_api_key: false,
            };
            config.providers.push(provider.clone());
            sync_provider(&mut config, &provider.id, &provider.kind);
        }
        let mut sessions = SessionState::default();
        sessions.supported_providers = vec!["codex".into(), "grok".into()];
        sessions.adapters_available.insert("codex".into(), true);
        sessions.adapters_available.insert("grok".into(), false);
        // 回退值故意传 true：per-kind 结果优先，Grok 行不得借用 Codex 的 availability。
        let items = views(&config, true, &sessions);
        let codex = items.iter().find(|view| view.provider_id == "codex").unwrap();
        let grok = items.iter().find(|view| view.provider_id == "grok").unwrap();
        assert!(codex.adapter_available);
        assert!(!grok.adapter_available);
    }

    /// 没有已核实身份时读到空身份：同样一个字段都不改写（证据必须绑定账号）。
    #[tokio::test]
    async fn an_empty_identity_never_writes_unattributed_evidence() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        adapter.status.lock().unwrap().identity = None;
        adapter.status.lock().unwrap().state = ConnectionState::NotConnected;
        let before = serde_json::to_value(store.read().subscriptions.get("codex").unwrap()).unwrap();
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("did not report an account identity"), "{error}");
        let connection = store.read().subscriptions.get("codex").cloned().unwrap();
        assert!(connection.evidence.is_none());
        assert_eq!(serde_json::to_value(&connection).unwrap(), before);
        // 无法归属就不发起本次目录/额度读取。
        assert_eq!(adapter.catalog_reads.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.quota_reads.load(Ordering::SeqCst), 0);
    }

    /// 一次成功目录读取要能单独呈现「被移除的模型」，恢复后清零；stale 保留旧值。
    #[tokio::test]
    async fn a_removed_catalog_model_is_reported_separately() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        let fixture = |ids: &[&str]| {
            ids.iter()
                .map(|id| DiscoveredModel { model_id: (*id).into(), name: None, eligible: true })
                .collect::<Vec<_>>()
        };
        // 第一次成功读取：目录从「无」变成两个模型，没有任何移除。
        adapter.catalog.lock().unwrap().models = fixture(&["fixture-model", "fixture-extra"]);
        let connection = refresh(&store, "codex").await.unwrap();
        assert!(connection.current_evidence().unwrap().catalog.removed_models.is_empty());
        // 第二次成功读取少了一个：按旧目录顺序记录被移除的标识。
        adapter.catalog.lock().unwrap().models = fixture(&["fixture-extra"]);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.removed_models, vec!["fixture-model"]);
        assert_eq!(evidence.models.len(), 1);
        // 目录读取失败（stale）不动这份记录。
        adapter.fail_models.store(true, Ordering::SeqCst);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.catalog.removed_models, vec!["fixture-model"]);
        // 再次成功读取把模型读回来：移除记录清零。
        adapter.fail_models.store(false, Ordering::SeqCst);
        adapter.catalog.lock().unwrap().models = fixture(&["fixture-model", "fixture-extra"]);
        let connection = refresh(&store, "codex").await.unwrap();
        assert!(connection.current_evidence().unwrap().catalog.removed_models.is_empty());
    }

    /// N1：本次列表不可信（缺 `models` 键或有条目被跳过）时，整份读取都不算权威结果：
    /// 有已核实目录就保留它并标陈旧；没有就标失败、不留空目录冒充可用。
    #[tokio::test]
    async fn an_untrustworthy_catalog_list_never_replaces_a_verified_catalog() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        let fixture = |ids: &[&str]| {
            ids.iter()
                .map(|id| DiscoveredModel { model_id: (*id).into(), name: None, eligible: true })
                .collect::<Vec<_>>()
        };
        // 不可信的读取刻意带上与上一次不同的 source/observed_at：它们不得写进证据。
        let catalog = |models: Vec<DiscoveredModel>, missing: Vec<&str>| CatalogRead {
            state: EvidenceState::Available,
            models,
            source: Some("fixture:models:untrusted".into()),
            observed_at: Some("2026-10-01T00:00:00Z".into()),
            missing_fields: missing.into_iter().map(str::to_owned).collect(),
        };
        // 基线：两次可信读取建立「已核实目录 + 非空移除记录」，好让「保留旧值」可被观察到。
        adapter.catalog.lock().unwrap().models = fixture(&["grok-build", "grok-mini"]);
        refresh(&store, "codex").await.unwrap();
        *adapter.catalog.lock().unwrap() = CatalogRead {
            state: EvidenceState::Available,
            models: fixture(&["grok-mini"]),
            source: Some("fixture:models".into()),
            observed_at: Some(STUB_CATALOG_OBSERVED_AT.into()),
            missing_fields: Vec::new(),
        };
        let connection = refresh(&store, "codex").await.unwrap();
        let verified = connection.current_evidence().unwrap().catalog.clone();
        assert_eq!(verified.state, EvidenceState::Available);
        assert_eq!(verified.removed_models, vec!["grok-build"]);

        // 第二次事件缺 `models` 键：保留已核实目录（模型/来源/时间/移除记录），整份标陈旧。
        *adapter.catalog.lock().unwrap() = catalog(Vec::new(), vec!["models"]);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.models.iter().map(|m| m.model_id.as_str()).collect::<Vec<_>>(), vec!["grok-mini"]);
        assert_eq!(evidence.catalog.source, verified.source);
        assert_eq!(evidence.catalog.observed_at, verified.observed_at);
        assert_eq!(evidence.catalog.removed_models, vec!["grok-build"]);
        assert_eq!(evidence.catalog.missing_fields, vec!["models"], "missing_fields 用本次的记账");

        // 有可用条目被跳过（models[i].id）：同样整份不可信，处理方式相同。
        *adapter.catalog.lock().unwrap() = catalog(fixture(&["grok-mini"]), vec!["models[1].id"]);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.models.iter().map(|m| m.model_id.as_str()).collect::<Vec<_>>(), vec!["grok-mini"]);
        assert_eq!(evidence.catalog.source, verified.source);
        assert_eq!(evidence.catalog.observed_at, verified.observed_at);
        assert_eq!(evidence.catalog.removed_models, vec!["grok-build"]);
        assert_eq!(evidence.catalog.missing_fields, vec!["models[1].id"]);

        // 列表重新可信：整体替换、移除记录清零。
        *adapter.catalog.lock().unwrap() = CatalogRead {
            state: EvidenceState::Available,
            models: fixture(&["grok-build", "grok-mini"]),
            source: Some("fixture:models".into()),
            observed_at: Some(STUB_CATALOG_OBSERVED_AT.into()),
            missing_fields: Vec::new(),
        };
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Available);
        assert_eq!(evidence.models.len(), 2);
        assert!(evidence.catalog.removed_models.is_empty());
    }

    /// N1 的另一半：此前从未成功读过目录时，不可信列表如实报失败、不留「可用 + 0 个模型」。
    #[tokio::test]
    async fn an_untrustworthy_first_catalog_read_is_reported_as_failed() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        *adapter.catalog.lock().unwrap() = CatalogRead {
            state: EvidenceState::Available,
            models: Vec::new(),
            source: Some("fixture:models:untrusted".into()),
            observed_at: Some("2026-10-01T00:00:00Z".into()),
            missing_fields: vec!["models".to_owned()],
        };
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Failed);
        assert!(evidence.models.is_empty());
        assert!(evidence.catalog.source.is_none() && evidence.catalog.observed_at.is_none());
        assert!(evidence.catalog.removed_models.is_empty());
        assert_eq!(evidence.catalog.missing_fields, vec!["models"]);
    }

    #[tokio::test]
    async fn a_late_result_from_another_generation_is_discarded() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        // 读取期间换号：迟到的只读结果不得写入新世代。
        adapter.switch_during_read.store(true, Ordering::SeqCst);
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
        assert!(!store.subscription.available(&ProviderKind::CodexSubscription));
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
        assert!(!store.subscription.available(&ProviderKind::CodexSubscription));
        let views = views(
            &store.read(),
            store.subscription.available(&ProviderKind::CodexSubscription),
            &SessionState::default(),
        );
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
        /// 置位后 `login_result` 立刻返回 `Ok(None)`：模拟辅助进程在授权中途退出。
        helper_exited: AtomicBool,
        renames: Mutex<Vec<(String, String)>>,
        rename_error: Mutex<Option<String>>,
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
                helper_exited: AtomicBool::new(false),
                renames: Mutex::new(Vec::new()),
                rename_error: Mutex::new(None),
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

        /// 替身只支持 Codex 订阅：Grok 必须被明确拒绝，且不得拉起任何进程。
        fn supports(&self, kind: &ProviderKind) -> bool {
            matches!(kind, ProviderKind::CodexSubscription)
        }

        fn helper_status(&self) -> HelperStatus {
            self.helper.clone()
        }

        fn status<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
            Box::pin(async move { Ok(self.status.clone()) })
        }

        fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<CatalogRead>> {
            Box::pin(async {
                Ok(CatalogRead {
                    state: EvidenceState::Available,
                    models: vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
                    source: Some("fixture".into()),
                    observed_at: Some("2026-09-30T00:00:00Z".into()),
                    missing_fields: Vec::new(),
                })
            })
        }

        fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
            Box::pin(async {
                Ok(QuotaEvidence {
                    state: EvidenceState::Available,
                    source: Some("fixture".into()),
                    observed_at: Some("2026-09-30T00:00:00Z".into()),
                    ..QuotaEvidence::default()
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
                if self.helper_exited.load(Ordering::SeqCst) {
                    // 辅助进程中途退出：没有结果可读，如实返回 None。
                    return Ok(None);
                }
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

        fn rename<'a>(&'a self, old_id: &'a str, new_id: &'a str) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.renames.lock().unwrap().push((old_id.to_owned(), new_id.to_owned()));
                if let Some(error) = self.rename_error.lock().unwrap().clone() {
                    bail!("{error}");
                }
                Ok(())
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
        let config = store.read();
        // 与 lib.rs 的 snapshot 一样：per-kind helper 自述 + 真正支持的行。
        guard.helpers = config
            .providers
            .iter()
            .filter(|provider| is_subscription_provider(provider))
            .map(|provider| (provider.id.clone(), store.subscription.helper_status(&provider.kind)))
            .collect();
        guard.supported_providers = config
            .providers
            .iter()
            .filter(|provider| is_subscription_provider(provider) && store.subscription.supports(&provider.kind))
            .map(|provider| provider.id.clone())
            .collect();
        views(&config, adapter_available, &guard).pop().unwrap()
    }

    fn connection(store: &ConfigStore, provider_id: &str) -> Connection {
        store.read().subscriptions.get(provider_id).cloned().unwrap_or_default()
    }

    #[tokio::test]
    async fn login_success_binds_identity_to_the_current_generation() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        assert!(store.subscription.helper_status(&ProviderKind::CodexSubscription).available);
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
        let view = view_of(&store, store.subscription.available(&ProviderKind::CodexSubscription), &sessions).await;
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
                        ..QuotaEvidence::default()
                    },
                    catalog: CatalogEvidence::default(),
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
    async fn logout_without_an_observable_helper_records_unknown_results() {
        let adapter = LifecycleStub::new();
        *adapter.logout_outcome.lock().unwrap() = Err("No subscription helper is available in this build".into());
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        let outcome = logout(&store, "codex", &sessions).await.unwrap();
        // 没拿到适配器的“本地已清”确认：不得声称本地已清除。
        assert!(!outcome.local_cleared);
        let record = session(&sessions, "codex").await.last_logout.unwrap();
        assert!(!record.local_cleared);
        assert_eq!(record.remote, None);
        let view = view_of(&store, true, &sessions).await;
        let logout_view = view.logout.unwrap();
        assert_eq!(logout_view.local, LogoutLocal::Retained);
        assert_eq!(logout_view.remote, LogoutRemote::Unknown);
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
        sessions.supported_providers = vec!["codex".into()];
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

    #[tokio::test]
    async fn unsupported_subscription_kinds_are_rejected_without_touching_state() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
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
                let connection = config.subscriptions.get_mut("grok").unwrap();
                connection.state = ConnectionState::Connected;
                connection.identity = Some("grok@example.invalid".into());
            })
            .unwrap();
        let before = serde_json::to_value(connection(&store, "grok")).unwrap();
        let begin_error = begin_login(&store, "grok", &sessions).await.unwrap_err();
        assert!(begin_error.contains("Grok subscription sign-in is not implemented yet"), "{begin_error}");
        assert!(cancel_login(&store, "grok", &sessions).await.is_err());
        assert!(logout(&store, "grok", &sessions).await.is_err());
        assert!(switch_account(&store, "grok", &sessions).await.is_err());
        assert!(refresh(&store, "grok").await.is_err());
        // 连接状态一点没动，没有伪造会话，也没有拉起任何辅助进程。
        assert_eq!(serde_json::to_value(connection(&store, "grok")).unwrap(), before);
        assert!(sessions.lock().await.session("grok").is_none());
        assert!(adapter.started.lock().unwrap().is_empty());
        assert!(adapter.logouts.lock().unwrap().is_empty());
        assert!(adapter.cancels.lock().unwrap().is_empty());
        // 视图：只有支持的 Codex 行带 helper，Grok 行如实不可用（两家互不冒用）。
        let mut state = SessionState::default();
        state.helper = HelperStatus {
            available: true,
            version: Some("fixture-helper-1.0".into()),
            auth_home: Some("/tmp/fixture-helper-home".into()),
        };
        state.supported_providers = vec!["codex".into()];
        let all = views(&store.read(), true, &state);
        let codex = all.iter().find(|view| view.provider_id == "codex").unwrap();
        let grok = all.iter().find(|view| view.provider_id == "grok").unwrap();
        assert!(codex.helper.available && codex.helper.version.is_some() && codex.helper.auth_home.is_some());
        assert!(!grok.helper.available && grok.helper.version.is_none() && grok.helper.auth_home.is_none());
        assert_eq!(grok.identity.as_deref(), Some("grok@example.invalid"));
    }

    #[tokio::test]
    async fn a_helper_that_exits_mid_sign_in_never_leaves_the_session_pending() {
        let adapter = LifecycleStub::new();
        adapter.helper_exited.store(true, Ordering::SeqCst);
        let (store, _directory, sessions) = fixture(adapter.clone()).await;

        begin_login(&store, "codex", &sessions).await.unwrap();
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Pending);

        // 辅助进程中途退出：与出错同等处理，不得让会话永远停在 Pending。
        let error = await_login(store.clone(), "codex".into(), sessions.clone()).await.unwrap_err();
        assert!(error.contains("exited"), "{error}");
        let settled = session(&sessions, "codex").await;
        assert_eq!(settled.stage, LoginStage::Failed);
        assert!(settled.error.is_some());
        assert_eq!(connection(&store, "codex").state, ConnectionState::NotConnected);

        // 失败后可以再次发起登录，登录入口不会被上一次卡住。
        begin_login(&store, "codex", &sessions).await.unwrap();
        assert_eq!(session(&sessions, "codex").await.stage, LoginStage::Pending);
        assert_eq!(adapter.started.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn disposing_a_subscription_provider_releases_the_helper_before_deletion() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        begin_login(&store, "codex", &sessions).await.unwrap();

        dispose(&store, "codex", &sessions).await.unwrap();

        assert_eq!(adapter.logouts.lock().unwrap().len(), 1, "删除订阅服务商必须先退出");
        assert!(sessions.lock().await.session("codex").is_none(), "内存会话必须一并清除");
        assert_eq!(connection(&store, "codex").state, ConnectionState::NotConnected);

        // 未知服务商是空操作：不得误伤其它状态或重复退出。
        dispose(&store, "missing", &sessions).await.unwrap();
        assert_eq!(adapter.logouts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn renaming_a_subscription_provider_migrates_the_helper_state() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        begin_login(&store, "codex", &sessions).await.unwrap();

        migrate_helper(&store, "codex", "codex-work", &sessions).await.unwrap();

        assert_eq!(
            adapter.renames.lock().unwrap().as_slice(),
            [("codex".to_owned(), "codex-work".to_owned())],
            "改名必须让适配器迁移自有状态，不能只改配置"
        );
        assert!(adapter.logouts.lock().unwrap().is_empty(), "成功迁移不得走退出");
        assert!(sessions.lock().await.session("codex").is_none());
        // 在途尝试绑定旧标识：迁移后作废，不允许新标识停在 Pending。
        assert_eq!(session(&sessions, "codex-work").await.stage, LoginStage::Idle);
    }

    #[tokio::test]
    async fn a_failed_rename_signs_the_old_provider_out_and_forces_a_new_sign_in() {
        let adapter = LifecycleStub::new();
        *adapter.rename_error.lock().unwrap() = Some("the helper home could not be moved".into());
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        store
            .update(|config| {
                let connection = config.subscriptions.get_mut("codex").unwrap();
                connection.state = ConnectionState::Connected;
                connection.identity = Some("old@example.invalid".into());
            })
            .unwrap();
        begin_login(&store, "codex", &sessions).await.unwrap();

        migrate_helper(&store, "codex", "codex-work", &sessions).await.unwrap();

        assert_eq!(adapter.logouts.lock().unwrap().len(), 1, "迁移失败必须对旧标识走退出");
        assert_eq!(session(&sessions, "codex-work").await.stage, LoginStage::Idle);
        // 配置键迁移由 apply_provider_edit 完成；旧身份不得被带到新标识。
        store.update(|config| rename_provider(config, "codex", "codex-work")).unwrap();
        let moved = connection(&store, "codex-work");
        assert_eq!(moved.state, ConnectionState::NotConnected);
        assert!(moved.identity.is_none() && moved.evidence.is_none());
    }

    #[test]
    fn renaming_a_provider_voids_an_in_flight_sign_in() {
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
        config.subscriptions.get_mut("codex").unwrap().state = ConnectionState::AuthorizationPending;

        rename_provider(&mut config, "codex", "codex-work");

        assert!(!config.subscriptions.contains_key("codex"));
        assert_eq!(
            config.subscriptions["codex-work"].state,
            ConnectionState::NotConnected,
            "在途登录不可能跨标识完成，改名后必须回到未连接"
        );
    }
}
