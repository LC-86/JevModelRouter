//! Explicit API source identity; protocol and secret storage remain in existing modules.
use crate::config::{AppConfig, Model, Provider};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    OfficialApi,
    ThirdPartyApi,
    CodingPlan,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdentityState {
    #[default]
    Unknown,
    UserDeclared,
}

/// Input labels are declarations, never verified account or plan entitlement.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceDraft {
    pub kind: SourceKind,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub plan_label: Option<String>,
}

/// Non-secret persisted projection. Each connection owns exactly one generation credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub connection_instance_id: String,
    pub generation: u64,
    pub kind: SourceKind,
    /// Explicit protocol base, including any version/plan path. No /v1 guessing.
    pub endpoint: String,
    pub api_type: String,
    pub credential_reference: String,
    /// Deleted public IDs stay reserved and cannot refer to a different account.
    #[serde(default)]
    pub retired: bool,
    #[serde(default)]
    pub model_bindings: std::collections::HashMap<String, String>,
    pub account_label: Option<String>,
    pub plan_label: Option<String>,
    #[serde(default)]
    pub account_state: IdentityState,
    #[serde(default)]
    pub plan_state: IdentityState,
}

impl Connection {
    pub fn draft(&self) -> SourceDraft {
        SourceDraft {
            kind: self.kind.clone(),
            account_label: self.account_label.clone(),
            plan_label: self.plan_label.clone(),
        }
    }
}

/// Validate before any credential or subscription side effect. API identity is immutable:
/// changing endpoint, account, plan, protocol or credential requires a new connection.
pub fn validate_edit(
    config: &AppConfig,
    provider: &Provider,
    original_id: Option<&str>,
    draft: Option<&SourceDraft>,
    has_new_key: bool,
) -> Result<()> {
    let old_id = original_id.unwrap_or(&provider.id);
    if let Some(saved) = config.api_sources.get(old_id) {
        ensure!(
            !saved.retired,
            "Create a new source connection with a new ID; this deleted source ID remains reserved"
        );
        ensure!(provider.id == old_id && !crate::subscription::is_subscription_provider(provider)
            && saved.endpoint == provider.base_url.trim_end_matches('/')
            && saved.api_type == provider.api_type
            && config.providers.iter().any(|p| p.id == old_id && p.kind == provider.kind)
            && draft.is_none_or(|d| saved.draft() == *d)
            && !has_new_key, "Create a new source connection to change endpoint, account, plan, protocol or credential");
    } else if let Some(draft) = draft {
        ensure!(!config.providers.iter().any(|p| p.id == old_id), "Create a new connection to classify an existing API source; legacy references are preserved");
        ensure!(
            !crate::subscription::is_subscription_provider(provider)
                && provider.kind != crate::config::ProviderKind::Ollama,
            "Explicit API sources require a generation key, not subscription authorization"
        );
        validate_endpoint(&provider.base_url)?;
        crate::protocol::Protocol::parse(&provider.api_type)?;
        ensure!(
            draft.account_label.as_ref().is_none_or(|s| s.len() <= 160)
                && draft.plan_label.as_ref().is_none_or(|s| s.len() <= 160),
            "Source labels must be at most 160 characters"
        );
    }
    Ok(())
}

pub fn apply_edit(
    config: &mut AppConfig,
    provider: &Provider,
    draft: Option<&SourceDraft>,
    instance_id: &str,
) {
    if let Some(draft) = draft.filter(|_| !config.api_sources.contains_key(&provider.id)) {
        let account_label = label(&draft.account_label);
        let plan_label = label(&draft.plan_label);
        config.api_sources.insert(
            provider.id.clone(),
            Connection {
                connection_instance_id: instance_id.into(),
                generation: 1,
                kind: draft.kind.clone(),
                endpoint: provider.base_url.trim_end_matches('/').into(),
                api_type: provider.api_type.clone(),
                credential_reference: format!("api-generation:{instance_id}"),
                retired: false,
                model_bindings: Default::default(),
                account_state: if account_label.is_some() {
                    IdentityState::UserDeclared
                } else {
                    IdentityState::Unknown
                },
                plan_state: if plan_label.is_some() {
                    IdentityState::UserDeclared
                } else {
                    IdentityState::Unknown
                },
                account_label,
                plan_label,
            },
        );
    }
    let models: Vec<_> = config
        .models
        .iter()
        .filter(|model| model.provider_id == provider.id)
        .cloned()
        .collect();
    for model in &models {
        remember_model(config, model);
    }
}

