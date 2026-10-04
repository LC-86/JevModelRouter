//! CPA-specific binding plus a source-independent reviewed finite policy.
use super::{Binding, Connection};
pub use crate::hand_run_policy::{EvidenceKind, Ledger, Policy, ReviewedEvidence};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandRunProof {
    pub plan_id: String,
    pub binding: Binding,
    pub artifact_sha256: String,
    #[serde(default)]
    pub jev_artifact_sha256: String,
    pub credential_ref: String,
    #[serde(flatten)]
    pub policy: Policy,
}
#[derive(Clone, Debug)]
pub struct Permit {
    pub proof: HandRunProof,
    pub enabled: bool,
    pub used: u32,
    pub expires_at: i64,
    pub stopped: bool,
}
impl HandRunProof {
    pub fn validate(&self, id: &str, c: &Connection) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.plan_id).is_ok(),
            "A finite HAND_RUN plan UUID is required"
        );
        ensure!(
            self.binding.matches(id, c, &self.binding.model_id),
            "HAND_RUN identity/model binding changed"
        );
        ensure!(
            c.identity.identity.is_some() && c.plan.is_some(),
            "HAND_RUN account and plan remain unknown"
        );
        ensure!(
            self.artifact_sha256 == super::service::artifact_sha()
                && self.jev_artifact_sha256 == crate::runtime::artifact_sha()?
                && c.credential_ref.as_deref() == Some(self.credential_ref.as_str()),
            "HAND_RUN artifact or credential binding changed"
        );
        ensure!(
            self.policy.max_auxiliary_requests >= self.policy.max_requests,
            "CPA credential checks need one auxiliary request per dispatch"
        );
        let provider = crate::config::Provider {
            id: id.into(),
            name: c.provider.clone(),
            kind: if c.provider == "codex" {
                crate::config::ProviderKind::CodexSubscription
            } else {
                crate::config::ProviderKind::GrokSubscription
            },
            base_url: String::new(),
            enabled: true,
            preset: String::new(),
            api_type: String::new(),
            test_model: String::new(),
            has_api_key: false,
        };
        let hosts: &[&str] = if c.provider == "codex" {
            &["chatgpt.com", "help.openai.com", "openai.com"]
        } else {
            &["grok.com", "accounts.x.ai", "docs.x.ai", "x.ai"]
        };
        self.policy.validate(&provider, hosts)
    }
}
impl Permit {
    pub fn ready(&self, id: &str, c: &Connection, model: &str) -> bool {
        !self.stopped
            && self.enabled
            && self.expires_at > chrono::Utc::now().timestamp()
            && self.used < self.proof.policy.max_requests
            && self.proof.binding.matches(id, c, model)
            && self.proof.validate(id, c).is_ok()
    }
    pub fn validate_request(
        &self,
        protocol: crate::protocol::Protocol,
        body: &serde_json::Value,
    ) -> Result<()> {
        self.proof.policy.validate_request(protocol, body)
    }
}
