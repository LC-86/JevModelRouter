// Native config adapters. Parsing and editing are isolated from IO for fixture testing.
use std::{fs, path::{Path, PathBuf}};
use anyhow::{anyhow, Context, Result};
use serde::{Serialize, Deserialize};
use serde_json::{json, Value};
use toml_edit::{DocumentMut, Item, Table, value};

fn config_override(var:&str,home:&Path)->Option<std::ffi::OsString>{
    if dirs::home_dir().as_deref()==Some(home){std::env::var_os(var)}else{None}
}
fn home_dir(var: &str, home: &Path, fallback: &str) -> PathBuf {
    config_override(var,home).filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(fallback))
}
pub fn paths(id: &str, home: &Path) -> Vec<PathBuf> {
    match id {
        "grok" => vec![home_dir("GROK_HOME", home, ".grok").join("config.toml")],
        "kimi" => vec![if home.join(".kimi-code/config.toml").exists() { home.join(".kimi-code/config.toml") }
            else if home.join(".kimi/config.toml").exists() { home.join(".kimi/config.toml") }
            else { home.join(".kimi-code/config.toml") }],
        "openclaw" => vec![config_override("OPENCLAW_CONFIG_PATH",home).map(PathBuf::from)
            .unwrap_or_else(|| home_dir("OPENCLAW_STATE_DIR", home, ".openclaw").join("openclaw.json"))],
        "opencode" => {
            let dir = home_dir("XDG_CONFIG_HOME", home, ".config").join("opencode");
            vec![config_override("OPENCODE_CONFIG",home).map(PathBuf::from).unwrap_or_else(|| {
                if dir.join("opencode.jsonc").exists() {dir.join("opencode.jsonc")} else {dir.join("opencode.json")}
            })]
        }
        "hermes" => vec![home_dir("HERMES_HOME", home, ".hermes").join("config.yaml")],
        "omp" => {let d = home_dir("PI_CODING_AGENT_DIR", home, ".omp/agent"); vec![d.join("models.yml"), d.join("config.yml")]},
        "fastclaw" => vec![home_dir("FASTCLAW_HOME", home, ".fastclaw").join("fastclaw.db")],
        "gemini" => vec![home.join(".gemini/settings.json"), home.join(".gemini/.env")],
        "cursor" => vec![home_dir("CURSOR_CONFIG_DIR", home, ".cursor").join("cli-config.json")],
        _ => vec![],
    }
}
pub fn limitation(id: &str) -> Option<&'static str> {
    match id {

        "cursor" => Some("Cursor CLI does not expose a documented custom model endpoint setting."),
        _ => None,
    }
}
pub fn supported(id: &str) -> bool {
    matches!(id, "grok" | "kimi" | "openclaw" | "opencode" | "hermes" | "omp" | "fastclaw" | "gemini")
}
fn parse_json(s: &str) -> Result<Value> {
    let v = if s.trim().is_empty() { json!({}) } else { json5::from_str(s).context("Invalid JSON configuration")? };
    if !v.is_object() { return Err(anyhow!("Configuration must be an object")); } Ok(v)
}
fn parse_yaml(s: &str) -> Result<Value> {
    let v = if s.trim().is_empty() { json!({}) } else { serde_yaml::from_str(s).context("Invalid YAML configuration")? };
    if !v.is_object() { return Err(anyhow!("Configuration must be an object")); } Ok(v)
}
fn merge(v: &mut Value, patch: Value) {
    if let (Some(target), Some(source)) = (v.as_object_mut(), patch.as_object()) {
        for (key, item) in source { merge(target.entry(key).or_insert(Value::Null), item.clone()); }
    } else { *v = patch; }
}
fn table<'a>(doc: &'a mut dyn toml_edit::TableLike, key: &str) -> Result<&'a mut dyn toml_edit::TableLike> {
    if doc.get(key).is_none() { doc.insert(key, Item::Table(Table::new())); }
    doc.get_mut(key).unwrap().as_table_like_mut().ok_or_else(|| anyhow!("Invalid table: {key}"))
}
pub fn render(id: &str, original: &[String], port: u16, binding: &str, api: &str) -> Result<Vec<String>> {
    render_for_agent(id, id, original, port, binding, api)
}
pub fn render_for_agent(id: &str, agent_id: &str, original: &[String], port: u16, binding: &str, api: &str) -> Result<Vec<String>> {
    let local_key = format!("autojev-local-{agent_id}");
    let model = crate::agent_catalog::wire_id(binding);
    let base = format!("http://127.0.0.1:{port}/v1");
    let messages = api == "messages";
    if id == "gemini" {
        if messages {return Err(anyhow!("Gemini bridge requires a Chat Completions model"));}
        let mut d=parse_json(&original[0])?;
        merge(&mut d,json!({"model":{"name":model},"security":{"auth":{"selectedType":"gemini-api-key"}}}));
        let mut lines=original[1].lines().filter(|line|{
            let key=line.trim_start().strip_prefix("export ").unwrap_or(line.trim_start()).split('=').next().unwrap_or("").trim();
            !matches!(key,"GEMINI_API_KEY"|"GOOGLE_GEMINI_BASE_URL"|"GEMINI_MODEL")
        }).map(str::to_owned).collect::<Vec<_>>();
        lines.push(format!("GEMINI_API_KEY={local_key}"));
        lines.push(format!("GEMINI_MODEL={model}"));
        lines.push(format!("GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:{port}"));
        return Ok(vec![serde_json::to_string_pretty(&d)?,lines.join("\n")+"\n"]);
    }
    if id == "grok" || id == "kimi" {
        let mut d = original[0].parse::<DocumentMut>().context("Invalid TOML configuration")?;
        if id == "grok" {
            table(d.as_table_mut(), "models")?.insert("default", value("autojev"));
            let models = table(d.as_table_mut(), "model")?;
            let m = table(models, "autojev")?;
            for (k, v) in [("name","AutoJev"), ("model",model.as_str()), ("base_url",base.as_str()),
                ("api_key",local_key.as_str()), ("api_backend",if messages {"messages"} else {"chat_completions"})] { m.insert(k, value(v)); }
        } else {
            d["default_model"] = value("autojev");
            let providers = table(d.as_table_mut(), "providers")?;
            let p = table(providers, "autojev")?;
            for (k,v) in [("type",if messages {"anthropic"} else {"openai"}), ("base_url",base.as_str()),("api_key",local_key.as_str())] {p.insert(k,value(v));}
            table(p, "custom_headers")?.insert("x-autojev-agent", value(agent_id));
            let models = table(d.as_table_mut(), "models")?;
            let m = table(models, "autojev")?;
            m.insert("provider", value("autojev")); m.insert("model", value(&model));
            m.insert("max_context_size", value(128000i64));
        }
        return Ok(vec![d.to_string()]);
    }
    let mut d = if matches!(id,"hermes"|"omp") {parse_yaml(&original[0])?} else {parse_json(&original[0])?};
    match id {
        "openclaw" => merge(&mut d,json!({
            "models":{"providers":{"autojev":{"baseUrl":base,"apiKey":local_key,
                "api": if messages {"anthropic-messages"} else {"openai-completions"},
                "models":[{"id":model,"name":"AutoJev","contextWindow":128000,"maxTokens":8192}]}}},
            "agents":{"defaults":{"model":{"primary":format!("autojev/{model}")},
                "models":{format!("autojev/{model}"): {}}}}
        })),
        "opencode" => merge(&mut d,json!({
            "provider":{"autojev":{"npm":if messages {"@ai-sdk/anthropic"} else {"@ai-sdk/openai-compatible"},
                "name":"AutoJev","options":{"baseURL":base,"apiKey":local_key},
                "models":{model.clone():{"name":"AutoJev"}}}},
            "model":format!("autojev/{model}")
        })),
        "hermes" => {
            if messages {return Err(anyhow!("This Hermes adapter requires a Chat Completions model"));}
            for key in ["model", "providers"] {
                if !d[key].is_null() && !d[key].is_object() { return Err(anyhow!("Hermes {key} must be a mapping")); }
            }
            if let Some(provider) = d.pointer_mut("/providers/autojev").and_then(Value::as_object_mut) {
                provider.remove("models");
                provider.remove("autojev_default_binding");
                if let Some(headers) = provider.get_mut("extra_headers").and_then(Value::as_object_mut) { headers.remove("x-autojev-binding"); headers.remove("x-autojev-agent"); }
            }
            merge(&mut d,json!({
                "providers":{"autojev":{"name":"AutoJev Model Gateway","api":base,"api_key":local_key,"transport":"chat_completions","default_model":model}},
                "model":{"provider":"autojev","default":model,"base_url":base,"api_key":local_key,"api_mode":"chat_completions"}
            }));
        },
        "omp" => {
            merge(&mut d,json!({"providers":{"autojev":{"baseUrl":base,"apiKey":local_key,"api":if messages {"anthropic-messages"} else {"openai-completions"},
                "headers":{"x-autojev-agent":agent_id},
                "models":[{"id":model,"name":"AutoJev","contextWindow":128000,"maxTokens":8192}]}}}));
            let mut settings = parse_yaml(&original[1])?;
            merge(&mut settings,json!({"modelRoles":{"default":format!("autojev/{model}")}}));
            return Ok(vec![serde_yaml::to_string(&d)?,serde_yaml::to_string(&settings)?]);
        },
        _ => return Err(anyhow!("Unsupported agent")),
    }
    Ok(vec![if id == "hermes" {serde_yaml::to_string(&d)?} else {serde_json::to_string_pretty(&d)?}])
}
#[derive(Serialize,Deserialize)]
struct Backup { original: Vec<Option<Vec<u8>>> }
fn render_grok_catalog(content: &str, binding: &str, catalog: &[crate::agent_catalog::Entry]) -> Result<String> {
    let default = catalog.iter().find(|e| e.binding == binding).ok_or_else(|| anyhow!("Default model must be selected"))?;
    let mut doc = content.parse::<DocumentMut>()?;
    let template = doc["model"]["autojev"].clone();
    table(doc.as_table_mut(), "models")?.insert("default", value(&default.id));
    let models = table(doc.as_table_mut(), "model")?;
    let owned: Vec<String> = models.iter().filter(|(_, item)| {
        item.get("api_key").and_then(Item::as_str) == Some("autojev-local-grok")
            && item.get("base_url").and_then(Item::as_str) == template.get("base_url").and_then(Item::as_str)
    }).map(|(key, _)| key.to_owned()).collect();
    for key in owned { models.remove(&key); }
    for entry in catalog {
        if models.get(&entry.id).is_some() { return Err(anyhow!("Grok model ID conflicts with an existing model: {}", entry.id)); }
        let mut model = template.clone();
        let fields = model.as_table_like_mut().ok_or_else(|| anyhow!("Invalid Grok model configuration"))?;
        fields.insert("name", value(&entry.name));
        fields.insert("model", value(&entry.id));
        models.insert(&entry.id, model);
    }
    Ok(doc.to_string())
}
fn render_kimi_catalog(content: &str, binding: &str, catalog: &[crate::agent_catalog::Entry]) -> Result<String> {
    let default = catalog.iter().find(|e| e.binding == binding).ok_or_else(|| anyhow!("Default model must be selected"))?;
    let mut doc = content.parse::<DocumentMut>()?;
    doc["default_model"] = value(&default.id);
    let models = table(doc.as_table_mut(), "models")?;
    let owned: Vec<String> = models.iter().filter(|(_, item)| item.get("provider").and_then(Item::as_str) == Some("autojev"))
        .map(|(key, _)| key.to_owned()).collect();
    for key in owned { models.remove(&key); }
    for entry in catalog {
        if models.get(&entry.id).is_some() { return Err(anyhow!("Kimi model ID conflicts with an existing model: {}", entry.id)); }
        let m = table(models, &entry.id)?;
        m.insert("provider", value("autojev"));
        m.insert("model", value(&entry.id));
        m.insert("max_context_size", value(128000i64));
    }
    Ok(doc.to_string())
}
fn backup_path(paths: &[PathBuf]) -> PathBuf {paths[0].with_extension("autojev-backup.json")}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().ok_or_else(|| anyhow!("Invalid configuration path"))?)?;
    let tmp = path.with_extension(format!("autojev-{}.tmp",uuid::Uuid::new_v4()));
    use std::io::Write;
    let mut options = fs::OpenOptions::new(); options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let result = (|| -> Result<()> {let mut file=options.open(&tmp)?;file.write_all(bytes)?;file.sync_all()?;fs::rename(&tmp,path)?;Ok(())})();
    if result.is_err(){let _=fs::remove_file(&tmp);} result
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {Ok(v)=>Ok(Some(v)),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(e)=>Err(e.into())}
}
fn apply(paths: &[PathBuf], contents: &[Option<Vec<u8>>]) -> Result<()> {
    for (path,bytes) in paths.iter().zip(contents) {
        if let Some(bytes)=bytes {atomic(path,bytes)?;}
        else if path.exists(){fs::remove_file(path)?;}
    } Ok(())
}
pub fn connect(id: &str, port: u16, binding: &str, api: &str, home: &Path) -> Result<()> {
    connect_with_model(id, port, binding, api, home, None)
}
pub fn connect_with_model(id: &str, port: u16, binding: &str, api: &str, home: &Path, model: Option<(&str, &str)>) -> Result<()> {
    connect_options(id, port, binding, api, home, model, None)
}
pub fn connect_catalog(id: &str, port: u16, binding: &str, api: &str, home: &Path, catalog: &[crate::agent_catalog::Entry]) -> Result<()> {
    connect_options(id, port, binding, api, home, None, Some(catalog))
}
fn connect_options(id: &str, port: u16, binding: &str, api: &str, home: &Path, model: Option<(&str, &str)>, catalog: Option<&[crate::agent_catalog::Entry]>) -> Result<()> {
    if !supported(id){return Err(anyhow!(limitation(id).unwrap_or("Unsupported agent")));}
    if id=="fastclaw" {return super::fastclaw_adapter::connect(&paths(id,home)[0],port,binding,api);}
    let paths=paths(id,home);
    if backup_path(&paths).exists() && paths.iter().all(|p|crate::ownership::owner(p).ok().flatten().is_none()) {
        if let Some(previous)=[port,crate::config::DEFAULT_PORT,crate::config::DEV_PORT,9487].into_iter().find(|port|paths.iter().any(|p|crate::ownership::points_to(p,*port))){adopt_legacy(id,home,previous)?;}
    }
    let original=paths.iter().map(|p|read_optional(p)).collect::<Result<Vec<_>>>()?;
    let text=original.iter().map(|v|String::from_utf8(v.clone().unwrap_or_default()).map_err(Into::into)).collect::<Result<Vec<_>>>()?;
    let mut rendered=render(id,&text,port,binding,api)?;
    if id == "hermes" {
        if let Some((model_id, name)) = model {
            let mut d = parse_yaml(&rendered[0])?;
            d["model"]["default"] = model_id.into();
            let provider = &mut d["providers"]["autojev"];
            provider["default_model"] = model_id.into();
            provider["models"] = json!({model_id: {"name": name}});
            provider["extra_headers"] = json!({"x-autojev-binding": binding});
            rendered[0] = serde_yaml::to_string(&d)?;
        }
    }
    if let Some(catalog) = catalog.filter(|_| id == "kimi") {
        rendered[0] = render_kimi_catalog(&rendered[0], binding, catalog)?;
    }
    if let Some(catalog) = catalog.filter(|_| id == "grok") {
        rendered[0] = render_grok_catalog(&rendered[0], binding, catalog)?;
    }
    if let Some(catalog) = catalog.filter(|_| !matches!(id, "kimi" | "grok")) {
        let default = catalog.iter().find(|entry| entry.binding == binding).ok_or_else(|| anyhow!("Default model must be selected"))?;
        let mut d = if matches!(id, "hermes" | "omp") { parse_yaml(&rendered[0])? } else { parse_json(&rendered[0])? };
        match id {
            "hermes" => {
                d["model"]["default"] = default.id.clone().into();
                d["providers"]["autojev"]["default_model"] = default.id.clone().into();
                d["providers"]["autojev"]["models"] = catalog.iter().map(|e| (e.id.clone(), json!({"name":e.name}))).collect::<serde_json::Map<_,_>>().into();
                d["providers"]["autojev"]["extra_headers"] = json!({"x-autojev-agent":"hermes"});
                d["providers"]["autojev"]["autojev_default_binding"] = binding.into();
            },
            "opencode" => {
                d["provider"]["autojev"]["models"] = catalog.iter().map(|e| (e.id.clone(), json!({"name":e.name}))).collect::<serde_json::Map<_,_>>().into();
                d["model"] = format!("autojev/{}", default.id).into();
            },
            "openclaw" | "omp" => {
                let items = catalog.iter().map(|e| json!({"id":e.id,"name":e.name,"contextWindow":128000,"maxTokens":8192})).collect::<Vec<_>>();
                if id == "omp" {
                    d["providers"]["autojev"]["models"] = items.into();
                    let mut settings = parse_yaml(&rendered[1])?;
                    settings["modelRoles"]["default"] = format!("autojev/{}", default.id).into();
                    rendered[1] = serde_yaml::to_string(&settings)?;
                }
                else {
                    d["models"]["providers"]["autojev"]["models"] = items.into();
                    d["agents"]["defaults"]["model"]["primary"] = format!("autojev/{}", default.id).into();
                    if let Some(models) = d["agents"]["defaults"]["models"].as_object_mut() { models.retain(|key, _| !key.starts_with("autojev/")); }
                    for e in catalog { d["agents"]["defaults"]["models"][format!("autojev/{}", e.id)] = json!({}); }
                }
            },
            _ => return Err(anyhow!("Agent does not support a model catalog")),
        }
        rendered[0] = if matches!(id, "hermes" | "omp") {serde_yaml::to_string(&d)?} else {serde_json::to_string_pretty(&d)?};
    }
    let backup=backup_path(&paths);
    let fresh=!backup.exists();
    if fresh {atomic(&backup,&serde_json::to_vec(&Backup{original:original.clone()})?)?;}
    let saved:Backup=serde_json::from_slice(&fs::read(&backup)?)?;
    let originals=saved.original.iter().map(|v|v.clone().map(String::from_utf8).transpose().map_err(Into::into)).collect::<Result<Vec<_>>>()?;
    if let Err(error)=crate::ownership::apply(&paths,port,&originals,&rendered) {if fresh{fs::remove_file(backup)?;}return Err(error);}
    Ok(())
}
pub fn restore(id: &str, home: &Path) -> Result<()> {
    if id=="fastclaw" {return super::fastclaw_adapter::restore(&paths(id,home)[0]);}
    let paths=paths(id,home); if paths.is_empty(){return Err(anyhow!("Unsupported agent"));}
    if let Some(port)=crate::ownership::owner(&paths[0])? {
        for path in &paths {crate::ownership::prepare(path,port)?;}
        for path in &paths {crate::ownership::restore(path,port)?;}
        let backup=backup_path(&paths);if backup.exists(){fs::remove_file(backup)?;}return Ok(());
    }
    let backup=backup_path(&paths);
    let saved:Backup=serde_json::from_slice(&fs::read(&backup).context("No AutoJev backup exists")?)?;
    if saved.original.len()!=paths.len(){return Err(anyhow!("Invalid backup"));}
    let current=paths.iter().map(|p|read_optional(p)).collect::<Result<Vec<_>>>()?;
    if let Err(error)=apply(&paths,&saved.original){apply(&paths,&current)?;return Err(error);}
    fs::remove_file(backup)?; Ok(())
}
pub fn binding(id: &str, home: &Path) -> Option<String> {
    if id=="fastclaw" {return super::fastclaw_adapter::binding(&paths(id,home)[0]);}
    let paths=paths(id,home); let text=fs::read_to_string(paths.first()?).ok()?;
    let (model,base): (String,String) = match id {
        "grok"|"kimi" => {
            let d=text.parse::<DocumentMut>().ok()?;
            let read=|keys:&[&str]| -> Option<String> {
                let mut item=d.as_item(); for key in keys{item=item.get(key)?;} item.as_str().map(str::to_owned)
            };
            if id=="grok" {
                let selected = read(&["models","default"])?;
                (read(&["model",&selected,"model"])?,read(&["model",&selected,"base_url"])?)
            } else {
                let selected = read(&["default_model"])?;
                if read(&["models",&selected,"provider"])? != "autojev" { return None; }
                (read(&["models",&selected,"model"])?,read(&["providers","autojev","base_url"])?)
            }
        },
        _ => {
            let d=if matches!(id,"hermes"|"omp"){parse_yaml(&text).ok()?}else{parse_json(&text).ok()?};
            let (model,base)=match id {
                "gemini"=>{
                    if d.pointer("/security/auth/selectedType")?.as_str()?!="gemini-api-key"{return None;}
                    let env=fs::read_to_string(paths.get(1)?).ok()?;
                    let url=env.lines().find_map(|l|l.strip_prefix("GOOGLE_GEMINI_BASE_URL="))?;
                    (d.pointer("/model/name")?.as_str()?.to_owned(),url.to_owned())
                },
                "openclaw"=>(d.pointer("/agents/defaults/model/primary")?.as_str()?.strip_prefix("autojev/")?.to_owned(),d.pointer("/models/providers/autojev/baseUrl")?.as_str()?.to_owned()),
                "opencode"=>(d.get("model")?.as_str()?.strip_prefix("autojev/")?.to_owned(),d.pointer("/provider/autojev/options/baseURL")?.as_str()?.to_owned()),
                "hermes"=>{
                    let base = d.pointer("/model/base_url")?.as_str()?.to_owned();
                    if d.pointer("/model/provider").and_then(Value::as_str)==Some("autojev") && base.starts_with("http://127.0.0.1:") {
                        if let Some(binding)=d.pointer("/providers/autojev/autojev_default_binding").and_then(Value::as_str) {return Some(binding.into());}
                        if let Some(binding)=d.pointer("/providers/autojev/extra_headers/x-autojev-binding").and_then(Value::as_str) {return Some(binding.into());}
                    }
                    (d.pointer("/model/default")?.as_str()?.to_owned(),base)
                },
                "omp"=>{
                    let settings=parse_yaml(&fs::read_to_string(paths.get(1)?).ok()?).ok()?;
                    (settings.pointer("/modelRoles/default")?.as_str()?.strip_prefix("autojev/")?.to_owned(),d.pointer("/providers/autojev/baseUrl")?.as_str()?.to_owned())
                },
                _=>return None,
            };(model,base)
        }
    };
    if !base.starts_with("http://127.0.0.1:"){return None;}
    Some(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grok_catalog_injects_all_choices_and_restores_existing_models() {
        let home = tempfile::tempdir().unwrap();
        let path = paths("grok", home.path()).remove(0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[models]\ndefault = 'original'\n[model.original]\nmodel = 'original'\nbase_url = 'https://example.com/v1'\n").unwrap();
        let entries = vec![
            crate::agent_catalog::Entry { binding: "fast".into(), id: "autojev/fast".into(), name: "Fast route".into() },
            crate::agent_catalog::Entry { binding: "model/uuid".into(), id: "openrouter/openai/gpt-test".into(), name: "GPT test".into() },
        ];
        connect_catalog("grok", 9526, "model/uuid", "chat_completions", home.path(), &entries).unwrap();
        let doc = fs::read_to_string(&path).unwrap().parse::<DocumentMut>().unwrap();
        assert_eq!(doc["model"].as_table().unwrap().len(), 3);
        for entry in &entries {
            assert_eq!(doc["model"][&entry.id]["model"].as_str(), Some(entry.id.as_str()));
            assert_eq!(doc["model"][&entry.id]["name"].as_str(), Some(entry.name.as_str()));
            assert_eq!(doc["model"][&entry.id]["api_key"].as_str(), Some("autojev-local-grok"));
        }
        assert_eq!(binding("grok", home.path()).as_deref(), Some("openrouter/openai/gpt-test"));
        connect_catalog("grok", 9526, "fast", "messages", home.path(), &entries[..1]).unwrap();
        let doc = fs::read_to_string(&path).unwrap().parse::<DocumentMut>().unwrap();
        assert!(doc["model"].get("openrouter/openai/gpt-test").is_none());
        assert_eq!(doc["model"]["autojev/fast"]["api_backend"].as_str(), Some("messages"));
        assert_eq!(binding("grok", home.path()).as_deref(), Some("autojev/fast"));
        restore("grok", home.path()).unwrap();
        let doc = fs::read_to_string(&path).unwrap().parse::<DocumentMut>().unwrap();
        assert_eq!(doc["models"]["default"].as_str(), Some("original"));
        assert_eq!(doc["model"].as_table().unwrap().len(), 1);
    }
    #[test]
    fn every_file_adapter_injects_a_recognizable_agent_marker() {
        for id in ["grok", "kimi", "openclaw", "opencode", "hermes", "omp", "gemini"] {
            for api in ["chat_completions", "messages"] {
                if api == "messages" && matches!(id, "hermes" | "gemini") { continue; }
                let output = render(id, &[String::new(), String::new()], 9526, "autojev/fast", api).unwrap();
                let key = match id {
                    "grok" | "kimi" => {
                        let doc = output[0].parse::<DocumentMut>().unwrap();
                        doc[if id == "grok" { "model" } else { "providers" }]["autojev"]["api_key"].as_str().unwrap().to_owned()
                    },
                    "gemini" => output[1].lines().find_map(|l| l.strip_prefix("GEMINI_API_KEY=")).unwrap().to_owned(),
                    _ => {
                        let doc = if matches!(id, "hermes" | "omp") { parse_yaml(&output[0]).unwrap() } else { parse_json(&output[0]).unwrap() };
                        doc.pointer(match id {
                            "openclaw" => "/models/providers/autojev/apiKey",
                            "opencode" => "/provider/autojev/options/apiKey",
                            "hermes" => "/providers/autojev/api_key",
                            _ => "/providers/autojev/apiKey",
                        }).unwrap().as_str().unwrap().to_owned()
                    }
                };
                let mut headers = axum::http::HeaderMap::new();
                let (name, marker) = if id == "gemini" { ("x-goog-api-key", key) }
                    else if api == "messages" { ("x-api-key", key) } else { ("authorization", format!("Bearer {key}")) };
                headers.insert(name, marker.parse().unwrap());
                let capture = crate::traffic::Capture::new(api, &json!({}), &headers);
                assert_eq!(capture.lock().unwrap().log.agent, id, "{id}: {api}");
            }
        }
    }
    #[test]
    fn kimi_catalog_replaces_selected_models_and_preserves_other_providers() {
        let home = tempfile::tempdir().unwrap();
        let path = paths("kimi", home.path()).remove(0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "default_model = 'original'\n[models.original]\nprovider = 'moonshot'\nmodel = 'original'\n";
        fs::write(&path, original).unwrap();
        let entries = vec![
            crate::agent_catalog::Entry { binding: "fast".into(), id: "autojev/fast".into(), name: "Fast".into() },
            crate::agent_catalog::Entry { binding: "model/uuid".into(), id: "openrouter/openai/gpt-test".into(), name: "GPT".into() },
        ];
        connect_catalog("kimi", 9526, "model/uuid", "chat_completions", home.path(), &entries).unwrap();
        let doc = fs::read_to_string(&path).unwrap().parse::<DocumentMut>().unwrap();
        assert_eq!(doc["models"].as_table().unwrap().len(), 3);
        for entry in &entries { assert_eq!(doc["models"][&entry.id]["model"].as_str(), Some(entry.id.as_str())); }
        assert_eq!(binding("kimi", home.path()).as_deref(), Some("openrouter/openai/gpt-test"));
        connect_catalog("kimi", 9526, "fast", "messages", home.path(), &entries[..1]).unwrap();
        let doc = fs::read_to_string(&path).unwrap().parse::<DocumentMut>().unwrap();
        assert!(doc["models"].get("openrouter/openai/gpt-test").is_none());
        assert_eq!(doc["models"]["original"]["provider"].as_str(), Some("moonshot"));
        assert_eq!(binding("kimi", home.path()).as_deref(), Some("autojev/fast"));
        restore("kimi", home.path()).unwrap();
        let doc = fs::read_to_string(&path).unwrap().parse::<DocumentMut>().unwrap();
        assert_eq!(doc["default_model"].as_str(), Some("original"));
        assert_eq!(doc["models"].as_table().unwrap().len(), 1);
    }
    #[test]
    fn multiple_models_are_injected_replaced_and_restored() {
        use crate::agent_catalog::Entry;
        let entries = vec![Entry {binding:"model/one".into(), id:"first-model".into(), name:"First".into()}, Entry {binding:"model/two".into(), id:"second-model".into(), name:"Second".into()}];
        for agent in ["hermes", "opencode", "openclaw", "omp"] {
            let home = tempfile::tempdir().unwrap();
            connect_catalog(agent, 9526, "model/two", "chat_completions", home.path(), &entries).unwrap();
            let file = paths(agent, home.path()).remove(0);
            let content = fs::read_to_string(&file).unwrap();
            assert!(content.contains("First") && content.contains("Second"), "{agent}");
            assert_eq!(binding(agent, home.path()).as_deref(), Some(if agent == "hermes" { "model/two" } else { "second-model" }));
            if agent == "omp" {
                let d = parse_yaml(&content).unwrap();
                assert_eq!(d["providers"]["autojev"]["headers"]["x-autojev-agent"], "omp");
            }
            if agent == "hermes" {
                let d = parse_yaml(&content).unwrap();
                assert_eq!(d["model"]["default"], "second-model");
                assert_eq!(d["providers"]["autojev"]["extra_headers"]["x-autojev-agent"], "hermes");
                assert!(d["providers"]["autojev"]["extra_headers"]["x-autojev-binding"].is_null());
            }
            connect_catalog(agent, 9526, "model/one", "chat_completions", home.path(), &entries[..1]).unwrap();
            assert!(!fs::read_to_string(&file).unwrap().contains("Second"));
            assert_eq!(binding(agent, home.path()).as_deref(), Some(if agent == "hermes" { "model/one" } else { "first-model" }));
            restore(agent, home.path()).unwrap();
            assert!(!file.exists());
        }
    }
    #[test]
    fn hermes_uses_public_model_id_and_separate_binding() {
        let home = tempfile::tempdir().unwrap();
        connect_with_model("hermes", 9526, "model/internal-uuid", "chat_completions", home.path(), Some(("deepseek-v4-flash", "DeepSeek V4 Flash"))).unwrap();
        let path = paths("hermes", home.path()).remove(0);
        let d = parse_yaml(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(d["model"]["default"], "deepseek-v4-flash");
        assert_eq!(d["providers"]["autojev"]["models"]["deepseek-v4-flash"]["name"], "DeepSeek V4 Flash");
        assert_eq!(binding("hermes", home.path()).as_deref(), Some("model/internal-uuid"));
        connect("hermes", 9526, "daily", "chat_completions", home.path()).unwrap();
        assert_eq!(binding("hermes", home.path()).as_deref(), Some("daily"));
        restore("hermes", home.path()).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn hermes_registers_named_gateway_and_preserves_other_providers() {
        let original = "providers:\n  termany_gateway_existing:\n    name: Termany Model Gateway\n    api: http://localhost:1234/v1\nmodel:\n  provider: custom\n";
        let rendered = render("hermes", &[original.into()], 9526, "model/one", "chat_completions").unwrap();
        let d = parse_yaml(&rendered[0]).unwrap();
        assert_eq!(d["model"]["provider"], "autojev");
        assert_eq!(d["providers"]["autojev"]["name"], "AutoJev Model Gateway");
        assert_eq!(d["providers"]["autojev"]["api"], d["model"]["base_url"]);
        assert_eq!(d["providers"]["autojev"]["default_model"], "model/one");
        assert_eq!(d["providers"]["termany_gateway_existing"]["name"], "Termany Model Gateway");
        let updated = render("hermes", &rendered, 9876, "model/two", "chat_completions").unwrap();
        let d = parse_yaml(&updated[0]).unwrap();
        assert_eq!(d["providers"].as_object().unwrap().len(), 2);
        assert_eq!(d["providers"]["autojev"]["default_model"], "model/two");
        assert!(render("hermes", &["providers: []".into()], 9526, "auto", "chat_completions").is_err());
    }
    #[test]
    fn gemini_injection_replaces_old_auth_and_model() {
        let original = vec![r#"{"security":{"auth":{"selectedType":"vertex-ai"}}}"#.into(), "export GEMINI_MODEL=gemini-3-pro-preview\nOTHER=value\n".into()];
        let output = render("gemini", &original, 9526, "quality", "chat_completions").unwrap();
        let settings = parse_json(&output[0]).unwrap();
        assert_eq!(settings["security"]["auth"]["selectedType"], "gemini-api-key");
        let model = settings["model"]["name"].as_str().unwrap();
        assert!(output[1].contains(&format!("GEMINI_MODEL={model}\n")));
        assert!(!output[1].contains("gemini-3-pro-preview"));
        assert!(output[1].contains("OTHER=value"));
        assert_eq!(output[1].matches("GEMINI_MODEL=").count(), 1);
    }
    #[test]
    fn all_file_adapters_roundtrip_and_preserve_originals() {
        for id in ["grok","kimi","openclaw","opencode","hermes","omp","gemini"] {
            let home=tempfile::tempdir().unwrap();
            let files=paths(id,home.path());
            fs::create_dir_all(files[0].parent().unwrap()).unwrap();
            let source=match id {
                "grok"|"kimi"=>"# original comment
unrelated = 42
",
                "hermes"|"omp"=>"# original comment
unrelated: 42
",
                _=>"{/* original comment */ unrelated: 42,}",
            };
            fs::write(&files[0],source).unwrap();
            connect(id,9526,"model/one","chat_completions",home.path()).unwrap();
            assert_eq!(binding(id,home.path()).as_deref(),Some("model/one"),"{id}");
            connect(id,9876,"model/two","chat_completions",home.path()).unwrap();
            assert_eq!(binding(id,home.path()).as_deref(),Some("model/two"),"{id}");
            assert!(fs::read_to_string(&files[0]).unwrap().contains("unrelated"));
            restore(id,home.path()).unwrap();
            assert_eq!(fs::read_to_string(&files[0]).unwrap(),source,"{id}");
            for extra in files.iter().skip(1) {assert!(!extra.exists(),"{id} created file must be removed");}
            assert!(binding(id,home.path()).is_none());
        }
    }
    #[test] fn invalid_config_is_not_overwritten() {
        let home=tempfile::tempdir().unwrap();let path=paths("openclaw",home.path()).remove(0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();fs::write(&path,"broken {").unwrap();
        assert!(connect("openclaw",9526,"x","chat_completions",home.path()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(),"broken {");
        assert!(!backup_path(&[path]).exists());
    }
    #[test] fn native_messages_supported_where_advertised() {
        for id in ["grok","kimi","openclaw","opencode","omp"] {
            let home=tempfile::tempdir().unwrap();
            connect(id,9526,"anthropic","messages",home.path()).unwrap();
            assert_eq!(binding(id,home.path()).as_deref(),Some("anthropic"));
            restore(id,home.path()).unwrap();
            for path in paths(id,home.path()){assert!(!path.exists());}
        }
    }
    #[test] fn kimi_injection_identifies_both_api_modes() {
        for api in ["chat_completions", "messages"] {
            let rendered = render("kimi", &[String::new()], 9526, "autojev/fast", api).unwrap();
            let doc = rendered[0].parse::<DocumentMut>().unwrap();
            assert_eq!(doc["providers"]["autojev"]["api_key"].as_str(), Some("autojev-local-kimi"));
            assert_eq!(doc["providers"]["autojev"]["custom_headers"]["x-autojev-agent"].as_str(), Some("kimi"));
        }
    }
}

pub fn adopt_legacy(id:&str,home:&Path,port:u16)->Result<()> {
    let paths=paths(id,home);let saved:Backup=serde_json::from_slice(&fs::read(backup_path(&paths)).context("No legacy backup; reconnect agent before stopping")?)?;
    anyhow::ensure!(paths.len()==saved.original.len(),"Invalid legacy backup");
    for(i,path)in paths.iter().enumerate(){
        let keys:Vec<&str>=match(id,i){
            ("grok",_)=>vec!["/models/default","/model/autojev"],
            ("kimi",_)=>vec!["/default_model","/providers/autojev","/models/autojev"],
            ("hermes",_)=>vec!["/providers/autojev","/model/provider","/model/default","/model/base_url","/model/api_key","/model/api_mode"],
            ("opencode",_)=>vec!["/model","/provider/autojev"],
            ("openclaw",_)=>vec!["/models/providers/autojev","/agents/defaults/model/primary"],
            ("omp",0)=>vec!["/providers/autojev"],("omp",_)=>vec!["/modelRoles/default"],
            ("gemini",0)=>vec!["/model/name","/security/auth/selectedType"],("gemini",_)=>vec!["/GEMINI_API_KEY","/GOOGLE_GEMINI_BASE_URL"],
            _=>return Err(anyhow!("Unsupported legacy adapter"))
        };
        let old=saved.original[i].as_deref().map(std::str::from_utf8).transpose()?;
        crate::ownership::adopt(path,port,old,&keys)?;
    }Ok(())
}
