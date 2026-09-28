//! A separate, minimal process restores agent configurations if the desktop process dies.
//! Instance locks prevent crash recovery racing a newly started gateway.
use anyhow::{Context, Result};
use std::{fs, path::PathBuf};

pub fn root() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("Cannot locate home directory")?
        .join(".autojev"))
}
pub fn lock(port: u16) -> Result<fs::File> {
    let root = root()?;
    for _ in 0..20 {
        if let Ok(file) = lock_at(port, &root) {
            return Ok(file);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    lock_at(port, &root)
}
fn lock_at(port: u16, root: &std::path::Path) -> Result<fs::File> {
    fs::create_dir_all(root)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(format!("gateway-{port}.lock")))?;
    file.try_lock()
        .context("This AutoJev gateway is already running or recovering")?;
    Ok(file)
}
pub fn notice(port: u16) -> Option<String> {
    fs::read_to_string(root().ok()?.join(format!("recovery-{port}.txt"))).ok()
}
pub fn spawn_watchdog(port: u16) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(std::env::current_exe()?)
            .args([
                "--autojev-watchdog",
                &std::process::id().to_string(),
                &port.to_string(),
            ])
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Start gateway recovery watchdog")?;
    }
    Ok(())
}
pub fn watchdog_entry() -> bool {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) != Some("--autojev-watchdog") {
        return false;
    }
    #[cfg(unix)]
    {
        let Some((pid, port)) = args
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .zip(args.get(3).and_then(|s| s.parse::<u16>().ok()))
        else {
            return true;
        };
        if !matches!(port, crate::config::DEV_PORT | crate::config::DEFAULT_PORT) || pid < 2 {
            return true;
        }
        if let Some(home) = dirs::home_dir() {
            watch_parent(pid, port, &home);
        }
    }
    true
}
#[cfg(test)]
mod tests {
    #[test]
    fn exclusive_file_lock_prevents_recovery_race() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("gateway.lock");
        let a = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        a.try_lock().unwrap();
        let b = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(b.try_lock().is_err());
        drop(a);
        b.try_lock().unwrap();
    }
}

#[cfg(unix)]
fn watch_parent(pid: u32, port: u16, home: &std::path::Path) {
    while std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    // A replacement instance owns recovery if it acquired the lock first.
    let Ok(_lock) = lock_at(port, &home.join(".autojev")) else {
        return;
    };
    // Do not touch a gateway launched by an older version without an instance lock.
    if std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(250),
    )
    .is_ok()
    {
        return;
    }
    let result = crate::agents::restore_gateway_at(port, home);
    let message = match result {
        Ok(()) => format!(
            "{}: Agent configuration recovery completed.",
            chrono::Utc::now().to_rfc3339()
        ),
        Err(e) => format!(
            "{}: Agent recovery needs attention: {e}",
            chrono::Utc::now().to_rfc3339()
        ),
    };
    {
        let root = home.join(".autojev");
        let _ = crate::ownership::atomic(
            &root.join(format!("recovery-{port}.txt")),
            message.as_bytes(),
        );
    }
}

#[cfg(all(test, unix))]
mod recovery_process_tests {
    #[test]
    fn parent_death_recovers_owned_files_in_isolated_home() {
        let home = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let path = home.path().join(".codex/config.toml");
        let applied=format!("model = 'autojev/test'\n[model_providers.autojev]\nbase_url = 'http://127.0.0.1:{port}/v1'\n");
        crate::ownership::apply(
            &[path.clone()],
            port,
            &[Some("model = 'original'\n".into())],
            &[applied],
        )
        .unwrap();
        let mut parent = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = parent.id();
        let directory = home.path().to_owned();
        let watcher = std::thread::spawn(move || super::watch_parent(pid, port, &directory));
        parent.kill().unwrap();
        parent.wait().unwrap();
        watcher.join().unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "model = 'original'\n"
        );
        assert!(
            std::fs::read_to_string(home.path().join(format!(".autojev/recovery-{port}.txt")))
                .unwrap()
                .contains("completed")
        );
    }
}
