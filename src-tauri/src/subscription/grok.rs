//! Grok subscription boundary. Production account/catalog/quota reads and login remain fail-closed
//! until the installed CLI source and its ACP wire contract are mapped and reviewed. The older
//! newline-event adapter below is exercised only by unit-test fakes; it is not a production CLI
//! contract. Candidate ACP framing and the static UI gate live in [`readonly`].

// Production generation remains fail-closed until the real CLI contract is verified in #26.
#[allow(dead_code)]
mod generation;
#[cfg(test)]
pub(crate) mod billing;
// The candidate ACP client is intentionally disconnected from production until its source pin is verified.
#[allow(dead_code)]
pub(crate) mod readonly;

use std::{path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use chrono::DateTime;
use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::{
    config::ProviderKind,
    subscription::{
        helper::{self, OwnedProcesses, ReadCommand},
        quota_state, CatalogRead, ConnectionState, ConnectionStatus, DiscoveredModel, EvidenceState,
        GenerationRequest, GenerationStream, HelperStatus, LoginResult, LoginStart, LogoutOutcome,
        QuotaBucket, QuotaCredits, QuotaEvidence, QuotaPermission, QuotaView, QuotaWindow,
        SubscriptionAdapter,
    },
};

/// 一次只读读取的上限；真实 CLI 的响应时间未经验证。
const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// 本契约的额度只有一个订阅池桶（视图 JSON 冻结了 `limit_id = subscription_pool`）。
const SUBSCRIPTION_POOL: &str = "subscription_pool";
/// `pool.period.type` 缺失或不可用时的窗口占位（契约 §3）。
const DEFAULT_WINDOW_LABEL: &str = "subscription";
const CODE_HELPER_ISOLATED: &str = "helper_isolated";
const CODE_HELPER_MISSING: &str = "helper_missing";
const CODE_HELPER_TIMEOUT: &str = "helper_timeout";
const CODE_HELPER_EXITED: &str = "helper_exited";
const CODE_GROK_INTERFACE_UNVERIFIED: &str = "grok_interface_unverified";

#[cfg(test)]
#[derive(Clone)]
pub(super) struct PendingClientToolCall {
    pub call: crate::codex_helper::ClientToolCall,
    pub arguments: Value,
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct PendingClientToolTurn {
    pub protocol: crate::protocol::Protocol,
    pub model_id: String,
    pub tools: Vec<Value>,
    pub allow_parallel: bool,
    pub user_text: Vec<String>,
    pub assistant_history: Vec<String>,
    pub calls: Vec<PendingClientToolCall>,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CompletedClientToolExchange {
    pub name: String,
    pub arguments: Value,
    pub output: String,
    pub is_error: bool,
}

/// 稳定的失败说明：错误必须脱敏后才进入日志或界面。
fn refusal(code: &str, message: impl std::fmt::Display) -> String {
    helper::redact(&format!("{code}: {message}"))
}

/// 本次只读读取实际使用的程序，以及它是否是隔离验收的 pinned 替身。
///
/// - 隔离环境只认 [`crate::runtime::grok_helper_override`]（显式 `--autojev-grok-helper`）；
///   没有 pinned 替身就如实拒绝，绝不拉起任何进程。
/// - 生产只认登录路径解析到的官方 CLI（`AUTOJEV_GROK_HELPER` / `PATH` 上的 `grok`）；
///   隔离开关在非隔离构建里恒为 `None`，所以生产行为不受它影响。
///
/// 抽成纯函数便于直接单测这条守卫；返回 `Err` 即必须如实失败。
fn read_program(
    isolated: bool,
    pinned: Option<&std::path::Path>,
    program: Option<&std::path::Path>,
) -> Result<(PathBuf, bool), String> {
    if isolated {
        return pinned
            .map(|path| (path.to_path_buf(), true))
            .ok_or_else(|| refusal(CODE_HELPER_ISOLATED, "Grok read-only reads are unavailable in isolated validation"));
    }
    program
        .map(|path| (path.to_path_buf(), false))
        .ok_or_else(|| refusal(CODE_HELPER_MISSING, "No Grok CLI helper was found on PATH"))
}

/// 一行 stdout 事件。无法解析或未知 `event` 的行在 [`parse_event`] 处直接忽略（不是失败）。
enum ReadEvent {
    Account(Value),
    Catalog(Value),
    Quota(Value),
    /// 上游自述「固定版本没有这个机器接口」；`interface` 决定它属于哪次读取。
    Unsupported { interface: Option<String> },
    Error { code: String, message: String },
    Done,
}

/// 一次读取的终态：成功载荷、`unsupported`，或由 [`terminal_of`] 如实报错。
enum Terminal {
    Payload(ReadEvent),
    Unsupported,
}

fn parse_event(line: &str) -> Option<ReadEvent> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    match value.get("event")?.as_str()? {
        "account" => Some(ReadEvent::Account(value)),
        "catalog" => Some(ReadEvent::Catalog(value)),
        "quota" => Some(ReadEvent::Quota(value)),
        "unsupported" => Some(ReadEvent::Unsupported {
            interface: value.get("interface").and_then(Value::as_str).map(str::to_owned),
        }),
        "error" => Some(ReadEvent::Error {
            code: value.get("code").and_then(Value::as_str).unwrap_or("helper_error").to_owned(),
            message: value.get("message").and_then(Value::as_str).unwrap_or_default().to_owned(),
        }),
        "done" => Some(ReadEvent::Done),
        _ => None,
    }
}

/// 取一次读取的终态：`error` 一律如实失败（fail-closed）；`unsupported` 只在
/// `interface` 与本次读取一致时才算终态（别的接口的 unsupported 直接忽略，既不是失败也不改变结果）；
/// 没有任何终态事件同样是一次如实失败——绝不伪造成成功或空结果。
fn terminal_of(events: Vec<ReadEvent>, expected_interface: &str) -> Result<Terminal> {
    let mut payload = None;
    let mut unsupported = false;
    for event in events {
        match event {
            ReadEvent::Done => break,
            ReadEvent::Error { code, message } => bail!("{}", refusal(&code, &message)),
            ReadEvent::Unsupported { interface } if interface.as_deref() == Some(expected_interface) => {
                unsupported = true
            }
            ReadEvent::Unsupported { .. } => {}
            other => payload = Some(other),
        }
    }
    if unsupported {
        return Ok(Terminal::Unsupported);
    }
    match payload {
        Some(event) => Ok(Terminal::Payload(event)),
        None => bail!("{}", refusal(CODE_HELPER_EXITED, "the helper ended without a terminal event")),
    }
}

/// Grok 的只读适配器。程序、专用 home 与环境白名单都沿用登录路径（[`helper`]），
/// 但它只读、只承载 [`ProviderKind::GrokSubscription`]，绝不借用别家的进程或身份。
pub struct GrokSubscriptionAdapter {
    home: PathBuf,
    program: Option<PathBuf>,
    extra_args: Vec<String>,
    processes: OwnedProcesses,
    /// 同一服务商的只读读取串行化：并发刷新不得互相插队，也不得复用一个进程。
    reads: tokio::sync::Mutex<()>,
    /// 一次读取的上限；生产固定 15s，测试可注入更短的值。
    read_timeout: Duration,
    /// 每家账号串行处理生成；每次请求另建 ACP 会话。
    #[cfg(test)]
    generation_locks: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    #[cfg(test)]
    generation_timeout: Duration,
    /// 账号世代改变时取消该世代的子进程。
    active_generations: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(String, u64), std::collections::HashMap<String, tokio::sync::watch::Sender<bool>>>>>,
    #[cfg(test)]
    pending_tool_turns: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(String, u64, String), PendingClientToolTurn>>>,
    #[cfg(test)]
    completed_tool_exchanges: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(String, u64, String), CompletedClientToolExchange>>>,
    #[cfg(test)]
    test_generation: bool,
    /// Test-only machine-read fixture protocol; production builds never set this.
    #[cfg(test)]
    test_read_protocol: bool,
}

/// Validate the strict ACP text subset before creating an HTTP response.
pub(crate) fn validate_generation_request(
    protocol: crate::protocol::Protocol,
    body: &Value,
    model_id: &str,
    generation: u64,
) -> Result<()> {
    generation::validate_generation_request(protocol, body, model_id, generation)
}

impl GrokSubscriptionAdapter {
    pub fn new() -> Self {
        let home = crate::runtime::home_dir().unwrap_or_default();
        let (program, extra_args) = match helper::resolve_program() {
            Some((program, extra_args)) => (Some(program), extra_args),
            None => (None, Vec::new()),
        };
        Self::from_parts(home, program, extra_args, READ_TIMEOUT)
    }

    fn from_parts(home: PathBuf, program: Option<PathBuf>, extra_args: Vec<String>, read_timeout: Duration) -> Self {
        Self {
            home,
            program,
            extra_args,
            processes: OwnedProcesses::new(),
            reads: tokio::sync::Mutex::new(()),
            read_timeout,
            #[cfg(test)]
            generation_locks: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(test)]
            generation_timeout: Duration::from_secs(120),
            active_generations: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            #[cfg(test)]
            pending_tool_turns: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            #[cfg(test)]
            completed_tool_exchanges: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            #[cfg(test)]
            test_generation: false,
            #[cfg(test)]
            test_read_protocol: false,
        }
    }

    /// 测试专用：注入隔离 home 与本地假 helper，绝不触碰真实用户目录或真实 CLI。
    #[cfg(test)]
    pub(crate) fn with_test_helper(home: PathBuf, program: PathBuf) -> Self {
        let mut adapter = Self::from_parts(home, Some(program), Vec::new(), READ_TIMEOUT);
        adapter.test_generation = true;
        adapter.test_read_protocol = true;
        adapter
    }

    /// The JSON-line account/catalog/quota protocol is only a test fixture contract.
    /// The release app has no verified machine-readable interfaces for these values.
    fn read_protocol_supported(&self) -> bool {
        if crate::runtime::isolated() {
            #[cfg(all(not(test), feature = "isolation-check"))]
            {
                return crate::runtime::grok_helper_override().is_some();
            }
            #[cfg(any(test, not(feature = "isolation-check")))]
            {
                return false;
            }
        }
        #[cfg(test)]
        {
            self.test_read_protocol
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    /// 测试专用：缩短读取上限，好让超时路径也能在单测里被覆盖。
    #[cfg(test)]
    fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }

    fn provider_home(&self, provider_id: &str) -> Result<PathBuf, String> {
        helper::helper_home(&ProviderKind::GrokSubscription, provider_id, &self.home)
            .map_err(|error| helper::redact(&error.to_string()))
    }

    fn spec(&self, provider_id: &str, program: &PathBuf, command: ReadCommand) -> Result<helper::HelperSpec, String> {
        helper::read_spec_with_program(
            &ProviderKind::GrokSubscription,
            provider_id,
            &self.home,
            program,
            &self.extra_args,
            command,
        )
        .map_err(|error| helper::redact(&error.to_string()))
    }

    /// 一次读取 = 一个一次性进程：spawn → 逐行读 stdout → 收到终态事件或 EOF → 只回收该 pid。
    /// 超时、提前退出、无终态事件都是如实失败，绝不伪造成成功或空结果。
    async fn run_read(&self, provider_id: &str, command: ReadCommand) -> Result<Vec<ReadEvent>> {
        // 同一服务商的只读读取串行化。
        let _serialized = self.reads.lock().await;
        // 隔离验收可以显式传入 pinned 只读替身（--autojev-grok-helper）；没有就如实拒绝。
        let isolated = crate::runtime::isolated();
        let pinned = if isolated { crate::runtime::grok_helper_override() } else { None };
        let (program, pinned) = read_program(isolated, pinned, self.program.as_deref()).map_err(anyhow::Error::msg)?;
        let spec = self.spec(provider_id, &program, command).map_err(anyhow::Error::msg)?;
        if let Err(error) = helper::prepare_home(&spec.home) {
            bail!("{}", refusal(CODE_HELPER_MISSING, error));
        }
        // 只拉起自己这一个进程：不复用登录会话的进程，也不按服务商回收别人的 pid。
        let spawned = if pinned {
            self.processes.spawn_pinned(provider_id, &spec)
        } else {
            self.processes.spawn(provider_id, &spec)
        };
        let pid = spawned.map_err(|error| anyhow::Error::msg(helper::redact(&error.to_string())))?;
        let Some(stdout) = self.processes.take_stdout(pid) else {
            self.processes.reclaim_pid(pid);
            bail!("{}", refusal(CODE_HELPER_MISSING, "the helper produced no output stream"));
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let deadline = tokio::time::Instant::now() + self.read_timeout;
        let mut events = Vec::new();
        let mut timed_out = false;
        loop {
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Some(line)) => {
                    let Some(event) = parse_event(&line) else { continue };
                    let done = matches!(event, ReadEvent::Done);
                    events.push(event);
                    if done {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    timed_out = true;
                    break;
                }
            }
        }
        // 收尾只回收自己那个 pid：绝不按名字批量杀进程，也不连坐同一服务商的其它读取。
        self.processes.reclaim_pid(pid);
        if timed_out {
            bail!("{}", refusal(CODE_HELPER_TIMEOUT, "the helper did not finish the read in time"));
        }
        if events.is_empty() {
            bail!("{}", refusal(CODE_HELPER_EXITED, "the helper exited without reporting anything"));
        }
        Ok(events)
    }
}

impl Default for GrokSubscriptionAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ---- 事件 → 证据映射：只做表示换算，缺字段/类型不符一律如实记账，绝不截断、补齐或改写 ----

/// 越界或类型不符时要记的是**原始文本**：用户要看到上游到底给了什么，不是我们改写成什么。
fn raw_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 取一个字符串字段：字符串原样采用；缺失/null 记入 `missing`；类型不符把原始文本记入 `invalid`。
fn text_field(
    object: Option<&Value>,
    key: &str,
    path: &str,
    missing: &mut Vec<String>,
    invalid: &mut Vec<String>,
) -> Option<String> {
    match object.and_then(|object| object.get(key)) {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Null) | None => {
            missing.push(path.to_owned());
            None
        }
        Some(other) => {
            invalid.push(raw_text(other));
            None
        }
    }
}

/// 取一个布尔字段：只有真的布尔才采用；缺失/null 记 `missing`，类型不符把原始文本记 `invalid`。
fn bool_field(
    object: Option<&Value>,
    key: &str,
    path: &str,
    missing: &mut Vec<String>,
    invalid: &mut Vec<String>,
) -> Option<bool> {
    match object.and_then(|object| object.get(key)) {
        Some(Value::Bool(flag)) => Some(*flag),
        Some(Value::Null) | None => {
            missing.push(path.to_owned());
            None
        }
        Some(other) => {
            invalid.push(raw_text(other));
            None
        }
    }
}

/// 许可轴：`true`=allowed、`false`=denied，缺失/null/非布尔一律 unknown（不推导、不补齐）。
fn permission_field(
    object: Option<&Value>,
    key: &str,
    path: &str,
    missing: &mut Vec<String>,
    invalid: &mut Vec<String>,
) -> QuotaPermission {
    match object.and_then(|object| object.get(key)) {
        Some(Value::Bool(true)) => QuotaPermission::Allowed,
        Some(Value::Bool(false)) => QuotaPermission::Denied,
        Some(Value::Null) | None => {
            missing.push(path.to_owned());
            QuotaPermission::Unknown
        }
        Some(other) => {
            invalid.push(raw_text(other));
            QuotaPermission::Unknown
        }
    }
}

/// 取一个对象字段：非对象（且非 null）是类型不符，记原始文本；缺失/null 交由字段级缺失记账。
fn object_field<'a>(value: &'a Value, key: &str, invalid: &mut Vec<String>) -> Option<&'a Value> {
    match value.get(key) {
        Some(entry) if entry.is_object() => Some(entry),
        Some(Value::Null) | None => None,
        Some(other) => {
            invalid.push(raw_text(other));
            None
        }
    }
}

