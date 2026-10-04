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
    errored: bool,
    plan: String,
    coding: bool,
    function_tools: bool,
}
impl Drop for ResponseLease {
    fn drop(&mut self) {
        if !self.complete {
            self.on_failure()();
        }
        if !self.coding {
            self.store.cpa.finish_transport(&self.id);
        }
    }
}
impl ResponseLease {
    pub(crate) fn reject(&mut self) {
        self.errored = true;
        self.on_failure()();
    }
    /// Outer parsers can fail after a valid upstream terminal; bind the original plan.
    pub(crate) fn on_failure(&self) -> Arc<dyn Fn() + Send + Sync> {
        let (store, id, plan, coding) = (
            self.store.clone(),
            self.id.clone(),
            self.plan.clone(),
            self.coding,
        );
        Arc::new(move || {
            if coding {
                let _ = crate::coding_hand_run::fail_plan(&store, &id, &plan);
            } else {
                let _ = store.cpa.fail_plan(&store, &id, &plan);
            }
        })
    }
    pub(crate) fn coding(
        store: Arc<ConfigStore>,
        id: String,
        plan: String,
        function_tools: bool,
    ) -> Self {
        let mut lease = Self::new(store, id);
        lease.plan = plan;
        lease.coding = true;
        lease.function_tools = function_tools;
        lease
    }
    pub(crate) fn new(store: Arc<ConfigStore>, id: String) -> Self {
        let (plan, function_tools) = store
            .read()
            .cpa_subscriptions
            .get(&id)
            .and_then(|c| c.permit.as_ref())
            .map(|p| (p.proof.plan_id.clone(), p.proof.policy.function_tools))
            .unwrap_or_default();
        Self {
            store,
            id,
            complete: false,
            errored: false,
            plan,
            coding: false,
            function_tools,
        }
    }
}
pub(crate) fn guarded<S>(
    stream: S,
    lease: ResponseLease,
    sse: bool,
) -> impl Stream<Item = Result<axum::body::Bytes, std::io::Error>>
where
    S: Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send + 'static,
{
    let stream: Pin<Box<S>> = Box::pin(stream);
    futures_util::stream::unfold(
        (stream, lease, Vec::new(), false),
        move |(mut stream, mut lease, mut data, ended)| async move {
            if ended {
                return None;
            }
            match stream.next().await {
                Some(Ok(bytes)) => {
                    if data.len() + bytes.len() <= 65536 {
                        data.extend_from_slice(&bytes);
                    } else {
                        lease.reject();
                    }
                    if sse && !lease.errored {
                        match sse_complete(&data, lease.function_tools) {
                            Ok(complete) => lease.complete = complete && !lease.errored,
                            Err(()) => {
                                lease.reject();
                                return Some((
                                    Err(std::io::Error::other("Invalid upstream finite response")),
                                    (stream, lease, data, true),
                                ));
                            }
                        }
                    }
                    Some((Ok(bytes), (stream, lease, data, false)))
                }
                Some(Err(error)) => {
                    lease.reject();
                    Some((Err(error), (stream, lease, data, true)))
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

/// Validate the assembled choices, never just a terminal marker in raw bytes.
fn sse_complete(bytes: &[u8], tools: bool) -> Result<bool, ()> {
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    let mut parser = crate::protocol::SseParser::default();
    let frames = parser.push(bytes).map_err(|_| ())?;
    let mut choices: BTreeMap<u64, Value> = BTreeMap::new();
    let mut calls: BTreeMap<(u64, u64), Value> = BTreeMap::new();
    let mut done = false;
    for (_, data) in frames {
        if done {
            return Err(());
        }
        if data == "[DONE]" {
            done = true;
            continue;
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| ())?;
        if value.get("error").is_some() {
            return Err(());
        }
        let Some(items) = value["choices"].as_array() else {
            continue;
        };
        for item in items {
            let index = match item.get("index") {
                Some(v) => v.as_u64().ok_or(())?,
                None => 0,
            };
            let choice = choices.entry(index).or_insert_with(
                || json!({"message":{"role":"assistant","content":""},"finish_reason":null}),
            );
            let delta = &item["delta"];
            if !choice["finish_reason"].is_null()
                && delta.as_object().is_some_and(|d| !d.is_empty())
            {
                return Err(());
            }
            if let Some(role) = delta.get("role") {
                if role != "assistant" {
                    return Err(());
                }
            }
            append_delta(&mut choice["message"], delta, "content")?;
            if delta.get("function_call").is_some_and(|v| !v.is_null()) {
                return Err(());
            }
            if has_tool_call(delta) && !tools {
                return Err(());
            }
            if let Some(tool_delta) = delta.get("tool_calls").filter(|v| !v.is_null()) {
                for tool in tool_delta.as_array().ok_or(())? {
                    let tool_index = tool["index"].as_u64().ok_or(())?;
                    let call = calls
                        .entry((index, tool_index))
                        .or_insert_with(|| json!({"function":{}}));
                    append_delta(call, tool, "id")?;
                    if let Some(kind) = tool.get("type") {
                        if kind != "function" {
                            return Err(());
                        }
                        call["type"] = kind.clone();
                    }
                    for field in ["name", "arguments"] {
                        append_delta(&mut call["function"], &tool["function"], field)?;
                    }
                }
            }
            if let Some(reason) = item.get("finish_reason").filter(|r| !r.is_null()) {
                choice["finish_reason"] = reason.clone();
            }
        }
    }
    if !done {
        return Ok(false);
    }
    if !parser.clean_eof() {
        return Err(());
    }
    for ((index, _), call) in calls {
        let message = &mut choices.get_mut(&index).ok_or(())?["message"];
        if message["tool_calls"].is_null() {
            message["tool_calls"] = json!([]);
        }
        message["tool_calls"].as_array_mut().ok_or(())?.push(call);
    }
    if !json_complete(
        &json!({"choices":choices.into_values().collect::<Vec<_>>()}),
        tools,
    ) {
        return Err(());
    }
    Ok(true)
}

fn append_delta(
    target: &mut serde_json::Value,
    delta: &serde_json::Value,
    field: &str,
) -> Result<(), ()> {
    if let Some(value) = delta.get(field).filter(|v| !v.is_null()) {
        let fragment = value.as_str().ok_or(())?;
        let mut assembled = target[field].as_str().unwrap_or_default().to_owned();
        assembled.push_str(fragment);
        target[field] = assembled.into();
    }
    Ok(())
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
                                && t["id"].as_str().is_some_and(|s| !s.is_empty())
                                && t["function"]["name"]
                                    .as_str()
                                    .is_some_and(|s| !s.is_empty())
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
