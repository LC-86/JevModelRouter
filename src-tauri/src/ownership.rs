//! Three-way configuration restoration: only revert values still owned by this gateway.
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Serialize, Deserialize)]
struct Journal {
    port: u16,
    original: Option<String>,
    applied: String,
    #[serde(default)]
    model_ids: Vec<String>,
}
fn journal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.autojev-owner.json", path.display()))
}
fn journal_model_ids(path: &Path, journal: &Journal) -> Vec<String> {
    let mut ids = journal.model_ids.clone();
    // Older Codex journals predate the shared catalog metadata.
    if let Ok(applied) = parse(path, &journal.applied) {
        if let Some(catalog) = applied.get("model_catalog_json").and_then(Value::as_str).map(Path::new) {
            if catalog.parent() == path.parent() {
                if let Ok(bytes) = fs::read(journal_path(catalog)) {
                    if let Ok(companion) = serde_json::from_slice::<Journal>(&bytes) {
                        if companion.port == journal.port {
                            if let Ok(value) = parse(catalog, &companion.applied) { ids.extend(catalog_ids(&value)); }
                        }
                    }
                }
            }
        }
    }
    ids
}
pub fn atomic(path: &Path, data: &[u8]) -> Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("Invalid configuration path"))?,
    )?;
    let tmp = PathBuf::from(format!("{}.{}.tmp", path.display(), uuid::Uuid::new_v4()));
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut file = opts.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
pub fn record(path: &Path, port: u16, original: Option<String>, applied: &str) -> Result<()> {
    record_with_models(path, port, original, applied, &[])
}
fn record_with_models(path: &Path, port: u16, original: Option<String>, applied: &str, model_ids: &[String]) -> Result<()> {
    let jp = journal_path(path);
    let prior = if jp.exists() {
        Some(serde_json::from_slice::<Journal>(&fs::read(&jp)?)?)
    } else {
        None
    };
    let original = if let Some(prior) = prior {
        let current = match fs::read_to_string(path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if current.as_deref() == Some(&prior.applied) {
            prior.original
        } else {
            let before = prior
                .original
                .as_deref()
                .map(|s| parse(path, s))
                .transpose()?;
            let previous = parse(path, &prior.applied)?;
            let live = current.as_deref().map(|s| parse(path, s)).transpose()?;
            revert_model_configuration(before.as_ref(), &previous, live.as_ref(), &journal_model_ids(path, &prior))
                .map(|value| serialize(path, &value, current.as_deref().unwrap_or("")))
                .transpose()?
        }
    } else {
        original
    };
    atomic(
        &jp,
        &serde_json::to_vec(&Journal {
            port,
            original,
            applied: applied.into(),
            model_ids: model_ids.to_vec(),
        })?,
    )
}
pub fn owns(path: &Path, port: u16) -> bool {
    fs::read(journal_path(path))
        .ok()
        .and_then(|b| serde_json::from_slice::<Journal>(&b).ok())
        .is_some_and(|j| j.port == port)
}
fn parse(path: &Path, text: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    Ok(
        match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "toml" => toml_edit::de::from_str(text)?,
            "yaml" | "yml" => serde_yaml::from_str(text)?,
            "env" => {
                let mut object = serde_json::Map::new();
                for line in text.lines() {
                    if !line.trim_start().starts_with('#') {
                        if let Some((k, v)) = line.split_once('=') {
                            object.insert(
                                k.trim().strip_prefix("export ").unwrap_or(k.trim()).into(),
                                v.into(),
                            );
                        }
                    }
                }
                object.into()
            }
            _ if path.file_name().is_some_and(|n| n == ".env") => {
                let mut object = serde_json::Map::new();
                for line in text.lines() {
                    if !line.trim_start().starts_with('#') {
                        if let Some((k, v)) = line.split_once('=') {
                            object.insert(
                                k.trim().strip_prefix("export ").unwrap_or(k.trim()).into(),
                                v.into(),
                            );
                        }
                    }
                }
                object.into()
            }
            _ => json5::from_str(text)?,
        },
    )
}
/// Changes to a field after injection win. Unchanged injected fields revert to their originals.
pub fn revert(
    before: Option<&Value>,
    applied: Option<&Value>,
    current: Option<&Value>,
) -> Option<Value> {
    if before == applied {
        return current.cloned();
    }
    if current == applied {
        return before.cloned();
    }
    if let (Some(a), Some(c)) = (
        applied.and_then(Value::as_object),
        current.and_then(Value::as_object),
    ) {
        let b = before.and_then(Value::as_object);
        let mut result = c.clone();
        let keys = a
            .keys()
            .chain(b.into_iter().flat_map(|m| m.keys()))
            .collect::<std::collections::HashSet<_>>();
        for key in keys {
            match revert(b.and_then(|m| m.get(key)), a.get(key), c.get(key)) {
                Some(v) => {
                    result.insert(key.clone(), v);
                }
                None => {
                    result.remove(key);
                }
            }
        }
        if result.is_empty() && before.is_none() {
            None
        } else {
            Some(result.into())
        }
    } else {
        current.cloned()
    }
}
// Client-side model switches remain gateway-owned when they target our catalog.
fn catalog_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for pointer in ["/models", "/modelPicker/options", "/providers/autojev/models", "/provider/autojev/models", "/models/providers/autojev/models"] {
        if let Some(rows) = value.pointer(pointer).and_then(Value::as_array) {
            for row in rows { if let Some(id) = row.get("slug").or_else(|| row.get("id")).or_else(|| row.get("model")).and_then(Value::as_str) { ids.push(id.into()); } }
        }
        if pointer != "/models" {
            if let Some(rows) = value.pointer(pointer).and_then(Value::as_object) { ids.extend(rows.keys().cloned()); }
        }
    }
    for pointer in ["/models", "/model"] {
        if let Some(rows) = value.pointer(pointer).and_then(Value::as_object) {
            for (key, row) in rows {
                if row.get("provider").and_then(Value::as_str) == Some("autojev")
                    || row.get("api_key").and_then(Value::as_str).is_some_and(|s| s.starts_with("autojev-local-")) {
                    ids.push(key.clone());
                    if let Some(id) = row.get("model").and_then(Value::as_str) { ids.push(id.into()); }
                }
            }
        }
    }
    ids
}
pub(crate) fn revert_model_configuration(before: Option<&Value>, applied: &Value, current: Option<&Value>, model_ids: &[String]) -> Option<Value> {
    let mut restored = revert(before, Some(applied), current);
    let mut ids = catalog_ids(applied);
    ids.extend_from_slice(model_ids);
    let mut selectors: Vec<Vec<String>> = [
        "model", "model_provider", "small_model", "default_model", "models/default", "model/name", "model/default",
        "agents/defaults/model/primary", "GEMINI_MODEL",
        "env/ANTHROPIC_MODEL", "env/ANTHROPIC_DEFAULT_OPUS_MODEL", "env/ANTHROPIC_DEFAULT_SONNET_MODEL",
        "env/ANTHROPIC_DEFAULT_HAIKU_MODEL", "env/ANTHROPIC_DEFAULT_FABLE_MODEL",
    ].iter().map(|s| s.split('/').map(str::to_owned).collect()).collect();
    if let Some(current) = current {
        for group in ["modelRoles", "profiles"] {
            if let Some(map) = current.get(group).and_then(Value::as_object) {
                for key in map.keys() {
                    let mut path = vec![group.into(), key.clone()];
                    if group == "profiles" {
                        selectors.push(vec![group.into(), key.clone(), "model_provider".into()]);
                        path.push("model".into());
                    }
                    selectors.push(path);
                }
            }
        }
    }
    fn get<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
        path.iter().try_fold(root, |v, key| v.get(key))
    }
    fn restore_field(root: &mut Value, path: &[String], original: Option<&Value>) {
        if path.len() == 1 {
            if let Some(map) = root.as_object_mut() {
                if let Some(value) = original { map.insert(path[0].clone(), value.clone()); }
                else { map.remove(&path[0]); }
            }
        } else if let Some(child) = root.get_mut(&path[0]) { restore_field(child, &path[1..], original); }
    }
    for path in &selectors {
        if let Some(id) = get(applied, path).and_then(Value::as_str) { ids.push(id.into()); }
    }
    for path in selectors {
        let selected = current.and_then(|v| get(v, &path)).and_then(Value::as_str);
        if selected.is_some_and(|id| id.starts_with("autojev/") || ids.iter().any(|candidate| candidate == id)
            || (path.last().is_some_and(|key| key == "model_provider") && id == "autojev")) {
            if let Some(root) = restored.as_mut() { restore_field(root, &path, before.and_then(|v| get(v, &path))); }
        }
    }
    restored
}
fn serialize(path: &Path, value: &Value, current: &str) -> Result<String> {
    Ok(
        match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "toml" => toml_edit::ser::to_string_pretty(value)?,
            "yaml" | "yml" => serde_yaml::to_string(value)?,
            _ if path.file_name().is_some_and(|n| n == ".env")
                || path.extension().is_some_and(|n| n == "env") =>
            {
                let mut lines = current
                    .lines()
                    .filter(|line| line.trim_start().starts_with('#') || !line.contains('='))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if let Some(m) = value.as_object() {
                    for (k, v) in m {
                        lines.push(format!("{k}={}", v.as_str().unwrap_or("")));
                    }
                }
                lines.join("\n") + "\n"
            }
            _ => serde_json::to_string_pretty(value)?,
        },
    )
}
/// Repair pre-journal model selections only when the entire client configuration
/// has neither an owner nor a configured endpoint. Never guess ownership of bare IDs.
pub fn repair_orphan_models(paths: &[PathBuf]) -> Result<()> {
    fn has_endpoint(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.iter().any(|(key, value)| {
                let key = key.to_ascii_lowercase();
                // MCP transport URLs are independent of the model provider.
                // They must not block removing a dangling gateway model selection.
                if matches!(key.as_str(), "mcp_servers" | "mcpservers") { return false; }
                ((key.contains("url") || key.contains("endpoint"))
                    && value.as_str().is_some_and(|s| !s.trim_matches('"').is_empty()))
                    || has_endpoint(value)
            }),
            Value::Array(rows) => rows.iter().any(has_endpoint),
            _ => false,
        }
    }
    let mut documents = Vec::new();
    for path in paths {
        if owner(path)?.is_some() { return Ok(()); }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let value = parse(path, &text)?;
        if has_endpoint(&value) { return Ok(()); }
        documents.push((path, text, value));
    }
    for (path, text, value) in documents {
        // An empty applied document restricts matching to the reserved namespace.
        let cleaned = revert_model_configuration(None, &json!({}), Some(&value), &[])
            .unwrap_or_else(|| json!({}));
        if cleaned != value {
            let backup = PathBuf::from(format!("{}.autojev-orphan-{}.bak", path.display(), uuid::Uuid::new_v4()));
            atomic(&backup, text.as_bytes())?;
            atomic(path, serialize(path, &cleaned, &text)?.as_bytes())?;
        }
    }
    Ok(())
}
fn references_port(value: &Value, port: u16) -> bool {
    match value {
        Value::String(s) => s.lines().any(|line| {
            line.trim_matches('"')
                .parse::<reqwest::Url>()
                .ok()
                .is_some_and(|u| {
                    matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
                        && u.port() == Some(port)
                })
        }),
        Value::Array(a) => a.iter().any(|v| references_port(v, port)),
        Value::Object(o) => o.values().any(|v| references_port(v, port)),
        _ => false,
    }
}
pub fn prepare(path: &Path, port: u16) -> Result<Option<Option<String>>> {
    if !owns(path, port) {
        return Ok(None);
    }
    let journal: Journal = serde_json::from_slice(&fs::read(journal_path(path))?)?;
    let current = match fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if current.as_deref() == Some(&journal.applied) {
        if let Some(original) = journal.original.as_deref() {
            anyhow::ensure!(!references_port(&parse(path,original)?,port),"Original backup still points to this gateway; restore the original endpoint manually before stopping");
        }
        return Ok(Some(journal.original));
    }
    let Some(current) = current else {
        return Ok(Some(None));
    };
    let before = journal
        .original
        .as_deref()
        .map(|s| parse(path, s))
        .transpose()?;
    let applied = parse(path, &journal.applied)?;
    let live = parse(path, &current)?;
    let restored = revert_model_configuration(before.as_ref(), &applied, Some(&live), &journal_model_ids(path, &journal)).unwrap_or(json!({}));
    anyhow::ensure!(!references_port(&restored,port),"{} still points to this gateway after a conflicting edit; restore its endpoint before stopping",path.display());
    Ok(Some(Some(serialize(path, &restored, &current)?)))
}
pub fn restore(path: &Path, port: u16) -> Result<bool> {
    let Some(contents) = prepare(path, port)? else {
        return Ok(false);
    };
    if let Some(contents) = contents {
        atomic(path, contents.as_bytes())?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    fs::remove_file(journal_path(path)).context("Remove restored ownership record")?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn orphan_codex_model_is_removed_with_mcp_servers_configured() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        let text = "model = 'autojev/jev-router'\nmodel_reasoning_effort = 'medium'\n[mcp_servers.openaiDeveloperDocs]\nurl = 'https://developers.openai.com/mcp'\n";
        atomic(&path, text.as_bytes()).unwrap();
        repair_orphan_models(&[path.clone()]).unwrap();
        let repaired = parse(&path, &fs::read_to_string(&path).unwrap()).unwrap();
        assert!(repaired.get("model").is_none());
        let original = parse(&path, text).unwrap();
        assert_eq!(repaired["mcp_servers"], original["mcp_servers"]);
        assert_eq!(repaired["model_reasoning_effort"], "medium");
        // A real model endpoint still protects the user's configuration.
        let active = format!("{text}\n[model_providers.autojev]\nbase_url = 'http://localhost:9526/v1'\n");
        atomic(&path, active.as_bytes()).unwrap();
        repair_orphan_models(&[path.clone()]).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), active);
    }
    #[test]
    fn repairs_pre_journal_orphans_without_touching_active_clients() {
        let home = std::env::temp_dir().join(format!("autojev-orphans-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).unwrap();
        for (name, content, pointer) in [
            ("claude.json", "{\"model\":\"autojev/quality\",\"theme\":\"dark\"}", "/model"),
            ("codex.toml", "model = 'autojev/fast'\ntheme = 'dark'\n", "/model"),
            ("hermes.yaml", "model:\n  default: autojev/fast\ntheme: dark\n", "/model/default"),
            ("kimi.toml", "default_model = 'autojev/cheap'\ntheme = 'dark'\n", "/default_model"),
            ("omp.yml", "modelRoles:\n  default: autojev/fast\ntheme: dark\n", "/modelRoles/default"),
        ] {
            let path = home.join(name);
            atomic(&path, content.as_bytes()).unwrap();
            repair_orphan_models(&[path.clone()]).unwrap();
            let repaired = fs::read_to_string(&path).unwrap();
            let value = parse(&path, &repaired).unwrap();
            assert!(value.pointer(pointer).is_none(), "{name}");
            assert_eq!(value["theme"], "dark");
            repair_orphan_models(&[path.clone()]).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), repaired);
        }
        let path = home.join("active.json");
        for value in [json!({"model":"sonnet"}), json!({"model":"provider/model"}),
            json!({"model":"autojev/fast","env":{"ANTHROPIC_BASE_URL":"http://localhost:9526"}})] {
            atomic(&path, value.to_string().as_bytes()).unwrap();
            repair_orphan_models(&[path.clone()]).unwrap();
            assert_eq!(parse(&path, &fs::read_to_string(&path).unwrap()).unwrap(), value);
        }
        atomic(&path, b"{\"model\":\"autojev/fast\"}").unwrap();
        record(&path, 9526, None, "{}").unwrap();
        repair_orphan_models(&[path.clone()]).unwrap();
        assert_eq!(parse(&path, &fs::read_to_string(&path).unwrap()).unwrap()["model"], "autojev/fast");
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn claude_model_switches_restore_gateway_choices_but_keep_external_choices() {
        for original_model in [None, Some("sonnet")] {
            for selected in ["autojev/quality", "provider/model", "opus"] {
                for reconnect in [false, true] {
                    let dir = tempfile::tempdir().unwrap();
                    let path = dir.path().join("settings.json");
                    let mut before = json!({"theme":"dark"});
                    if let Some(model) = original_model { before["model"] = json!(model); }
                    let applied = json!({"theme":"dark","model":"autojev/fast",
                        "env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9526","ANTHROPIC_MODEL":"autojev/fast"},
                        "modelPicker":{"options":[{"model":"autojev/fast"},{"model":"autojev/quality"},{"model":"provider/model"}]}});
                    record(&path, 9526, Some(before.to_string()), &applied.to_string()).unwrap();
                    let mut live = applied.clone();
                    live["model"] = json!(selected);
                    live["theme"] = json!("light");
                    fs::write(&path, live.to_string()).unwrap();
                    if reconnect {
                        record(&path, 9526, None, &applied.to_string()).unwrap();
                        fs::write(&path, applied.to_string()).unwrap();
                    }
                    restore(&path, 9526).unwrap();
                    let restored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                    let mut expected = before.clone();
                    expected["theme"] = json!("light");
                    if selected == "opus" { expected["model"] = json!("opus"); }
                    assert_eq!(restored, expected, "{selected}, reconnect={reconnect}");
                }
            }
        }
    }
    #[test]
    fn legacy_claude_injection_restores_a_newly_saved_model_field() {
        let before = json!({"theme":"dark"});
        let applied = json!({"theme":"dark","env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9526","ANTHROPIC_MODEL":"autojev/fast"}});
        let mut live = applied.clone();
        live["model"] = json!("autojev/quality");
        assert_eq!(revert_model_configuration(Some(&before), &applied, Some(&live), &[]), Some(before));
    }
    #[test]
    fn all_client_model_selectors_restore_only_gateway_choices() {
        for (applied, live, expected) in [
            (json!({"model":"autojev/fast","model_provider":"autojev"}), json!({"model":"provider/other","model_provider":"autojev","theme":"light"}), json!({"theme":"light"})),
            (json!({"model":"autojev/fast"}), json!({"model":"external/model","theme":"light"}), json!({"model":"external/model","theme":"light"})),
            (json!({"default_model":"autojev/fast"}), json!({"default_model":"provider/other"}), json!({})),
            (json!({"models":{"default":"autojev/fast"}}), json!({"models":{"default":"provider/other"}}), json!({"models":{}})),
            (json!({"agents":{"defaults":{"model":{"primary":"autojev/autojev/fast"}}}}), json!({"agents":{"defaults":{"model":{"primary":"autojev/provider/other"}}}}), json!({"agents":{"defaults":{"model":{}}}})),
            (json!({"model":{"default":"autojev/fast","provider":"autojev"}}), json!({"model":{"default":"provider/other","provider":"autojev"}}), json!({"model":{}})),
            (json!({"modelRoles":{"default":"autojev/autojev/fast"}}), json!({"modelRoles":{"default":"autojev/provider/other","planning":"autojev/autojev/quality"}}), json!({"modelRoles":{}})),
            (json!({"model":{"name":"autojev/fast"}}), json!({"model":{"name":"autojev/quality"}}), json!({"model":{}})),
            (json!({"GEMINI_MODEL":"autojev/fast"}), json!({"GEMINI_MODEL":"autojev/quality"}), json!({})),
        ] {
            assert_eq!(revert_model_configuration(Some(&json!({})), &applied, Some(&live), &["provider/other".into()]), Some(expected), "{live}");
        }
    }
    #[test]
    fn codex_companion_catalog_tracks_direct_model_switches_across_reconnect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let catalog = dir.path().join("models.json");
        let before = "model = 'original'\ntheme = 'dark'\n";
        fs::write(&path, before).unwrap();
        let applied = "model = 'autojev/fast'\nmodel_provider = 'autojev'\ntheme = 'dark'\n";
        let models = r#"{"models":[{"slug":"provider/one"},{"slug":"provider/two"}]}"#;
        apply(&[path.clone(),catalog.clone()],9526,&[Some(before.into()),None],&[applied.into(),models.into()]).unwrap();
        fs::write(&path, applied.replace("autojev/fast","provider/two").replace("dark","light")).unwrap();
        apply(&[path.clone(),catalog.clone()],9526,&[Some(before.into()),None],&[applied.into(),models.into()]).unwrap();
        restore(&path,9526).unwrap();
        restore(&catalog,9526).unwrap();
        let restored = parse(&path,&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored,json!({"model":"original","theme":"light"}));
        assert!(!catalog.exists());
    }
    #[test]
    fn preserves_user_changes_and_separates_instances() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let before = r#"{"model":"old","other":1}"#;
        let applied =
            r#"{"model":"autojev/x","other":1,"provider":{"url":"http://127.0.0.1:9526/v1"}}"#;
        fs::write(&path, applied).unwrap();
        record(&path, 9526, Some(before.into()), applied).unwrap();
        assert!(!restore(&path, 9527).unwrap());
        fs::write(&path,r#"{"model":"user-choice","other":2,"provider":{"url":"http://127.0.0.1:9526/v1"},"new":true}"#).unwrap();
        restore(&path, 9526).unwrap();
        let result: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(result, json!({"model":"user-choice","other":2,"new":true}));
    }
    #[test]
    fn reconnect_keeps_first_backup_and_deletes_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        record(&path, 9526, None, "model: first").unwrap();
        fs::write(&path, "model: first").unwrap();
        record(&path, 9526, Some("model: first".into()), "model: second").unwrap();
        fs::write(&path, "model: second").unwrap();
        restore(&path, 9526).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn restores_toml_and_preserves_added_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        record(
            &path,
            9526,
            Some("model = 'old'\n".into()),
            "model = 'autojev/x'\n",
        )
        .unwrap();
        fs::write(&path, "model = 'autojev/x'\nnew = true\n").unwrap();
        restore(&path, 9526).unwrap();
        let v = parse(&path, &fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["model"], "old");
        assert_eq!(v["new"], true);
    }
}

