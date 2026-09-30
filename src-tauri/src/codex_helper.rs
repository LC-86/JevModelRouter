//! 官方 Codex 辅助进程的监管：专用授权目录、可执行文件解析、stdio JSON-RPC 客户端与脱敏。
//!
//! 本模块只管理**自己 spawn 的**子进程：专用 `CODEX_HOME`、单进程读写、退出时 kill + wait。
//! 生产只解析官方 `codex`；`--autojev-helper` 覆盖只存在于 `isolation-check` 构建，且需要
//! `runtime::isolated()`。错误与日志在这里统一脱敏，不输出 token、authorization code 或 refresh token 原文。

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::config::ProviderKind;
use crate::subscription::{
    CatalogRead, ConnectionState, ConnectionStatus, GenerationRequest, GenerationStream, HelperStatus,
    LoginResult, LoginStart, LogoutOutcome, QuotaEvidence, RemoteRevocation, SubscriptionAdapter,
};

/// 用户主目录下由本应用独占的辅助进程根目录。
const HELPER_ROOT: &str = ".autojev/helpers/codex";
/// 官方 Codex 常见安装位置；PATH 优先。
#[cfg(unix)]
const COMMON_CODEX_LOCATIONS: &[&str] = &[
    "/opt/homebrew/bin/codex",
    "/usr/local/bin/codex",
    "/usr/bin/codex",
    "/bin/codex",
];
#[cfg(not(unix))]
const COMMON_CODEX_LOCATIONS: &[&str] = &[];

/// 从 helper 环境里剔除的凭据类变量；辅助进程只应使用自己的 `CODEX_HOME`。
const CREDENTIAL_ENV: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENAI_ACCESS_TOKEN",
    "OPENAI_REFRESH_TOKEN",
    "OPENAI_ID_TOKEN",
    "CHATGPT_TOKEN",
    "CHATGPT_API_KEY",
    "CODEX_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "CODEX_REFRESH_TOKEN",
    "CODEX_ID_TOKEN",
    "AUTOJEV_CODEX_HELPER_TOKEN",
];

/// 专用授权目录：`<home>/.autojev/helpers/codex/<sanitized provider id>`。
/// 绝不读写用户的 `~/.codex`。
pub fn helper_home(provider_id: &str) -> PathBuf {
    helper_home_in(&crate::runtime::home_dir().unwrap_or_default(), provider_id)
}

fn helper_home_in(home: &Path, provider_id: &str) -> PathBuf {
    home.join(HELPER_ROOT).join(sanitize_provider_id(provider_id))
}

/// 仅保留 `[A-Za-z0-9_-]`，其余替换为 `_`；结果为空时用 `provider`。
fn sanitize_provider_id(provider_id: &str) -> String {
    let sanitized: String = provider_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "provider".to_owned()
    } else {
        sanitized
    }
}

/// 一次辅助进程启动的完整描述（可执行文件 + 固定参数）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// 官方辅助进程的子命令：`codex app-server`，默认 stdio 传输。
const HELPER_SUBCOMMAND: &str = "app-server";
/// `initialize` 的客户端自述；只用于握手，不含账号或凭据。
const CLIENT_INFO_NAME: &str = "autojev";
const CLIENT_INFO_TITLE: &str = "AutoJev";

/// 官方 `codex` 的启动描述。子命令固定在代码里，不由配置或环境决定。
fn official_launch(program: PathBuf) -> HelperLaunch {
    HelperLaunch { program, args: vec![HELPER_SUBCOMMAND.to_owned()] }
}

/// 解析要启动的辅助进程。生产只接受 PATH 或常见安装位置里的官方 `codex`；
/// 覆盖入口只存在于 `isolation-check` 构建，且必须已经处于隔离模式。
pub fn resolve_launch() -> Result<HelperLaunch> {
    if let Some(launch) = override_launch() {
        return Ok(launch);
    }
    let program = resolve_official(std::env::var_os("PATH").as_deref(), &common_locations())
        .context("The official Codex executable was not found; install Codex or add it to PATH")?;
    Ok(official_launch(program))
}

/// 隔离验收专用的覆盖入口。生产构建里它被编译成恒 `None`。
#[cfg(feature = "isolation-check")]
fn override_launch() -> Option<HelperLaunch> {
    if !crate::runtime::isolated() {
        return None;
    }
    crate::runtime::helper_override().map(|program| HelperLaunch { program: program.to_path_buf(), args: Vec::new() })
}

#[cfg(not(feature = "isolation-check"))]
fn override_launch() -> Option<HelperLaunch> {
    None
}

fn common_locations() -> Vec<PathBuf> {
    COMMON_CODEX_LOCATIONS.iter().map(PathBuf::from).collect()
}

/// 纯函数形式的解析接缝：显式传入 PATH 与候选目录，便于离线断言而不改动进程环境。
fn resolve_official(path_env: Option<&OsStr>, common: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path_env) = path_env {
        for directory in std::env::split_paths(path_env) {
            if let Some(program) = executable_in(&directory) {
                return Some(program);
            }
        }
    }
    common.iter().find_map(|candidate| candidate.is_file().then(|| candidate.clone()))
}

fn executable_in(directory: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let names = ["codex.exe", "codex.cmd", "codex.bat"];
    #[cfg(not(windows))]
    let names = ["codex"];
    names.iter().find_map(|name| {
        let candidate = directory.join(name);
        candidate.is_file().then(|| candidate.clone())
    })
}

