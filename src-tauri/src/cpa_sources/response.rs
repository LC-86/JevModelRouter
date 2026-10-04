//! Dropping an unfinished response locks its finite plan; no replacement dispatch.
use crate::config::ConfigStore;
use futures_util::{Stream, StreamExt};
use std::{pin::Pin, sync::Arc};
pub(crate) struct ResponseLease {
    store: Arc<ConfigStore>,
    id: String,
    complete: bool,
    terminal: bool,
    errored: bool,
    coding_plan: Option<String>,
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
    pub(crate) fn coding(store: Arc<ConfigStore>, id: String, plan: String) -> Self {
        let mut lease = Self::new(store, id);
        lease.coding_plan = Some(plan);
        lease
    }
    pub(crate) fn new(store: Arc<ConfigStore>, id: String) -> Self {
        Self {
            store,
            id,
            complete: false,
            terminal: false,
            errored: false,
            coding_plan: None,
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
                                    lease.terminal |= v["choices"].as_array().is_some_and(|c| {
                                        c.iter().any(|c| {
                                            matches!(
                                                c["finish_reason"].as_str(),
                                                Some("stop" | "tool_calls")
                                            )
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
                            && serde_json::from_slice::<serde_json::Value>(&data).is_ok_and(|v| {
                                v.get("error").is_none()
                                    && v["choices"].as_array().is_some_and(|c| !c.is_empty())
                            });
                    }
                    drop(lease);
                    None
                }
            }
        },
    )
}