pub fn owner(path: &Path) -> Result<Option<u16>> {
    let p = journal_path(path);
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice::<Journal>(&fs::read(p)?)?.port))
}
pub fn points_to(path: &Path, port: u16) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| parse(path, &s).ok())
        .is_some_and(|v| references_port(&v, port))
}
/// Write-ahead ownership records and configuration changes roll back together on an I/O failure.
pub fn apply(
    paths: &[PathBuf],
    port: u16,
    originals: &[Option<String>],
    updated: &[String],
) -> Result<()> {
    let model_ids = paths.iter().zip(updated).map(|(path, text)| parse(path, text).map(|v| catalog_ids(&v)))
        .collect::<Result<Vec<_>>>()?.into_iter().flatten().collect::<Vec<_>>();
    let mut rollback = Vec::new();
    for p in paths {
        for target in [p.clone(), journal_path(p)] {
            let bytes = match fs::read(&target) {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            rollback.push((target, bytes));
        }
    }
    let result = (|| {
        for ((p, b), a) in paths.iter().zip(originals).zip(updated) {
            record_with_models(p, port, b.clone(), a, &model_ids)?;
            atomic(p, a.as_bytes())?;
        }
        Ok(())
    })();
    if result.is_err() {
        for (p, b) in rollback {
            if let Some(b) = b {
                atomic(&p, &b)?;
            } else if p.exists() {
                fs::remove_file(p)?;
            }
        }
    }
    result
}

/// Legacy backups lack an applied snapshot. Adopt only known adapter-owned keys.
pub fn adopt(path: &Path, port: u16, original: Option<&str>, keys: &[&str]) -> Result<()> {
    if owner(path)?.is_some() {
        return Ok(());
    }
    let current = fs::read_to_string(path)?;
    let live = parse(path, &current)?;
    let old = original
        .map(|s| parse(path, s))
        .transpose()?
        .unwrap_or(json!({}));
    let mut before = live.clone();
    fn set(v: &mut Value, parts: &[&str], old: Option<&Value>) {
        if parts.len() == 1 {
            if let Some(map) = v.as_object_mut() {
                if let Some(old) = old {
                    map.insert(parts[0].into(), old.clone());
                } else {
                    map.remove(parts[0]);
                }
            }
        } else if let Some(child) = v.get_mut(parts[0]) {
            set(child, &parts[1..], old);
        }
    }
    for key in keys {
        let parts = key.trim_start_matches('/').split('/').collect::<Vec<_>>();
        set(&mut before, &parts, old.pointer(key));
    }
    record(
        path,
        port,
        Some(serialize(path, &before, &current)?),
        &current,
    )
}

pub fn config_lock(home: &Path) -> Result<fs::File> {
    let dir = home.join(".autojev");
    fs::create_dir_all(&dir)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("agent-config.lock"))?;
    file.lock().context("Lock agent configurations")?;
    Ok(file)
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;
    #[test]
    fn reconnect_rebases_user_edits_before_recording_next_injection() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.json");
        apply(
            &[path.clone()],
            9526,
            &[Some(r#"{"model":"original","setting":1}"#.into())],
            &[r#"{"model":"autojev/first","setting":1}"#.into()],
        )
        .unwrap();
        fs::write(&path, r#"{"model":"autojev/first","setting":2,"new":true}"#).unwrap();
        apply(
            &[path.clone()],
            9526,
            &[None],
            &[r#"{"model":"autojev/second","setting":2,"new":true}"#.into()],
        )
        .unwrap();
        restore(&path, 9526).unwrap();
        assert_eq!(
            parse(&path, &fs::read_to_string(&path).unwrap()).unwrap(),
            json!({"model":"original","setting":2,"new":true})
        );
    }
}