/// RFC3339 → Unix 秒：只做表示换算，不推算重置时间，也不在解析失败时编造时间。
fn rfc3339_seconds(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp())
}

/// `catalog` 事件 → [`CatalogRead`]。`id` 缺失或为空白的条目直接跳过并记 `models[i].id`，
/// 绝不编造标识；`display_name` 缺失即 `None`；发现的模型一律 `eligible = false`。
fn catalog_read(value: &Value) -> CatalogRead {
    let mut missing_fields = Vec::new();
    let mut models = Vec::new();
    match value.get("models") {
        Some(Value::Array(entries)) => {
            for (index, entry) in entries.iter().enumerate() {
                let id = entry.get("id").and_then(Value::as_str).map(str::trim).filter(|id| !id.is_empty());
                match id {
                    Some(id) => models.push(DiscoveredModel {
                        model_id: id.to_owned(),
                        name: entry.get("display_name").and_then(Value::as_str).map(str::to_owned),
                        eligible: false,
                    }),
                    None => missing_fields.push(format!("models[{index}].id")),
                }
            }
        }
        // `models` 缺失或类型不符：如实记缺失字段并返回空列表，不伪造任何条目。
        _ => missing_fields.push("models".to_owned()),
    }
    CatalogRead {
        state: EvidenceState::Available,
        models,
        source: value.get("source").and_then(Value::as_str).map(str::to_owned),
        observed_at: value.get("observed_at").and_then(Value::as_str).map(str::to_owned),
        missing_fields,
    }
}

