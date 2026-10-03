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

pub fn apply_edit(config: &mut AppConfig, provider: &Provider, draft: Option<&SourceDraft>) {
    if config.api_sources.contains_key(&provider.id) {
        return;
    }
    if let Some(draft) = draft {
        let account_label = label(&draft.account_label);
        let plan_label = label(&draft.plan_label);
        config.api_sources.insert(
            provider.id.clone(),
            Connection {
                connection_instance_id: uuid::Uuid::new_v4().to_string(),
                generation: 1,
                kind: draft.kind.clone(),
                endpoint: provider.base_url.trim_end_matches('/').into(),
                api_type: provider.api_type.clone(),
                credential_reference: format!("provider:{}", provider.id),
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
pub fn generation_key(
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
            !source.connection_instance_id.is_empty()
                && source.generation > 0
                && source.endpoint == provider.base_url.trim_end_matches('/')
                && source.api_type == provider.api_type
                && source.credential_reference == format!("provider:{}", provider.id),
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
    let key = store
        .read_secret(&format!("provider:{}", provider.id))
        .filter(|key| !key.trim().is_empty());
    ensure!(
        key.is_some(),
        "source_credential_missing: Add a generation credential to this source connection"
    );
    Ok(key)
}

/// Redact this request's credential before protocol parsing, logs or downstream output.
/// Keep a short tail so a secret split across HTTP chunks cannot escape.
pub fn redacted_stream<S, E>(
    input: S,
    secret: Option<String>,
) -> impl futures_util::Stream<Item = std::result::Result<axum::body::Bytes, std::io::Error>> + Send
where
    S: futures_util::Stream<Item = std::result::Result<axum::body::Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    use futures_util::StreamExt;
    let needle = secret.unwrap_or_default().into_bytes();
    futures_util::stream::unfold(
        (Box::pin(input), needle, Vec::new(), false),
        |(mut stream, needle, mut buffer, mut ended)| async move {
            loop {
                if ended {
                    if buffer.is_empty() {
                        return None;
                    }
                    return Some((
                        Ok(axum::body::Bytes::from(std::mem::take(&mut buffer))),
                        (stream, needle, buffer, true),
                    ));
                }
                match stream.next().await {
                    Some(Ok(chunk)) if needle.is_empty() => {
                        return Some((Ok(chunk), (stream, needle, buffer, false)))
                    }
                    Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                    Some(Err(_)) => {
                        return Some((
                            Err(std::io::Error::other("Upstream connection interrupted")),
                            (stream, needle, Vec::new(), true),
                        ))
                    }
                    None => ended = true,
                }
                let limit = if ended {
                    buffer.len()
                } else {
                    buffer.len().saturating_sub(needle.len().saturating_sub(1))
                };
                let mut output = Vec::new();
                let mut index = 0;
                while index < limit {
                    if !needle.is_empty() && buffer[index..].starts_with(&needle) {
                        output.extend_from_slice(b"[REDACTED]");
                        index += needle.len();
                    } else {
                        output.push(buffer[index]);
                        index += 1;
                    }
                }
                buffer.drain(..index);
                if !output.is_empty() {
                    return Some((
                        Ok(axum::body::Bytes::from(output)),
                        (stream, needle, buffer, ended),
                    ));
                }
            }
        },
    )
}
