//! Explicit validation profile; never selected by ordinary dev startup.
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

static ISOLATION_ROOT: OnceLock<PathBuf> = OnceLock::new();
static UPSTREAM: OnceLock<reqwest::Url> = OnceLock::new();
static GATEWAY_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
#[cfg(feature = "isolation-check")]
static CPA_VALIDATION_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

#[cfg(feature = "isolation-check")]
pub fn cpa_validation_port(port: u16) { CPA_VALIDATION_PORT.store(port, std::sync::atomic::Ordering::SeqCst); }

#[cfg(feature = "isolation-check")]
pub fn check_cpa_upstream(url: &reqwest::Url) -> Result<()> {
    ensure!(UPSTREAM.get().is_some_and(|upstream| url.origin() == upstream.origin()), "CPA upstream must be the explicitly configured loopback fixture");
    Ok(())
}
/// `--autojev-helper <path>` 只在 isolation-check 构建里存在；生产二进制没有这个开关。
#[cfg(feature = "isolation-check")]
static HELPER_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();
/// `--autojev-grok-helper <path>`：Grok **只读**读取的隔离替身，同样只在 isolation-check 构建里存在。
/// 它不适用于 Grok 登录路径（`GrokCliAuth`）：隔离下登录仍必须拒绝。
#[cfg(feature = "isolation-check")]
static GROK_HELPER_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

pub fn init() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--autojev-isolated") {
        ensure!(
            cfg!(debug_assertions),
            "Isolation is available only in development builds"
        );
        let root = args
            .get(index + 1)
            .context("--autojev-isolated requires a temporary directory")?;
        let upstream = args
            .iter()
            .position(|arg| arg == "--autojev-upstream")
            .and_then(|index| args.get(index + 1))
            .context("Isolation requires an explicit --autojev-upstream loopback fixture URL")?;
        let upstream = reqwest::Url::parse(&upstream.to_string_lossy())?;
        crate::dispatch::ensure_loopback(&upstream)?;
        ensure!(
            !matches!(upstream.port_or_known_default(), Some(9526 | 9527 | 11434)),
            "Use an independent fixture port"
        );
        let root = prepare_root(Path::new(root))?;
        UPSTREAM
            .set(upstream)
            .map_err(|_| anyhow::anyhow!("Isolation already initialized"))?;
        ISOLATION_ROOT
            .set(root)
            .map_err(|_| anyhow::anyhow!("Isolation already initialized"))?;
    }
    #[cfg(feature = "isolation-check")]
    if let Some(index) = args.iter().position(|arg| arg == "--autojev-helper") {
        // 覆盖辅助进程可执行文件只服务于隔离验收：必须已经进入隔离模式，且必须是绝对路径的真实文件。
        ensure!(
            isolated(),
            "--autojev-helper requires --autojev-isolated"
        );
        let path = PathBuf::from(
            args.get(index + 1)
                .context("--autojev-helper requires an absolute path")?,
        );
        ensure!(path.is_absolute(), "--autojev-helper requires an absolute path");
        ensure!(
            path.is_file(),
            "--autojev-helper must point at an existing helper executable"
        );
        HELPER_OVERRIDE
            .set(path)
            .map_err(|_| anyhow::anyhow!("Isolation already initialized"))?;
    }
    #[cfg(feature = "isolation-check")]
    if let Some(index) = args.iter().position(|arg| arg == "--autojev-grok-helper") {
        // 与 --autojev-helper 同一套约束：只在隔离验收里、必须是绝对路径的真实文件。
        // 这条开关只服务 Grok 只读读取；Grok 登录路径绝不使用它。
        ensure!(
            isolated(),
            "--autojev-grok-helper requires --autojev-isolated"
        );
        let path = PathBuf::from(
            args.get(index + 1)
                .context("--autojev-grok-helper requires an absolute path")?,
        );
        ensure!(path.is_absolute(), "--autojev-grok-helper requires an absolute path");
        ensure!(
            path.is_file(),
            "--autojev-grok-helper must point at an existing helper executable"
        );
        GROK_HELPER_OVERRIDE
            .set(path)
            .map_err(|_| anyhow::anyhow!("Isolation already initialized"))?;
    }
    Ok(())
}

