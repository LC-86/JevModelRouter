//! 订阅服务商边界：每家一个活动连接、证据绑定连接世代，以及所有生成入口共用的 fail-closed 准入。
//!
//! 本模块不依赖 Tauri 或 HTTP 框架，连接、证据、准入与只读刷新都能直接单测。适配器只在构造
//! `ConfigStore` 时注入：默认构造（含测试）装的是不提供任何辅助进程的 [`UnavailableAdapter`]，
//! 生产在 `lib.rs` 唯一的 ConfigStore 构造处注入官方 Codex 适配器。两者都由代码固定，配置、环境
//! 与界面都没有把它换成替身的开关；未验证的订阅生成一律拒绝。

pub mod auth;
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
    match provider.kind {
        ProviderKind::CodexSubscription => "Codex",
        ProviderKind::GrokSubscription => "Grok",
        _ => "API",
    }
}

/// 连接的授权状态。身份未确认时按未连接处理；已核实历史只用于展示与同账号恢复。
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
    /// 读取成功但上游账号被明确拒绝（许可为 `denied`）；界面不得把它写成「未知」。
    Denied,
}

/// 订阅额度许可：`ordinaryUsageAllowed` 的只读映射，缺失、null 或非布尔一律视为未知。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPermission {
    #[default]
    Unknown,
    Allowed,
    Denied,
}

/// 额度结果的来源视图：多桶优先，缺失或为空时回落到旧版单桶；两者都没有时为未知。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaView {
    #[default]
    Unknown,
    RateLimitsByLimitId,
    RateLimits,
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

/// 一个额度窗口。`used_percent` 是上游原始百分比，剩余百分比由界面用 `100 - used` 推导，
/// 后端不预先换算、不截断、不补齐越界值。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// `primary` / `secondary` / `single`。
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

/// 上游 credits 的只读映射。`balance` 原样保留字符串，不解析金额、不推断单位。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaCredits {
    #[serde(default)]
    pub has_credits: Option<bool>,
    #[serde(default)]
    pub unlimited: Option<bool>,
    #[serde(default)]
    pub balance: Option<String>,
    #[serde(default)]
    pub missing_fields: Vec<String>,
}

/// 一个额度桶（多桶视图下每个 `limitId` 一个，旧版单桶视图下只有一个）。
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

/// 目录证据的元数据；目录条目本身仍放在 `Evidence::models`。
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
}

/// 一次目录读取的完整结果：条目 + 解析时记下的缺失字段 + 来源与时间。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRead {
    #[serde(default)]
    pub models: Vec<DiscoveredModel>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
    #[serde(default)]
    pub missing_fields: Vec<String>,
}