pub fn remember_model(config: &mut AppConfig, model: &Model) {
    if let Some(source) = config.api_sources.get_mut(&model.provider_id) {
        source
            .model_bindings
            .entry(model.id.clone())
            .or_insert_with(|| model.model_id.clone());
    }
}

fn label(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

pub fn validate_endpoint(endpoint: &str) -> Result<()> {
    let url =
        reqwest::Url::parse(endpoint).map_err(|_| anyhow::anyhow!("Invalid source endpoint"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "Source endpoint must use HTTPS or loopback HTTP"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Source endpoint must not contain credentials, query or fragment"
    );
    Ok(())
}

pub fn endpoint_url(
    config: &AppConfig,
    provider: &Provider,
    protocol: crate::protocol::Protocol,
) -> String {
    if let Some(source) = config.api_sources.get(&provider.id) {
        format!(
            "{}{}",
            source.endpoint.trim_end_matches('/'),
            &protocol.path()[3..]
        )
    } else {
        crate::proxy::endpoint_url(&provider.base_url, protocol.path())
    }
}

/// Stable saved targets cannot silently change source/account/plan or upstream model.
pub fn validate_model_identity(config: &AppConfig, model: &Model) -> Result<()> {
    for (provider_id, source) in &config.api_sources {
        if let Some(original) = source.model_bindings.get(&model.id) {
            ensure!(
                !source.retired && provider_id == &model.provider_id && original == &model.model_id,
                "Create a new model to change an explicit source target"
            );
        }
    }
    if let Some(previous) = config.models.iter().find(|m| m.id == model.id) {
        if config.api_sources.contains_key(&previous.provider_id)
            || config.api_sources.contains_key(&model.provider_id)
        {
            ensure!(
                previous.provider_id == model.provider_id && previous.model_id == model.model_id,
                "Create a new model to change an explicit source target"
            );
        }
    }
    Ok(())
}

/// Shared admission for the gateway, saved tests and manual probes.
pub fn admit_generation_target(
    store: &crate::config::ConfigStore,
    config: &AppConfig,
    provider: &Provider,
    model: &Model,
    protocol: crate::protocol::Protocol,
) -> Result<Option<String>> {
    ensure!(
        provider.enabled && model.enabled,
        "source_disabled: This fixed source target is disabled"
    );
    ensure!(
        config.models.iter().any(|m| m.id == model.id
            && m.provider_id == provider.id
            && m.model_id == model.model_id),
        "source_model_invalid: This fixed model is no longer available"
    );
    if let Some(source) = config.api_sources.get(&provider.id) {
        ensure!(
            !source.retired
                && !source.connection_instance_id.is_empty()
                && source.generation > 0
                && source.endpoint == provider.base_url.trim_end_matches('/')
                && source.api_type == provider.api_type
                && source.credential_reference
                    == format!("api-generation:{}", source.connection_instance_id)
                && source.model_bindings.get(&model.id) == Some(&model.model_id),
            "source_target_changed: Stored source identity no longer matches this fixed target"
        );
        validate_endpoint(&source.endpoint)?;
        ensure!(crate::protocol::Protocol::parse(&source.api_type)? == protocol,
            "source_protocol_unsupported: This endpoint was configured for a different API protocol");
    }
    if provider.kind == crate::config::ProviderKind::Ollama
        || crate::subscription::is_subscription_provider(provider)
    {
        return Ok(None);
    }
    let account = config
        .api_sources
        .get(&provider.id)
        .map(|source| source.credential_reference.clone())
        .unwrap_or_else(|| format!("provider:{}", provider.id));
    let key = store
        .read_secret(&account)
        .filter(|key| !key.trim().is_empty());
    ensure!(
        key.is_some(),
        "source_credential_missing: Add a generation credential to this source connection"
    );
    Ok(key)
}

/// Redact this request's credential before protocol parsing, logs or downstream output.
enum RedactionBuffer {
    Json(Vec<u8>),
    Sse(crate::protocol::SseParser),
}

fn redact_value(value: &mut serde_json::Value, key: &str) {
    match value {
        serde_json::Value::String(text) => *text = text.replace(key, "[REDACTED]"),
        serde_json::Value::Array(items) => {
            for item in items {
                redact_value(item, key);
            }
        }
        serde_json::Value::Object(fields) => {
            let previous = std::mem::take(fields);
            for (name, mut value) in previous {
                redact_value(&mut value, key);
                fields.insert(name.replace(key, "[REDACTED]"), value);
            }
        }
        _ => {}
    }
}

fn redact_json(bytes: &[u8], key: &str) -> Vec<u8> {
    if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        redact_value(&mut value, key);
        serde_json::to_vec(&value).expect("JSON value serializes")
    } else {
        String::from_utf8_lossy(bytes)
            .replace(key, "[REDACTED]")
            .into_bytes()
    }
}

fn redact_sse(frame: &[u8], key: &str) -> std::io::Result<Vec<u8>> {
    let text = std::str::from_utf8(frame)
        .map_err(|_| std::io::Error::other("Invalid upstream event stream"))?;
    let data = text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(|value| value.strip_prefix(' ').unwrap_or(value))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let safe =
        String::from_utf8(redact_json(data.as_bytes(), key)).expect("redacted JSON is UTF-8");
    if safe == data && !text.contains(key) {
        return Ok(frame.to_vec());
    }
    let mut output = String::new();
    let mut wrote_data = false;
    for line in text.lines().filter(|line| !line.is_empty()) {
        if line.starts_with("data:") {
            if wrote_data {
                continue;
            }
            output.push_str("data: ");
            output.push_str(&safe);
            wrote_data = true;
        } else {
            output.push_str(&line.replace(key, "[REDACTED]"));
        }
        output.push('\n');
    }
    output.push('\n');
    Ok(output.into_bytes())
}

/// Redact this request's credential before protocol parsing, logs or downstream output.
/// Decode JSON before redaction so escaped credentials cannot reappear after parsing.
pub fn redacted_stream<S, E>(
    input: S,
    secret: Option<String>,
    is_sse: bool,
) -> impl futures_util::Stream<Item = std::result::Result<axum::body::Bytes, std::io::Error>> + Send
where
    S: futures_util::Stream<Item = std::result::Result<axum::body::Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    use futures_util::StreamExt;
    let key = secret.unwrap_or_default();
    let buffer = if is_sse {
        RedactionBuffer::Sse(Default::default())
    } else {
        RedactionBuffer::Json(Vec::new())
    };
    futures_util::stream::unfold(
        (Box::pin(input), key, buffer, false),
        |(mut stream, key, mut buffer, mut ended)| async move {
            loop {
                if ended {
                    return None;
                }
                let next = stream.next().await;
                if key.is_empty() {
                    return next.map(|chunk| {
                        (
                            chunk.map_err(|_| {
                                std::io::Error::other("Upstream connection interrupted")
                            }),
                            (stream, key, buffer, false),
                        )
                    });
                }
                let output: std::io::Result<Option<Vec<u8>>> = match next {
                    Some(Err(_)) => Err(std::io::Error::other("Upstream connection interrupted")),
                    Some(Ok(chunk)) => match &mut buffer {
                        RedactionBuffer::Json(bytes) => {
                            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                                Err(std::io::Error::other("Upstream JSON is too large"))
                            } else {
                                bytes.extend_from_slice(&chunk);
                                Ok(Some(Vec::new()))
                            }
                        }
                        RedactionBuffer::Sse(parser) => parser
                            .push_raw(&chunk)
                            .map_err(|_| std::io::Error::other("Invalid upstream event stream"))
                            .and_then(|frames| {
                                frames
                                    .into_iter()
                                    .map(|frame| redact_sse(&frame, &key))
                                    .collect::<std::io::Result<Vec<_>>>()
                                    .map(|frames| Some(frames.concat()))
                            }),
                    },
                    None => {
                        ended = true;
                        match &buffer {
                            RedactionBuffer::Json(bytes) => Ok(Some(redact_json(bytes, &key))),
                            RedactionBuffer::Sse(parser) if parser.clean_eof() => Ok(None),
                            _ => Err(std::io::Error::other("Incomplete upstream event stream")),
                        }
                    }
                };
                match output {
                    Err(error) => return Some((Err(error), (stream, key, buffer, true))),
                    Ok(Some(bytes)) => {
                        return Some((
                            Ok(axum::body::Bytes::from(bytes)),
                            (stream, key, buffer, ended),
                        ))
                    }
                    _ => {}
                }
            }
        },
    )
}
