//! Grok ACP generation. The production entry remains closed until Issue #26 verifies the CLI.

#[cfg(test)]
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
#[cfg(test)]
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, watch},
};

use crate::protocol::Protocol;

#[cfg(test)]
use super::GrokSubscriptionAdapter;
#[cfg(test)]
use crate::subscription::{
    helper, GenerationEvent, GenerationFinishReason, GenerationRequest, GenerationStream,
};

#[cfg(test)]
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const CANCEL_NOTIFY_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_INPUT_BYTES: usize = 64 * 1024;
#[cfg(test)]
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
struct TextTurn {
    text: Vec<String>,
    max_tokens: Option<u64>,
}

/// Accept only the text subset ACP can represent without silently dropping API semantics.
fn parse_text_turn(protocol: Protocol, body: &Value, model_id: &str) -> Result<TextTurn> {
    ensure!(
        !model_id.trim().is_empty() && model_id.len() <= 256,
        "A bounded model id is required"
    );
    let object = body
        .as_object()
        .context("The request body must be an object")?;
    let allowed: &[&str] = match protocol {
        Protocol::Chat => &[
            "model",
            "stream",
            "messages",
            "max_tokens",
            "max_completion_tokens",
        ],
        Protocol::Responses => &["model", "stream", "input", "max_output_tokens"],
        Protocol::Messages => &["model", "stream", "messages", "max_tokens"],
    };
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        bail!("Unsupported Grok generation field `{field}`");
    }
    if let Some(value) = body.get("model") {
        let requested = value.as_str().context("`model` must be a string")?;
        ensure!(
            !requested.trim().is_empty() && requested.len() <= 256,
            "`model` must be a bounded non-empty string"
        );
    }
    if body.get("stream").is_some_and(|value| !value.is_boolean()) {
        bail!("`stream` must be a boolean");
    }
    let max_tokens = request_max_tokens(protocol, body)?;
    if protocol == Protocol::Messages {
        ensure!(
            max_tokens.is_some(),
            "Messages requires a positive integer `max_tokens`"
        );
    }

    let messages =
        match protocol {
            Protocol::Chat | Protocol::Messages => {
                let items = body
                    .get("messages")
                    .and_then(Value::as_array)
                    .context("The request requires a `messages` array")?;
                ensure!(
                    items.len() == 1,
                    "Grok ACP accepts exactly one text user message per request"
                );
                let message = &items[0];
                let message = message
                    .as_object()
                    .context("Each message must be an object")?;
                if let Some(field) = message
                    .keys()
                    .find(|field| !["role", "content"].contains(&field.as_str()))
                {
                    bail!("Unsupported Grok message field `{field}`");
                }
                ensure!(
                    message.get("role").and_then(Value::as_str) == Some("user"),
                    "Grok ACP accepts only user text, not system, assistant, or tool history"
                );
                message
                    .get("content")
                    .context("A user message requires `content`")?
            }
            Protocol::Responses => {
                let input = body.get("input").context("Responses requires `input`")?;
                match input {
                    Value::String(_) => input,
                    Value::Array(items) => {
                        ensure!(
                            items.len() == 1,
                            "Grok ACP accepts exactly one text user message per request"
                        );
                        let item = items.first().unwrap();
                        let item = item
                            .as_object()
                            .context("Each input item must be an object")?;
                        if let Some(field) = item
                            .keys()
                            .find(|field| !["type", "role", "content"].contains(&field.as_str()))
                        {
                            bail!("Unsupported Responses input field `{field}`");
                        }
                        ensure!(
                            item.get("type").and_then(Value::as_str) == Some("message"),
                            "Grok ACP accepts only text message input"
                        );
                        ensure!(item.get("role").and_then(Value::as_str) == Some("user"),
                        "Grok ACP accepts only user text, not system, assistant, or tool history");
                        item.get("content")
                            .context("A user input item requires `content`")?
                    }
                    _ => bail!("Responses requires string input or one text message"),
                }
            }
        };
    let text = text_value(messages)?;
    ensure!(
        text.iter().any(|part| !part.trim().is_empty()),
        "Grok ACP requires non-empty user text"
    );
    let text_bytes = text
        .iter()
        .fold(0usize, |total, part| total.saturating_add(part.len()));
    ensure!(
        text_bytes <= MAX_INPUT_BYTES,
        "Grok input exceeded the 64 KiB text limit"
    );
    Ok(TextTurn { text, max_tokens })
}

fn request_max_tokens(protocol: Protocol, body: &Value) -> Result<Option<u64>> {
    let keys: &[&str] = match protocol {
        Protocol::Chat => &["max_tokens", "max_completion_tokens"],
        Protocol::Responses => &["max_output_tokens"],
        Protocol::Messages => &["max_tokens"],
    };
    let present: Vec<_> = keys
        .iter()
        .filter(|key| body.get(**key).is_some())
        .collect();
    ensure!(present.len() <= 1, "Specify only one output-token limit");
    let Some(key) = present.first() else {
        return Ok(None);
    };
    let value = body
        .get(**key)
        .and_then(Value::as_u64)
        .context("The output-token limit must be a positive integer")?;
    ensure!(
        (1..=1_000_000).contains(&value),
        "The output-token limit must be between 1 and 1,000,000"
    );
    Ok(Some(value))
}

pub(crate) fn validate_generation_request(
    protocol: Protocol,
    body: &Value,
    model_id: &str,
) -> Result<()> {
    parse_text_turn(protocol, body, model_id).map(|_| ())
}

fn text_value(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::String(text) => Ok(vec![text.clone()]),
        Value::Array(blocks) => {
            ensure!(!blocks.is_empty(), "Text content cannot be empty");
            let mut text = Vec::with_capacity(blocks.len());
            for block in blocks {
                let object = block
                    .as_object()
                    .context("Content blocks must be objects")?;
                if let Some(field) = object
                    .keys()
                    .find(|field| !["type", "text"].contains(&field.as_str()))
                {
                    bail!("Unsupported Grok content field `{field}`");
                }
                ensure!(
                    object.get("type").and_then(Value::as_str) == Some("text"),
                    "Grok ACP accepts text content only; images and other content are unsupported"
                );
                let part = object
                    .get("text")
                    .and_then(Value::as_str)
                    .context("Text blocks require string `text`")?;
                text.push(part.to_owned());
            }
            Ok(text)
        }
        _ => bail!("Grok ACP accepts text content only; images and other content are unsupported"),
    }
}

