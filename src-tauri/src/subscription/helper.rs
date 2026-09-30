//! 订阅辅助进程边界：专用二进制解析、专用 home、脱敏，以及只回收本应用自有进程的登记表。
//!
//! 本模块不依赖 Tauri，也不发起任何网络请求。它只做三件事：把官方 CLI 解析成一个受控的启动
//! spec、把它的存储限制在应用自有目录里、把错误与日志里的凭据抹掉。隔离验证环境
//! （[`crate::runtime::isolated`]）下绝不拉起任何真实进程。
//!
//! 真实 CLI 的参数与输出契约未经验证（见 issue #26 手测项）：默认参数 `["login"]` 与 stdout 逐行
//! JSON 事件都是本应用的适配约定，不是已核实的上游行为。

use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
    sync::Mutex,
};

use anyhow::{bail, ensure, Context, Result};

use crate::config::ProviderKind;

/// 一次受控启动所需的全部信息。`env` 已按白名单过滤，调用方不得再补环境变量。
pub struct HelperSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub home: PathBuf,
}

/// 可以原样带进辅助进程的环境变量：认证来源必须由 `GROK_HOME`/`HOME` 决定，其余上游凭据一律清除。
/// `HOME` 不在此列，它由 [`helper_env`] 显式指向应用自有 home。
const ENV_WHITELIST: &[&str] = &["PATH", "TMPDIR", "LANG", "TERM"];

/// 环境变量覆盖入口，可含空格分隔的附加参数（例如自定义子命令）。
const HELPER_OVERRIDE: &str = "AUTOJEV_GROK_HELPER";

/// 默认参数：这是本应用的适配约定，未对真实 CLI 验证。
const DEFAULT_ARGS: &[&str] = &["login"];

/// 订阅类型在存储路径里的稳定名字，与配置里的 serde 形状保持一致。
fn kind_key(kind: &ProviderKind) -> Result<&'static str> {
    match kind {
        ProviderKind::CodexSubscription => Ok("codex_subscription"),
        ProviderKind::GrokSubscription => Ok("grok_subscription"),
        _ => bail!("Helper storage is only defined for subscription providers"),
    }
}

/// 应用自有的辅助进程根目录：`<home_dir>/.autojev/subscription-helpers/<kind>/<provider_id>/home`。
/// 纯路径计算，不创建目录、不联网。
pub fn helper_home(kind: &ProviderKind, provider_id: &str, home_dir: &Path) -> Result<PathBuf> {
    let kind = kind_key(kind)?;
    let provider_id = provider_id.trim();
    ensure!(!provider_id.is_empty(), "A subscription provider id is required for the helper home");
    ensure!(
        !provider_id.contains('/') && !provider_id.contains('\\') && provider_id != "." && provider_id != "..",
        "The subscription provider id cannot be used as a directory name"
    );
    Ok(home_dir
        .join(".autojev")
        .join("subscription-helpers")
        .join(kind)
        .join(provider_id)
        .join("home"))
}

/// 二进制解析顺序：`AUTOJEV_GROK_HELPER`（可含空格分隔的参数），否则 PATH 中的 `grok`。
/// 找不到就是找不到：不下载、不安装、不猜测路径。
pub(crate) fn resolve_program() -> Option<(PathBuf, Vec<String>)> {
    if let Ok(value) = std::env::var(HELPER_OVERRIDE) {
        let value = value.trim();
        if !value.is_empty() {
            let mut parts = value.split_whitespace();
            let program = parts.next()?;
            return Some((PathBuf::from(program), parts.map(str::to_owned).collect()));
        }
    }
    let path = std::env::var("PATH").ok()?;
    find_on_path("grok", &path).map(|program| (program, Vec::new()))
}

fn find_on_path(name: &str, path: &str) -> Option<PathBuf> {
    path.split(':')
        .filter(|directory| !directory.is_empty())
        .map(|directory| Path::new(directory).join(name))
        .find(|candidate| candidate.is_file())
}

