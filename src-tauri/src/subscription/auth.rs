//! Grok 登录、退出与换号的生命周期：进程内登录会话 + 持久化连接世代。
//!
//! 分工是刻意的：[`GrokCliAuth`] 只管辅助进程与进程内会话，[`begin`]/[`poll`]/[`cancel`]/
//! [`logout`]/[`switch_account`] 负责把结果写进 [`ConfigStore`]。因此这些函数都能用测试替身驱动，
//! 生产实现也不需要真实 OAuth、浏览器或模型调用就能被完整单测。
//!
//! 登录会话是进程内的：重启后 [`crate::config::ConfigStore`] 会把残留的 `AuthorizationPending`
//! 归位为未连接，pending 登录不会跨进程存活。

use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::{
    config::{AppConfig, ConfigStore, Provider, ProviderKind},
    subscription::{
        helper::{self, OwnedProcesses},
        is_subscription_provider, ConnectionState,
    },
};

/// 界面与错误共用的稳定 code。未知 code 由前端回退原文。
pub const CODE_ALREADY_CONNECTED: &str = "already_connected";
pub const CODE_HELPER_ISOLATED: &str = "helper_isolated";
pub const CODE_HELPER_MISSING: &str = "helper_missing";
/// 该订阅服务商的授权路线尚未接入：不得借用 Grok 辅助进程。
pub const CODE_HELPER_UNSUPPORTED: &str = "helper_unsupported";
/// 登录启动期间连接被换号或服务商被删除：新尝试已中止。
const CODE_LOGIN_SUPERSEDED: &str = "login_superseded";
/// 退出时更早的世代守卫已经失效（读配置与写配置之间连接被并发登录/换号推进）：
/// 这次退出既没有写连接状态、也没有回收进程或清理专用 home，界面必须如实告知并允许重试。
const CODE_LOGOUT_SUPERSEDED: &str = "logout_superseded";
pub const CODE_NOT_CONNECTED: &str = "not_connected";
pub const CODE_NOT_SUBSCRIPTION: &str = "not_subscription";
pub const CODE_HELPER_EXITED: &str = "helper_exited";
pub const CODE_HELPER_TIMEOUT: &str = "helper_timeout";
pub const CODE_IDENTITY_MISSING: &str = "identity_missing";

/// 等待辅助进程给出第一个 challenge 的上限；真实 CLI 的响应时间未经验证。
const HELPER_CHALLENGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// 远端撤销的诚实说明：本票没有实现真实撤销，就不能暗示已撤销。
const REMOTE_NOT_ATTEMPTED: &str = "No remote revocation was attempted; the upstream account was not revoked by AutoJev.";

/// 进行中的登录阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthPhase {
    #[default]
    Idle,
    Pending,
    Succeeded,
    Failed,
    Cancelled,
}

/// 本地退出结果：只描述应用自有存储与进程被清理的事实。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalLogoutState {
    #[default]
    NotAttempted,
    Cleared,
    Failed,
}

/// 远端撤销结果：未做真实撤销时绝不能出现 `Verified`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRevokeState {
    #[default]
    NotAttempted,
    Failed,
    Verified,
    Unsupported,
}

/// 需要用户配合完成的挑战：说明 + 可复制的验证地址与用户码。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub kind: String,
    pub instructions: String,
    #[serde(default)]
    pub verification_url: Option<String>,
    #[serde(default)]
    pub user_code: Option<String>,
}

/// 登录失败：稳定 code + 原因 + 恢复动作。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthError {
    pub code: String,
    pub message: String,
    pub recovery: String,
}

/// 退出证据：本地清除与远端撤销必须分开报告。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogoutEvidence {
    pub local: LocalLogoutState,
    #[serde(default)]
    pub local_detail: Option<String>,
    pub remote: RemoteRevokeState,
    #[serde(default)]
    pub remote_detail: Option<String>,
}

/// 辅助进程可用性与存储目标。`version` 未知时为 `None`，不编造。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperInfo {
    pub available: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub home: Option<String>,
}

/// 界面视图：只有状态与已核实身份，不含任何凭据或辅助进程输出原文。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthView {
    pub provider_id: String,
    pub phase: AuthPhase,
    pub generation: u64,
    #[serde(default)]
    pub attempt: Option<u64>,
    #[serde(default)]
    pub challenge: Option<AuthChallenge>,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub error: Option<AuthError>,
    pub helper: HelperInfo,
    pub logout: LogoutEvidence,
}

impl Default for AuthView {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            phase: AuthPhase::Idle,
            generation: 1,
            attempt: None,
            challenge: None,
            identity: None,
            error: None,
            helper: HelperInfo::default(),
            logout: LogoutEvidence::default(),
        }
    }
}

/// 一次轮询的结果。`Superseded` 表示这次结果属于旧世代或旧 attempt，调用方不得写入任何状态。
/// `Cancelled` 只在测试替身里被构造；生产实现的取消直接落到 [`AuthPhase::Cancelled`]。
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthPoll {
    Pending,
    Succeeded { identity: String },
    Failed { error: AuthError },
    Cancelled,
    Superseded,
}

fn auth_error(code: &str, message: impl Into<String>, recovery: &str) -> AuthError {
    AuthError {
        code: helper::redact(code),
        message: helper::redact(&message.into()),
        recovery: helper::redact(recovery),
    }
}

/// 订阅授权边界：只读视图、登录会话、退出与自有进程回收。
/// 实现只在构造 [`ConfigStore`] 时注入，生产构造没有任何配置或界面开关能把它换成替身。
pub trait SubscriptionAuth: Send + Sync {
    fn available(&self) -> bool;
    fn isolated(&self) -> bool;
    /// 纯内存视图，不做任何 I/O。
    fn view(&self, provider_id: &str, generation: u64) -> AuthView;
    /// 开始一次登录。成功后必须清除该 provider 的旧退出证据：
    /// 上一世代的 `cleared` 记录不得继续显示在新账号上。
    fn begin<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<AuthChallenge, String>>;
    fn poll<'a>(&'a self, provider_id: &'a str, generation: u64, attempt: u64) -> BoxFuture<'a, Result<AuthPoll, String>>;
    /// 取消一次登录。语义固定：只在 session 仍为 Pending 且 attempt 匹配时生效，否则纯 no-op，
    /// 不得改动任何既有会话状态（已成功的 poll 不会被取消“撤销”）。
    /// 收尾同样遵守不变式：只销毁这次尝试自己的进程；只有该 provider 已无自有存活进程时才清专用 home。
    fn cancel<'a>(&'a self, provider_id: &'a str, attempt: u64) -> BoxFuture<'a, Result<(), String>>;
    fn logout<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<LogoutEvidence, String>>;
    /// 只回收本应用登记的自有进程，返回 pid 列表；不改配置。
    fn shutdown(&self) -> Vec<u32>;
    /// 同步丢弃某家服务商的自有资源：回收自有进程 + 丢弃进程内 session 与退出证据 + 清空专用 home。
    /// 返回清理是否成功；失败时调用方应拒绝继续 teardown（例如删除服务商）。
    fn dispose(&self, provider_id: &str) -> Result<(), String>;
    /// 同步迁移某家服务商的自有资源到新 id：session 与退出证据改键，old home 存在时移动到 new home。
    /// new home 已存在或移动失败返回 Err，绝不覆盖。
    fn rename(&self, old_id: &str, new_id: &str) -> Result<(), String>;
}

/// 一次进程内登录会话。事件接收端被取消后即丢弃，迟到的输出不会再被消费。
struct Session {
    generation: u64,
    attempt: u64,
    phase: AuthPhase,
    challenge: Option<AuthChallenge>,
    identity: Option<String>,
    error: Option<AuthError>,
    events: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// 这次尝试自己拉起的自有进程 pid：失败/取消的收尾只能回收它，
    /// 不得按 provider 连坐——同一服务商的并发新尝试不得被旧尝试的收尾杀掉。
    pid: Option<u32>,
}

/// 生产实现：官方 Grok CLI 辅助进程 + 应用自有 home。
pub struct GrokCliAuth {
    home: PathBuf,
    program: Option<PathBuf>,
    extra_args: Vec<String>,
    processes: OwnedProcesses,
    sessions: Mutex<HashMap<String, Session>>,
    logouts: Mutex<HashMap<String, LogoutEvidence>>,
}

impl GrokCliAuth {
    pub fn new() -> Self {
        let home = crate::runtime::home_dir().unwrap_or_default();
        let (program, extra_args) = match helper::resolve_program() {
            Some((program, extra_args)) => (Some(program), extra_args),
            None => (None, Vec::new()),
        };
        Self::from_parts(home, program, extra_args)
    }

    fn from_parts(home: PathBuf, program: Option<PathBuf>, extra_args: Vec<String>) -> Self {
        Self {
            home,
            program,
            extra_args,
            processes: OwnedProcesses::new(),
            sessions: Mutex::new(HashMap::new()),
            logouts: Mutex::new(HashMap::new()),
        }
    }

    /// 测试专用：注入隔离 home 与本地假 helper，绝不触碰真实用户目录或真实 CLI。
    /// `pub(crate)` 让 config.rs 的载入清理用例也能用真实实现驱动。
    #[cfg(test)]
    pub(crate) fn with_test_helper(home: PathBuf, program: PathBuf) -> Self {
        Self::from_parts(home, Some(program), Vec::new())
    }

    fn helper_info(&self, provider_id: &str) -> HelperInfo {
        HelperInfo {
            // 隔离验证环境不拉起任何真实进程，因此对本应用而言辅助进程就是不可用的；
            // 这同时也保证隔离检查在任何开发机上都能看到 available=false。
            available: self.available(),
            // 探测不拉起进程，拿不到版本就必须保持未知。
            version: None,
            program: self.program.as_ref().map(|program| program.display().to_string()),
            home: helper::helper_home(&ProviderKind::GrokSubscription, provider_id, &self.home)
                .ok()
                .map(|home| home.display().to_string()),
        }
    }

    fn spec(&self, provider_id: &str, program: &PathBuf) -> Result<helper::HelperSpec, String> {
        helper::spec_with_program(&ProviderKind::GrokSubscription, provider_id, &self.home, program, &self.extra_args)
            .map_err(|error| helper::redact(&error.to_string()))
    }

    /// 被取消或失败的登录必须清空该服务商的应用自有 home：pending 时不可能存在有效账号
    /// （begin 已拒绝 Connected），因此清理安全，这是 AC4「退出清理」的一部分。
    /// 不变式：专用 home 由同一服务商的所有尝试共享，**只有该 provider 已无自有存活进程时**
    /// 才允许调用这里的清理，否则会清掉并发新尝试正在用的目录。
    /// 清理失败不影响状态归位，也不把失败伪装成成功。
    fn cleanup_provider_home(&self, provider_id: &str) {
        if let Ok(home) = helper::helper_home(&ProviderKind::GrokSubscription, provider_id, &self.home) {
            let _ = helper::cleanup_home(&home);
        }
    }

    /// 一次失败尝试的收尾：回收自己刚拉起的自有进程，并（在安全时）清掉专用 home，
    /// 避免失败路径留下脏目录（下一次 begin 不得复用脏目录）。
    /// 不变式：**销毁只针对自己那次尝试**——
    /// 有 pid 时只回收这个 pid，绝不按 provider 连坐；专用 home 是同一服务商共享的目录，
    /// 只有该 provider 已无任何自有存活进程（说明没有并发的新尝试在用）时才清理。
    fn abort_attempt(&self, provider_id: &str, pid: Option<u32>) {
        if let Some(pid) = pid {
            self.processes.reclaim_pid(pid);
        }
        if !self.processes.has_owned(provider_id) {
            self.cleanup_provider_home(provider_id);
        }
    }

    fn provider_home(&self, provider_id: &str) -> Result<PathBuf, String> {
        helper::helper_home(&ProviderKind::GrokSubscription, provider_id, &self.home)
            .map_err(|error| helper::redact(&error.to_string()))
    }
}

impl Default for GrokCliAuth {
    fn default() -> Self {
        Self::new()
    }
}

/// 辅助进程 stdout 的适配事件。这是本应用的适配契约，未对真实 CLI 验证。
enum HelperEvent {
    Challenge(AuthChallenge),
    Identity(String),
    Done,
    Error(AuthError),
}

/// 事件字段原样保留：身份标识与验证地址是用户要核对的证据，不得被脱敏涂掉。
/// 脱敏只用于 [`AuthError`] 的 code/message/recovery 与日志/detail。
fn json_text(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|entry| entry.as_str()).map(str::to_owned)
}

fn parse_event(line: &str) -> Option<HelperEvent> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    match value.get("event")?.as_str()? {
        "challenge" => Some(HelperEvent::Challenge(AuthChallenge {
            kind: json_text(&value, "kind").unwrap_or_else(|| "device_code".into()),
            instructions: json_text(&value, "instructions").unwrap_or_default(),
            verification_url: json_text(&value, "verification_url"),
            user_code: json_text(&value, "user_code"),
        })),
        "identity" => Some(HelperEvent::Identity(json_text(&value, "identity").unwrap_or_default())),
        "done" => Some(HelperEvent::Done),
        "error" => Some(HelperEvent::Error(auth_error(
            &json_text(&value, "code").unwrap_or_else(|| "helper_error".into()),
            json_text(&value, "message").unwrap_or_default(),
            "Check the official Grok CLI output, then start the login again.",
        ))),
        _ => None,
    }
}