#[cfg(test)]
pub(super) async fn generate<'a>(
    adapter: &'a GrokSubscriptionAdapter,
    request: GenerationRequest<'a>,
) -> Result<GenerationStream<'a>> {
    let turn = parse_text_turn(request.protocol, &request.body, request.model_id)?;
    ensure!(
        !request.provider_id.trim().is_empty(),
        "A Grok provider id is required"
    );
    let program = adapter
        .program
        .as_ref()
        .context("No Grok CLI helper was found")?;
    let spec = helper::generation_spec_with_program(
        request.provider_id,
        &adapter.home,
        program,
        request.model_id,
    )?;
    helper::prepare_home(&spec.home)?;

    let provider_id = request.provider_id.to_owned();
    let generation = request.generation;
    let key = (provider_id.clone(), generation);
    let request_id = uuid::Uuid::new_v4().to_string();
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    adapter
        .active_generations
        .lock()
        .unwrap()
        .entry(key.clone())
        .or_default()
        .insert(request_id.clone(), cancel_tx);
    let guard = ActiveGenerationGuard {
        active: adapter.active_generations.clone(),
        key,
        request_id,
    };

    let generation_lock = adapter
        .generation_locks
        .lock()
        .unwrap()
        .entry(provider_id.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let provider_guard = tokio::select! {
        _ = cancel_rx.changed() => bail!("Grok generation was cancelled before dispatch"),
        guard = generation_lock.lock_owned() => guard,
    };
    (request.pre_dispatch_check)().map_err(anyhow::Error::msg)?;

    let workspace = RequestWorkspace::create()?;
    let mut child = start_process(&spec, workspace.path())?;
    let stdin = child
        .stdin
        .take()
        .context("The Grok ACP helper has no stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("The Grok ACP helper has no stdout")?;
    let mut rpc = RpcProcess {
        child: Some(child),
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 0,
    };
    let prompt_content: Vec<_> = turn
        .text
        .iter()
        .map(|text| json!({"type":"text","text":text}))
        .collect();
    let turn_timeout = adapter.generation_timeout;
    let prepared = async {
        let init = call_cancellable(&mut rpc, "initialize", json!({
            "protocolVersion": 1,
            "clientInfo": {"name":"autojev","title":"AutoJev","version":env!("CARGO_PKG_VERSION")},
            "clientCapabilities": {"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}
        }), &mut cancel_rx).await?;
        let cached = init
            .get("authMethods")
            .and_then(Value::as_array)
            .is_some_and(|methods| {
                methods
                    .iter()
                    .any(|method| method.get("id").and_then(Value::as_str) == Some("cached_token"))
            });
        ensure!(
            cached,
            "Grok ACP did not advertise cached account authentication"
        );
        call_cancellable(
            &mut rpc,
            "authenticate",
            json!({"methodId":"cached_token","_meta":{"headless":true}}),
            &mut cancel_rx,
        )
        .await?;
        let session = call_cancellable(
            &mut rpc,
            "session/new",
            json!({"cwd":workspace.path(),"mcpServers":[]}),
            &mut cancel_rx,
        )
        .await?;
        let session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|session| !session.trim().is_empty())
            .context("Grok ACP returned no session id")?
            .to_owned();
        if let Some(max_tokens) = turn.max_tokens {
            set_max_tokens(&mut rpc, &session_id, &session, max_tokens, &mut cancel_rx).await?;
        }
        (request.pre_dispatch_check)().map_err(anyhow::Error::msg)?;

        let turn_deadline = tokio::time::Instant::now() + turn_timeout;
        let prompt_id = rpc
            .send_request_cancellable(
                "session/prompt",
                json!({"sessionId":session_id,"prompt":prompt_content}),
                &mut cancel_rx,
                turn_deadline,
            )
            .await?;
        Ok::<_, anyhow::Error>((prompt_id, session_id, turn_deadline))
    }
    .await;
    let (prompt_id, session_id, turn_deadline) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            rpc.stop().await;
            return Err(error);
        }
    };
    let (tx, rx) = mpsc::unbounded_channel::<GenerationEvent>();
    let provider_id = request.provider_id.to_owned();
    let model_id = request.model_id.to_owned();
    let protocol_check = request.pre_dispatch_check.clone();
    tokio::spawn(async move {
        let _request_guard = guard;
        let _provider_guard = provider_guard;
        let _workspace = workspace;
        let _ = tx.send(GenerationEvent::Started { generation });
        let outcome = prompt_loop(
            &mut rpc,
            prompt_id,
            &session_id,
            tx.clone(),
            &mut cancel_rx,
            protocol_check,
            turn_deadline,
        )
        .await;
        rpc.stop().await;
        match outcome {
            PromptOutcome::Finished => {
                let _ = tx.send(GenerationEvent::Finished { status: 200 });
            }
            PromptOutcome::MaxTokens => {
                let _ = tx.send(GenerationEvent::FinishedWithReason {
                    status: 200,
                    reason: GenerationFinishReason::MaxTokens,
                });
            }
            PromptOutcome::Cancelled => {
                let _ = tx.send(GenerationEvent::Cancelled);
            }
            PromptOutcome::Failed(message) => {
                let _ = tx.send(GenerationEvent::Failed { message });
            }
        }
        let _ = (&provider_id, &model_id);
    });
    let stream = futures_util::stream::unfold(rx, |mut receiver| async move {
        receiver.recv().await.map(|event| (event, receiver))
    });
    Ok(Box::pin(stream))
}

#[cfg(test)]
fn start_process(spec: &helper::HelperSpec, cwd: &Path) -> Result<Child> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .env_clear()
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command
        .spawn()
        .with_context(|| format!("Start the Grok ACP helper {}", spec.program.display()))
}

