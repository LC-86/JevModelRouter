use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};
use crate::config::{Provider, ProviderKind};

pub struct Candidate { pub provider: Provider, pub key: String }

fn endpoint_key(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    reqwest::Url::parse(value)
        .map(|url| url.to_string().trim_end_matches('/').to_owned())
        .unwrap_or_else(|_| value.to_owned())
}

pub fn already_imported(existing: &[Provider], incoming: &Provider) -> bool {
    let endpoint = endpoint_key(&incoming.base_url);
    existing.iter().any(|provider| provider.id == incoming.id || endpoint_key(&provider.base_url) == endpoint)
}

fn candidate(source: &str, id: &str, name: &str, url: &str, key: &str) -> Option<Candidate> {
    let url = url.trim().trim_end_matches('/');
    if id.is_empty() || url.is_empty() { return None; }
    let parsed = reqwest::Url::parse(url).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() { return None; }
    let kind = if parsed.host_str() == Some("openrouter.ai") { ProviderKind::Openrouter } else { ProviderKind::OpenaiCompatible };
    let url = if kind == ProviderKind::Openrouter { url.trim_end_matches("/v1") } else { url };
    let provider = Provider { preset: String::new(), api_type: String::new(), test_model: String::new(), id: format!("import-{}-{:x}", source, Sha256::digest(id.as_bytes())), name: if name.trim().is_empty() { "Imported provider".into() } else { name.into() }, kind, base_url: url.into(), enabled: true, has_api_key: false };
    if crate::validate_provider(&provider).is_err() { return None; }
    Some(Candidate { provider, key: key.trim().into() })
}
fn nonempty<'a>(first: &'a str, fallback: &'a str) -> &'a str { if first.trim().is_empty() { fallback } else { first } }
fn string<'a>(v: &'a Value, pointer: &str) -> &'a str { v.pointer(pointer).and_then(Value::as_str).unwrap_or("") }