impl SubscriptionAuth for GrokCliAuth {
    fn available(&self) -> bool {
        // 隔离验证环境不允许拉起辅助进程，可用性必须如实报否。
        self.program.is_some() && !crate::runtime::isolated()
    }

    fn isolated(&self) -> bool {
        crate::runtime::isolated()
    }

    fn view(&self, provider_id: &str, generation: u64) -> AuthView {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions.get(provider_id).filter(|session| session.generation == generation);
        AuthView {
            provider_id: provider_id.to_owned(),
            phase: session.map(|session| session.phase).unwrap_or_default(),
            generation,
            attempt: session.map(|session| session.attempt),
            challenge: session.and_then(|session| session.challenge.clone()),
            identity: session.and_then(|session| session.identity.clone()),
            error: session.and_then(|session| session.error.clone()),
            helper: self.helper_info(provider_id),
            logout: self.logouts.lock().unwrap().get(provider_id).cloned().unwrap_or_default(),
        }
    }

    fn begin<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<AuthChallenge, String>> {
        Box::pin(async move {
            if crate::runtime::isolated() {
                return Err(refusal(CODE_HELPER_ISOLATED, "Grok authorization is unavailable in isolated validation"));
            }
            let Some(program) = self.program.clone() else {
                return Err(refusal(CODE_HELPER_MISSING, "No Grok CLI helper was found on PATH"));
            };
            let spec = self.spec(provider_id, &program)?;
            if let Err(error) = helper::prepare_home(&spec.home) {
                // 准备 home 失败也可能已经建出目录：同样按失败尝试收尾（此时还没有 pid）。
                self.abort_attempt(provider_id, None);
                return Err(refusal(CODE_HELPER_MISSING, &error));
            }
            // 新尝试取代旧尝试：连接处于 Pending 时重入 begin 会拉起第二个 helper，
            // 因此先回收同一服务商仍存活的自有进程，避免遗留孤儿；只回收本应用登记过的 pid。
            self.processes.reclaim(provider_id);
            let pid = match self.processes.spawn(provider_id, &spec) {
                Ok(pid) => pid,
                Err(error) => {
                    self.abort_attempt(provider_id, None);
                    return Err(helper::redact(&error.to_string()));
                }
            };
            let Some(stdout) = self.processes.take_stdout(pid) else {
                self.abort_attempt(provider_id, Some(pid));
                return Err(refusal(CODE_HELPER_MISSING, "the helper produced no output stream"));
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
            // 首个 challenge 决定这次登录是否真的开始；真实 CLI 输出契约未经验证。
            let challenge = loop {
                match tokio::time::timeout(HELPER_CHALLENGE_TIMEOUT, receiver.recv()).await {
                    Ok(Some(line)) => match parse_event(&line) {
                        Some(HelperEvent::Challenge(challenge)) => break challenge,
                        Some(HelperEvent::Error(error)) => {
                            self.abort_attempt(provider_id, Some(pid));
                            return Err(refusal(&error.code, &error.message));
                        }
                        _ => {}
                    },
                    Ok(None) => {
                        self.abort_attempt(provider_id, Some(pid));
                        return Err(refusal(CODE_HELPER_EXITED, "the helper exited before returning a challenge"));
                    }
                    Err(_) => {
                        self.abort_attempt(provider_id, Some(pid));
                        return Err(refusal(CODE_HELPER_TIMEOUT, "the helper did not return a challenge in time"));
                    }
                }
            };
            let mut sessions = self.sessions.lock().unwrap();
            let previous = sessions
                .get(provider_id)
                .filter(|session| session.generation == generation)
                .map(|session| session.attempt)
                .unwrap_or(0);
            sessions.insert(
                provider_id.to_owned(),
                Session {
                    generation,
                    // attempt 在同一世代内单调递增；换号后重新从 1 开始。
                    attempt: previous + 1,
                    phase: AuthPhase::Pending,
                    challenge: Some(challenge.clone()),
                    identity: None,
                    error: None,
                    events: Some(receiver),
                    pid: Some(pid),
                },
            );
            // 新登录开始即作废上一世代的退出证据：否则新账号界面仍会显示旧的「本地已清除」。
            self.logouts.lock().unwrap().remove(provider_id);
            Ok(challenge)
        })
    }

    fn poll<'a>(&'a self, provider_id: &'a str, generation: u64, attempt: u64) -> BoxFuture<'a, Result<AuthPoll, String>> {
        Box::pin(async move {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(session) = sessions.get_mut(provider_id) else {
                return Ok(AuthPoll::Superseded);
            };
            // 旧世代、旧 attempt 或已终结的会话一律 Superseded：迟到的结果不得写入任何状态。
            if session.generation != generation || session.attempt != attempt || session.phase != AuthPhase::Pending {
                return Ok(AuthPoll::Superseded);
            }
            let identity_missing = || {
                auth_error(
                    CODE_IDENTITY_MISSING,
                    "The helper finished the login without a verified account identity",
                    "Authorize again and make sure the helper reports the account identity.",
                )
            };
            let mut terminal: Option<AuthPoll> = None;
            if let Some(receiver) = session.events.as_mut() {
                loop {
                    match receiver.try_recv() {
                        Ok(line) => match parse_event(&line) {
                            Some(HelperEvent::Challenge(challenge)) => session.challenge = Some(challenge),
                            Some(HelperEvent::Identity(identity)) => {
                                let identity = identity.trim().to_owned();
                                if identity.is_empty() {
                                    terminal = Some(AuthPoll::Failed { error: identity_missing() });
                                } else {
                                    session.identity = Some(identity);
                                }
                            }
                            Some(HelperEvent::Done) => {
                                terminal = Some(match session.identity.clone() {
                                    Some(identity) => AuthPoll::Succeeded { identity },
                                    None => AuthPoll::Failed { error: identity_missing() },
                                });
                            }
                            Some(HelperEvent::Error(error)) => terminal = Some(AuthPoll::Failed { error }),
                            None => {}
                        },
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            if terminal.is_none() {
                                terminal = Some(AuthPoll::Failed {
                                    error: auth_error(
                                        CODE_HELPER_EXITED,
                                        "The helper exited before the login finished",
                                        "Start the login again once the official CLI is available.",
                                    ),
                                });
                            }
                            break;
                        }
                    }
                    if terminal.is_some() {
                        break;
                    }
                }
            }
            let result = match terminal {
                Some(AuthPoll::Succeeded { identity }) => {
                    session.phase = AuthPhase::Succeeded;
                    session.identity = Some(identity.clone());
                    session.challenge = None;
                    session.error = None;
                    AuthPoll::Succeeded { identity }
                }
                Some(AuthPoll::Failed { error }) => {
                    session.phase = AuthPhase::Failed;
                    session.identity = None;
                    session.challenge = None;
                    session.error = Some(error.clone());
                    AuthPoll::Failed { error }
                }
                _ => AuthPoll::Pending,
            };
            let finished = !matches!(result, AuthPoll::Pending);
            let attempt_pid = session.pid;
            drop(sessions);
            if finished {
                // 只回收这次尝试自己的进程：并发的新尝试不得被本尝试的终态收尾连坐。
                if let Some(pid) = attempt_pid {
                    self.processes.reclaim_pid(pid);
                }
                // 失败的尝试按 AC4 清掉应用自有 home 内容；但只有该 provider 已无自有存活进程时才清，
                // 否则会把并发新尝试正在用的目录清掉。成功则保留该次授权需要的存储。
                if matches!(result, AuthPoll::Failed { .. }) && !self.processes.has_owned(provider_id) {
                    self.cleanup_provider_home(provider_id);
                }
            }
            Ok(result)
        })
    }

    fn cancel<'a>(&'a self, provider_id: &'a str, attempt: u64) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let (cancelled, attempt_pid) = {
                let mut sessions = self.sessions.lock().unwrap();
                match sessions.get_mut(provider_id) {
                    Some(session) if session.phase == AuthPhase::Pending && session.attempt == attempt => {
                        session.phase = AuthPhase::Cancelled;
                        session.challenge = None;
                        // 丢弃读取端：迟到的输出不会被消费，也就无法写入任何状态。
                        session.events = None;
                        (true, session.pid)
                    }
                    _ => (false, None),
                }
            };
            if let Some(pid) = attempt_pid {
                // 只回收本次尝试自己的进程：并发的新尝试（例如已 spawn 但还没写入 session 的那个）
                // 不得被这次取消连坐杀掉。
                self.processes.reclaim_pid(pid);
            }
            if cancelled && !self.processes.has_owned(provider_id) {
                // 中止的尝试按 AC4 清掉应用自有 home 内容；pending 期间不可能存在有效账号。
                // 只在无自有存活进程时清，避免清掉并发新尝试正在用的目录。
                self.cleanup_provider_home(provider_id);
            }
            Ok(())
        })
    }

    fn logout<'a>(&'a self, provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LogoutEvidence, String>> {
        Box::pin(async move {
            self.processes.reclaim(provider_id);
            {
                let mut sessions = self.sessions.lock().unwrap();
                sessions.remove(provider_id);
            }
            let local = match helper::helper_home(&ProviderKind::GrokSubscription, provider_id, &self.home) {
                Ok(home) => match helper::cleanup_home(&home) {
                    Ok(()) => (LocalLogoutState::Cleared, format!("Cleared {}", home.display())),
                    Err(error) => (LocalLogoutState::Failed, error.to_string()),
                },
                Err(error) => (LocalLogoutState::Failed, error.to_string()),
            };
            let evidence = LogoutEvidence {
                local: local.0,
                local_detail: Some(helper::redact(&local.1)),
                // 本票没有实现真实远端撤销：只能如实报未尝试，绝不写 Verified。
                remote: RemoteRevokeState::NotAttempted,
                remote_detail: Some(REMOTE_NOT_ATTEMPTED.to_owned()),
            };
            self.logouts.lock().unwrap().insert(provider_id.to_owned(), evidence.clone());
            Ok(evidence)
        })
    }

    fn shutdown(&self) -> Vec<u32> {
        self.processes.reclaim_all()
    }

    fn dispose(&self, provider_id: &str) -> Result<(), String> {
        self.processes.reclaim(provider_id);
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.remove(provider_id);
        }
        self.logouts.lock().unwrap().remove(provider_id);
        let home = self.provider_home(provider_id)?;
        if home.exists() {
            // 清理失败必须变成 Err（而不是静默成功），删除／重命名才能据此拒绝继续。
            helper::cleanup_home(&home).map_err(|error| helper::redact(&error.to_string()))?;
        }
        Ok(())
    }

    fn rename(&self, old_id: &str, new_id: &str) -> Result<(), String> {
        if old_id == new_id {
            return Ok(());
        }
        let old_home = self.provider_home(old_id)?;
        let new_home = self.provider_home(new_id)?;
        // 先把 home 搬完再迁移内存状态：失败时不会留下半迁移的 session／证据。
        helper::move_home(&old_home, &new_home).map_err(|error| helper::redact(&error.to_string()))?;
        // 自有进程登记也要改键，否则新 id 的 reclaim 抓不到旧进程，它会活到应用退出。
        self.processes.rename_provider(old_id, new_id);
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(session) = sessions.remove(old_id) {
                sessions.insert(new_id.to_owned(), session);
            }
        }
        {
            let mut logouts = self.logouts.lock().unwrap();
            if let Some(evidence) = logouts.remove(old_id) {
                logouts.insert(new_id.to_owned(), evidence);
            }
        }
        Ok(())
    }
}

/// 找到订阅服务商；订阅授权只对订阅服务商开放。
fn provider_of(config: &AppConfig, provider_id: &str) -> Result<Provider, String> {
    config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .cloned()
        .ok_or_else(|| helper::redact(&format!("Unknown subscription provider: {provider_id}")))
}

/// 拒绝消息统一形如 `<code>: <原因>`，并且必须经过脱敏。
fn refusal(code: &str, message: impl std::fmt::Display) -> String {
    helper::redact(&format!("{code}: {message}"))
}

fn require_subscription(provider: &Provider) -> Result<(), String> {
    if is_subscription_provider(provider) {
        Ok(())
    } else {
        Err(refusal(CODE_NOT_SUBSCRIPTION, format!("{} is not a subscription provider", provider.id)))
    }
}

/// 两家订阅的授权路线分开验收：本构建只管理 Grok CLI 辅助进程，
/// 其他订阅服务商不得借用它，也不得因登录/换号而改动世代与身份。
fn require_grok(provider: &Provider) -> Result<(), String> {
    if provider.kind == ProviderKind::GrokSubscription {
        Ok(())
    } else {
        Err(refusal(
            CODE_HELPER_UNSUPPORTED,
            format!(
                "{} authorization is not implemented in this build; only the Grok CLI helper is managed",
                provider.name
            ),
        ))
    }
}

fn connection_of(config: &AppConfig, provider_id: &str) -> crate::subscription::Connection {
    config.subscriptions.get(provider_id).cloned().unwrap_or_default()
}