/// 额度状态规则（契约 B）：至少一个桶时，任一桶 denied → Denied；
/// 全部桶 allowed 且至少一个窗口有有效 `used_percent` → Available；否则 Unknown。
pub(crate) fn quota_state(buckets: &[QuotaBucket]) -> EvidenceState {
    if buckets.is_empty() {
        return EvidenceState::Unknown;
    }
    if buckets.iter().any(|bucket| bucket.permission == QuotaPermission::Denied) {
        return EvidenceState::Denied;
    }
    let all_allowed = buckets.iter().all(|bucket| bucket.permission == QuotaPermission::Allowed);
    let measured = buckets.iter().flat_map(|bucket| bucket.windows.iter()).any(|window| window.used_percent.is_some());
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
        (self.state == ConnectionState::Connected).then(|| self.account_evidence()).flatten()
    }

    /// 严格绑定账号与世代的历史；不代表当前授权，也不供生成准入使用。
    fn account_evidence(&self) -> Option<&Evidence> {
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
    /// 读取不完整与明确退出分开；仅适配器边界内部使用，不持久化。
    pub identity_incomplete: bool,
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
    // denied 是上游对普通包含用量/已知耗尽的明确拒绝，刷新并不能恢复，不能给误导性的重试建议。
    let recovery = match state {
        EvidenceState::Denied => "Use an API provider for this request, or wait until the upstream account restores included usage.",
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
    // 资格只看账号目录：同一账号同一世代的读取失败仍算合格（网络失败 ≠ 被移除）。
    // 只读证据里的 `eligible` 是当次读取的原始结果，不再单独构成调用依据。
    let eligibility = crate::subscription_catalog::eligibility(config, &provider.id, model_id);
    if !eligibility.is_eligible() {
        return Err(Denial::new(
            eligibility.code().unwrap_or("model_unqualified"),
            DenialFamily::NotEligible,
            format!("{} cannot use {model_id}: {}.", label(provider), eligibility.reason()),
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
/// 目标若已有对应模型行（模型测试/测速入口也会走到这里），停用优先于资格：禁止一切调用。
pub fn admit_target(config: &AppConfig, provider: &Provider, model_id: &str, protocol: Protocol) -> Result<(), Denial> {
    let model_id = model_id.trim();
    let model = config
        .models
        .iter()
        .find(|model| model.provider_id == provider.id && model.model_id == model_id);
    evaluate(config, provider, model, model_id, protocol)
}

/// 自动选路用的过滤条件：被拒绝的订阅模型不进入候选。
pub fn generation_ready(config: &AppConfig, model: &Model, protocol: Protocol) -> bool {
    config
        .providers
        .iter()
        .find(|provider| provider.id == model.provider_id)
        .is_some_and(|provider| admit_model(config, model, provider, protocol).is_ok())
}

/// 可派发候选的口径：公共目录（`/v1/models`）、Agent 可选列表与自动候选要求模型已选、未停用、
/// 服务商启用，且订阅模型在当前账号与世代下资格合格。
/// 界面「模型列表」另展示已停用的已选项（行内标注停用），因此与这里不等价；
/// 取消选择只影响这些集合，显式原 ID 直调不经过这里。
pub fn catalog_listed(config: &AppConfig, model: &Model) -> bool {
    if !model.selected || !model.enabled {
        return false;
    }
    let Some(provider) = config.providers.iter().find(|provider| provider.id == model.provider_id) else {
        return false;
    };
    if !provider.enabled {
        return false;
    }
    if !is_subscription_provider(provider) {
        return true;
    }
    crate::subscription_catalog::eligibility(config, &provider.id, &model.model_id).is_eligible()
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
/// 身份改绑同时作废账号目录资格：保留稳定标识、选择与停用，等重新核对再恢复资格。
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
            crate::subscription_catalog::invalidate_account(config, provider_id);
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
            let generation = {
                let connection = config.subscriptions.get_mut(provider_id).expect("checked above");
                connection.generation += 1;
                let generation = connection.generation;
                connection.state = ConnectionState::NotConnected;
                connection.identity = None;
                connection.evidence = None;
                generation
            };
            // 退出后旧账号的目录资格整体作废：保留稳定标识、选择与停用，等重核再恢复。
            crate::subscription_catalog::invalidate_account(config, provider_id);
            generation
        })
        .map_err(|error| error.to_string())?;
    let previous_generation = generation.saturating_sub(1);
    let adapter = store.subscription.logout(provider_id, previous_generation).await;
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
    if store.subscription.supports(&kind) {
        if let Err(error) = store.subscription.rename(old_id, new_id).await {
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

/// 服务商保存后同步连接记录：订阅服务商得到一个活动连接，API 服务商不保留订阅身份。
pub fn sync_provider(config: &mut AppConfig, provider_id: &str, kind: &ProviderKind) {
    if is_subscription(kind) {
        config.subscriptions.entry(provider_id.to_owned()).or_default();
    } else {
        config.subscriptions.remove(provider_id);
    }
}

/// 服务商标识重命名时迁移连接，保留世代与已核实身份；订阅目录随标识一起迁移。
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
    crate::subscription_catalog::rename_provider(config, old_id, new_id);
}

/// 删除服务商时一并丢弃其连接、证据与订阅目录。
pub fn forget_provider(config: &mut AppConfig, provider_id: &str) {
    config.subscriptions.remove(provider_id);
    crate::subscription_catalog::forget_provider(config, provider_id);
}

// CHUNK-6

/// 此前是否有一份已核实的目录：成功读过（可能是空目录），或保留了非空模型列表。
/// 只有它成立时，读取失败才标 `Stale` 并保留旧目录；否则如实标 `Failed`。
fn had_verified_catalog(previous: Option<&Evidence>) -> bool {
    previous.is_some_and(|evidence| {
        !evidence.models.is_empty() || matches!(evidence.catalog.state, EvidenceState::Available | EvidenceState::Stale)
    })
}

/// 只读刷新：按当前连接世代读取状态、目录与额度并写回证据。
/// 它与生成准入无关，因此生成被拒绝、网关暂停或服务商停用时仍可执行。
///
/// 身份不完整或状态读取失败：立即阻止准入，保留同账号同世代的历史并标陈旧/失败，
/// 不发起本次目录与额度读取。明确未登录：推进世代并清空身份与所有证据。
pub async fn refresh(store: &ConfigStore, provider_id: &str) -> Result<Connection, String> {
    let request = store.begin_subscription_refresh(provider_id)?;
    let provider = require_subscription_provider(store, provider_id)?;
    // 不支持的订阅类型不得借用其它服务商的辅助进程读取证据（#12「两家互不冒用」）。
    require_supported(store, &provider, "read-only status")?;
    let before = store.read().subscriptions.get(provider_id).cloned().unwrap_or_default();
    if before.state == ConnectionState::AuthorizationPending {
        // 挂起登录期间只读刷新不得伪装成已连接，也不得让旧身份复活：一个连接字段都不改写。
        return Ok(before);
    }
    let generation = before.generation;
    let (status, status_error) = match store.subscription.status(provider_id, generation).await {
        Ok(status) => (status, None),
        Err(error) => (ConnectionStatus { identity_incomplete: true, ..ConnectionStatus::default() }, Some(error.to_string())),
    };
    let read_identity = status.identity.clone().filter(|identity| !identity.trim().is_empty());
    // 必须先落盘当前失效状态，不能由“保留历史”的身份守卫忽略 helper 的明确退出。
    if status.identity_incomplete || status.state != ConnectionState::Connected || read_identity.is_none() {
        let incomplete = status.identity_incomplete || (status.state == ConnectionState::Connected && read_identity.is_none());
        let result = store.update_subscription_refresh(provider_id, request, |config| -> Result<Connection, String> {
            if !config.providers.iter().any(|provider| provider.id == provider_id && is_subscription_provider(provider)) {
                return Err("The subscription provider was removed while refreshing".into());
            }
            let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
            if connection.generation != generation || connection.identity != before.identity || connection.state != before.state {
                return Err("The connection changed while refreshing; the read-only result was discarded".into());
            }
            if incomplete {
                connection.state = ConnectionState::NotConnected;
                let had_catalog = had_verified_catalog(connection.account_evidence());
                if let Some(evidence) = connection.evidence.as_mut() {
                    evidence.catalog.state = if had_catalog { EvidenceState::Stale } else { EvidenceState::Failed };
                    evidence.quota.state = EvidenceState::Failed;
                    evidence.quota.history = evidence.quota.observed_at.is_some() || !evidence.quota.buckets.is_empty();
                }
            } else {
                if connection.identity.is_some() || connection.evidence.is_some() || connection.state != status.state {
                    connection.generation += 1;
                }
                connection.state = status.state;
                connection.identity = None;
                connection.evidence = None;
            }
            // #32：身份未核实或明确退出后不再保留任何账号资格：目录行标未知，选择与停用保留。
            // 这里只作废资格，不改写 #31 已表达的历史证据保留（catalog/quota 的 Stale／history）。
            let result = connection.clone();
            crate::subscription_catalog::invalidate_account(config, provider_id);
            Ok(result)
        })?;
        return if incomplete {
            Err(status_error.unwrap_or_else(|| "The Codex helper could not confirm which account the data belongs to; connection suspended and history retained".into()))
        } else {
            Ok(result)
        };
    }
    if before.identity.as_ref().is_some_and(|verified| Some(verified) != read_identity.as_ref()) {
        return Err("The account changed while refreshing; the read-only result was discarded".into());
    }
    // 2) 目录与额度独立读取：任一失败都不影响另一项写回。
    let catalog_read = store.subscription.models(provider_id, generation).await;
    let quota_read = store.subscription.quota(provider_id, generation).await;
    // #32：目录读取成败与本次读取时间在写入前确定，供目录资格核对使用。
    let catalog_failed = catalog_read.is_err();
    let catalog_observed_at = catalog_read.as_ref().ok().and_then(|read| read.observed_at.clone());
    store.update_subscription_refresh(provider_id, request, |config| -> Result<Connection, String> {
        // 服务商可能在读取期间被删除或换号：迟到的只读结果不得复活或污染连接。
        if !config.providers.iter().any(|provider| provider.id == provider_id && is_subscription_provider(provider)) {
            return Err("The subscription provider was removed while refreshing".into());
        }
        let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
        if connection.generation != generation || connection.identity != before.identity || connection.state != before.state {
            return Err("The connection changed while refreshing; the read-only result was discarded".into());
        }
        let previous = connection.account_evidence().cloned();
        // 目录：成功即以本次权威结果整体替换；失败保留已核实目录并标陈旧，没有目录才标失败。
        let (models, catalog) = match catalog_read {
            Ok(read) => (
                read.models,
                CatalogEvidence {
                    state: EvidenceState::Available,
                    source: read.source,
                    observed_at: read.observed_at,
                    missing_fields: read.missing_fields,
                },
            ),
            Err(_) if had_verified_catalog(previous.as_ref()) => {
                let kept = previous.as_ref().map(|evidence| evidence.catalog.clone()).unwrap_or_default();
                (previous.as_ref().map(|evidence| evidence.models.clone()).unwrap_or_default(), CatalogEvidence { state: EvidenceState::Stale, ..kept })
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
        connection.identity = read_identity.clone();
        connection.evidence = Some(Evidence {
            generation,
            account: read_identity,
            helper_version: status.helper_version,
            account_path: status.account_path,
            models: models.clone(),
            capabilities,
            quota,
            catalog,
        });
        let account = connection.identity.clone();
        let result = connection.clone();
        // #32 目录资格衔接：本次权威目录读取失败只标陈旧（保留已核实项与选择／停用），
        // 读取成功则按当前已核实账号与世代整体重核；发现本身不构成调用资格。
        if catalog_failed {
            crate::subscription_catalog::mark_stale(config, provider_id);
        } else if let Some(account) = account.as_deref() {
            crate::subscription_catalog::reconcile(config, provider_id, account, generation, &models, catalog_observed_at.as_deref());
        }
        Ok(result)
    })
}

/// 订阅目录行：稳定标识（`model_id` 为上游身份、`internal_id` 为内部标识）与用户配置分开表达。
/// 选择与停用是用户配置，可用性与资格来自账号目录；界面按这三者分别展示。
#[derive(Clone, Debug, Serialize)]
pub struct SubscriptionCatalogView {
    pub model_id: String,
    pub name: Option<String>,
    pub internal_id: String,
    pub availability: crate::subscription_catalog::Availability,
    pub eligibility: crate::subscription_catalog::Eligibility,
    pub selected: bool,
    pub disabled: bool,
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
    /// 订阅目录行：按服务商取全部已建档模型，逐行给出当前账号与世代下的资格。
    /// 与只读证据字段 `catalog`（`CatalogEvidence`）分开：这里是用户配置与账号资格的投影。
    pub catalog_entries: Vec<SubscriptionCatalogView>,
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
            let evidence = connection.account_evidence();
            let session = sessions.session(&provider.id);
            let known = crate::subscription_catalog::catalog(config, &provider.id);
            // 目录视图按模型行组装：选择/停用来自用户配置，可用性与资格来自账号目录。
            let catalog: Vec<_> = crate::subscription_catalog::models(config, &provider.id)
                .into_iter()
                .map(|model| {
                    let entry = known.and_then(|known| known.entry(&model.model_id));
                    SubscriptionCatalogView {
                        model_id: model.model_id.clone(),
                        name: entry
                            .and_then(|entry| entry.name.clone())
                            .or_else(|| (!model.name.is_empty()).then(|| model.name.clone())),
                        internal_id: model.id.clone(),
                        availability: entry.map(|entry| entry.availability).unwrap_or_default(),
                        eligibility: crate::subscription_catalog::eligibility(
                            config,
                            &provider.id,
                            &model.model_id,
                        ),
                        selected: model.selected,
                        disabled: !model.enabled,
                    }
                })
                .collect();
            // 适配器不支持这类服务商时，如实报 helper 不可用：不借用其它服务商的进程信息。
            let supported = sessions.supports(&provider.id);
            SubscriptionView {
                provider_id: provider.id.clone(),
                label: label(provider).to_owned(),
                generation: connection.generation,
                state: connection.state,
                identity: connection.identity.clone(),
                helper_version: evidence.and_then(|evidence| evidence.helper_version.clone()),
                account_path: evidence.and_then(|evidence| evidence.account_path.clone()),
                models: evidence.map(|evidence| evidence.models.clone()).unwrap_or_default(),
                capabilities: connection.current_evidence().map(|evidence| evidence.capabilities.clone()).unwrap_or_default(),
                catalog: evidence.map(|evidence| evidence.catalog.clone()).unwrap_or_default(),
                quota: evidence.map(|evidence| evidence.quota.clone()).unwrap_or_default(),
                catalog_entries: catalog,
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
                    available: sessions.helper.available && supported,
                    version: if supported { sessions.helper.version.clone() } else { None },
                    auth_home: if supported { sessions.helper.auth_home.clone() } else { None },
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
            selected: true,
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
        let models = vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }];
        let (identity, generation) = {
            let connection = config.subscriptions.entry(FIXTURE_PROVIDER.to_owned()).or_default();
            let generation = connection.generation;
            connection.state = ConnectionState::Connected;
            connection.identity = Some("fixture@example.invalid".into());
            connection.evidence = Some(Evidence {
                generation,
                account: connection.identity.clone(),
                helper_version: Some("fixture-helper-1.0".into()),
                account_path: Some("/tmp/fixture-account".into()),
                models: models.clone(),
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
            (connection.identity.clone().unwrap(), generation)
        };
        // 账号目录是资格来源：已核实身份与当前世代下核对一次，模型才有调用资格。
        crate::subscription_catalog::reconcile(
            config,
            FIXTURE_PROVIDER,
            &identity,
            generation,
            &models,
            Some("2026-09-30T00:00:00Z"),
        );
    }

    /// 以当前已核实身份与世代重新核对目录（用于制造撤销/移除等资格变化）。
    fn reconcile_fixture(config: &mut AppConfig, discovered: &[DiscoveredModel]) {
        let (identity, generation) = {
            let connection = config.subscriptions.get(FIXTURE_PROVIDER).unwrap();
            (connection.identity.clone().unwrap(), connection.generation)
        };
        crate::subscription_catalog::reconcile(
            config,
            FIXTURE_PROVIDER,
            &identity,
            generation,
            discovered,
            Some("2026-09-30T00:00:00Z"),
        );
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

    /// 取当前配置里的 fixture 模型行：用例改动配置后必须重新取，避免用到旧快照。
    fn stored_model(config: &AppConfig) -> Model {
        config.models.iter().find(|model| model.provider_id == FIXTURE_PROVIDER).unwrap().clone()
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
        // API 模型不因本票改变：默认已选、出现在模型列表里。
        assert!(api_model.selected);
        assert!(catalog_listed(&config, &api_model));
        // 取消选择把 API 模型移出列表，但直调不受影响。
        config.models[0].selected = false;
        assert!(!catalog_listed(&config, &config.models[0]));
        assert!(admit_model(&config, &api_model, &api_provider, Protocol::Chat).is_ok());
        config.models[0].selected = true;
        // 另一份未连接的订阅服务商不影响 API 服务商本身。
        config.providers.push(provider("codex", ProviderKind::CodexSubscription));
        assert!(admit_model(&config, &api_model, &api_provider, Protocol::Chat).is_ok());
        assert!(catalog_listed(&config, &config.models[0]));
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
            ("model_unqualified", Box::new(|config: &mut AppConfig| {
                connected(config);
                // 身份/世代未变但资格被作废（退出、换号尚未重核）：稳定拒绝码是 model_unqualified。
                crate::subscription_catalog::invalidate_account(config, FIXTURE_PROVIDER);
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
    fn eligibility_codes_come_from_the_account_catalog() {
        let cases: Vec<(&str, Box<dyn Fn(&mut AppConfig)>)> = vec![
            ("model_not_discovered", Box::new(|config: &mut AppConfig| {
                connected(config);
                // 目录里没有这个模型：视为本账号未发现。
                crate::subscription_catalog::forget_provider(config, FIXTURE_PROVIDER);
            })),
            ("model_revoked", Box::new(|config: &mut AppConfig| {
                connected(config);
                // 上游明确报告权限拒绝时才标撤销；当前 Codex 适配器只报告发现（eligible=false 是
                // 「未确认资格」，不是撤销），因此这里直接构造该状态来固化拒绝码。
                reconcile_fixture(config, &[DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }]);
                config.subscription_catalogs.get_mut(FIXTURE_PROVIDER).unwrap()
                    .entry_mut("fixture-model").unwrap()
                    .availability = crate::subscription_catalog::Availability::Revoked;
            })),
            ("model_removed", Box::new(|config: &mut AppConfig| {
                connected(config);
                // 权威目录不再包含该模型。
                reconcile_fixture(config, &[]);
            })),
            ("model_unqualified", Box::new(|config: &mut AppConfig| {
                connected(config);
                crate::subscription_catalog::invalidate_account(config, FIXTURE_PROVIDER);
            })),
        ];
        for (expected, change) in cases {
            let (mut config, provider, model) = fixture(ProviderKind::CodexSubscription);
            change(&mut config);
            let denial = denial(&config, &model, &provider);
            assert_eq!(denial.code, expected, "unexpected code for {expected}");
            assert_eq!(denial.family, DenialFamily::NotEligible);
            assert!(!catalog_listed(&config, &model), "{expected} must not be listed");
        }
    }

    #[test]
    fn disabling_a_model_or_provider_denies_every_entry_including_direct_calls() {
        let (mut config, provider, _) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        assert!(admit_model(&config, &stored_model(&config), &provider, Protocol::Chat).is_ok());
        assert!(admit_target(&config, &provider, "fixture-model", Protocol::Chat).is_ok());
        // 停用模型：显式原 ID 直调、测试/测速（按同一目标 ID）与自动候选全被拒。
        config.models.iter_mut().find(|m| m.provider_id == FIXTURE_PROVIDER).unwrap().enabled = false;
        assert_eq!(denial(&config, &stored_model(&config), &provider).code, "model_disabled");
        assert_eq!(
            admit_target(&config, &provider, "fixture-model", Protocol::Chat).unwrap_err().code,
            "model_disabled"
        );
        assert!(!generation_ready(&config, &stored_model(&config), Protocol::Chat));
        assert!(!catalog_listed(&config, &stored_model(&config)));
        config.models.iter_mut().find(|m| m.provider_id == FIXTURE_PROVIDER).unwrap().enabled = true;
        // 停用服务商：所有入口同样禁止。
        config.providers.iter_mut().find(|p| p.id == provider.id).unwrap().enabled = false;
        let disabled_provider = config.providers.iter().find(|p| p.id == provider.id).unwrap().clone();
        assert_eq!(denial(&config, &stored_model(&config), &disabled_provider).code, "provider_disabled");
        assert_eq!(
            admit_target(&config, &disabled_provider, "fixture-model", Protocol::Chat).unwrap_err().code,
            "provider_disabled"
        );
        assert!(!catalog_listed(&config, &stored_model(&config)));
    }

    #[test]
    fn cancelling_selection_only_removes_the_model_from_lists_and_candidates() {
        let (mut config, provider, _) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        config.models.iter_mut().find(|m| m.provider_id == FIXTURE_PROVIDER).unwrap().selected = false;
        let model = stored_model(&config);
        // 取消选择不禁止调用：显式原 ID 直调与测试入口在其它条件满足时仍然放行。
        assert!(admit_model(&config, &model, &provider, Protocol::Chat).is_ok());
        assert!(admit_target(&config, &provider, "fixture-model", Protocol::Chat).is_ok());
        // 但不再出现在模型列表与自动候选里。
        assert!(!catalog_listed(&config, &model));
        assert!(!crate::subscription_catalog::selected_models(&config, FIXTURE_PROVIDER)
            .iter()
            .any(|entry| entry.id == model.id));
        let view = views(&config, false, &SessionState::default()).pop().unwrap();
        let row = view.catalog_entries.iter().find(|row| row.internal_id == model.id).unwrap();
        assert_eq!(row.model_id, "fixture-model");
        assert!(!row.selected && !row.disabled);
        assert_eq!(row.availability, crate::subscription_catalog::Availability::Available);
        assert_eq!(row.eligibility, crate::subscription_catalog::Eligibility::Eligible);
    }

    #[test]
    fn newly_discovered_models_are_materialized_unselected_with_a_stable_identity() {
        let (mut config, provider, _) = fixture(ProviderKind::CodexSubscription);
        // 该服务商还没有任何模型行：核对后建档。
        config.models.clear();
        connected(&mut config);
        let model = config.models.iter().find(|entry| entry.provider_id == provider.id).unwrap().clone();
        assert_eq!(model.model_id, "fixture-model");
        assert!(!model.selected && model.enabled, "新发现模型默认未选但未停用");
        let entry = crate::subscription_catalog::catalog(&config, FIXTURE_PROVIDER)
            .and_then(|catalog| catalog.entry("fixture-model"))
            .unwrap()
            .clone();
        assert_eq!(entry.internal_id, model.id, "目录内部标识必须等于对应模型行标识");
        assert_eq!(
            crate::subscription_catalog::eligibility(&config, FIXTURE_PROVIDER, "fixture-model"),
            crate::subscription_catalog::Eligibility::Eligible
        );
        let view = views(&config, false, &SessionState::default()).pop().unwrap();
        assert_eq!(view.catalog_entries.len(), 1);
        assert_eq!(view.catalog_entries[0].internal_id, model.id);
        assert!(!view.catalog_entries[0].selected && !view.catalog_entries[0].disabled);
    }

    #[test]
    fn stale_qualification_from_the_same_account_is_still_eligible() {
        let (mut config, provider, model) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        crate::subscription_catalog::mark_stale(&mut config, FIXTURE_PROVIDER);
        let view = views(&config, false, &SessionState::default()).pop().unwrap();
        let row = view.catalog_entries.iter().find(|row| row.internal_id == model.id).unwrap();
        assert_eq!(row.availability, crate::subscription_catalog::Availability::Stale);
        assert_eq!(row.eligibility, crate::subscription_catalog::Eligibility::Stale);
        // 网络失败不等于被移除：陈旧但同账号同世代的资格仍然成立。
        assert!(crate::subscription_catalog::eligibility(&config, FIXTURE_PROVIDER, "fixture-model").is_eligible());
        assert!(admit_model(&config, &model, &provider, Protocol::Chat).is_ok());
        assert!(catalog_listed(&config, &model));
    }

    #[test]
    fn account_and_provider_changes_keep_the_user_configuration_and_stable_ids() {
        let (mut config, provider, model) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        let materialized = config
            .models
            .iter()
            .find(|entry| entry.provider_id == FIXTURE_PROVIDER && entry.model_id == "fixture-model")
            .unwrap()
            .clone();
        assert_eq!(materialized.id, model.id);
        config.models.iter_mut().find(|entry| entry.id == model.id).unwrap().selected = false;
        config.models.iter_mut().find(|entry| entry.id == model.id).unwrap().enabled = false;
        // 退出/换号作废资格，但保留稳定标识与用户配置。
        crate::subscription_catalog::invalidate_account(&mut config, FIXTURE_PROVIDER);
        assert_eq!(
            crate::subscription_catalog::eligibility(&config, FIXTURE_PROVIDER, "fixture-model"),
            crate::subscription_catalog::Eligibility::AccountChanged
        );
        let kept = config.models.iter().find(|entry| entry.id == model.id).unwrap();
        assert!(!kept.selected && !kept.enabled);
        // 重核后按同一上游 ID 恢复资格，标识不变。
        connected(&mut config);
        let restored = config.models.iter().find(|entry| entry.model_id == "fixture-model").unwrap();
        assert_eq!(restored.id, model.id);
        assert_eq!(
            crate::subscription_catalog::eligibility(&config, FIXTURE_PROVIDER, "fixture-model"),
            crate::subscription_catalog::Eligibility::Eligible
        );
        assert!(provider.enabled);
    }

    #[test]
    fn provider_rename_and_removal_move_the_catalog_with_the_connection() {
        let (mut config, provider, _) = fixture(ProviderKind::CodexSubscription);
        connected(&mut config);
        assert!(crate::subscription_catalog::catalog(&config, FIXTURE_PROVIDER).is_some());
        rename_provider(&mut config, &provider.id, "codex-renamed");
        assert!(crate::subscription_catalog::catalog(&config, FIXTURE_PROVIDER).is_none());
        assert!(crate::subscription_catalog::catalog(&config, "codex-renamed").is_some());
        forget_provider(&mut config, "codex-renamed");
        assert!(crate::subscription_catalog::catalog(&config, "codex-renamed").is_none());
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

    /// 只读替身：只在测试代码里存在，用来驱动适配边界本身。
    /// 它没有任何生产入口，配置与界面都无法把它装进应用。
    struct StubAdapter {
        status: Mutex<ConnectionStatus>,
        models: Mutex<Vec<DiscoveredModel>>,
        catalog_missing: Mutex<Vec<String>>,
        catalog_time: Mutex<String>,
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
        pause_models: AtomicBool,
        models_started: tokio::sync::Notify,
        resume_models: tokio::sync::Notify,
        pause_status: AtomicBool,
        status_started: tokio::sync::Notify,
        resume_status: tokio::sync::Notify,
    }

    /// 替身写回的固定读取时间：失败后必须原样保留，不能被刷新成「现在」。
    const STUB_CATALOG_OBSERVED_AT: &str = "2026-09-30T00:00:00Z";
    const STUB_QUOTA_OBSERVED_AT: &str = "2026-09-30T01:00:00Z";

    impl StubAdapter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                status: Mutex::new(ConnectionStatus {
                    identity_incomplete: false,
                    state: ConnectionState::Connected,
                    identity: Some("fixture@example.invalid".into()),
                    helper_version: Some("fixture-helper-1.0".into()),
                    account_path: Some("/tmp/fixture-account".into()),
                }),
                // 替身模拟「#17 已定义资格」之后的世界：只读刷新本身从不改动 eligible；
                // 真实 CodexAdapter 一律发现即 false（见 codex_helper 的目录单测）。
                models: Mutex::new(vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }]),
                catalog_missing: Mutex::new(Vec::new()),
                catalog_time: Mutex::new(STUB_CATALOG_OBSERVED_AT.into()),
                quota: Mutex::new(QuotaEvidence {
                    state: EvidenceState::Available,
                    source: Some("fixture:account/rateLimits/read".into()),
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
                pause_models: AtomicBool::new(false),
                models_started: tokio::sync::Notify::new(),
                resume_models: tokio::sync::Notify::new(),
                pause_status: AtomicBool::new(false),
                status_started: tokio::sync::Notify::new(),
                resume_status: tokio::sync::Notify::new(),
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
                if self.switch_during_read.load(Ordering::SeqCst) {
                    if let Some(store) = self.store.lock().unwrap().clone() {
                        store.update(|config| {
                            if let Some(connection) = config.subscriptions.get_mut(provider_id) {
                                connection.generation += 1;
                            }
                        })?;
                    }
                }
                let result = if self.fail_status.load(Ordering::SeqCst) {
                    Err(anyhow::anyhow!("fixture helper status read failed"))
                } else {
                    Ok(self.status.lock().unwrap().clone())
                };
                if self.pause_status.swap(false, Ordering::SeqCst) {
                    self.status_started.notify_one();
                    self.resume_status.notified().await;
                }
                result
            })
        }

        fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<CatalogRead>> {
            Box::pin(async move {
                self.catalog_reads.fetch_add(1, Ordering::SeqCst);
                if self.fail_models.load(Ordering::SeqCst) {
                    bail!("fixture catalog read failed");
                }
                let read = CatalogRead {
                    models: self.models.lock().unwrap().clone(),
                    source: Some("fixture:model/list".into()),
                    observed_at: Some(self.catalog_time.lock().unwrap().clone()),
                    missing_fields: self.catalog_missing.lock().unwrap().clone(),
                };
                if self.pause_models.swap(false, Ordering::SeqCst) {
                    self.models_started.notify_one();
                    self.resume_models.notified().await;
                }
                Ok(read)
            })
        }

        fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
            Box::pin(async move {
                self.quota_reads.fetch_add(1, Ordering::SeqCst);
                if self.fail_quota.load(Ordering::SeqCst) {
                    bail!("fixture quota read failed");
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

    /// 配置里当前落盘的连接。
    fn stored(store: &ConfigStore) -> Connection {
        store.read().subscriptions.get("codex").cloned().unwrap_or_default()
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
        assert_eq!(evidence.models[0].model_id, "fixture-model");
        assert_eq!(evidence.quota.state, EvidenceState::Available);
        assert_eq!(evidence.quota.view, QuotaView::RateLimitsByLimitId);
        assert!(!evidence.quota.history);
        assert_eq!(evidence.quota.buckets.len(), 1);
        assert_eq!(evidence.quota.buckets[0].permission, QuotaPermission::Allowed);
        assert_eq!(evidence.quota.buckets[0].windows[0].used_percent, Some(42.0));
        // 目录成功读取记下来源与时间，并覆盖上一次的权威结果。
        assert_eq!(evidence.catalog.state, EvidenceState::Available);
        assert_eq!(evidence.catalog.source.as_deref(), Some("fixture:model/list"));
        assert_eq!(evidence.catalog.observed_at.as_deref(), Some(STUB_CATALOG_OBSERVED_AT));
        // 成功刷新把权威目录写进目录状态：可用、绑定当前账号与世代。
        let catalog = crate::subscription_catalog::catalog(&store.read(), "codex").cloned().unwrap();
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].model_id, "fixture-model");
        assert_eq!(catalog.entries[0].availability, crate::subscription_catalog::Availability::Available);
        assert_eq!(catalog.entries[0].account.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(catalog.entries[0].confirmed_generation, Some(1));
        assert_eq!(
            crate::subscription_catalog::eligibility(&store.read(), "codex", "fixture-model"),
            crate::subscription_catalog::Eligibility::Eligible
        );
        // 已有模型行的用户配置不因一次目录核对改变；目录条目的内部标识必须等于该模型行。
        let existing = store.read().models.iter().find(|model| model.provider_id == "codex").unwrap().clone();
        assert!(existing.selected && existing.enabled);
        assert_eq!(catalog.entries[0].internal_id, existing.id);
        // 证据齐备后仍然缺能力记录：未验证能力不会因为一次只读刷新而被视为已验证。
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "capability_unverified");
        let view = views(&store.read(), true, &SessionState::default()).pop().unwrap();
        assert_eq!(view.identity.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(view.generation, 1);
        assert!(view.adapter_available);
        assert_eq!(view.catalog.state, EvidenceState::Available);
        assert_eq!(view.quota.buckets.len(), 1);
        // 连接级检查此时已通过；缺能力仍会挡在生成准入上。
        assert!(view.denial.is_none());
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["catalog"]["state"], "available");
        assert_eq!(json["quota"]["view"], "rate_limits_by_limit_id");
        assert_eq!(json["quota"]["history"], false);
    }

    #[tokio::test]
    async fn a_successful_catalog_read_replaces_the_previous_directory_wholesale() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        // 旧列表中不再出现的模型即被撤销：本次权威结果整体替换，而不是并集。
        adapter.models.lock().unwrap().clear();
        adapter.models.lock().unwrap().push(DiscoveredModel { model_id: "second-model".into(), name: None, eligible: false });
        adapter.catalog_missing.lock().unwrap().push("model.list[0].id".into());
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.models.len(), 1);
        assert_eq!(evidence.models[0].model_id, "second-model");
        assert_eq!(evidence.catalog.state, EvidenceState::Available);
        assert_eq!(evidence.catalog.missing_fields, vec!["model.list[0].id".to_owned()]);
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
    async fn a_status_read_failure_suspends_connection_and_preserves_history() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let before = serde_json::to_value(stored(&store)).unwrap();
        adapter.fail_status.store(true, Ordering::SeqCst);
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("fixture helper status read failed"), "{error}");
        let after = stored(&store);
        assert_eq!(after.state, ConnectionState::NotConnected);
        assert!(after.current_evidence().is_none());
        let history = serde_json::to_value(&after).unwrap();
        assert_eq!(history["evidence"]["models"], before["evidence"]["models"]);
        assert_eq!(history["evidence"]["quota"]["buckets"], before["evidence"]["quota"]["buckets"]);
        assert_eq!(history["evidence"]["quota"]["observed_at"], before["evidence"]["quota"]["observed_at"]);
        assert_eq!(adapter.reads.lock().unwrap().len(), 2, "status is read once per refresh");
    }

    #[tokio::test]
    async fn a_failed_catalog_read_keeps_the_verified_directory_and_marks_it_stale() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let previous = serde_json::to_value(stored(&store)).unwrap();
        adapter.fail_models.store(true, Ordering::SeqCst);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        // 目录失败：保留已核实目录、来源与时间，只标陈旧；额度不受影响照常刷新。
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.models.len(), 1);
        assert_eq!(evidence.catalog.source.as_deref(), Some("fixture:model/list"));
        assert_eq!(evidence.catalog.observed_at.as_deref(), Some(STUB_CATALOG_OBSERVED_AT));
        assert_eq!(evidence.quota.state, EvidenceState::Available);
        assert!(!evidence.quota.history);
        // 保留的那部分与上一次完全一致：失败不刷新目录时间，也不丢历史数字。
        let after = serde_json::to_value(&stored(&store)).unwrap();
        assert_eq!(after["evidence"]["models"], previous["evidence"]["models"]);
        assert_eq!(after["evidence"]["catalog"]["observed_at"], previous["evidence"]["catalog"]["observed_at"]);
        // #32：同账号、同世代的目录读取失败只把资格标陈旧——已核实项与选择／停用都保留，
        // 绝不会因为一次网络失败被当成「上游已移除」。
        let catalog = crate::subscription_catalog::catalog(&store.read(), "codex").cloned().unwrap();
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].availability, crate::subscription_catalog::Availability::Stale);
        assert_eq!(
            crate::subscription_catalog::eligibility(&store.read(), "codex", "fixture-model"),
            crate::subscription_catalog::Eligibility::Stale
        );
    }

    #[tokio::test]
    async fn a_failed_catalog_read_without_a_previous_directory_fails_honestly() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        adapter.fail_models.store(true, Ordering::SeqCst);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        // 从来没有已核实目录：如实标失败且不编造模型；额度仍然独立读取。
        assert_eq!(evidence.catalog.state, EvidenceState::Failed);
        assert!(evidence.models.is_empty());
        assert!(evidence.catalog.observed_at.is_none());
        assert_eq!(evidence.quota.state, EvidenceState::Available);
        assert_eq!(evidence.quota.buckets.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_quota_read_keeps_the_history_buckets_and_observation_time() {
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
        adapter.fail_quota.store(true, Ordering::SeqCst);
        // 额度失败不改成错误弹窗：命令仍返回 Ok，失败由 state/history 表达。
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        assert!(evidence.quota.history, "retained numbers must be labelled history");
        // 历史数字与时间原样保留：既不伪装成可用，也不伪装成 0 余额或刚刚读取。
        assert_eq!(evidence.quota.observed_at.as_deref(), Some(STUB_QUOTA_OBSERVED_AT));
        assert_eq!(evidence.quota.source.as_deref(), Some("fixture:account/rateLimits/read"));
        assert_eq!(evidence.quota.view, QuotaView::RateLimitsByLimitId);
        assert_eq!(evidence.quota.buckets.len(), 1);
        assert_eq!(evidence.quota.buckets[0].windows[0].used_percent, Some(42.0));
        // 目录独立读取成功，不因额度失败而丢失。
        assert_eq!(evidence.catalog.state, EvidenceState::Available);
        assert_eq!(evidence.models.len(), 1);
        // 额度失败不是目录失败：目录资格保持本次权威读取的结果，不被误标为陈旧。
        assert_eq!(
            crate::subscription_catalog::eligibility(&store.read(), "codex", "fixture-model"),
            crate::subscription_catalog::Eligibility::Eligible
        );
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "quota_failed");
    }

    #[tokio::test]
    async fn a_first_quota_failure_without_history_never_claims_history() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        // 从未成功读过额度：没有可标为「最后成功更新」的数字，history 必须为 false。
        adapter.fail_quota.store(true, Ordering::SeqCst);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        assert!(!evidence.quota.history, "nothing was ever read; there is no history to label");
        assert!(evidence.quota.observed_at.is_none());
        assert!(evidence.quota.buckets.is_empty());
        // 目录仍独立读取：一次额度失败不会牵连目录证据。
        assert_eq!(evidence.catalog.state, EvidenceState::Available);
    }

    #[tokio::test]
    async fn a_late_result_from_another_generation_is_discarded() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        // 读取期间换号：迟到的只读结果不得写入新世代。
        adapter.switch_during_read.store(true, Ordering::SeqCst);
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("changed while refreshing"), "{error}");
        let connection = stored(&store);
        assert_eq!(connection.generation, 2);
        assert!(connection.evidence.is_none());
    }

    async fn invalid_refresh_cannot_clobber_newer_recovery(mode: &str) {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        {
            let mut status = adapter.status.lock().unwrap();
            status.state = ConnectionState::NotConnected;
            status.identity = None;
            status.identity_incomplete = mode == "incomplete";
        }
        adapter.fail_status.store(mode == "rpc-failed", Ordering::SeqCst);
        adapter.pause_status.store(true, Ordering::SeqCst);
        let old_store = store.clone();
        let old = tokio::spawn(async move { refresh(&old_store, "codex").await });
        adapter.status_started.notified().await;
        // Old result is fixed before the barrier; newer request writes fresh evidence for the same A.
        {
            let mut status = adapter.status.lock().unwrap();
            status.state = ConnectionState::Connected;
            status.identity = Some("fixture@example.invalid".into());
            status.identity_incomplete = false;
        }
        adapter.fail_status.store(false, Ordering::SeqCst);
        adapter.models.lock().unwrap()[0].model_id = "recovered-model".into();
        *adapter.catalog_time.lock().unwrap() = "2026-09-30T05:00:00Z".into();
        {
            let mut quota = adapter.quota.lock().unwrap();
            quota.observed_at = Some("2026-09-30T05:01:00Z".into());
            quota.buckets[0].windows[0].used_percent = Some(73.0);
        }
        let recovered = refresh(&store, "codex").await.unwrap();
        adapter.resume_status.notify_one();
        let old_result = old.await.unwrap();
        assert_eq!(stored(&store), recovered, "stale {mode} must preserve the newer connection and all evidence");
        assert!(old_result.is_err(), "a superseded request must be discarded");
        let evidence = recovered.current_evidence().unwrap();
        assert_eq!(evidence.models[0].model_id, "recovered-model");
        assert_eq!(evidence.catalog.observed_at.as_deref(), Some("2026-09-30T05:00:00Z"));
        assert_eq!(evidence.quota.observed_at.as_deref(), Some("2026-09-30T05:01:00Z"));
    }

    #[tokio::test]
    async fn stale_incomplete_refresh_cannot_clobber_newer_recovery() {
        invalid_refresh_cannot_clobber_newer_recovery("incomplete").await;
    }

    #[tokio::test]
    async fn stale_failed_rpc_refresh_cannot_clobber_newer_recovery() {
        invalid_refresh_cannot_clobber_newer_recovery("rpc-failed").await;
    }

    #[tokio::test]
    async fn stale_signed_out_refresh_cannot_clobber_newer_recovery() {
        invalid_refresh_cannot_clobber_newer_recovery("signed-out").await;
    }

    #[tokio::test]
    async fn stale_success_refresh_cannot_clobber_newer_success() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        adapter.pause_models.store(true, Ordering::SeqCst);
        let old_store = store.clone();
        let old = tokio::spawn(async move { refresh(&old_store, "codex").await });
        adapter.models_started.notified().await;
        adapter.models.lock().unwrap()[0].model_id = "new-model".into();
        *adapter.catalog_time.lock().unwrap() = "2026-09-30T05:00:00Z".into();
        let newer = refresh(&store, "codex").await.unwrap();
        adapter.resume_models.notify_one();
        assert!(old.await.unwrap().is_err());
        assert_eq!(stored(&store), newer);
    }

    #[tokio::test]
    async fn refresh_order_is_independent_for_different_providers() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        store.update(|config| {
            let mut second = config.providers.iter().find(|p| p.id == "codex").unwrap().clone();
            second.id = "codex-second".into();
            config.providers.push(second);
            sync_provider(config, "codex-second", &ProviderKind::CodexSubscription);
        }).unwrap();
        adapter.pause_status.store(true, Ordering::SeqCst);
        let first_store = store.clone();
        let first = tokio::spawn(async move { refresh(&first_store, "codex").await });
        adapter.status_started.notified().await;
        // The second provider finishes while the first read is paused, without superseding it.
        let second = refresh(&store, "codex-second").await.unwrap();
        adapter.resume_status.notify_one();
        assert!(first.await.unwrap().unwrap().current_evidence().is_some());
        assert_eq!(store.read().subscriptions["codex-second"], second);
    }

    #[tokio::test]
    async fn a_late_refresh_cannot_revive_a_after_helper_disconnect_or_switch_to_b() {
        for switch_to_b in [false, true] {
            let adapter = StubAdapter::new();
            let (store, _directory) = fixture_store(adapter.clone());
            refresh(&store, "codex").await.unwrap();
            adapter.pause_models.store(true, Ordering::SeqCst);
            let late_store = store.clone();
            let late = tokio::spawn(async move { refresh(&late_store, "codex").await });
            adapter.models_started.notified().await;
            {
                let mut status = adapter.status.lock().unwrap();
                status.state = ConnectionState::NotConnected;
                status.identity = None;
            }
            let signed_out = refresh(&store, "codex").await.unwrap();
            assert!(signed_out.evidence.is_none());
            if switch_to_b {
                {
                    let mut status = adapter.status.lock().unwrap();
                    status.state = ConnectionState::Connected;
                    status.identity = Some("B@example.invalid".into());
                }
                adapter.models.lock().unwrap().clear();
                adapter.fail_quota.store(true, Ordering::SeqCst);
                let b = refresh(&store, "codex").await.unwrap();
                assert_eq!(b.identity.as_deref(), Some("B@example.invalid"));
                assert!(b.evidence.as_ref().unwrap().models.is_empty());
                assert!(b.evidence.as_ref().unwrap().quota.buckets.is_empty());
                assert!(!b.evidence.as_ref().unwrap().quota.history);
            }
            let before_late = stored(&store);
            adapter.resume_models.notify_one();
            assert!(late.await.unwrap().unwrap_err().contains("changed while refreshing"));
            assert_eq!(stored(&store), before_late, "A must not overwrite the disconnected or B connection");
        }
    }

    #[tokio::test]
    async fn a_read_from_another_account_is_discarded_with_an_error() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let before = serde_json::to_value(stored(&store)).unwrap();
        // 连接已核实账号没变，但辅助进程这次的响应属于另一个账号：换号期间的旧响应不得污染新账号。
        adapter.status.lock().unwrap().identity = Some("other@example.invalid".into());
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("account changed while refreshing"), "{error}");
        assert_eq!(serde_json::to_value(stored(&store)).unwrap(), before);
    }

    /// 本次 status 读不到有效账号身份时无法确认数据归属：不得把已核实身份覆盖成 `None`，
    /// 历史目录与额度仍严格绑定原账号，但不再是 `current_evidence`，也不得把本次读到的
    /// 目录/额度挂到旧账号名下——本次目录与额度刷新必须根本不发起。
    #[tokio::test]
    async fn a_refresh_without_an_account_identity_keeps_the_verified_account_and_history() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let before = serde_json::to_value(stored(&store)).unwrap();
        let catalog_reads = adapter.catalog_reads.load(Ordering::SeqCst);
        let quota_reads = adapter.quota_reads.load(Ordering::SeqCst);
        // 真实适配器能产生的组合：未连接 + 身份读取不完整。
        {
            let mut status = adapter.status.lock().unwrap();
            status.identity = None;
            status.state = ConnectionState::NotConnected;
            status.identity_incomplete = true;
        }
        let error = refresh(&store, "codex").await.unwrap_err();
        assert!(error.contains("could not confirm which account"), "{error}");
        let after = serde_json::to_value(stored(&store)).unwrap();
        assert_eq!(after["evidence"]["models"], before["evidence"]["models"]);
        assert_eq!(after["evidence"]["quota"]["buckets"], before["evidence"]["quota"]["buckets"]);
        let connection = stored(&store);
        assert_eq!(connection.identity.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert!(connection.current_evidence().is_none());
        let evidence = connection.account_evidence().expect("history stays bound to the verified account");
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        assert!(evidence.quota.history);
        let (model, provider) = target(&store.read());
        assert_eq!(admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err().code, "not_connected");
        assert_eq!(evidence.catalog.observed_at.as_deref(), Some(STUB_CATALOG_OBSERVED_AT));
        assert_eq!(evidence.quota.observed_at.as_deref(), Some(STUB_QUOTA_OBSERVED_AT));
        // 本次目录与额度刷新被停止：新数据既没有被读取，也不可能被挂到旧账号名下。
        assert_eq!(adapter.catalog_reads.load(Ordering::SeqCst), catalog_reads, "the catalog read must not start");
        assert_eq!(adapter.quota_reads.load(Ordering::SeqCst), quota_reads, "the quota read must not start");
    }

    /// 上一条守卫的后续路径：身份重新确认是同一个账号，但目录与额度读取都失败时，
    /// 仍必须能保留此前的已核实目录（stale）与历史额度（failed + history）。
    #[tokio::test]
    async fn history_survives_an_unconfirmed_read_followed_by_failed_reads() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        {
            let mut status = adapter.status.lock().unwrap();
            status.identity = None;
            status.state = ConnectionState::NotConnected;
            status.identity_incomplete = true;
        }
        assert!(refresh(&store, "codex").await.unwrap_err().contains("could not confirm which account"));
        // 身份重新确认是同一个 A，但两项读取都失败。
        {
            let mut status = adapter.status.lock().unwrap();
            status.identity = Some("fixture@example.invalid".into());
            status.state = ConnectionState::Connected;
            status.identity_incomplete = false;
        }
        adapter.fail_models.store(true, Ordering::SeqCst);
        adapter.fail_quota.store(true, Ordering::SeqCst);
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().expect("A must still own the retained evidence");
        assert_eq!(connection.identity.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.models.len(), 1);
        assert_eq!(evidence.catalog.observed_at.as_deref(), Some(STUB_CATALOG_OBSERVED_AT));
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        assert!(evidence.quota.history, "the retained numbers must still be labelled history");
        assert_eq!(evidence.quota.observed_at.as_deref(), Some(STUB_QUOTA_OBSERVED_AT));
        assert_eq!(evidence.quota.buckets[0].windows[0].used_percent, Some(42.0));
    }

    /// 区分「本次身份读取不完整」与「明确退出」：退出已经递增世代并清空身份与证据，
    /// 之后的一次只读刷新不能被新守卫当成身份不明而报错，也不能复活旧账号。
    #[tokio::test]
    async fn a_refresh_after_sign_out_is_not_blocked_by_the_identity_guard() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
        let sessions = tokio::sync::Mutex::new(SessionState::default());
        logout(&store, "codex", &sessions).await.unwrap();
        // 辅助进程已退出登录：account/read 不再返回身份（真适配器此时同样报未连接）。
        {
            let mut status = adapter.status.lock().unwrap();
            status.identity = None;
            status.state = ConnectionState::NotConnected;
        }
        let connection = refresh(&store, "codex").await.unwrap();
        assert!(connection.identity.is_none(), "a signed-out connection must not revive the old account");
        assert!(connection.current_evidence().is_none(), "signed-out evidence must not become valid again");
        assert_ne!(connection.state, ConnectionState::Connected, "an unknown identity is never connected");
    }

    #[tokio::test]
    async fn a_denied_permission_is_recorded_and_denies_generation() {
        let adapter = StubAdapter::new();
        let (store, _directory) = fixture_store(adapter.clone());
        refresh(&store, "codex").await.unwrap();
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
        {
            let mut quota = adapter.quota.lock().unwrap();
            quota.buckets[0].permission = QuotaPermission::Denied;
            quota.state = EvidenceState::Denied;
        }
        let connection = refresh(&store, "codex").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        // 许可被明确拒绝：如实记 denied，不写成未知；历史位保持 false（这是本次读到的实数）。
        assert_eq!(evidence.quota.state, EvidenceState::Denied);
        assert_eq!(evidence.quota.buckets[0].permission, QuotaPermission::Denied);
        assert!(!evidence.quota.history);
        let (model, provider) = target(&store.read());
        let denial = admit_model(&store.read(), &model, &provider, Protocol::Chat).unwrap_err();
        assert_eq!(denial.code, "quota_denied");
        assert_eq!(denial.family, DenialFamily::Quota);
        // denied 是上游的明确拒绝：刷新不会恢复，恢复动作不能写成「刷新后再试」。
        assert!(denial.recovery.contains("Use an API provider"), "{}", denial.recovery);
        assert!(!denial.recovery.contains("Refresh the connection"), "{}", denial.recovery);
        let view = views(&store.read(), true, &SessionState::default()).pop().unwrap();
        assert_eq!(serde_json::to_value(&view).unwrap()["quota"]["state"], "denied");
    }

    #[test]
    fn legacy_evidence_without_the_new_fields_reads_losslessly() {
        let legacy = serde_json::json!({
            "generation": 3,
            "account": "legacy@example.invalid",
            "models": [{"model_id": "legacy-model", "name": null, "eligible": true}],
            "capabilities": [],
            "quota": {"state": "available", "source": "legacy", "observed_at": "2026-01-01T00:00:00Z"}
        });
        let evidence: Evidence = serde_json::from_value(legacy).unwrap();
        assert_eq!(evidence.quota.state, EvidenceState::Available);
        assert_eq!(evidence.quota.view, QuotaView::Unknown);
        assert!(evidence.quota.buckets.is_empty() && evidence.quota.missing_fields.is_empty());
        assert!(!evidence.quota.history);
        assert_eq!(evidence.catalog.state, EvidenceState::Unknown);
        assert!(evidence.catalog.source.is_none() && evidence.catalog.observed_at.is_none());
        // 往返无损：新字段都有确定默认值，旧配置不会被改写。
        let round_trip: Evidence = serde_json::from_value(serde_json::to_value(&evidence).unwrap()).unwrap();
        assert_eq!(round_trip, evidence);
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
                    identity_incomplete: false,
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
                    models: vec![DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
                    source: Some("fixture:model/list".into()),
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
        guard.helper = store.subscription.helper_status();
        guard.supported_providers = store
            .read()
            .providers
            .iter()
            .filter(|provider| is_subscription_provider(provider) && store.subscription.supports(&provider.kind))
            .map(|provider| provider.id.clone())
            .collect();
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

    #[tokio::test]
    async fn sign_out_and_rebinding_invalidate_qualification_but_keep_user_configuration() {
        let adapter = LifecycleStub::new();
        let (store, _directory, sessions) = fixture(adapter.clone()).await;
        // 先建立一次已核实的账号资格。
        store
            .update(|config| {
                let generation = {
                    let connection = config.subscriptions.get_mut("codex").unwrap();
                    connection.state = ConnectionState::Connected;
                    connection.identity = Some("old@example.invalid".into());
                    connection.generation
                };
                crate::subscription_catalog::reconcile(
                    config,
                    "codex",
                    "old@example.invalid",
                    generation,
                    &[DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
                    Some("2026-09-30T00:00:00Z"),
                );
            })
            .unwrap();
        // 用户在目录里取消选择并停用：退出与重核都不得改写这两个状态。
        store
            .update(|config| {
                let model = config.models.iter_mut().find(|m| m.provider_id == "codex").unwrap();
                model.selected = false;
                model.enabled = false;
                model.name = "Renamed".into();
            })
            .unwrap();
        assert_eq!(
            crate::subscription_catalog::eligibility(&store.read(), "codex", "fixture-model"),
            crate::subscription_catalog::Eligibility::Eligible
        );
        // 退出：账号相关资格整体作废，稳定标识与用户配置保留。
        logout(&store, "codex", &sessions).await.unwrap();
        let config = store.read();
        assert_eq!(
            crate::subscription_catalog::eligibility(&config, "codex", "fixture-model"),
            crate::subscription_catalog::Eligibility::AccountChanged
        );
        assert!(config.subscription_catalogs["codex"]
            .entries
            .iter()
            .all(|entry| entry.availability == crate::subscription_catalog::Availability::Unknown));
        let kept = config.models.iter().find(|m| m.provider_id == "codex").unwrap();
        assert!(!kept.selected && !kept.enabled && kept.name == "Renamed");
        drop(config);
        // 身份改绑：新账号的资格必须重新核对，旧账号的目录证据不得沿用。
        let generation = store.read().subscriptions["codex"].generation;
        store
            .update(|config| {
                crate::subscription_catalog::reconcile(
                    config,
                    "codex",
                    "new@example.invalid",
                    generation,
                    &[DiscoveredModel { model_id: "fixture-model".into(), name: None, eligible: true }],
                    Some("2026-09-30T00:00:00Z"),
                );
            })
            .unwrap();
        assert_eq!(
            crate::subscription_catalog::eligibility(&store.read(), "codex", "fixture-model"),
            crate::subscription_catalog::Eligibility::AccountChanged,
            "目录还绑定旧账号时不算当前账号的资格"
        );
        bind_identity(&store, "codex", generation, "new@example.invalid").unwrap();
        let config = store.read();
        assert_eq!(
            crate::subscription_catalog::eligibility(&config, "codex", "fixture-model"),
            crate::subscription_catalog::Eligibility::AccountChanged
        );
        assert!(config.subscription_catalogs["codex"]
            .entries
            .iter()
            .all(|entry| entry.account.is_none() && entry.availability == crate::subscription_catalog::Availability::Unknown));
        let kept = config.models.iter().find(|m| m.provider_id == "codex").unwrap();
        assert!(!kept.selected && !kept.enabled);
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
        // 目录投影字段名冻结：前端与 lib.rs 快照都按 `catalog_entries` 取；
        // 同名的 `catalog` 是 #15 的只读目录证据（对象），两者不能混用。
        assert!(json["catalog_entries"].is_array());
        assert!(json["catalog"]["state"].is_string());
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