/// `quota` 事件 → [`QuotaEvidence`]：单桶（`subscription_pool`）+ 单窗口。
/// 订阅内许可（`pool.usage_allowed`）与 credits 许可（`extra_usage.permitted`）是两个计量轴，
/// 互不推导；前者参与额度状态判定，后者只在 credits 上如实展示。credits 的余额与单位原样保留。
fn quota_evidence(value: &Value) -> QuotaEvidence {
    let source = value.get("source").and_then(Value::as_str).map(str::to_owned);
    let observed_at = value.get("observed_at").and_then(Value::as_str).map(str::to_owned);

    let mut bucket = QuotaBucket::new(SUBSCRIPTION_POOL);
    let plan = object_field(value, "plan", &mut bucket.invalid_fields);
    let pool = object_field(value, "pool", &mut bucket.invalid_fields);
    let extra = object_field(value, "extra_usage", &mut bucket.invalid_fields);

    // plan：只把上游给的字符串原样带上，不解释成套餐语义。
    bucket.plan_type = text_field(plan, "code", "plan.code", &mut bucket.missing_fields, &mut bucket.invalid_fields);
    bucket.name = text_field(plan, "name", "plan.name", &mut bucket.missing_fields, &mut bucket.invalid_fields);

    let mut window = QuotaWindow::new(DEFAULT_WINDOW_LABEL);
    let period = pool.and_then(|pool| object_field(pool, "period", &mut window.invalid_fields));
    // 窗口 label 用 pool.period.type；缺失即占位 subscription。
    if let Some(label) = text_field(period, "type", "pool.period.type", &mut window.missing_fields, &mut window.invalid_fields) {
        if !label.trim().is_empty() {
            window.label = label;
        }
    }
    match pool.and_then(|pool| pool.get("used_percent")) {
        Some(Value::Number(number)) => match number.as_f64().filter(|used| used.is_finite() && (0.0..=100.0).contains(used)) {
            Some(used) => window.used_percent = Some(used),
            // 越界（例如 142）不截断成 100，也不回落成 0：保持 None 并记原始文本。
            None => window.invalid_fields.push(raw_text(&Value::Number(number.clone()))),
        },
        Some(Value::Null) | None => window.missing_fields.push("pool.used_percent".to_owned()),
        Some(other) => window.invalid_fields.push(raw_text(other)),
    }
    match period.and_then(|period| period.get("end")) {
        Some(Value::String(text)) => match rfc3339_seconds(text) {
            Some(seconds) => window.resets_at = Some(seconds),
            None => window.invalid_fields.push(text.clone()),
        },
        Some(Value::Null) | None => window.missing_fields.push("pool.period.end".to_owned()),
        Some(other) => window.invalid_fields.push(raw_text(other)),
    }
    bucket.windows.push(window);

    // 订阅内许可：只由 pool.usage_allowed 决定，credits.permission 不参与状态判定。
    bucket.permission =
        permission_field(pool, "usage_allowed", "pool.usage_allowed", &mut bucket.missing_fields, &mut bucket.invalid_fields);

    // 额外 credits 是独立的一条计量轴：即使本次事件没有 `extra_usage`，也必须如实给出一个
    // 「未知」的 credits 并记下缺失字段，界面不能因为缺这一层字段就整段消失。
    let mut credits = QuotaCredits::default();
    match extra {
        Some(extra) => {
            credits.has_credits = bool_field(
                Some(extra),
                "has_credits",
                "extra_usage.has_credits",
                &mut credits.missing_fields,
                &mut credits.invalid_fields,
            );
            credits.unlimited = bool_field(
                Some(extra),
                "unlimited",
                "extra_usage.unlimited",
                &mut credits.missing_fields,
                &mut credits.invalid_fields,
            );
            credits.balance = text_field(
                Some(extra),
                "balance",
                "extra_usage.balance",
                &mut credits.missing_fields,
                &mut credits.invalid_fields,
            );
            credits.unit =
                text_field(Some(extra), "unit", "extra_usage.unit", &mut credits.missing_fields, &mut credits.invalid_fields);
            credits.permission = permission_field(
                Some(extra),
                "permitted",
                "extra_usage.permitted",
                &mut credits.missing_fields,
                &mut credits.invalid_fields,
            );
        }
        // `extra_usage` 缺失、null 或不是对象：取值一律未知，并把这一层缺失如实记账。
        None => credits.missing_fields.push("extra_usage".to_owned()),
    }
    bucket.credits = Some(credits);

    let buckets = vec![bucket];
    QuotaEvidence {
        state: quota_state(&buckets),
        source,
        observed_at,
        view: QuotaView::GrokCliUsage,
        buckets,
        missing_fields: Vec::new(),
        history: false,
    }
}

impl SubscriptionAdapter for GrokSubscriptionAdapter {
    fn available(&self) -> bool {
        // 与只读读取走同一套程序选择：隔离环境下只有显式 pinned 的只读替身才算可用，
        // 生产环境则要求解析到了官方 CLI。登录路径不受这条影响。
        let isolated = crate::runtime::isolated();
        let pinned = if isolated { crate::runtime::grok_helper_override() } else { None };
        read_program(isolated, pinned, self.program.as_deref()).is_ok()
    }

    /// 只承载 Grok 订阅；Codex 等其它订阅服务商不得借用它的进程或身份。
    fn supports(&self, kind: &ProviderKind) -> bool {
        matches!(kind, ProviderKind::GrokSubscription)
    }

    fn helper_status(&self) -> HelperStatus {
        HelperStatus {
            available: self.available(),
            // 探测不拉起进程，拿不到版本就必须保持未知。
            user_agent: None,
            version: None,
            // 专用目录按服务商计算，而本方法没有 provider_id；真实路径由 status() 的 account_path 如实给出。
            auth_home: None,
        }
    }