/// 开始登录：仅订阅服务商；已连接、隔离环境、helper 缺失都拒绝。
/// 成功后世代不变、`attempt` 递增、连接状态进入 `AuthorizationPending`。
pub async fn begin(store: &ConfigStore, provider_id: &str) -> Result<AuthView, String> {
    let config = store.read();
    let provider = provider_of(&config, provider_id)?;
    require_subscription(&provider)?;
    // 类型检查必须早于已连接/隔离/可用性与任何世代改动：不支持的服务商一律不落地。
    require_grok(&provider)?;
    let generation = connection_of(&config, provider_id).generation;
    if config.subscriptions.get(provider_id).is_some_and(|connection| connection.state == ConnectionState::Connected) {
        return Err(refusal(
            CODE_ALREADY_CONNECTED,
            format!("{} is already connected; log out before starting a new login", provider.name),
        ));
    }
    if store.auth.isolated() {
        return Err(refusal(CODE_HELPER_ISOLATED, format!("{} authorization is unsupported in isolated validation", provider.name)));
    }
    if !store.auth.available() {
        return Err(refusal(
            CODE_HELPER_MISSING,
            "no official helper is available; install the CLI or set AUTOJEV_GROK_HELPER",
        ));
    }
    let auth = store.auth.clone();
    auth.begin(provider_id, generation).await.map_err(|error| helper::redact(&error))?;
    // 世代竞态：辅助进程启动期间连接可能被换号或服务商被删除。
    // 此时刚拉起的自有进程必须回收，且绝不写入任何状态。
    {
        let current = store.read();
        let still_valid = current
            .providers
            .iter()
            .any(|provider| provider.id == provider_id && provider.kind == ProviderKind::GrokSubscription)
            && connection_of(&current, provider_id).generation == generation;
        if !still_valid {
            if let Some(attempt) = auth.view(provider_id, generation).attempt {
                let _ = auth.cancel(provider_id, attempt).await;
            }
            return Err(refusal(
                CODE_LOGIN_SUPERSEDED,
                "the connection changed while the login was starting; the new attempt was aborted",
            ));
        }
    }
    store
        .update(|config| {
            let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
            if connection.generation == generation {
                connection.state = ConnectionState::AuthorizationPending;
            }
        })
        .map_err(|error| helper::redact(&error.to_string()))?;
    Ok(auth.view(provider_id, generation))
}

/// 轮询进行中的登录：只有当前世代与当前 attempt 的结果才会写入连接状态。
/// 不支持的订阅服务商直接拒绝：它的视图会带上 Grok 辅助进程的信息，返回 Ok 等于把两家路线混在一起。
pub async fn poll(store: &ConfigStore, provider_id: &str) -> Result<AuthView, String> {
    let config = store.read();
    let provider = provider_of(&config, provider_id)?;
    require_subscription(&provider)?;
    require_grok(&provider)?;
    let generation = connection_of(&config, provider_id).generation;
    let auth = store.auth.clone();
    let view = auth.view(provider_id, generation);
    let (Some(attempt), AuthPhase::Pending) = (view.attempt, view.phase) else {
        // 没有进行中的登录：保持现状，不写入任何状态。
        return Ok(view);
    };
    let result = auth
        .poll(provider_id, generation, attempt)
        .await
        .map_err(|error| helper::redact(&error))?;
    match result {
        // 迟到的结果：不写入任何状态，也不改视图。
        AuthPoll::Pending | AuthPoll::Superseded => {}
        AuthPoll::Succeeded { identity } => {
            let identity = identity.trim().to_owned();
            if identity.is_empty() {
                return Err(refusal(CODE_IDENTITY_MISSING, "the helper returned an empty account identity"));
            }
            store
                .update(|config| {
                    let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
                    if connection.generation != generation {
                        return;
                    }
                    connection.state = ConnectionState::Connected;
                    connection.identity = Some(identity);
                    // 旧证据因账号/世代不符自然作废：这里不复制、不沿用任何旧证据。
                    connection.evidence = None;
                })
                .map_err(|error| helper::redact(&error.to_string()))?;
        }
        // 失败与取消都归位为未连接；失败原因保留在视图里。
        AuthPoll::Failed { .. } => {
            store
                .update(|config| {
                    if let Some(connection) = config.subscriptions.get_mut(provider_id) {
                        if connection.generation == generation {
                            connection.state = ConnectionState::NotConnected;
                            connection.identity = None;
                        }
                    }
                })
                .map_err(|error| helper::redact(&error.to_string()))?;
        }
        AuthPoll::Cancelled => {
            store
                .update(|config| {
                    if let Some(connection) = config.subscriptions.get_mut(provider_id) {
                        if connection.generation == generation {
                            connection.state = ConnectionState::NotConnected;
                        }
                    }
                })
                .map_err(|error| helper::redact(&error.to_string()))?;
        }
    }
    Ok(auth.view(provider_id, generation))
}

/// 取消进行中的登录：世代不变、连接归位为未连接；此后的迟到轮询一律 Superseded。
/// 不支持的订阅服务商直接拒绝：取消会写连接状态，绝不能在别人的授权路线上生效。
pub async fn cancel(store: &ConfigStore, provider_id: &str) -> Result<AuthView, String> {
    let config = store.read();
    let provider = provider_of(&config, provider_id)?;
    require_subscription(&provider)?;
    require_grok(&provider)?;
    let generation = connection_of(&config, provider_id).generation;
    let auth = store.auth.clone();
    let view = auth.view(provider_id, generation);
    let (Some(attempt), AuthPhase::Pending) = (view.attempt, view.phase) else {
        // 没有进行中的登录（Idle/已成功/已失败/已取消）：与 poll 的早返回一致，
        // 不调用 auth.cancel、不写任何状态——否则会把一个已连接连接改成 NotConnected，
        // 而 identity/evidence/generation 不变，连接进入自相矛盾且无法退出的状态。
        return Ok(view);
    };
    auth.cancel(provider_id, attempt).await.map_err(|error| helper::redact(&error))?;
    // 竞态：poll 可能已经赢了这次登录。只有确实取消成功（会话进入 Cancelled 且 attempt 未变）
    // 才允许归位连接；否则不改写任何状态。
    let cancelled = auth.view(provider_id, generation);
    if !(cancelled.phase == AuthPhase::Cancelled && cancelled.attempt == Some(attempt)) {
        return Ok(cancelled);
    }
    store
        .update(|config| {
            if let Some(connection) = config.subscriptions.get_mut(provider_id) {
                // 与 poll/begin 一致：世代已变或连接已不处于授权中，就什么都不写。
                if connection.generation == generation && connection.state == ConnectionState::AuthorizationPending {
                    connection.state = ConnectionState::NotConnected;
                }
            }
        })
        .map_err(|error| helper::redact(&error.to_string()))?;
    Ok(auth.view(provider_id, generation))
}

/// 退出登录：世代 +1、身份与证据清空、helper home 清空、自有进程回收；provider/model/route 配置全部保留。
/// 未连接时诚实拒绝，绝不把“什么都没做”报成 `cleared`。
/// 不支持的订阅服务商直接拒绝，且不得有任何副作用：不推进世代、不清身份/证据、不写退出证据、不回收路径。
pub async fn logout(store: &ConfigStore, provider_id: &str) -> Result<AuthView, String> {
    let config = store.read();
    let provider = provider_of(&config, provider_id)?;
    require_subscription(&provider)?;
    require_grok(&provider)?;
    if store.auth.isolated() {
        return Err(refusal(CODE_HELPER_ISOLATED, format!("{} authorization is unsupported in isolated validation", provider.name)));
    }
    let connection = connection_of(&config, provider_id);
    if connection.state != ConnectionState::Connected {
        return Err(refusal(
            CODE_NOT_CONNECTED,
            format!("{} has no connected account to log out", provider.name),
        ));
    }
    let generation = connection.generation;
    let auth = store.auth.clone();
    // 先写配置（世代守卫）：失败时直接返回错误，磁盘凭据与专用 home 一个都不动。
    let (applied, next_generation) = store
        .update(|config| -> (bool, u64) {
            let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
            // 与 begin/poll 一致：世代已变就只回报当前世代，不推进、不清身份与证据。
            if connection.generation != generation {
                return (false, connection.generation);
            }
            connection.generation += 1;
            connection.state = ConnectionState::NotConnected;
            connection.identity = None;
            connection.evidence = None;
            (true, connection.generation)
        })
        .map_err(|error| helper::redact(&error.to_string()))?;
    if !applied {
        // 连接在退出过程中被并发登录/换号推进：这次退出什么都没做，
        // 绝不能在此基础上回收进程或删除当前世代的专用 home —— 那会留下「已连接但凭据已没」的状态。
        return Err(refusal(
            CODE_LOGOUT_SUPERSEDED,
            format!("{} authorization changed while signing out; nothing was cleared, try again", provider.name),
        ));
    }
    // 配置已落地为未连接，之后才做不可逆的本地清理：清理失败如实报错，绝不回退成已连接。
    auth.logout(provider_id, generation).await.map_err(|error| helper::redact(&error))?;
    Ok(auth.view(provider_id, next_generation))
}

/// 换号：先退出（新世代、身份与证据清空），再用新世代开始登录。
/// 登录开始失败时保持未连接、身份为空，绝不恢复旧账号身份或证据。
/// 服务商类型不符时直接拒绝：不得先退出、不得推进世代、不得清空身份或证据。
pub async fn switch_account(store: &ConfigStore, provider_id: &str) -> Result<AuthView, String> {
    let config = store.read();
    let provider = provider_of(&config, provider_id)?;
    require_subscription(&provider)?;
    require_grok(&provider)?;
    logout(store, provider_id).await?;
    begin(store, provider_id).await
}