/// 组装启动 spec。`program` 已经解析完成；环境按白名单重建，认证来源只由 `GROK_HOME` 决定。
pub(crate) fn spec_with_program(
    kind: &ProviderKind,
    provider_id: &str,
    home_dir: &Path,
    program: &Path,
    extra_args: &[String],
) -> Result<HelperSpec> {
    let home = helper_home(kind, provider_id, home_dir)?;
    let mut args = extra_args.to_vec();
    args.extend(DEFAULT_ARGS.iter().map(|arg| (*arg).to_owned()));
    Ok(HelperSpec { program: program.to_path_buf(), args, env: helper_env(&home), home })
}

/// `env_clear()` 之后唯一允许进入辅助进程的环境：基础运行变量 + `AUTOJEV_GROK_*` + `GROK_HOME`/`HOME`。
/// `HOME` 也指向应用自有 home：即使真实 CLI 只认 `HOME`，也不会落到日常 `~/.grok`。
/// 只透传本应用为该辅助进程显式定义的前缀：任意 `AUTOJEV_*` 都可能把真实 home 或其它来源带进辅助进程。
fn helper_env(home: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = ENV_WHITELIST
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).to_owned(), value)))
        .collect();
    let mut overrides: Vec<(String, String)> = std::env::vars().filter(|(key, _)| is_helper_override(key)).collect();
    overrides.sort();
    env.extend(overrides);
    let home = home.display().to_string();
    env.push(("GROK_HOME".into(), home.clone()));
    env.push(("HOME".into(), home));
    env
}

/// 只有本应用为辅助进程定义的前缀可以透传；其它 `AUTOJEV_*` 一律不进辅助进程环境。
fn is_helper_override(key: &str) -> bool {
    key.starts_with("AUTOJEV_GROK_")
}

const REDACTED: &str = "[redacted]";

/// 键名里出现这些片段就说明它的值可能是凭据。
const SECRET_KEY_HINTS: &[&str] = &[
    "token", "secret", "password", "passwd", "api_key", "apikey", "api-key", "authorization",
    "auth_key", "cookie", "credential", "private_key", "access_key",
    "device_code", "access_token", "refresh_token", "id_token", "client_secret",
];

/// 常见凭据前缀：即使长度不够门限也一律抹掉。
const SECRET_PREFIXES: &[&str] = &["sk-", "xai-", "gsk_", "ghp_", "gho_", "ghs_", "aiza", "bearer"];

/// 自由文本里连续片段的门限。长标识（如 codex-subscription-openai、claude-3-5-sonnet-20241022）
/// 会出现在登录错误文本里，因此只有 >= 32 个连续 run 字符才算长随机串；赋值形式的凭据键不受此限。
const RANDOM_RUN_THRESHOLD: usize = 32;

/// 去掉凭据：赋值形式（`key=value`/`key:value`）的键命中凭据片段时，值无论多短都遮盖；
/// 自由文本里只有 >= [`RANDOM_RUN_THRESHOLD`] 的连续随机串被遮盖；`Bearer` 后面的一个片段同样遮盖。
/// 所有错误、详情与日志都必须先经过它。事件字段（身份、挑战、地址）原样保留，不经过它。
pub fn redact(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut redact_next = false;
    for (index, token) in text.split_whitespace().enumerate() {
        if index > 0 {
            result.push(' ');
        }
        if redact_next {
            result.push_str(REDACTED);
            redact_next = false;
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if lower == "bearer" || lower == "token" || lower == "secret" {
            redact_next = true;
            result.push_str(token);
            continue;
        }
        if let Some((key, separator)) = split_assignment(token) {
            if is_secret_key(key) {
                result.push_str(key);
                result.push(separator);
                result.push_str(REDACTED);
                continue;
            }
        }
        result.push_str(&mask_long_runs(token));
    }
    if redact_next {
        result.push(' ');
        result.push_str(REDACTED);
    }
    result
}

/// 按第一个 `=` 或 `:` 拆出键名；键名带引号也算，因为 JSON 输出很常见。
fn split_assignment(token: &str) -> Option<(&str, char)> {
    let index = token.find(['=', ':'])?;
    let (key, rest) = token.split_at(index);
    let separator = rest.chars().next()?;
    if key.is_empty() {
        return None;
    }
    Some((key, separator))
}

fn is_secret_key(key: &str) -> bool {
    let key = key.trim_matches(|character: char| character == '"' || character == '\'' || character == '{' || character == '[');
    let lower = key.to_ascii_lowercase();
    SECRET_KEY_HINTS.iter().any(|hint| lower.contains(hint))
}

/// 连续片段的分隔符：`/` 与 `=` 不算，路径与 URL 才不会被当成随机串误伤。
fn is_run_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
}

