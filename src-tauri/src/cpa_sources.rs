//! CPA owns credentials/OAuth/refresh. Jev stores references and identity generations.
//! R3 has no real generation grant; registration and discovery cannot create one.
use crate::{
    config::{AppConfig, ConfigStore, Model, Provider, ProviderKind},
    subscription::{Connection as Identity, ConnectionState, Denial, DenialFamily, EvidenceState},
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

mod client;
mod service;
mod hand_run;
pub mod response;
pub use hand_run::HandRunProof;
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
    #[serde(default)]
    pub owned_profile: Option<service::View>,
    #[serde(skip)]
    pub permit: Option<hand_run::Permit>,
    #[serde(default)]
    pub hand_run_ledger: HashMap<String,hand_run::Ledger>,
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
    pub connection_name: String,
    pub stage: Stage,
    pub connection_instance_id: String,
    pub generation: u64,
    pub account: Option<String>,
    pub plan: Option<String>,
    pub credential_reference:Option<String>,
    pub catalog_state: EvidenceState,
    pub observed_at: Option<String>,
    pub models: Vec<ModelView>,
    pub error: Option<String>,
    pub authorization_url: Option<String>,
    pub service_available: bool,
    pub qualification: &'static str,
    pub quota: &'static str,
    pub capability: &'static str,
    pub owned_service: Option<service::View>,
    pub hand_run: Option<serde_json::Value>,
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
    held:bool,
}
impl Drop for CleanupLease<'_> {
    fn drop(&mut self) {
        if self.held {self.active.lock().unwrap().remove(&self.id);}
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
    services: Mutex<HashMap<String, service::Service>>,
}

pub struct GenerationTarget {pub base:String,pub key:String,pub(crate) lease:Option<response::ResponseLease>}

impl Manager {
    pub fn reconcile_owned(&self,store:&ConfigStore)->Result<()> {
        let dead:Vec<_>=self.services.lock().unwrap().iter_mut().filter_map(|(id,s)|(!s.running().unwrap_or(false)).then(||id.clone())).collect();
        let config=store.read();
        for id in dead {if config.cpa_subscriptions.get(&id).and_then(|c|c.permit.as_ref()).is_some_and(|p|p.enabled || !p.stopped){self.fail_hand_run(store,&id)?;}}
        Ok(())
    }
    pub(crate) fn finish_transport(&self,id:&str){self.cleaning.lock().unwrap().remove(id);}
    pub async fn generation_target(&self,store:&Arc<ConfigStore>,id:&str,model:&Model,protocol:crate::protocol::Protocol,body:&serde_json::Value)->Result<GenerationTarget> {
        let mut ownership=self.cleanup_lease(id)?;
        let outcome=async {
            self.ensure_owned_running(id)?;
            let snapshot=store.read();let c=snapshot.cpa_subscriptions.get(id).context("Unknown CPA connection")?;
            let identity=stamp(id,c);
            admit_model(&snapshot,snapshot.providers.iter().find(|p|p.id==id).context("Unknown provider")?,Some(model),protocol)?;
            c.permit.as_ref().context("CPA permission missing")?.validate_request(protocol,body)?;
            store.update(|config|->Result<()> {
                let c=config.cpa_subscriptions.get_mut(id).context("Connection removed")?;
                ensure!(identity.matches(id,c),"Identity changed before management read");
                let p=c.permit.as_ref().context("CPA permission missing")?;
                let ledger=c.hand_run_ledger.get_mut(&p.proof.plan_id).context("Missing plan ledger")?;
                ensure!(!ledger.stopped && ledger.auxiliary_used<p.proof.policy.max_auxiliary_requests,"Finite auxiliary request budget exhausted");
                ledger.auxiliary_used+=1;Ok(())
            })??;
            let files=self.client(id)?.credentials().await?;
            ensure!(files.len()==1 && files[0].provider==c.provider && !files[0].disabled && Some(&files[0].name)==c.credential_ref.as_ref(),"Owned credential became missing or ambiguous");
            ensure!(known(files[0].identity())==identity.account && known(files[0].id_token.plan_type.clone())==identity.plan,"Owned account or plan changed before dispatch");
            ensure!(identity.matches(id,store.read().cpa_subscriptions.get(id).context("Connection removed")?),"Connection changed during admission");
            let owned=self.services.lock().unwrap().get(id).map(|s|GenerationTarget{base:s.base(),key:s.model_key(),lease:None});
            #[cfg(test)]
            let owned=owned.or_else(||self.fixture.lock().unwrap().as_ref().map(|c|GenerationTarget{base:c.model_base(),key:"fictional-cpa-generation".into(),lease:None}));
            let mut target=owned.context("Owned CPA transport is unavailable")?;
            self.reserve_hand_run(store,id,model,protocol,body)?;
            target.lease=Some(response::ResponseLease::new(store.clone(),id.into()));
            ownership.held=false;
            Ok(target)
        }.await;
        if outcome.is_err(){self.fail_hand_run(store,id)?;}
        outcome
    }

