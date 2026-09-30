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
pub fn home_dir() -> Option<PathBuf> {
    ISOLATION_ROOT.get().cloned().or_else(dirs::home_dir)
}

pub fn gateway_port(port: u16) {
    GATEWAY_PORT.store(port, std::sync::atomic::Ordering::SeqCst);
}
pub fn check_url(url: &reqwest::Url) -> Result<()> {
    if let Some(upstream) = UPSTREAM.get() {
        let gateway = GATEWAY_PORT.load(std::sync::atomic::Ordering::SeqCst);
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