/// 抹掉单个片段里的凭据形状：常见前缀，或足够长的连续随机串。
fn mask_long_runs(token: &str) -> String {
    let characters: Vec<char> = token.chars().collect();
    let mut result = String::with_capacity(token.len());
    let mut index = 0;
    while index < characters.len() {
        if !is_run_char(characters[index]) {
            result.push(characters[index]);
            index += 1;
            continue;
        }
        let start = index;
        while index < characters.len() && is_run_char(characters[index]) {
            index += 1;
        }
        let run: String = characters[start..index].iter().collect();
        if looks_secret(&run) {
            result.push_str(REDACTED);
        } else {
            result.push_str(&run);
        }
    }
    result
}

fn looks_secret(run: &str) -> bool {
    let lower = run.to_ascii_lowercase();
    SECRET_PREFIXES.iter().any(|prefix| lower.starts_with(prefix)) || run.chars().count() >= RANDOM_RUN_THRESHOLD
}

/// 建好专用 home 并收紧权限：`create_dir_all` + `0700`，拒绝 symlink。
/// 只检查 home 自身与它的直接父目录；更上层（例如 macOS 的 `/var`）属于系统既有布局。
pub fn prepare_home(home: &Path) -> Result<()> {
    ensure!(!home.as_os_str().is_empty(), "The subscription helper home must not be empty");
    ensure!(home.is_absolute(), "The subscription helper home must be absolute");
    ensure!(
        !home.components().any(|component| matches!(component, Component::ParentDir)),
        "The subscription helper home must not contain '..'"
    );
    reject_symlink(home)?;
    if let Some(parent) = home.parent() {
        reject_symlink(parent)?;
    }
    fs::create_dir_all(home).context("create the subscription helper home")?;
    reject_symlink(home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(home, fs::Permissions::from_mode(0o700)).context("restrict the subscription helper home")?;
    }
    Ok(())
}

/// 只清空自家 home 的内容，保留目录本身。目录不存在算已清理；symlink 一律拒绝。
pub fn cleanup_home(home: &Path) -> Result<()> {
    ensure_owned_helper_home(home)?;
    reject_symlink(home)?;
    let entries = match fs::read_dir(home) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("read the subscription helper home"),
    };
    for entry in entries {
        let entry = entry.context("read the subscription helper home")?;
        let path = entry.path();
        let kind = entry.file_type().context("inspect a subscription helper file")?;
        if kind.is_dir() {
            fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))?;
        } else {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            !metadata.file_type().is_symlink(),
            "The subscription helper home must not be a symlink: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect the subscription helper home"),
    }
    Ok(())
}

/// 只允许清理「应用自有 helper 根目录」下的 `<provider>/home`，避免误删别处。
fn ensure_owned_helper_home(home: &Path) -> Result<()> {
    ensure!(home.is_absolute(), "The subscription helper home must be absolute");
    ensure!(
        !home.components().any(|component| matches!(component, Component::ParentDir)),
        "The subscription helper home must not contain '..'"
    );
    ensure!(
        home.file_name().is_some_and(|name| name == "home"),
        "Refusing to clean a directory that is not a subscription helper home"
    );
    ensure!(
        home.components().any(|component| component.as_os_str() == "subscription-helpers"),
        "Refusing to clean a directory outside the subscription helper root"
    );
    Ok(())
}