    /// `account` 读取：辅助进程在自己的应用自有 home 里是否已有账号、是哪个账号。
    /// 辅助进程缺失、`unsupported`、`error`、超时或隔离环境一律 `Err`（refresh 因此不改写任何字段）；
    /// `identity` 非空 → connected，`null`/缺失 → not_connected（这是事实，不是失败）。
    fn status<'a>(&'a self, provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
        Box::pin(async move {
            if !self.read_protocol_supported() {
                let (code, reason) = if crate::runtime::isolated() {
                    (CODE_HELPER_ISOLATED, "Grok account status is unavailable in isolated validation")
                } else {
                    (CODE_GROK_INTERFACE_UNVERIFIED, "No verified machine-readable Grok account identity interface is available; identity remains unknown")
                };
                bail!("{}", refusal(code, reason));
            }
            let events = self.run_read(provider_id, ReadCommand::Account).await?;
            let payload = match terminal_of(events, ReadCommand::Account.interface())? {
                Terminal::Payload(ReadEvent::Account(value)) => value,
                Terminal::Unsupported => {
                    bail!("{}", refusal(CODE_HELPER_MISSING, "the helper exposes no machine-readable account state"))
                }
                Terminal::Payload(_) => {
                    bail!("{}", refusal(CODE_HELPER_EXITED, "the helper did not report the account state"))
                }
            };
            let identity = payload
                .get("identity")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|identity| !identity.is_empty())
                .map(str::to_owned);
            Ok(ConnectionStatus {
                state: if identity.is_some() { ConnectionState::Connected } else { ConnectionState::NotConnected },
                // Grok 只读读不到账号事实时一律 Err（由调用方按「身份不完整」处理），
                // 因此这里不会给出「读到但不完整」的状态。
                identity_incomplete: false,
                identity,
                helper_version: None,
                account_path: Some(self.provider_home(provider_id).map_err(anyhow::Error::msg)?.display().to_string()),
            })
        })
    }

    fn models<'a>(&'a self, provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<CatalogRead>> {
        Box::pin(async move {
            if !self.read_protocol_supported() {
                return Ok(CatalogRead {
                    state: EvidenceState::Unknown,
                    missing_fields: vec!["machine_readable_catalog_interface_unverified".into()],
                    ..CatalogRead::default()
                });
            }
            let events = self.run_read(provider_id, ReadCommand::Models).await?;
            match terminal_of(events, ReadCommand::Models.interface())? {
                // 固定版本没有该机器接口：如实报 unsupported，不是失败，也绝不返回空目录冒充成功。
                Terminal::Unsupported => Ok(CatalogRead { state: EvidenceState::Unsupported, ..CatalogRead::default() }),
                Terminal::Payload(ReadEvent::Catalog(value)) => Ok(catalog_read(&value)),
                Terminal::Payload(_) => bail!("{}", refusal(CODE_HELPER_EXITED, "the helper did not return a catalog event")),
            }
        })
    }

    fn quota<'a>(&'a self, provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
        Box::pin(async move {
            if !self.read_protocol_supported() {
                return Ok(QuotaEvidence {
                    state: EvidenceState::Unknown,
                    view: QuotaView::Unknown,
                    missing_fields: vec!["machine_readable_subscription_quota_interface_unverified".into()],
                    ..QuotaEvidence::default()
                });
            }
            let events = self.run_read(provider_id, ReadCommand::Usage).await?;
            match terminal_of(events, ReadCommand::Usage.interface())? {
                Terminal::Unsupported => Ok(QuotaEvidence { state: EvidenceState::Unsupported, ..QuotaEvidence::default() }),
                Terminal::Payload(ReadEvent::Quota(value)) => Ok(quota_evidence(&value)),
                Terminal::Payload(_) => bail!("{}", refusal(CODE_HELPER_EXITED, "the helper did not return a quota event")),
            }
        })
    }

    /// 未经 #26 人工核验前，生产版本明确 fail-closed。单测仅能经 `with_test_helper` 注入本地 ACP 替身。
    fn generate<'a>(&'a self, request: GenerationRequest<'a>) -> BoxFuture<'a, Result<GenerationStream<'a>>> {
        #[cfg(not(test))]
        {
            let _ = request;
            Box::pin(async { bail!("Grok generation is disabled until its production entry is verified") })
        }
        #[cfg(test)]
        {
            Box::pin(async move {
                if !self.test_generation {
                    bail!("Grok generation is unavailable without an injected local test helper");
                }
                generation::generate(self, request).await
            })
        }
    }

    fn cancel_generation(&self, provider_id: &str, generation: u64) {
        #[cfg(test)]
        self.pending_tool_turns.lock().unwrap().retain(|(provider, current, _), _| {
            provider != provider_id || *current != generation
        });
        #[cfg(test)]
        self.completed_tool_exchanges.lock().unwrap().retain(|(provider, current, _), _| {
            provider != provider_id || *current != generation
        });
        if let Some(senders) = self.active_generations.lock().unwrap().get(&(provider_id.to_owned(), generation)) {
            for sender in senders.values() {
                let _ = sender.send(true);
            }
        }
    }

    fn abandon_client_tool_calls(&self, provider_id: &str, generation: u64, call_ids: &[String]) {
        #[cfg(test)]
        {
            let mut pending = self.pending_tool_turns.lock().unwrap();
            let matches_batch = pending.iter().any(|((provider, current, _), turn)| {
                if provider != provider_id || *current != generation {
                    return false;
                }
                let expected: std::collections::HashSet<_> =
                    turn.calls.iter().map(|call| call.call.id.as_str()).collect();
                let supplied: std::collections::HashSet<_> = call_ids.iter().map(String::as_str).collect();
                expected == supplied
                    && turn.calls.iter().all(|call| {
                        pending.contains_key(&(provider_id.to_owned(), generation, call.call.id.clone()))
                    })
            });
            if matches_batch {
                for call_id in call_ids {
                    pending.remove(&(provider_id.to_owned(), generation, call_id.clone()));
                }
            }
        }
        #[cfg(not(test))]
        let _ = (provider_id, generation, call_ids);
    }

    fn start_login<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LoginStart>> {
        Box::pin(async { bail!("Grok subscription sign-in is not implemented in this adapter") })
    }

    fn login_result<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Option<LoginResult>>> {
        Box::pin(async { bail!("Grok subscription sign-in is not implemented in this adapter") })
    }

    fn cancel_login<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { bail!("Grok subscription sign-in is not implemented in this adapter") })
    }

    fn logout<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LogoutOutcome>> {
        Box::pin(async { bail!("Grok subscription sign-out is not implemented in this adapter") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigStore, Provider};
    use crate::subscription::{refresh, sync_provider, SubscriptionAdapters};
    use std::sync::Arc;

    /// 本地假 helper：不联网、不装 CLI、不碰真实凭据。
    fn fake_helper(directory: &tempfile::TempDir, body: &str) -> PathBuf {
        let script = directory.path().join("fake-grok.sh");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    fn fake_adapter(directory: &tempfile::TempDir, body: &str) -> GrokSubscriptionAdapter {
        GrokSubscriptionAdapter::with_test_helper(directory.path().to_path_buf(), fake_helper(directory, body))
    }

    /// 完整的三条只读接口：目录两条模型、额度一个订阅池桶 + credits。
    const GOOD_HELPER: &str = r#"
mode=$(cat "$GROK_HOME/mode" 2>/dev/null || echo ok)
identity=$(cat "$GROK_HOME/identity" 2>/dev/null || echo grok@example.invalid)
if [ "$mode" = "fail" ] && [ "$1" != "account" ]; then
  printf '%s\n' '{"event":"error","code":"upstream_unavailable","message":"fixture failure"}'
  printf '%s\n' '{"event":"done"}'
  exit 1
fi
if [ "$mode" = "unsupported" ] && [ "$1" != "account" ]; then
  printf '%s\n' '{"event":"unsupported","interface":"catalog"}'
  printf '%s\n' '{"event":"done"}'
  exit 0
fi
case "$1" in
  account)
    printf '%s\n' "{\"event\":\"account\",\"identity\":\"$identity\",\"source\":\"grok-cli:account\",\"observed_at\":\"2026-09-30T00:00:00Z\"}"
    printf '%s\n' '{"event":"done"}'
    ;;
  models)
    printf '%s\n' '{"event":"catalog","source":"grok-cli:models","observed_at":"2026-09-30T00:00:00Z","models":[{"id":"grok-4","display_name":"Grok 4"},{"id":"grok-3","display_name":null}]}'
    printf '%s\n' '{"event":"done"}'
    ;;
  usage)
    printf '%s\n' '{"event":"quota","source":"grok-cli:usage","observed_at":"2026-09-30T01:02:03Z","plan":{"code":"supergrok","name":"SuperGrok"},"pool":{"period":{"type":"weekly","end":"2026-10-07T00:00:00Z"},"used_percent":42.5,"usage_allowed":true},"extra_usage":{"has_credits":true,"unlimited":false,"balance":"12.50","unit":"USD","permitted":false}}'
    printf '%s\n' '{"event":"done"}'
    ;;