/// 文本里按 `key=value` / `key:value` 形式出现的敏感键。
const TEXT_SECRET_KEYS: &[&str] = &[
    "authorization", "authorization_code", "access_token", "refresh_token", "id_token",
    "api_key", "apikey", "secret", "password", "passwd", "token", "code",
];
/// JSON 对象里整体隐藏取值的敏感键（`authorizationUrl`、`code` 这类非秘密字段不在此列）。
const JSON_SECRET_KEYS: &[&str] =
    &["token", "secret", "password", "passwd", "credential", "api_key", "apikey", "authorization_code"];

/// 脱敏一行文本：隐藏敏感键的取值与高熵长串，并限制长度。
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(512));
    for (index, word) in text.split_whitespace().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&redact_word(word));
        if out.len() > 400 {
            // 绝不在多字节字符中间截断：先回退到合法字符边界（redact 不能 panic）。
            let mut limit = 400;
            while limit > 0 && !out.is_char_boundary(limit) {
                limit -= 1;
            }
            out.truncate(limit);
            out.push('…');
            break;
        }
    }
    out
}

fn redact_word(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    for separator in ['=', ':'] {
        if let Some(position) = lower.find(separator) {
            let key = lower[..position].trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_');
            if TEXT_SECRET_KEYS.contains(&key) {
                return format!("{}[redacted]", &word[..position + 1]);
            }
        }
    }
    if looks_like_secret(word) {
        "[redacted]".to_owned()
    } else {
        word.to_owned()
    }
}

/// 长且同时含字母与数字的紧致串按秘密处理；短词保留可读性。
fn looks_like_secret(word: &str) -> bool {
    let cleaned = word.trim_matches(|character: char| {
        !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '='))
    });
    if cleaned.len() < 24 {
        return false;
    }
    let opaque = cleaned
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '='));
    let mixed = cleaned.chars().any(|character| character.is_ascii_alphabetic())
        && cleaned.chars().any(|character| character.is_ascii_digit());
    opaque && mixed
}