#[cfg(test)]
struct RpcProcess {
    child: Option<Child>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

#[cfg(test)]
impl RpcProcess {
    async fn send_request_cancellable(
        &mut self,
        method: &str,
        params: Value,
        cancel: &mut watch::Receiver<bool>,
        deadline: tokio::time::Instant,
    ) -> Result<u64> {
        self.next_id += 1;
        let id = self.next_id;
        self.write_cancellable(
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            cancel,
            deadline,
        )
        .await?;
        Ok(id)
    }

    async fn notify_until(
        &mut self,
        method: &str,
        params: Value,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        self.write_until(
            &json!({"jsonrpc":"2.0","method":method,"params":params}),
            deadline,
        )
        .await
    }

    async fn write(&mut self, value: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .context("Write to the Grok ACP helper")?;
        self.stdin
            .flush()
            .await
            .context("Flush the Grok ACP helper request")
    }

    async fn write_cancellable(
        &mut self,
        value: &Value,
        cancel: &mut watch::Receiver<bool>,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        tokio::select! {
            biased;
            _ = cancel.changed() => bail!("Grok ACP request was cancelled during write"),
            result = tokio::time::timeout_at(deadline, self.write(value)) => {
                result.context("Grok ACP request write timed out")??;
            }
        }
        Ok(())
    }

    async fn write_until(&mut self, value: &Value, deadline: tokio::time::Instant) -> Result<()> {
        tokio::time::timeout_at(deadline, self.write(value))
            .await
            .context("Grok ACP notification write timed out")??;
        Ok(())
    }

    async fn read(&mut self) -> Result<Option<Value>> {
        let mut line = String::new();
        let count = self
            .stdout
            .read_line(&mut line)
            .await
            .context("Read from the Grok ACP helper")?;
        if count == 0 {
            return Ok(None);
        }
        serde_json::from_str(&line).context("Grok ACP returned invalid JSON-RPC")
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    }
}

#[cfg(test)]
async fn call_cancellable(
    rpc: &mut RpcProcess,
    method: &str,
    params: Value,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
    let id = rpc
        .send_request_cancellable(method, params, cancel, deadline)
        .await?;
    loop {
        let value = tokio::select! {
            biased;
            _ = cancel.changed() => bail!("Grok ACP request was cancelled"),
            result = tokio::time::timeout_at(deadline, rpc.read()) => {
                result.context("Grok ACP request timed out")??
                    .context("Grok ACP ended before completing a request")?
            }
        };
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(error) = value.get("error") {
                bail!(
                    "Grok ACP rejected `{method}`: {}",
                    helper::redact(&error.to_string())
                );
            }
            return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
        }
        if value.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            deny_permission(rpc, &value, cancel, deadline).await?;
            bail!("Grok ACP requested unsupported tool or permission access");
        }
    }
}

#[cfg(test)]
async fn set_max_tokens(
    rpc: &mut RpcProcess,
    session_id: &str,
    session: &Value,
    max_tokens: u64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<()> {
    let desired = max_tokens.to_string();
    let option = session
        .get("configOptions")
        .and_then(Value::as_array)
        .and_then(|options| {
            options
                .iter()
                .find(|option| option.get("id").and_then(Value::as_str) == Some("max_tokens"))
        });
    let supported = option.is_some_and(|option| {
        option.get("type").and_then(Value::as_str) == Some("select")
            && option
                .get("options")
                .and_then(Value::as_array)
                .is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| value.get("value").and_then(Value::as_str) == Some(&desired))
                })
    });
    ensure!(
        supported,
        "Grok ACP cannot preserve the requested output-token limit"
    );
    let result = call_cancellable(
        rpc,
        "session/set_config_option",
        json!({"sessionId":session_id,"configId":"max_tokens","value":desired}),
        cancel,
    )
    .await?;
    let applied = result
        .get("configOptions")
        .and_then(Value::as_array)
        .is_some_and(|options| {
            options.iter().any(|option| {
                option.get("id").and_then(Value::as_str) == Some("max_tokens")
                    && option.get("currentValue").and_then(Value::as_str) == Some(&desired)
            })
        });
    ensure!(
        applied,
        "Grok ACP did not confirm the requested output-token limit"
    );
    Ok(())
}

#[cfg(test)]
enum PromptOutcome {
    Finished,
    MaxTokens,
    Cancelled,
    Failed(String),
}

