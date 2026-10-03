use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use serde::{Serialize, Deserialize};
use serde_json::{json, Value};
use toml_edit::{value, DocumentMut, Item, Table};

#[derive(Clone, Debug, Serialize)]
pub struct AgentStatus {
    pub args: Vec<String>,
    pub injection: Option<crate::custom_agents::Injection>,
    pub icon: Option<String>,
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub connected: bool,
    pub config_path: String,
    pub detail: String,
    pub route_id: Option<String>,
    pub command: String,
    pub executable_path: Option<String>,
    pub custom: bool,
    pub can_connect: bool,
    pub connection_note: Option<String>,
    /// 保存的目录（`agent_catalogs`）与按当前模型／路由／资格重算的目录不一致，且外部配置仍是旧的。
    /// 只作界面提示：后端准入与后端撤销都不等这个标记。
    pub catalog_pending_sync: bool,
}

/// 保存的 Agent 目录与此刻应当注入的目录是否已经不一致（#17：Agent 保存目录单独核对）。
/// 没有保存目录时没有可同步的对象，报 false；保存的目录已无法重建（候选失效）同样算待同步。
pub fn catalog_pending_sync(config: &crate::config::AppConfig, agent_id: &str) -> bool {
    let Some(saved) = config.agent_catalogs.get(agent_id).filter(|entries| !entries.is_empty()) else {
        return false;
    };
    let bindings: Vec<String> = saved.iter().map(|entry| entry.binding.clone()).collect();
    // 重建失败说明已保存的绑定（模型被取消选择／停用、路由失效等）不再可选：外部配置必然还是旧的。
    crate::agent_catalog::build(config, &bindings).map_or(true, |rebuilt| rebuilt != *saved)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomAgent {
    #[serde(default)]
    pub config_path: Option<String>,
    #[serde(default)]
    pub injection: Option<crate::custom_agents::Injection>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub id: String,
    pub name: String,
    pub command: String,
}
pub fn validate_custom(agent: &CustomAgent) -> Result<()> {
    if agent.name.trim().is_empty() || agent.command.trim().is_empty() {
        return Err(anyhow!("Agent name and executable are required"));
    }
    if !agent.id.starts_with("custom-") { return Err(anyhow!("Invalid custom agent ID")); }
    if let Some(injection) = &agent.injection { injection.resolve(&crate::runtime::home_dir().unwrap_or_default())?; }
    Ok(())
}
pub fn detect(custom: &[CustomAgent]) -> Vec<AgentStatus> {
    let mut result = vec![detect_codex(), detect_claude()];
    for (id, name, command) in [
        ("gemini", "Gemini", "gemini"), ("grok", "Grok Build", "grok"),
        ("openclaw", "OpenClaw", "openclaw"),
        ("hermes", "Hermes", "hermes"), ("opencode", "OpenCode", "opencode"),
        ("cursor", "Cursor", "cursor-agent"), ("kimi", "Kimi", "kimi"), ("omp", "OMP", "omp")
    ] {
        result.push(detect_executable(id, name, command, false));
    }
    result.extend(custom.iter().map(|a| { let mut status = detect_executable(&a.id, &a.name, &a.command, true); status.icon = a.icon.clone(); status.args = a.args.clone(); status.injection = a.injection.clone(); status.config_path = a.config_path.clone().unwrap_or_default();
        if let Some(injection) = &a.injection { status.config_path = injection.path.clone(); status.can_connect = injection.resolve(&crate::runtime::home_dir().unwrap_or_default()).is_ok(); }
        status }));
    result
}
fn detect_executable(id: &str, name: &str, command: &str, custom: bool) -> AgentStatus {
    let home = crate::runtime::home_dir().unwrap_or_default();
    let paths = crate::agent_adapters::paths(id, &home);
    let bound = crate::agent_adapters::binding(id, &home);
    let executable = executable_path(command);
    AgentStatus { args: vec![], injection: None, icon: None, id: id.into(), name: name.into(), command: command.into(),
        installed: executable.is_some(), executable_path: executable, connected: bound.is_some(),
        config_path: paths.iter().map(|p|display_home(p)).collect::<Vec<_>>().join(", "),
        detail: crate::agent_adapters::limitation(id).unwrap_or(command).into(),
        connection_note: crate::agent_adapters::limitation(id).map(str::to_owned),
        route_id: bound, custom, can_connect: crate::agent_adapters::supported(id), catalog_pending_sync: false }
}

pub fn connect(id: &str, port: u16, route_id: &str, api: &str) -> Result<()> {
    match id {
        "codex" => connect_codex(port, route_id),
        "claude" => connect_claude(port, route_id),
        _ => crate::agent_adapters::connect(id, port, route_id, api, &crate::runtime::home_dir().unwrap_or_default()),
    }
}

fn restore_at(id:&str,home:&Path)->Result<()> {
    if !matches!(id,"codex"|"claude") {return crate::agent_adapters::restore(id,home);}
    let path = match id {
        "codex" => home.join(".codex/config.toml"),
        "claude" => home.join(".claude/settings.json"),
        _ => return Err(anyhow!("Unsupported agent: {id}")),
    };
    if let Some(port)=crate::ownership::owner(&path)? {
        let catalog=home.join(".codex/autojev-models.json");
        if id=="codex" && crate::ownership::owns(&catalog,port) {
            crate::ownership::prepare(&path,port)?;
            crate::ownership::prepare(&catalog,port)?;
            crate::ownership::restore(&catalog,port)?;
        }
        crate::ownership::restore(&path,port)?;let backup=backup_path(&path);if backup.exists(){fs::remove_file(backup)?;}return Ok(());}
    let backup = backup_path(&path);
    if !backup.exists() {
        return Err(anyhow!("No AutoJev backup exists for {id}"));
    }
    fs::copy(&backup, &path).with_context(|| format!("restore {}", path.display()))?;
    fs::remove_file(&backup).context("remove used AutoJev backup")?;
    Ok(())
}

fn detect_codex() -> AgentStatus {
    let path = codex_path();
    let content = fs::read_to_string(&path).unwrap_or_default();
    AgentStatus {
        args: vec![], injection: None, icon: None, command: "codex".into(), executable_path: executable_path("codex"), custom: false, can_connect: true, connection_note: None,
        id: "codex".into(),
        name: "Codex".into(),
        installed: path.exists() || command_exists("codex"),
        connected: content.contains("model_provider = \"autojev\"")
            && content.contains("127.0.0.1:"),
        config_path: display_home(&path),
        detail: "OpenAI Responses API".into(),
        route_id: content.parse::<DocumentMut>().ok().and_then(|doc| doc.get("model").and_then(Item::as_str)
            .map(str::to_owned)),
        catalog_pending_sync: false,
    }
}

fn detect_claude() -> AgentStatus {
    let path = claude_path();
    let content = fs::read_to_string(&path).unwrap_or_default();
    AgentStatus {
        args: vec![], injection: None, icon: None, command: "claude".into(), executable_path: executable_path("claude"), custom: false, can_connect: true, connection_note: None,
        id: "claude".into(),
        name: "Claude Code".into(),
        installed: path.exists() || command_exists("claude"),
        connected: content.contains("ANTHROPIC_BASE_URL") && content.contains("127.0.0.1:") && (content.contains("autojev/") || content.contains("model/") || content.contains("route/")),
        config_path: display_home(&path),
        detail: "Anthropic Messages API".into(),
        route_id: serde_json::from_str::<Value>(&content).ok().and_then(|doc| doc.pointer("/env/ANTHROPIC_MODEL")
            .and_then(Value::as_str).map(str::to_owned)),
        catalog_pending_sync: false,
    }
}

fn connect_codex(port: u16, route_id: &str) -> Result<()> {
    connect_codex_catalog(port, route_id, &[crate::agent_catalog::Entry { binding: route_id.into(), id: crate::agent_catalog::wire_id(route_id), name: route_id.into() }], &crate::runtime::home_dir().context("Cannot locate home directory")?)
}

pub fn connect_codex_catalog(port: u16, route_id: &str, catalog: &[crate::agent_catalog::Entry], home: &Path) -> Result<()> {
    let path = home.join(".codex/config.toml");
    let catalog_path = home.join(".codex/autojev-models.json");
    ensure_parent(&path)?;
    migrate_before_connect(&path,port)?;
    backup_once(&path)?;
    let content = fs::read_to_string(&path).unwrap_or_default();
    let mut document = if content.trim().is_empty() {
        DocumentMut::new()
    } else {
        content
            .parse::<DocumentMut>()
            .context("parse existing Codex config.toml")?
    };
    document["model"] = value(&catalog.iter().find(|entry| entry.binding == route_id).context("Default model must be selected")?.id);
    document["model_provider"] = value("autojev");
    document["model_catalog_json"] = value(catalog_path.to_string_lossy().as_ref());
    // Hosted search is not portable to Chat Completions / Messages models.
    document["web_search"] = value("disabled");
    if !document.as_table().contains_key("model_providers") {
        document["model_providers"] = Item::Table(Table::new());
    }
    if !document["model_providers"]
        .as_table()
        .is_some_and(|table| table.contains_key("autojev"))
    {
        document["model_providers"]["autojev"] = Item::Table(Table::new());
    }
    document["model_providers"]["autojev"]["name"] = value("AutoJev");
    document["model_providers"]["autojev"]["base_url"] =
        value(format!("http://127.0.0.1:{port}/v1"));
    document["model_providers"]["autojev"]["wire_api"] = value("responses");
    let provider = document["model_providers"]["autojev"].as_table_like_mut().context("Invalid provider table")?;
    if provider.get("http_headers").is_none() { provider.insert("http_headers", Item::Table(Table::new())); }
    provider.get_mut("http_headers").and_then(Item::as_table_like_mut).context("Invalid provider headers")?
        .insert("x-autojev-agent", value("codex"));
    let models: Vec<Value> = catalog.iter().enumerate().map(|(index, entry)| json!({
        "slug": entry.id, "display_name": entry.name,
        "description": format!("AutoJev · {}",entry.binding),
        "default_reasoning_level":"medium", "supported_reasoning_levels":[],
        "shell_type":"unified_exec", "visibility":"list", "supported_in_api":true,
        "priority":index, "base_instructions":"You are a coding assistant. Use the available tools to complete the user's task.",
        "supports_reasoning_summaries":false, "support_verbosity":false,
        "default_verbosity":null, "apply_patch_tool_type":"freeform",
        "truncation_policy":{"mode":"tokens","limit":10000},
        "context_window":null, "effective_context_window_percent":95,
        "experimental_supported_tools":[], "input_modalities":["text","image"],
        "supports_search_tool":false, "use_responses_lite":false,
        "supports_reasoning_effort_updates":false
    })).collect();
    let originals = [fs::read_to_string(&path).ok(),fs::read_to_string(&catalog_path).ok()];
    crate::ownership::apply(&[path,catalog_path],port,&originals,&[document.to_string(),serde_json::to_string_pretty(&json!({"models":models}))?])
}

fn connect_claude(port: u16, route_id: &str) -> Result<()> {
    let entry = crate::agent_catalog::Entry { binding: route_id.into(), id: route_id.into(), name: route_id.into() };
    connect_claude_catalog_at(port, route_id, &[entry], &claude_path())
}
pub fn connect_claude_catalog(port: u16, binding: &str, catalog: &[crate::agent_catalog::Entry], home: &Path) -> Result<()> {
    connect_claude_catalog_at(port, binding, catalog, &home.join(".claude/settings.json"))
}
fn connect_claude_catalog_at(port: u16, binding: &str, catalog: &[crate::agent_catalog::Entry], path: &Path) -> Result<()> {
    let default = catalog.iter().find(|e| e.binding == binding).context("Default model must be selected")?;
    let route_id = default.id.as_str();
    ensure_parent(&path)?;
    migrate_before_connect(&path,port)?;
    backup_once(&path)?;
    let content = fs::read_to_string(&path).unwrap_or_default();
    let mut document: Value = if content.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&content).context("parse existing Claude settings.json")?
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings must be a JSON object"))?;
    object.insert("model".into(), json!(route_id));
    object.insert("modelPicker".into(), json!({
        "replaceBuiltInOptions": true,
        "options": catalog.iter().map(|entry| json!({
            "model": entry.id,
            "label": if entry.name.trim().is_empty() { &entry.id } else { &entry.name },
            "description": format!("AutoJev · {}", entry.id),
            // Client-side protocol/prompt compatibility only, never an upstream model rewrite.
            "behavesAs": "claude-sonnet-4-6"
        })).collect::<Vec<_>>()
    }));
    let env = object.entry("env").or_insert_with(|| json!({}));
    let env = env
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings env must be a JSON object"))?;
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        Value::String(format!("http://127.0.0.1:{port}")),
    );
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".into(),
        Value::String("autojev-local-claude".into()),
    );
    env.insert(
        "ANTHROPIC_MODEL".into(),
        Value::String(crate::agent_catalog::wire_id(route_id)),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_HAIKU_MODEL".into(),
        Value::String(crate::agent_catalog::wire_id(route_id)),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
        Value::String(crate::agent_catalog::wire_id(route_id)),
    );
    env.insert("ANTHROPIC_DEFAULT_FABLE_MODEL".into(), json!(route_id));
    env.insert(
        "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
        Value::String(crate::agent_catalog::wire_id(route_id)),
    );
    let output = serde_json::to_vec_pretty(&document)?;
    inject_owned(&path, port, std::str::from_utf8(&output)?)
}