/// 本应用登记的自有辅助进程。只回收这里出现过的 pid，绝不按名字批量杀进程。
pub struct OwnedProcesses {
    children: Mutex<Vec<OwnedChild>>,
}

struct OwnedChild {
    /// 拉起时显式登记的服务商标识；reclaim 按它过滤，不从路径反推。
    provider_id: String,
    pid: u32,
    child: Child,
    stdout: Option<ChildStdout>,
}

/// 隔离验证环境不得拉起任何真实进程。抽成独立函数便于直接单测这条守卫。
fn ensure_spawn_allowed(isolated: bool) -> Result<()> {
    ensure!(!isolated, "Refusing to launch a subscription helper in isolated validation");
    Ok(())
}

impl OwnedProcesses {
    pub fn new() -> Self {
        Self { children: Mutex::new(Vec::new()) }
    }

    /// 拉起辅助进程并登记 pid。隔离验证环境直接拒绝，绝不拉起真实进程。
    pub fn spawn(&self, provider_id: &str, spec: &HelperSpec) -> Result<u32> {
        ensure_spawn_allowed(crate::runtime::isolated())?;
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("launch the subscription helper {}", spec.program.display()))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        self.children.lock().unwrap().push(OwnedChild {
            provider_id: provider_id.to_owned(),
            pid,
            child,
            stdout,
        });
        Ok(pid)
    }

    /// 取出某个自有进程的标准输出读取端（同一个 pid 只能取一次）。
    /// 这是本模块内部的读取接缝，调用方按 spawn 返回的 pid 取用。
    pub fn take_stdout(&self, pid: u32) -> Option<ChildStdout> {
        self.children
            .lock()
            .unwrap()
            .iter_mut()
            .find(|entry| entry.pid == pid)
            .and_then(|entry| entry.stdout.take())
    }

    /// 回收某家服务商的自有进程，返回实际回收的 pid。
    pub fn reclaim(&self, provider_id: &str) -> Vec<u32> {
        self.reclaim_where(|owned| owned.provider_id == provider_id)
    }

    /// 回收全部自有进程，返回实际回收的 pid。
    pub fn reclaim_all(&self) -> Vec<u32> {
        self.reclaim_where(|_| true)
    }

    fn reclaim_where(&self, matches: impl Fn(&OwnedChild) -> bool) -> Vec<u32> {
        let mut children = self.children.lock().unwrap();
        let all: Vec<OwnedChild> = children.drain(..).collect();
        let mut reclaimed = Vec::new();
        let mut kept = Vec::new();
        for mut owned in all {
            if matches(&owned) {
                let _ = owned.child.kill();
                let _ = owned.child.wait();
                reclaimed.push(owned.pid);
            } else {
                kept.push(owned);
            }
        }
        *children = kept;
        reclaimed
    }
}

impl Default for OwnedProcesses {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn grok() -> ProviderKind {
        ProviderKind::GrokSubscription
    }

    #[test]
    fn helper_home_stays_inside_the_app_owned_directory() {
        let home = helper_home(&grok(), "grok", Path::new("/tmp/fixture-home")).unwrap();
        assert_eq!(
            home,
            PathBuf::from("/tmp/fixture-home/.autojev/subscription-helpers/grok_subscription/grok/home")
        );
        // API 服务商没有辅助进程存储；目录名不允许逃逸。
        assert!(helper_home(&ProviderKind::OpenaiCompatible, "grok", Path::new("/tmp")).is_err());
        assert!(helper_home(&grok(), "../escape", Path::new("/tmp")).is_err());
        assert!(helper_home(&grok(), "", Path::new("/tmp")).is_err());
    }

