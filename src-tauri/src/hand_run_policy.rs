//! Source-independent review/time/quota and finite request policy; source binding stays in each adapter.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub evidence: Vec<ReviewedEvidence>,
    pub quota: crate::subscription::QuotaEvidence,
    pub subscription_only: bool,
    pub streaming: bool,
    pub function_tools: bool,
    pub max_requests: u32,
    pub max_input_bytes: usize,
    pub max_output_tokens: u32,
    pub max_auxiliary_requests: u32,
    pub max_tool_rounds: u32,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Identity,
    Plan,
    Eligibility,
    Protocol,
    Quota,
    WholeCallCost,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewedEvidence {
    pub kind: EvidenceKind,
    pub state: crate::subscription::EvidenceState,
    pub authority: String,
    pub observed_at: i64,
    pub valid_until: i64,
    pub receipt_sha256: String,
    pub reviewed: bool,
}
impl Policy {
    pub fn validate(
        &self,
        provider: &crate::config::Provider,
        authority_hosts: &[&str],
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        ensure!(
            self.evidence.len() == 6,
            "Six independently reviewed evidence records are required"
        );
        for kind in [
            EvidenceKind::Identity,
            EvidenceKind::Plan,
            EvidenceKind::Eligibility,
            EvidenceKind::Protocol,
            EvidenceKind::Quota,
            EvidenceKind::WholeCallCost,
        ] {
            let entries: Vec<_> = self.evidence.iter().filter(|e| e.kind == kind).collect();
            ensure!(
                entries.len() == 1,
                "Evidence kinds must be independent and unique"
            );
            let e = entries[0];
            let url = reqwest::Url::parse(&e.authority)?;
            let provider_host = authority_hosts.contains(&url.host_str().unwrap_or_default());
            ensure!(
                e.state == crate::subscription::EvidenceState::Available
                    && e.reviewed
                    && provider_host
                    && url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none(),
                "Evidence is unreviewed, unknown, or outside the provider's authority"
            );
            ensure!(
                e.observed_at <= now
                    && e.observed_at >= now - 900
                    && e.valid_until > now
                    && e.valid_until <= e.observed_at + 900,
                "Evidence is stale or outside its finite validity window"
            );
            ensure!(
                e.receipt_sha256.len() == 64
                    && e.receipt_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "A reviewed redacted receipt checksum is required"
            );
        }
        ensure!(
            self.subscription_only,
            "Whole-call subscription-only consumption is not proven"
        );
        if let Some(denial) = crate::subscription::admission_denial(provider, &self.quota) {
            anyhow::bail!("{}: {}", denial.code, denial.message);
        }
        ensure!(
            self.quota
                .observed_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .is_some_and(|t| t.timestamp() <= now && t.timestamp() >= now - 900)
                && self.quota.source.as_deref().is_some_and(|s| self
                    .evidence
                    .iter()
                    .any(|e| e.kind == EvidenceKind::Quota && e.authority == s)),
            "Quota authority/time must match the reviewed receipt"
        );
        ensure!(
            self.quota.buckets.iter().all(|b| !b.windows.is_empty()
                && b.windows.iter().all(|w| w
                    .used_percent
                    .is_some_and(|p| p.is_finite() && (0.0..100.0).contains(&p))
                    && w.resets_at.is_some_and(|t| t > now))),
            "Quota window is exhausted, expired or unknown"
        );
        ensure!(
            (1..=7).contains(&self.max_requests)
                && (1..=2048).contains(&self.max_input_bytes)
                && (1..=64).contains(&self.max_output_tokens),
            "HAND_RUN exceeds the finite request/input/output bounds"
        );
        ensure!(
            self.max_auxiliary_requests <= 7
                && self.max_tool_rounds <= 2
                && (self.function_tools || self.max_tool_rounds == 0),
            "Auxiliary request/tool round plan is invalid"
        );
        Ok(())
    }
    pub fn validate_request(
        &self,
        protocol: crate::protocol::Protocol,
        body: &Value,
    ) -> Result<()> {
        ensure!(protocol==crate::protocol::Protocol::Chat,"Finite HAND_RUN currently verifies only Chat Completions; other protocols remain disabled");
        let object = body
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected a Chat request"))?;
        ensure!(object.keys().all(|key|matches!(key.as_str(),"model"|"messages"|"stream"|"stream_options"|"tools"|"tool_choice"|"max_tokens"|"max_completion_tokens"|"temperature"|"top_p"|"parallel_tool_calls")),"Unverified request option; media, built-in tools, service tiers and API switching remain disabled");
        let max = body
            .get("max_tokens")
            .or_else(|| body.get("max_completion_tokens"))
            .and_then(Value::as_u64);
        ensure!(
            max.is_some_and(|n| n > 0 && n <= u64::from(self.max_output_tokens)),
            "An explicit finite output limit is required"
        );
        ensure!(
            !(body.get("max_tokens").is_some() && body.get("max_completion_tokens").is_some()),
            "Use one output limit"
        );
        let mut input = body.clone();
        input.as_object_mut().unwrap().remove("model");
        ensure!(
            serde_json::to_string(&input)?.len() <= self.max_input_bytes,
            "HAND_RUN input limit exceeded"
        );
        ensure!(
            body["stream"] != true || self.streaming,
            "SSE capability has not been verified"
        );
        if let Some(tools) = body.get("tools") {
            let tools = tools
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Invalid function tools"))?;
            ensure!(
                tools.is_empty() || self.function_tools,
                "Client function tools have not been verified"
            );
            ensure!(
                tools.iter().all(|t| t["type"] == "function"),
                "Built-in tools remain disabled"
            );
        }
        let messages = body["messages"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Chat messages are required"))?;
        ensure!(
            self.function_tools
                || !messages
                    .iter()
                    .any(|m| m["role"] == "tool" || m.get("tool_calls").is_some()),
            "Tool results and assistant tool calls require verified function capability"
        );
        ensure!(
            !messages.is_empty()
                && messages.iter().all(|m| m["content"].is_string()
                    || (m["content"].is_null() && m["tool_calls"].is_array())),
            "Only text and client function tool messages are verified"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub proof_sha256: String,
    pub used: u32,
    pub max_requests: u32,
    pub stopped: bool,
    #[serde(default)]
    pub auxiliary_used: u32,
    #[serde(default)]
    pub tool_rounds_used: u32,
}
pub fn proof_sha256(proof: &impl Serialize) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(proof)?)))
}