fn codex_path() -> PathBuf {
    crate::runtime::home_dir()
        .unwrap_or_default()
        .join(".codex/config.toml")
}

fn claude_path() -> PathBuf {
    crate::runtime::home_dir()
        .unwrap_or_default()
        .join(".claude/settings.json")
}

fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.autojev.bak", path.display()))
}

fn backup_once(path: &Path) -> Result<()> {
    let backup = backup_path(path);
    if backup.exists() {
        return Ok(());
    }
    if path.exists() {
        crate::ownership::atomic(&backup,&fs::read(path)?).with_context(|| format!("back up {}", path.display()))?;
    } else {
        crate::ownership::atomic(&backup,b"").context("create empty AutoJev backup")?;
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(())
}

fn migrate_before_connect(path:&Path,port:u16)->Result<()> {
    if backup_path(path).exists() && crate::ownership::owner(path)?.is_none(){
        if let Some(previous)=[port,crate::config::DEFAULT_PORT,crate::config::DEV_PORT,9487].into_iter().find(|p|crate::ownership::points_to(path,*p)) {
            let id=if path==codex_path(){"codex"}else{"claude"};prepare_legacy(id,&crate::runtime::home_dir().unwrap_or_default(),previous)?;
        }
    }Ok(())
}
fn inject_owned(path:&Path,port:u16,output:&str)->Result<()> {
    let original=fs::read_to_string(backup_path(path))?;
    crate::ownership::apply(&[path.to_owned()],port,&[if original.is_empty(){None}else{Some(original)}],&[output.to_owned()])
}
pub fn restore_gateway(port:u16)->Result<()> {restore_gateway_at(port,&crate::runtime::home_dir().ok_or_else(||anyhow!("Cannot locate home directory"))?)}
pub fn repair_orphan_models(home: &Path) -> Result<()> {
    for id in ["codex", "claude", "grok", "kimi", "openclaw", "opencode", "hermes", "omp", "gemini"] {
        let paths = match id {
            "codex" => vec![home.join(".codex/config.toml")],
            "claude" => vec![home.join(".claude/settings.json")],
            _ => crate::agent_adapters::paths(id, home),
        };
        crate::ownership::repair_orphan_models(&paths).with_context(|| format!("Repair stale {id} model selection"))?;
    }
    Ok(())
}
pub fn restore_gateway_at(port:u16,home:&Path)->Result<()> {
    let _lock=crate::ownership::config_lock(home)?;
    let ids=["codex","claude","grok","kimi","openclaw","opencode","hermes","omp","gemini"];
    let mut owned=Vec::new();
    for id in ids {
        let paths=prepare_legacy(id,home,port)?;
        for path in &paths {
            if crate::ownership::owner(path)?==Some(port){crate::ownership::prepare(path,port).with_context(||format!("Cannot restore {id}; gateway remains running"))?;if !owned.contains(&id){owned.push(id);}}
            else if crate::ownership::points_to(path,port) {return Err(anyhow!("{id} has a legacy or conflicting gateway configuration. Reconnect it once to create a safe ownership record before stopping."));}
        }
    }
    let custom_paths = crate::custom_agents::registered(home, port)?;
    for path in &custom_paths { crate::ownership::prepare(path, port)?; }
    let fast=crate::agent_adapters::paths("fastclaw",&home);
    if let Some(path)=fast.first(){if super::fastclaw_adapter::uses_port(path,port){super::fastclaw_adapter::restore(path).context("Cannot restore FastClaw; gateway remains running")?;}}
    for path in custom_paths { crate::ownership::restore(&path, port)?; }
    for id in owned {restore_at(id,home).with_context(||format!("Cannot restore {id}; gateway remains running"))?;}
    repair_orphan_models(home)?;
    Ok(())
}

fn prepare_legacy(id:&str,home:&Path,port:u16)->Result<Vec<PathBuf>> {
        let paths=match id{"codex"=>vec![home.join(".codex/config.toml"),home.join(".codex/autojev-models.json")],"claude"=>vec![home.join(".claude/settings.json")],_=>crate::agent_adapters::paths(id,&home)};
        if paths.iter().any(|p|crate::ownership::points_to(p,port)) && paths.iter().all(|p|crate::ownership::owner(p).ok().flatten().is_none()) {
            if matches!(id,"codex"|"claude") {
                let old=fs::read_to_string(backup_path(&paths[0])).context("Legacy backup missing; reconnect agent before stopping")?;
                let keys=if id=="codex"{vec!["/model","/model_provider","/model_providers/autojev"]}else{vec!["/env/ANTHROPIC_BASE_URL","/env/ANTHROPIC_AUTH_TOKEN","/env/ANTHROPIC_MODEL","/env/ANTHROPIC_DEFAULT_HAIKU_MODEL","/env/ANTHROPIC_DEFAULT_SONNET_MODEL","/env/ANTHROPIC_DEFAULT_OPUS_MODEL"]};
                crate::ownership::adopt(&paths[0],port,Some(&old),&keys)?;
            }else{crate::agent_adapters::adopt_legacy(id,&home,port)?;}
        }
    Ok(paths)
}
pub fn restore_for_gateway(id:&str,port:u16)->Result<()> {
    let home=crate::runtime::home_dir().ok_or_else(||anyhow!("Cannot locate home directory"))?;
    if id!="fastclaw"{prepare_legacy(id,&home,port)?;}
    restore_at(id,&home)
}

fn executable_path(command: &str) -> Option<String> {
    if crate::runtime::isolated() { return None; }
    let output = Command::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()))
        .args(["-lc", "command -v -- \"$1\"", "autojev-detect", command])
        .output().ok()?;
    if !output.status.success() { return None; }
    String::from_utf8_lossy(&output.stdout).lines().rev()
        .map(str::trim).find(|line| Path::new(line).is_absolute() && Path::new(line).is_file())
        .map(str::to_owned)
}
fn command_exists(command: &str) -> bool { executable_path(command).is_some() }