/// 递归脱敏 JSON：敏感键整体隐藏，其余字符串走 [`redact`]。
pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    if JSON_SECRET_KEYS.iter().any(|secret| lower.contains(secret)) {
                        (key.clone(), Value::String("[redacted]".into()))
                    } else {
                        (key.clone(), redact_json(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        Value::String(text) => Value::String(redact(text)),
        other => other.clone(),
    }
}

/// 一次 RPC 的等待上限。平台侧正常都应远快于此。
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// stdio 换行分隔 JSON-RPC 2.0 客户端：一个辅助进程一个实例，只读写自己 spawn 的子进程。
pub struct CodexAppServer {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    auth_home: PathBuf,
    version: Option<String>,
    next_id: u64,
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<std::result::Result<Value, String>>>>>,
    notifications: tokio::sync::mpsc::UnboundedReceiver<Value>,
}

impl CodexAppServer {
    /// 启动辅助进程并完成 `initialize` 握手。子进程的 `CODEX_HOME` 只能是专用目录。
    pub async fn start(provider_id: &str, launch: HelperLaunch) -> Result<CodexAppServer> {
        Self::start_with_home(helper_home(provider_id), launch).await
    }

    /// 测试专用：把专用目录根换成临时目录，避免碰真实用户主目录。
    #[cfg(test)]
    pub async fn start_in(home_root: &Path, provider_id: &str, launch: HelperLaunch) -> Result<CodexAppServer> {
        Self::start_with_home(helper_home_in(home_root, provider_id), launch).await
    }

    async fn start_with_home(auth_home: PathBuf, launch: HelperLaunch) -> Result<CodexAppServer> {
        prepare_auth_home(&auth_home)?;
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .env("CODEX_HOME", &auth_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in CREDENTIAL_ENV {
            command.env_remove(name);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("Start the Codex helper {}", launch.program.display()))?;
        let stdin = child.stdin.take().context("The Codex helper has no stdin")?;
        let stdout = child.stdout.take().context("The Codex helper has no stdout")?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (sender, notifications) = tokio::sync::mpsc::unbounded_channel();
        spawn_reader(stdout, pending.clone(), sender);
        let mut server = CodexAppServer {
            child: Some(child),
            stdin: Some(stdin),
            auth_home,
            version: None,
            next_id: 0,
            pending,
            notifications,
        };
        // 官方 app-server 握手：`initialize` 带客户端自述，收到响应后再发一条无 id 的 `initialized` 通知。
        let client_info = json!({
            "name": CLIENT_INFO_NAME,
            "title": CLIENT_INFO_TITLE,
            "version": env!("CARGO_PKG_VERSION"),
        });
        let initialized = server.call("initialize", json!({ "clientInfo": client_info })).await?;
        server.version = initialized.get("version").and_then(Value::as_str).map(str::to_owned);
        server.notify("initialized", json!({})).await?;
        Ok(server)
    }

    /// 发送一次请求并等待同 id 的响应；错误信息整体脱敏。
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        {
            let stdin = self.stdin.as_mut().context("The Codex helper is not running")?;
            let mut line = serde_json::to_vec(&request)?;
            line.push(b'\n');
            if let Err(error) = stdin.write_all(&line).and_then(|_| stdin.flush()) {
                self.pending.lock().unwrap().remove(&id);
                bail!("Write to the Codex helper failed: {error}");
            }
        }
        let response = match tokio::time::timeout(CALL_TIMEOUT, receiver).await {
            Ok(Ok(Ok(value))) => value,
            Ok(Ok(Err(message))) => bail!("{message}"),
            Ok(Err(_)) => bail!("The Codex helper exited before answering {method}"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                bail!("The Codex helper did not answer {method} in time");
            }
        };
        if let Some(error) = response.get("error") {
            let sanitized = redact_json(error);
            let message = sanitized
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown helper error");
            bail!("Codex helper rejected {method}: {message}");
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// 发送一条无 id 的通知（例如 `initialized`）：不等响应，只确认写进了子进程 stdin。
    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let stdin = self.stdin.as_mut().context("The Codex helper is not running")?;
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .and_then(|_| stdin.flush())
            .with_context(|| format!("Notify the Codex helper {method} failed"))?;
        Ok(())
    }

    /// 下一条通知（无 id 的消息）；辅助进程 stdout 关闭时返回 `None`。
    pub async fn next_notification(&mut self) -> Option<Value> {
        self.notifications.recv().await
    }

    pub fn auth_home(&self) -> &Path {
        &self.auth_home
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// 只终止并回收自己 spawn 的子进程：kill + wait，绝不广域杀进程。
    pub fn shutdown(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn prepare_auth_home(auth_home: &Path) -> Result<()> {
    std::fs::create_dir_all(auth_home)
        .with_context(|| format!("Create the dedicated Codex helper directory {}", auth_home.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(auth_home, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// 读取线程：响应按 id 唤醒等待者，其余消息进入通知队列；stdout 关闭时叫醒全部等待者。
fn spawn_reader(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<std::result::Result<Value, String>>>>>,
    notifications: tokio::sync::mpsc::UnboundedSender<Value>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            match value.get("id").and_then(Value::as_u64) {
                Some(id) => {
                    if let Some(sender) = pending.lock().unwrap().remove(&id) {
                        let _ = sender.send(Ok(value));
                    }
                }
                None => {
                    if notifications.send(value).is_err() {
                        break;
                    }
                }
            }
        }
        for (_, sender) in pending.lock().unwrap().drain() {
            let _ = sender.send(Err("The Codex helper stopped responding".to_owned()));
        }
    });
}

/// 等待 `account/login/completed` 的上限；超时按失败处理，不无限挂起。
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
/// 轮询通知的切片长度：等待完成通知时不长期独占服务商槽位，cancel/logout 仍能拿到锁。
const POLL_SLICE: std::time::Duration = std::time::Duration::from_millis(120);

#[derive(Clone)]
struct ActiveLogin {
    login_id: String,
    generation: u64,
}

#[derive(Clone, Default)]
struct AdapterState {
    version: Option<String>,
    auth_home: Option<String>,
}

/// 真实订阅适配器：每个订阅服务商一个由本应用管理的官方 Codex 辅助进程。
/// 生成能力仍然拒绝；本应用拥有连接状态、账号绑定与准入。
pub struct CodexAdapter {
    servers: tokio::sync::Mutex<HashMap<String, CodexAppServer>>,
    logins: Mutex<HashMap<String, ActiveLogin>>,
    /// 已被某个等待者取走、但属于另一次尝试的完成通知；由对应等待者领回，避免串线丢失。
    deferred: Mutex<HashMap<String, VecDeque<(String, Value)>>>,
    state: Mutex<AdapterState>,
    /// 仅在测试构建里可显式指定；生产只能走 [`resolve_launch`]。
    launch: Option<HelperLaunch>,
    /// 仅在测试构建里可替换专用目录根；生产永远是用户主目录。
    #[cfg_attr(not(test), allow(dead_code))]
    home_root: Option<PathBuf>,
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self {
            servers: tokio::sync::Mutex::new(HashMap::new()),
            logins: Mutex::new(HashMap::new()),
            deferred: Mutex::new(HashMap::new()),
            state: Mutex::new(AdapterState::default()),
            launch: None,
            home_root: None,
        }
    }

    /// 测试专用：直接指定替身可执行文件与专用目录根。生产代码里没有这个入口。
    #[cfg(test)]
    pub fn with_launch(home_root: PathBuf, launch: HelperLaunch) -> Self {
        let mut adapter = Self::new();
        adapter.launch = Some(launch);
        adapter.home_root = Some(home_root);
        adapter
    }

    /// 该服务商的专用授权目录；测试可换根，生产永远是用户主目录下的自有路径。
    pub(crate) fn auth_home_for(&self, provider_id: &str) -> PathBuf {
        match &self.home_root {
            Some(root) => helper_home_in(root, provider_id),
            None => helper_home(provider_id),
        }
    }

    /// 惰性解析官方 `codex`：解析失败返回可读错误，绝不 panic。
    fn launch(&self) -> Result<HelperLaunch> {
        match &self.launch {
            Some(launch) => Ok(launch.clone()),
            None => resolve_launch(),
        }
    }

    /// 保证该服务商有一个已握手的辅助进程，并把整个会话交给调用方使用。
    async fn servers(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, CodexAppServer>> {
        self.servers.lock().await
    }

    async fn server_for(
        &self,
        provider_id: &str,
    ) -> Result<tokio::sync::MutexGuard<'_, HashMap<String, CodexAppServer>>> {
        let mut servers = self.servers.lock().await;
        if !servers.contains_key(provider_id) {
            let launch = self.launch()?;
            let server = match &self.home_root {
                #[cfg(test)]
                Some(root) => CodexAppServer::start_in(root, provider_id, launch).await?,
                _ => CodexAppServer::start(provider_id, launch).await?,
            };
            *self.state.lock().unwrap() = AdapterState {
                version: server.version().map(str::to_owned),
                auth_home: Some(server.auth_home().to_string_lossy().into_owned()),
            };
            servers.insert(provider_id.to_owned(), server);
        }
        Ok(servers)
    }

    /// 结束一次登录尝试：只清理仍属于它的挂起记录，然后用 `account/read` 核实身份。
    async fn complete_login(&self, provider_id: &str, params: Value) -> Result<Option<LoginResult>> {
        if let Some(login_id) = params.get("loginId").and_then(Value::as_str) {
            let mut logins = self.logins.lock().unwrap();
            if logins.get(provider_id).is_some_and(|active| active.login_id == login_id) {
                logins.remove(provider_id);
            }
        }
        if !params.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let error = params
                .get("error")
                .and_then(Value::as_str)
                .map(redact)
                .unwrap_or_else(|| "The Codex helper reported a failed sign-in".to_owned());
            return Ok(Some(LoginResult::Failed(error)));
        }
        // 完成通知不足以激活账号：必须再用 account/read 核实返回的身份。
        let mut servers = self.servers.lock().await;
        let Some(server) = servers.get_mut(provider_id) else {
            return Ok(Some(LoginResult::Failed(
                "The Codex helper exited before the account could be verified".to_owned(),
            )));
        };
        let verified = server.call("account/read", json!({})).await;
        let identity = verified.as_ref().ok().and_then(account_email);
        let Some(identity) = identity else {
            return Ok(Some(LoginResult::Failed(
                "The Codex helper did not confirm a signed-in account".to_owned(),
            )));
        };
        let reported = params.get("account").and_then(|account| account.get("email")).and_then(Value::as_str);
        if reported.is_some_and(|reported| reported != identity) {
            return Ok(Some(LoginResult::Failed(
                "The Codex helper returned an identity that does not match the signed-in account".to_owned(),
            )));
        }
        Ok(Some(LoginResult::Completed { identity }))
    }
}

/// 只处理 `account/login/completed`：返回它绑定的 loginId 与 params。
fn login_completion(value: &Value) -> Option<(String, Value)> {
    if value.get("method").and_then(Value::as_str) != Some("account/login/completed") {
        return None;
    }
    let params = value.get("params")?.clone();
    let login_id = params.get("loginId").and_then(Value::as_str)?.to_owned();
    Some((login_id, params))
}

fn defer(deferred: &Mutex<HashMap<String, VecDeque<(String, Value)>>>, provider_id: &str, login_id: &str, params: Value) {
    deferred
        .lock()
        .unwrap()
        .entry(provider_id.to_owned())
        .or_default()
        .push_back((login_id.to_owned(), params));
}

fn take_deferred(deferred: &Mutex<HashMap<String, VecDeque<(String, Value)>>>, provider_id: &str, login_id: &str) -> Option<Value> {
    let mut store = deferred.lock().unwrap();
    let queue = store.get_mut(provider_id)?;
    let index = queue.iter().position(|(id, _)| id == login_id)?;
    let (_, params) = queue.remove(index)?;
    if queue.is_empty() {
        store.remove(provider_id);
    }
    Some(params)
}

impl SubscriptionAdapter for CodexAdapter {
    fn available(&self) -> bool {
        self.launch().is_ok()
    }

    /// 这个适配器只承载官方 Codex；Grok 等其它订阅服务商不得借用它的进程或身份。
    fn supports(&self, kind: &ProviderKind) -> bool {
        matches!(kind, ProviderKind::CodexSubscription)
    }

    fn helper_status(&self) -> HelperStatus {
        let state = self.state.lock().unwrap().clone();
        HelperStatus { available: self.launch().is_ok(), version: state.version, auth_home: state.auth_home }
    }

    fn status<'a>(&'a self, provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<ConnectionStatus>> {
        Box::pin(async move {
            let mut servers = self.server_for(provider_id).await?;
            let server = servers.get_mut(provider_id).expect("the helper was just started");
            let result = server.call("account/read", json!({})).await?;
            let identity = account_email(&result);
            let requires_auth = result
                .get("requiresAuth")
                .and_then(Value::as_bool)
                .unwrap_or(identity.is_none());
            let state = if requires_auth || identity.is_none() {
                ConnectionState::NotConnected
            } else {
                ConnectionState::Connected
            };
            Ok(ConnectionStatus {
                state,
                identity,
                helper_version: server.version().map(str::to_owned),
                account_path: Some(server.auth_home().to_string_lossy().into_owned()),
            })
        })
    }

    /// 模型目录与额度读取属于后续票据；这里如实报“尚未实现”，不编造证据。
    fn models<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<CatalogRead>> {
        Box::pin(async { bail!("Codex helper directory reads are not implemented in this build") })
    }

    fn quota<'a>(&'a self, _provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<QuotaEvidence>> {
        Box::pin(async { bail!("Codex helper quota reads are not implemented in this build") })
    }

    fn start_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, Result<LoginStart>> {
        Box::pin(async move {
            let mut servers = self.server_for(provider_id).await?;
            let server = servers.get_mut(provider_id).expect("the helper was just started");
            let result = server.call("account/login/start", json!({"mode": "browser"})).await?;
            let login_id = result
                .get("loginId")
                .and_then(Value::as_str)
                .context("The Codex helper did not return a login id")?
                .to_owned();
            self.logins.lock().unwrap().insert(
                provider_id.to_owned(),
                ActiveLogin { login_id: login_id.clone(), generation },
            );
            Ok(LoginStart {
                login_id,
                authorization_url: result.get("authorizationUrl").and_then(Value::as_str).map(str::to_owned),
                user_code: result.get("userCode").and_then(Value::as_str).map(str::to_owned),
            })
        })
    }

    /// 等待一次具体登录尝试的完成通知，并且只认这次尝试。
    /// 被取消或被新的尝试替换后立刻返回 `Ok(None)`：既不空等，也不接管别人的通知。
    fn login_result<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, Result<Option<LoginResult>>> {
        Box::pin(async move {
            let tracked = self
                .logins
                .lock()
                .unwrap()
                .get(provider_id)
                .cloned()
                .filter(|active| active.generation == generation);
            let Some(tracked) = tracked else {
                return Ok(None);
            };
            let deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
            loop {
                let current = self.logins.lock().unwrap().get(provider_id).cloned();
                if current.as_ref().map(|active| active.login_id.as_str()) != Some(tracked.login_id.as_str()) {
                    return Ok(None);
                }
                if let Some(params) = take_deferred(&self.deferred, provider_id, &tracked.login_id) {
                    return self.complete_login(provider_id, params).await;
                }
                let next = {
                    let mut servers = self.servers.lock().await;
                    match servers.get_mut(provider_id) {
                        Some(server) => tokio::time::timeout(POLL_SLICE, server.next_notification()).await,
                        None => return Ok(None),
                    }
                };
                let value = match next {
                    Ok(Some(value)) => value,
                    Ok(None) => return Ok(None),
                    Err(_) => {
                        if tokio::time::Instant::now() >= deadline {
                            bail!("The Codex authorization did not complete in time");
                        }
                        continue;
                    }
                };
                let Some((login_id, params)) = login_completion(&value) else {
                    continue;
                };
                if login_id != tracked.login_id {
                    // 属于另一次尝试：放回队列交给它自己的等待者，绝不吞掉。
                    defer(&self.deferred, provider_id, &login_id, params);
                    continue;
                }
                return self.complete_login(provider_id, params).await;
            }
        })
    }

    fn cancel_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let active = self.logins.lock().unwrap().remove(provider_id);
            self.deferred.lock().unwrap().remove(provider_id);
            let Some(active) = active.filter(|active| active.generation == generation) else {
                return Ok(());
            };
            let mut servers = self.servers().await;
            if let Some(server) = servers.get_mut(provider_id) {
                server.call("account/login/cancel", json!({"loginId": active.login_id})).await?;
            }
            Ok(())
        })
    }

    /// 退出：无论 RPC 成败都终止自有子进程并删除本服务商的专用授权目录。
    /// 只删 `helper_home(provider_id)` 这一个自有目录，绝不触碰用户的 `~/.codex`；
    /// 没有活动会话时也要清理，`local_cleared` 只在目录确实删除成功时为真。
    /// 服务商重命名。有活动辅助进程时**拒绝热迁移**：子进程的 `CODEX_HOME` 在 spawn 后就固定了，
    /// 搬目录会造成「磁盘已在新路径、进程仍写旧路径」。此时返回错误，由编排层对旧标识走退出
    /// （杀进程 + 清专用目录），新标识落未连接逼重登——与迁移失败的回退完全一致。
    /// 没有活动进程时才把专用授权目录与挂起状态整体迁到新标识。
    fn rename<'a>(&'a self, old_id: &'a str, new_id: &'a str) -> futures_util::future::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if old_id == new_id {
                return Ok(());
            }
            // 全程持有 servers 锁：迁移期间不允许再为旧标识拉起辅助进程（关闭检查-再搬的竞态）。
            let servers = self.servers.lock().await;
            anyhow::ensure!(
                !servers.contains_key(old_id),
                "The Codex helper is still running for {old_id}; sign out or sign in again after the rename"
            );
            let old_home = self.auth_home_for(old_id);
            let new_home = self.auth_home_for(new_id);
            if old_home.exists() {
                if let Some(parent) = new_home.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("Prepare the helper root {}", parent.display()))?;
                }
                std::fs::rename(&old_home, &new_home).with_context(|| {
                    format!(
                        "Move the dedicated helper home {} to {}",
                        old_home.display(),
                        new_home.display()
                    )
                })?;
            }
            drop(servers);
            if let Some(login) = self.logins.lock().unwrap().remove(old_id) {
                self.logins.lock().unwrap().insert(new_id.to_owned(), login);
            }
            if let Some(queue) = self.deferred.lock().unwrap().remove(old_id) {
                self.deferred.lock().unwrap().insert(new_id.to_owned(), queue);
            }
            let mut state = self.state.lock().unwrap();
            if state.auth_home.as_deref() == Some(old_home.to_string_lossy().as_ref()) {
                state.auth_home = Some(new_home.to_string_lossy().into_owned());
            }
            Ok(())
        })
    }

    fn logout<'a>(&'a self, provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<LogoutOutcome>> {
        Box::pin(async move {
            self.logins.lock().unwrap().remove(provider_id);
            self.deferred.lock().unwrap().remove(provider_id);
            let auth_home = match &self.home_root {
                Some(root) => helper_home_in(root, provider_id),
                None => helper_home(provider_id),
            };
            let remote = match self.servers().await.remove(provider_id) {
                Some(mut server) => {
                    let result = server.call("account/logout", json!({})).await;
                    server.shutdown();
                    match result {
                        Ok(value) => match value.get("remote").and_then(Value::as_str) {
                            Some("revoked") => RemoteRevocation::Revoked,
                            Some("unsupported") => RemoteRevocation::Unsupported,
                            _ => RemoteRevocation::Failed,
                        },
                        Err(_) => RemoteRevocation::Failed,
                    }
                }
                None => RemoteRevocation::Failed,
            };
            let local_cleared = remove_helper_home(&auth_home);
            if local_cleared {
                // 授权目录已不存在，不再对外声称一个已删除的路径。
                let deleted = auth_home.to_string_lossy().into_owned();
                let mut state = self.state.lock().unwrap();
                if state.auth_home.as_deref() == Some(deleted.as_str()) {
                    state.auth_home = None;
                }
            }
            Ok(LogoutOutcome { local_cleared, remote })
        })
    }

    /// 生成仍然默认拒绝：登录成功不改变任何准入，真实传输不属于本票。
    fn generate<'a>(&'a self, _request: GenerationRequest<'a>) -> futures_util::future::BoxFuture<'a, Result<GenerationStream<'a>>> {
        Box::pin(async { bail!("Codex subscription generation is not implemented; this build keeps it denied") })
    }
}