#[cfg(test)]
async fn prompt_loop(
    rpc: &mut RpcProcess,
    prompt_id: u64,
    session_id: &str,
    tx: mpsc::UnboundedSender<GenerationEvent>,
    cancel: &mut watch::Receiver<bool>,
    pre_dispatch_check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    deadline: tokio::time::Instant,
) -> PromptOutcome {
    let mut output_bytes = 0usize;
    loop {
        let line = tokio::select! {
            biased;
            _ = tx.closed() => {
                notify_session_cancel(rpc, session_id).await;
                return PromptOutcome::Cancelled;
            }
            changed = cancel.changed() => {
                let _ = changed;
                notify_session_cancel(rpc, session_id).await;
                return PromptOutcome::Cancelled;
            }
            result = tokio::time::timeout_at(deadline, rpc.read()) => match result {
                Ok(Ok(Some(value))) => value,
                Ok(Ok(None)) => return PromptOutcome::Failed("Grok ACP exited before reporting a terminal status".into()),
                Ok(Err(_)) => return PromptOutcome::Failed("Grok ACP returned an invalid stream event".into()),
                Err(_) => {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Failed("Grok generation exceeded its turn timeout".into());
                }
            }
        };
        if value_id(&line) == Some(prompt_id) {
            if line.get("error").is_some() {
                return PromptOutcome::Failed("Grok ACP failed the request".into());
            }
            match line.pointer("/result/stopReason").and_then(Value::as_str) {
                Some("end_turn") => return PromptOutcome::Finished,
                Some("cancelled") => return PromptOutcome::Cancelled,
                Some("max_tokens") => return PromptOutcome::MaxTokens,
                _ => {
                    return PromptOutcome::Failed(
                        "Grok ACP returned an unsupported terminal status".into(),
                    )
                }
            }
        }
        if line.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            if deny_permission(rpc, &line, cancel, deadline).await.is_err() {
                return PromptOutcome::Failed(
                    "Grok ACP requested unsupported tool or permission access".into(),
                );
            }
            return PromptOutcome::Failed(
                "Grok ACP requested unsupported tool or permission access".into(),
            );
        }
        if line.get("method").and_then(Value::as_str) != Some("session/update") {
            continue;
        }
        let Some(params) = line.get("params") else {
            return PromptOutcome::Failed("Grok ACP sent an invalid update".into());
        };
        if params.get("sessionId").and_then(Value::as_str) != Some(session_id) {
            continue;
        }
        let Some(update) = params.get("update") else {
            return PromptOutcome::Failed("Grok ACP sent an invalid update".into());
        };
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
                    return PromptOutcome::Failed("Grok ACP returned non-text output".into());
                };
                output_bytes = output_bytes.saturating_add(text.len());
                if output_bytes > MAX_OUTPUT_BYTES {
                    return PromptOutcome::Failed(
                        "Grok output exceeded the 16 MiB response limit".into(),
                    );
                }
                if pre_dispatch_check().is_err() {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Cancelled;
                }
                if !text.is_empty() && tx.send(GenerationEvent::Chunk(text.to_owned())).is_err() {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Cancelled;
                }
            }
            Some("tool_call" | "tool_call_update") => {
                notify_session_cancel(rpc, session_id).await;
                return PromptOutcome::Failed("Grok ACP attempted unsupported tool use".into());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
fn value_id(value: &Value) -> Option<u64> {
    value.get("id").and_then(Value::as_u64)
}

#[cfg(test)]
async fn notify_session_cancel(rpc: &mut RpcProcess, session_id: &str) {
    let deadline = tokio::time::Instant::now() + CANCEL_NOTIFY_TIMEOUT;
    let _ = rpc
        .notify_until("session/cancel", json!({"sessionId":session_id}), deadline)
        .await;
}

#[cfg(test)]
async fn deny_permission(
    rpc: &mut RpcProcess,
    message: &Value,
    cancel: &mut watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Result<()> {
    let id = message
        .get("id")
        .cloned()
        .context("Permission request has no id")?;
    rpc.write_cancellable(
        &json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}}),
        cancel,
        deadline,
    )
    .await
}

#[cfg(test)]
struct ActiveGenerationGuard {
    active: Arc<Mutex<HashMap<(String, u64), HashMap<String, watch::Sender<bool>>>>>,
    key: (String, u64),
    request_id: String,
}

