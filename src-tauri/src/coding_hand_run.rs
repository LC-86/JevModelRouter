//! Coding Plan Key evidence is independent of CPA OAuth and binds the exact plan endpoint.
use crate::{
    api_sources::SourceKind,
    config::{AppConfig, ConfigStore, Model, Provider},
    hand_run_policy::{Ledger, Policy},
    protocol::Protocol,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub provider_id: String,
    pub connection_instance_id: String,
    pub generation: u64,
    pub endpoint: String,
    pub credential_reference: String,
    pub model_id: String,
    pub model_uuid: String,
    pub account: String,
    pub plan: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub plan_id: String,
    pub binding: Binding,
    pub jev_artifact_sha256: String,
    #[serde(flatten)]
    pub policy: Policy,
}
#[derive(Clone, Debug)]
pub struct Permit {
    pub proof: Proof,
    pub enabled: bool,
    pub used: u32,
    pub expires_at: i64,
}
#[derive(Serialize)]
pub struct View {
    provider_id: String,
    name: String,
    endpoint: String,
    connection_instance_id: String,
    generation: u64,
    credential_reference: String,
    account: Option<String>,
    plan: Option<String>,
    evidence_file: String,
    artifact_sha256: String,
    models: Vec<serde_json::Value>,
    hand_run: Option<serde_json::Value>,
}
pub fn views(store: &ConfigStore, config: &AppConfig) -> Vec<View> {
    config.providers.iter().filter_map(|p| {
        let s=config.api_sources.get(&p.id)?;
        if s.kind!=SourceKind::CodingPlan || s.retired {return None;}
        let hand_run=s.hand_run.as_ref().map(|r| {
            let stopped=s.hand_run_ledger.get(&r.proof.plan_id).is_none_or(|l|l.stopped);
            serde_json::json!({"enabled":r.enabled && !stopped && config.models.iter().find(|m|m.id==r.proof.binding.model_uuid).is_some_and(|m|admit(config,p,m,Protocol::Chat).is_ok()),"stopped":stopped,"used":r.used,"max_requests":r.proof.policy.max_requests,"expires_at":r.expires_at,"model_id":r.proof.binding.model_uuid})
        });
        Some(View{provider_id:p.id.clone(),name:p.name.clone(),endpoint:s.endpoint.clone(),connection_instance_id:s.connection_instance_id.clone(),generation:s.generation,credential_reference:s.credential_reference.clone(),account:s.account_label.clone(),plan:s.plan_label.clone(),evidence_file:store.hand_run_root().join(&s.connection_instance_id).join("coding-plan.reviewed.json").display().to_string(),artifact_sha256:crate::runtime::artifact_sha().unwrap_or_default(),models:config.models.iter().filter(|m|m.provider_id==p.id).map(|m|serde_json::json!({"id":m.id,"model_id":m.model_id,"selected":m.selected})).collect(),hand_run})
    }).collect()
}
pub fn import_reviewed(store: &ConfigStore, id: &str) -> Result<()> {
    let c = store.read();
    let s = c.api_sources.get(id).context("Unknown plan source")?;
    ensure!(
        s.kind == SourceKind::CodingPlan && !s.retired,
        "Not an active plan source"
    );
    uuid::Uuid::parse_str(&s.connection_instance_id).context("Invalid owned source instance")?;
    let root = store.hand_run_root();
    let instance = root.join(&s.connection_instance_id);
    for dir in [&root, &instance] {
        let m = std::fs::symlink_metadata(dir)
            .context("Create the displayed owned evidence directory")?;
        ensure!(
            m.is_dir() && !m.file_type().is_symlink(),
            "Owned evidence directories must be regular directories"
        );
    }
    let path = instance.join("coding-plan.reviewed.json");
    let m = std::fs::symlink_metadata(&path)
        .with_context(|| format!("Put independently reviewed evidence at {}", path.display()))?;
    ensure!(
        m.is_file() && m.len() <= 65536,
        "Use a regular owned evidence file up to 64 KiB"
    );
    let proof: Proof = serde_json::from_slice(&std::fs::read(path)?)?;
    enable(store, id, proof)
}
#[tauri::command]
pub async fn set_coding_plan_hand_run(
    state: tauri::State<'_, crate::AppState>,
    provider_id: String,
    enabled: bool,
) -> std::result::Result<crate::DashboardSnapshot, String> {
    if enabled {
        import_reviewed(&state.store, &provider_id)
    } else {
        disable(&state.store, &provider_id)
    }
    .map_err(|e| e.to_string())?;
    Ok(crate::snapshot(&state).await)
}
impl Proof {
    fn validate(
        &self,
        config: &AppConfig,
        provider: &Provider,
        model: &Model,
        protocol: Protocol,
    ) -> Result<()> {
        let source = config
            .api_sources
            .get(&provider.id)
            .context("Unknown Coding Plan source")?;
        let b = &self.binding;
        ensure!(
            source.kind == SourceKind::CodingPlan && !source.retired,
            "Not an active Coding Plan source"
        );
        ensure!(
            provider.enabled
                && model.enabled
                && model.provider_id == provider.id
                && b.provider_id == provider.id
                && b.connection_instance_id == source.connection_instance_id
                && b.generation == source.generation
                && b.endpoint == source.endpoint
                && b.endpoint == provider.base_url.trim_end_matches('/')
                && b.credential_reference == source.credential_reference
                && b.model_id == model.model_id
                && b.model_uuid == model.id
                && source.model_bindings.get(&model.id) == Some(&model.model_id),
            "Coding Plan fixed source/endpoint/key/model binding changed"
        );
        ensure!(
            !b.account.trim().is_empty()
                && b.account.len() <= 200
                && !b.plan.trim().is_empty()
                && b.plan.len() <= 200
                && source
                    .account_label
                    .as_ref()
                    .is_none_or(|s| s == &b.account)
                && source.plan_label.as_ref().is_none_or(|s| s == &b.plan),
            "Coding Plan reviewed account/plan differs from the source declaration"
        );
        ensure!(
            uuid::Uuid::parse_str(&self.plan_id).is_ok()
                && self.jev_artifact_sha256 == crate::runtime::artifact_sha()?,
            "Coding Plan plan/artifact binding is invalid"
        );
        ensure!(
            protocol == Protocol::Chat
                && Protocol::parse(&source.api_type)? == protocol
                && Protocol::upstream(model, provider)? == protocol,
            "Coding Plan protocol is unverified"
        );
        let url = reqwest::Url::parse(&source.endpoint)?;
        let host = url.host_str().context("Plan endpoint host missing")?;
        ensure!(
            self.policy.max_auxiliary_requests == 0,
            "Coding Plan has no auxiliary request path"
        );
        self.policy.validate(provider, &[host])
    }
}
pub fn admit(
    config: &AppConfig,
    provider: &Provider,
    model: &Model,
    protocol: Protocol,
) -> Result<()> {
    let source = config
        .api_sources
        .get(&provider.id)
        .context("Unknown plan source")?;
    let permit=source.hand_run.as_ref().context("coding_plan_unverified: Independent plan identity, eligibility, protocol, quota and whole-call cost evidence are required")?;
    ensure!(
        permit.enabled
            && permit.expires_at > chrono::Utc::now().timestamp()
            && permit.used < permit.proof.policy.max_requests,
        "coding_plan_unverified: Plan permission is disabled, expired or exhausted"
    );
    let ledger = source
        .hand_run_ledger
        .get(&permit.proof.plan_id)
        .context("coding_plan_unverified: Plan ledger missing")?;
    ensure!(
        !ledger.stopped && ledger.used < ledger.max_requests,
        "coding_plan_unverified: Plan is stopped or exhausted"
    );
    permit.proof.validate(config, provider, model, protocol)
}
pub fn enable(store: &ConfigStore, id: &str, proof: Proof) -> Result<()> {
    store.update(|config| -> Result<()> {
        let provider = config
            .providers
            .iter()
            .find(|p| p.id == id)
            .context("Unknown plan provider")?;
        let model = config
            .models
            .iter()
            .find(|m| m.id == proof.binding.model_uuid && m.provider_id == id)
            .context("Unknown fixed plan model")?;
        ensure!(model.selected, "Select this plan model before enabling");
        proof.validate(config, provider, model, Protocol::Chat)?;
        let source = config.api_sources.get_mut(id).unwrap();
        ensure!(
            source.hand_run_ledger.len() < 100
                || source.hand_run_ledger.contains_key(&proof.plan_id),
            "Too many finite plans"
        );
        let ledger = source
            .hand_run_ledger
            .entry(proof.plan_id.clone())
            .or_insert(Ledger {
                proof_sha256: crate::hand_run_policy::proof_sha256(&proof)?,
                used: 0,
                max_requests: proof.policy.max_requests,
                stopped: false,
                auxiliary_used: 0,
                tool_rounds_used: 0,
            });
        ensure!(
            !ledger.stopped
                && ledger.max_requests == proof.policy.max_requests
                && ledger.proof_sha256 == crate::hand_run_policy::proof_sha256(&proof)?,
            "A failed plan cannot resume or expand"
        );
        let expires_at = proof
            .policy
            .evidence
            .iter()
            .map(|e| e.valid_until)
            .min()
            .unwrap();
        source.hand_run = Some(Permit {
            proof,
            enabled: true,
            used: ledger.used,
            expires_at,
        });
        Ok(())
    })??;
    Ok(())
}
pub fn disable(store: &ConfigStore, id: &str) -> Result<()> {
    store.update(|c| {
        if let Some(p) = c.api_sources.get_mut(id).and_then(|s| s.hand_run.as_mut()) {
            p.enabled = false;
        }
    })?;
    Ok(())
}
pub fn fail_plan(store: &ConfigStore, id: &str, plan: &str) -> Result<()> {
    store.update(|c| {
        if let Some(s) = c.api_sources.get_mut(id) {
            if let Some(l) = s.hand_run_ledger.get_mut(plan) {
                l.stopped = true;
            }
            if let Some(p) = s.hand_run.as_mut().filter(|p| p.proof.plan_id == plan) {
                p.enabled = false;
            }
        }
    })?;
    Ok(())
}
pub fn fail_active(store: &ConfigStore, id: &str) -> Result<()> {
    let plan = store
        .read()
        .api_sources
        .get(id)
        .and_then(|s| s.hand_run.as_ref())
        .map(|p| p.proof.plan_id.clone());
    if let Some(plan) = plan {
        fail_plan(store, id, &plan)?;
    }
    Ok(())
}
pub fn reserve(
    store: &Arc<ConfigStore>,
    provider: &Provider,
    model: &Model,
    protocol: Protocol,
    body: &serde_json::Value,
) -> Result<crate::cpa_sources::response::ResponseLease> {
    let plan = store.update(|config| -> Result<String> {
        admit(config, provider, model, protocol)?;
        let source = config.api_sources.get_mut(&provider.id).unwrap();
        let permit = source.hand_run.as_mut().unwrap();
        permit.proof.policy.validate_request(protocol, body)?;
        let ledger = source
            .hand_run_ledger
            .get_mut(&permit.proof.plan_id)
            .unwrap();
        let tool = body["messages"]
            .as_array()
            .is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
        ensure!(
            !tool || ledger.tool_rounds_used < permit.proof.policy.max_tool_rounds,
            "Coding Plan tool round limit reached"
        );
        ledger.used += 1;
        if tool {
            ledger.tool_rounds_used += 1;
        }
        permit.used = ledger.used;
        Ok(permit.proof.plan_id.clone())
    })?;
    match plan {
        Ok(plan) => Ok(crate::cpa_sources::response::ResponseLease::coding(
            store.clone(),
            provider.id.clone(),
            plan,
        )),
        Err(e) => {
            if let Some(p) = store
                .read()
                .api_sources
                .get(&provider.id)
                .and_then(|s| s.hand_run.as_ref())
            {
                fail_plan(store, &provider.id, &p.proof.plan_id)?;
            }
            Err(e)
        }
    }
}