    pub fn enable_hand_run(&self, store:&ConfigStore,id:&str,proof:HandRunProof)->Result<()> {
        ensure!(!self.cleaning.lock().unwrap().contains(id),"Finish this owned request before enabling another plan");
        self.ensure_owned_running(id)?;
        store.update(|config|->Result<()> {
            let c=config.cpa_subscriptions.get(id).context("Unknown CPA connection")?;
            ensure!(c.stage==Stage::Connected && c.catalog_state==EvidenceState::Available,"Connect and refresh this source before HAND_RUN");
            proof.validate(id,c)?;
            ensure!(c.catalog.iter().any(|m|m.model_id==proof.binding.model_id) && config.models.iter().any(|m|m.provider_id==id && m.model_id==proof.binding.model_id && m.selected && m.enabled && config.cpa_model_bindings.get(&m.id)==Some(&proof.binding)),"Select a current model bound to this identity first");
            let c=config.cpa_subscriptions.get_mut(id).unwrap();
            ensure!(c.hand_run_ledger.len()<100 || c.hand_run_ledger.contains_key(&proof.plan_id),"HAND_RUN plan limit reached");
            let ledger=c.hand_run_ledger.entry(proof.plan_id.clone()).or_insert(hand_run::Ledger{proof_sha256:crate::hand_run_policy::proof_sha256(&proof)?,used:0,max_requests:proof.policy.max_requests,stopped:false,auxiliary_used:0,tool_rounds_used:0});
            ensure!(!ledger.stopped && proof.policy.max_requests==ledger.max_requests && ledger.proof_sha256==crate::hand_run_policy::proof_sha256(&proof)?,"A stopped plan cannot resume or expand its budget");
            let expires_at=proof.policy.evidence.iter().map(|e|e.valid_until).min().unwrap();
            let tools=proof.policy.function_tools;let upstream_model=proof.binding.model_id.clone();
            c.permit=Some(hand_run::Permit{proof,enabled:true,used:ledger.used,expires_at,stopped:false});
            for m in config.models.iter_mut().filter(|m|m.provider_id==id && m.model_id==upstream_model){m.supports_tools=tools;}
            Ok(())
        })??;
        Ok(())
    }
    pub fn disable_hand_run(&self,store:&ConfigStore,id:&str)->Result<()> {
        store.update(|config| {if let Some(p)=config.cpa_subscriptions.get_mut(id).and_then(|c|c.permit.as_mut()){p.enabled=false;}})?;
        Ok(())
    }
    pub fn fail_hand_run(&self,store:&ConfigStore,id:&str)->Result<()> {
        store.update(|config| {if let Some(c)=config.cpa_subscriptions.get_mut(id){if let Some(p)=c.permit.as_mut(){p.enabled=false;p.stopped=true;if let Some(l)=c.hand_run_ledger.get_mut(&p.proof.plan_id){l.stopped=true;}}}})?;
        Ok(())
    }
    fn ensure_owned_running(&self,id:&str)->Result<()> {
        let live=self.services.lock().unwrap().get_mut(id).map(|s|s.running()).transpose()?;
        ensure!(live==Some(true) || (cfg!(test) && self.fixture.lock().unwrap().is_some()),"Owned CPA service is unavailable; recover explicitly");
        ensure!(self.clients.lock().unwrap().contains_key(id),"Dedicated management client is unavailable");
        Ok(())
    }
    pub fn reserve_hand_run(&self,store:&ConfigStore,id:&str,model:&Model,protocol:crate::protocol::Protocol,body:&serde_json::Value)->Result<()> {
        if let Err(error)=self.ensure_owned_running(id){self.fail_hand_run(store,id)?;return Err(error);}
        let result=store.update(|config|->Result<()> {
            let provider=config.providers.iter().find(|p|p.id==id).context("Unknown provider")?;
            admit_model(config,provider,Some(model),protocol)?;
            let c=config.cpa_subscriptions.get_mut(id).unwrap();
            let permit=c.permit.as_ref().context("CPA generation remains disabled")?;
            permit.validate_request(protocol,body)?;
            let plan=permit.proof.plan_id.clone();
            let ledger=c.hand_run_ledger.get_mut(&plan).context("HAND_RUN ledger is unavailable")?;
            ensure!(!ledger.stopped && ledger.used<ledger.max_requests,"HAND_RUN finite budget is exhausted or stopped");
            let tool_round=body["messages"].as_array().is_some_and(|m|m.iter().any(|m|m["role"]=="tool"));
            ensure!(!tool_round || ledger.tool_rounds_used<permit.proof.policy.max_tool_rounds,"Finite client tool round budget exhausted");
            if tool_round {ledger.tool_rounds_used+=1;}
            ledger.used+=1;c.permit.as_mut().unwrap().used=ledger.used;
            Ok(())
        })?;
        if result.is_err(){self.fail_hand_run(store,id)?;}
        result
    }