#[cfg(test)]
impl Drop for ActiveGenerationGuard {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap();
        if let Some(requests) = active.get_mut(&self.key) {
            requests.remove(&self.request_id);
            if requests.is_empty() {
                active.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
struct RequestWorkspace {
    path: PathBuf,
}

#[cfg(test)]
impl RequestWorkspace {
    fn create() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "autojev-grok-request-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&path).context("Create an isolated Grok request directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .context("Restrict the Grok request directory")?;
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
impl Drop for RequestWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::SubscriptionAdapter;
    use futures_util::StreamExt;

    fn install_fake_acp(directory: &Path, script_body: &str) -> PathBuf {
        let helper = directory.join("fake-acp.py");
        std::fs::write(&helper, script_body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        helper
    }

    async fn blocked_prompt_write_cleans_up_helper(cancel_request: bool) {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("helper.pid");
        let ready_file = temp.path().join("helper.ready");
        let pid_path = serde_json::to_string(pid_file.to_str().unwrap()).unwrap();
        let ready_path = serde_json::to_string(ready_file.to_str().unwrap()).unwrap();
        let script = format!(
            r##"#!/usr/bin/env python3
import json, os, sys, time
session = "blocked-prompt-session"
for line in sys.stdin:
    req = json.loads(line); method = req.get("method"); ident = req.get("id")
    if method == "initialize": result = {{"protocolVersion":1,"authMethods":[{{"id":"cached_token"}}]}}
    elif method == "authenticate": result = {{}}
    elif method == "session/new": result = {{"sessionId":session}}
    else: result = {{}}
    sys.stdout.write(json.dumps({{"jsonrpc":"2.0","id":ident,"result":result}}) + "\n"); sys.stdout.flush()
    if method == "session/new":
        with open({pid_path}, "w") as pid: pid.write(str(os.getpid()))
        with open({ready_path}, "w") as ready: ready.write("ready")
        time.sleep(30)
        break
"##
        );
        let helper = install_fake_acp(temp.path(), &script);
        let mut adapter =
            GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper);
        adapter.generation_timeout = if cancel_request {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(100)
        };
        let adapter = Arc::new(adapter);
        let request_adapter = adapter.clone();
        let provider_id = if cancel_request {
            "grok-blocked-write-cancel"
        } else {
            "grok-blocked-write-timeout"
        };
        let body = json!({
            "model":"grok-test",
            "messages":[{"role":"user","content":"x".repeat(MAX_INPUT_BYTES)}]
        });
        let request_task = tokio::spawn(async move {
            let mut events = request_adapter
                .generate(GenerationRequest {
                    provider_id,
                    generation: 77,
                    model_id: "grok-test",
                    protocol: Protocol::Chat,
                    body,
                    pre_dispatch_check: Arc::new(|| Ok(())),
                })
                .await?;
            while events.next().await.is_some() {}
            Ok::<_, anyhow::Error>(())
        });

        let ready = tokio::time::timeout(Duration::from_secs(10), async {
            while !ready_file.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if ready.is_err() {
            let task_result = if request_task.is_finished() {
                format!("; generation completed early: {:?}", request_task.await)
            } else {
                "; generation still pending".to_owned()
            };
            panic!("fake helper did not reach the blocked prompt write{task_result}");
        }

        if cancel_request {
            tokio::time::sleep(Duration::from_millis(100)).await;
            adapter.cancel_generation(provider_id, 77);
        }
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(3), request_task)
            .await
            .expect("blocked prompt write must remain bounded")
            .unwrap();
        let error = match result {
            Ok(_) => panic!("blocked prompt write must fail"),
            Err(error) => error,
        };
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "write cancellation or timeout took {:?}",
            started.elapsed()
        );
        assert!(
            error.to_string().contains(if cancel_request {
                "cancelled"
            } else {
                "timed out"
            }),
            "unexpected blocked-write failure: {error:#}"
        );

        #[cfg(unix)]
        {
            let pid = std::fs::read_to_string(pid_file).unwrap();
            let status = std::process::Command::new("kill")
                .arg("-0")
                .arg(pid.trim())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                !status.success(),
                "failed Grok ACP helper {pid} was not reaped"
            );
        }
    }

    fn admitted_grok_store(
        directory: &Path,
        adapter: Arc<GrokSubscriptionAdapter>,
    ) -> Arc<crate::config::ConfigStore> {
        use crate::{
            config::{Model, Provider, ProviderKind},
            dispatch::ApiDispatcher,
            subscription::{
                Capability, CapabilityStatus, CatalogEvidence, ConnectionState, Evidence,
                EvidenceState, QuotaBucket, QuotaCredits, QuotaEvidence, QuotaPermission,
                QuotaView, QuotaWindow,
            },
        };
        let dispatcher = Arc::new(ApiDispatcher {
            loopback_only: true,
        });
        let store = Arc::new(
            crate::config::ConfigStore::load_with_adapters(
                directory.join("gateway.db"),
                dispatcher,
                adapter,
            )
            .unwrap(),
        );
        store
            .update(|config| {
                let provider = Provider {
                    preset: String::new(),
                    api_type: String::new(),
                    test_model: String::new(),
                    id: "grok-fixture".into(),
                    name: "Grok fixture".into(),
                    kind: ProviderKind::GrokSubscription,
                    base_url: String::new(),
                    enabled: true,
                    has_api_key: false,
                };
                config.providers.push(provider.clone());
                let mut model: Model = config.models[0].clone();
                model.id = "grok-fixture-binding".into();
                model.provider_id = provider.id.clone();
                model.model_id = "grok-test".into();
                model.name = "Grok fixture model".into();
                model.enabled = true;
                model.selected = true;
                model.supports_vision = false;
                config.models.push(model.clone());
                crate::subscription::sync_provider(config, &provider.id, &provider.kind);
                let connection = config.subscriptions.get_mut(&provider.id).unwrap();
                connection.state = ConnectionState::Connected;
                connection.identity = Some("fixture@example.invalid".into());
                connection.evidence = Some(Evidence {
                    generation: connection.generation,
                    account: connection.identity.clone(),
                    helper_version: Some("fixture".into()),
                    account_path: Some("isolated-fixture".into()),
                    models: Vec::new(),
                    capabilities: [Protocol::Chat, Protocol::Responses, Protocol::Messages]
                        .into_iter()
                        .map(|protocol| Capability {
                            model_id: model.model_id.clone(),
                            protocol: crate::subscription::protocol_key(protocol).into(),
                            status: CapabilityStatus::Verified,
                        })
                        .collect(),
                    quota: QuotaEvidence {
                        state: EvidenceState::Available,
                        source: Some("controlled fixture".into()),
                        view: QuotaView::GrokCliUsage,
                        buckets: vec![QuotaBucket {
                            limit_id: "subscription_pool".into(),
                            permission: QuotaPermission::Allowed,
                            windows: vec![QuotaWindow {
                                label: "primary".into(),
                                used_percent: Some(1.0),
                                window_minutes: Some(60),
                                ..Default::default()
                            }],
                            credits: Some(QuotaCredits {
                                permission: QuotaPermission::Denied,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    catalog: CatalogEvidence {
                        state: EvidenceState::Available,
                        source: Some("controlled fixture".into()),
                        ..Default::default()
                    },
                });
                config.subscription_catalogs.insert(
                    provider.id.clone(),
                    crate::subscription_catalog::ProviderCatalog {
                        entries: vec![crate::subscription_catalog::CatalogEntry {
                            model_id: model.model_id.clone(),
                            name: Some(model.name.clone()),
                            internal_id: model.id.clone(),
                            availability: crate::subscription_catalog::Availability::Available,
                            first_seen: Some("fixture".into()),
                            last_confirmed: Some("fixture".into()),
                            confirmed_generation: Some(connection.generation),
                            account: connection.identity.clone(),
                        }],
                    },
                );
            })
            .unwrap();
        store
    }

    #[test]
    fn accepts_one_text_user_message_for_each_protocol() {
        let cases = [
            (
                Protocol::Chat,
                json!({"model":"grok-test","stream":true,"messages":[{"role":"user","content":"hello"}]}),
            ),
            (
                Protocol::Responses,
                json!({"model":"grok-test","stream":true,"input":"hello"}),
            ),
            (
                Protocol::Messages,
                json!({"model":"grok-test","stream":true,"max_tokens":32,"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]}),
            ),
        ];
        for (protocol, body) in cases {
            assert_eq!(
                parse_text_turn(protocol, &body, "grok-test").unwrap().text,
                vec!["hello".to_owned()]
            );
        }
    }

    #[tokio::test]
    async fn grok_acp_preserves_text_block_boundaries_without_inserting_separators() {
        let temp = tempfile::tempdir().unwrap();
        let captured_prompt = temp.path().join("captured-prompt.json");
        let captured_prompt_literal =
            serde_json::to_string(captured_prompt.to_str().unwrap()).unwrap();
        let script = format!(
            r##"#!/usr/bin/env python3
import json, sys
session = "text-block-session"
for line in sys.stdin:
    req = json.loads(line); method = req.get("method"); ident = req.get("id")
    if method == "initialize": result = {{"protocolVersion":1,"authMethods":[{{"id":"cached_token"}}]}}
    elif method == "authenticate": result = {{}}
    elif method == "session/new": result = {{"sessionId":session}}
    elif method == "session/prompt":
        with open({captured_prompt_literal}, "w") as capture: json.dump(req["params"]["prompt"], capture)
        result = {{"stopReason":"end_turn"}}
    else: result = {{}}
    sys.stdout.write(json.dumps({{"jsonrpc":"2.0","id":ident,"result":result}}) + "\n"); sys.stdout.flush()
    if method == "session/prompt": break
"##
        );
        let helper = install_fake_acp(temp.path(), &script);
        let adapter = GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper);
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "grok-text-blocks",
                generation: 1,
                model_id: "grok-test",
                protocol: Protocol::Chat,
                body: json!({
                    "model":"grok-test",
                    "messages":[{"role":"user","content":[
                        {"type":"text","text":"Reply exactly: AB"},
                        {"type":"text","text":"CD"}
                    ]}]
                }),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        while events.next().await.is_some() {}

        let captured: Value =
            serde_json::from_str(&std::fs::read_to_string(captured_prompt).unwrap()).unwrap();
        assert_eq!(
            captured,
            json!([
                {"type":"text","text":"Reply exactly: AB"},
                {"type":"text","text":"CD"}
            ])
        );
    }

    #[test]
    fn rejects_semantics_acp_cannot_preserve() {
        let unsupported = [
            (
                Protocol::Chat,
                json!({"model":"grok-test","temperature":0.2,"messages":[{"role":"user","content":"hello"}]}),
            ),
            (
                Protocol::Chat,
                json!({"model":"grok-test","messages":[{"role":"assistant","content":"history"}]}),
            ),
            (
                Protocol::Responses,
                json!({"model":"grok-test","instructions":"system","input":"hello"}),
            ),
            (
                Protocol::Responses,
                json!({"model":"grok-test","input":[{"type":"message","role":"user","content":[{"type":"input_image","image_url":"https://invalid"}]}]}),
            ),
            (
                Protocol::Messages,
                json!({"model":"grok-test","max_tokens":0,"messages":[{"role":"user","content":"hello"}]}),
            ),
        ];
        for (protocol, body) in unsupported {
            assert!(
                parse_text_turn(protocol, &body, "grok-test").is_err(),
                "{protocol:?}: {body}"
            );
        }
        assert!(parse_text_turn(
            Protocol::Chat,
            &json!({"model":false,"messages":[{"role":"user","content":"x"}]}),
            "grok-test"
        )
        .is_err());
    }

    #[tokio::test]
    async fn normal_adapter_cannot_enter_generation_without_the_test_only_helper() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = GrokSubscriptionAdapter::from_parts(
            temp.path().to_path_buf(),
            None,
            Vec::new(),
            Duration::from_secs(15),
        );
        let result = adapter
            .generate(GenerationRequest {
                provider_id: "grok-no-bypass",
                generation: 1,
                model_id: "grok-test",
                protocol: Protocol::Chat,
                body: json!({"model":"grok-test","messages":[{"role":"user","content":"hello"}]}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await;
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("without an injected local test helper"));
    }

    #[tokio::test]
    async fn actual_grok_adapter_streams_fake_acp_and_fails_after_partial_text_without_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let helper = temp.path().join("fake-acp.py");
        let script = r##"#!/usr/bin/env python3
import json, sys
session = "fake-session"
for line in sys.stdin:
    req = json.loads(line)
    method, ident = req.get("method"), req.get("id")
    if method == "initialize": result = {"protocolVersion":1,"authMethods":[{"id":"cached_token"}]}
    elif method == "authenticate": result = {}
    elif method == "session/new": result = {"sessionId":session,"configOptions":[{"id":"max_tokens","type":"select","currentValue":"4096","options":[{"value":"32","name":"32"},{"value":"4096","name":"4096"}]}]}
    elif method == "session/set_config_option": result = {"configOptions":[{"id":"max_tokens","type":"select","currentValue":req["params"]["value"],"options":[{"value":"32","name":"32"},{"value":"4096","name":"4096"}]}]}
    elif method == "session/prompt":
        params = req["params"]
        text = params["prompt"][0]["text"]
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"echo:" + text}}}}) + "\n"); sys.stdout.flush()
        if text == "partial-failure":
            sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":{"stopReason":"max_turn_requests"}}) + "\n")
        else:
            reason = "max_tokens" if text == "max-token" else "end_turn"
            sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":{"stopReason":reason}}) + "\n")
        sys.stdout.flush()
        break
    else: result = {}
    sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":result}) + "\n"); sys.stdout.flush()
"##;
        std::fs::write(&helper, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let adapter = GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper);
        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            let body = match protocol {
                Protocol::Chat => {
                    json!({"model":"grok-test","stream":true,"messages":[{"role":"user","content":"hello"}]})
                }
                Protocol::Responses => json!({"model":"grok-test","stream":true,"input":"hello"}),
                Protocol::Messages => {
                    json!({"model":"grok-test","stream":true,"max_tokens":32,"messages":[{"role":"user","content":"hello"}]})
                }
            };
            let mut events = adapter
                .generate(GenerationRequest {
                    provider_id: "grok-a",
                    generation: 1,
                    model_id: "grok-test",
                    protocol,
                    body,
                    pre_dispatch_check: Arc::new(|| Ok(())),
                })
                .await
                .unwrap();
            let mut collected = Vec::new();
            while let Some(event) = events.next().await {
                collected.push(event);
            }
            assert!(
                collected.contains(&GenerationEvent::Chunk("echo:hello".into())),
                "{protocol:?}: {collected:?}"
            );
            assert!(
                collected.contains(&GenerationEvent::Finished { status: 200 }),
                "{protocol:?}: {collected:?}"
            );
        }
        let mut failed = adapter.generate(GenerationRequest {
            provider_id:"grok-a", generation:1, model_id:"grok-test", protocol:Protocol::Chat,
            body:json!({"model":"grok-test","messages":[{"role":"user","content":"partial-failure"}]}),
            pre_dispatch_check:Arc::new(|| Ok(())),
        }).await.unwrap();
        let mut collected = Vec::new();
        while let Some(event) = failed.next().await {
            collected.push(event);
        }
        assert!(collected.contains(&GenerationEvent::Chunk("echo:partial-failure".into())));
        assert!(collected
            .iter()
            .any(|event| matches!(event, GenerationEvent::Failed { .. })));
        assert!(!collected
            .iter()
            .any(|event| matches!(event, GenerationEvent::Finished { .. })));

        let mut limited = adapter
            .generate(GenerationRequest {
                provider_id: "grok-a",
                generation: 1,
                model_id: "grok-test",
                protocol: Protocol::Messages,
                body: json!({"model":"grok-test","max_tokens":32,"messages":[{"role":"user","content":"max-token"}]}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        let mut collected = Vec::new();
        while let Some(event) = limited.next().await {
            collected.push(event);
        }
        assert!(collected.contains(&GenerationEvent::FinishedWithReason {
            status: 200,
            reason: GenerationFinishReason::MaxTokens,
        }));
    }

    #[tokio::test]
    async fn grok_gateway_returns_json_and_incremental_sse_for_all_three_protocols() {
        let temp = tempfile::tempdir().unwrap();
        let helper = install_fake_acp(
            temp.path(),
            r##"#!/usr/bin/env python3
import json, os, sys
home = os.environ["GROK_HOME"]
with open(os.path.join(home, "runs"), "a") as log: log.write(os.getcwd() + "\n")
session = "fixture-session"
for line in sys.stdin:
    req = json.loads(line); method = req.get("method"); ident = req.get("id")
    if method == "initialize": result = {"protocolVersion":1,"authMethods":[{"id":"cached_token"}]}
    elif method == "authenticate": result = {}
    elif method == "session/new": result = {"sessionId":session,"configOptions":[{"id":"max_tokens","type":"select","currentValue":"4096","options":[{"value":"32","name":"32"},{"value":"4096","name":"4096"}]}]}
    elif method == "session/set_config_option": result = {"configOptions":[{"id":"max_tokens","type":"select","currentValue":req["params"]["value"],"options":[{"value":"32","name":"32"},{"value":"4096","name":"4096"}]}]}
    elif method == "session/prompt":
        params = req["params"]; prompt = params["prompt"][0]["text"]
        text = "grok:" + prompt
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}) + "\n")
        reason = "max_tokens" if prompt == "max-token" else "end_turn"
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":{"stopReason":reason}}) + "\n"); sys.stdout.flush(); break
    else: result = {}
    sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":result}) + "\n"); sys.stdout.flush()
"##,
        );
        let adapter = Arc::new(GrokSubscriptionAdapter::with_test_helper(
            temp.path().to_path_buf(),
            helper,
        ));
        let store = admitted_grok_store(temp.path(), adapter.clone());
        let alias = "autojev/model/grok-fixture-binding";
        let mut calls = 0;

        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            for streaming in [false, true] {
                let body = match protocol {
                    Protocol::Chat => {
                        json!({"model":alias,"stream":streaming,"max_tokens":32,"messages":[{"role":"user","content":"hello"}]})
                    }
                    Protocol::Responses => {
                        json!({"model":alias,"stream":streaming,"max_output_tokens":32,"input":"hello"})
                    }
                    Protocol::Messages => {
                        json!({"model":alias,"stream":streaming,"max_tokens":32,"messages":[{"role":"user","content":"hello"}]})
                    }
                };
                let reply = crate::proxy::forward_test_request(
                    store.clone(),
                    body,
                    crate::subscription::protocol_key(protocol),
                )
                .await;
                assert_eq!(
                    reply.status(),
                    axum::http::StatusCode::OK,
                    "{protocol:?}/{streaming}"
                );
                assert_eq!(
                    reply
                        .headers()
                        .get("x-should-retry")
                        .and_then(|value| value.to_str().ok()),
                    Some("false")
                );
                let bytes = axum::body::to_bytes(reply.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                let text = String::from_utf8(bytes.to_vec()).unwrap();
                if streaming {
                    assert!(text.contains("grok:hello"), "{protocol:?}: {text}");
                    match protocol {
                        Protocol::Chat => assert!(text.contains("data: [DONE]"), "{text}"),
                        Protocol::Responses => {
                            assert!(text.contains("event: response.completed"), "{text}")
                        }
                        Protocol::Messages => {
                            assert!(text.contains("event: message_stop"), "{text}")
                        }
                    }
                } else {
                    let body: Value = serde_json::from_str(&text).unwrap();
                    let output = match protocol {
                        Protocol::Chat => body.pointer("/choices/0/message/content"),
                        Protocol::Responses => body.pointer("/output/0/content/0/text"),
                        Protocol::Messages => body.pointer("/content/0/text"),
                    }
                    .and_then(Value::as_str);
                    assert_eq!(output, Some("grok:hello"), "{protocol:?}: {body}");
                }
                calls += 1;
            }
        }

        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            for streaming in [false, true] {
                let body = match protocol {
                    Protocol::Chat => {
                        json!({"model":alias,"stream":streaming,"max_tokens":32,"messages":[{"role":"user","content":"max-token"}]})
                    }
                    Protocol::Responses => {
                        json!({"model":alias,"stream":streaming,"max_output_tokens":32,"input":"max-token"})
                    }
                    Protocol::Messages => {
                        json!({"model":alias,"stream":streaming,"max_tokens":32,"messages":[{"role":"user","content":"max-token"}]})
                    }
                };
                let reply = crate::proxy::forward_test_request(
                    store.clone(),
                    body,
                    crate::subscription::protocol_key(protocol),
                )
                .await;
                assert_eq!(
                    reply.status(),
                    axum::http::StatusCode::OK,
                    "{protocol:?}/{streaming}"
                );
                let bytes = axum::body::to_bytes(reply.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                let text = String::from_utf8(bytes.to_vec()).unwrap();
                if streaming {
                    let expected = match protocol {
                        Protocol::Chat => "\"finish_reason\":\"length\"",
                        Protocol::Responses => "event: response.incomplete",
                        Protocol::Messages => "\"stop_reason\":\"max_tokens\"",
                    };
                    assert!(text.contains(expected), "{protocol:?}: {text}");
                    if protocol == Protocol::Responses {
                        let data = text
                            .split("event: response.incomplete\n")
                            .nth(1)
                            .and_then(|event| {
                                event.lines().find_map(|line| line.strip_prefix("data: "))
                            })
                            .expect("Responses token-limit terminal event");
                        let response: Value = serde_json::from_str(data).unwrap();
                        assert_eq!(
                            response.pointer("/response/status").and_then(Value::as_str),
                            Some("incomplete")
                        );
                        assert_eq!(
                            response
                                .pointer("/response/incomplete_details/reason")
                                .and_then(Value::as_str),
                            Some("max_output_tokens")
                        );
                        assert_eq!(
                            response
                                .pointer("/response/output/0/status")
                                .and_then(Value::as_str),
                            Some("completed")
                        );
                    }
                } else {
                    let body: Value = serde_json::from_str(&text).unwrap();
                    let reason = match protocol {
                        Protocol::Chat => body
                            .pointer("/choices/0/finish_reason")
                            .and_then(Value::as_str),
                        Protocol::Responses => body
                            .pointer("/incomplete_details/reason")
                            .and_then(Value::as_str),
                        Protocol::Messages => body.get("stop_reason").and_then(Value::as_str),
                    };
                    assert_eq!(
                        reason,
                        Some(match protocol {
                            Protocol::Chat => "length",
                            Protocol::Responses => "max_output_tokens",
                            Protocol::Messages => "max_tokens",
                        }),
                        "{protocol:?}: {body}"
                    );
                    if protocol == Protocol::Responses {
                        assert_eq!(
                            body.pointer("/output/0/status").and_then(Value::as_str),
                            Some("completed")
                        );
                    }
                }
                calls += 1;
            }
        }

        let rejected = crate::proxy::forward_test_request(
            store,
            json!({
                "model":alias,"messages":[{"role":"user","content":"hello"}],"tools":[]
            }),
            crate::subscription::protocol_key(Protocol::Chat),
        )
        .await;
        assert_eq!(
            rejected.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        let run_log = std::fs::read_to_string(
            temp.path()
                .join(".autojev/subscription-helpers/grok_subscription/grok-fixture/home/runs"),
        )
        .unwrap();
        assert_eq!(
            run_log.lines().count(),
            calls,
            "unsupported tools must be refused before starting ACP"
        );
        let workspaces: std::collections::HashSet<_> = run_log.lines().collect();
        assert_eq!(
            workspaces.len(),
            calls,
            "each request gets an isolated ACP working directory"
        );

        for provider_id in ["grok-account-a", "grok-account-b"] {
            let mut events = adapter.generate(GenerationRequest {
                provider_id, generation: 1, model_id: "grok-test", protocol: Protocol::Chat,
                body: json!({"model":"grok-test","messages":[{"role":"user","content":"account probe"}]}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            }).await.unwrap();
            while events.next().await.is_some() {}
        }
        let account_a = helper::helper_home(
            &crate::config::ProviderKind::GrokSubscription,
            "grok-account-a",
            temp.path(),
        )
        .unwrap();
        let account_b = helper::helper_home(
            &crate::config::ProviderKind::GrokSubscription,
            "grok-account-b",
            temp.path(),
        )
        .unwrap();
        assert_ne!(
            account_a, account_b,
            "each provider account must receive a separate credential home"
        );
        assert!(account_a.join("runs").is_file() && account_b.join("runs").is_file());
    }

    #[tokio::test]
    async fn generation_cancellation_sends_acp_cancel_and_terminates_the_owned_process() {
        let temp = tempfile::tempdir().unwrap();
        let helper = install_fake_acp(
            temp.path(),
            r##"#!/usr/bin/env python3
import json, sys
session = "cancel-session"
for line in sys.stdin:
    req = json.loads(line); method = req.get("method"); ident = req.get("id")
    if method == "initialize": result = {"protocolVersion":1,"authMethods":[{"id":"cached_token"}]}
    elif method == "authenticate": result = {}
    elif method == "session/new": result = {"sessionId":session}
    elif method == "session/prompt":
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"partial"}}}}) + "\n"); sys.stdout.flush()
        for cancel in sys.stdin:
            if json.loads(cancel).get("method") == "session/cancel": break
        break
    else: result = {}
    sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":result}) + "\n"); sys.stdout.flush()