fn display_home(path: &Path) -> String {
    if let Some(home) = crate::runtime::home_dir() {
        if let Ok(relative) = path.strip_prefix(home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod detection_tests {
    use super::*;
    #[test]
    fn executable_input_is_not_shell_code() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("should-not-exist");
        assert!(!command_exists(&format!("missing; touch {}", marker.display())));
        assert!(!marker.exists());
        assert!(command_exists("sh"));
    }
    #[test]
    fn custom_agent_validation_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.db");
        let store = crate::config::ConfigStore::load(path.clone()).unwrap();
        let agent = CustomAgent { config_path: None, injection: None, id: "custom-test".into(), name: "Test".into(), command: "sh".into(), args: vec!["--version".into()], icon: None };
        assert!(validate_custom(&agent).is_ok());
        store.update(|c| c.custom_agents.push(agent)).unwrap();
        let saved = crate::config::ConfigStore::load(path).unwrap().read();
        assert_eq!(saved.custom_agents[0].command, "sh");
        assert_eq!(saved.custom_agents[0].args, vec!["--version"]);
        let found = detect_executable("custom-test", "Test", "sh", true);
        assert!(found.installed && found.custom && !found.can_connect);
    }
}

fn terminal_command(path: &str) -> String {
    format!("exec '{}'", path.replace('\'', "'\\''"))
}
fn argument_suffix(args: &[String]) -> String {
    args.iter().map(|arg| format!(" '{}'", arg.replace('\'', "'\\''"))).collect()
}
pub fn prompt_args(args: &[String], prompt: &str) -> Vec<String> {
    let mut result: Vec<String> = args.iter().map(|a| a.replace("{prompt}", prompt)).collect();
    if !args.iter().any(|a| a.contains("{prompt}")) { result.push(prompt.into()); }
    result
}
pub async fn test_custom(agent: CustomAgent) -> Result<String> {
    crate::runtime::external_action()?;
    validate_custom(&agent)?;
    let path = executable_path(agent.command.trim()).ok_or_else(|| anyhow!("Agent executable not found"))?;
    let mut command = tokio::process::Command::new(path);
    command.args(prompt_args(&agent.args, "Reply with OK only."))
        .stdin(std::process::Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
        .await.map_err(|_| anyhow!("Agent test timed out after 60 seconds"))??;
    if !output.status.success() { return Err(anyhow!("Agent test failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr).chars().take(2000).collect::<String>())); }
    let reply = String::from_utf8_lossy(&output.stdout).trim().chars().take(2000).collect::<String>();
    if reply.is_empty() { return Err(anyhow!("Agent returned no response")); }
    Ok(reply)
}

pub fn launch(id: &str, custom: &[CustomAgent]) -> Result<()> {
    crate::runtime::external_action()?;
    let agent = detect(custom).into_iter().find(|a| a.id == id)
        .ok_or_else(|| anyhow!("Agent not found"))?;
    let path = agent.executable_path.ok_or_else(|| anyhow!("Agent executable not found"))?;
    let args = custom.iter().find(|a| a.id == id).map(|a| a.args.clone()).unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        // Launch Services honors the user's default handler for executable .command files.
        let script_path = std::env::temp_dir().join(format!("autojev-launch-{}.command", uuid::Uuid::new_v4()));
        let command = terminal_command(&path) + &argument_suffix(&args);
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let script = format!("#!/bin/sh\nrm -f -- \"$0\"\n{}{}\n", terminal_command(&shell), argument_suffix(&["-l".into(), "-c".into(), command]));
        crate::ownership::atomic(&script_path, script.as_bytes())?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))?;
        let status = Command::new("/usr/bin/open").arg(&script_path).status().context("open default terminal")?;
        if !status.success() { let _ = fs::remove_file(&script_path); return Err(anyhow!("Could not open terminal")); }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("x-terminal-emulator").args(["-e", &path]).args(&args).spawn()
            .context("open terminal")?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        Err(anyhow!("Terminal launch is not supported on this platform"))
    }
}

#[cfg(test)]
mod launch_tests {
    #[test]
    fn custom_agent_old_config_and_literal_arguments() {
        let old: super::CustomAgent = serde_json::from_str(r#"{"id":"custom-old","name":"Old","command":"echo"}"#).unwrap();
        assert!(old.args.is_empty() && old.icon.is_none());
        assert_eq!(super::prompt_args(&["--message={prompt}".into()], "hello world"), vec!["--message=hello world"]);
        assert_eq!(super::prompt_args(&["--model".into(), "my model".into()], "hello"), vec!["--model", "my model", "hello"]);
        assert_eq!(super::argument_suffix(&["$(touch nope)".into(), "a'b".into()]), " '$(touch nope)' 'a'\\''b'");
    }
    #[tokio::test]
    async fn custom_agent_test_executes_without_shell_interpolation() {
        let agent = super::CustomAgent { config_path: None, injection: None, id: "custom-echo".into(), name: "Echo".into(), command: "/bin/echo".into(), args: vec!["$(echo unsafe)".into(), "{prompt}".into()], icon: None };
        assert_eq!(super::test_custom(agent).await.unwrap(), "$(echo unsafe) Reply with OK only.");
    }
    #[test]
    fn terminal_command_quotes_executable_as_one_argument() {
        assert_eq!(super::terminal_command("/tmp/my agent"), "exec '/tmp/my agent'");
        assert_eq!(super::terminal_command("/tmp/a'b"), "exec '/tmp/a'\\''b'");
        assert_eq!(super::terminal_command("/tmp/$(touch nope)"), "exec '/tmp/$(touch nope)'");
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn stop_repairs_orphans_even_when_previous_restore_removed_the_journal() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude/settings.json");
        crate::ownership::atomic(&path, br#"{"model":"autojev/quality","theme":"dark"}"#).unwrap();
        // This is the state left by the old restore code: no endpoint, backup or owner.
        restore_gateway_at(9526, home.path()).unwrap();
        let restored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored, json!({"theme":"dark"}));
        restore_gateway_at(9526, home.path()).unwrap();
        // A polluted original from an earlier reconnect must not reintroduce it either.
        crate::ownership::apply(&[path.clone()], 9526,
            &[Some(r#"{"model":"autojev/quality","theme":"dark"}"#.into())],
            &[r#"{"model":"autojev/fast","theme":"dark","env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9526"}}"#.into()]).unwrap();
        restore_gateway_at(9526, home.path()).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap(), restored);
    }
    #[test]
    fn stop_recovers_legacy_config_without_overwriting_user_settings(){
        let home=tempfile::tempdir().unwrap();let path=home.path().join(".codex/config.toml");fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(backup_path(&path),"model = 'original'\ncustom = 1\n").unwrap();
        fs::write(&path,"model = 'autojev/test'\nmodel_provider = 'autojev'\ncustom = 2\n[model_providers.autojev]\nbase_url = 'http://127.0.0.1:9526/v1'\n").unwrap();
        restore_gateway_at(9527,home.path()).unwrap();assert!(crate::ownership::points_to(&path,9526));
        restore_gateway_at(9526,home.path()).unwrap();let text=fs::read_to_string(&path).unwrap();let doc=text.parse::<DocumentMut>().unwrap();
        assert_eq!(doc["model"].as_str(),Some("original"));assert_eq!(doc["custom"].as_integer(),Some(2));assert!(!crate::ownership::points_to(&path,9526));assert!(!backup_path(&path).exists());
    }
    #[test]
    fn preflight_conflict_leaves_all_configs_connected(){
        let home=tempfile::tempdir().unwrap();let codex=home.path().join(".codex/config.toml");let claude=home.path().join(".claude/settings.json");
        let applied="model = 'autojev/test'\n[model_providers.autojev]\nbase_url = 'http://127.0.0.1:9526/v1'\n";
        crate::ownership::apply(&[codex.clone()],9526,&[None],&[applied.into()]).unwrap();
        let c=r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9526"}}"#;
        crate::ownership::apply(&[claude.clone()],9526,&[None],&[c.into()]).unwrap();
        fs::write(&claude,r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9526/changed"}}"#).unwrap();
        assert!(restore_gateway_at(9526,home.path()).is_err());assert_eq!(fs::read_to_string(codex).unwrap(),applied);
    }
}

pub fn owned_by(id:&str,port:u16)->bool {
    let home=crate::runtime::home_dir().unwrap_or_default();
    if id=="fastclaw"{return crate::agent_adapters::paths(id,&home).first().is_some_and(|p|crate::fastclaw_adapter::uses_port(p,port));}
    let paths=match id{"codex"=>vec![codex_path()],"claude"=>vec![claude_path()],_=>crate::agent_adapters::paths(id,&home)};
    paths.iter().any(|p|crate::ownership::owner(p).ok().flatten()==Some(port) || crate::ownership::points_to(p,port))
}

#[cfg(test)]
mod codex_catalog_tests {
    use super::*;
    #[test]
    fn codex_catalog_lists_every_selected_binding_and_restores_configuration() {
        let home=tempfile::tempdir().unwrap();
        let config=home.path().join(".codex/config.toml");
        ensure_parent(&config).unwrap();
        let original="model = \"original\"\nweb_search = \"live\"\n";
        fs::write(&config,original).unwrap();
        let entries=vec![crate::agent_catalog::Entry{binding:"fast".into(),id:"autojev/fast".into(),name:"Fast route".into()},crate::agent_catalog::Entry{binding:"model/internal".into(),id:"vendor/model".into(),name:"Direct model".into()}];
        connect_codex_catalog(9526,"fast",&entries,home.path()).unwrap();
        let doc=fs::read_to_string(&config).unwrap().parse::<DocumentMut>().unwrap();
        assert_eq!(doc["model"].as_str(),Some("autojev/fast"));
        assert_eq!(doc["web_search"].as_str(),Some("disabled"));
        assert_eq!(doc["model_providers"]["autojev"]["http_headers"]["x-autojev-agent"].as_str(), Some("codex"));
        let path=home.path().join(".codex/autojev-models.json");
        let catalog:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(catalog["models"][0]["slug"],"autojev/fast");
        assert_eq!(catalog["models"][1]["slug"],"vendor/model");
        assert_eq!(catalog["models"][0]["visibility"],"list");
        assert_eq!(catalog["models"][0]["supports_search_tool"],false);
        restore_at("codex",home.path()).unwrap();
        assert_eq!(fs::read_to_string(config).unwrap().parse::<DocumentMut>().unwrap()["web_search"].as_str(),Some("live"));
        assert!(!path.exists());
    }
    #[test]
    fn claude_injects_agent_marker_and_restores() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude/settings.json");
        ensure_parent(&path).unwrap();
        fs::write(&path, r#"{"env":{"EXISTING":"keep"}}"#).unwrap();
        let entries = vec![
            crate::agent_catalog::Entry { binding: "fast".into(), id: "autojev/fast".into(), name: "Fast route".into() },
            crate::agent_catalog::Entry { binding: "model/uuid".into(), id: "provider/model".into(), name: "Model title".into() },
        ];
        connect_claude_catalog_at(9526, "model/uuid", &entries, &path).unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc["env"]["ANTHROPIC_AUTH_TOKEN"], "autojev-local-claude");
        assert_eq!(doc["model"], "provider/model");
        assert_eq!(doc["env"]["ANTHROPIC_MODEL"], "provider/model");
        assert_eq!(doc["modelPicker"]["replaceBuiltInOptions"], true);
        assert_eq!(doc["modelPicker"]["options"].as_array().unwrap().len(), 2);
        assert_eq!(doc["modelPicker"]["options"][1]["label"], "Model title");
        assert_eq!(doc["modelPicker"]["options"][1]["model"], "provider/model");
        assert_eq!(doc["modelPicker"]["options"][1]["behavesAs"], "claude-sonnet-4-6");
        connect_claude_catalog_at(9526, "fast", &entries[..1], &path).unwrap();
        let refreshed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(refreshed["modelPicker"]["options"].as_array().unwrap().len(), 1);
        assert_eq!(refreshed["model"], "autojev/fast");
        crate::ownership::restore(&path, 9526).unwrap();
        let restored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored, json!({"env":{"EXISTING":"keep"}}));
    }
}

pub fn configuration_paths(id: &str) -> Vec<PathBuf> {
    match id {
        "codex" => vec![codex_path()],
        "claude" => vec![claude_path()],
        _ => crate::agent_adapters::paths(id, &crate::runtime::home_dir().unwrap_or_default()),
    }
}

#[cfg(test)]
mod catalog_pending_sync_tests {
    use super::*;

    /// 已保存目录与当前模型/路由一致 → 无需同步；不一致或已无法重建 → 待同步。
    #[test]
    fn saved_agent_catalog_reports_pending_sync_once_selection_changes() {
        let mut config = crate::config::AppConfig::default();
        config.models[0].selected = true;
        config.models[0].enabled = true;
        let model = config.models[0].clone();
        let binding = format!("model/{}", model.id);
        let saved = vec![crate::agent_catalog::Entry {
            binding: binding.clone(),
            id: format!("autojev/model/{}", model.id),
            name: model.name.clone(),
        }];
        // 从未保存过目录：没有可同步的对象。
        assert!(!catalog_pending_sync(&config, "codex"));
        config.agent_catalogs.insert("codex".into(), Vec::new());
        assert!(!catalog_pending_sync(&config, "codex"));
        config.agent_catalogs.insert("codex".into(), saved.clone());
        assert!(!catalog_pending_sync(&config, "codex"));
        // 取消选择：保存的绑定不再可选，外部配置仍是旧的。
        config.models[0].selected = false;
        assert!(catalog_pending_sync(&config, "codex"));
        config.models[0].selected = true;
        // 停用同理。
        config.models[0].enabled = false;
        assert!(catalog_pending_sync(&config, "codex"));
        config.models[0].enabled = true;
        // 显示名变化会让已保存目录与重算结果不一致，提示需要重新同步外部配置。
        config.models[0].name = "Renamed".into();
        assert!(catalog_pending_sync(&config, "codex"));
        assert!(!catalog_pending_sync(&config, "hermes"));
    }
}