    #[test]
    fn prepare_home_creates_0700_and_refuses_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper_home(&grok(), "grok", directory.path()).unwrap();
        prepare_home(&home).unwrap();
        let mode = fs::metadata(&home).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "helper home must be 0700, got {mode:o}");
        // 重复准备是幂等的。
        prepare_home(&home).unwrap();

        let link = directory.path().join("linked-home");
        std::os::unix::fs::symlink(&home, &link).unwrap();
        assert!(prepare_home(&link).is_err(), "a symlinked helper home must be refused");
    }

    #[test]
    fn cleanup_home_only_clears_its_own_contents() {
        let directory = tempfile::tempdir().unwrap();
        let home = helper_home(&grok(), "grok", directory.path()).unwrap();
        prepare_home(&home).unwrap();
        fs::create_dir_all(home.join("accounts")).unwrap();
        fs::write(home.join("accounts/credentials.json"), "fixture").unwrap();
        fs::write(home.join("session"), "fixture").unwrap();
        cleanup_home(&home).unwrap();
        assert!(home.is_dir(), "the helper home directory itself is kept");
        assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
        // 目录不存在时视为已清理。
        cleanup_home(&home).unwrap();
    }

    #[test]
    fn cleanup_home_refuses_directories_it_does_not_own() {
        let directory = tempfile::tempdir().unwrap();
        assert!(cleanup_home(directory.path()).is_err());
        let foreign = directory.path().join("subscription-helpers/grok/not-home");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("keep"), "untouched").unwrap();
        assert!(cleanup_home(&foreign).is_err());
        assert!(foreign.join("keep").is_file(), "a refused cleanup must not delete anything");
    }

    #[test]
    fn redact_removes_credentials_and_keeps_identifiers() {
        // ① 合法长标识、普通路径、邮箱与普通文本原样保留（自由文本门限是 >= 32 连续 run 字符）。
        for kept in [
            "codex-subscription-openai",
            "claude-3-5-sonnet-20241022",
            "fixture@example.invalid",
            "/tmp/fixture/subscription-helpers/home",
            "Grok helper is missing",
            "abcdefghij0123456789klm",          // 23
            "abcdefghij0123456789klmnopqrstu",  // 31
        ] {
            assert_eq!(redact(kept), kept, "{kept} must stay untouched");
        }
        // ② 赋值形式：键命中凭据片段时，值无论多短都遮盖。
        assert_eq!(redact("device_code=ABCD1234EFGH"), "device_code=[redacted]");
        assert_eq!(redact("access_token=short"), "access_token=[redacted]");
        assert_eq!(redact("token=fixture-secret-value"), "token=[redacted]");
        assert_eq!(redact("XAI_API_KEY=fixture"), "XAI_API_KEY=[redacted]");
        assert_eq!(redact("device_code=ABCD-EFGH"), "device_code=[redacted]");
        assert_eq!(redact("id_token=fixture"), "id_token=[redacted]");
        assert_eq!(redact("client_secret:fixture"), "client_secret:[redacted]");
        assert_eq!(redact("\"refresh_token\":\"fixture\""), "\"refresh_token\":[redacted]");
        // ③ 自由文本里的 >= 32 连续随机串仍遮盖（31 与 23 见 ①）。
        let long_run = "a".repeat(32);
        assert_eq!(redact(&format!("helper said {long_run}")), "helper said [redacted]");
        let long = "xai-".to_owned() + &"a1b2c3d4e5".repeat(5);
        assert_eq!(redact(&format!("helper said {long}")), "helper said [redacted]");
        let with_space = redact(&format!("client_secret: {long_run}"));
        assert!(!with_space.contains(&long_run), "{with_space}");
        assert!(with_space.contains("[redacted]"), "{with_space}");
        // ④ 错误文本里的 token= 与 Bearer 仍遮盖（Bearer 后面的片段不看长度）。
        assert_eq!(redact("login failed: token=abc Bearer xyz"), "login failed: token=[redacted] Bearer [redacted]");
        assert_eq!(
            redact("Authorization: Bearer fixture-credential"),
            "Authorization:[redacted] Bearer [redacted]"
        );
    }

    #[test]
    fn only_the_grok_override_prefix_is_passed_through() {
        assert!(is_helper_override("AUTOJEV_GROK_HELPER"));
        assert!(is_helper_override("AUTOJEV_GROK_HOME"));
        assert!(!is_helper_override("AUTOJEV_GROK"));
        assert!(!is_helper_override("AUTOJEV_HOME"));
        assert!(!is_helper_override("AUTOJEV_CLOUD_KEY"));
        assert!(!is_helper_override("XAI_API_KEY"));
        assert!(!is_helper_override("PATH"));
    }

    #[test]
    fn the_helper_spec_uses_the_resolved_program_and_a_whitelisted_environment() {
        // 覆盖值可以带参数；空覆盖值回落到 PATH。
        let (program, args) = {
            let resolved = PathBuf::from("/tmp/fixture-helper");
            (resolved, vec!["--device".to_owned()])
        };
        let spec = spec_with_program(&grok(), "grok", Path::new("/tmp/fixture-home"), &program, &args).unwrap();
        assert_eq!(spec.args, vec!["--device".to_owned(), "login".to_owned()]);
        assert_eq!(spec.program, program);
        assert_eq!(spec.home, PathBuf::from("/tmp/fixture-home/.autojev/subscription-helpers/grok_subscription/grok/home"));
        let helper_home = spec.home.display().to_string();
        assert!(spec.env.iter().any(|(key, value)| key == "GROK_HOME" && value == &helper_home));
        // HOME 必须指向应用自有 home：真实 CLI 若只认 HOME，也不能落到日常 ~/.grok。
        assert!(spec.env.iter().any(|(key, value)| key == "HOME" && value == &helper_home));
        let real_home = std::env::var("HOME").unwrap_or_default();
        assert!(!real_home.is_empty());
        assert!(
            !spec.env.iter().any(|(_, value)| value == &real_home),
            "the real user home must not reach the helper environment"
        );
        assert!(
            !spec.env.iter().any(|(key, _)| key == "OPENAI_API_KEY" || key == "XAI_API_KEY" || key == "HTTP_PROXY"),
            "credential-bearing environment variables must not reach the helper"
        );

        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("grok"), "#!/bin/sh\n").unwrap();
        let found = find_on_path("grok", directory.path().to_str().unwrap()).unwrap();
        assert_eq!(found, directory.path().join("grok"));
        assert!(find_on_path("grok", "/nonexistent-fixture-dir").is_none());
    }

    #[test]
    fn ensure_spawn_allowed_rejects_isolated_and_permits_normal_builds() {
        let refused = ensure_spawn_allowed(true).unwrap_err();
        assert!(refused.to_string().contains("isolated"), "{refused}");
        assert!(ensure_spawn_allowed(false).is_ok());
    }

    #[test]
    fn owned_processes_spawn_stream_and_reclaim_only_their_own_pids() {
        use std::io::BufRead;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake-helper.sh");
        fs::write(&script, "#!/bin/sh\nprintf '%s\\n' 'first'\nsleep 0.3\nprintf '%s\\n' 'second'\nexec sleep 30\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let spec = spec_with_program(&grok(), "grok", directory.path(), &script, &[]).unwrap();
        prepare_home(&spec.home).unwrap();
        let processes = OwnedProcesses::new();
        let pid = processes.spawn("grok", &spec).unwrap();
        assert!(pid > 0);
        let mut reader = std::io::BufReader::new(processes.take_stdout(pid).expect("helper stdout"));
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "first");
        // 只回收自有 pid：别家服务商不匹配，自有进程仍活着（能读出第二行）。
        assert!(processes.reclaim("other").is_empty());
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "second", "reclaim 别家服务商不得杀掉自有进程");
        assert_eq!(processes.reclaim("grok"), vec![pid]);
        // 已回收的 pid 不再持有读取端，也绝不对未知 pid 做任何事。
        assert!(processes.take_stdout(pid).is_none());
        assert!(processes.reclaim_all().is_empty());
    }
}