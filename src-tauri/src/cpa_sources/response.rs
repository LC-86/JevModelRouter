//! Dropping an unfinished response locks its finite plan; no replacement dispatch.
use crate::config::ConfigStore;
use futures_util::{Stream, StreamExt};
use std::{pin::Pin, sync::Arc};
/// Once a fixed source is known, every pre-dispatch rejection stops that exact plan.
pub(crate) struct AdmissionGuard {
    store: Arc<ConfigStore>,
    id: String,
    plan: String,
    cpa: bool,
    armed: bool,
}
impl AdmissionGuard {
    pub(crate) fn fixed(store: &Arc<ConfigStore>, model: &str) -> Option<Self> {
        let c = store.read();
        let id = &c.models.iter().find(|m| m.id == model)?.provider_id;
        let (plan, cpa) = if let Some(p) = c
            .cpa_subscriptions
            .get(id)
            .and_then(|c| c.permit.as_ref())
            .filter(|p| p.enabled)
        {
            (p.proof.plan_id.clone(), true)
        } else {
            let p = c
                .api_sources
                .get(id)?
                .hand_run
                .as_ref()
                .filter(|p| p.enabled)?;
            (p.proof.plan_id.clone(), false)
        };
        Some(Self {
            store: store.clone(),
            id: id.clone(),
            plan,
            cpa,
            armed: true,
        })
    }
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        if self.armed {
            if self.cpa {
                let _ = self.store.cpa.fail_plan(&self.store, &self.id, &self.plan);
            } else {
                let _ = crate::coding_hand_run::fail_plan(&self.store, &self.id, &self.plan);
            }
        }
    }
}
pub(crate) struct ResponseLease {
    store: Arc<ConfigStore>,
    id: String,
    complete: bool,
    terminal: bool,
    errored: bool,
    coding_plan: Option<String>,
    function_tools: bool,
}
impl Drop for ResponseLease {
    fn drop(&mut self) {
        if !self.complete {
            if let Some(plan) = &self.coding_plan {
                let _ = crate::coding_hand_run::fail_plan(&self.store, &self.id, plan);
            } else {
                let _ = self.store.cpa.fail_hand_run(&self.store, &self.id);
            }
        }
        if self.coding_plan.is_none() {
            self.store.cpa.finish_transport(&self.id);
        }
    }
}
impl ResponseLease {
    pub(crate) fn reject(&mut self) {
        self.errored = true;
        if let Some(plan) = &self.coding_plan {
            let _ = crate::coding_hand_run::fail_plan(&self.store, &self.id, plan);
        } else {
            let _ = self.store.cpa.fail_hand_run(&self.store, &self.id);
        }
    }
    pub(crate) fn coding(
        store: Arc<ConfigStore>,
        id: String,
        plan: String,
        function_tools: bool,
    ) -> Self {
        let mut lease = Self::new(store, id);
        lease.coding_plan = Some(plan);
        lease.function_tools = function_tools;
        lease
    }
    pub(crate) fn new(store: Arc<ConfigStore>, id: String) -> Self {
        let function_tools = store
            .read()
            .cpa_subscriptions
            .get(&id)
            .and_then(|c| c.permit.as_ref())
            .is_some_and(|p| p.proof.policy.function_tools);
        Self {
            store,
            id,
            complete: false,
            terminal: false,
            errored: false,
            coding_plan: None,
            function_tools,
        }
    }
}
pub(crate) fn guarded<S>(
    stream: S,
    lease: ResponseLease,
    sse: bool,
) -> impl Stream<Item = Result<axum::body::Bytes, reqwest::Error>>
where
    S: Stream<Item = Result<axum::body::Bytes, reqwest::Error>> + Send + 'static,
{
    let stream: Pin<Box<S>> = Box::pin(stream);
    futures_util::stream::unfold(
        (stream, lease, Vec::new()),
        move |(mut stream, mut lease, mut data)| async move {
            match stream.next().await {
                Some(Ok(bytes)) => {
                    if data.len() + bytes.len() <= 65536 {
                        data.extend_from_slice(&bytes);
                    } else {
                        lease.reject();
                    }
                    if sse {
                        for line in data.split(|b| *b == b'\n') {
                            if let Some(json) = line.strip_prefix(b"data: ") {
                                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) {
                                    lease.errored |= v.get("error").is_some();
                                    lease.errored |= !lease.function_tools
                                        && v["choices"].as_array().is_some_and(|c| {
                                            c.iter().any(|c| has_tool_call(&c["delta"]))
                                        });
                                    lease.terminal |= v["choices"].as_array().is_some_and(|c| {
                                        c.iter().any(|c| {
                                            c["finish_reason"] == "stop"
                                                || (lease.function_tools
                                                    && c["finish_reason"] == "tool_calls")
                                        })
                                    });
                                }
                            }
                        }
                        lease.complete = lease.terminal
                            && !lease.errored
                            && data.windows(12).any(|w| w == b"data: [DONE]");
                    }
                    Some((Ok(bytes), (stream, lease, data)))
                }
                Some(Err(error)) => {
                    lease.reject();
                    Some((Err(error), (stream, lease, data)))
                }
                None => {
                    if !sse {
                        lease.complete = !lease.errored
                            && serde_json::from_slice::<serde_json::Value>(&data)
                                .is_ok_and(|v| json_complete(&v, lease.function_tools));
                    }
                    drop(lease);
                    None
                }
            }
        },
    )
}

fn json_complete(v: &serde_json::Value, tools: bool) -> bool {
    v.get("error").is_none()
        && v["choices"]
            .as_array()
            .is_some_and(|c| !c.is_empty() && c.iter().all(|c| choice_complete(c, tools)))
}
fn choice_complete(c: &serde_json::Value, tools: bool) -> bool {
    let m = &c["message"];
    if m["role"] != "assistant" {
        return false;
    }
    match c["finish_reason"].as_str() {
        Some("stop") => m["content"].is_string() && !has_tool_call(m),
        Some("tool_calls") => {
            tools
                && m["tool_calls"].as_array().is_some_and(|t| {
                    !t.is_empty()
                        && t.iter().all(|t| {
                            t["type"] == "function"
                                && t["id"].is_string()
                                && t["function"]["name"].is_string()
                                && t["function"]["arguments"].as_str().is_some_and(|a| {
                                    serde_json::from_str::<serde_json::Value>(a).is_ok()
                                })
                        })
                })
        }
        _ => false,
    }
}

fn has_tool_call(v: &serde_json::Value) -> bool {
    v.get("tool_calls")
        .is_some_and(|v| !v.is_null() && !v.as_array().is_some_and(|a| a.is_empty()))
        || v.get("function_call").is_some_and(|v| !v.is_null())
}