/// 只删除本应用的专用授权目录；路径越界一律拒绝（绝不触碰用户 `~/.codex`）。
/// 目录本来就不存在也算清理成功。
fn remove_helper_home(auth_home: &Path) -> bool {
    let expected_parent = Path::new(HELPER_ROOT);
    if auth_home.parent().map(|parent| parent.ends_with(expected_parent)) != Some(true) {
        return false;
    }
    match std::fs::remove_dir_all(auth_home) {
        Ok(()) => true,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn account_email(result: &Value) -> Option<String> {
    result
        .get("account")
        .and_then(|account| account.get("email"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|email| !email.trim().is_empty())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// 只存在于 Rust 单测临时目录的假 Codex 辅助进程：不联网、不读任何真实凭据。
    /// 第一个参数是环境记录文件路径。
    const FIXTURE_HELPER: &str = r#"#!/bin/sh
env_log="$1"
if [ -n "$env_log" ]; then
  {
    printf 'CODEX_HOME=%s\n' "$CODEX_HOME"
    printf 'OPENAI_API_KEY=%s\n' "${OPENAI_API_KEY:-<unset>}"
  } > "$env_log"
fi
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "$id" ]; then continue; fi
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"version":"fixture-helper-1.0","codexHome":"%s"}}\n' "$id" "$CODEX_HOME" ;;
    *'"method":"account/read"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"account":{"email":"fixture@example.invalid","planType":"pro"},"requiresAuth":false}}\n' "$id" ;;
    *'"method":"account/login/start"'*) n=0; if [ -f "$1.counter" ]; then n=$(cat "$1.counter"); fi; n=$((n+1)); printf '%s' "$n" > "$1.counter"; printf '{"jsonrpc":"2.0","id":%s,"result":{"loginId":"fixture-login-%s","authorizationUrl":"https://example.invalid/auth"}}\n' "$id" "$n"; ( sleep 0.2; printf '{"jsonrpc":"2.0","method":"account/login/completed","params":{"loginId":"fixture-login-%s","ok":true,"account":{"email":"fixture@example.invalid","planType":"pro"}}}\n' "$n" ) & ;;
    *'"method":"account/login/cancel"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"cancelled":true}}\n' "$id" ;;
    *'"method":"account/logout"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"local":"cleared","remote":"revoked"}}\n' "$id" ;;
    *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"no such method; authorization_code=SUPERSECRET1234567890 refresh_token=abcdef0123456789abcdef0123456789"}}\n' "$id" ;;
  esac