    pub async fn provision_owned(&self, store: &ConfigStore, id: &str, binary: String, port: u16) -> Result<service::View> {
        let _lease = self.cleanup_lease(id)?;
        ensure!(!self.attempts.lock().unwrap().contains_key(id), "Finish owned authorization before changing its service");
        {
            let mut services = self.services.lock().unwrap();
            if let Some(service) = services.get_mut(id) {
                ensure!(!service.running()?, "Stop the owned service before restarting it");
            }
            services.remove(id);
        }
        self.clients.lock().unwrap().remove(id);
        self.disable_hand_run(store,id)?;
        let connection = store.read().cpa_subscriptions.get(id).cloned().context("Unknown CPA connection")?;
        let mut owned = service::Service::start(store.cpa_profile_root(), &connection.identity.connection_instance_id, binary, port).await?;
        self.configure_owned_client(id, owned.base(), owned.management_key())?;
        let view = owned.view()?;
        store.update(|config| {
            if let Some(c) = config.cpa_subscriptions.get_mut(id) { c.owned_profile = Some(view.clone()); }
        })?;
        self.services.lock().unwrap().insert(id.into(), owned);
        Ok(view)
    }
    pub fn stop_owned(&self, id: &str) -> Result<()> {
        let _lease = self.cleanup_lease(id)?;
        ensure!(!self.attempts.lock().unwrap().contains_key(id), "Cancel authorization before stopping its service");
        let owned=self.services.lock().unwrap().remove(id);
        if let Some(mut service) = owned { service.stop()?; }
        self.clients.lock().unwrap().remove(id);
        Ok(())
    }
    // Called only after the process owner has provisioned this connection's profile.
    pub(crate) fn configure_owned_client(&self, id: &str, base: String, key: String) -> Result<()> {
        let client = Arc::new(client::Client::owned_service(base, key)?);
        self.clients.lock().unwrap().insert(id.into(), client);
        Ok(())
    }
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
            held:true,
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
        // Remove only this process handle. A failed stop retains its recovery entry.
        let owned = self.services.lock().unwrap().remove(id);
        if let Some(mut service) = owned {
            if let Err(error) = service.stop() {
                self.services.lock().unwrap().insert(id.into(), service);
                return Err(error);
            }
        }
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
            matches!(provider,"codex"|"xai"),
            "Only the pinned Codex and xAI management contracts are available"
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
                kind: if provider=="codex"{ProviderKind::CodexSubscription}else{ProviderKind::GrokSubscription},
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
                    owned_profile: None,
                    permit: None,
                    hand_run_ledger: HashMap::new(),
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
                connection_name: config
                    .providers
                    .iter()
                    .find(|provider| provider.id == *id)
                    .map(|provider| provider.name.clone())
                    .unwrap_or_else(|| c.provider.clone()),
                stage: c.stage,
                connection_instance_id: c.identity.connection_instance_id.clone(),
                generation: c.identity.generation,
                account: c.identity.identity.clone(),
                plan: c.plan.clone(),
                credential_reference:c.credential_ref.clone(),
                catalog_state: c.catalog_state,
                observed_at: c.observed_at.clone(),
                error: c.error.clone(),
                service_available: clients.contains_key(id) && self.services.lock().unwrap().get_mut(id).is_none_or(|s| s.running().unwrap_or(false)),
                authorization_url: attempts
                    .get(id)
                    .filter(|a| a.stamp.matches(id, c) && c.stage == Stage::Waiting)
                    .and_then(|a| a.url.clone()),
                hand_run:c.permit.as_ref().map(|p|serde_json::json!({"enabled":p.ready(id,c,&p.proof.binding.model_id),"stopped":p.stopped,"used":p.used,"max_requests":p.proof.policy.max_requests,"expires_at":p.expires_at,"model_id":p.proof.binding.model_id})),
                qualification: if c.permit.as_ref().is_some_and(|p|p.ready(id,c,&p.proof.binding.model_id)){"reviewed_available"}else{"unknown"},
                quota: if c.permit.as_ref().is_some_and(|p|p.ready(id,c,&p.proof.binding.model_id)){"reviewed_available"}else{"unknown"},
                capability: if c.permit.as_ref().is_some_and(|p|p.ready(id,c,&p.proof.binding.model_id)){"reviewed_chat"}else{"unverified"},
                owned_service: self.services.lock().unwrap().get_mut(id).and_then(|service| service.view().ok())
                    .or_else(|| c.owned_profile.clone().map(|mut view| {view.running=false;view.pid=0;view})),
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
            if !client.credentials().await?.is_empty() {
                self.retire_profile(id, &client);
                anyhow::bail!(
                    "Dedicated CPA profile contains an unclaimed credential; old service profile isolated. 旧服务配置已隔离；请删除旧连接并配置新的专用服务。未认领凭据不会删除。"
                );
            }
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
                    held:true,
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
                c.identity.identity = known(credential.identity());
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
            let account=known(file.identity());
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
        let mut services = std::mem::take(&mut *self.services.lock().unwrap());
        let owned_ids:Vec<_>=services.keys().cloned().collect();
        for service in services.values_mut() { service.stop()?; }
        let mut clients=self.clients.lock().unwrap();
        for id in owned_ids {clients.remove(&id);}
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
    } else if model.is_some_and(|m| c.permit.as_ref().is_some_and(|p| p.ready(&provider.id,c,&m.model_id))) {
        return None;
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
    c.permit = None;
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
        c.permit = None;
    c.identity.state = ConnectionState::NotConnected;
        c.error = Some(error.into());
        Ok(())
    })??;
    Ok(())
}

