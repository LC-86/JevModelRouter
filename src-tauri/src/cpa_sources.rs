//! CPA owns credentials/OAuth/refresh. Jev stores references and identity generations.
//! R3 has no real generation grant; registration and discovery cannot create one.
use crate::{
    config::{AppConfig, ConfigStore, Model, Provider, ProviderKind},
    subscription::{Connection as Identity, ConnectionState, Denial, DenialFamily, EvidenceState},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

mod client;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    #[default]
    Idle,
    Starting,
    Waiting,
    Connected,
    Failed,
    Cancelled,
    Disconnected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Discovered {
    pub model_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub identity: Identity,
    pub provider: String,
    pub stage: Stage,
    pub plan: Option<String>,
    pub credential_ref: Option<String>,
    pub catalog: Vec<Discovered>,
    #[serde(default)]
    pub model_ids: HashMap<String, String>,
    pub catalog_state: EvidenceState,
    pub observed_at: Option<String>,
    pub error: Option<String>,
}

impl Connection {
    pub fn retain_model_uuid(&mut self, model: &Model) {
        self.model_ids.entry(model.model_id.clone()).or_insert_with(|| model.id.clone());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityStamp {
    pub provider_id: String,
    pub connection_instance_id: String,
    pub generation: u64,
    pub account: Option<String>,
    pub plan: Option<String>,
}
impl IdentityStamp {
    fn matches(&self, id: &str, c: &Connection) -> bool {
        self.provider_id == id
            && self.connection_instance_id == c.identity.connection_instance_id
            && self.generation == c.identity.generation
            && self.account == c.identity.identity
            && self.plan == c.plan
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    #[serde(flatten)]
    pub identity: IdentityStamp,
    pub model_id: String,
}
impl Binding {
    fn matches(&self, id: &str, c: &Connection, model: &str) -> bool {
        self.identity.matches(id, c) && self.model_id == model
    }
}

#[derive(Clone, Serialize)]
pub struct ModelView {
    pub id: String,
    pub model_id: String,
    pub name: String,
    pub selected: bool,
    pub bound: bool,
}
#[derive(Clone, Serialize)]
pub struct View {
    pub provider_id: String,
    pub provider: String,
    pub stage: Stage,
    pub connection_instance_id: String,
    pub generation: u64,
    pub account: Option<String>,
    pub plan: Option<String>,
    pub catalog_state: EvidenceState,
    pub observed_at: Option<String>,
    pub models: Vec<ModelView>,
    pub error: Option<String>,
    pub authorization_url: Option<String>,
    pub service_available: bool,
    pub qualification: &'static str,
    pub quota: &'static str,
    pub capability: &'static str,
}

#[derive(Clone)]
struct Attempt {
    stamp: IdentityStamp,
    state: Option<String>,
    url: Option<String>,
    cancelled: Option<bool>,
    poll_waiting: bool,
}
struct CleanupLease<'a> {
    active: &'a Mutex<HashSet<String>>,
    id: String,
}
impl Drop for CleanupLease<'_> {
    fn drop(&mut self) {
        self.active.lock().unwrap().remove(&self.id);
    }
}
#[derive(Default)]
pub struct Manager {
    // One dedicated service profile per connection. Never scan/adopt a shared CPA.
    clients: Mutex<HashMap<String, Arc<client::Client>>>,
    attempts: Mutex<HashMap<String, Attempt>>,
    cleaning: Mutex<HashSet<String>>,
    polling: Mutex<HashSet<String>>,
    fixture: Mutex<Option<Arc<client::Client>>>,
}

impl Manager {
    #[cfg(any(test, feature = "isolation-check"))]
    pub fn owned_fixture(base: String) -> Result<Self> {
        Ok(Self {
            fixture: Mutex::new(Some(Arc::new(client::Client::owned_fixture(base)?))),
            ..Default::default()
        })
    }
    fn client(&self, id: &str) -> Result<Arc<client::Client>> {
        self.clients
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "CPA service is not provisioned; real authorization is pending R6/R7"
                )
            })
    }
    fn cleanup_lease(&self, id: &str) -> Result<CleanupLease<'_>> {
        ensure!(
            self.cleaning.lock().unwrap().insert(id.into()),
            "Owned CPA cleanup is already in progress"
        );
        Ok(CleanupLease {
            active: &self.cleaning,
            id: id.into(),
        })
    }
    fn retire_profile(&self, id: &str, client: &Arc<client::Client>) {
        let mut clients = self.clients.lock().unwrap();
        if clients.get(id).is_some_and(|owned| Arc::ptr_eq(owned, client)) {
            clients.remove(id);
        }
        let mut fixture = self.fixture.lock().unwrap();
        if fixture.as_ref().is_some_and(|owned| Arc::ptr_eq(owned, client)) {
            *fixture = None;
        }
    }
    pub fn cleanup_finished(&self, id: &str) -> bool {
        !self.attempts.lock().unwrap().contains_key(id)
            && !self.cleaning.lock().unwrap().contains(id)
            && !self.polling.lock().unwrap().contains(id)
    }
    pub fn remove(&self, store: &ConfigStore, id: &str) -> Result<()> {
        let attempts = self.attempts.lock().unwrap();
        let cleaning = self.cleaning.lock().unwrap();
        ensure!(
            !attempts.contains_key(id)
                && !cleaning.contains(id)
                && !self.polling.lock().unwrap().contains(id),
            "Finish owned CPA cleanup before deleting this connection"
        );
        store.update(|config| -> Result<()> {
            let c = config
                .cpa_subscriptions
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("Unknown CPA connection"))?;
            ensure!(
                c.credential_ref.is_none()
                    && !matches!(c.stage, Stage::Starting | Stage::Waiting | Stage::Connected),
                "Disconnect and finish owned CPA cleanup before deleting this connection"
            );
            config.cpa_subscriptions.remove(id);
            config
                .cpa_model_bindings
                .retain(|_, binding| binding.identity.provider_id != id);
            config.providers.retain(|provider| provider.id != id);
            config.models.retain(|model| model.provider_id != id);
            Ok(())
        })??;
        self.clients.lock().unwrap().remove(id);
        Ok(())
    }
    pub fn create(&self, store: &ConfigStore, provider: &str, name: &str) -> Result<String> {
        ensure!(
            provider == "codex",
            "This R3 integration currently supports the Codex management contract only"
        );
        ensure!(
            !name.trim().is_empty() && name.len() <= 100,
            "Enter a connection name"
        );
        let id = format!("cpa-{}", uuid::Uuid::new_v4());
        let mut clients = self.clients.lock().unwrap();
        store.update(|config| {
            config.providers.push(Provider {
                id: id.clone(),
                name: name.trim().into(),
                kind: ProviderKind::CodexSubscription,
                base_url: String::new(),
                enabled: true,
                preset: String::new(),
                api_type: String::new(),
                test_model: String::new(),
                has_api_key: false,
            });
            config.cpa_subscriptions.insert(
                id.clone(),
                Connection {
                    identity: Identity::default(),
                    provider: provider.into(),
                    stage: Stage::Idle,
                    plan: None,
                    credential_ref: None,
                    catalog: vec![],
                    model_ids: HashMap::new(),
                    catalog_state: EvidenceState::Unknown,
                    observed_at: None,
                    error: None,
                },
            );
        })?;
        if clients.is_empty() {
            if let Some(client) = self.fixture.lock().unwrap().as_ref() {
                clients.insert(id.clone(), client.clone());
            }
        }
        Ok(id)
    }

    pub fn views(&self, config: &AppConfig) -> Vec<View> {
        let attempts = self.attempts.lock().unwrap();
        let clients = self.clients.lock().unwrap();
        let mut views: Vec<_> = config
            .cpa_subscriptions
            .iter()
            .map(|(id, c)| View {
                provider_id: id.clone(),
                provider: c.provider.clone(),
                stage: c.stage,
                connection_instance_id: c.identity.connection_instance_id.clone(),
                generation: c.identity.generation,
                account: c.identity.identity.clone(),
                plan: c.plan.clone(),
                catalog_state: c.catalog_state,
                observed_at: c.observed_at.clone(),
                error: c.error.clone(),
                service_available: clients.contains_key(id),
                authorization_url: attempts
                    .get(id)
                    .filter(|a| a.stamp.matches(id, c) && c.stage == Stage::Waiting)
                    .and_then(|a| a.url.clone()),
                qualification: "unknown",
                quota: "unknown",
                capability: "unverified",
                models: config
                    .models
                    .iter()
                    .filter(|m| m.provider_id == *id)
                    .map(|m| ModelView {
                        id: m.id.clone(),
                        model_id: m.model_id.clone(),
                        name: m.name.clone(),
                        selected: m.selected,
                        bound: config
                            .cpa_model_bindings
                            .get(&m.id)
                            .is_some_and(|b| b.matches(id, c, &m.model_id)),
                    })
                    .collect(),
            })
            .collect();
        views.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));
        views
    }

    pub async fn begin(&self, store: &ConfigStore, id: &str) -> Result<()> {
        let client = self.client(id)?;
        let stamp = {
            let mut attempts = self.attempts.lock().unwrap();
            let cleaning = self.cleaning.lock().unwrap();
            ensure!(
                !attempts.contains_key(id)
                    && !cleaning.contains(id)
                    && !self.polling.lock().unwrap().contains(id),
                "Wait for the previous owned authorization/cleanup to finish"
            );
            let stamp = store.update(|config| -> Result<IdentityStamp> {
                let c = config
                    .cpa_subscriptions
                    .get_mut(id)
                    .ok_or_else(|| anyhow::anyhow!("Unknown CPA connection"))?;
                ensure!(
                    !matches!(c.stage, Stage::Starting | Stage::Waiting | Stage::Connected),
                    "Disconnect or cancel this connection first"
                );
                ensure!(
                    c.credential_ref.is_none(),
                    "Retry credential cleanup before reconnecting"
                );
                advance(c, Stage::Starting)?;
                Ok(stamp(id, c))
            })??;
            attempts.insert(
                id.into(),
                Attempt {
                    stamp: stamp.clone(),
                    state: None,
                    url: None,
                    cancelled: None,
                    poll_waiting: false,
                },
            );
            stamp
        };
        let result = async {
            ensure!(
                client.credentials().await?.is_empty(),
                "Dedicated CPA profile contains an unclaimed credential; do not adopt or delete it"
            );
            client
                .begin(&store.read().cpa_subscriptions[id].provider)
                .await
        }
        .await;
        let (state, url) = match result {
            Ok(v) => v,
            Err(e) => {
                let failed = fail(store, id, &stamp, &e.to_string());
                let mut attempts = self.attempts.lock().unwrap();
                if attempts.get(id).is_some_and(|a| a.stamp == stamp) {
                    attempts.remove(id);
                }
                failed?;
                return Err(e);
            }
        };
        let committed = {
            let mut attempts = self.attempts.lock().unwrap();
            let committed = store.update(|config| -> Result<()> {
                let c = current(config, id, &stamp)?;
                ensure!(c.stage == Stage::Starting, "Authorization was superseded");
                c.stage = Stage::Waiting;
                c.identity.state = ConnectionState::AuthorizationPending;
                Ok(())
            })?;
            if attempts.get(id).is_some_and(|a| a.stamp == stamp) {
                attempts.insert(
                    id.into(),
                    Attempt {
                        stamp: stamp.clone(),
                        state: Some(state),
                        url: Some(url),
                        cancelled: None,
                        poll_waiting: false,
                    },
                );
            }
            committed
        };
        if committed.is_err() {
            let _lease = self.cleanup_lease(id)?;
            let config = store.read();
            let c = config
                .cpa_subscriptions
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("Connection removed"))?;
            self.cleanup_owned(store, id, &self::stamp(id, c)).await?;
        }
        committed
    }

    pub async fn poll(&self, store: &ConfigStore, id: &str) -> Result<()> {
        let client = self.client(id)?;
        let (attempt, provider, _poll) = {
            let attempts = self.attempts.lock().unwrap();
            let attempt = attempts
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("No owned authorization session"))?;
            let config = store.read();
            let c = config
                .cpa_subscriptions
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("Unknown CPA connection"))?;
            ensure!(
                attempt.stamp.matches(id, c) && c.stage == Stage::Waiting,
                "Authorization was superseded"
            );
            ensure!(
                self.polling.lock().unwrap().insert(id.into()),
                "Owned authorization poll is already in progress"
            );
            (
                attempt,
                c.provider.clone(),
                CleanupLease {
                    active: &self.polling,
                    id: id.into(),
                },
            )
        };
        let mut waiting = false;
        let outcome = async {
            let result = async {
                let session=attempt.state.as_deref().ok_or_else(||anyhow::anyhow!("Authorization request is still starting"))?;
                match client.status(session).await? {
                    client::SessionStatus::Waiting => { waiting = true; return Ok(None); },
                    client::SessionStatus::Complete => {},
                    client::SessionStatus::Expired => anyhow::bail!("CPA authorization record expired; disconnect to isolate the old service profile"),
                }
                let mut files=client.credentials().await?;
                ensure!(files.len()==1 && files[0].provider==provider && !files[0].disabled, "Dedicated CPA profile must contain exactly one active credential of the expected provider");
                Ok(Some(files.remove(0)))
            }.await;
            let credential = match result {
                Ok(None) => return Ok(()),
                Ok(Some(c)) => c,
                Err(e) => {
                    fail(store, id, &attempt.stamp, &e.to_string())?;
                    return Err(e);
                }
            };
            let mut attempts = self.attempts.lock().unwrap();
            store.update(|config| -> Result<()> {
                let c = current(config, id, &attempt.stamp)?;
                ensure!(c.stage == Stage::Waiting, "Authorization was superseded");
                c.identity.state = ConnectionState::Connected;
                c.identity.identity = known(credential.id_token.chatgpt_account_id);
                c.plan = known(credential.id_token.plan_type);
                c.credential_ref = Some(credential.name);
                c.stage = Stage::Connected;
                c.error = None;
                Ok(())
            })??;
            if attempts.get(id).is_some_and(|a| a.stamp == attempt.stamp) {
                attempts.remove(id);
            }
            Ok(())
        }.await;
        // The last of poll/cancel to settle completes ownership. A Waiting result
        // is retryable only after successful cancellation and a proven empty profile.
        let config = store.read();
        if !config.cpa_subscriptions.get(id).is_some_and(|c| {
            c.identity.connection_instance_id == attempt.stamp.connection_instance_id
                && c.identity.generation == attempt.stamp.generation
        }) {
            let settled = {
                let mut attempts = self.attempts.lock().unwrap();
                let a = attempts.get_mut(id).filter(|a| a.stamp == attempt.stamp);
                if let Some(a) = a {
                    a.poll_waiting = waiting;
                    a.clone()
                } else {
                    Attempt { poll_waiting: waiting, ..attempt.clone() }
                }
            };
            if !waiting || settled.cancelled.is_some() {
                self.settle_cancelled_poll(store, id, &client, &settled).await?;
            }
        }
        outcome
    }

    async fn settle_cancelled_poll(
        &self, store: &ConfigStore, id: &str,
        client: &Arc<client::Client>, attempt: &Attempt,
    ) -> Result<()> {
        let retryable = attempt.poll_waiting && attempt.cancelled == Some(true)
            && matches!(client.credentials().await, Ok(files) if files.is_empty());
        if !retryable {
            self.retire_profile(id, client);
            store.update(|config| {
                if let Some(c) = config.cpa_subscriptions.get_mut(id) {
                    c.credential_ref = None;
                    c.error = Some("Cancelled authorization poll settled; old service profile isolated. 迟到完成或残留状态不明，旧服务配置已隔离；请删除旧连接并配置新的专用服务。未认领凭据不会删除。".into());
                }
            })?;
        }
        let mut attempts = self.attempts.lock().unwrap();
        if attempts.get(id).is_some_and(|a| a.stamp == attempt.stamp) {
            attempts.remove(id);
        }
        Ok(())
    }

    pub async fn disconnect(&self, store: &ConfigStore, id: &str, cancel: bool) -> Result<()> {
        // Commit the denial before any network wait. Older poll/refresh completions fail their stamp.
        let _lease = self.cleanup_lease(id)?;
        let cleanup_stamp = store.update(|config| -> Result<IdentityStamp> {
            let c = config
                .cpa_subscriptions
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("Unknown CPA connection"))?;
            if cancel {
                ensure!(
                    matches!(c.stage, Stage::Starting | Stage::Waiting),
                    "No pending authorization to cancel"
                );
            }
            advance(
                c,
                if cancel {
                    Stage::Cancelled
                } else {
                    Stage::Disconnected
                },
            )?;
            Ok(stamp(id, c))
        })??;
        self.cleanup_owned(store, id, &cleanup_stamp).await
    }

    async fn cleanup_owned(
        &self,
        store: &ConfigStore,
        id: &str,
        cleanup_stamp: &IdentityStamp,
    ) -> Result<()> {
        let attempt = self.attempts.lock().unwrap().get(id).cloned();
        if attempt.as_ref().is_some_and(|a| a.state.is_none()) {
            // The in-flight start remains owned and blocks profile reuse. Its result will
            // be cancelled/cleaned here when the URL arrives, without restoring identity.
            return Ok(());
        }
        let config = store.read();
        let c = config
            .cpa_subscriptions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Connection removed"))?;
        ensure!(cleanup_stamp.matches(id, c), "Cleanup identity changed");
        let credential = c.credential_ref.clone();
        let result = async {
            let mut credential = credential;
            if let Some(a) = &attempt {
                let client = self.client(id)?;
                let session = a.state.as_deref().expect("pending start handled above");
                let cancelled = client.cancel(session).await?;
                let settled = {
                    let mut attempts = self.attempts.lock().unwrap();
                    if let Some(owned) = attempts.get_mut(id).filter(|owned| owned.stamp == a.stamp) {
                        owned.cancelled = Some(cancelled);
                        owned.clone()
                    } else {
                        a.clone()
                    }
                };
                if self.polling.lock().unwrap().contains(id)
                    || !self.clients.lock().unwrap().contains_key(id) {
                    // The poll retains this attempt and finishes ownership when it settles.
                    return Ok(());
                }
                if settled.poll_waiting {
                    self.settle_cancelled_poll(store, id, &client, &settled).await?;
                    return Ok(());
                }
                if !cancelled && credential.is_none() {
                    // Fixed CPA returns false after completion. This attempt started with an
                    // empty, exclusive profile: claim a proven completion for cleanup only.
                    // An already persisted cleanup reference outlives CPA's completed-session TTL.
                    let files = client.credentials().await?;
                    if !files.is_empty() {
                        match client.status(session).await? {
                            client::SessionStatus::Expired => {
                                // No session-to-file proof remains. Detach this profile;
                                // never adopt or delete its unclaimed credentials.
                                store.update(|config| -> Result<()> {
                                    current(config, id, cleanup_stamp)?.error = Some(
                                        "Authorization record expired; old service profile isolated. 授权记录已到期，旧服务配置已隔离；请删除旧连接并配置新的专用服务。未认领凭据不会删除。".into());
                                    Ok(())
                                })??;
                                self.retire_profile(id, &client);
                                return Ok(());
                            },
                            client::SessionStatus::Complete => {},
                            client::SessionStatus::Waiting => anyhow::bail!("Owned authorization has not completed"),
                        }
                        ensure!(
                            files.len() == 1
                                && files[0].provider == store.read().cpa_subscriptions[id].provider,
                            "Completed authorization has ambiguous credential ownership"
                        );
                        let name = files[0].name.clone();
                        store.update(|config| -> Result<()> {
                            current(config, id, &cleanup_stamp)?.credential_ref =
                                Some(name.clone());
                            Ok(())
                        })??;
                        credential = Some(name);
                    }
                }
            }
            if let Some(name) = &credential {
                let client = self.client(id)?;
                // Repeating a logout after successful remote cleanup is safe, even if
                // an earlier local write failed; never delete an unrelated reference.
                if client
                    .credentials()
                    .await?
                    .iter()
                    .any(|file| &file.name == name)
                {
                    client.delete(name).await?;
                }
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            // Keep the owned cleanup reference for an explicit retry; never restore connection state.
            store.update(|config| { if let Ok(c)=current(config,id,&cleanup_stamp) { c.error=Some("Local connection disabled; CPA cleanup failed. Retry disconnect before reconnecting.".into()); } })?;
        } else {
            let mut attempts = self.attempts.lock().unwrap();
            store.update(|config| {
                if let Ok(c) = current(config, id, &cleanup_stamp) {
                    c.credential_ref = None;
                }
            })?;
            if !self.polling.lock().unwrap().contains(id)
                && attempts
                    .get(id)
                    .is_some_and(|a| attempt.as_ref().is_some_and(|old| old.stamp == a.stamp))
            {
                attempts.remove(id);
            }
        }
        result
    }

    pub async fn refresh(&self, store: &ConfigStore, id: &str) -> Result<()> {
        let client = self.client(id)?;
        let config = store.read();
        let c = config
            .cpa_subscriptions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Unknown CPA connection"))?;
        ensure!(
            c.stage == Stage::Connected,
            "Connect before reading the directory"
        );
        let b = stamp(id, c);
        let name = c
            .credential_ref
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Credential reference is unknown"))?;
        let sequence = store
            .begin_subscription_refresh(id)
            .map_err(anyhow::Error::msg)?;
        let metadata = client.credentials().await;
        // Identity commits before discovery: a directory failure cannot preserve a changed account.
        let b=store.update_subscription_refresh(id,sequence,|config| {
            let c=current(config,id,&b).map_err(|e|e.to_string())?;
            let files=match &metadata {
                Ok(files)=>files,
                Err(error)=> {mark_read_failed(c);return Err(error.to_string());}
            };
            let file=if files.len()==1 {files.iter().find(|f|f.name==name&&f.provider==c.provider&&!f.disabled)} else {None};
            let Some(file)=file else {
                advance(c,Stage::Disconnected).map_err(|e|e.to_string())?;
                c.error=Some("Owned CPA credential disappeared, was disabled, or became ambiguous; connection disabled".into());
                return Err(c.error.clone().unwrap());
            };
            let account=known(file.id_token.chatgpt_account_id.clone());
            let plan=known(file.id_token.plan_type.clone());
            if account!=c.identity.identity||plan!=c.plan {
                advance(c,Stage::Connected).map_err(|e|e.to_string())?;
                c.identity.state=ConnectionState::Connected;c.identity.identity=account;c.plan=plan;
            }
            Ok(stamp(id,c))
        }).map_err(anyhow::Error::msg)?;
        let result = client.models(&name).await;
        store
            .update_subscription_refresh(
                id,
                sequence,
                |config| -> std::result::Result<(), String> {
                    let c = current(config, id, &b).map_err(|e| e.to_string())?;
                    match &result {
                        Ok(models) => {
                            c.catalog = models.clone();
                            c.catalog_state = EvidenceState::Available;
                            c.observed_at = Some(chrono::Utc::now().to_rfc3339());
                            c.error = None;
                        }
                        Err(_) => mark_read_failed(c),
                    }
                    if let Ok(models) = &result {
                        let connection = config.cpa_subscriptions.get_mut(id)
                            .expect("current connection validated above");
                        for discovered in models {
                            if let Some(existing) = config.models.iter()
                                .find(|m| m.provider_id == id && m.model_id == discovered.model_id) {
                                connection.retain_model_uuid(existing);
                            } else {
                                let internal_id = connection.model_ids.entry(discovered.model_id.clone())
                                    .or_insert_with(|| uuid::Uuid::new_v4().to_string()).clone();
                                config.models.push(Model {
                                    id: internal_id,
                                    provider_id: id.into(),
                                    model_id: discovered.model_id.clone(),
                                    name: discovered.name.clone(),
                                    api_type: String::new(),
                                    tier: crate::config::ModelTier::Balanced,
                                    selected: false,
                                    enabled: true,
                                    supports_tools: false,
                                    supports_vision: false,
                                    supports_reasoning: false,
                                    context_window: 0,
                                    input_cost_per_million: 0.0,
                                    output_cost_per_million: 0.0,
                                    cache_cost_per_million: 0.0,
                                    input_price_known: Some(false),
                                    output_price_known: Some(false),
                                    cache_price_known: Some(false),
                                });
                            }
                        }
                    }
                    Ok(())
                },
            )
            .map_err(anyhow::Error::msg)?;
        result.map(|_| ())
    }

    /// Selecting is the user's explicit binding/rebinding action. Discovery never changes it.
    pub fn select(
        &self,
        store: &ConfigStore,
        id: &str,
        model_id: &str,
        selected: bool,
    ) -> Result<()> {
        store.update(|config| -> Result<()> {
            let c = config
                .cpa_subscriptions
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("Unknown CPA connection"))?;
            let model = config
                .models
                .iter_mut()
                .find(|m| m.id == model_id && m.provider_id == id)
                .ok_or_else(|| anyhow::anyhow!("Unknown source model"))?;
            if selected {
                ensure!(
                    c.stage == Stage::Connected
                        && c.identity.identity.is_some()
                        && c.plan.is_some()
                        && c.catalog_state == EvidenceState::Available
                        && c.catalog.iter().any(|m| m.model_id == model.model_id),
                    "Current account, plan and directory are required before binding this model"
                );
                config.cpa_model_bindings.insert(
                    model.id.clone(),
                    Binding {
                        identity: stamp(id, c),
                        model_id: model.model_id.clone(),
                    },
                );
            }
            model.selected = selected;
            Ok(())
        })??;
        Ok(())
    }

    pub async fn shutdown(&self, store: &ConfigStore) -> Result<()> {
        let ids: Vec<_> = store
            .read()
            .cpa_subscriptions
            .iter()
            .filter(|(id, c)| {
                matches!(c.stage, Stage::Starting | Stage::Waiting)
                    || self.attempts.lock().unwrap().contains_key(*id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut failures = Vec::new();
        for id in ids {
            if let Err(error) = self.disconnect(store, &id, false).await {
                failures.push(error.to_string());
            }
        }
        ensure!(
            failures.is_empty(),
            "CPA owned-session cleanup failed: {}",
            failures.join("; ")
        );
        ensure!(
            self.polling.lock().unwrap().is_empty(),
            "Owned CPA authorization poll is still finishing; retry shutdown"
        );
        ensure!(
            self.attempts
                .lock()
                .unwrap()
                .values()
                .all(|a| a.state.is_some()),
            "Owned CPA authorization request is still finishing; retry shutdown"
        );
        Ok(())
    }
}

fn mark_read_failed(c: &mut Connection) {
    c.catalog_state = if c.catalog.is_empty() {
        EvidenceState::Failed
    } else {
        EvidenceState::Stale
    };
    c.error = Some("CPA directory read failed; historical configuration retained".into());
}

pub fn denial(config: &AppConfig, provider: &Provider, model: Option<&Model>) -> Option<Denial> {
    let c = config.cpa_subscriptions.get(&provider.id)?;
    let (code, family, message) = if !provider.enabled || model.is_some_and(|m| !m.enabled) {
        (
            "provider_disabled",
            DenialFamily::Disabled,
            "CPA target is disabled",
        )
    } else if c.stage != Stage::Connected {
        (
            "cpa_not_connected",
            DenialFamily::NotConnected,
            "CPA subscription connection is not connected",
        )
    } else if model.is_none_or(|m| {
        !config
            .cpa_model_bindings
            .get(&m.id)
            .is_some_and(|b| b.matches(&provider.id, c, &m.model_id))
    }) {
        (
            "cpa_target_identity_changed",
            DenialFamily::NotEligible,
            "This fixed target is not bound to the current account, plan and connection generation",
        )
    } else {
        ("cpa_qualification_unknown",DenialFamily::Quota,"CPA login and directory do not prove model eligibility, protocol capability or whole-call subscription-only consumption")
    };
    Some(Denial {
        code: code.into(),
        family,
        message: message.into(),
        recovery:
            "Review this source in R7. This target never falls back to another account or API."
                .into(),
    })
}

pub fn reconcile_loaded(config: &mut AppConfig) -> Result<bool> {
    let mut changed = false;
    for c in config.cpa_subscriptions.values_mut() {
        if matches!(c.stage, Stage::Starting | Stage::Waiting) {
            advance(c, Stage::Disconnected)?;
            c.error = Some(
                "Pending authorization ended with the previous process; reconnect explicitly"
                    .into(),
            );
            changed = true;
        }
    }
    Ok(changed)
}

fn known(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s.len() <= 200)
}
fn stamp(id: &str, c: &Connection) -> IdentityStamp {
    IdentityStamp {
        provider_id: id.into(),
        connection_instance_id: c.identity.connection_instance_id.clone(),
        generation: c.identity.generation,
        account: c.identity.identity.clone(),
        plan: c.plan.clone(),
    }
}
fn advance(c: &mut Connection, stage: Stage) -> Result<()> {
    c.identity.generation = c
        .identity
        .generation
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("Connection generation exhausted"))?;
    c.identity.state = ConnectionState::NotConnected;
    c.identity.identity = None;
    c.identity.evidence = None;
    c.plan = None;
    c.stage = stage;
    c.error = None;
    c.catalog_state = if c.catalog.is_empty() {
        EvidenceState::Unknown
    } else {
        EvidenceState::Stale
    };
    Ok(())
}
fn current<'a>(
    config: &'a mut AppConfig,
    id: &str,
    b: &IdentityStamp,
) -> Result<&'a mut Connection> {
    let c = config
        .cpa_subscriptions
        .get_mut(id)
        .ok_or_else(|| anyhow::anyhow!("Connection removed"))?;
    ensure!(
        b.matches(id, c),
        "Connection identity changed; late result discarded"
    );
    Ok(c)
}
fn fail(store: &ConfigStore, id: &str, b: &IdentityStamp, error: &str) -> Result<()> {
    store.update(|config| -> Result<()> {
        let c = current(config, id, b)?;
        c.stage = Stage::Failed;
        c.identity.state = ConnectionState::NotConnected;
        c.error = Some(error.into());
        Ok(())
    })??;
    Ok(())
}