"##,
        );
        let adapter = GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper);
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "grok-cancel",
                generation: 9,
                model_id: "grok-test",
                protocol: Protocol::Responses,
                body: json!({"model":"grok-test","input":"hello"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert_eq!(
            events.next().await,
            Some(GenerationEvent::Started { generation: 9 })
        );
        assert_eq!(
            events.next().await,
            Some(GenerationEvent::Chunk("partial".into()))
        );
        adapter.cancel_generation("grok-cancel", 9);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), events.next())
                .await
                .unwrap(),
            Some(GenerationEvent::Cancelled)
        );
        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn generation_turn_timeout_fails_the_partial_response_and_terminates_the_owned_process() {
        let temp = tempfile::tempdir().unwrap();
        let helper = install_fake_acp(
            temp.path(),
            r##"#!/usr/bin/env python3
import json, sys
session = "timeout-session"
for line in sys.stdin:
    req = json.loads(line); method = req.get("method"); ident = req.get("id")
    if method == "initialize": result = {"protocolVersion":1,"authMethods":[{"id":"cached_token"}]}
    elif method == "authenticate": result = {}
    elif method == "session/new": result = {"sessionId":session}
    elif method == "session/prompt":
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"partial"}}}}) + "\n"); sys.stdout.flush()
        for _ in sys.stdin: pass
        break
    else: result = {}
    sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":result}) + "\n"); sys.stdout.flush()
"##,
        );
        let mut adapter =
            GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper);
        adapter.generation_timeout = Duration::from_millis(50);
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "grok-timeout",
                generation: 10,
                model_id: "grok-test",
                protocol: Protocol::Responses,
                body: json!({"model":"grok-test","input":"hello"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert!(matches!(
            events.next().await,
            Some(GenerationEvent::Started { .. })
        ));
        assert_eq!(
            events.next().await,
            Some(GenerationEvent::Chunk("partial".into()))
        );
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), events.next())
                .await
                .unwrap(),
            Some(GenerationEvent::Failed { .. })
        ));
        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn cancelling_a_backpressured_prompt_write_stops_and_reaps_the_helper() {
        blocked_prompt_write_cleans_up_helper(true).await;
    }

    #[tokio::test]
    async fn a_backpressured_prompt_write_obeys_the_turn_deadline_and_reaps_the_helper() {
        blocked_prompt_write_cleans_up_helper(false).await;
    }
}