#[tauri::command]
pub async fn configure_cpa_service(state: tauri::State<'_, crate::AppState>, provider_id: String, binary: String, port: u16) -> std::result::Result<crate::DashboardSnapshot, String> {
    state.store.cpa.provision_owned(&state.store, &provider_id, binary, port).await.map_err(|e|format!("{e:#}"))?;
    Ok(crate::snapshot(&state).await)
}
#[tauri::command]
pub async fn stop_cpa_service(state: tauri::State<'_, crate::AppState>, provider_id: String) -> std::result::Result<crate::DashboardSnapshot, String> {
    state.store.cpa.disable_hand_run(&state.store,&provider_id).map_err(|e|e.to_string())?;
    state.store.cpa.stop_owned(&provider_id).map_err(|e|format!("{e:#}"))?;
    Ok(crate::snapshot(&state).await)
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

pub fn admit_model(config:&AppConfig,provider:&Provider,model:Option<&Model>,protocol:crate::protocol::Protocol)->Result<()> {
    if let Some(denial)=denial(config,provider,model){anyhow::bail!("{}: {}",denial.code,denial.message);}
    let c=config.cpa_subscriptions.get(&provider.id).context("Unknown CPA connection")?;
    let model=model.context("Select a stable bound CPA model")?;
    ensure!(c.catalog_state==EvidenceState::Available && c.catalog.iter().any(|m|m.model_id==model.model_id),"CPA model eligibility is stale or removed");
    let permit=c.permit.as_ref().context("CPA generation remains disabled")?;
    ensure!(permit.ready(&provider.id,c,&model.model_id),"CPA finite permission is unavailable");
    ensure!(protocol==crate::protocol::Protocol::Chat,"CPA protocol capability is unverified");
    permit.proof.validate(&provider.id,c)
}

#[tauri::command]
pub async fn set_cpa_hand_run(state:tauri::State<'_,crate::AppState>,provider_id:String,enabled:bool)->std::result::Result<crate::DashboardSnapshot,String> {
    let manager=&state.store.cpa;
    if enabled {
        let config=state.store.read();let c=config.cpa_subscriptions.get(&provider_id).ok_or("Unknown CPA connection")?;
        let path=state.store.cpa_profile_root().join(&c.identity.connection_instance_id).join("hand-run.reviewed.json");
        let metadata=std::fs::symlink_metadata(&path).map_err(|_|format!("Put independently reviewed source evidence at {}",path.display()))?;
        if !metadata.is_file() || metadata.len()>65536 {return Err("Use a regular owned evidence file up to 64 KiB".into());}
        let proof:HandRunProof=serde_json::from_slice(&std::fs::read(path).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        manager.enable_hand_run(&state.store,&provider_id,proof).map_err(|e|e.to_string())?;
    }else{manager.disable_hand_run(&state.store,&provider_id).map_err(|e|e.to_string())?;}
    Ok(crate::snapshot(&state).await)
}

#[tauri::command]
pub fn open_cpa_authorization(state:tauri::State<'_,crate::AppState>,app:tauri::AppHandle,provider_id:String)->std::result::Result<(),String> {
    use tauri_plugin_opener::OpenerExt;
    crate::runtime::external_action().map_err(|e|e.to_string())?;
    let config=state.store.read();
    let url=state.store.cpa.views(&config).into_iter().find(|c|c.provider_id==provider_id).and_then(|c|c.authorization_url).ok_or("No current owned authorization URL")?;
    app.opener().open_url(url,None::<&str>).map_err(|e|e.to_string())
}
