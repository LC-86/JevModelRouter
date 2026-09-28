//! Explicit configuration templates for user-defined executables.
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Injection {
    pub template: String,
    pub path: String,
    pub api: String,
}
impl Injection {
    pub fn resolve(&self, home: &Path) -> Result<PathBuf> {
        let path = self.path.trim();
        let path = if let Some(relative) = path.strip_prefix("~/") { home.join(relative) } else { PathBuf::from(path) };
        if !path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(anyhow!("Use an absolute configuration path or ~/ path"));
        }
        let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("");
        let valid = match self.template.as_str() {
            "kilo" | "opencode" | "openclaw" => matches!(extension, "json" | "jsonc"),
            "hermes" => matches!(extension, "yaml" | "yml"),
            "grok" | "kimi" => extension == "toml",
            _ => false,
        };
        if !valid { return Err(anyhow!("Configuration extension must match the selected template")); }
        if !matches!(self.api.as_str(), "chat_completions" | "messages") || (self.template == "hermes" && self.api != "chat_completions") {
            return Err(anyhow!("Unsupported API for the selected template"));
        }
        Ok(if path.exists() { path.canonicalize()? } else { path })
    }
}
/// Infer only known client layouts, never assume arbitrary JSON/TOML files share a schema.
pub fn infer(command: &str, requested_path: &str, home: &Path) -> Result<Option<Injection>> {
    let binary = Path::new(command.trim()).file_name().and_then(|s| s.to_str()).unwrap_or("").trim_end_matches(".exe").to_ascii_lowercase();
    let path = requested_path.trim();
    let file = Path::new(path).file_name().and_then(|s| s.to_str()).unwrap_or("");
    let known = ["kilo", "opencode", "openclaw", "hermes", "grok", "kimi"];
    let kind = if known.contains(&binary.as_str()) { binary.as_str() } else {
        match file { "kilo.json" | "kilo.jsonc" => "kilo", "opencode.json" | "opencode.jsonc" => "opencode", "openclaw.json" => "openclaw", _ => return Ok(None) }
    };
    let default = match kind {
        "kilo" => "~/.config/kilo/kilo.jsonc",
        "opencode" => "~/.config/opencode/opencode.json",
        "openclaw" => "~/.openclaw/openclaw.json",
        "hermes" => "~/.hermes/config.yaml",
        "grok" => "~/.grok/config.toml",
        "kimi" => "~/.kimi/config.toml",
        _ => unreachable!(),
    };
    let mut resolved_path = if path.is_empty() { default.to_owned() } else { path.to_owned() };
    if path.is_empty() && matches!(kind, "kilo" | "opencode") {
        let alternate = if default.ends_with("jsonc") { default.trim_end_matches('c').to_owned() } else { format!("{default}c") };
        let primary = home.join(default.trim_start_matches("~/"));
        if !primary.exists() && home.join(alternate.trim_start_matches("~/")).exists() { resolved_path = alternate; }
    }
    let injection = Injection { template: kind.into(), path: resolved_path, api: "chat_completions".into() };
    injection.resolve(home)?;
    Ok(Some(injection))
}
fn registry(home: &Path, port: u16) -> PathBuf { home.join(format!(".autojev/custom-injections-{port}.json")) }
pub fn registered(home: &Path, port: u16) -> Result<Vec<PathBuf>> {
    match fs::read(registry(home, port)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}
pub fn connect(injection: &Injection, home: &Path, port: u16, binding: &str, agent_id: &str) -> Result<()> {
    let path = injection.resolve(home)?;
    if crate::ownership::owner(&path)?.is_some_and(|owner| owner != port) {
        return Err(anyhow!("Configuration is owned by another gateway instance"));
    }
    let original = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let output = crate::agent_adapters::render_for_agent(if injection.template == "kilo" { "opencode" } else { &injection.template }, agent_id, &[original.clone().unwrap_or_default()], port, binding, &injection.api)?;
    let mut paths = registered(home, port)?;
    if !paths.contains(&path) { paths.push(path.clone()); }
    // Persist recovery discovery before touching the client configuration, including crash recovery.
    crate::ownership::atomic(&registry(home, port), &serde_json::to_vec(&paths)?)?;
    crate::ownership::apply(&[path], port, &[original], &output).context("Inject custom agent configuration")
}
pub fn owned(injection: &Injection, home: &Path, port: u16) -> bool {
    injection.resolve(home).is_ok_and(|p| crate::ownership::owns(&p, port))
}
pub fn restore(injection: &Injection, home: &Path, port: u16) -> Result<()> {
    crate::ownership::restore(&injection.resolve(home)?, port)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates_inject_restore_and_recover_without_losing_user_edits() {
        for (template, file, original) in [
            ("kilo", "agent.jsonc", "{\"theme\":\"dark\"}"),
            ("opencode", "agent.json", "{\"theme\":\"dark\"}"),
            ("openclaw", "agent.json", "{\"theme\":\"dark\"}"),
            ("hermes", "agent.yaml", "theme: dark\n"),
            ("grok", "agent.toml", "theme = 'dark'\n"),
            ("kimi", "agent.toml", "theme = 'dark'\n"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(file);
            fs::write(&path, original).unwrap();
            let injection = Injection { template: template.into(), path: path.to_string_lossy().into(), api: "chat_completions".into() };
            connect(&injection, dir.path(), 9526, "autojev/fast", "custom-test").unwrap();
            assert!(owned(&injection, dir.path(), 9526));
            let applied = fs::read_to_string(&path).unwrap();
            assert!(applied.contains("autojev/fast") && applied.contains("127.0.0.1:9526"));
            assert!(applied.contains("autojev-local-custom-test"), "{template}");
            fs::write(&path, applied.replace("dark", "light")).unwrap();
            connect(&injection, dir.path(), 9526, "autojev/quality", "custom-test").unwrap();
            assert_eq!(registered(dir.path(), 9526).unwrap(), vec![path.canonicalize().unwrap()]);
            // Uses the same gateway-wide restoration path as stop, exit and watchdog recovery.
            crate::agents::restore_gateway_at(9526, dir.path()).unwrap();
            let restored = fs::read_to_string(&path).unwrap();
            assert!(restored.contains("light") && !restored.contains("127.0.0.1:9526"));
            assert!(!owned(&injection, dir.path(), 9526));
        }
    }
    #[test]
    fn auto_detection_uses_client_identity_and_preserves_explicit_paths() {
        let dir = tempfile::tempdir().unwrap();
        let kilo = infer("/Users/test/.kilo/bin/kilo", "", dir.path()).unwrap().unwrap();
        assert_eq!(kilo.template, "kilo");
        assert_eq!(kilo.path, "~/.config/kilo/kilo.jsonc");
        assert_eq!(kilo.api, "chat_completions");
        let custom = dir.path().join("client.jsonc").to_string_lossy().into_owned();
        assert_eq!(infer("kilo", &custom, dir.path()).unwrap().unwrap().path, custom);
        assert!(infer("kilo", "~/wrong.toml", dir.path()).is_err());
        assert!(infer("unknown", "~/config.json", dir.path()).unwrap().is_none());
        assert!(infer("unknown", "", dir.path()).unwrap().is_none());
        assert_eq!(infer("wrapper", "~/kilo.jsonc", dir.path()).unwrap().unwrap().template, "kilo");
        fs::create_dir_all(dir.path().join(".config/kilo")).unwrap();
        fs::write(dir.path().join(".config/kilo/kilo.json"), "{}").unwrap();
        assert_eq!(infer("kilo", "", dir.path()).unwrap().unwrap().path, "~/.config/kilo/kilo.json");
    }
    #[test]
    fn malformed_input_and_other_instance_are_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.json");
        let injection = Injection { template: "opencode".into(), path: path.to_string_lossy().into(), api: "chat_completions".into() };
        fs::write(&path, "{broken").unwrap();
        assert!(connect(&injection, dir.path(), 9526, "autojev/fast", "custom-test").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
        fs::remove_file(&path).unwrap();
        connect(&injection, dir.path(), 9526, "autojev/fast", "custom-test").unwrap();
        assert!(connect(&injection, dir.path(), 9527, "autojev/fast", "custom-test").is_err());
        restore(&injection, dir.path(), 9526).unwrap();
        assert!(!path.exists());
        assert!(Injection { path: "relative.json".into(), ..injection.clone() }.resolve(dir.path()).is_err());
        assert!(Injection { template: "unknown".into(), ..injection }.resolve(dir.path()).is_err());
    }
}