#[tauri::command]
pub async fn create_cpa_subscription(
    state: tauri::State<'_, crate::AppState>,
    provider: String,
    name: String,
) -> std::result::Result<crate::DashboardSnapshot, String> {
    state
        .store
        .cpa
        .create(&state.store, &provider, &name)
        .map_err(|e| e.to_string())?;
    Ok(crate::snapshot(&state).await)
}
#[tauri::command]
pub async fn cpa_subscription_action(
    state: tauri::State<'_, crate::AppState>,
    provider_id: String,
    action: String,
) -> std::result::Result<crate::DashboardSnapshot, String> {
    let manager = &state.store.cpa;
    let result = match action.as_str() {
        "begin" => manager.begin(&state.store, &provider_id).await,
        "poll" => manager.poll(&state.store, &provider_id).await,
        "refresh" => manager.refresh(&state.store, &provider_id).await,
        "cancel" => manager.disconnect(&state.store, &provider_id, true).await,
        "disconnect" => manager.disconnect(&state.store, &provider_id, false).await,
        "switch" => match manager.disconnect(&state.store, &provider_id, false).await {
            Ok(()) => manager.begin(&state.store, &provider_id).await,
            Err(e) => Err(e),
        },
        _ => Err(anyhow::anyhow!("Unknown CPA connection action")),
    };
    result.map_err(|e| e.to_string())?;
    Ok(crate::snapshot(&state).await)
}
#[tauri::command]
pub async fn select_cpa_model(
    state: tauri::State<'_, crate::AppState>,
    provider_id: String,
    model_id: String,
    selected: bool,
) -> std::result::Result<crate::DashboardSnapshot, String> {
    state
        .store
        .cpa
        .select(&state.store, &provider_id, &model_id, selected)
        .map_err(|e| e.to_string())?;
    Ok(crate::snapshot(&state).await)
}