pub fn read(source: &str) -> Result<(Vec<Candidate>, usize)> {
    let home = crate::runtime::home_dir().context("Home directory unavailable")?;
    let path = match source { "ccswitch" => home.join(".cc-switch/cc-switch.db"), "termany" => home.join(".termany/termany.db"), _ => anyhow::bail!("Unknown import source") };
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).context("Could not open the source database. Make sure the app is installed on this device.")?;
    read_database(&db, source)
}
fn read_database(db: &Connection, source: &str) -> Result<(Vec<Candidate>, usize)> {
    let mut candidates = Vec::new();
    let mut skipped = 0;
    if source == "ccswitch" {
        let mut query = db.prepare("SELECT id, app_type, name, settings_config FROM providers")?;
        let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?;
        for row in rows {
            let (id, app, name, raw) = row?;
            let item = (|| {
                let settings: Value = serde_json::from_str(&raw).ok()?;
                if app == "claude" || app == "claude-desktop" {
                    let mut item = candidate(source, &format!("{app}:{id}"), &name,
                        string(&settings, "/env/ANTHROPIC_BASE_URL"),
                        nonempty(string(&settings, "/env/ANTHROPIC_AUTH_TOKEN"), string(&settings, "/env/ANTHROPIC_API_KEY")))?;
                    item.provider.api_type = "messages".into();
                    item.provider.test_model = string(&settings, "/env/ANTHROPIC_MODEL").into();
                    return Some(item);
                }
                if app != "codex" { return None; }
                let doc = string(&settings, "/config").parse::<toml_edit::DocumentMut>().ok()?;
                let selected = doc.get("model_provider")?.as_str()?;
                let section = doc.get("model_providers")?.get(selected)?;
                let url = section.get("base_url")?.as_str()?;
                let key = section.get("experimental_bearer_token").and_then(|v| v.as_str()).unwrap_or_else(|| string(&settings, "/auth/OPENAI_API_KEY"));
                let mut item = candidate(source, &format!("{app}:{id}"), &name, url, key)?;
                item.provider.test_model = doc.get("model").and_then(|v| v.as_str()).unwrap_or("").into();
                item.provider.api_type = "chat_completions".into();
                Some(item)
            })();
            match item { Some(item) => candidates.push(item), None => skipped += 1 }
        }
    } else {
        let mut query = db.prepare("SELECT key, value FROM app_meta WHERE key = 'models'")?;
        let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (_, raw) = row?;
            let value: Value = serde_json::from_str(&raw).context("Invalid Termany provider configuration")?;
            for p in value.get("providers").and_then(Value::as_array).into_iter().flatten() {
                let mut item = if matches!(string(p, "/kind"), "openai" | "anthropic") {
                    candidate(source, &format!("models:{}", string(p, "/id")), string(p, "/name"), nonempty(string(p, "/apiBase"), if string(p, "/kind") == "anthropic" { "https://api.anthropic.com" } else { "https://api.openai.com/v1" }), string(p, "/apiKey"))
                } else { None };
                if let Some(ref mut item) = item {
                    let default = string(&value, "/defaultModel");
                    let prefix = format!("{}/", string(p, "/id"));
                    item.provider.test_model = default.strip_prefix(&prefix).filter(|v| !v.is_empty()).unwrap_or_else(|| p.get("models").and_then(Value::as_array).and_then(|m| m.first()).and_then(Value::as_str).unwrap_or("")).into();
                    item.provider.api_type = if string(p, "/kind") == "anthropic" { "messages" } else { "chat_completions" }.into();
                }
                match item { Some(item) => candidates.push(item), None => skipped += 1 }
            }
        }
    }
    Ok((candidates, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_termany_key_and_default_model() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE app_meta (key TEXT, value TEXT)").unwrap();
        db.execute("INSERT INTO app_meta VALUES ('models', ?)", [r#"{"providers":[{"id":"a","name":"Example","kind":"openai","apiBase":"https://example.com/v1","apiKey":"test-key","models":["model-first","model-selected"]},{"id":"b","kind":"unsupported"}],"defaultModel":"a/model-selected"}"#]).unwrap();
        let (items, skipped) = read_database(&db, "termany").unwrap();
        assert_eq!(items.len(), 1); assert_eq!(skipped, 1); assert_eq!(items[0].key, "test-key");
        assert_eq!(items[0].provider.base_url, "https://example.com/v1");
        assert_eq!(items[0].provider.test_model, "model-selected");
    }
    #[test]
    fn imports_ccswitch_selected_codex_section() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE providers (id TEXT, app_type TEXT, name TEXT, settings_config TEXT)").unwrap();
        let mut settings = serde_json::json!({"auth":{"OPENAI_API_KEY":"test-key"},"config":"model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://openrouter.ai/api/v1\""});
        settings["config"] = Value::String(format!("model = \"test-model\"\n{}\nwire_api = \"responses\"", settings["config"].as_str().unwrap()));
        db.execute("INSERT INTO providers VALUES ('a', 'codex', 'Example', ?)", [settings.to_string()]).unwrap();
        let (items, skipped) = read_database(&db, "ccswitch").unwrap();
        assert_eq!(skipped, 0); assert_eq!(items[0].provider.base_url, "https://openrouter.ai/api"); assert_eq!(items[0].key, "test-key");
        assert_eq!(items[0].provider.test_model, "test-model");
        assert_eq!(items[0].provider.api_type, "chat_completions");
    }
    #[test]
    fn ignores_termany_agent_provider_settings() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE app_meta (key TEXT, value TEXT)").unwrap();
        let data = serde_json::json!({"providers":[{"id":"a","appId":"codex","name":"Example","env":{"CUSTOM_KEY":"test-key"},"codex":{"section":{"base_url":"https://example.com/v1","env_key":"CUSTOM_KEY","wire_api":"responses"},"topLevel":{"model":"test-model"}}}]});
        db.execute("INSERT INTO app_meta VALUES ('agentProviders', ?)", [data.to_string()]).unwrap();
        let (items, skipped) = read_database(&db, "termany").unwrap();
        assert_eq!(skipped, 0);
        assert!(items.is_empty());
    }
    #[test]
    fn imports_ccswitch_anthropic_key_and_model() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE providers (id TEXT, app_type TEXT, name TEXT, settings_config TEXT)").unwrap();
        let data = serde_json::json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com","ANTHROPIC_API_KEY":"test-key","ANTHROPIC_MODEL":"test-model"}});
        db.execute("INSERT INTO providers VALUES ('a', 'claude', 'Example', ?)", [data.to_string()]).unwrap();
        let (items, skipped) = read_database(&db, "ccswitch").unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(items[0].key, "test-key");
        assert_eq!(items[0].provider.test_model, "test-model");
        assert_eq!(items[0].provider.api_type, "messages");
    }

    #[test]
    fn skips_same_endpoint_across_sources_and_with_trailing_slash() {
        let original = candidate("termany", "a", "Original", "https://example.com/v1", "key-a").unwrap();
        let mut duplicate = candidate("ccswitch", "b", "Other name", "https://example.com/v1", "key-b").unwrap();
        duplicate.provider.base_url = "https://EXAMPLE.com/v1/".into();
        duplicate.provider.api_type = "messages".into();
        let existing = vec![original.provider];
        assert!(already_imported(&existing, &duplicate.provider));
        duplicate.provider.base_url = "https://example.com/other".into();
        assert!(!already_imported(&existing, &duplicate.provider));
        let mut batch = Vec::new();
        assert!(!already_imported(&batch, &duplicate.provider));
        batch.push(duplicate.provider.clone());
        assert!(already_imported(&batch, &duplicate.provider));
    }

}
