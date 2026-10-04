//! Own one pinned CPA process and persistent private profile per connection.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

#[derive(Serialize, Deserialize)]
struct Profile {
    instance: String,
    binary: String,
    port: u16,
    management_key: String,
    model_key: String,
}
pub struct Service {
    child: Child,
    profile: Profile,
    root: PathBuf,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct View {
    pub running: bool,
    pub pid: u32,
    pub port: u16,
    pub binary: String,
    pub artifact_sha256: String,
    #[serde(default)]
    pub evidence_file: String,
}
pub fn artifact_sha() -> String {
    serde_json::from_str::<serde_json::Value>(include_str!("../../../scripts/cpa-artifact.json"))
        .unwrap()["binary_sha256"]
        .as_str()
        .unwrap()
        .into()
}
fn write_private(path: &Path, value: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.is_file(),
            "Owned profile file cannot be a symlink or directory"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            ensure!(
                metadata.nlink() == 1,
                "Owned profile file cannot be shared by hard links"
            );
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(value)?;
    Ok(())
}
impl Service {
    pub async fn start(root: PathBuf, instance: &str, binary: String, port: u16) -> Result<Self> {
        let executable = Path::new(&binary);
        ensure!(
            executable.is_absolute() && executable.is_file(),
            "CPA artifact executable is missing"
        );
        ensure!(
            format!("{:x}", Sha256::digest(fs::read(executable)?)) == artifact_sha(),
            "CPA artifact version/checksum mismatch"
        );
        ensure!(
            uuid::Uuid::parse_str(instance).is_ok(),
            "Invalid owned profile instance"
        );
        let root = root.join(instance);
        fs::create_dir_all(&root)?;
        ensure!(
            !fs::symlink_metadata(&root)?.file_type().is_symlink(),
            "Owned profile cannot be a symlink"
        );
        let file = root.join("profile.json");
        if file.exists() {
            ensure!(
                fs::symlink_metadata(&file)?.is_file(),
                "Owned profile manifest must be a regular file"
            );
        }
        let mut profile: Profile = if file.exists() {
            serde_json::from_slice(&fs::read(&file)?)?
        } else {
            Profile {
                instance: instance.into(),
                binary: binary.clone(),
                port,
                management_key: uuid::Uuid::new_v4().to_string(),
                model_key: uuid::Uuid::new_v4().to_string(),
            }
        };
        ensure!(
            profile.instance == instance && profile.binary == binary,
            "Owned artifact binding changed; create a new connection"
        );
        ensure!(
            profile.port == 0 || port == profile.port,
            "Recover the saved port; a different port requires a new connection"
        );
        let listener = std::net::TcpListener::bind(("127.0.0.1", profile.port))
            .context("CPA port is already occupied")?;
        profile.port = listener.local_addr()?.port();
        let auth = root.join("auth");
        fs::create_dir_all(&auth)?;
        ensure!(
            !fs::symlink_metadata(&auth)?.file_type().is_symlink(),
            "Owned auth directory cannot be a symlink"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&root, &auth] {
                fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
            }
        }
        write_private(&file, &serde_json::to_vec(&profile)?)?;
        let config = json!({"config-version":8,
            "server":{"host":"127.0.0.1","port":profile.port,"discovery":{"enabled":false}},
            "management":{"allow-remote":false,"secret-key":profile.management_key,"disable-control-panel":true,"disable-auto-update-panel":true},
            "access":{"api-keys":[profile.model_key]}, "oauth":{"auth-dir":auth},
            "routing":{"force-model-prefix":true,"session-affinity":false,"retry":{"request-retry":0,"max-retry-credentials":1,"max-retry-interval":0}},
            "requests":{"streaming":{"bootstrap-retries":0}}});
        write_private(&root.join("config.json"), &serde_json::to_vec(&config)?)?;
        let log_path = root.join("service.log");
        if log_path.exists() {
            ensure!(
                fs::symlink_metadata(&log_path)?.is_file(),
                "Owned log must be a regular file"
            );
        }
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("service.log"), fs::Permissions::from_mode(0o600))?;
        }
        drop(listener);
        let mut command = if cfg!(test) || crate::runtime::loopback_only() {
            let mut c = Command::new("/usr/bin/sandbox-exec");
            c.args(["-p", "(version 1)(allow default)(deny network*)(allow network* (remote ip \"localhost:*\"))(allow network* (local ip \"localhost:*\"))"]);
            c.arg(&profile.binary);
            c
        } else {
            Command::new(&profile.binary)
        };
        let child = command
            .args([
                "--config",
                root.join("config.json")
                    .to_str()
                    .context("Invalid config path")?,
                "--local-model",
            ])
            .env_clear()
            .env("HOME", &root)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .context("Start owned CPA")?;
        let mut service = Self {
            child,
            profile,
            root,
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(1))
            .build()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            ensure!(service.running()?, "Owned CPA exited before readiness");
            if let Ok(response) = client
                .get(format!("{}/v8/management/config", service.base()))
                .bearer_auth(service.management_key())
                .send()
                .await
            {
                if response.status().is_success() {
                    ensure!(
                        response
                            .headers()
                            .get("x-cpa-commit")
                            .and_then(|h| h.to_str().ok())
                            == Some("e2bff0107bb307337aaa19018ccddd55f64253d5"),
                        "CPA runtime version mismatch"
                    );
                    return Ok(service);
                }
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "Owned CPA readiness timed out"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    pub fn running(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.profile.port)
    }
    pub fn management_key(&self) -> String {
        self.profile.management_key.clone()
    }
    pub fn model_key(&self) -> String {
        self.profile.model_key.clone()
    }
    pub fn view(&mut self) -> Result<View> {
        Ok(View {
            running: self.running()?,
            pid: self.child.id(),
            port: self.profile.port,
            binary: self.profile.binary.clone(),
            artifact_sha256: artifact_sha(),
            evidence_file: self
                .root
                .join("hand-run.reviewed.json")
                .to_string_lossy()
                .into_owned(),
        })
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.running()? {
            self.child.kill()?;
        }
        self.child.wait()?;
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