done
"#;

    fn fixture_launch(directory: &Path, env_log: &Path) -> HelperLaunch {
        let script = directory.join("fake-codex-app-server");
        std::fs::write(&script, FIXTURE_HELPER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        HelperLaunch { program: script, args: vec![env_log.to_string_lossy().into_owned()] }
    }

    #[test]
    fn helper_home_is_provider_scoped_and_sanitized() {
        let root = Path::new("/tmp/fixture-root");
        assert_eq!(helper_home_in(root, "codex"), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/codex"));
        assert_eq!(helper_home_in(root, "work/../home"), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/work____home"));
        assert_eq!(helper_home_in(root, ""), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/provider"));
        assert_eq!(helper_home_in(root, "codex-work_1"), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/codex-work_1"));
        if let Some(home) = crate::runtime::home_dir() {
            let home_path = helper_home("codex");
            assert_eq!(home_path, home.join(".autojev/helpers/codex/codex"));
            assert_ne!(home_path, home.join(".codex"));
            assert!(!home_path.starts_with(home.join(".codex")));
        }
    }

    #[test]
    fn official_codex_resolution_uses_path_then_known_locations() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("codex");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([directory.path()]).unwrap();
        assert_eq!(resolve_official(Some(path.as_os_str()), &[]), Some(script.clone()));
        let empty = tempfile::tempdir().unwrap();
        let empty_path = std::env::join_paths([empty.path()]).unwrap();
        assert_eq!(resolve_official(Some(empty_path.as_os_str()), &[]), None);
        assert_eq!(resolve_official(None, &[directory.path().join("missing-codex")]), None);
        assert_eq!(resolve_official(None, &[script.clone()]), Some(script));
    }

    #[test]
    fn the_official_helper_is_started_as_a_stdio_app_server() {
        let launch = official_launch(PathBuf::from("/fixture/bin/codex"));
        assert_eq!(launch.program, PathBuf::from("/fixture/bin/codex"));
        assert_eq!(launch.args, vec!["app-server".to_owned()]);
        // 本机解析到官方 codex 时，真实启动描述也必须带同一子命令。
        if let Ok(resolved) = resolve_launch() {
            assert_eq!(resolved.args, vec!["app-server".to_owned()], "production must start codex app-server");
        }
    }

    #[cfg(not(feature = "isolation-check"))]
    #[test]
    fn production_has_no_switch_to_install_a_stub() {
        // 覆盖入口在非 isolation-check 构建里被编译成恒 None，也没有任何配置或环境开关。
        assert!(override_launch().is_none());
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("env.log");
        let launch = fixture_launch(directory.path(), &log);
        if let Ok(resolved) = resolve_launch() {
            assert_ne!(resolved.program, launch.program);
        }
    }

    #[test]
    fn secrets_are_never_written_verbatim() {
        let text = redact("login failed authorization_code=SUPERSECRET1234567890 refresh_token=abcdef0123456789abcdef0123456789");
        assert!(!text.contains("SUPERSECRET"), "{text}");
        assert!(!text.contains("abcdef0123456789"), "{text}");
        assert_eq!(redact("plain short words"), "plain short words");
        let value = redact_json(&json!({
            "error": "token=abcdef0123456789abcdef0123456789",
            "authorization_code": "SUPERSECRET1234567890",
            "code": -32601,
            "authorizationUrl": "https://example.invalid/auth",
        }));
        assert_eq!(value["authorization_code"], "[redacted]");
        assert_eq!(value["code"], -32601);
        assert!(!value["error"].as_str().unwrap().contains("abcdef"));
        assert_eq!(value["authorizationUrl"], "https://example.invalid/auth");
    }

    #[test]
    fn redaction_never_panics_on_multibyte_text() {
        // 单个无空格长词，每个汉字 3 字节：400 一定落在多字节字符中间（旧实现会 panic）。
        let long = "汉字混合文本".repeat(30);
        assert!(long.len() > 400 && !long.is_char_boundary(400));
        let redacted = redact(&long);
        assert!(redacted.ends_with('…'), "{redacted}");
        assert!(redacted.len() <= 403, "{}", redacted.len());
        // 输出仍是合法 UTF-8，且未把字符切坏。
        assert_eq!(redacted, String::from_utf8(redacted.clone().into_bytes()).unwrap());
        let mixed = format!("{long} authorization_code=SUPERSECRET1234567890");
        let redacted = redact(&mixed);
        assert!(!redacted.contains("SUPERSECRET"), "{redacted}");
        assert!(redacted.ends_with('…'));
    }

    #[tokio::test]
    async fn start_uses_a_dedicated_codex_home_and_strips_credentials() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut server = CodexAppServer::start_in(home.path(), "codex-fixture", fixture_launch(home.path(), &log)).await.unwrap();
        assert_eq!(server.version(), Some("fixture-helper-1.0"));
        let expected = helper_home_in(home.path(), "codex-fixture");
        assert_eq!(server.auth_home(), expected.as_path());
        assert!(expected.starts_with(home.path().join(".autojev/helpers/codex")));
        assert_ne!(expected, crate::runtime::home_dir().unwrap_or_default().join(".codex"));
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(recorded.contains(&format!("CODEX_HOME={}", expected.display())), "{recorded}");
        assert!(recorded.contains("OPENAI_API_KEY=<unset>"), "credential variables must be stripped: {recorded}");
        server.shutdown();
    }

    #[tokio::test]
    async fn rpc_round_trip_routes_notifications_and_redacts_errors() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut server = CodexAppServer::start_in(home.path(), "codex-fixture", fixture_launch(home.path(), &log)).await.unwrap();
        let read = server.call("account/read", json!({})).await.unwrap();
        assert_eq!(read["account"]["email"], "fixture@example.invalid");
        let start = server.call("account/login/start", json!({"mode": "browser"})).await.unwrap();
        assert_eq!(start["loginId"], "fixture-login-1");
        let completed = server.next_notification().await.expect("a completion notification");
        assert_eq!(completed["method"], "account/login/completed");
        assert_eq!(completed["params"]["loginId"], "fixture-login-1");
        assert_eq!(completed["params"]["ok"], true);
        let error = server.call("fixture/unknown", json!({})).await.unwrap_err().to_string();
        assert!(!error.contains("SUPERSECRET"), "{error}");
        assert!(!error.contains("abcdef0123456789"), "{error}");
        assert!(error.contains("[redacted]"), "{error}");
        server.shutdown();
    }

    #[tokio::test]
    async fn shutdown_reaps_only_its_own_child() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut server = CodexAppServer::start_in(home.path(), "codex-fixture", fixture_launch(home.path(), &log)).await.unwrap();
        let pid = server.pid().expect("a live helper has a pid");
        let mut unrelated = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        server.shutdown();
        let status = std::process::Command::new("ps").arg("-p").arg(pid.to_string()).status().unwrap();
        assert!(!status.success(), "the owned helper must be terminated and reaped");
        assert!(unrelated.try_wait().unwrap().is_none(), "an unrelated process must not be touched");
        let _ = unrelated.kill();
        let _ = unrelated.wait();
    }

    #[tokio::test]
    async fn adapter_login_verify_logout_and_generation_denied() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        assert!(adapter.available());
        assert!(adapter.helper_status().available);
        let start = adapter.start_login("codex-fixture", 1).await.unwrap();
        assert_eq!(start.login_id, "fixture-login-1");
        assert_eq!(start.authorization_url.as_deref(), Some("https://example.invalid/auth"));
        let status = adapter.helper_status();
        assert_eq!(status.version.as_deref(), Some("fixture-helper-1.0"));
        assert!(status.auth_home.as_deref().unwrap_or_default().contains("codex-fixture"));
        let result = adapter.login_result("codex-fixture", 1).await.unwrap();
        assert_eq!(result, Some(LoginResult::Completed { identity: "fixture@example.invalid".into() }));
        let read = adapter.status("codex-fixture", 1).await.unwrap();
        assert_eq!(read.state, ConnectionState::Connected);
        assert_eq!(read.identity.as_deref(), Some("fixture@example.invalid"));
        // 真实生成仍然默认拒绝：登录成功不改变生成入口。
        let error = match adapter
            .generate(GenerationRequest {
                provider_id: "codex-fixture",
                generation: 1,
                model_id: "fixture-model",
                protocol: crate::protocol::Protocol::Chat,
                body: json!({}),
                streaming: false,
            })
            .await
        {
            Ok(_) => panic!("generation must stay denied"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("keeps it denied"), "{error}");
        let outcome = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(outcome.local_cleared);
        assert_eq!(outcome.remote, RemoteRevocation::Revoked);
        // 没有活动会话时也要清理：再次退出不报错，远端撤销如实记为失败。
        let again = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(again.local_cleared);
        assert_eq!(again.remote, RemoteRevocation::Failed);
    }

    #[tokio::test]
    async fn adapter_logout_removes_only_its_own_auth_home() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        adapter.login_result("codex-fixture", 1).await.unwrap();
        let auth_home = helper_home_in(home.path(), "codex-fixture");
        assert!(auth_home.is_dir(), "the dedicated auth home is used while signed in");
        let outcome = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(outcome.local_cleared, "logout must report the dedicated directory as cleared");
        assert!(!auth_home.exists(), "the dedicated auth home must be removed");
        assert!(adapter.helper_status().auth_home.is_none(), "a deleted path must not be reported");
        // 没有活动会话时也删除：重启后残留的授权目录必须能被退出清掉。
        std::fs::create_dir_all(&auth_home).unwrap();
        let again = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(again.local_cleared);
        assert_eq!(again.remote, RemoteRevocation::Failed);
        assert!(!auth_home.exists());
        // 只删自有目录：用户的 ~/.codex 原样保留。
        let user_codex = home.path().join(".codex");
        std::fs::create_dir_all(&user_codex).unwrap();
        std::fs::write(user_codex.join("auth.json"), "fixture").unwrap();
        adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(user_codex.join("auth.json").is_file(), "the user's own ~/.codex must never be touched");
    }

    #[tokio::test]
    async fn rename_never_hot_migrates_a_running_helper() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        // 拉起 fixture 辅助进程：旧标识下存在活动 helper。
        adapter.start_login("codex-fixture", 1).await.unwrap();
        let old_home = helper_home_in(home.path(), "codex-fixture");
        let new_home = helper_home_in(home.path(), "codex-renamed");
        std::fs::write(old_home.join("fictional-auth.json"), "fixture").unwrap();

        let error = adapter.rename("codex-fixture", "codex-renamed").await.unwrap_err();
        assert!(error.to_string().contains("still running"), "{error}");
        // 绝不出现「磁盘已在新路径、子进程仍写旧 CODEX_HOME」。
        assert!(old_home.join("fictional-auth.json").is_file());
        assert!(!new_home.exists());

        // 回退与迁移失败一致：对旧标识退出（杀进程 + 清专用目录），新标识逼重登。
        let outcome = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(outcome.local_cleared);
        assert!(!old_home.exists());

        // 没有活动进程后才整体迁移：目录与内容跟随新标识。
        std::fs::create_dir_all(&old_home).unwrap();
        std::fs::write(old_home.join("fictional-auth.json"), "fixture").unwrap();
        adapter.rename("codex-fixture", "codex-renamed").await.unwrap();
        assert!(!old_home.exists());
        assert!(new_home.join("fictional-auth.json").is_file());
        // 清理跟随新标识：退出删的是新目录。
        let moved = adapter.logout("codex-renamed", 1).await.unwrap();
        assert!(moved.local_cleared);
        assert!(!new_home.exists());
    }

    #[tokio::test]
    async fn a_superseded_login_waiter_never_starves_the_new_attempt() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        // 真正让旧等待者先进去轮询，再用同世代发起第二次尝试。
        let stale = tokio::spawn({
            let adapter = adapter.clone();
            async move { adapter.login_result("codex-fixture", 1).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let second = adapter.start_login("codex-fixture", 1).await.unwrap();
        assert_eq!(second.login_id, "fixture-login-2");
        // 旧等待者最多只会拿到第一次尝试的结果（编排层按尝试号整体丢弃），并且必须自己退出。
        let stale_result = tokio::time::timeout(std::time::Duration::from_secs(3), stale)
            .await
            .expect("the superseded waiter must stop")
            .expect("the waiter task must not panic")
            .unwrap();
        if let Some(LoginResult::Completed { identity }) = &stale_result {
            assert_eq!(identity, "fixture@example.invalid");
        }
        // 关键性质：第二次尝试的完成通知不会被旧等待者吞掉。
        let current = tokio::time::timeout(std::time::Duration::from_secs(5), adapter.login_result("codex-fixture", 1))
            .await
            .expect("the current attempt still receives its completion")
            .unwrap();
        assert_eq!(current, Some(LoginResult::Completed { identity: "fixture@example.invalid".into() }));
    }

    #[test]
    fn completion_notifications_are_routed_by_login_id() {
        let completed = json!({"jsonrpc": "2.0", "method": "account/login/completed", "params": {"loginId": "a", "ok": true}});
        let (login_id, params) = login_completion(&completed).unwrap();
        assert_eq!(login_id, "a");
        assert_eq!(params["ok"], true);
        assert!(login_completion(&json!({"jsonrpc": "2.0", "method": "account/rateLimits/read"})).is_none());
        let deferred = Mutex::new(HashMap::new());
        defer(&deferred, "codex", "b", json!({"loginId": "b"}));
        defer(&deferred, "codex", "a", json!({"loginId": "a"}));
        assert_eq!(take_deferred(&deferred, "codex", "a"), Some(json!({"loginId": "a"})));
        assert_eq!(take_deferred(&deferred, "codex", "a"), None);
        assert_eq!(take_deferred(&deferred, "codex", "b"), Some(json!({"loginId": "b"})));
        assert!(take_deferred(&deferred, "codex", "b").is_none());
    }

    #[tokio::test]
    async fn adapter_login_result_is_bound_to_its_generation() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        assert_eq!(adapter.login_result("codex-fixture", 2).await.unwrap(), None);
        adapter.cancel_login("codex-fixture", 1).await.unwrap();
    }
}