/// 界面视图：覆盖所有订阅服务商，按 provider_id 排序。
/// 只有 Grok 订阅接入了辅助进程；其他订阅服务商不得借用它的可用性与存储目标。
pub fn views(config: &AppConfig, auth: &dyn SubscriptionAuth) -> Vec<AuthView> {
    let mut items: Vec<_> = config
        .providers
        .iter()
        .filter(|provider| is_subscription_provider(provider))
        .map(|provider| {
            let generation = config
                .subscriptions
                .get(&provider.id)
                .map(|connection| connection.generation)
                .unwrap_or(1);
            let mut view = auth.view(&provider.id, generation);
            if provider.kind != ProviderKind::GrokSubscription {
                view.helper = HelperInfo::default();
                view.phase = AuthPhase::Idle;
            }
            view
        })
        .collect();
    items.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
    items
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::subscription::{sync_provider, UnavailableAdapter};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// 只存在于测试的授权替身：脚本化轮询结果 + 内存会话，绝不拉起任何进程。
    /// 它没有生产入口，配置与界面都无法把它装进应用。
    struct StubAuth {
        available: AtomicBool,
        isolated: AtomicBool,
        begin_error: Mutex<Option<String>>,
        scripted: Mutex<HashMap<String, VecDeque<AuthPoll>>>,
        sessions: Mutex<HashMap<String, AuthView>>,
        logouts: Mutex<HashMap<String, LogoutEvidence>>,
        cancelled: Mutex<Vec<(String, u64)>>,
        logout_evidence: Mutex<LogoutEvidence>,
        shutdown_pids: Vec<u32>,
        shutdown_calls: Mutex<u32>,
        /// 供竞态用例把 store 接进来，在某个调用内部推进世代。
        store: Mutex<Option<Arc<ConfigStore>>>,
        bump_generation_on_begin: AtomicBool,
        bump_generation_on_cancel: AtomicBool,
        /// cancel 变成 no-op 时模拟“会话已终结/attempt 不匹配”的竞态。
        cancel_is_noop: AtomicBool,
        /// dispose/rename 的记录与脚本化失败。
        dispose_calls: Mutex<Vec<String>>,
        rename_calls: Mutex<Vec<(String, String)>>,
        dispose_error: Mutex<Option<String>>,
        rename_error: Mutex<Option<String>>,
    }

    impl StubAuth {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                available: AtomicBool::new(true),
                isolated: AtomicBool::new(false),
                begin_error: Mutex::new(None),
                scripted: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                logouts: Mutex::new(HashMap::new()),
                cancelled: Mutex::new(Vec::new()),
                logout_evidence: Mutex::new(LogoutEvidence {
                    local: LocalLogoutState::Cleared,
                    local_detail: Some("fixture local clear".into()),
                    remote: RemoteRevokeState::NotAttempted,
                    remote_detail: Some("fixture".into()),
                }),
                shutdown_pids: vec![4242],
                shutdown_calls: Mutex::new(0),
                store: Mutex::new(None),
                bump_generation_on_begin: AtomicBool::new(false),
                bump_generation_on_cancel: AtomicBool::new(false),
                cancel_is_noop: AtomicBool::new(false),
                dispose_calls: Mutex::new(Vec::new()),
                rename_calls: Mutex::new(Vec::new()),
                dispose_error: Mutex::new(None),
                rename_error: Mutex::new(None),
            })
        }

        fn script(&self, provider_id: &str, result: AuthPoll) {
            self.scripted.lock().unwrap().entry(provider_id.to_owned()).or_default().push_back(result);
        }

        fn session(&self, provider_id: &str) -> Option<AuthView> {
            self.sessions.lock().unwrap().get(provider_id).cloned()
        }

        fn set_logout_evidence(&self, evidence: LogoutEvidence) {
            *self.logout_evidence.lock().unwrap() = evidence;
        }

        /// 模拟“调用进行中世代被推进”：验证生命周期函数的世代守卫。
        fn bump_generation(&self, provider_id: &str) {
            if let Some(store) = self.store.lock().unwrap().clone() {
                store
                    .update(|config| {
                        let connection = config.subscriptions.entry(provider_id.to_owned()).or_default();
                        connection.generation += 1;
                    })
                    .unwrap();
            }
        }
    }

    impl SubscriptionAuth for StubAuth {
        fn available(&self) -> bool {
            self.available.load(Ordering::SeqCst)
        }

        fn isolated(&self) -> bool {
            self.isolated.load(Ordering::SeqCst)
        }

        fn view(&self, provider_id: &str, generation: u64) -> AuthView {
            let mut view = self
                .sessions
                .lock()
                .unwrap()
                .get(provider_id)
                .filter(|session| session.generation == generation)
                .cloned()
                .unwrap_or_default();
            view.provider_id = provider_id.to_owned();
            view.generation = generation;
            view.helper = HelperInfo {
                available: self.available.load(Ordering::SeqCst),
                version: None,
                program: Some("fixture-helper".into()),
                home: Some(format!("/tmp/fixture-helpers/{provider_id}/home")),
            };
            view.logout = self.logouts.lock().unwrap().get(provider_id).cloned().unwrap_or_default();
            view
        }

        fn begin<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<AuthChallenge, String>> {
            Box::pin(async move {
                if let Some(error) = self.begin_error.lock().unwrap().clone() {
                    return Err(error);
                }
                let challenge = AuthChallenge {
                    kind: "device_code".into(),
                    instructions: "Open the fixture URL".into(),
                    verification_url: Some("https://example.invalid/device".into()),
                    user_code: Some("FIXTURE-CODE".into()),
                };
                let mut sessions = self.sessions.lock().unwrap();
                let previous = sessions
                    .get(provider_id)
                    .filter(|session| session.generation == generation)
                    .and_then(|session| session.attempt)
                    .unwrap_or(0);
                sessions.insert(
                    provider_id.to_owned(),
                    AuthView {
                        provider_id: provider_id.to_owned(),
                        phase: AuthPhase::Pending,
                        generation,
                        attempt: Some(previous + 1),
                        challenge: Some(challenge.clone()),
                        ..Default::default()
                    },
                );
                drop(sessions);
                if self.bump_generation_on_begin.load(Ordering::SeqCst) {
                    self.bump_generation(provider_id);
                }
                Ok(challenge)
            })
        }

        fn poll<'a>(&'a self, provider_id: &'a str, generation: u64, attempt: u64) -> BoxFuture<'a, Result<AuthPoll, String>> {
            Box::pin(async move {
                let mut sessions = self.sessions.lock().unwrap();
                let Some(session) = sessions.get_mut(provider_id) else {
                    return Ok(AuthPoll::Superseded);
                };
                if session.generation != generation || session.attempt != Some(attempt) || session.phase != AuthPhase::Pending {
                    return Ok(AuthPoll::Superseded);
                }
                let result = self
                    .scripted
                    .lock()
                    .unwrap()
                    .get_mut(provider_id)
                    .and_then(|queue| queue.pop_front())
                    .unwrap_or(AuthPoll::Pending);
                match &result {
                    AuthPoll::Succeeded { identity } => {
                        session.phase = AuthPhase::Succeeded;
                        session.identity = Some(identity.clone());
                        session.challenge = None;
                        session.error = None;
                    }
                    AuthPoll::Failed { error } => {
                        session.phase = AuthPhase::Failed;
                        session.identity = None;
                        session.challenge = None;
                        session.error = Some(error.clone());
                    }
                    AuthPoll::Cancelled => {
                        session.phase = AuthPhase::Cancelled;
                        session.challenge = None;
                    }
                    _ => {}
                }
                Ok(result)
            })
        }

        fn cancel<'a>(&'a self, provider_id: &'a str, attempt: u64) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.cancelled.lock().unwrap().push((provider_id.to_owned(), attempt));
                // 语义固定：只在 session 仍为 Pending 且 attempt 匹配时生效，否则 no-op。
                if !self.cancel_is_noop.load(Ordering::SeqCst) {
                    if let Some(session) = self.sessions.lock().unwrap().get_mut(provider_id) {
                        if session.phase == AuthPhase::Pending && session.attempt == Some(attempt) {
                            session.phase = AuthPhase::Cancelled;
                            session.challenge = None;
                        }
                    }
                }
                if self.bump_generation_on_cancel.load(Ordering::SeqCst) {
                    self.bump_generation(provider_id);
                }
                Ok(())
            })
        }

        fn logout<'a>(&'a self, provider_id: &'a str, _generation: u64) -> BoxFuture<'a, Result<LogoutEvidence, String>> {
            Box::pin(async move {
                self.sessions.lock().unwrap().remove(provider_id);
                let evidence = self.logout_evidence.lock().unwrap().clone();
                self.logouts.lock().unwrap().insert(provider_id.to_owned(), evidence.clone());
                Ok(evidence)
            })
        }

        fn shutdown(&self) -> Vec<u32> {
            *self.shutdown_calls.lock().unwrap() += 1;
            self.shutdown_pids.clone()
        }

        fn dispose(&self, provider_id: &str) -> Result<(), String> {
            self.dispose_calls.lock().unwrap().push(provider_id.to_owned());
            if let Some(error) = self.dispose_error.lock().unwrap().clone() {
                // 脚本化失败：模拟清理失败，调用方必须据此拒绝继续。
                return Err(error);
            }
            self.sessions.lock().unwrap().remove(provider_id);
            self.logouts.lock().unwrap().remove(provider_id);
            Ok(())
        }

        fn rename(&self, old_id: &str, new_id: &str) -> Result<(), String> {
            self.rename_calls.lock().unwrap().push((old_id.to_owned(), new_id.to_owned()));
            if let Some(error) = self.rename_error.lock().unwrap().clone() {
                return Err(error);
            }
            if old_id == new_id {
                return Ok(());
            }
            {
                let mut sessions = self.sessions.lock().unwrap();
                if let Some(session) = sessions.remove(old_id) {
                    sessions.insert(new_id.to_owned(), session);
                }
            }
            {
                let mut logouts = self.logouts.lock().unwrap();
                if let Some(evidence) = logouts.remove(old_id) {
                    logouts.insert(new_id.to_owned(), evidence);
                }
            }
            Ok(())
        }
    }

    fn fixture_store(auth: Arc<StubAuth>) -> (Arc<ConfigStore>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            ConfigStore::load_with_adapters_and_auth(
                directory.path().join("autojev.db"),
                Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true }),
                Arc::new(UnavailableAdapter),
                auth.clone(),
            )
            .unwrap(),
        );
        // 供竞态用例在替身内部推进世代。
        *auth.store.lock().unwrap() = Some(store.clone());
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
                let mut model = config.models[0].clone();
                model.id = "grok-model".into();
                model.provider_id = "grok".into();
                model.model_id = "grok-4".into();
                config.models.push(model);
                config.routes.push(crate::config::RouteRule {
                    all_models: false,
                    automatic_policy: None,
                    model_settings: Default::default(),
                    id: "fixture-route".into(),
                    name: "Fixture".into(),
                    strategy: "priority".into(),
                    model_ids: vec!["grok-model".into()],
                    enabled: true,
                });
            })
            .unwrap();
        (store, directory)
    }

    fn stored_connection(store: &ConfigStore) -> crate::subscription::Connection {
        stored_connection_of(store, "grok")
    }

    fn stored_connection_of(store: &ConfigStore, provider_id: &str) -> crate::subscription::Connection {
        store.read().subscriptions.get(provider_id).cloned().unwrap()
    }

    /// 在同一份配置里加入一个 Codex 订阅服务商，用来验证两家路线不互相借用。
    fn add_codex_subscription(store: &ConfigStore) {
        store
            .update(|config| {
                let mut provider = config.providers[0].clone();
                provider.id = "codex".into();
                provider.name = "Codex".into();
                provider.kind = ProviderKind::CodexSubscription;
                provider.base_url = String::new();
                config.providers.push(provider.clone());
                sync_provider(config, &provider.id, &provider.kind);
            })
            .unwrap();
    }

    fn connect_provider_account(store: &ConfigStore, provider_id: &str, identity: &str) {
        store
            .update(|config| {
                let connection = config.subscriptions.get_mut(provider_id).unwrap();
                let generation = connection.generation;
                connection.state = ConnectionState::Connected;
                connection.identity = Some(identity.into());
                connection.evidence = Some(crate::subscription::Evidence {
                    generation,
                    account: Some(identity.into()),
                    helper_version: Some("fixture-helper-1.0".into()),
                    account_path: Some("/tmp/fixture-account".into()),
                    models: Vec::new(),
                    capabilities: Vec::new(),
                    quota: Default::default(),
                });
            })
            .unwrap();
    }

    /// 造一个“已核实到可派发程度”的旧账号连接，供退出与换号用例使用。
    fn connect_old_account(store: &ConfigStore) {
        connect_provider_account(store, "grok", "old@example.invalid");
    }

    /// 单个 Grok 订阅服务商的配置装置，授权实现可注入。
    fn subscription_fixture_store(directory: &tempfile::TempDir, auth: Arc<dyn SubscriptionAuth>) -> Arc<ConfigStore> {
        let store = Arc::new(
            ConfigStore::load_with_adapters_and_auth(
                directory.path().join("autojev.db"),
                Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true }),
                Arc::new(UnavailableAdapter),
                auth,
            )
            .unwrap(),
        );
        store
            .update(|config| {
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
        store
    }

    /// 真实 GrokCliAuth 的生命周期装置：专用 home 根就是临时目录，不拉起任何进程。
    fn grok_cli_fixture_store(directory: &tempfile::TempDir) -> (Arc<ConfigStore>, Arc<GrokCliAuth>) {
        let auth = Arc::new(GrokCliAuth::with_test_helper(
            directory.path().to_path_buf(),
            directory.path().join("unused-fixture-helper"),
        ));
        let store = subscription_fixture_store(directory, auth.clone());
        (store, auth)
    }

    /// 在「读配置与写配置之间」推进世代的真实 GrokCliAuth 包装：只服务 logout 竞态用例，
    /// 用来证明世代失效时既不写连接状态、也不调用 auth.logout（进程回收、home 清理、退出证据都不发生）。
    struct SupersedingGrokAuth {
        inner: GrokCliAuth,
        store: Mutex<Option<Arc<ConfigStore>>>,
        logout_calls: Mutex<u32>,
    }

    impl SupersedingGrokAuth {
        fn new(directory: &tempfile::TempDir) -> Arc<Self> {
            Arc::new(Self {
                inner: GrokCliAuth::with_test_helper(
                    directory.path().to_path_buf(),
                    directory.path().join("unused-fixture-helper"),
                ),
                store: Mutex::new(None),
                logout_calls: Mutex::new(0),
            })
        }

        /// 模拟并发登录/换号在生命周期中途推进世代。
        fn bump_generation(&self) {
            if let Some(store) = self.store.lock().unwrap().clone() {
                store
                    .update(|config| {
                        let connection = config.subscriptions.entry("grok".to_owned()).or_default();
                        connection.generation += 1;
                    })
                    .unwrap();
            }
        }
    }

    impl SubscriptionAuth for SupersedingGrokAuth {
        fn available(&self) -> bool {
            self.inner.available()
        }

        fn isolated(&self) -> bool {
            // 生命周期在 store.read() 之后、store.update 之前调用本方法：此处推进世代
            // 正好模拟「读配置与写配置之间被并发写入抢先」。
            self.bump_generation();
            self.inner.isolated()
        }

        fn view(&self, provider_id: &str, generation: u64) -> AuthView {
            self.inner.view(provider_id, generation)
        }

        fn begin<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<AuthChallenge, String>> {
            self.inner.begin(provider_id, generation)
        }

        fn poll<'a>(&'a self, provider_id: &'a str, generation: u64, attempt: u64) -> BoxFuture<'a, Result<AuthPoll, String>> {
            self.inner.poll(provider_id, generation, attempt)
        }

        fn cancel<'a>(&'a self, provider_id: &'a str, attempt: u64) -> BoxFuture<'a, Result<(), String>> {
            self.inner.cancel(provider_id, attempt)
        }

        fn logout<'a>(&'a self, provider_id: &'a str, generation: u64) -> BoxFuture<'a, Result<LogoutEvidence, String>> {
            *self.logout_calls.lock().unwrap() += 1;
            self.inner.logout(provider_id, generation)
        }

        fn shutdown(&self) -> Vec<u32> {
            self.inner.shutdown()
        }

        fn dispose(&self, provider_id: &str) -> Result<(), String> {
            self.inner.dispose(provider_id)
        }

        fn rename(&self, old_id: &str, new_id: &str) -> Result<(), String> {
            self.inner.rename(old_id, new_id)
        }
    }

    /// 竞态用例装置：世代会在「读配置与写配置之间」被推进。
    fn superseding_store(directory: &tempfile::TempDir) -> (Arc<ConfigStore>, Arc<SupersedingGrokAuth>) {
        let auth = SupersedingGrokAuth::new(directory);
        let store = subscription_fixture_store(directory, auth.clone());
        *auth.store.lock().unwrap() = Some(store.clone());
        (store, auth)
    }

    fn configuration_ids(config: &AppConfig) -> (Vec<String>, Vec<String>, Vec<String>) {
        (
            config.providers.iter().map(|provider| provider.id.clone()).collect(),
            config.models.iter().map(|model| model.id.clone()).collect(),
            config.routes.iter().map(|route| route.id.clone()).collect(),
        )
    }

    #[tokio::test]
    async fn begin_only_accepts_subscription_providers_that_are_not_connected() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        // API 服务商不开放登录。
        let error = begin(&store, "openrouter").await.unwrap_err();
        assert!(error.contains(CODE_NOT_SUBSCRIPTION), "{error}");
        connect_old_account(&store);
        let error = begin(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_ALREADY_CONNECTED), "{error}");
        // 拒绝不改变连接，也不动世代。
        let connection = stored_connection(&store);
        assert_eq!(connection.state, ConnectionState::Connected);
        assert_eq!(connection.generation, 1);
        assert_eq!(connection.identity.as_deref(), Some("old@example.invalid"));
    }

    #[tokio::test]
    async fn begin_marks_pending_and_keeps_the_generation_with_a_monotonic_attempt() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        let view = begin(&store, "grok").await.unwrap();
        assert_eq!(view.challenge.as_ref().and_then(|challenge| challenge.user_code.as_deref()), Some("FIXTURE-CODE"));
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 1, "登录不动世代");
        assert_eq!(connection.state, ConnectionState::AuthorizationPending);
        assert_eq!(connection.identity, None);
        assert_eq!(view.phase, AuthPhase::Pending);
        assert_eq!(view.attempt, Some(1));
        assert_eq!(view.challenge.unwrap().verification_url.as_deref(), Some("https://example.invalid/device"));
        // 同一世代内 attempt 单调递增。
        begin(&store, "grok").await.unwrap();
        assert_eq!(views(&store.read(), &*auth).pop().unwrap().attempt, Some(2));
    }

    #[tokio::test]
    async fn begin_refuses_isolated_and_missing_helpers_with_stable_codes() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        auth.isolated.store(true, Ordering::SeqCst);
        let error = begin(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_HELPER_ISOLATED) && error.contains("isolated"), "{error}");
        assert_eq!(stored_connection(&store).state, ConnectionState::NotConnected);
        auth.isolated.store(false, Ordering::SeqCst);
        auth.available.store(false, Ordering::SeqCst);
        let error = begin(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_HELPER_MISSING) && error.contains("helper"), "{error}");
        assert_eq!(stored_connection(&store).state, ConnectionState::NotConnected);
    }

    #[tokio::test]
    async fn poll_confirms_the_identity_and_drops_the_previous_evidence() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        connect_old_account(&store);
        // 先回到未连接并留下旧证据，模拟“退出后重新登录”。
        store
            .update(|config| {
                let connection = config.subscriptions.get_mut("grok").unwrap();
                connection.state = ConnectionState::NotConnected;
                connection.identity = None;
            })
            .unwrap();
        begin(&store, "grok").await.unwrap();
        auth.script("grok", AuthPoll::Succeeded { identity: "new@example.invalid".into() });
        let view = poll(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Succeeded);
        assert_eq!(view.identity.as_deref(), Some("new@example.invalid"));
        assert!(view.error.is_none());
        let connection = stored_connection(&store);
        assert_eq!(connection.state, ConnectionState::Connected);
        assert_eq!(connection.identity.as_deref(), Some("new@example.invalid"));
        // 证据整体清空：旧账号的证据绝不会被沿用成新账号已核实。
        assert!(connection.evidence.is_none());
        assert!(connection.current_evidence().is_none());
    }

    #[tokio::test]
    async fn poll_failure_and_cancellation_return_to_not_connected_without_losing_the_reason() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        begin(&store, "grok").await.unwrap();
        auth.script("grok", AuthPoll::Failed { error: auth_error("helper_error", "the fixture login failed", "Try again.") });
        let view = poll(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Failed);
        assert_eq!(view.error.as_ref().map(|error| error.code.as_str()), Some("helper_error"));
        let connection = stored_connection(&store);
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert_eq!(connection.identity, None);
        // 取消：世代不变、挑战清空、连接归位为未连接。
        begin(&store, "grok").await.unwrap();
        let view = cancel(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Cancelled);
        assert!(view.challenge.is_none());
        let connection = stored_connection(&store);
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert_eq!(connection.generation, 1, "取消不动世代");
        assert_eq!(auth.cancelled.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_late_result_after_cancellation_never_writes_any_state() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        begin(&store, "grok").await.unwrap();
        let attempt = views(&store.read(), &*auth).pop().unwrap().attempt.unwrap();
        cancel(&store, "grok").await.unwrap();
        // 辅助进程其实已经成功了：迟到的结果必须被丢弃。
        auth.script("grok", AuthPoll::Succeeded { identity: "late@example.invalid".into() });
        let view = poll(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Cancelled);
        assert!(view.identity.is_none());
        assert_eq!(auth.session("grok").unwrap().phase, AuthPhase::Cancelled);
        let connection = stored_connection(&store);
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert!(connection.identity.is_none());
        assert!(connection.evidence.is_none());
        // 旧 attempt 直接问替身也一律 Superseded。
        let auth_ref: &dyn SubscriptionAuth = &*auth;
        assert_eq!(auth_ref.poll("grok", 1, attempt).await.unwrap(), AuthPoll::Superseded);
        assert_eq!(auth_ref.poll("grok", 1, attempt + 1).await.unwrap(), AuthPoll::Superseded);
    }

    #[tokio::test]
    async fn a_generation_change_supersedes_the_pending_login_without_writing() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        begin(&store, "grok").await.unwrap();
        auth.script("grok", AuthPoll::Succeeded { identity: "late@example.invalid".into() });
        // 登录期间世代被推进（例如别处退出/换号）：本次登录整体作废。
        store
            .update(|config| {
                let connection = config.subscriptions.get_mut("grok").unwrap();
                connection.generation += 1;
                connection.state = ConnectionState::NotConnected;
            })
            .unwrap();
        let view = poll(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Idle);
        assert!(view.identity.is_none());
        let connection = stored_connection(&store);
        assert_eq!(connection.identity, None);
        assert!(connection.evidence.is_none());
        // 新世代下的会话已经不存在；直接问替身也是 Superseded。
        let auth_ref: &dyn SubscriptionAuth = &*auth;
        assert_eq!(auth_ref.poll("grok", 2, 1).await.unwrap(), AuthPoll::Superseded);
    }

    #[tokio::test]
    async fn logout_bumps_the_generation_clears_identity_and_evidence_and_keeps_configuration() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        connect_old_account(&store);
        let before = configuration_ids(&store.read());
        let view = logout(&store, "grok").await.unwrap();
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 2);
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert!(connection.identity.is_none());
        assert!(connection.evidence.is_none());
        assert_eq!(view.generation, 2);
        assert_eq!(view.phase, AuthPhase::Idle);
        assert_eq!(view.logout.local, LocalLogoutState::Cleared);
        // 本地清除不代表远端已撤销。
        assert_eq!(view.logout.remote, RemoteRevokeState::NotAttempted);
        assert_ne!(view.logout.remote, RemoteRevokeState::Verified);
        assert_eq!(configuration_ids(&store.read()), before, "provider/model/route 配置必须保留");
    }

    /// B4：退出必须先写配置（世代 +1、未连接、身份/证据清空），成功后才做不可逆的本地清理。
    #[tokio::test]
    async fn logout_clears_the_helper_home_only_after_the_config_write_succeeds() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _auth) = grok_cli_fixture_store(&directory);
        connect_old_account(&store);
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        helper::prepare_home(&home).unwrap();
        std::fs::write(home.join("session"), "fixture credential").unwrap();

        let view = logout(&store, "grok").await.unwrap();
        assert_eq!(view.generation, 2);
        assert_eq!(view.logout.local, LocalLogoutState::Cleared);
        assert_eq!(view.logout.remote, RemoteRevokeState::NotAttempted);
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 2, "世代必须 +1");
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert!(connection.identity.is_none() && connection.evidence.is_none());
        assert!(home.is_dir() && std::fs::read_dir(&home).unwrap().count() == 0, "配置写成功后 home 必须被清空");
        assert!(!home.join("session").exists());
    }

    /// B4：配置写失败时任何凭据都不得销毁（旧实现会先删 home 再写库，留下已连接但无凭据的配置）。
    #[tokio::test]
    async fn logout_keeps_the_helper_home_when_the_config_write_fails() {
        let directory = tempfile::tempdir().unwrap();
        let (store, auth) = grok_cli_fixture_store(&directory);
        connect_old_account(&store);
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        helper::prepare_home(&home).unwrap();
        std::fs::write(home.join("session"), "fixture credential").unwrap();

        // 让配置写入必然失败（内存态不变），与 config 模块既有用例同一手法。
        rusqlite::Connection::open(directory.path().join("autojev.db"))
            .unwrap()
            .execute_batch("DROP TABLE app_meta;")
            .unwrap();

        let error = logout(&store, "grok").await.unwrap_err();
        assert!(!error.is_empty());
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 1, "写配置失败不得推进世代");
        assert_eq!(connection.state, ConnectionState::Connected);
        assert_eq!(connection.identity.as_deref(), Some("old@example.invalid"));
        assert!(connection.evidence.is_some());
        assert!(home.join("session").is_file(), "写配置失败时不得清理专用 home");
        assert!(
            !matches!(auth.view("grok", 1).logout.local, LocalLogoutState::Cleared),
            "未执行的清理不得被写成证据"
        );
    }

    #[tokio::test]
    async fn logout_reports_a_local_failure_without_claiming_remote_revocation() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        connect_old_account(&store);
        auth.set_logout_evidence(LogoutEvidence {
            local: LocalLogoutState::Failed,
            local_detail: Some("fixture cleanup failure".into()),
            remote: RemoteRevokeState::NotAttempted,
            remote_detail: Some("fixture".into()),
        });
        let view = logout(&store, "grok").await.unwrap();
        assert_eq!(view.logout.local, LocalLogoutState::Failed);
        assert_eq!(view.logout.remote, RemoteRevokeState::NotAttempted);
        assert_eq!(stored_connection(&store).generation, 2, "本地清除失败也要推进世代，旧世代必须失效");
    }

    #[tokio::test]
    async fn logout_is_honestly_refused_when_not_connected_or_isolated() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        let error = logout(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_NOT_CONNECTED), "{error}");
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 1);
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert_eq!(views(&store.read(), &*auth).pop().unwrap().logout.local, LocalLogoutState::NotAttempted);
        // 隔离环境同样拒绝：不触碰世代、身份与证据。
        connect_old_account(&store);
        auth.isolated.store(true, Ordering::SeqCst);
        let error = logout(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_HELPER_ISOLATED) && error.contains("isolated"), "{error}");
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 1);
        assert_eq!(connection.identity.as_deref(), Some("old@example.invalid"));
        assert_eq!(views(&store.read(), &*auth).pop().unwrap().logout.local, LocalLogoutState::NotAttempted);
    }

    #[tokio::test]
    async fn a_switch_account_uses_a_new_generation_and_keeps_other_configuration() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        connect_old_account(&store);
        let before = configuration_ids(&store.read());
        let view = switch_account(&store, "grok").await.unwrap();
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 2);
        assert_eq!(connection.state, ConnectionState::AuthorizationPending);
        assert!(connection.identity.is_none());
        assert!(connection.evidence.is_none());
        assert_eq!(view.phase, AuthPhase::Pending);
        assert_eq!(view.attempt, Some(1), "新世代重新从 1 开始");
        assert_eq!(configuration_ids(&store.read()), before);
    }

    #[tokio::test]
    async fn a_failed_switch_account_never_restores_the_old_account() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        connect_old_account(&store);
        *auth.begin_error.lock().unwrap() = Some(format!("{CODE_HELPER_MISSING}: no fixture helper"));
        let error = switch_account(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_HELPER_MISSING), "{error}");
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 2, "换号已经推进世代");
        assert_eq!(connection.state, ConnectionState::NotConnected);
        assert!(connection.identity.is_none(), "不得恢复旧账号身份");
        assert!(connection.evidence.is_none(), "不得恢复旧账号证据");
        assert!(connection.current_evidence().is_none());
    }

    #[tokio::test]
    async fn shutdown_reclaims_only_owned_processes_and_leaves_the_configuration_alone() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        connect_old_account(&store);
        let before = configuration_ids(&store.read());
        assert_eq!(store.auth.shutdown(), vec![4242]);
        assert_eq!(*auth.shutdown_calls.lock().unwrap(), 1);
        assert_eq!(configuration_ids(&store.read()), before);
        assert_eq!(stored_connection(&store).identity.as_deref(), Some("old@example.invalid"));
    }

    #[tokio::test]
    async fn auth_views_cover_all_subscription_providers_sorted_and_carry_no_credentials() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        store
            .update(|config| {
                let mut provider = config.providers[0].clone();
                provider.id = "codex".into();
                provider.name = "Codex".into();
                provider.kind = ProviderKind::CodexSubscription;
                provider.base_url = String::new();
                config.providers.push(provider.clone());
                sync_provider(config, &provider.id, &provider.kind);
            })
            .unwrap();
        let items = views(&store.read(), &*auth);
        let ids: Vec<String> = items.iter().map(|view| view.provider_id.clone()).collect();
        assert_eq!(ids, vec!["codex".to_owned(), "grok".to_owned()]);
        assert!(items.iter().all(|view| view.phase == AuthPhase::Idle));
        assert!(items.iter().all(|view| view.logout.remote == RemoteRevokeState::NotAttempted));

        // 失败原因里混入凭据：视图与序列化结果都不得把凭据带出去。
        let secret = format!("xai-{}", "0123456789abcdef".repeat(3));
        begin(&store, "grok").await.unwrap();
        auth.script(
            "grok",
            AuthPoll::Failed { error: auth_error("helper_error", format!("failed with {secret}"), "Try again.") },
        );
        let view = poll(&store, "grok").await.unwrap();
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains(&secret), "credentials must never reach the view");
        assert!(json.contains("[redacted]"), "{json}");
    }

    /// Codex 订阅没有接入本票的辅助进程：登录必须诚实拒绝，且不能碰任何连接状态。
    #[tokio::test]
    async fn a_codex_subscription_login_is_refused_without_touching_the_connection() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        add_codex_subscription(&store);
        connect_provider_account(&store, "codex", "codex@example.invalid");
        let before = stored_connection_of(&store, "codex");
        // 已连接也不能借用 Grok 路线：helper_unsupported 优先于 already_connected。
        let error = begin(&store, "codex").await.unwrap_err();
        assert!(error.contains(CODE_HELPER_UNSUPPORTED), "{error}");
        let after = stored_connection_of(&store, "codex");
        assert_eq!(after.state, ConnectionState::Connected);
        assert_eq!(after.generation, before.generation, "世代不得改变");
        assert_eq!(after.identity.as_deref(), Some("codex@example.invalid"));
        assert_eq!(after.evidence, before.evidence);
        // Grok 订阅不受影响，路线仍然可用。
        begin(&store, "grok").await.unwrap();
    }

    /// 换号在不支持的服务商上必须整段拒绝：不得先退出，也不得清空旧账号。
    #[tokio::test]
    async fn a_codex_subscription_switch_account_keeps_the_old_account() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        add_codex_subscription(&store);
        connect_provider_account(&store, "codex", "codex@example.invalid");
        let before = stored_connection_of(&store, "codex");
        let error = switch_account(&store, "codex").await.unwrap_err();
        assert!(error.contains(CODE_HELPER_UNSUPPORTED), "{error}");
        let after = stored_connection_of(&store, "codex");
        assert_eq!(after.generation, before.generation, "换号不得推进世代");
        assert_eq!(after.state, ConnectionState::Connected);
        assert_eq!(after.identity.as_deref(), Some("codex@example.invalid"), "身份不得被清空");
        assert_eq!(after.evidence, before.evidence, "证据不得被清空");
        assert!(auth.logouts.lock().unwrap().is_empty(), "kind 不符时不得执行退出");
    }

    /// 不支持的订阅服务商退出必须整段拒绝：无世代推进、无身份/证据清空、无退出证据。
    #[tokio::test]
    async fn a_codex_subscription_logout_is_refused_without_touching_the_connection() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        add_codex_subscription(&store);
        connect_provider_account(&store, "codex", "codex@example.invalid");
        let before = stored_connection_of(&store, "codex");
        let error = logout(&store, "codex").await.unwrap_err();
        assert!(error.contains(CODE_HELPER_UNSUPPORTED), "{error}");
        // 无任何副作用：世代、状态、身份、证据全部保持原样。
        let after = stored_connection_of(&store, "codex");
        assert_eq!(after.generation, before.generation, "退出不得推进世代");
        assert_eq!(after.state, ConnectionState::Connected);
        assert_eq!(after.identity.as_deref(), Some("codex@example.invalid"));
        assert_eq!(after.evidence, before.evidence);
        // 绝不写退出证据，更不能把未执行的本地清除报成 cleared。
        assert!(auth.logouts.lock().unwrap().is_empty(), "kind 不符时不得记录退出证据");
        let view = views(&store.read(), &*auth).into_iter().find(|view| view.provider_id == "codex").unwrap();
        assert_eq!(view.logout.local, LocalLogoutState::NotAttempted);
        assert_eq!(view.logout.remote, RemoteRevokeState::NotAttempted);
    }

    /// 不支持的订阅服务商上取消与轮询同样拒绝：不得写连接状态，也不得以成功语义返回。
    #[tokio::test]
    async fn a_codex_subscription_cancel_and_poll_never_write_state_or_report_success() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        add_codex_subscription(&store);
        connect_provider_account(&store, "codex", "codex@example.invalid");
        let before = stored_connection_of(&store, "codex");
        let cancel_error = cancel(&store, "codex").await.unwrap_err();
        assert!(cancel_error.contains(CODE_HELPER_UNSUPPORTED), "{cancel_error}");
        let poll_error = poll(&store, "codex").await.unwrap_err();
        assert!(poll_error.contains(CODE_HELPER_UNSUPPORTED), "{poll_error}");
        let after = stored_connection_of(&store, "codex");
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.state, ConnectionState::Connected);
        assert_eq!(after.identity.as_deref(), Some("codex@example.invalid"));
        assert_eq!(after.evidence, before.evidence);
        assert!(auth.cancelled.lock().unwrap().is_empty(), "kind 不符时不得执行取消");
    }

    /// 已连接的连接上调用取消必须是纯粹的 no-op：不得把状态改成未连接。
    #[tokio::test]
    async fn cancel_on_a_connected_connection_does_not_write_any_state() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        connect_old_account(&store);
        let before = stored_connection(&store);
        let view = cancel(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Idle);
        assert!(view.attempt.is_none());
        let after = stored_connection(&store);
        assert_eq!(after.state, ConnectionState::Connected, "没有进行中的登录时取消不得改状态");
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.identity.as_deref(), Some("old@example.invalid"));
        assert_eq!(after.evidence, before.evidence);
        assert!(auth.cancelled.lock().unwrap().is_empty(), "没有进行中的登录时不得执行取消");
        // 状态没被改坏，因此退出仍然可用。
        logout(&store, "grok").await.unwrap();
        assert_eq!(stored_connection(&store).generation, 2);
    }

    /// 未连接（Idle）的连接上调用取消同样不写任何状态。
    #[tokio::test]
    async fn cancel_on_an_idle_connection_does_not_write_any_state() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        let before = stored_connection(&store);
        let view = cancel(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Idle);
        let after = stored_connection(&store);
        assert_eq!(after.state, ConnectionState::NotConnected);
        assert_eq!(after.generation, before.generation);
        assert!(after.identity.is_none() && after.evidence.is_none());
        assert!(auth.cancelled.lock().unwrap().is_empty(), "没有进行中的登录时不得执行取消");
    }

    /// M3：poll 已经赢下登录时，cancel 不得改写连接，也不得调用取消。
    #[tokio::test]
    async fn cancel_does_not_rewrite_a_connection_whose_login_already_finished() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        begin(&store, "grok").await.unwrap();
        // 真实竞态窗口：会话已经成功，但连接状态还停在授权中。
        auth.script("grok", AuthPoll::Succeeded { identity: "winner@example.invalid".into() });
        assert!(matches!(auth.poll("grok", 1, 1).await.unwrap(), AuthPoll::Succeeded { .. }));
        let before = stored_connection(&store);
        assert_eq!(before.state, ConnectionState::AuthorizationPending);
        let view = cancel(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Succeeded);
        let after = stored_connection(&store);
        assert_eq!(after.state, ConnectionState::AuthorizationPending, "已完成的登录不得被取消改写");
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.identity, before.identity);
        assert!(auth.cancelled.lock().unwrap().is_empty(), "会话已终结时不得调用 cancel");
    }

    /// M3：auth.cancel 是 no-op 时，生命周期 cancel 不得写任何状态，后续轮询仍可完成。
    #[tokio::test]
    async fn a_noop_cancel_never_writes_connection_state() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        begin(&store, "grok").await.unwrap();
        auth.cancel_is_noop.store(true, Ordering::SeqCst);
        let view = cancel(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Pending, "no-op 取消不得把会话标成 Cancelled");
        let after = stored_connection(&store);
        assert_eq!(after.state, ConnectionState::AuthorizationPending, "no-op 取消不得改写连接");
        assert_eq!(after.generation, 1);
        auth.script("grok", AuthPoll::Succeeded { identity: "later@example.invalid".into() });
        assert_eq!(poll(&store, "grok").await.unwrap().phase, AuthPhase::Succeeded);
        assert_eq!(stored_connection(&store).state, ConnectionState::Connected);
    }

    /// M3：attempt 不匹配时取消是纯 no-op。
    #[tokio::test]
    async fn cancel_with_a_mismatched_attempt_is_a_noop() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        begin(&store, "grok").await.unwrap();
        begin(&store, "grok").await.unwrap();
        assert_eq!(auth.view("grok", 1).attempt, Some(2));
        auth.cancel("grok", 1).await.unwrap();
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Pending, "错 attempt 不得取消会话");
        assert_eq!(auth.view("grok", 1).attempt, Some(2));
    }

    /// 替身自身：dispose/rename 会记录调用，并可脚本化失败供 teardown 用例断言。
    #[test]
    fn the_stub_auth_records_and_scripts_dispose_and_rename() {
        let auth = StubAuth::new();
        auth.dispose("grok").unwrap();
        auth.rename("grok", "grok-2").unwrap();
        assert_eq!(*auth.dispose_calls.lock().unwrap(), vec!["grok".to_owned()]);
        assert_eq!(*auth.rename_calls.lock().unwrap(), vec![("grok".to_owned(), "grok-2".to_owned())]);
        *auth.dispose_error.lock().unwrap() = Some("fixture dispose failure".into());
        assert!(auth.dispose("grok").is_err());
        *auth.rename_error.lock().unwrap() = Some("fixture rename failure".into());
        assert!(auth.rename("grok", "grok-3").is_err());
    }

    /// 取消写库带世代守卫：调用期间世代已变则什么都不写（与 begin/poll 一致）。
    #[tokio::test]
    async fn cancel_does_not_write_state_when_the_generation_changed_mid_flight() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        begin(&store, "grok").await.unwrap();
        assert_eq!(stored_connection(&store).state, ConnectionState::AuthorizationPending);
        auth.bump_generation_on_cancel.store(true, Ordering::SeqCst);
        let view = cancel(&store, "grok").await.unwrap();
        assert_eq!(view.phase, AuthPhase::Cancelled);
        let after = stored_connection(&store);
        assert_eq!(after.generation, 2, "替身在调用中推进了世代");
        assert_eq!(after.state, ConnectionState::AuthorizationPending, "世代已变则取消不得写状态");
    }

    /// 退出写库带世代守卫：调用期间世代已被并发写入推进时，连接状态、身份证据与专用 home 都不得被清。
    #[tokio::test]
    async fn logout_does_not_clear_identity_when_the_generation_changed_mid_flight() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _auth) = superseding_store(&directory);
        connect_old_account(&store);
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        helper::prepare_home(&home).unwrap();
        std::fs::write(home.join("session"), "current-generation credential").unwrap();

        let error = logout(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_LOGOUT_SUPERSEDED), "{error}");
        let after = stored_connection(&store);
        assert_eq!(after.generation, 2, "并发写入在调用中推进了世代");
        assert_eq!(after.state, ConnectionState::Connected, "世代已变则退出不得清状态");
        assert_eq!(after.identity.as_deref(), Some("old@example.invalid"));
        assert!(after.evidence.is_some(), "世代已变则退出不得清证据");
        assert!(home.join("session").is_file(), "世代已变则退出不得清理专用 home");
    }

    /// Bugbot High：世代失效的退出必须什么都不销毁（不调用 auth.logout、home 原样、连接原样）并诚实报错。
    #[tokio::test]
    async fn a_superseded_logout_never_destroys_the_current_generation() {
        let directory = tempfile::tempdir().unwrap();
        let (store, auth) = superseding_store(&directory);
        connect_old_account(&store);
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        helper::prepare_home(&home).unwrap();
        std::fs::write(home.join("session"), "new-generation credential").unwrap();

        let error = logout(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_LOGOUT_SUPERSEDED), "必须返回稳定 code：{error}");
        assert_eq!(*auth.logout_calls.lock().unwrap(), 0, "世代失效时不得调用 auth.logout（进程回收/清理/退出证据都不发生）");
        assert!(home.join("session").is_file(), "不得删除当前世代的凭据文件");
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 1, "专用 home 必须原样保留");
        let connection = stored_connection(&store);
        assert_eq!(connection.generation, 2);
        assert_eq!(connection.state, ConnectionState::Connected);
        assert_eq!(connection.identity.as_deref(), Some("old@example.invalid"));
        assert!(connection.evidence.is_some());
    }

    /// begin 世代竞态：启动期间世代变化时，新尝试必须中止回收，且不写任何状态。
    #[tokio::test]
    async fn a_begin_that_loses_the_generation_race_aborts_and_reclaims_the_attempt() {
        let auth = StubAuth::new();
        let (store, _directory) = fixture_store(auth.clone());
        auth.bump_generation_on_begin.store(true, Ordering::SeqCst);
        let error = begin(&store, "grok").await.unwrap_err();
        assert!(error.contains(CODE_LOGIN_SUPERSEDED), "{error}");
        let after = stored_connection(&store);
        assert_eq!(after.generation, 2);
        assert_eq!(after.state, ConnectionState::NotConnected, "竞态不得写入授权中");
        assert!(after.identity.is_none());
        // 刚拉起的尝试被回收：替身收到 cancel(provider, 1)。
        assert_eq!(auth.cancelled.lock().unwrap().clone(), vec![("grok".to_owned(), 1)]);
    }

    /// 只有 Grok 订阅能看到 Grok 辅助进程的可用性与存储目标。
    #[test]
    fn views_do_not_lend_the_grok_helper_to_other_subscription_providers() {
        let directory = tempfile::tempdir().unwrap();
        let script = fake_helper(&directory, "fake-grok.sh", "exit 0");
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script.clone());
        let mut config = AppConfig::default();
        for (id, name, kind) in [
            ("codex-subscription", "Codex subscription", ProviderKind::CodexSubscription),
            ("grok", "Grok", ProviderKind::GrokSubscription),
        ] {
            let mut provider = config.providers[0].clone();
            provider.id = id.into();
            provider.name = name.into();
            provider.kind = kind.clone();
            provider.base_url = String::new();
            config.providers.push(provider.clone());
            crate::subscription::sync_provider(&mut config, &provider.id, &provider.kind);
        }
        let items = views(&config, &auth);
        assert_eq!(items.len(), 2);
        let codex = items.iter().find(|view| view.provider_id == "codex-subscription").unwrap();
        assert_eq!(codex.phase, AuthPhase::Idle);
        assert_eq!(codex.helper, HelperInfo::default(), "不得借用 Grok 的可用性与存储目标");
        assert!(!codex.helper.available && codex.helper.program.is_none() && codex.helper.home.is_none());
        assert!(codex.attempt.is_none() && codex.challenge.is_none() && codex.identity.is_none());
        let grok = items.iter().find(|view| view.provider_id == "grok").unwrap();
        assert!(grok.helper.available, "Grok 订阅仍要报告真实探测结果");
        assert_eq!(grok.helper.program.as_deref(), script.to_str());
        assert!(grok.helper.home.as_deref().unwrap().ends_with("grok_subscription/grok/home"));
    }

    /// 前端契约是逐字对应的：这里把字段名与枚举取值锁死，改名会直接让测试失败。
    #[test]
    fn the_auth_view_shape_matches_the_frontend_contract() {
        let view = AuthView {
            provider_id: "grok".into(),
            phase: AuthPhase::Pending,
            generation: 3,
            attempt: Some(2),
            challenge: Some(AuthChallenge {
                kind: "device_code".into(),
                instructions: "Open".into(),
                verification_url: Some("https://example.invalid/device".into()),
                user_code: Some("ABCD".into()),
            }),
            identity: None,
            error: Some(auth_error("helper_error", "failed", "Retry.")),
            helper: HelperInfo {
                available: false,
                version: None,
                program: Some("grok".into()),
                home: Some("/tmp/fixture/home".into()),
            },
            logout: LogoutEvidence {
                local: LocalLogoutState::Failed,
                local_detail: None,
                remote: RemoteRevokeState::Unsupported,
                remote_detail: None,
            },
        };
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["phase"], "pending");
        assert_eq!(json["attempt"], 2);
        assert_eq!(json["generation"], 3);
        assert_eq!(json["challenge"]["verification_url"], "https://example.invalid/device");
        assert_eq!(json["challenge"]["user_code"], "ABCD");
        assert_eq!(json["error"]["code"], "helper_error");
        assert_eq!(json["helper"]["available"], false);
        assert_eq!(json["logout"]["local"], "failed");
        assert_eq!(json["logout"]["remote"], "unsupported");
        assert_eq!(json.as_object().unwrap().len(), 9);
    }

    fn fake_helper(directory: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let script = directory.path().join(name);
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    /// 真实拉起路径：本地假 helper 脚本，不联网、不装 CLI、不碰真实凭据。
    #[tokio::test]
    async fn grok_cli_auth_drives_a_local_fake_helper_through_the_real_spawn_path() {
        let directory = tempfile::tempdir().unwrap();
        let script = fake_helper(
            &directory,
            "fake-grok.sh",
            "printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Open the fixture URL\",\"verification_url\":\"https://example.invalid/device\",\"user_code\":\"ABCD-1234\"}'\nprintf '%s\\n' '{\"event\":\"identity\",\"identity\":\"fixture@example.invalid\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script.clone());
        assert!(auth.available());
        let challenge = auth.begin("grok", 1).await.unwrap();
        assert_eq!(challenge.user_code.as_deref(), Some("ABCD-1234"));
        assert_eq!(challenge.verification_url.as_deref(), Some("https://example.invalid/device"));
        let view = auth.view("grok", 1);
        assert_eq!(view.phase, AuthPhase::Pending);
        assert_eq!(view.attempt, Some(1));
        assert_eq!(view.helper.program.as_deref(), script.to_str());
        // 探测不拉起进程读版本，因此版本必须保持未知，不得编造。
        assert!(view.helper.version.is_none());
        let helper_home = view.helper.home.clone().unwrap();
        assert!(helper_home.starts_with(&directory.path().display().to_string()));
        assert!(helper_home.ends_with("grok_subscription/grok/home"));
        // stdout 是逐行异步到达的：轮询到终态为止。
        let mut poll = AuthPoll::Pending;
        for _ in 0..200 {
            poll = auth.poll("grok", 1, 1).await.unwrap();
            if poll != AuthPoll::Pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(poll, AuthPoll::Succeeded { identity: "fixture@example.invalid".into() });
        let view = auth.view("grok", 1);
        assert_eq!(view.phase, AuthPhase::Succeeded);
        assert_eq!(view.identity.as_deref(), Some("fixture@example.invalid"));
        assert!(view.challenge.is_none());
        // 登录结束即回收自己的进程，并且视图里没有凭据字样。
        assert!(auth.shutdown().is_empty(), "登录结束即回收自己的进程");
        assert!(!serde_json::to_string(&view).unwrap().contains("token"));
    }

    /// AC4：被取消的登录尝试必须清空应用自有 home 内容。
    #[tokio::test]
    async fn grok_cli_auth_cancel_clears_the_provider_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let script = fake_helper(
            &directory,
            "marker-grok.sh",
            "printf '%s' 'fixture' > \"$GROK_HOME/session\"\nprintf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 1).await.unwrap();
        assert!(home.join("session").is_file(), "假 helper 必须先写入应用自有 home");
        auth.cancel("grok", 1).await.unwrap();
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "取消必须清空应用自有 home");
        assert!(home.is_dir(), "只清空内容，保留目录本身");
        assert!(auth.shutdown().is_empty());
    }

    /// AC4：失败的轮询同样必须清空应用自有 home 内容。
    #[tokio::test]
    async fn grok_cli_auth_failed_poll_clears_the_provider_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let script = fake_helper(
            &directory,
            "marker-failing-grok.sh",
            "printf '%s' 'fixture' > \"$GROK_HOME/session\"\nprintf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'\nprintf '%s\\n' '{\"event\":\"error\",\"code\":\"login_failed\",\"message\":\"fixture rejection\"}'",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 1).await.unwrap();
        assert!(home.join("session").is_file());
        let mut poll = AuthPoll::Pending;
        for _ in 0..200 {
            poll = auth.poll("grok", 1, 1).await.unwrap();
            if poll != AuthPoll::Pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(matches!(poll, AuthPoll::Failed { .. }), "expected a failure, got {poll:?}");
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "失败的尝试必须清空应用自有 home");
        assert!(auth.shutdown().is_empty());
    }

    /// 新登录必须作废上一世代的退出证据。
    #[tokio::test]
    async fn grok_cli_auth_begin_drops_the_previous_logout_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let script = fake_helper(
            &directory,
            "fake-grok.sh",
            "printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Open\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        // 先制造一次退出证据：本地已清除。
        let evidence = auth.logout("grok", 1).await.unwrap();
        assert_eq!(evidence.local, LocalLogoutState::Cleared);
        assert_eq!(auth.view("grok", 1).logout.local, LocalLogoutState::Cleared);
        // 新登录开始后，上一世代的退出证据不得继续显示。
        auth.begin("grok", 2).await.unwrap();
        let view = auth.view("grok", 2);
        assert_eq!(view.logout, LogoutEvidence::default());
        assert_eq!(view.logout.local, LocalLogoutState::NotAttempted);
        assert!(auth.shutdown().len() == 1);
    }

    /// 身份与挑战字段必须原样保留：长账号标识与长验证地址不得被脱敏涂掉。
    #[tokio::test]
    async fn event_fields_are_preserved_verbatim_in_the_view() {
        let directory = tempfile::tempdir().unwrap();
        let identity = format!("{}@example.invalid", "a".repeat(40));
        let verification_url = format!("https://example.invalid/device?code={}", "b".repeat(40));
        let user_code = "c".repeat(30);
        let script = fake_helper(
            &directory,
            "long-grok.sh",
            &format!(
                "printf '%s\\n' '{{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Open the long fixture URL\",\"verification_url\":\"{verification_url}\",\"user_code\":\"{user_code}\"}}'\nprintf '%s\\n' '{{\"event\":\"identity\",\"identity\":\"{identity}\"}}'\nprintf '%s\\n' '{{\"event\":\"done\"}}'"
            ),
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        let challenge = auth.begin("grok", 1).await.unwrap();
        assert_eq!(challenge.verification_url.as_deref(), Some(verification_url.as_str()), "验证地址必须原样保留");
        assert_eq!(challenge.user_code.as_deref(), Some(user_code.as_str()));
        assert_eq!(challenge.instructions, "Open the long fixture URL");
        let mut poll = AuthPoll::Pending;
        for _ in 0..200 {
            poll = auth.poll("grok", 1, 1).await.unwrap();
            if poll != AuthPoll::Pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(poll, AuthPoll::Succeeded { identity: identity.clone() }, "长账号标识必须原样保留");
        assert_eq!(auth.view("grok", 1).identity.as_deref(), Some(identity.as_str()));
    }

    /// H1：失败的登录尝试必须清掉专用 home，下一次 begin 不得复用脏目录。
    #[tokio::test]
    async fn a_failed_begin_cleans_the_provider_home_before_the_next_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let failing = fake_helper(
            &directory,
            "failing-grok.sh",
            "printf '%s' 'dirty' > \"$GROK_HOME/dirty-session\"\nprintf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Open\"}'\nprintf '%s\\n' '{\"event\":\"error\",\"code\":\"login_failed\",\"message\":\"fixture rejection\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), failing);
        auth.begin("grok", 1).await.unwrap();
        assert!(home.join("dirty-session").is_file());
        // 轮询拿到失败后必须清掉脏 home。
        let mut poll = AuthPoll::Pending;
        for _ in 0..200 {
            poll = auth.poll("grok", 1, 1).await.unwrap();
            if poll != AuthPoll::Pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(matches!(poll, AuthPoll::Failed { .. }), "expected failure, got {poll:?}");
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "失败的尝试不得留下脏 home");
        auth.shutdown();

        // 同一个 home 再次用于一次成功的登录：不得复用上一次的脏内容。
        let good = fake_helper(
            &directory,
            "good-grok.sh",
            "printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Open\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), good);
        auth.begin("grok", 1).await.unwrap();
        assert!(!home.join("dirty-session").exists(), "新的尝试不得复用脏目录");
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Pending);
        auth.shutdown();
    }

    /// H1：spawn 失败也要清掉 prepare_home 建出来的目录内容。
    #[tokio::test]
    async fn a_spawn_failure_cleans_the_prepared_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), directory.path().join("missing-helper"));
        let error = auth.begin("grok", 1).await.unwrap_err();
        assert!(!error.is_empty());
        assert!(home.is_dir(), "prepare_home 已建出目录");
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "spawn 失败必须清空专用 home");
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Idle);
    }

    /// H1：读取端在给出 challenge 前关闭，同样不得留下脏 home。
    #[tokio::test]
    async fn a_helper_that_exits_without_a_challenge_cleans_the_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let script = fake_helper(&directory, "silent-grok.sh", "printf '%s' 'dirty' > \"$GROK_HOME/dirty-session\"\nexit 0");
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        let error = auth.begin("grok", 1).await.unwrap_err();
        assert!(error.contains(CODE_HELPER_EXITED), "{error}");
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "退出前不得留下脏 home");
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Idle);
    }

    /// H2：dispose 丢弃 session 与退出证据、回收进程并清空 home。
    #[tokio::test]
    async fn grok_cli_auth_dispose_drops_session_evidence_processes_and_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let script = fake_helper(
            &directory,
            "fake-grok.sh",
            "printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 1).await.unwrap();
        std::fs::write(home.join("session"), "fixture").unwrap();
        let _ = auth.logout("grok", 1).await.unwrap();
        assert_eq!(auth.view("grok", 1).logout.local, LocalLogoutState::Cleared);
        auth.dispose("grok").unwrap();
        assert!(auth.shutdown().is_empty(), "dispose 必须回收自有进程");
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Idle, "session 必须被丢弃");
        assert_eq!(auth.view("grok", 1).logout.local, LocalLogoutState::NotAttempted, "退出证据必须被丢弃");
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "专用 home 必须清空");
    }

    /// H2：rename 迁移 home 与 session，目标已存在时拒绝覆盖。
    #[tokio::test]
    async fn grok_cli_auth_rename_moves_home_and_session_and_refuses_to_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let old_home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let new_home = helper::helper_home(&ProviderKind::GrokSubscription, "grok-2", directory.path()).unwrap();
        let script = fake_helper(
            &directory,
            "fake-grok.sh",
            "printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 1).await.unwrap();
        std::fs::write(old_home.join("session"), "fixture").unwrap();
        auth.rename("grok", "grok-2").unwrap();
        assert!(!old_home.exists(), "旧 home 必须已迁走");
        assert!(new_home.join("session").is_file(), "home 内容必须迁到新 id");
        assert_eq!(auth.view("grok-2", 1).phase, AuthPhase::Pending, "session 必须改键到新 id");
        assert_eq!(auth.view("grok-2", 1).attempt, Some(1));
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Idle);
        auth.shutdown();

        // 目标已存在：拒绝覆盖，源与目标都不改动。
        auth.begin("grok", 1).await.unwrap();
        std::fs::write(old_home.join("dirty"), "fixture").unwrap();
        assert!(auth.rename("grok", "grok-2").is_err());
        assert!(new_home.join("session").is_file(), "拒绝覆盖时目标不得被改动");
        assert!(old_home.join("dirty").is_file(), "拒绝覆盖时源目录保持不变");
        assert_eq!(auth.view("grok-2", 1).attempt, Some(1), "失败不得迁移 session");
        auth.shutdown();
    }

    /// N2：rename 必须把自有进程登记改到新 key，否则新 id 抓不到旧进程。
    #[tokio::test]
    async fn grok_cli_auth_rename_rekeys_the_owned_process_registry() {
        let directory = tempfile::tempdir().unwrap();
        let script = fake_helper(
            &directory,
            "fake-grok.sh",
            "printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 1).await.unwrap();
        auth.rename("grok", "grok-2").unwrap();
        assert!(auth.processes.reclaim("grok").is_empty(), "旧 key 不得再登记该进程");
        let reclaimed = auth.processes.reclaim("grok-2");
        assert_eq!(reclaimed.len(), 1, "rename 后必须能用新 id 回收自有进程");
        assert!(auth.processes.reclaim_all().is_empty());
    }

    /// 对同一服务商再次 begin 必须回收旧尝试的进程，只留下 1 个自有 pid。
    #[tokio::test]
    async fn grok_cli_auth_begin_reclaims_the_previous_attempt_before_spawning() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        let script = fake_helper(
            &directory,
            "slow-grok.sh",
            "printf '%s\\n' \"$$\" >> \"$GROK_HOME/spawns\"\nprintf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'\nexec sleep 30",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 1).await.unwrap();
        auth.begin("grok", 1).await.unwrap();
        // 假 helper 每次启动都会登记自己的 pid：两次 begin 确实拉起过两个进程。
        let spawns = std::fs::read_to_string(home.join("spawns")).unwrap();
        assert_eq!(spawns.lines().count(), 2, "两次 begin 各拉起一个 helper");
        assert_eq!(auth.view("grok", 1).attempt, Some(2), "新尝试的 attempt 必须递增");
        // 但同一服务商登记的自有进程只应剩 1 个：第一次尝试在第二次 spawn 前已被回收。
        let reclaimed = auth.shutdown();
        assert_eq!(reclaimed.len(), 1, "重入 begin 必须回收旧尝试，只留 1 个自有进程");
    }

    #[tokio::test]
    async fn grok_cli_auth_cancel_supersedes_late_results_and_reclaims_the_process() {
        let directory = tempfile::tempdir().unwrap();
        let script = fake_helper(
            &directory,
            "slow-grok.sh",
            "printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'\nsleep 0.4\nprintf '%s\\n' '{\"event\":\"identity\",\"identity\":\"late@example.invalid\"}'\nprintf '%s\\n' '{\"event\":\"done\"}'",
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 7).await.unwrap();
        auth.cancel("grok", 1).await.unwrap();
        assert!(auth.shutdown().is_empty(), "取消必须回收自有进程");
        // 迟到事件到达后，旧 attempt 依然是 Superseded，不写入任何状态。
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        assert_eq!(auth.poll("grok", 7, 1).await.unwrap(), AuthPoll::Superseded);
        let view = auth.view("grok", 7);
        assert_eq!(view.phase, AuthPhase::Cancelled);
        assert!(view.identity.is_none());
        assert!(view.challenge.is_none());
    }

    #[tokio::test]
    async fn grok_cli_auth_logout_clears_its_home_and_never_claims_remote_revocation() {
        let directory = tempfile::tempdir().unwrap();
        let script = fake_helper(&directory, "unused-grok.sh", "exit 0");
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();
        helper::prepare_home(&home).unwrap();
        std::fs::write(home.join("credentials.json"), "fixture").unwrap();
        let evidence = auth.logout("grok", 3).await.unwrap();
        assert_eq!(evidence.local, LocalLogoutState::Cleared);
        assert_eq!(evidence.remote, RemoteRevokeState::NotAttempted);
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
        assert!(home.is_dir(), "只清空内容，保留目录本身");
        assert_eq!(auth.view("grok", 3).logout.local, LocalLogoutState::Cleared);
        assert_eq!(auth.view("grok", 3).logout.remote, RemoteRevokeState::NotAttempted);
    }

    #[tokio::test]
    async fn grok_cli_auth_reports_helper_errors_with_credentials_redacted() {
        let directory = tempfile::tempdir().unwrap();
        let secret = format!("xai-{}", "0123456789abcdef".repeat(3));
        let script = fake_helper(
            &directory,
            "failing-grok.sh",
            &format!(
                "printf '%s\\n' '{{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Open\"}}'\nprintf '%s\\n' '{{\"event\":\"error\",\"code\":\"login_failed\",\"message\":\"rejected {secret}\"}}'"
            ),
        );
        let auth = GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script);
        auth.begin("grok", 1).await.unwrap();
        let mut poll = AuthPoll::Pending;
        for _ in 0..200 {
            poll = auth.poll("grok", 1, 1).await.unwrap();
            if poll != AuthPoll::Pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        match poll {
            AuthPoll::Failed { error } => {
                assert_eq!(error.code, "login_failed");
                assert!(!error.message.contains(&secret), "{}", error.message);
                assert!(error.message.contains("[redacted]"), "{}", error.message);
            }
            other => panic!("expected a failed poll, got {other:?}"),
        }
    }

    /// 按调用序号行为的假 helper：每次写 `session-<n>` 标记；`challenge_from` 及之后的调用才输出
    /// challenge（可选随后输出 error 终态），最后 `exec sleep 30` 保持存活以便回收断言。
    /// 全部是本地脚本：不联网、不装 CLI、不碰真实凭据。
    fn sequenced_helper(directory: &tempfile::TempDir, name: &str, challenge_from: u32, fail_after_challenge: bool) -> PathBuf {
        let mut body = String::from(
            "n=$(cat \"$GROK_HOME/invocations\" 2>/dev/null || echo 0); n=$((n+1)); echo \"$n\" > \"$GROK_HOME/invocations\"; ",
        );
        body.push_str(&format!("if [ \"$n\" -ge {challenge_from} ]; then "));
        body.push_str("echo fixture > \"$GROK_HOME/session-$n\"; ");
        body.push_str("printf '%s\\n' '{\"event\":\"challenge\",\"kind\":\"device_code\",\"instructions\":\"Wait\"}'; ");
        if fail_after_challenge {
            body.push_str("sleep 0.2; printf '%s\\n' '{\"event\":\"error\",\"code\":\"helper_error\",\"message\":\"fixture poll failure\"}'; ");
        }
        body.push_str("fi; exec sleep 30");
        fake_helper(directory, name, &body)
    }

    /// 等条件成立，避免依赖线程调度时序。
    async fn wait_until(mut condition: impl FnMut() -> bool) {
        for _ in 0..300 {
            if condition() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("condition was not met in time");
    }

    /// 不变式：被取代尝试的失败收尾不得销毁当前尝试的进程与专用 home。
    #[tokio::test]
    async fn a_superseded_attempt_never_reclaims_the_current_attempt() {
        let directory = tempfile::tempdir().unwrap();
        // 第一次调用不输出 challenge：尝试 A 拿不到 challenge，随后被 B 取代。
        let script = sequenced_helper(&directory, "superseded-grok.sh", 2, false);
        let auth = Arc::new(GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script));
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();

        // 尝试 A 的进程（还停在首个 challenge 的等待里，随后会被 B 的 pre-reclaim 取代）。
        let spec = auth.spec("grok", &auth.program.clone().unwrap()).unwrap();
        helper::prepare_home(&spec.home).unwrap();
        let pid_a = auth.processes.spawn("grok", &spec).unwrap();
        // 等 A 的 helper 真正跑起来（写入调用计数）：否则 B 的 pre-reclaim 可能在 A 的脚本执行前就杀掉它，
        // 第二次调用就会因为看不到计数而拿不到 challenge。
        wait_until(|| home.join("invocations").is_file()).await;

        // 尝试 B：真实 begin 取代 A 并成功进入 Pending（第二次调用输出 challenge 并写 session-2）。
        auth.begin("grok", 1).await.unwrap();
        assert!(auth.processes.has_owned("grok"), "尝试 B 的进程必须已登记");
        assert!(home.join("session-2").is_file(), "尝试 B 必须写入自己的 home 内容");
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Pending);

        // A 迟到的失败收尾（它自己那个已经被取代的 pid）——显式调用以保证确定性，不依赖线程调度。
        auth.abort_attempt("grok", Some(pid_a));

        assert!(auth.processes.has_owned("grok"), "当前尝试 B 的进程不得被 A 的收尾杀掉");
        assert!(home.join("session-2").is_file(), "当前尝试写入的 home 内容不得被清掉");
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Pending, "当前尝试必须仍是 Pending");
    }

    /// 不变式：取消只销毁被取消那次尝试自己的进程；旧 attempt 的取消是 no-op，
    /// 取消当前 attempt 时也不得连坐杀掉已 spawn 但尚未写入 session 的并发新尝试。
    #[tokio::test]
    async fn cancel_of_a_previous_attempt_does_not_reclaim_the_current_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let script = sequenced_helper(&directory, "cancel-grok.sh", 1, false);
        let auth = Arc::new(GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script));
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();

        auth.begin("grok", 1).await.unwrap();
        auth.begin("grok", 1).await.unwrap();
        let view = auth.view("grok", 1);
        assert_eq!(view.phase, AuthPhase::Pending);
        assert_eq!(view.attempt, Some(2));

        // 旧 attempt 的取消是纯 no-op：当前尝试的进程与 home 都不受影响。
        auth.cancel("grok", 1).await.unwrap();
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Pending);
        assert!(auth.processes.has_owned("grok"));
        assert!(home.join("session-2").is_file());

        // 并发的新尝试：进程已登记，但 session 里还没有它（begin 在 spawn 与写 session 之间的窗口）。
        let spec = auth.spec("grok", &auth.program.clone().unwrap()).unwrap();
        let concurrent_pid = auth.processes.spawn("grok", &spec).unwrap();
        wait_until(|| home.join("session-3").is_file()).await;
        auth.cancel("grok", 2).await.unwrap();
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Cancelled);
        assert!(auth.processes.has_owned("grok"), "并发新尝试的进程必须仍存活");
        assert!(home.join("session-3").is_file(), "并发新尝试正在用的 home 不得被清掉");
        assert_eq!(auth.processes.reclaim("grok"), vec![concurrent_pid], "只允许回收被取消那次尝试自己的 pid");
    }

    /// 不变式：poll 的 Failed 终态只销毁自己那次尝试；旧 attempt 的 poll 是 Superseded，
    /// 当前 attempt 以 Failed 收尾时不得清掉并发新尝试正在用的 home。
    #[tokio::test]
    async fn a_failed_poll_of_a_previous_attempt_keeps_the_current_home() {
        let directory = tempfile::tempdir().unwrap();
        let script = sequenced_helper(&directory, "poll-grok.sh", 1, true);
        let auth = Arc::new(GrokCliAuth::with_test_helper(directory.path().to_path_buf(), script));
        let home = helper::helper_home(&ProviderKind::GrokSubscription, "grok", directory.path()).unwrap();

        auth.begin("grok", 1).await.unwrap();
        auth.begin("grok", 1).await.unwrap();
        assert_eq!(auth.view("grok", 1).attempt, Some(2));
        // 旧 attempt 的 poll 必须 Superseded，且什么都不销毁。
        assert_eq!(auth.poll("grok", 1, 1).await.unwrap(), AuthPoll::Superseded);
        assert!(auth.processes.has_owned("grok"));
        assert!(home.join("session-2").is_file());

        // 并发新尝试（已 spawn、尚未写 session）：当前 attempt 的 Failed 终态不得连坐。
        let spec = auth.spec("grok", &auth.program.clone().unwrap()).unwrap();
        let concurrent_pid = auth.processes.spawn("grok", &spec).unwrap();
        wait_until(|| home.join("session-3").is_file()).await;
        let mut result = AuthPoll::Pending;
        for _ in 0..300 {
            result = auth.poll("grok", 1, 2).await.unwrap();
            if !matches!(result, AuthPoll::Pending) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(matches!(result, AuthPoll::Failed { .. }), "当前 attempt 应以 Failed 收尾");
        assert!(auth.processes.has_owned("grok"), "并发新尝试的进程必须仍存活");
        assert!(home.join("session-3").is_file(), "失败收尾不得清掉并发新尝试正在用的 home");
        assert_eq!(auth.processes.reclaim("grok"), vec![concurrent_pid], "只允许回收本尝试自己的 pid");
    }

    #[tokio::test]
    async fn grok_cli_auth_refuses_to_start_without_a_helper() {
        let directory = tempfile::tempdir().unwrap();
        let auth = GrokCliAuth::from_parts(directory.path().to_path_buf(), None, Vec::new());
        assert!(!auth.available());
        let error = auth.begin("grok", 1).await.unwrap_err();
        assert!(error.contains(CODE_HELPER_MISSING), "{error}");
        assert_eq!(auth.view("grok", 1).phase, AuthPhase::Idle);
        assert_eq!(auth.view("grok", 1).attempt, None);
    }
}