esac
"#;

    fn grok_store(adapter: Arc<GrokSubscriptionAdapter>) -> (Arc<ConfigStore>, tempfile::TempDir) {
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
                config.port = 0;
                let provider = Provider {
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
                config.providers.push(provider.clone());
                sync_provider(config, &provider.id, &provider.kind);
            })
            .unwrap();
        (store, directory)
    }

    /// 假 helper 的专用 home：与适配器自己算出来的路径一致。
    fn provider_home(directory: &tempfile::TempDir) -> PathBuf {
        helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap()
    }

    fn write_helper_note(directory: &tempfile::TempDir, name: &str, value: &str) {
        let home = provider_home(directory);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(name), value).unwrap();
    }

    #[tokio::test]
    async fn production_read_path_keeps_grok_identity_catalog_and_quota_unknown_without_spawning_cli() {
        let directory = tempfile::tempdir().unwrap();
        let script = fake_helper(
            &directory,
            "printf '%s\\n' \"$*\" >> \"$GROK_HOME/invocations\"\\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let adapter = GrokSubscriptionAdapter::from_parts(directory.path().to_path_buf(), Some(script), Vec::new(), READ_TIMEOUT);

        let status = adapter.status("grok", 1).await.unwrap_err().to_string();
        assert!(status.contains("grok_interface_unverified"), "{status}");
        let catalog = adapter.models("grok", 1).await.unwrap();
        assert_eq!(catalog.state, EvidenceState::Unknown);
        assert!(catalog.models.is_empty());
        assert!(catalog.source.is_none() && catalog.observed_at.is_none());
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.state, EvidenceState::Unknown);
        assert_eq!(quota.view, QuotaView::Unknown);
        assert!(quota.buckets.is_empty());
        assert!(quota.source.is_none() && quota.observed_at.is_none());

        let home = provider_home(&directory);
        assert!(!home.join("invocations").exists(), "unsupported CLI commands must never be spawned");
        assert!(!home.exists(), "an unsupported read must not create an auth home");
    }

    /// `pool.period.end` 不是 RFC3339：`resets_at` 保持 None，原始文本进 invalid_fields（不推算重置时间）。
    #[tokio::test]
    async fn grok_never_invents_a_reset_time_from_unparsable_text() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"period\":{\"type\":\"weekly\",\"end\":\"next tuesday\"},\"used_percent\":10,\"usage_allowed\":true}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        let window = &quota.buckets[0].windows[0];
        assert_eq!(window.resets_at, None);
        assert_eq!(window.invalid_fields, vec!["next tuesday"]);
        assert!(window.missing_fields.is_empty());
        // 无效的重置时间不影响其它字段：订阅内许可与百分比照常采用。
        assert_eq!(window.used_percent, Some(10.0));
        assert_eq!(quota.state, EvidenceState::Available);
    }

    /// 同一次读取里先出现成功载荷、再出现本接口的 `unsupported`：`unsupported` 优先，结果为 unsupported。
    #[tokio::test]
    async fn an_unsupported_trailer_wins_over_an_earlier_payload() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"catalog\",\"source\":\"grok-cli:models\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"models\":[{\"id\":\"grok-4\"}]}'\nprintf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"catalog\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let catalog = adapter.models("grok", 1).await.unwrap();
        assert_eq!(catalog.state, EvidenceState::Unsupported);
        assert!(catalog.models.is_empty());
    }

    #[tokio::test]
    async fn grok_reads_the_catalog_and_quota_through_the_real_spawn_path() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(&directory, GOOD_HELPER);
        assert!(adapter.available());

        let status = adapter.status("grok", 1).await.unwrap();
        assert_eq!(status.state, ConnectionState::Connected);
        assert_eq!(status.identity.as_deref(), Some("grok@example.invalid"));
        // 探测不拉起进程读版本：拿不到就保持未知。
        assert!(status.helper_version.is_none());
        assert_eq!(status.account_path.as_deref(), Some(provider_home(&directory).display().to_string().as_str()));

        let catalog = adapter.models("grok", 1).await.unwrap();
        assert_eq!(catalog.state, EvidenceState::Available);
        assert_eq!(catalog.source.as_deref(), Some("grok-cli:models"));
        assert_eq!(catalog.observed_at.as_deref(), Some("2026-09-30T00:00:00Z"));
        assert!(catalog.missing_fields.is_empty(), "{:?}", catalog.missing_fields);
        // 发现 ≠ 资格：一律 eligible=false，名字缺失即 None。
        assert_eq!(catalog.models[0], DiscoveredModel { model_id: "grok-4".into(), name: Some("Grok 4".into()), eligible: false });
        assert_eq!(catalog.models[1], DiscoveredModel { model_id: "grok-3".into(), name: None, eligible: false });

        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.state, EvidenceState::Available);
        assert_eq!(quota.view, QuotaView::GrokCliUsage);
        assert_eq!(quota.source.as_deref(), Some("grok-cli:usage"));
        assert_eq!(quota.observed_at.as_deref(), Some("2026-09-30T01:02:03Z"));
        assert!(!quota.history);
        let bucket = &quota.buckets[0];
        assert_eq!(bucket.limit_id, SUBSCRIPTION_POOL);
        assert_eq!(bucket.name.as_deref(), Some("SuperGrok"));
        assert_eq!(bucket.plan_type.as_deref(), Some("supergrok"));
        assert_eq!(bucket.permission, QuotaPermission::Allowed);
        assert_eq!(bucket.windows[0].label, "weekly");
        assert_eq!(bucket.windows[0].used_percent, Some(42.5));
        assert_eq!(bucket.windows[0].resets_at, rfc3339_seconds("2026-10-07T00:00:00Z"));
        let credits = bucket.credits.as_ref().unwrap();
        assert_eq!(credits.has_credits, Some(true));
        assert_eq!(credits.unlimited, Some(false));
        // 余额与单位原样保留，不解析金额、不换算。
        assert_eq!(credits.balance.as_deref(), Some("12.50"));
        assert_eq!(credits.unit.as_deref(), Some("USD"));
        // credits 许可与订阅内许可是两个轴，互不推导。
        assert_eq!(credits.permission, QuotaPermission::Denied);
        assert!(credits.missing_fields.is_empty() && credits.invalid_fields.is_empty());
    }

    #[tokio::test]
    async fn grok_reports_unsupported_interfaces_without_pretending_success() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"catalog\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let catalog = adapter.models("grok", 1).await.unwrap();
        assert_eq!(catalog.state, EvidenceState::Unsupported);
        assert!(catalog.models.is_empty());

        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"quota\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.state, EvidenceState::Unsupported);
        assert!(quota.buckets.is_empty());
        // 没有消费到额度事件时视图保持未知，不借用别家契约的名字。
        assert_eq!(quota.view, QuotaView::Unknown);

        // `account` 没有 machine 接口时不是「未连接」而是读不到事实：必须如实报错。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"account\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        assert!(adapter.status("grok", 1).await.is_err());
    }

    /// `unsupported` 必须认 interface：只有本次读取对应的接口才算终态，别的接口直接忽略。
    #[tokio::test]
    async fn an_unsupported_event_only_counts_for_its_own_interface() {
        let directory = tempfile::tempdir().unwrap();
        // 目录读取收到 quota 的 unsupported：忽略它，目录仍然可以成功读到。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"quota\"}'\nprintf '%s\\n' '{\"event\":\"catalog\",\"source\":\"grok-cli:models\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"models\":[{\"id\":\"grok-4\"}]}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let catalog = adapter.models("grok", 1).await.unwrap();
        assert_eq!(catalog.state, EvidenceState::Available);
        assert_eq!(catalog.models.len(), 1);

        // 只有别的接口的 unsupported：本次没有终态事件，如实失败，不伪造成 unsupported，也不伪造成功。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"quota\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let error = adapter.models("grok", 1).await.unwrap_err().to_string();
        assert!(error.contains(CODE_HELPER_EXITED), "{error}");

        // 缺 interface 的 unsupported 同样不算本次读取的终态。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        assert!(adapter.models("grok", 1).await.is_err());

        // 本接口的 unsupported 仍然如实认领。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"catalog\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        assert_eq!(adapter.models("grok", 1).await.unwrap().state, EvidenceState::Unsupported);

        // 额度读取只认 quota：catalog 的 unsupported 不改变额度结果。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"unsupported\",\"interface\":\"catalog\"}'\nprintf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":10,\"usage_allowed\":true}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.state, EvidenceState::Available);
        assert_eq!(quota.view, QuotaView::GrokCliUsage);
    }

    #[tokio::test]
    async fn grok_ignores_malformed_and_unknown_lines() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' 'not json at all' '{\"no_event\":true}' '{\"event\":\"future_thing\",\"value\":1}'\nprintf '%s\\n' '{\"event\":\"catalog\",\"source\":\"grok-cli:models\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"models\":[{\"id\":\"grok-4\"}]}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let catalog = adapter.models("grok", 1).await.unwrap();
        assert_eq!(catalog.state, EvidenceState::Available);
        assert_eq!(catalog.models.len(), 1);
        assert!(catalog.missing_fields.is_empty());
    }

    #[tokio::test]
    async fn grok_skips_catalog_entries_without_a_usable_id() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"catalog\",\"source\":\"grok-cli:models\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"models\":[{\"id\":\"grok-4\"},{\"display_name\":\"No id\"},{\"id\":\"   \"},\"not-an-object\",{\"id\":\"grok-3\"}]}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let catalog = adapter.models("grok", 1).await.unwrap();
        assert_eq!(catalog.models.iter().map(|model| model.model_id.as_str()).collect::<Vec<_>>(), vec!["grok-4", "grok-3"]);
        // 缺可用 id 的条目按契约逐个记账，绝不补造标识。
        assert_eq!(catalog.missing_fields, vec!["models[1].id", "models[2].id", "models[3].id"]);
    }

    #[tokio::test]
    async fn grok_never_rewrites_an_out_of_range_used_percent() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":142,\"usage_allowed\":true}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        let window = &quota.buckets[0].windows[0];
        // 越界不截断成 100、也不回落成 0：保持 None 并记原始文本。
        assert_eq!(window.used_percent, None);
        assert_eq!(window.invalid_fields, vec!["142"]);
        // 订阅内许可 allowed，但没有有效百分比 → 未知，绝不伪造成 available。
        assert_eq!(quota.buckets[0].permission, QuotaPermission::Allowed);
        assert_eq!(quota.state, EvidenceState::Unknown);

        // 字符串数字也不替上游解析。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":\"42\",\"usage_allowed\":true}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.buckets[0].windows[0].used_percent, None);
        assert_eq!(quota.buckets[0].windows[0].invalid_fields, vec!["42"]);

        // 边界值 0 与 100 都是有效百分比，必须原样采用。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":0,\"usage_allowed\":true}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.buckets[0].windows[0].used_percent, Some(0.0));
        assert_eq!(quota.state, EvidenceState::Available);
    }

    #[tokio::test]
    async fn grok_records_every_missing_field_instead_of_inventing_values() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":null,\"observed_at\":null,\"plan\":{},\"pool\":{},\"extra_usage\":{}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.state, EvidenceState::Unknown);
        assert!(quota.source.is_none() && quota.observed_at.is_none());
        let bucket = &quota.buckets[0];
        assert_eq!(bucket.plan_type, None);
        assert_eq!(bucket.name, None);
        assert_eq!(bucket.permission, QuotaPermission::Unknown);
        assert_eq!(bucket.missing_fields, vec!["plan.code", "plan.name", "pool.usage_allowed"]);
        let window = &bucket.windows[0];
        // label 用契约占位，绝不臆造窗口类型。
        assert_eq!(window.label, DEFAULT_WINDOW_LABEL);
        assert_eq!(window.used_percent, None);
        assert_eq!(window.resets_at, None);
        assert_eq!(window.missing_fields, vec!["pool.period.type", "pool.used_percent", "pool.period.end"]);
        let credits = bucket.credits.as_ref().unwrap();
        assert_eq!(credits.has_credits, None);
        assert_eq!(credits.balance, None);
        assert_eq!(credits.unit, None);
        assert_eq!(credits.permission, QuotaPermission::Unknown);
        assert_eq!(
            credits.missing_fields,
            vec![
                "extra_usage.has_credits",
                "extra_usage.unlimited",
                "extra_usage.balance",
                "extra_usage.unit",
                "extra_usage.permitted"
            ]
        );
        assert!(credits.invalid_fields.is_empty());

        // `extra_usage` 整个缺失时 credits 仍然存在（只是全部未知），并把这一层缺失记账：
        // 界面不能因为缺这一层字段就整段消失。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":10,\"usage_allowed\":true}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        let credits = quota.buckets[0].credits.as_ref().expect("credits 必须存在，只是未知");
        assert_eq!(credits.has_credits, None);
        assert_eq!(credits.unlimited, None);
        assert_eq!(credits.balance, None);
        assert_eq!(credits.unit, None);
        assert_eq!(credits.permission, QuotaPermission::Unknown);
        assert_eq!(credits.missing_fields, vec!["extra_usage"]);
        assert!(credits.invalid_fields.is_empty());
        // credits 轴不参与额度状态判定：订阅内 allowed + 有效百分比仍然是 available。
        assert_eq!(quota.state, EvidenceState::Available);

        // `extra_usage` 类型不符（不是对象）：同样给出未知 credits，并把原始文本记进桶的 invalid。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":10,\"usage_allowed\":true},\"extra_usage\":\"none\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.buckets[0].invalid_fields, vec!["none"]);
        assert_eq!(quota.buckets[0].credits.as_ref().unwrap().missing_fields, vec!["extra_usage"]);
        assert_eq!(quota.state, EvidenceState::Available);

        // 目录事件没有 `models` 时如实记缺失，不伪造条目。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"catalog\",\"source\":\"grok-cli:models\",\"observed_at\":\"2026-09-30T00:00:00Z\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let catalog = adapter.models("grok", 1).await.unwrap();
        assert!(catalog.models.is_empty());
        assert_eq!(catalog.missing_fields, vec!["models"]);
    }

    #[tokio::test]
    async fn grok_keeps_the_credit_balance_and_unit_verbatim() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":10,\"usage_allowed\":true},\"extra_usage\":{\"has_credits\":\"yes\",\"unlimited\":null,\"balance\":\"1,234.567 credits\",\"unit\":\"\",\"permitted\":null}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        let credits = quota.buckets[0].credits.as_ref().unwrap();
        // 余额原样保留：不解析金额、不补单位、不做任何换算。
        assert_eq!(credits.balance.as_deref(), Some("1,234.567 credits"));
        // 空字符串是上游给的原文，照实保留（不是「缺失」）。
        assert_eq!(credits.unit.as_deref(), Some(""));
        // 类型不符的布尔取值保持 None，并把原始文本记进 invalid。
        assert_eq!(credits.has_credits, None);
        assert_eq!(credits.invalid_fields, vec!["yes"]);
        assert_eq!(credits.unlimited, None);
        assert_eq!(credits.permission, QuotaPermission::Unknown);
        assert_eq!(credits.missing_fields, vec!["extra_usage.unlimited", "extra_usage.permitted"]);
    }

    #[tokio::test]
    async fn grok_keeps_the_two_permission_axes_independent() {
        let directory = tempfile::tempdir().unwrap();
        // 订阅内许可 allowed + 有效百分比 → available；credits 自身 denied 不参与状态判定。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":10,\"usage_allowed\":true},\"extra_usage\":{\"permitted\":false}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.buckets[0].permission, QuotaPermission::Allowed);
        assert_eq!(quota.buckets[0].credits.as_ref().unwrap().permission, QuotaPermission::Denied);
        assert_eq!(quota.state, EvidenceState::Available);

        // 订阅内许可是 denied → 额度状态 denied，不论 credits 怎么说。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":10,\"usage_allowed\":false},\"extra_usage\":{\"permitted\":true}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.state, EvidenceState::Denied);
        assert_eq!(quota.buckets[0].credits.as_ref().unwrap().permission, QuotaPermission::Allowed);

        // 非布尔许可如实未知，不猜成 allowed。
        let adapter = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"quota\",\"source\":\"grok-cli:usage\",\"observed_at\":\"2026-09-30T00:00:00Z\",\"pool\":{\"used_percent\":10,\"usage_allowed\":\"true\"}}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let quota = adapter.quota("grok", 1).await.unwrap();
        assert_eq!(quota.buckets[0].permission, QuotaPermission::Unknown);
        assert_eq!(quota.buckets[0].invalid_fields, vec!["true"]);
        assert_eq!(quota.state, EvidenceState::Unknown);
    }

    #[tokio::test]
    async fn grok_unverified_reads_stay_unknown_and_test_protocol_failures_are_honest() {
        // 缺少真实 CLI 或已核实机器接口：身份报明确错误，目录/额度维持 Unknown，不伪造成空结果。
        let directory = tempfile::tempdir().unwrap();
        let missing = GrokSubscriptionAdapter::from_parts(directory.path().to_path_buf(), None, Vec::new(), READ_TIMEOUT);
        assert!(!missing.available());
        assert!(missing.status("grok", 1).await.unwrap_err().to_string().contains(CODE_GROK_INTERFACE_UNVERIFIED));
        let catalog = missing.models("grok", 1).await.unwrap();
        assert_eq!(catalog.state, EvidenceState::Unknown);
        assert!(catalog.models.is_empty() && catalog.source.is_none() && catalog.observed_at.is_none());
        assert_eq!(catalog.missing_fields, vec!["machine_readable_catalog_interface_unverified"]);
        let quota = missing.quota("grok", 1).await.unwrap();
        assert_eq!(quota.state, EvidenceState::Unknown);
        assert_eq!(quota.view, QuotaView::Unknown);
        assert!(quota.buckets.is_empty() && quota.source.is_none() && quota.observed_at.is_none());
        assert_eq!(quota.missing_fields, vec!["machine_readable_subscription_quota_interface_unverified"]);

        // 提前退出、没有任何输出：如实失败，不伪造成空目录或空额度。
        let empty = fake_adapter(&directory, "exit 0");
        assert!(empty.models("grok", 1).await.unwrap_err().to_string().contains(CODE_HELPER_EXITED));

        // 只有无法解析/未知事件：同样没有终态事件，如实失败。
        let nonsense = fake_adapter(&directory, "printf '%s\\n' 'nonsense' '{\"event\":\"future\"}'");
        assert!(nonsense.quota("grok", 1).await.unwrap_err().to_string().contains(CODE_HELPER_EXITED));

        // 超时：如实失败并回收自己那个 pid。
        let slow = fake_adapter(&directory, "sleep 5").with_read_timeout(Duration::from_millis(250));
        assert!(slow.status("grok", 1).await.unwrap_err().to_string().contains(CODE_HELPER_TIMEOUT));

        // `error` 事件：如实失败，且 message 必须已脱敏。
        let failing = fake_adapter(
            &directory,
            "printf '%s\\n' '{\"event\":\"error\",\"code\":\"upstream_unavailable\",\"message\":\"login failed with sk-abcdefghijklmnopqrstuvwxyz0123456789\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let error = failing.models("grok", 1).await.unwrap_err().to_string();
        assert!(error.contains("upstream_unavailable"), "{error}");
        assert!(!error.contains("sk-abcdefghijklmnopqrstuvwxyz0123456789"), "{error}");
        assert!(error.contains("[redacted]"), "{error}");
    }

    #[test]
    fn read_program_only_allows_the_pinned_helper_inside_isolation() {
        let cli = std::path::Path::new("/tmp/fake-grok-cli");
        let pinned = std::path::Path::new("/tmp/pinned-grok-helper");
        // 隔离环境：只有显式 pinned 的只读替身才允许拉起；解析到的官方 CLI 不作数。
        let admitted = read_program(true, Some(pinned), Some(cli)).unwrap();
        assert_eq!(admitted, (pinned.to_path_buf(), true));
        let isolated = read_program(true, None, Some(cli)).unwrap_err();
        assert!(isolated.contains(CODE_HELPER_ISOLATED), "{isolated}");
        let isolated = read_program(true, None, None).unwrap_err();
        assert!(isolated.contains(CODE_HELPER_ISOLATED), "{isolated}");
        // 生产：隔离开关不适用，pinned 替身被忽略，只认登录路径解析到的官方 CLI。
        let admitted = read_program(false, Some(pinned), Some(cli)).unwrap();
        assert_eq!(admitted, (cli.to_path_buf(), false));
        let missing = read_program(false, Some(pinned), None).unwrap_err();
        assert!(missing.contains(CODE_HELPER_MISSING), "{missing}");
    }

    #[tokio::test]
    async fn refresh_writes_grok_catalog_and_quota_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = Arc::new(fake_adapter(&directory, GOOD_HELPER));
        let (store, _db) = grok_store(adapter);
        let connection = refresh(&store, "grok").await.unwrap();
        assert_eq!(connection.state, ConnectionState::Connected);
        assert_eq!(connection.identity.as_deref(), Some("grok@example.invalid"));
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.generation, 1);
        assert_eq!(evidence.account.as_deref(), Some("grok@example.invalid"));
        assert_eq!(evidence.catalog.state, EvidenceState::Available);
        assert_eq!(evidence.catalog.source.as_deref(), Some("grok-cli:models"));
        assert_eq!(evidence.models.len(), 2);
        assert_eq!(evidence.quota.state, EvidenceState::Available);
        assert_eq!(evidence.quota.view, QuotaView::GrokCliUsage);

        // 视图形状：catalog 只增字段，helper 只来自 Grok 自己。
        let mut sessions = crate::subscription::SessionState::default();
        sessions.helpers.insert(
            "grok".into(),
            store.subscription.helper_status(&ProviderKind::GrokSubscription),
        );
        sessions.supported_providers = vec!["grok".into()];
        let view = crate::subscription::views(&store.read(), true, &sessions).pop().unwrap();
        assert_eq!(view.catalog.state, EvidenceState::Available);
        assert_eq!(view.models.len(), 2);
        assert_eq!(view.quota.view, QuotaView::GrokCliUsage);
        assert!(view.helper.available && view.helper.version.is_none() && view.helper.auth_home.is_none());
        // 冻结的视图 JSON 形状（契约 §6）：字段名与枚举取值都必须是契约里的样子。
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["catalog"]["state"], "available");
        assert_eq!(json["catalog"]["source"], "grok-cli:models");
        assert_eq!(json["catalog"]["missing_fields"], serde_json::json!([]));
        assert_eq!(json["quota"]["state"], "available");
        assert_eq!(json["quota"]["view"], "grok_cli_usage");
        assert_eq!(json["quota"]["history"], false);
        assert_eq!(json["quota"]["buckets"][0]["limit_id"], "subscription_pool");
        assert_eq!(json["quota"]["buckets"][0]["permission"], "allowed");
        assert_eq!(json["quota"]["buckets"][0]["windows"][0]["label"], "weekly");
        assert_eq!(json["quota"]["buckets"][0]["windows"][0]["used_percent"], 42.5);
        assert_eq!(json["quota"]["buckets"][0]["credits"]["balance"], "12.50");
        assert_eq!(json["quota"]["buckets"][0]["credits"]["unit"], "USD");
        assert_eq!(json["quota"]["buckets"][0]["credits"]["permission"], "denied");

        // 世代绑定：换号递增世代后旧证据整体作废，再次刷新绑到新世代。
        store
            .update(|config| {
                config.subscriptions.get_mut("grok").unwrap().generation += 1;
            })
            .unwrap();
        assert!(store.read().subscriptions.get("grok").unwrap().current_evidence().is_none());
        let rebased = refresh(&store, "grok").await.unwrap();
        assert_eq!(rebased.generation, 2);
        assert_eq!(rebased.current_evidence().unwrap().generation, 2);
    }

    #[tokio::test]
    async fn refresh_keeps_stale_catalog_and_history_quota_when_a_later_read_fails() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = Arc::new(fake_adapter(&directory, GOOD_HELPER));
        let (store, _db) = grok_store(adapter);
        let before = refresh(&store, "grok").await.unwrap();
        let observed_at = before.current_evidence().unwrap().quota.observed_at.clone();

        // account 仍然读得到，models/usage 失败：目录标陈旧、额度标失败并保留历史数字与时间。
        write_helper_note(&directory, "mode", "fail");
        let connection = refresh(&store, "grok").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(evidence.models.len(), 2);
        assert_eq!(evidence.catalog.source.as_deref(), Some("grok-cli:models"));
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        assert!(evidence.quota.history);
        assert_eq!(evidence.quota.observed_at, observed_at);
        assert_eq!(evidence.quota.buckets.len(), 1);
        // 失败的读取绝不刷新时间，也绝不把桶清零。
        assert_eq!(evidence.quota.buckets[0].windows[0].used_percent, Some(42.5));
    }

    #[tokio::test]
    async fn refresh_reports_failed_without_history_when_nothing_was_ever_read() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = Arc::new(fake_adapter(&directory, GOOD_HELPER));
        let (store, _db) = grok_store(adapter);
        write_helper_note(&directory, "mode", "fail");
        let connection = refresh(&store, "grok").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.catalog.state, EvidenceState::Failed);
        assert!(evidence.models.is_empty());
        assert_eq!(evidence.quota.state, EvidenceState::Failed);
        // 从来没有成功读过，就没有「最后成功更新」可言。
        assert!(!evidence.quota.history);
        assert!(evidence.quota.buckets.is_empty() && evidence.quota.observed_at.is_none());
    }

    #[tokio::test]
    async fn refresh_discards_the_read_when_the_account_changed() {
        let directory = tempfile::tempdir().unwrap();
        let adapter = Arc::new(fake_adapter(&directory, GOOD_HELPER));
        let (store, _db) = grok_store(adapter);
        refresh(&store, "grok").await.unwrap();
        let before = serde_json::to_value(store.read().subscriptions.get("grok").unwrap()).unwrap();
        // 辅助进程报出的是另一个账号：本次读取整体丢弃，一个字段都不改写。
        write_helper_note(&directory, "identity", "other@example.invalid");
        let error = refresh(&store, "grok").await.unwrap_err();
        assert!(error.contains("account changed"), "{error}");
        let after = serde_json::to_value(store.read().subscriptions.get("grok").unwrap()).unwrap();
        assert_eq!(after, before);
    }

    /// 只承载 Codex 的替身：用来证明注册表不会把它的读取结果或 helper 自述借给 Grok。
    struct CodexOnlyStub {
        reads: std::sync::atomic::AtomicUsize,
    }

    impl SubscriptionAdapter for CodexOnlyStub {
        fn available(&self) -> bool {
            true
        }

        fn supports(&self, kind: &ProviderKind) -> bool {
            matches!(kind, ProviderKind::CodexSubscription)
        }

        fn helper_status(&self) -> HelperStatus {
            HelperStatus { available: true, user_agent: None, version: Some("9.9.9".into()), auth_home: Some("/tmp/codex-home".into()) }
        }

        fn status<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<ConnectionStatus>> {
            Box::pin(async move {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ConnectionStatus {
                    state: ConnectionState::Connected,
                    identity_incomplete: false,
                    identity: Some("codex@example.invalid".into()),
                    helper_version: Some("9.9.9".into()),
                    account_path: Some("/tmp/codex-home".into()),
                })
            })
        }

        fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<CatalogRead>> {
            Box::pin(async move {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(CatalogRead {
                    state: EvidenceState::Available,
                    models: vec![DiscoveredModel { model_id: "codex-model".into(), name: None, eligible: false }],
                    source: Some("codex-cli:models".into()),
                    observed_at: Some("2026-09-30T00:00:00Z".into()),
                    missing_fields: Vec::new(),
                })
            })
        }

        fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<QuotaEvidence>> {
            Box::pin(async move {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(QuotaEvidence { state: EvidenceState::Available, source: Some("codex-cli:usage".into()), ..QuotaEvidence::default() })
            })
        }

        fn generate<'a>(&'a self, _request: GenerationRequest<'a>) -> BoxFuture<'a, Result<GenerationStream<'a>>> {
            Box::pin(async { bail!("stub") })
        }

        fn start_login<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LoginStart>> {
            Box::pin(async { bail!("stub") })
        }

        fn login_result<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<Option<LoginResult>>> {
            Box::pin(async { bail!("stub") })
        }

        fn cancel_login<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { bail!("stub") })
        }

        fn logout<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LogoutOutcome>> {
            Box::pin(async { bail!("stub") })
        }
    }

    #[tokio::test]
    async fn a_registry_never_lends_grok_the_codex_adapter() {
        let directory = tempfile::tempdir().unwrap();
        let grok = Arc::new(fake_adapter(&directory, GOOD_HELPER));
        let codex = Arc::new(CodexOnlyStub { reads: std::sync::atomic::AtomicUsize::new(0) });
        // 生产形状的注册表：两家按 kind 分派。
        let registry = SubscriptionAdapters::new(vec![codex.clone(), grok.clone()]);
        assert!(registry.supports(&ProviderKind::GrokSubscription));
        assert!(registry.supports(&ProviderKind::CodexSubscription));
        // per-kind helper 状态：Grok 行拿不到 Codex 的版本与授权目录。
        let grok_helper = registry.helper_status(&ProviderKind::GrokSubscription);
        assert!(grok_helper.version.is_none() && grok_helper.auth_home.is_none());
        let codex_helper = registry.helper_status(&ProviderKind::CodexSubscription);
        assert_eq!(codex_helper.version.as_deref(), Some("9.9.9"));
        assert_eq!(codex_helper.auth_home.as_deref(), Some("/tmp/codex-home"));
        assert!(registry.available(&ProviderKind::GrokSubscription) && registry.available(&ProviderKind::CodexSubscription));

        // 真刷新也走 Grok 自己的适配器：Codex 替身一次都没被调用。
        let db = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::load_with_adapters(
            db.path().join("autojev.db"),
            Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true }),
            grok.clone(),
        )
        .unwrap();
        store.subscription = Arc::new(SubscriptionAdapters::new(vec![codex.clone(), grok.clone()]));
        let store = Arc::new(store);
        store
            .update(|config| {
                for (id, kind) in [("grok", ProviderKind::GrokSubscription), ("codex", ProviderKind::CodexSubscription)] {
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
                    sync_provider(config, &provider.id, &provider.kind);
                }
            })
            .unwrap();
        let connection = refresh(&store, "grok").await.unwrap();
        let evidence = connection.current_evidence().unwrap();
        assert_eq!(evidence.account.as_deref(), Some("grok@example.invalid"));
        assert_eq!(evidence.catalog.source.as_deref(), Some("grok-cli:models"));
        assert_eq!(evidence.models[0].model_id, "grok-4");
        assert_eq!(codex.reads.load(std::sync::atomic::Ordering::SeqCst), 0, "Grok 读取不得碰 Codex 适配器");
    }
}