fn prepare_root(root: &Path) -> Result<PathBuf> {
    ensure!(root.is_absolute(), "Isolation directory must be absolute");
    let root = root
        .canonicalize()
        .context("Create an empty temporary directory first")?;
    let temp = std::env::temp_dir().canonicalize()?;
    let alternate = Path::new("/tmp").canonicalize().ok();
    ensure!(
        (root.starts_with(&temp) && root != temp)
            || alternate.is_some_and(|temp| root.starts_with(&temp) && root != temp),
        "Isolation directory must be inside the system temporary directory"
    );
    let marker = root.join(".autojev-isolated");
    if marker.exists() {
        ensure!(
            fs::read_to_string(&marker)? == "AutoJev isolated validation v1\n",
            "Unrecognized isolation directory"
        );
        reject_symlinks(&root)?;
    } else {
        ensure!(
            fs::read_dir(&root)?.next().is_none(),
            "First isolation startup requires an empty directory"
        );
        use std::io::Write;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)?
            .write_all(b"AutoJev isolated validation v1\n")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    }
    Ok(root)
}

fn reject_symlinks(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "Isolation directory must not contain symlinks"
        );
        if kind.is_dir() {
            reject_symlinks(&entry.path())?;
        }
    }
    Ok(())
}

pub fn isolated() -> bool {
    ISOLATION_ROOT.get().is_some()
}
/// 隔离验收显式指定的辅助进程可执行文件；生产构建里这个入口不存在。
#[cfg(feature = "isolation-check")]
pub fn helper_override() -> Option<&'static Path> {
    HELPER_OVERRIDE.get().map(PathBuf::as_path)
}

/// 隔离验收显式指定的 Grok 只读替身；生产构建里这个入口不存在（恒为 `None`）。
/// 它只允许 Grok 只读读取在隔离下拉起这一个可执行文件，绝不放开登录路径或通用拉起守卫。
#[cfg(feature = "isolation-check")]
pub fn grok_helper_override() -> Option<&'static Path> {
    GROK_HELPER_OVERRIDE.get().map(PathBuf::as_path)
}

/// 非隔离验收构建（含生产）没有 pinned Grok 只读替身：隔离下的 Grok 只读仍然如实拒绝。
#[cfg(not(feature = "isolation-check"))]
pub fn grok_helper_override() -> Option<&'static Path> {
    None
}
pub fn home_dir() -> Option<PathBuf> {
    ISOLATION_ROOT.get().cloned().or_else(dirs::home_dir)
}

pub fn gateway_port(port: u16) {
    GATEWAY_PORT.store(port, std::sync::atomic::Ordering::SeqCst);
}
pub fn check_url(url: &reqwest::Url) -> Result<()> {
    if let Some(upstream) = UPSTREAM.get() {
        let gateway = GATEWAY_PORT.load(std::sync::atomic::Ordering::SeqCst);
        #[cfg(feature = "isolation-check")]
        if url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && url.port().is_some_and(|port| port != 0 && port == CPA_VALIDATION_PORT.load(std::sync::atomic::Ordering::SeqCst)) { return Ok(()); }
        ensure!(
            url.origin() == upstream.origin()
                || (gateway != 0
                    && url.host_str() == Some("127.0.0.1")
                    && url.port() == Some(gateway)),
            "Isolated validation target is outside the owned fixture and gateway ports"
        );
    }
    Ok(())
}

pub fn external_action() -> Result<()> {
    ensure!(
        !isolated(),
        "External application actions are disabled in isolated validation"
    );
    Ok(())
}

pub fn check_path(path: &Path) -> Result<()> {
    if let Some(root) = ISOLATION_ROOT.get() {
        ensure!(
            path.is_absolute()
                && !path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir)),
            "Invalid isolated path"
        );
        let mut existing = path;
        while !existing.exists() {
            existing = existing.parent().context("Invalid isolated path")?;
        }
        ensure!(
            path.starts_with(root) && existing.canonicalize()?.starts_with(root),
            "Agent configuration must stay inside the isolation directory"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn isolation_refuses_existing_data_and_allows_owned_restart() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("personal-data"), "untouched").unwrap();
        assert!(super::prepare_root(root.path()).is_err());
        let root = tempfile::tempdir().unwrap();
        assert!(super::prepare_root(root.path()).is_ok());
        std::fs::write(root.path().join("fixture.db"), "fixture").unwrap();
        assert!(super::prepare_root(root.path()).is_ok());
    }
}
