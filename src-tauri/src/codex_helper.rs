//! 官方 Codex 辅助进程的监管：专用授权目录、可执行文件解析、stdio JSON-RPC 客户端与脱敏。
//!
//! 本模块只管理**自己 spawn 的**子进程：专用 `CODEX_HOME`、单进程读写、退出时 kill + wait。
//! 生产只解析官方 `codex`；`--autojev-helper` 覆盖只存在于 `isolation-check` 构建，且需要
//! `runtime::isolated()`。错误与日志在这里统一脱敏，不输出 token、authorization code 或 refresh token 原文。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsStr,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::subscription::{
    quota_state, CatalogRead, ConnectionState, ConnectionStatus, DiscoveredModel, EvidenceState, GenerationEvent,
    GenerationRequest, GenerationStream, HelperStatus, LoginResult, LoginStart, LogoutOutcome, QuotaBucket,
    QuotaCredits, QuotaEvidence, QuotaPermission, QuotaView, QuotaWindow, RemoteRevocation, SubscriptionAdapter,
};
use crate::{config::ProviderKind, protocol::Protocol};

/// 用户主目录下由本应用独占的辅助进程根目录。
const HELPER_ROOT: &str = ".autojev/helpers/codex";
/// 官方 Codex 常见安装位置；PATH 优先。
#[cfg(unix)]
const COMMON_CODEX_LOCATIONS: &[&str] = &[
    "/opt/homebrew/bin/codex",
    "/usr/local/bin/codex",
    "/usr/bin/codex",
    "/bin/codex"];
#[cfg(not(unix))]
const COMMON_CODEX_LOCATIONS: &[&str] = &[];

/// 从 helper 环境里剔除的凭据类变量；辅助进程只应使用自己的 `CODEX_HOME`。
const CREDENTIAL_ENV: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENAI_ACCESS_TOKEN",
    "OPENAI_REFRESH_TOKEN",
    "OPENAI_ID_TOKEN",
    "CHATGPT_TOKEN",
    "CHATGPT_API_KEY",
    "CODEX_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "CODEX_REFRESH_TOKEN",
    "CODEX_ID_TOKEN",
    "AUTOJEV_CODEX_HELPER_TOKEN",
];

/// 专用授权目录：`<home>/.autojev/helpers/codex/<sanitized provider id>`。
/// 绝不读写用户的 `~/.codex`。
pub fn helper_home(provider_id: &str) -> PathBuf {
    helper_home_in(&crate::runtime::home_dir().unwrap_or_default(), provider_id)
}

fn helper_home_in(home: &Path, provider_id: &str) -> PathBuf {
    home.join(HELPER_ROOT).join(sanitize_provider_id(provider_id))
}

/// 仅保留 `[A-Za-z0-9_-]`，其余替换为 `_`；结果为空时用 `provider`。
fn sanitize_provider_id(provider_id: &str) -> String {
    let sanitized: String = provider_id
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_' })
        .collect();
    if sanitized.is_empty() {
        "provider".to_owned()
    } else {
        sanitized
    }
}

/// 一次辅助进程启动的完整描述（可执行文件 + 固定参数）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// 官方辅助进程的子命令：`codex app-server`，默认 stdio 传输。
const HELPER_SUBCOMMAND: &str = "app-server";
/// `initialize` 的客户端自述；只用于握手，不含账号或凭据。
const CLIENT_INFO_NAME: &str = "autojev";
const CLIENT_INFO_TITLE: &str = "AutoJev";

/// 官方 `codex` 的启动描述。子命令固定在代码里，不由配置或环境决定。
fn official_launch(program: PathBuf) -> HelperLaunch {
    HelperLaunch { program, args: vec![HELPER_SUBCOMMAND.to_owned()] }
}

/// 解析要启动的辅助进程。生产只接受 PATH 或常见安装位置里的官方 `codex`；
/// 覆盖入口只存在于 `isolation-check` 构建，且必须已经处于隔离模式。
pub fn resolve_launch() -> Result<HelperLaunch> {
    if let Some(launch) = override_launch() {
        return Ok(launch);
    }
    let program = resolve_official(std::env::var_os("PATH").as_deref(), &common_locations())
        .context("The official Codex executable was not found; install Codex or add it to PATH")?;
    Ok(official_launch(program))
}

/// 隔离验收专用的覆盖入口。生产构建里它被编译成恒 `None`。
#[cfg(feature = "isolation-check")]
fn override_launch() -> Option<HelperLaunch> {
    if !crate::runtime::isolated() {
        return None;
    }
    crate::runtime::helper_override().map(|program| HelperLaunch { program: program.to_path_buf(), args: Vec::new() })
}

#[cfg(not(feature = "isolation-check"))]
fn override_launch() -> Option<HelperLaunch> {
    None
}

fn common_locations() -> Vec<PathBuf> {
    COMMON_CODEX_LOCATIONS.iter().map(PathBuf::from).collect()
}

/// 纯函数形式的解析接缝：显式传入 PATH 与候选目录，便于离线断言而不改动进程环境。
fn resolve_official(path_env: Option<&OsStr>, common: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path_env) = path_env {
        for directory in std::env::split_paths(path_env) {
            if let Some(program) = executable_in(&directory) {
                return Some(program);
            }
        }
    }
    common.iter().find_map(|candidate| candidate.is_file().then(|| candidate.clone()))
}

fn executable_in(directory: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let names = ["codex.exe", "codex.cmd", "codex.bat"];
    #[cfg(not(windows))]
    let names = ["codex"];
    names.iter().find_map(|name| {
        let candidate = directory.join(name);
        candidate.is_file().then(|| candidate.clone())
    })
}

/// 文本里按 `key=value` / `key:value` 形式出现的敏感键。
const TEXT_SECRET_KEYS: &[&str] = &[
    "authorization", "authorization_code", "access_token", "refresh_token", "id_token",
    "api_key", "apikey", "secret", "password", "passwd", "token", "code",
];
/// JSON 对象里整体隐藏取值的敏感键（`authorizationUrl`、`code` 这类非秘密字段不在此列）。
const JSON_SECRET_KEYS: &[&str] =
    &["token", "secret", "password", "passwd", "credential", "api_key", "apikey", "authorization_code"];

/// 脱敏一行文本：隐藏敏感键的取值与高熵长串，并限制长度。
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(512));
    for (index, word) in text.split_whitespace().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&redact_word(word));
        if out.len() > 400 {
            // 绝不在多字节字符中间截断：先回退到合法字符边界（redact 不能 panic）。
            let mut limit = 400;
            while limit > 0 && !out.is_char_boundary(limit) {
                limit -= 1;
            }
            out.truncate(limit);
            out.push('…');
            break;
        }
    }
    out
}

fn redact_word(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    for separator in ['=', ':'] {
        if let Some(position) = lower.find(separator) {
            let key = lower[..position].trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_');
            if TEXT_SECRET_KEYS.contains(&key) {
                return format!("{}[redacted]", &word[..position + 1]);
            }
        }
    }
    if looks_like_secret(word) {
        "[redacted]".to_owned()
    } else {
        word.to_owned()
    }
}

/// 长且同时含字母与数字的紧致串按秘密处理；短词保留可读性。
fn looks_like_secret(word: &str) -> bool {
    let cleaned = word.trim_matches(|character: char| !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '=')));
    if cleaned.len() < 24 {
        return false;
    }
    let opaque = cleaned
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '='));
    let mixed = cleaned.chars().any(|character| character.is_ascii_alphabetic())
        && cleaned.chars().any(|character| character.is_ascii_digit());
    opaque && mixed
}

/// Keep the initialize self-report display-only. The version parser deliberately accepts only
/// a canonical `codex_cli_rs/<semver>` first token; no result here affects protocol/model gates.
fn helper_self_report(response: &Value) -> (Option<String>, Option<String>) {
    let raw = response.get("userAgent").and_then(Value::as_str);
    let version = raw.and_then(parse_user_agent_version);
    let identity = raw.and_then(|text| {
        let printable = text.chars().filter(|character| !character.is_control()).collect::<String>();
        let redacted = redact(printable.trim());
        (!redacted.is_empty()).then_some(redacted)
    });
    (identity, version)
}

fn parse_user_agent_version(user_agent: &str) -> Option<String> {
    let token = user_agent.split_ascii_whitespace().next()?;
    let version = token.strip_prefix("codex_cli_rs/")?;
    is_semver(version).then(|| version.to_owned())
}

fn is_semver(version: &str) -> bool {
    let mut build_parts = version.split('+');
    let Some(prerelease_and_core) = build_parts.next() else { return false };
    let build = build_parts.next();
    if build_parts.next().is_some() || build.is_some_and(|part| !valid_semver_identifiers(part, false)) {
        return false;
    }
    let core = match prerelease_and_core.split_once('-') {
        Some((core, prerelease)) if valid_semver_identifiers(prerelease, true) => core,
        Some(_) => return false,
        None => prerelease_and_core,
    };
    let components = core.split('.').collect::<Vec<_>>();
    components.len() == 3 && components.iter().all(|component| valid_semver_number(component))
}

fn valid_semver_number(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn valid_semver_identifiers(value: &str, reject_numeric_leading_zero: bool) -> bool {
    !value.is_empty()
        && value.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && (!reject_numeric_leading_zero
                    || !identifier.bytes().all(|byte| byte.is_ascii_digit())
                    || identifier == "0"
                    || !identifier.starts_with('0'))
        })
}

/// 递归脱敏 JSON：敏感键整体隐藏，其余字符串走 [`redact`]。
pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    if JSON_SECRET_KEYS.iter().any(|secret| lower.contains(secret)) {
                        (key.clone(), Value::String("[redacted]".into()))
                    } else {
                        (key.clone(), redact_json(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        Value::String(text) => Value::String(redact(text)),
        other => other.clone(),
    }
}

/// 一次 RPC 的等待上限。平台侧正常都应远快于此。
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// stdio 换行分隔 JSON-RPC 2.0 客户端：一个辅助进程一个实例，只读写自己 spawn 的子进程。
pub struct CodexAppServer {
    instance_id: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    auth_home: PathBuf,
    user_agent: Option<String>,
    version: Option<String>,
    next_id: u64,
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<std::result::Result<Value, String>>>>>,
    messages: tokio::sync::mpsc::UnboundedReceiver<HelperMessage>,
    deferred_messages: VecDeque<HelperMessage>,
    /// 全新辅助进程最多可安全关联一次缺少 loginId 的完成通知：它只能属于首次登录请求。
    /// 后续无 ID 通知无法区分旧尝试迟到或重复，因此必须按不明事件处理。
    unattributed_login_completion_available: bool,
}

enum HelperMessage {
    Notification(Value),
    ServerRequest(Value),
}

/// A function call returned to the API client. The client owns execution and result handling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq)]
struct ClientToolResult {
    id: String,
    output: Vec<String>,
    is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct ToolCallReference {
    id: String,
    name: String,
    arguments: Value,
}

/// The verified subset of a caller request that maps to app-server's text and dynamic-function
/// interface. Unsupported API controls are rejected instead of being silently dropped.
#[derive(Default)]
struct CodexTurnRequest {
    base_instructions: Option<String>,
    developer_instructions: Option<String>,
    input: Vec<Value>,
    dynamic_tools: Vec<Value>,
    tool_calls: Vec<ToolCallReference>,
    tool_outputs: Vec<ClientToolResult>,
    has_assistant_history: bool,
    effort: Option<String>,
    summary: Option<String>,
    service_tier: Option<String>,
}

fn unsupported_generation_field(field: &str, reason: &str) -> anyhow::Error {
    anyhow::anyhow!("Codex app-server cannot preserve `{field}`: {reason}")
}

fn reject_unknown_fields(body: &Value, allowed: &[&str]) -> Result<()> {
    if let Some(object) = body.as_object() {
        if let Some(field) = object.keys().find(|field| !allowed.contains(&field.as_str())) {
            return Err(unsupported_generation_field(field, "the behavior has not been verified"));
        }
    } else {
        bail!("The request body must be an object");
    }
    Ok(())
}

fn text_content(value: &Value, field: &str) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    let Some(items) = value.as_array() else {
        return Err(unsupported_generation_field(field, "only text content is supported"));
    };
    let mut text = String::new();
    for item in items {
        reject_unknown_fields(item, &["type", "text"])?;
        if !matches!(item["type"].as_str(), Some("text" | "input_text" | "output_text")) {
            let kind = item["type"].as_str().unwrap_or("unknown content");
            return Err(unsupported_generation_field(field, kind));
        }
        let Some(part) = item["text"].as_str() else {
            bail!("Text content blocks must include a string `text` field");
        };
        text.push_str(part);
    }
    Ok(text)
}

fn append_instruction(target: &mut Option<String>, text: String) {
    if text.is_empty() {
        return;
    }
    match target {
        Some(previous) => {
            previous.push_str("\n\n");
            previous.push_str(&text);
        }
        None => *target = Some(text),
    }
}

fn parse_dynamic_tools(protocol: Protocol, body: &Value) -> Result<Vec<Value>> {
    let empty = vec![];
    let tools = match body.get("tools") {
        None | Some(Value::Null) => &empty,
        Some(value) => value.as_array().context("`tools` must be an array")?,
    };
    anyhow::ensure!(tools.len() <= 64, "Codex supports at most 64 client function tools per request");
    let mut result = Vec::with_capacity(tools.len());
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        let function = match protocol {
            Protocol::Chat => {
                reject_unknown_fields(tool, &["type", "function"])?;
                anyhow::ensure!(tool["type"] == "function", "Only client function tools are supported");
                reject_unknown_fields(&tool["function"], &["name", "description", "parameters", "strict"])?;
                &tool["function"]
            }
            Protocol::Responses => {
                reject_unknown_fields(tool, &["type", "name", "description", "parameters", "strict"])?;
                anyhow::ensure!(tool["type"] == "function", "Only Responses function tools are supported");
                tool
            }
            Protocol::Messages => {
                reject_unknown_fields(tool, &["name", "description", "input_schema"])?;
                tool
            }
        };
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
            .context("Function tool names must be 1–64 ASCII letters, digits, underscores, or hyphens")?;
        anyhow::ensure!(names.insert(name.to_owned()), "Function tool names must be unique");
        if protocol != Protocol::Messages {
            let strict = function.get("strict").and_then(Value::as_bool).unwrap_or(false);
            anyhow::ensure!(!strict, "Strict function schema enforcement is not supported by this Codex adapter");
        }
        let schema_key = if protocol == Protocol::Messages { "input_schema" } else { "parameters" };
        let schema = function.get(schema_key).cloned().unwrap_or_else(|| json!({"type":"object","properties":{}}));
        anyhow::ensure!(
            schema.is_object() && schema["type"] == "object",
            "Function tool schemas must be JSON Schema objects with `type: object`"
        );
        result.push(json!({
            "type":"function",
            "name":name,
            "description":function.get("description").and_then(Value::as_str).unwrap_or(""),
            "inputSchema":schema
        }));
    }
    result.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    let choice = body.get("tool_choice").filter(|choice| !choice.is_null());
    if let Some(choice) = choice {
        let auto = choice.as_str() == Some("auto") || choice.get("type").and_then(Value::as_str) == Some("auto");
        anyhow::ensure!(auto, "Only automatic client function selection is supported");
        if choice.get("disable_parallel_tool_use").and_then(Value::as_bool) == Some(true) {
            bail!("Disabling parallel tool calls is not supported by this Codex adapter");
        }
    }
    if body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false) {
        bail!("Disabling parallel tool calls is not supported by this Codex adapter");
    }
    Ok(result)
}

fn parse_call_reference(id: &Value, name: &Value, arguments: &Value) -> Result<ToolCallReference> {
    let id = id.as_str().filter(|id| !id.is_empty()).context("Tool calls require a non-empty call ID")?.to_owned();
    let name = name.as_str().filter(|name| !name.is_empty()).context("Tool calls require a non-empty function name")?.to_owned();
    let arguments = match arguments {
        Value::String(text) => serde_json::from_str::<Value>(text).context("Tool call arguments must be valid JSON")?,
        value => value.clone(),
    };
    anyhow::ensure!(arguments.is_object(), "Tool call arguments must be a JSON object");
    Ok(ToolCallReference { id, name, arguments })
}

fn parse_tool_result(id: &Value, output: &Value, is_error: bool) -> Result<ClientToolResult> {
    let id = id.as_str().filter(|id| !id.is_empty()).context("Tool results require a non-empty call ID")?.to_owned();
    let output = match output {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => {
            let mut text = Vec::with_capacity(items.len());
            for item in items {
                reject_unknown_fields(item, &["type", "text"])?;
                if !matches!(item["type"].as_str(), Some("text" | "input_text" | "output_text")) {
                    bail!("Only text tool results are supported by the Codex dynamic tool interface");
                }
                text.push(item["text"].as_str().context("Text tool result blocks require string `text`")?.to_owned());
            }
            text
        }
        _ => bail!("Tool results must be text or an array of text blocks"),
    };
    anyhow::ensure!(
        output.iter().map(String::len).sum::<usize>() <= MAX_GENERATION_INPUT_BYTES,
        "Tool result exceeds the 32 KiB request limit"
    );
    Ok(ClientToolResult { id, output, is_error })
}

fn optional_text_content(value: &Value, field: &str) -> Result<String> {
    if value.is_null() {
        Ok(String::new())
    } else {
        text_content(value, field)
    }
}

fn validate_tool_controls(body: &Value, protocol: Protocol) -> Result<Vec<Value>> {
    let tools = parse_dynamic_tools(protocol, body)?;
    let total = tools.iter().map(Value::to_string).map(|value| value.len()).sum::<usize>();
    anyhow::ensure!(total <= MAX_GENERATION_INPUT_BYTES, "Codex function schemas exceed the 32 KiB request limit");
    Ok(tools)
}

/// Convert text, client function schemas, and previously emitted tool result pairs. Prior tool
/// calls are only meaningful when the adapter still holds the matching live app-server turn.
fn codex_turn_request(protocol: Protocol, body: &Value) -> Result<CodexTurnRequest> {
    let mut request = CodexTurnRequest::default();
    request.dynamic_tools = validate_tool_controls(body, protocol)?;
    let input = |text: String| json!({"type":"text","text":text});
    match protocol {
        Protocol::Chat => {
            reject_unknown_fields(body, &["model", "stream", "messages", "reasoning_effort", "service_tier",
                    "tools",
                    "tool_choice",
                    "parallel_tool_calls",
                ],
            )?;
            if body.get("stream").is_some_and(|value| !value.is_boolean()) {
                bail!("`stream` must be a boolean");
            }
            if let Some(effort) = body.get("reasoning_effort") {
                request.effort = Some(effort.as_str().filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("`reasoning_effort` must be a non-empty string"))?.to_owned(),
                );
            }
            if let Some(tier) = body.get("service_tier") {
                request.service_tier = Some(tier.as_str().filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("`service_tier` must be a non-empty string"))?.to_owned());
            }
            let messages = body["messages"].as_array().context("Chat Completions requires a `messages` array")?;
            for message in messages {
                reject_unknown_fields(message, &["role", "content", "tool_calls", "tool_call_id"])?;
                let role = message["role"].as_str().context("Each message requires a string `role`")?;
                let content = optional_text_content(&message["content"], "messages[].content")?;
                match role {
                    "system" => append_instruction(&mut request.base_instructions, content),
                    "developer" => append_instruction(&mut request.developer_instructions, content),
                    "user" => request.input.push(input(content)),
                    "assistant" => {
                        request.has_assistant_history = true;
                        for call in message.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                            reject_unknown_fields(call, &["id", "type", "function"])?;
                            anyhow::ensure!(call["type"] == "function", "Only function tool-call history is supported");
                            reject_unknown_fields(&call["function"], &["name", "arguments"])?;
                            request.tool_calls.push(parse_call_reference(&call["id"], &call["function"]["name"], &call["function"]["arguments"])?);
                        }
                    }
                    "tool" => request.tool_outputs.push(parse_tool_result(&message["tool_call_id"], &message["content"], false)?),
                    _ => return Err(unsupported_generation_field("messages[].role", role)),
                }
            }
        }
        Protocol::Responses => {
            reject_unknown_fields(body, &["model", "stream", "input", "instructions", "reasoning", "service_tier",
                    "tools",
                    "tool_choice",
                    "parallel_tool_calls",
                ],
            )?;
            if body.get("stream").is_some_and(|value| !value.is_boolean()) {
                bail!("`stream` must be a boolean");
            }
            if let Some(instructions) = body.get("instructions") {
                let Some(instructions) = instructions.as_str() else {
                    return Err(unsupported_generation_field("instructions", "only a string is supported"));
                };
                append_instruction(&mut request.developer_instructions, instructions.to_owned());
            }
            if let Some(reasoning) = body.get("reasoning") {
                if !reasoning.is_object() {
                    bail!("`reasoning` must be an object");
                }
                if let Some(value) = reasoning.get("effort") {
                    request.effort = Some(value.as_str().filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("`reasoning.effort` must be a non-empty string"))?.to_owned());
                }
                if let Some(value) = reasoning.get("summary") {
                    request.summary = Some(value.as_str().filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("`reasoning.summary` must be a non-empty string"))?.to_owned(),
                    );
                }
                if let Some(field) = reasoning.as_object().and_then(|object| object.keys().find(|field| !["effort", "summary"].contains(&field.as_str()))) {
                    return Err(unsupported_generation_field(&format!("reasoning.{field}"), "the behavior has not been verified"));
                }
            }
            if let Some(tier) = body.get("service_tier") {
                request.service_tier = Some(tier.as_str().filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("`service_tier` must be a non-empty string"))?.to_owned());
            }
            match &body["input"] {
                Value::String(text) => request.input.push(input(text.clone())),
                Value::Array(items) => {
                    for item in items {
                        match item["type"].as_str().unwrap_or("message") {
                            "message" => {
                                let role = item["role"].as_str().context("Each Responses message requires a string `role`")?;
                                if role == "assistant" {
                                    reject_unknown_fields(item, &["type", "id", "status", "role", "content"])?;
                                    anyhow::ensure!(item["id"].as_str().is_some_and(|id| id.starts_with("msg_")), "Responses assistant output history requires its emitted message ID");
                                    anyhow::ensure!(item["status"] == "completed", "Only completed Responses assistant output history is supported");
                                    let content = item["content"].as_array().context("Responses assistant output history requires a content array")?;
                                    for part in content {
                                        reject_unknown_fields(part, &["type", "text", "annotations"])?;
                                        anyhow::ensure!(part["type"] == "output_text", "Only emitted Responses output_text history is supported");
                                        part["text"].as_str().context("Responses output_text history requires a string `text`")?;
                                        if let Some(annotations) = part.get("annotations") {
                                            anyhow::ensure!(annotations.as_array().is_some_and(Vec::is_empty), "Only empty Responses output_text annotations emitted by this adapter are supported");
                                        }
                                    }
                                    request.has_assistant_history = true;
                                } else {
                                    reject_unknown_fields(item, &["type", "role", "content"])?;
                                    let content = text_content(&item["content"], "input[].content")?;
                                    match role {
                                        "user" => request.input.push(input(content)),
                                        "system" => append_instruction(&mut request.base_instructions, content),
                                        "developer" => append_instruction(&mut request.developer_instructions, content),
                                        _ => return Err(unsupported_generation_field("input[].role", role)),
                                    }
                                }
                            }
                            "function_call" => {
                                reject_unknown_fields(item, &["type", "id", "call_id", "name", "arguments", "status"])?;
                                anyhow::ensure!(item["id"].as_str().is_some_and(|id| id.starts_with("fc_")), "Responses function-call history requires its emitted item ID");
                                anyhow::ensure!(item["status"] == "completed", "Only completed Responses function-call history is supported");
                                request.has_assistant_history = true;
                                request.tool_calls.push(parse_call_reference(&item["call_id"], &item["name"], &item["arguments"])?);
                }
                            "function_call_output" => {
                                reject_unknown_fields(item, &["type", "id", "call_id", "output"])?;
                                request.tool_outputs.push(parse_tool_result(&item["call_id"], &item["output"], false)?);
                            }
                            kind => return Err(unsupported_generation_field("input[].type", kind)),
                        }
                    }
                }
                _ => bail!("Responses requires text `input`"),
            }
        }
        Protocol::Messages => {
            reject_unknown_fields(body, &["model", "stream", "system", "messages", "tools", "tool_choice", "parallel_tool_calls"])?;
            if body.get("stream").is_some_and(|value| !value.is_boolean()) {
                bail!("`stream` must be a boolean");
            }
            if body.get("system").is_some_and(|value| !value.is_null()) {
                append_instruction(&mut request.base_instructions, text_content(&body["system"], "system")?);
            }
            let messages = body["messages"].as_array().context("Messages requires a `messages` array")?;
            for message in messages {
                reject_unknown_fields(message, &["role", "content"])?;
                let role = message["role"].as_str().context("Each message requires a string `role`")?;
                if let Some(text) = message["content"].as_str() {
                    match role {
                        "user" => request.input.push(input(text.to_owned())),
                        "assistant" => request.has_assistant_history = true,
                        _ => return Err(unsupported_generation_field("messages[].role", role)),
                    }
                    continue;
                }
                let content = message["content"].as_array().context("Messages content must be a string or an array of blocks")?;
                match role {
                    "user" => {
                        let mut user_text = Vec::new();
                        for block in content {
                            match block["type"].as_str() {
                                Some("tool_result") => {
                                    reject_unknown_fields(block, &["type", "tool_use_id", "content", "is_error"])?;
                                    let is_error = block.get("is_error").map(|value| value.as_bool().context("`is_error` must be a boolean")).transpose()?.unwrap_or(false);
                                    request.tool_outputs.push(parse_tool_result(&block["tool_use_id"], &block["content"], is_error)?);
                                }
                                Some("text") => {
                                    reject_unknown_fields(block, &["type", "text"])?;
                                    user_text.push(block["text"].as_str().context("Text blocks require a string `text`")?.to_owned());
                                }
                                kind => return Err(unsupported_generation_field("messages[].content[].type", kind.unwrap_or("unknown"))),
                            }
                        }
                        if !user_text.is_empty() {
                            request.input.push(input(user_text.join("")));
                        }
                    }
                    "assistant" => {
                        request.has_assistant_history = true;
                        for block in content {
                            match block["type"].as_str() {
                                Some("tool_use") => {
                                    reject_unknown_fields(block, &["type", "id", "name", "input"])?;
                                    request.tool_calls.push(parse_call_reference(&block["id"], &block["name"], &block["input"])?);
                                }
                                Some("text") => {
                                    reject_unknown_fields(block, &["type", "text"])?;
                                    block["text"].as_str().context("Text blocks require a string `text`")?;
                                }
                                kind => return Err(unsupported_generation_field("messages[].content[].type", kind.unwrap_or("unknown"))),
                            }
                        }
                    }
                    _ => return Err(unsupported_generation_field("messages[].role", role)),
                }
            }
        }
    }
    if request.input.is_empty() && request.tool_outputs.is_empty() {
        bail!("Codex app-server requires a text user message or a client tool result");
    }
    Ok(request)
}

/// Validate the same strict text subset before the gateway creates an HTTP response. The
/// adapter repeats conversion at its own boundary so direct callers cannot bypass it.
pub(crate) fn validate_generation_request(protocol: Protocol, body: &Value, generation: u64) -> Result<()> {
    let parsed = codex_turn_request(protocol, body)?;
    for result in &parsed.tool_outputs {
        anyhow::ensure!(
            result.id.starts_with(&format!("call_ajv1_{generation}_")),
            "Tool result belongs to a stale or unknown Codex connection generation"
        );
    }
    for call in &parsed.tool_calls {
        anyhow::ensure!(
            call.id.starts_with(&format!("call_ajv1_{generation}_")),
            "Tool call history belongs to a stale or unknown Codex connection generation"
        );
    }
    Ok(())
}

fn validate_tool_followup(pending: &mut PendingToolTurn, request: &CodexTurnRequest) -> Result<Vec<(Value, Value)>> {
    anyhow::ensure!(request.has_assistant_history, "A Codex tool continuation must include the emitted assistant tool-call history");
    anyhow::ensure!(
        request.dynamic_tools == pending.dynamic_tools,
        "The function schemas changed while a Codex tool call was pending; the turn was discarded"
    );
    let generation_prefix = format!("call_ajv1_{}_", pending.generation);
    let mut seen_calls = std::collections::HashSet::new();
    for call in &request.tool_calls {
        anyhow::ensure!(
            call.id.starts_with(&generation_prefix),
            "Tool call history belongs to a stale connection generation"
        );
        anyhow::ensure!(seen_calls.insert(call.id.as_str()), "Tool call history contains a duplicate call ID");
        let Some(issued) = pending.issued_tool_calls.get(&call.id) else {
            bail!("Tool call history does not match a call issued by the current Codex turn");
        };
        anyhow::ensure!(
            issued.name == call.name && issued.arguments == call.arguments,
            "Tool call history was changed after Codex issued the call"
        );
    }
    let issued_calls: std::collections::HashSet<_> = pending.issued_tool_calls.keys().map(String::as_str).collect();
    anyhow::ensure!(seen_calls == issued_calls, "The assistant tool-call history is incomplete or contains calls from another turn");

    let mut supplied = HashMap::<&str, &ClientToolResult>::new();
    for output in &request.tool_outputs {
        anyhow::ensure!(output.id.starts_with(&generation_prefix), "Tool result belongs to a stale connection generation");
        anyhow::ensure!(supplied.insert(&output.id, output).is_none(), "A tool result was repeated in the request");
        anyhow::ensure!(
            pending.issued_tool_calls.contains_key(&output.id),
            "Tool result does not match a call issued by the current Codex turn"
        );
    }
    let expected: std::collections::HashSet<_> = pending.issued_tool_calls.iter().filter_map(|(id, call)| (!call.answered).then_some(id.as_str())).collect();
    let supplied_pending: std::collections::HashSet<_> = supplied.keys().copied().filter(|id| pending.issued_tool_calls.get(*id).is_some_and(|call| !call.answered)).collect();
    anyhow::ensure!(!expected.is_empty(), "The Codex turn has no pending client tool calls");
    anyhow::ensure!(
        expected == supplied_pending,
        "The tool results were incomplete or repeated; the Codex turn was discarded and calls will not be replayed"
    );

    let mut responses = Vec::with_capacity(expected.len());
    for (id, output) in supplied {
        let issued = pending.issued_tool_calls.get_mut(id).expect("the result was validated above");
        if issued.answered {
            continue;
        }
        let content_items: Vec<_> = output.output.iter().map(|text| json!({"type":"inputText","text":text})).collect();
        let result = json!({"success":!output.is_error,"contentItems":content_items});
        responses.push((issued.request_id.clone(), result));
        issued.answered = true;
    }
    Ok(responses)
}

fn client_tool_calls_from_requests(
    requests: Vec<Value>,
    thread_id: &str,
    turn_id: &str,
    generation: u64,
    dynamic_tools: &[Value],
    issued: &mut HashMap<String, IssuedToolCall>,
) -> Result<Vec<ClientToolCall>> {
    anyhow::ensure!(
        !requests.is_empty() && requests.len() <= 64,
        "Codex returned an empty or oversized dynamic tool batch"
    );
    let mut calls = Vec::with_capacity(requests.len());
    let mut internal_ids = std::collections::HashSet::new();
    let mut rpc_ids = std::collections::HashSet::new();
    let mut output_bytes = 0usize;
    for request in requests {
        anyhow::ensure!(request["method"] == "item/tool/call", "Codex requested an unsupported helper-side operation");
        let params = &request["params"];
        anyhow::ensure!(
            params["threadId"].as_str() == Some(thread_id) && params["turnId"].as_str() == Some(turn_id),
            "Codex returned a dynamic tool call for a different turn"
        );
        let internal_call_id = params["callId"].as_str().filter(|id| !id.is_empty()).context("Codex dynamic tool request omitted its call ID")?.to_owned();
        anyhow::ensure!(internal_ids.insert(internal_call_id.clone()), "Codex repeated a dynamic tool call ID");
        anyhow::ensure!(
            issued.values().all(|call| call.internal_call_id != internal_call_id),
            "Codex repeated a dynamic tool call ID in the same turn"
        );
        let name = params["tool"].as_str().filter(|name| !name.is_empty()).context("Codex dynamic tool request omitted its function name")?.to_owned();
        anyhow::ensure!(
            dynamic_tools.iter().any(|tool| tool["name"].as_str() == Some(name.as_str())),
            "Codex requested a function that the API client did not provide"
        );
        let arguments = params.get("arguments").cloned().context("Codex dynamic tool request omitted arguments")?;
        anyhow::ensure!(arguments.is_object(), "Codex function arguments must be a JSON object");
        let id = format!("call_ajv1_{generation}_{}", uuid::Uuid::new_v4().simple());
        let arguments_text = arguments.to_string();
        let rpc_id = request.get("id").cloned().context("Codex dynamic tool request omitted its JSON-RPC id")?;
        anyhow::ensure!(
            rpc_id.as_str().is_some_and(|id| !id.is_empty()) || rpc_id.is_number(),
            "Codex dynamic tool request returned an invalid JSON-RPC id"
        );
        anyhow::ensure!(
            rpc_ids.insert(rpc_id.clone()) && issued.values().all(|call| &call.request_id != &rpc_id),
            "Codex repeated a dynamic tool JSON-RPC id"
        );
        output_bytes = output_bytes.saturating_add(id.len()).saturating_add(name.len()).saturating_add(arguments_text.len());
        anyhow::ensure!(
            output_bytes <= MAX_TOOL_CALL_OUTPUT_BYTES,
            "Codex function-call output exceeds the 16 MiB response limit"
        );
        anyhow::ensure!(
            issued
                .insert(
                    id.clone(),
                    IssuedToolCall { request_id: rpc_id, internal_call_id, name: name.clone(), arguments, answered: false }
                )
                .is_none(),
            "Codex generated a duplicate API call ID"
        );
        calls.push(ClientToolCall { id, name, arguments: arguments_text });
    }
    Ok(calls)
}

impl CodexAppServer {
    /// 启动辅助进程并完成 `initialize` 握手。子进程的 `CODEX_HOME` 只能是专用目录。
    pub async fn start(provider_id: &str, launch: HelperLaunch) -> Result<CodexAppServer> {
        Self::start_with_home(helper_home(provider_id), launch).await
    }

    /// 测试专用：把专用目录根换成临时目录，避免碰真实用户主目录。
    #[cfg(test)]
    pub async fn start_in(home_root: &Path, provider_id: &str, launch: HelperLaunch) -> Result<CodexAppServer> {
        Self::start_with_home(helper_home_in(home_root, provider_id), launch).await
    }

    async fn start_with_home(auth_home: PathBuf, launch: HelperLaunch) -> Result<CodexAppServer> {
        prepare_auth_home(&auth_home)?;
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .env("CODEX_HOME", &auth_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in CREDENTIAL_ENV {
            command.env_remove(name);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("Start the Codex helper {}", launch.program.display()))?;
        let stdin = child.stdin.take().context("The Codex helper has no stdin")?;
        let stdout = child.stdout.take().context("The Codex helper has no stdout")?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (sender, messages) = tokio::sync::mpsc::unbounded_channel();
        spawn_reader(stdout, pending.clone(), sender);
        let mut server = CodexAppServer {
            instance_id: uuid::Uuid::new_v4().to_string(),
            child: Some(child),
            stdin: Some(stdin),
            auth_home,
            user_agent: None,
            version: None,
            next_id: 0,
            pending,
            messages,
            deferred_messages: VecDeque::new(),
            unattributed_login_completion_available: true,
        };
        // 官方 app-server 握手：`initialize` 带客户端自述，收到响应后再发一条无 id 的 `initialized` 通知。
        let client_info = json!({
            "name": CLIENT_INFO_NAME,
            "title": CLIENT_INFO_TITLE,
            "version": env!("CARGO_PKG_VERSION"),
        });
        let initialized = server.call("initialize", json!({ "clientInfo": client_info, "capabilities": {"experimentalApi": true} })).await?;
        (server.user_agent, server.version) = helper_self_report(&initialized);
        server.notify("initialized", json!({})).await?;
        Ok(server)
    }

    /// 发送一次请求并等待同 id 的响应；错误信息整体脱敏。
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        {
            let stdin = self.stdin.as_mut().context("The Codex helper is not running")?;
            let mut line = serde_json::to_vec(&request)?;
            line.push(b'\n');
            if let Err(error) = stdin.write_all(&line).and_then(|_| stdin.flush()) {
                self.pending.lock().unwrap().remove(&id);
                bail!("Write to the Codex helper failed: {error}");
            }
        }
        let response = match tokio::time::timeout(CALL_TIMEOUT, receiver).await {
            Ok(Ok(Ok(value))) => value,
            Ok(Ok(Err(message))) => bail!("{message}"),
            Ok(Err(_)) => bail!("The Codex helper exited before answering {method}"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                bail!("The Codex helper did not answer {method} in time");
            }
        };
        if let Some(error) = response.get("error") {
            let sanitized = redact_json(error);
            let message = sanitized
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown helper error");
            bail!("Codex helper rejected {method}: {message}");
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// 发送一条无 id 的通知（例如 `initialized`）：不等响应，只确认写进了子进程 stdin。
    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let stdin = self.stdin.as_mut().context("The Codex helper is not running")?;
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .and_then(|_| stdin.flush())
            .with_context(|| format!("Notify the Codex helper {method} failed"))?;
        Ok(())
    }

    /// 下一条通知（无 id 的消息）；辅助进程 stdout 关闭时返回 `None`。
    pub async fn next_notification(&mut self) -> Option<Value> {
        loop {
            match self.messages.recv().await? {
                HelperMessage::Notification(value) => return Some(value),
                request @ HelperMessage::ServerRequest(_) => self.deferred_messages.push_back(request),
            }
        }
    }

    async fn next_message(&mut self) -> Option<HelperMessage> {
        if let Some(message) = self.deferred_messages.pop_front() {
            return Some(message);
        }
        self.messages.recv().await
    }

    async fn collect_server_requests(&mut self, first: Value) -> Vec<Value> {
        let mut requests = vec![first];
        // The app-server emits one request per client tool call. Allow the reader a brief quiet
        // interval so a parallel batch is returned as one API tool_calls result.
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(20), self.next_message()).await {
                Ok(Some(HelperMessage::ServerRequest(request))) => requests.push(request),
                Ok(Some(message)) => {
                    self.deferred_messages.push_front(message);
                    break;
                }
                Ok(None) | Err(_) => break,
            }
        }
        requests
    }

    pub async fn respond_server_request(&mut self, id: Value, result: Value) -> Result<()> {
        let response = json!({"jsonrpc":"2.0","id":id,"result":result});
        let stdin = self.stdin.as_mut().context("The Codex helper is not running")?;
        let mut line = serde_json::to_vec(&response)?;
        line.push(b'\n');
        stdin.write_all(&line).and_then(|_| stdin.flush()).context("Write the client-owned tool result to the Codex helper")?;
        Ok(())
    }

    pub fn auth_home(&self) -> &Path {
        &self.auth_home
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// 只终止并回收自己 spawn 的子进程：kill + wait，绝不广域杀进程。
    pub fn shutdown(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn prepare_auth_home(auth_home: &Path) -> Result<()> {
    std::fs::create_dir_all(auth_home)
        .with_context(|| format!("Create the dedicated Codex helper directory {}", auth_home.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(auth_home, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// 读取线程：响应按 id 唤醒等待者，其余消息进入通知队列；stdout 关闭时叫醒全部等待者。
fn spawn_reader(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<std::result::Result<Value, String>>>>>,
    messages: tokio::sync::mpsc::UnboundedSender<HelperMessage>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue;
            };
            if value.get("method").is_some() && value.get("id").is_some() {
                if messages.send(HelperMessage::ServerRequest(value)).is_err() {
                    break;
                }
                continue;
            }
            match value.get("id").and_then(Value::as_u64) {
                Some(id) => {
                    if let Some(sender) = pending.lock().unwrap().remove(&id) {
                        let _ = sender.send(Ok(value));
                    }
                }
                None => {
                    if messages.send(HelperMessage::Notification(value)).is_err() {
                        break;
                    }
                }
            }
        }
        for (_, sender) in pending.lock().unwrap().drain() {
            let _ = sender.send(Err("The Codex helper stopped responding".to_owned()));
        }
    });
}

/// 等待 `account/login/completed` 的上限；超时按失败处理，不无限挂起。
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
/// 轮询通知的切片长度：等待完成通知时不长期独占服务商槽位，cancel/logout 仍能拿到锁。
const POLL_SLICE: std::time::Duration = std::time::Duration::from_millis(120);

#[derive(Clone)]
struct ActiveLogin {
    login_id: String,
    generation: u64,
    allow_unattributed_completion: bool,
}

#[derive(Clone, Default)]
struct AdapterState {
    user_agent: Option<String>,
    version: Option<String>,
    auth_home: Option<String>,
}

const GENERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
const TURN_COMPLETION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_GENERATION_INPUT_BYTES: usize = 32 * 1024;
const MAX_TOOL_CALL_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

type GenerationGuardCell = Arc<Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TurnCancelStatus {
    Running,
    Requested,
    Complete,
}

#[derive(Clone)]
struct ActiveTurnControl {
    request_id: String,
    cancel: tokio::sync::watch::Sender<TurnCancelStatus>,
    helper_instance_id: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    armed: Arc<std::sync::atomic::AtomicBool>,
    generation_guard: GenerationGuardCell,
}

struct ActiveTurnRegistration {
    active_turns: Arc<Mutex<HashMap<String, ActiveTurnControl>>>,
    provider_id: String,
    request_id: String,
}

impl ActiveTurnRegistration {
    fn update(&self, update: impl FnOnce(&mut ActiveTurnControl)) {
        let mut active_turns = self.active_turns.lock().unwrap();
        if let Some(active) = active_turns.get_mut(&self.provider_id) {
            if active.request_id == self.request_id {
                update(active);
            }
        }
    }

    fn set_helper_instance(&self, instance_id: String) {
        self.update(|active| active.helper_instance_id = Some(instance_id));
    }

    fn set_thread(&self, thread_id: String) {
        self.update(|active| active.thread_id = Some(thread_id));
    }

    fn set_turn(&self, turn_id: String) {
        self.update(|active| active.turn_id = Some(turn_id));
    }
}

impl Drop for ActiveTurnRegistration {
    fn drop(&mut self) {
        let mut active_turns = self.active_turns.lock().unwrap();
        if active_turns
            .get(&self.provider_id)
            .is_some_and(|active| active.request_id == self.request_id)
        {
            active_turns.remove(&self.provider_id);
        }
    }
}

#[cfg(test)]
struct CancellationCleanupGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

struct TurnCancellation {
    servers: Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>,
    provider_id: String,
    helper_instance_id: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    workspace: Option<PathBuf>,
    armed: Arc<std::sync::atomic::AtomicBool>,
    generation_guard: GenerationGuardCell,
    active_registration: ActiveTurnRegistration,
    #[cfg(test)]
    cleanup_gate: Option<CancellationCleanupGate>,
}

impl TurnCancellation {
    fn disarm_and_release(&mut self) {
        self.armed.store(false, std::sync::atomic::Ordering::SeqCst);
        self.generation_guard.lock().unwrap().take();
    }

    fn release_for_tool_handoff(&mut self) -> Option<PathBuf> {
        self.armed.store(false, std::sync::atomic::Ordering::SeqCst);
        self.generation_guard.lock().unwrap().take();
        self.workspace.take()
    }
}

impl Drop for TurnCancellation {
    fn drop(&mut self) {
        let Some(workspace) = self.workspace.take() else { return;
        };
        let armed = self.armed.swap(false, std::sync::atomic::Ordering::SeqCst);
        if !armed {
            self.generation_guard.lock().unwrap().take();
            let _ = std::fs::remove_dir_all(workspace);
            return;
        }
        let servers = self.servers.clone();
        let provider_id = self.provider_id.clone();
        let helper_instance_id = self.helper_instance_id.clone();
        let thread_id = self.thread_id.clone();
        let turn_id = self.turn_id.clone();
        let generation_guard = self.generation_guard.lock().unwrap().take();
        #[cfg(test)]
        let cleanup_gate = self.cleanup_gate.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            // Before turn/start returns an id, interrupt cannot address the accepted work. Reap
            // this app-owned helper instead. Transfer the generation guard into this task so the
            // provider cannot reuse the helper until its exact instance has been interrupted or
            // reaped. Keep the workspace until cleanup completes.
            runtime.spawn(async move {
                let _generation_guard = generation_guard;
                #[cfg(test)]
                if let Some(gate) = cleanup_gate {
                    let _ = gate.started.send(());
                    let _ = gate.release.await;
                }
                if let Some(helper_instance_id) = helper_instance_id.as_deref() {
                    match (thread_id, turn_id) {
                        (Some(thread_id), Some(turn_id)) => {
                            stop_active_turn(&servers, &provider_id, helper_instance_id, &thread_id, &turn_id).await;
                        }
                        _ => {
                            reap_owned_helper(&servers, &provider_id, helper_instance_id).await;
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(workspace);
            });
        } else {
            if let (Some(helper_instance_id), Ok(mut servers)) = (self.helper_instance_id.as_deref(), self.servers.try_lock()) {
                if servers.get(&self.provider_id).is_some_and(|server| server.instance_id() == helper_instance_id) {
                    if let Some(mut server) = servers.remove(&self.provider_id) {
                        server.shutdown();
                    }
                }
            }
            drop(generation_guard);
            let _ = std::fs::remove_dir_all(workspace);
        }
    }
}

struct GenerationState {
    servers: Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>,
    provider_id: String,
    model_id: String,
    protocol: Protocol,
    thread_id: String,
    turn_id: String,
    generation: u64,
    dynamic_tools: Vec<Value>,
    request_context: Value,
    issued_tool_calls: HashMap<String, IssuedToolCall>,
    pending_tool_turns: Arc<Mutex<HashMap<String, PendingToolTurn>>>,
    pending_tool_cleanup: Arc<PendingToolCleanup>,
    generation_lock: Arc<tokio::sync::Mutex<()>>,
    started: bool,
    terminal: bool,
    deadline: tokio::time::Instant,
    cancel: tokio::sync::watch::Receiver<TurnCancelStatus>,
    cancellation: TurnCancellation,
}

#[derive(Clone, Debug)]
struct IssuedToolCall {
    request_id: Value,
    internal_call_id: String,
    name: String,
    arguments: Value,
    answered: bool,
}

#[derive(Clone)]
struct PendingToolTurn {
    generation: u64,
    model_id: String,
    protocol: Protocol,
    helper_instance_id: String,
    thread_id: String,
    turn_id: String,
    workspace: PathBuf,
    dynamic_tools: Vec<Value>,
    request_context: Value,
    issued_tool_calls: HashMap<String, IssuedToolCall>,
    deadline: tokio::time::Instant,
}

#[derive(Default)]
struct PendingToolCleanup {
    // Body drop removes a pending turn synchronously, but its helper still belongs to
    // cleanup until interrupt completion or helper reaping has finished.
    providers: Mutex<HashSet<String>>,
    changed: tokio::sync::Notify,
}

impl PendingToolCleanup {
    fn begin(&self, provider_id: &str) {
        self.providers.lock().unwrap().insert(provider_id.to_owned());
    }

    fn is_active(&self, provider_id: &str) -> bool {
        self.providers.lock().unwrap().contains(provider_id)
    }

    fn finish(&self, provider_id: &str) {
        self.providers.lock().unwrap().remove(provider_id);
        self.changed.notify_waiters();
    }

    async fn wait_until_clear(&self, provider_id: &str) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_active(provider_id) {
                return;
            }
            notified.await;
        }
    }
}

fn request_context(request: &CodexTurnRequest) -> Value {
    json!({
        "baseInstructions":request.base_instructions,
        "developerInstructions":request.developer_instructions,
        "input":request.input,
        "dynamicTools":request.dynamic_tools,
        "effort":request.effort,
        "summary":request.summary,
        "serviceTier":request.service_tier
    })
}

fn pending_matches_request(pending: &PendingToolTurn, model_id: &str, protocol: Protocol, request: &CodexTurnRequest) -> bool {
    if pending.model_id != model_id || pending.protocol != protocol || pending.request_context != request_context(request) || request.tool_outputs.is_empty() {
        return false;
    }
    request.tool_calls.iter().any(|call| pending.issued_tool_calls.contains_key(&call.id))
        || request.tool_outputs.iter().any(|result| pending.issued_tool_calls.contains_key(&result.id))
}

fn generation_workspace() -> Result<PathBuf> {
    let workspace = std::env::temp_dir().join(format!("autojev-codex-turn-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&workspace).context("Create an isolated Codex request workspace")?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(workspace)
}

/// 真实订阅适配器：每个订阅服务商一个由本应用管理的官方 Codex 辅助进程。
/// 网关只会在共享准入通过后交接文本生成；本适配器仍拥有连接状态与账号隔离。
pub struct CodexAdapter {
    servers: Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>,
    generation_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    active_turns: Arc<Mutex<HashMap<String, ActiveTurnControl>>>,
    logins: Mutex<HashMap<String, ActiveLogin>>,
    /// 已被某个等待者取走、但属于另一次尝试的完成通知；由对应等待者领回，避免串线丢失。
    deferred: Mutex<HashMap<String, VecDeque<(String, Value)>>>,
    pending_tool_turns: Arc<Mutex<HashMap<String, PendingToolTurn>>>,
    pending_tool_cleanup: Arc<PendingToolCleanup>,
    state: Mutex<AdapterState>,
    /// 仅在测试构建里可显式指定；生产只能走 [`resolve_launch`]。
    launch: Option<HelperLaunch>,
    /// 仅在测试构建里可替换专用目录根；生产永远是用户主目录。
    #[cfg_attr(not(test), allow(dead_code))]
    home_root: Option<PathBuf>,
    #[cfg(test)]
    cleanup_gate: Mutex<Option<CancellationCleanupGate>>,
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            generation_locks: Mutex::new(HashMap::new()),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
            logins: Mutex::new(HashMap::new()),
            deferred: Mutex::new(HashMap::new()),
            pending_tool_turns: Arc::new(Mutex::new(HashMap::new())),
            pending_tool_cleanup: Arc::new(PendingToolCleanup::default()),
            state: Mutex::new(AdapterState::default()),
            launch: None,
            home_root: None,
            #[cfg(test)]
            cleanup_gate: Mutex::new(None),
        }
    }

    /// 测试专用：直接指定替身可执行文件与专用目录根。生产代码里没有这个入口。
    #[cfg(test)]
    pub fn with_launch(home_root: PathBuf, launch: HelperLaunch) -> Self {
        let mut adapter = Self::new();
        adapter.launch = Some(launch);
        adapter.home_root = Some(home_root);
        adapter
    }

    /// 该服务商的专用授权目录；测试可换根，生产永远是用户主目录下的自有路径。
    pub(crate) fn auth_home_for(&self, provider_id: &str) -> PathBuf {
        match &self.home_root {
            Some(root) => helper_home_in(root, provider_id),
            None => helper_home(provider_id),
        }
    }

    /// 惰性解析官方 `codex`：解析失败返回可读错误，绝不 panic。
    fn launch(&self) -> Result<HelperLaunch> {
        match &self.launch {
            Some(launch) => Ok(launch.clone()),
            None => resolve_launch(),
        }
    }

    fn generation_lock(&self, provider_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.generation_locks
            .lock()
            .unwrap()
            .entry(provider_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// 保证该服务商有一个已握手的辅助进程，并把整个会话交给调用方使用。
    async fn servers(
        &self) -> tokio::sync::MutexGuard<'_, HashMap<String, CodexAppServer>> {
        self.servers.lock().await
    }

    async fn server_for(
        &self,
        provider_id: &str) -> Result<tokio::sync::MutexGuard<'_, HashMap<String, CodexAppServer>>> {
        let mut servers = self.servers.lock().await;
        if !servers.contains_key(provider_id) {
            let launch = self.launch()?;
            let server = match &self.home_root {
                #[cfg(test)]
                Some(root) => CodexAppServer::start_in(root, provider_id, launch).await?,
                _ => CodexAppServer::start(provider_id, launch).await?,
            };
            *self.state.lock().unwrap() = AdapterState {
                user_agent: server.user_agent().map(str::to_owned),
                version: server.version().map(str::to_owned),
                auth_home: Some(server.auth_home().to_string_lossy().into_owned()),
            };
            servers.insert(provider_id.to_owned(), server);
        }
        Ok(servers)
    }

    /// 结束一次登录尝试：只清理仍属于它的挂起记录，然后用 `account/read` 核实身份。
    async fn complete_login(
        &self,
        provider_id: &str,
        expected_login_id: &str,
        params: Value,
    ) -> Result<Option<LoginResult>> {
        let notification_login_id = match params.get("loginId") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str().unwrap_or_default()),
        };
        {
            let mut logins = self.logins.lock().unwrap();
            let Some(active) = logins
                .get(provider_id)
                .cloned()
                .filter(|active| active.login_id == expected_login_id)
            else {
                return Ok(None);
            };
            if notification_login_id.is_some_and(|login_id| login_id != expected_login_id) {
                return Ok(None);
            }
            // Helper notifications have no per-request channel. After the fresh helper's one
            // initial attempt, an idless event could be late or duplicated; ignore it without
            // consuming the replacement attempt so its matching loginId can still complete.
            if notification_login_id.is_none() && !active.allow_unattributed_completion {
                return Ok(None);
            }
            logins.remove(provider_id);
        }
        let Some(success) = params.get("success").and_then(Value::as_bool) else {
            return Ok(Some(LoginResult::Failed(
                "The Codex helper returned an invalid sign-in completion response".to_owned(),
            )));
        };
        if !success {
            let error = params
                .get("error")
                .and_then(Value::as_str)
                .map(redact)
                .unwrap_or_else(|| "The Codex helper reported a failed sign-in".to_owned());
            return Ok(Some(LoginResult::Failed(error)));
        }
        // 完成通知不足以激活账号：必须再用 account/read 核实返回的身份。
        let mut servers = self.servers.lock().await;
        let Some(server) = servers.get_mut(provider_id) else {
            return Ok(Some(LoginResult::Failed(
                "The Codex helper exited before the account could be verified".to_owned(),
            )));
        };
        let verified = server.call("account/read", json!({})).await;
        let identity = verified
            .as_ref()
            .ok()
            .filter(|result| result["account"]["type"].as_str() == Some("chatgpt"))
            .and_then(|result| {
                let (state, identity, _) = account_status(result);
                (state == ConnectionState::Connected)
                    .then_some(identity)
                    .flatten()
            });
        let Some(identity) = identity else {
            return Ok(Some(LoginResult::Failed(
                "The Codex helper did not confirm a signed-in account".to_owned(),
            )));
        };
        Ok(Some(LoginResult::Completed { identity }))
    }
}

/// 只处理 `account/login/completed`：官方协议允许 loginId 缺失或为 null。
fn login_completion(value: &Value) -> Option<(Option<String>, Value)> {
    if value.get("method").and_then(Value::as_str) != Some("account/login/completed") {
        return None;
    }
    let params = value.get("params")?.clone();
    let login_id = match params.get("loginId") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str()?.to_owned()),
    };
    Some((login_id, params))
}

fn defer(deferred: &Mutex<HashMap<String, VecDeque<(String, Value)>>>, provider_id: &str, login_id: &str, params: Value) {
    deferred
        .lock()
        .unwrap()
        .entry(provider_id.to_owned())
        .or_default()
        .push_back((login_id.to_owned(), params));
}

fn take_deferred(deferred: &Mutex<HashMap<String, VecDeque<(String, Value)>>>, provider_id: &str, login_id: &str) -> Option<Value> {
    let mut store = deferred.lock().unwrap();
    let queue = store.get_mut(provider_id)?;
    let index = queue.iter().position(|(id, _)| id == login_id)?;
    let (_, params) = queue.remove(index)?;
    if queue.is_empty() {
        store.remove(provider_id);
    }
    Some(params)
}

impl SubscriptionAdapter for CodexAdapter {
    fn available(&self) -> bool {
        self.launch().is_ok()
    }

    /// 这个适配器只承载官方 Codex；Grok 等其它订阅服务商不得借用它的进程或身份。
    fn supports(&self, kind: &ProviderKind) -> bool {
        matches!(kind, ProviderKind::CodexSubscription)
    }

    fn helper_status(&self) -> HelperStatus {
        let state = self.state.lock().unwrap().clone();
        HelperStatus {
            available: self.launch().is_ok(),
            user_agent: state.user_agent,
            version: state.version,
            auth_home: state.auth_home,
        }
    }

    fn status<'a>(&'a self, provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<ConnectionStatus>> {
        Box::pin(async move {
            let mut servers = self.server_for(provider_id).await?;
            let server = servers.get_mut(provider_id).expect("the helper was just started");
            let result = server.call("account/read", json!({})).await?;
            let (state, identity, identity_incomplete) = account_status(&result);
            Ok(ConnectionStatus {
                state,
                identity,
                identity_incomplete,
                helper_version: server.version().map(str::to_owned),
                account_path: Some(server.auth_home().to_string_lossy().into_owned()),
            })
        })
    }

    /// 模型目录：`model/list`。
    ///
    /// 发现 ≠ 资格：契约 A 固定了 `eligible` 一律为 `false`。已核实的固定版本响应里条目字段是
    /// `id`/`model`/`displayName`/`hidden`/`isDefault`/`availableAccessPrograms`，**没有任何资格
    /// 布尔字段**；资格由 #17 定义，本票不得把发现的模型标成可调用。
    fn models<'a>(&'a self, provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<CatalogRead>> {
        Box::pin(async move {
            let mut servers = self.server_for(provider_id).await?;
            let server = servers.get_mut(provider_id).expect("the helper was just started");
            let result = server.call(CATALOG_METHOD, json!({})).await?;
            parse_catalog(&result)
        })
    }

    /// 额度：`account/rateLimits/read`。只读映射：不换算金额、不补默认值、不截断越界数字。
    fn quota<'a>(&'a self, provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<QuotaEvidence>> {
        Box::pin(async move {
            let mut servers = self.server_for(provider_id).await?;
            let server = servers.get_mut(provider_id).expect("the helper was just started");
            let result = server.call(QUOTA_METHOD, json!({})).await?;
            Ok(parse_quota(&result))
        })
    }

    fn start_login<'a>(
        &'a self,
        provider_id: &'a str,
        generation: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<LoginStart>> {
        Box::pin(async move {
            let mut servers = self.server_for(provider_id).await?;
            let server = servers
                .get_mut(provider_id)
                .expect("the helper was just started");
            let allow_unattributed_completion = server.unattributed_login_completion_available;
            // 先消耗首次无 ID 通知关联窗口。RPC 超时或取消后，迟到通知不得绑定后续尝试。
            server.unattributed_login_completion_available = false;
            let result = server
                .call("account/login/start", json!({"type": "chatgpt"}))
                .await?;
            anyhow::ensure!(
                result.get("type").and_then(Value::as_str) == Some("chatgpt"),
                "The Codex helper did not return a browser sign-in response"
            );
            let login_id = result
                .get("loginId")
                .and_then(Value::as_str)
                .filter(|login_id| !login_id.is_empty())
                .context("The Codex helper did not return a login id")?
                .to_owned();
            let authorization_url = result
                .get("authUrl")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
                .context("The Codex helper did not return an authorization URL")?
                .to_owned();
            self.logins.lock().unwrap().insert(
                provider_id.to_owned(),
                ActiveLogin {
                    login_id: login_id.clone(),
                    generation,
                    allow_unattributed_completion,
                },
            );
            Ok(LoginStart {
                login_id,
                authorization_url: Some(authorization_url),
                user_code: None,
            })
        })
    }

    /// 等待一次具体登录尝试的完成通知，并且只认这次尝试。
    /// 被取消或被新的尝试替换后立刻返回 `Ok(None)`：既不空等，也不接管别人的通知。
    fn login_result<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, Result<Option<LoginResult>>> {
        Box::pin(async move {
            let tracked = self
                .logins
                .lock()
                .unwrap()
                .get(provider_id)
                .cloned()
                .filter(|active| active.generation == generation);
            let Some(tracked) = tracked else {
                return Ok(None);
            };
            let deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
            loop {
                let current = self.logins.lock().unwrap().get(provider_id).cloned();
                if current.as_ref().map(|active| active.login_id.as_str()) != Some(tracked.login_id.as_str()) {
                    return Ok(None);
                }
                if let Some(params) = take_deferred(&self.deferred, provider_id, &tracked.login_id) {
                    if let Some(result) = self.complete_login(provider_id, &tracked.login_id, params).await? {
                        return Ok(Some(result));
                    }
                    continue;
                }
                let next = {
                    let mut servers = self.servers.lock().await;
                    match servers.get_mut(provider_id) {
                        Some(server) => tokio::time::timeout(POLL_SLICE, server.next_notification()).await,
                        None => return Ok(None),
                    }
                };
                let value = match next {
                    Ok(Some(value)) => value,
                    Ok(None) => return Ok(None),
                    Err(_) => {
                        if tokio::time::Instant::now() >= deadline {
                            bail!("The Codex authorization did not complete in time");
                        }
                        continue;
                    }
                };
                let Some((login_id, params)) = login_completion(&value) else {
                    continue;
                };
                if let Some(login_id) = login_id {
                    if login_id != tracked.login_id {
                        // 属于另一次尝试：放回队列交给它自己的等待者，绝不吞掉。
                        defer(&self.deferred, provider_id, &login_id, params);
                        continue;
                    }
                }
                if let Some(result) = self.complete_login(provider_id, &tracked.login_id, params).await? {
                    return Ok(Some(result));
                }
            }
        })
    }

    fn cancel_login<'a>(&'a self, provider_id: &'a str, generation: u64) -> futures_util::future::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let active = self.logins.lock().unwrap().remove(provider_id);
            self.deferred.lock().unwrap().remove(provider_id);
            let Some(active) = active.filter(|active| active.generation == generation) else {
                return Ok(());
            };
            let mut servers = self.servers().await;
            if let Some(server) = servers.get_mut(provider_id) {
                server.call("account/login/cancel", json!({"loginId": active.login_id})).await?;
            }
            Ok(())
        })
    }

    /// 退出：无论 RPC 成败都终止自有子进程并删除本服务商的专用授权目录。
    /// 只删 `helper_home(provider_id)` 这一个自有目录，绝不触碰用户的 `~/.codex`；
    /// 没有活动会话时也要清理，`local_cleared` 只在目录确实删除成功时为真。
    /// 服务商重命名。有活动辅助进程时**拒绝热迁移**：子进程的 `CODEX_HOME` 在 spawn 后就固定了，
    /// 搬目录会造成「磁盘已在新路径、进程仍写旧路径」。此时返回错误，由编排层对旧标识走退出
    /// （杀进程 + 清专用目录），新标识落未连接逼重登——与迁移失败的回退完全一致。
    /// 没有活动进程时才把专用授权目录与挂起状态整体迁到新标识。
    fn rename<'a>(&'a self, old_id: &'a str, new_id: &'a str) -> futures_util::future::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if old_id == new_id {
                return Ok(());
            }
            // 全程持有 servers 锁：迁移期间不允许再为旧标识拉起辅助进程（关闭检查-再搬的竞态）。
            let servers = self.servers.lock().await;
            anyhow::ensure!(
                !servers.contains_key(old_id),
                "The Codex helper is still running for {old_id}; sign out or sign in again after the rename"
            );
            let old_home = self.auth_home_for(old_id);
            let new_home = self.auth_home_for(new_id);
            if old_home.exists() {
                if let Some(parent) = new_home.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("Prepare the helper root {}", parent.display()))?;
                }
                std::fs::rename(&old_home, &new_home).with_context(|| format!(
                        "Move the dedicated helper home {} to {}",
                        old_home.display(),
                        new_home.display()
                    ))?;
            }
            drop(servers);
            if let Some(login) = self.logins.lock().unwrap().remove(old_id) {
                self.logins.lock().unwrap().insert(new_id.to_owned(), login);
            }
            if let Some(queue) = self.deferred.lock().unwrap().remove(old_id) {
                self.deferred.lock().unwrap().insert(new_id.to_owned(), queue);
            }
            let mut state = self.state.lock().unwrap();
            if state.auth_home.as_deref() == Some(old_home.to_string_lossy().as_ref()) {
                state.auth_home = Some(new_home.to_string_lossy().into_owned());
            }
            Ok(())
        })
    }

    fn logout<'a>(&'a self, provider_id: &'a str, _generation: u64) -> futures_util::future::BoxFuture<'a, Result<LogoutOutcome>> {
        Box::pin(async move {
            // The subscription layer has already invalidated this generation. Serialize helper
            // revocation with generation so queued old requests recheck and fail before dispatch.
            // Signal and interrupt the active turn before waiting for the lock: the stream consumer
            // may be backpressured and unable to poll its cancellation event itself.
            let pending = self.pending_tool_turns.lock().unwrap().remove(provider_id);
            if let Some(pending) = pending {
                discard_pending_tool_turn(&self.servers, provider_id, pending).await;
            }
            let active = self.active_turns.lock().unwrap().get(provider_id).cloned()
                .filter(|active| active.armed.load(std::sync::atomic::Ordering::SeqCst) || active.turn_id.is_none());
            let mut transferred_generation_guard = None;
            if let Some(active) = active {
                let _ = active.cancel.send(TurnCancelStatus::Requested);
                let helper_instance_id = active.helper_instance_id.as_deref();
                let stopped = match (helper_instance_id, active.thread_id.as_deref(), active.turn_id.as_deref()) {
                    (Some(instance_id), Some(thread_id), Some(turn_id)) => {
                        if interrupt_active_turn(&self.servers, provider_id, instance_id, thread_id, turn_id).await {
                            true
                        } else {
                            matches!(
                                reap_owned_helper(&self.servers, provider_id, instance_id).await,
                                ReapResult::Removed | ReapResult::AlreadyAbsent
                            )
                        }
                    }
                    (Some(instance_id), _, _) => matches!(
                        reap_owned_helper(&self.servers, provider_id, instance_id).await,
                        ReapResult::Removed | ReapResult::AlreadyAbsent
                    ),
                    (None, _, _) => false,
                };
                if stopped {
                    active.armed.store(false, std::sync::atomic::Ordering::SeqCst);
                    let _ = active.cancel.send(TurnCancelStatus::Complete);
                    transferred_generation_guard = active.generation_guard.lock().unwrap().take();
                }
            }
            let _generation_guard = match transferred_generation_guard {
                Some(guard) => guard,
                None => self.generation_lock(provider_id).lock_owned().await,
            };
            self.logins.lock().unwrap().remove(provider_id);
            self.deferred.lock().unwrap().remove(provider_id);
            let auth_home = match &self.home_root {
                Some(root) => helper_home_in(root, provider_id),
                None => helper_home(provider_id),
            };
            let remote = match self.servers().await.remove(provider_id) {
                Some(mut server) => {
                    let result = server.call("account/logout", json!({})).await;
                    server.shutdown();
                    match result {
                        Ok(value) => match value.get("remote").and_then(Value::as_str) {
                            Some("revoked") => RemoteRevocation::Revoked,
                            Some("unsupported") => RemoteRevocation::Unsupported,
                            _ => RemoteRevocation::Failed,
                        },
                        Err(_) => RemoteRevocation::Failed,
                    }
                }
                None => RemoteRevocation::Failed,
            };
            let local_cleared = remove_helper_home(&auth_home);
            if local_cleared {
                // 授权目录已不存在，不再对外声称一个已删除的路径。
                let deleted = auth_home.to_string_lossy().into_owned();
                let mut state = self.state.lock().unwrap();
                if state.auth_home.as_deref() == Some(deleted.as_str()) {
                    state.auth_home = None;
                }
            }
            Ok(LogoutOutcome { local_cleared, remote })
        })
    }

    /// Each admitted HTTP request receives a fresh ephemeral thread. A per-account mutex
    /// serializes this adapter's single notification reader; distinct accounts stay isolated.
    fn generate<'a>(&'a self, request: GenerationRequest<'a>) -> futures_util::future::BoxFuture<'a, Result<GenerationStream<'a>>> {
        let protocol = request.protocol;
        let turn = match codex_turn_request(protocol, &request.body) {
            Ok(turn) => turn,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let mut input_bytes = turn.input.iter().map(Value::to_string).map(|text| text.len()).sum::<usize>();
        input_bytes += turn.base_instructions.as_ref().map_or(0, String::len);
        input_bytes += turn.developer_instructions.as_ref().map_or(0, String::len);
        input_bytes += turn.dynamic_tools.iter().map(Value::to_string).map(|text| text.len()).sum::<usize>();
        input_bytes += turn.tool_outputs.iter().flat_map(|result| &result.output).map(String::len).sum::<usize>();
        if input_bytes > MAX_GENERATION_INPUT_BYTES {
            return Box::pin(async {
                Err(anyhow::anyhow!("Codex request input exceeds the 32 KiB limit; context compaction is not enabled"))
            });
        }

        let provider_id = request.provider_id.to_owned();
        let generation = request.generation;
        let model_id = request.model_id.to_owned();
        let pre_dispatch_check = request.pre_dispatch_check.clone();
        let pending_tool_turns = self.pending_tool_turns.clone();
        let pending_tool_cleanup = self.pending_tool_cleanup.clone();
        let generation_lock = self.generation_lock(&provider_id);
        Box::pin(async move {
            let generation_request_context = request_context(&turn);
            let servers_ref = self.servers.clone();
            let (generation_guard, mut pending, tool_responses, discard_error) = loop {
                pending_tool_cleanup.wait_until_clear(&provider_id).await;
                let generation_guard = generation_lock.clone().lock_owned().await;
                pre_dispatch_check().map_err(anyhow::Error::msg)?;
                let mut pending = None;
                let mut tool_responses = Vec::new();
                let mut discard_error = None;
                let retry_after_cleanup = {
                    let mut pending_turns = pending_tool_turns.lock().unwrap();
                    // Abandon can reserve cleanup while this request is queued for the
                    // generation mutex; don't let it dispatch until that cleanup completes.
                    if pending_tool_cleanup.is_active(&provider_id) {
                        true
                    } else {
                        if let Some(snapshot) = pending_turns.get(&provider_id).cloned() {
                            if !pending_matches_request(&snapshot, &model_id, protocol, &turn) {
                                bail!("A different Codex tool turn is pending; continue it with its original model, protocol, and tool results before sending another request");
                            }
                            let expired = snapshot.generation != generation || tokio::time::Instant::now() >= snapshot.deadline;
                            pending = pending_turns.remove(&provider_id);
                            if expired {
                                discard_error = Some(anyhow::anyhow!("The pending Codex tool turn expired or belongs to an old connection generation; it was discarded"));
                            } else {
                                match validate_tool_followup(pending.as_mut().expect("the matched pending turn was removed"), &turn) {
                                    Ok(responses) => tool_responses = responses,
                                    Err(error) => discard_error = Some(error),
                                }
                            }
                        } else if turn.has_assistant_history || !turn.tool_calls.is_empty() || !turn.tool_outputs.is_empty() {
                            bail!("Tool call history requires a matching live Codex turn from this connection generation");
                        }
                        false
                    }
                };
                if retry_after_cleanup {
                    drop(generation_guard);
                    continue;
                }
                break (generation_guard, pending, tool_responses, discard_error);
            };
            if let Some(error) = discard_error {
                if let Some(pending) = pending.take() {
                    discard_pending_tool_turn(&servers_ref, &provider_id, pending).await;
                }
                return Err(error);
            }
            let workspace = pending.as_ref().map(|pending| pending.workspace.clone()).map_or_else(generation_workspace, Ok)?;
            let generation_guard = Arc::new(Mutex::new(Some(generation_guard)));
            let armed = Arc::new(std::sync::atomic::AtomicBool::new(pending.is_some()));
            let request_id = uuid::Uuid::new_v4().to_string();
            let (cancel, mut cancel_receiver) = tokio::sync::watch::channel(TurnCancelStatus::Running);
            self.active_turns.lock().unwrap().insert(provider_id.clone(), ActiveTurnControl {
                request_id: request_id.clone(),
                cancel,
                helper_instance_id: pending.as_ref().map(|pending| pending.helper_instance_id.clone()),
                thread_id: pending.as_ref().map(|pending| pending.thread_id.clone()),
                turn_id: pending.as_ref().map(|pending| pending.turn_id.clone()),
                armed: armed.clone(),
                generation_guard: generation_guard.clone(),
            },
            );
            let active_registration = ActiveTurnRegistration {
                active_turns: self.active_turns.clone(),
                provider_id: provider_id.clone(),
                request_id };
            let previous = pending.as_ref();
            let mut cancellation = TurnCancellation {
                servers: servers_ref.clone(),
                provider_id: provider_id.clone(),
                helper_instance_id: previous.map(|pending| pending.helper_instance_id.clone()),
                thread_id: previous.map(|pending| pending.thread_id.clone()),
                turn_id: previous.map(|pending| pending.turn_id.clone()),
                workspace: Some(workspace.clone()),
                armed,
                generation_guard: generation_guard.clone(),
                active_registration,
                #[cfg(test)]
                cleanup_gate: self.cleanup_gate.lock().unwrap().take(),
            };
            let mut servers = self.server_for(&provider_id).await?;
            let server = servers.get_mut(&provider_id).expect("server_for inserted the Codex helper");
            let helper_instance_id = server.instance_id().to_owned();
            if pending.as_ref().is_some_and(|pending| pending.helper_instance_id != helper_instance_id) {
                bail!("The helper that owns the pending Codex tool call has changed; the turn was discarded");
            }
            cancellation.helper_instance_id = Some(helper_instance_id.clone());
            cancellation.active_registration.set_helper_instance(helper_instance_id);
            pre_dispatch_check().map_err(anyhow::Error::msg)?;
            if *cancel_receiver.borrow() != TurnCancelStatus::Running {
                anyhow::bail!("The Codex generation was cancelled before dispatch");
            }
            let (thread_id, turn_id, dynamic_tools, issued_tool_calls) = if let Some(pending) = pending.take() {
                for (rpc_id, result) in tool_responses {
                    tokio::select! {
                        response = server.respond_server_request(rpc_id, result) => response?,
                        _ = wait_for_turn_cancellation(&mut cancel_receiver) => anyhow::bail!("The Codex tool-result handoff was cancelled"),
                    }
                }
                (pending.thread_id, pending.turn_id, pending.dynamic_tools, pending.issued_tool_calls)
            } else {
                let mut developer_instructions = turn.developer_instructions.clone().unwrap_or_default();
                if !developer_instructions.is_empty() {
                developer_instructions.push_str("\n\n");
                }
                if turn.dynamic_tools.is_empty() {
                    developer_instructions.push_str("Application constraints: Use only the request text. Do not inspect files, use tools, browse, delegate, or continue with follow-up turns. Return assistant text only.");
            } else {
                    developer_instructions.push_str("Application constraints: Never inspect files, execute commands, browse, delegate, or use built-in tools. You may call only the client-provided functions. Return assistant text or client function calls and stop for the client to continue.");
                }
                let retry_provider_id = format!("autojev_no_retry_{}", uuid::Uuid::new_v4().simple());
            let mut thread_params = json!({
                "model": model_id,
                "ephemeral": true,
                "cwd": workspace.to_string_lossy(),
                "approvalPolicy": "untrusted",
                "sandbox": "read-only",
                "baseInstructions": turn.base_instructions,
                "developerInstructions": developer_instructions,
                "config": {"features": {
                    "shell_tool": false,
                    "view_image": false,
                    "sleep_tool": false,
                    "unified_exec": false,
                    "web_search_request": false,
                    "web_search_cached": false,
                    "standalone_web_search": false,
                    "code_mode": false,
                    "code_mode_host": false,
                    "multi_agent_v2": false,
                    "request_permissions_tool": false,
                    "unbounded_connection_retries": false
                }}
            });
                if !turn.dynamic_tools.is_empty() {
                    thread_params["dynamicTools"] = turn.dynamic_tools.clone().into();
                }
                let mut retry_providers = serde_json::Map::new();
            retry_providers.insert(retry_provider_id.clone(), json!({
                "name": "OpenAI",
                "requires_openai_auth": true,
                "request_max_retries": 0,
                "stream_max_retries": 0,
                "supports_websockets": false
            }),
                );
            thread_params["modelProvider"] = retry_provider_id.clone().into();
            thread_params["config"]["model_providers"] = Value::Object(retry_providers);
            if let Some(tier) = turn.service_tier.as_deref() {
                thread_params["serviceTier"] = tier.into();
            }
            let thread_result = tokio::select! {
                result = server.call("thread/start", thread_params) => result?,
                _ = wait_for_turn_cancellation(&mut cancel_receiver) => anyhow::bail!("The Codex generation was cancelled while starting its thread"),
                };
            let thread_id = thread_result.pointer("/thread/id").and_then(Value::as_str)
                .context("The Codex helper did not return a thread id")?.to_owned();
            cancellation.thread_id = Some(thread_id.clone());
            cancellation.active_registration.set_thread(thread_id.clone());
            anyhow::ensure!(
                thread_result.get("modelProvider").and_then(Value::as_str) == Some(retry_provider_id.as_str()),
                "The Codex helper did not apply the request's zero-retry model provider"
            );
            pre_dispatch_check().map_err(anyhow::Error::msg)?;
            let mut turn_params = json!({
                "threadId": thread_id,
                "model": model_id,
                "input": turn.input,
                "approvalPolicy": "untrusted",
                "sandboxPolicy": {"type":"readOnly","networkAccess":false}
            });
            if let Some(effort) = turn.effort.as_deref() {
                turn_params["effort"] = effort.into();
            }
            if let Some(summary) = turn.summary.as_deref() {
                turn_params["summary"] = summary.into();
            }
            if let Some(tier) = turn.service_tier.as_deref() {
                turn_params["serviceTierForTurn"] = tier.into();
            }
                cancellation.armed.store(true, std::sync::atomic::Ordering::SeqCst);
            let turn_result = tokio::select! {
                result = server.call("turn/start", turn_params) => result?,
                _ = wait_for_turn_cancellation(&mut cancel_receiver) => anyhow::bail!("The Codex generation was cancelled while starting its turn"),
                };
            let turn_id = turn_result.pointer("/turn/id").and_then(Value::as_str)
                .context("The Codex helper did not return a turn id")?.to_owned();
            cancellation.turn_id = Some(turn_id.clone());
            cancellation.active_registration.set_turn(turn_id.clone());
                (thread_id, turn_id, turn.dynamic_tools.clone(), HashMap::new())
            };
            drop(servers);

            let state = GenerationState {
                servers: servers_ref.clone(),
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
                protocol,
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                generation,
                dynamic_tools,
                request_context: generation_request_context,
                issued_tool_calls,
                pending_tool_turns: pending_tool_turns.clone(),
                pending_tool_cleanup: pending_tool_cleanup.clone(),
                generation_lock: generation_lock.clone(),
                started: false,
                terminal: false,
                deadline: tokio::time::Instant::now() + GENERATION_TIMEOUT,
                cancel: cancel_receiver,
                cancellation,
            };
            let stream = futures_util::stream::unfold(state, |mut state| async move {
                if state.terminal {
                    return None;
                }
                if !state.started {
                    state.started = true;
                    return Some((GenerationEvent::Started { generation: state.generation }, state));
                }
                loop {
                    if *state.cancel.borrow() != TurnCancelStatus::Running {
                        wait_for_cancel_completion(&mut state.cancel).await;
                        state.cancellation.disarm_and_release();
                        state.terminal = true;
                        return Some((GenerationEvent::Cancelled, state));
                    }
                    if tokio::time::Instant::now() >= state.deadline {
                        if let Some(helper_instance_id) = state.cancellation.helper_instance_id.as_deref() {
                            stop_active_turn(
                                &state.servers,
                                &state.provider_id,
                                helper_instance_id,
                                &state.thread_id,
                                &state.turn_id).await;
                        }
                        state.cancellation.disarm_and_release();
                        state.terminal = true;
                        return Some((GenerationEvent::Failed { message: "The Codex turn timed out".into() }, state));
                    }
                    let next = tokio::select! {
                        _ = wait_for_turn_cancellation(&mut state.cancel) => {
                            wait_for_cancel_completion(&mut state.cancel).await;
                            state.cancellation.disarm_and_release();
                            state.terminal = true;
                            return Some((GenerationEvent::Cancelled, state));
                        }
                        next = async {
                            let mut servers = state.servers.lock().await;
                            match servers.get_mut(&state.provider_id) {
                                Some(server) => Some(tokio::time::timeout(POLL_SLICE, server.next_message()).await),
                                None => None,
                            }
                        } => next,
                    };
                    let Some(next) = next else {
                        state.cancellation.disarm_and_release();
                        state.terminal = true;
                        return Some((GenerationEvent::Failed { message: "The Codex helper exited during generation".into() }, state));
                    };
                    let message = match next {
                        Err(_) => continue,
                        Ok(Some(message)) => message,
                        Ok(None) => {
                            state.cancellation.disarm_and_release();
                            state.terminal = true;
                            return Some((GenerationEvent::Failed { message: "The Codex helper stopped before completing the turn".into() },
                                state,
                            ));
                        }
                    };
                    let value = match message {
                        HelperMessage::Notification(value) => value,
                        HelperMessage::ServerRequest(first) => {
                            let requests = {
                                let mut servers = state.servers.lock().await;
                                match servers.get_mut(&state.provider_id) {
                                    Some(server) if server.instance_id() == state.cancellation.helper_instance_id.as_deref().unwrap_or("") => {
                                        server.collect_server_requests(first).await
                                    }
                                    _ => vec![first],
                                }
                            };
                            let calls = match client_tool_calls_from_requests(
                                requests,
                                &state.thread_id,
                                &state.turn_id,
                                state.generation,
                                &state.dynamic_tools,
                                &mut state.issued_tool_calls,
                            ) {
                                Ok(calls) => calls,
                                Err(error) => {
                                    if let Some(helper_instance_id) = state.cancellation.helper_instance_id.as_deref() {
                                        stop_active_turn(&state.servers, &state.provider_id, helper_instance_id, &state.thread_id, &state.turn_id).await;
                                    }
                                    state.cancellation.disarm_and_release();
                                    state.terminal = true;
                                    return Some((GenerationEvent::Failed { message: redact(&error.to_string()) }, state));
                                }
                            };
                            let Some(workspace) = state.cancellation.release_for_tool_handoff() else {
                                state.terminal = true;
                                return Some((
                                    GenerationEvent::Failed { message: "The Codex tool handoff could not preserve its request workspace".into() },
                                    state,
                                ));
                            };
                            let deadline = tokio::time::Instant::now() + GENERATION_TIMEOUT;
                            let pending = PendingToolTurn {
                                generation: state.generation,
                                model_id: state.model_id.clone(),
                                protocol: state.protocol,
                                helper_instance_id: state.cancellation.helper_instance_id.clone().unwrap_or_default(),
                                thread_id: state.thread_id.clone(),
                                turn_id: state.turn_id.clone(),
                                workspace,
                                dynamic_tools: state.dynamic_tools.clone(),
                                request_context: state.request_context.clone(),
                                issued_tool_calls: state.issued_tool_calls.clone(),
                                deadline,
                            };
                            let replaced = state.pending_tool_turns.lock().unwrap().insert(state.provider_id.clone(), pending);
                            if let Some(replaced) = replaced {
                                discard_pending_tool_turn(&state.servers, &state.provider_id, replaced).await;
                                state.terminal = true;
                                return Some((
                                    GenerationEvent::Failed { message: "Another Codex tool turn was pending; the earlier turn was discarded".into() },
                                    state,
                                ));
                            }
                            let pending_tool_turns = state.pending_tool_turns.clone();
                            let pending_tool_cleanup = state.pending_tool_cleanup.clone();
                            let generation_lock = state.generation_lock.clone();
                            let servers = state.servers.clone();
                            let provider_id = state.provider_id.clone();
                            let helper_instance_id = state.cancellation.helper_instance_id.clone().unwrap_or_default();
                            let thread_id = state.thread_id.clone();
                            let turn_id = state.turn_id.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep_until(deadline).await;
                                let expired = {
                                    let mut turns = pending_tool_turns.lock().unwrap();
                                    if turns.get(&provider_id).is_some_and(|pending| {
                                        pending.helper_instance_id == helper_instance_id
                                            && pending.thread_id == thread_id
                                            && pending.turn_id == turn_id
                                            && pending.deadline == deadline
                                    }) {
                                        pending_tool_cleanup.begin(&provider_id);
                                        turns.remove(&provider_id)
                                    } else {
                                        None
                                    }
                                };
                                if let Some(expired) = expired {
                                    let _generation_guard = generation_lock.lock_owned().await;
                                    discard_pending_tool_turn(&servers, &provider_id, expired).await;
                                    pending_tool_cleanup.finish(&provider_id);
                                }
                            });
                            state.terminal = true;
                            return Some((GenerationEvent::ToolCalls { calls }, state));
                        }
                    };
                    let params = &value["params"];
                    if params["threadId"].as_str() != Some(state.thread_id.as_str()) {
                        continue;
                    }
                    match value["method"].as_str().unwrap_or("") {
                        "item/agentMessage/delta" if params["turnId"].as_str() == Some(state.turn_id.as_str()) => {
                            if let Some(delta) = params["delta"].as_str().filter(|delta| !delta.is_empty()) {
                                return Some((GenerationEvent::Chunk(delta.to_owned()), state));
                            }
                        }
                        "turn/completed" if params.pointer("/turn/id").and_then(Value::as_str) == Some(state.turn_id.as_str()) => {
                            let turn = &params["turn"];
                            state.cancellation.disarm_and_release();
                            state.terminal = true;
                            let event = match turn["status"].as_str() {
                                Some("completed") => GenerationEvent::Finished { status: 200 },
                                Some("interrupted") => GenerationEvent::Cancelled,
                                Some("failed") => {
                                    let message = turn.pointer("/error/message").and_then(Value::as_str)
                                        .map(redact).unwrap_or_else(|| "The Codex turn failed".into());
                                    GenerationEvent::Failed { message }
                                }
                                _ => GenerationEvent::Failed { message: "The Codex helper returned an unknown turn status".into() },
                            };
                            return Some((event, state));
                        }
                        method if method.starts_with("item/") && params["turnId"].as_str() == Some(state.turn_id.as_str()) => {
                            let ordinary_lifecycle = matches!(method, "item/started" | "item/completed")
                                && matches!(params.pointer("/item/type").and_then(Value::as_str),
                                    Some("userMessage" | "agentMessage" | "reasoning" | "plan" | "dynamicToolCall"));
                            let reasoning_notification = matches!(method,
                                "item/reasoning/summaryTextDelta"
                                | "item/reasoning/summaryPartAdded"
                                | "item/reasoning/textDelta"
                            );
                            if ordinary_lifecycle || reasoning_notification || method == "item/plan/delta" {
                                continue;
                            }
                            if let Some(helper_instance_id) = state.cancellation.helper_instance_id.as_deref() {
                                stop_active_turn(
                                    &state.servers,
                                    &state.provider_id,
                                    helper_instance_id,
                                    &state.thread_id,
                                    &state.turn_id).await;
                            }
                            state.cancellation.disarm_and_release();
                            state.terminal = true;
                            return Some((GenerationEvent::Failed { message: "Codex tool activity or unknown item activity is not accepted by this gateway".into() }, state,
                            ));
                        }
                        _ => continue,
                    }
                }
            });
            let stream: GenerationStream<'a> = Box::pin(stream);
            Ok(stream)
        })
    }

    fn abandon_client_tool_calls(&self, provider_id: &str, generation: u64, call_ids: &[String]) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let supplied: std::collections::HashSet<_> = call_ids.iter().map(String::as_str).collect();
        if supplied.len() != call_ids.len() || supplied.is_empty() {
            return;
        }
        let pending_tool_cleanup = self.pending_tool_cleanup.clone();
        let pending = {
            let mut turns = self.pending_tool_turns.lock().unwrap();
            let Some(current) = turns.get(provider_id) else { return;
            };
            let outstanding: std::collections::HashSet<_> = current
                .issued_tool_calls
                .iter()
                .filter_map(|(id, call)| (!call.answered).then_some(id.as_str()))
                .collect();
            if current.generation != generation || outstanding != supplied {
                return;
            }
            // Reserve before removing the map entry so a same-account request cannot
            // mistake the temporarily empty map for an idle helper.
            pending_tool_cleanup.begin(provider_id);
            turns.remove(provider_id)
        };
        if let Some(pending) = pending {
            let servers = self.servers.clone();
            let provider_id = provider_id.to_owned();
            let generation_lock = self.generation_lock(&provider_id);
            runtime.spawn(async move {
                let _generation_guard = generation_lock.lock_owned().await;
                discard_pending_tool_turn(&servers, &provider_id, pending).await;
                pending_tool_cleanup.finish(&provider_id);
            });
        }
    }
}

async fn wait_for_turn_cancellation(receiver: &mut tokio::sync::watch::Receiver<TurnCancelStatus>) {
    loop {
        if *receiver.borrow_and_update() != TurnCancelStatus::Running || receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn wait_for_cancel_completion(receiver: &mut tokio::sync::watch::Receiver<TurnCancelStatus>) {
    loop {
        if *receiver.borrow_and_update() == TurnCancelStatus::Complete || receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn wait_for_turn_completion(
    servers: &Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>,
    provider_id: &str,
    helper_instance_id: &str,
    thread_id: &str,
    turn_id: &str,
) -> bool {
    let deadline = tokio::time::Instant::now() + TURN_COMPLETION_TIMEOUT;
    loop {
        let next = {
            let mut servers = servers.lock().await;
            let Some(server) = servers.get_mut(provider_id) else { return false;
            };
            if server.instance_id() != helper_instance_id { return false; }
            tokio::time::timeout_at(deadline, server.next_notification()).await
        };
        let value = match next {
            Ok(Some(value)) => value,
            Ok(None) | Err(_) => return false,
        };
        if value["method"] == "turn/completed"
            && value["params"]["threadId"].as_str() == Some(thread_id)
            && value.pointer("/params/turn/id").and_then(Value::as_str) == Some(turn_id)
        {
            return true;
        }
    }
}

async fn stop_active_turn(
    servers: &Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>,
    provider_id: &str,
    helper_instance_id: &str,
    thread_id: &str,
    turn_id: &str) {
    if interrupt_active_turn(servers, provider_id, helper_instance_id, thread_id, turn_id).await
        && wait_for_turn_completion(servers, provider_id, helper_instance_id, thread_id, turn_id).await
    {
        return;
    }
    // An interrupt reply only acknowledges the RPC. If it fails or the terminal turn
    // notification never arrives, kill and reap this exact app-owned helper before its
    // per-account generation guard can be released.
    reap_owned_helper(servers, provider_id, helper_instance_id).await;
}

async fn discard_pending_tool_turn(servers: &Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>, provider_id: &str, pending: PendingToolTurn) {
    stop_active_turn(servers, provider_id, &pending.helper_instance_id, &pending.thread_id, &pending.turn_id).await;
    let _ = std::fs::remove_dir_all(pending.workspace);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReapResult {
    Removed,
    AlreadyAbsent,
    DifferentInstance,
}

async fn interrupt_active_turn(
    servers: &Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>,
    provider_id: &str,
    helper_instance_id: &str,
    thread_id: &str,
    turn_id: &str,
) -> bool {
    let mut servers = servers.lock().await;
    let Some(server) = servers.get_mut(provider_id) else { return false;
    };
    if server.instance_id() != helper_instance_id {
        return false;
    }
    server.call("turn/interrupt", json!({"threadId":thread_id,"turnId":turn_id})).await.is_ok()
}

async fn reap_owned_helper(
    servers: &Arc<tokio::sync::Mutex<HashMap<String, CodexAppServer>>>,
    provider_id: &str,
    helper_instance_id: &str) -> ReapResult {
    let mut servers = servers.lock().await;
    match servers.get(provider_id) {
        Some(server) if server.instance_id() != helper_instance_id => ReapResult::DifferentInstance,
        Some(_) => {
            if let Some(mut server) = servers.remove(provider_id) {
                server.shutdown();
            }
            ReapResult::Removed
        }
        None => ReapResult::AlreadyAbsent,
    }
}

/// 只删除本应用的专用授权目录；路径越界一律拒绝（绝不触碰用户 `~/.codex`）。
/// 目录本来就不存在也算清理成功。
fn remove_helper_home(auth_home: &Path) -> bool {
    let expected_parent = Path::new(HELPER_ROOT);
    if auth_home.parent().map(|parent| parent.ends_with(expected_parent)) != Some(true) {
        return false;
    }
    match std::fs::remove_dir_all(auth_home) {
        Ok(()) => true,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// `account:null` 是明确无账号；ChatGPT 的 nullable email 缺失不是退出证据。
/// requiresOpenaiAuth 表示该服务需要 OpenAI 认证，已登录时同样可以为 true。
/// 兼容既有替身/旧响应的 requiresAuth，但不从 email 缺失推导已退出。
fn account_status(result: &Value) -> (ConnectionState, Option<String>, bool) {
    let account = result.get("account");
    let legacy_auth = result.get("requiresAuth").and_then(Value::as_bool);
    let requires_auth = result.get("requiresOpenaiAuth").and_then(Value::as_bool).or(legacy_auth);
    let signed_out = (account == Some(&Value::Null) && requires_auth.is_some())
        || (account.is_none() && legacy_auth == Some(true));
    if signed_out {
        return (ConnectionState::NotConnected, None, false);
    }
    let account_type = account.and_then(|account| account.get("type"));
    let subscription_account = account_type.is_none() || account_type.and_then(Value::as_str) == Some("chatgpt");
    let identity = subscription_account.then(|| account_email(result)).flatten();
    if identity.is_some() && requires_auth.is_some() && legacy_auth != Some(true) {
        (ConnectionState::Connected, identity, false)
    } else {
        (ConnectionState::NotConnected, None, true)
    }
}

fn account_email(result: &Value) -> Option<String> {
    result
        .get("account")
        .and_then(|account| account.get("email"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|email| !email.trim().is_empty())
}

/// 只读读取固定的 RPC 方法名：由本应用写死在代码里，不由配置或环境决定。
const CATALOG_METHOD: &str = "model/list";
const QUOTA_METHOD: &str = "account/rateLimits/read";

/// 证据来源：每次成功读取都记下方法与视图，失败时不得更新。
const CATALOG_SOURCE: &str = "codex-app-server:model/list";
const QUOTA_SOURCE_BY_LIMIT_ID: &str = "codex-app-server:account/rateLimits/read#rateLimitsByLimitId";
const QUOTA_SOURCE_RATE_LIMITS: &str = "codex-app-server:account/rateLimits/read#rateLimits";
const QUOTA_SOURCE_BASE: &str = "codex-app-server:account/rateLimits/read";

fn observed_at_now() -> String {
    // 规范形式：`...Z` + 秒精度，界面会原样外露「最后成功更新 <时间>」，不带纳秒与 +00:00。
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// 读取一个非空字符串字段（去掉首尾空白）；缺失、null 或空白都算没有值。
fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// `model/list` 的只读映射。主形状是 `{"data":[...]}`（`nextCursor` 本票不消费）；
/// 同一辅助进程的旧形状 `{"models":[...]}` 仍然容忍。没有任何可用标识的条目记入
/// `missing_fields`（`model.list[i].id`）并跳过，绝不编造标识。
fn parse_catalog(result: &Value) -> Result<CatalogRead> {
    let Some(entries) = ["data", "models"].iter().find_map(|key| result.get(*key).and_then(Value::as_array)) else {
        // 既不是 data[] 也不是 models[]：这不是一份权威目录，按读取失败处理，
        // 而不是声称「目录为空」（那会撤销全部已发现的模型）。
        bail!("The Codex helper returned no model list");
    };
    let mut models = Vec::new();
    let mut missing_fields = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(model_id) = string_field(entry, "id").or_else(|| string_field(entry, "model")) else {
            missing_fields.push(format!("model.list[{index}].id"));
            continue;
        };
        let name = string_field(entry, "displayName").or_else(|| string_field(entry, "name"));
        // 发现 ≠ 资格：固定版本没有任何资格布尔字段，本票一律保持 false（资格由 #17 定义）。
        models.push(DiscoveredModel { model_id, name, eligible: false });
    }
    Ok(CatalogRead { state: EvidenceState::Available, models, source: Some(CATALOG_SOURCE.to_owned()), observed_at: Some(observed_at_now()), missing_fields,
    })
}

// CHUNK-QUOTA

/// `account/rateLimits/read` 的只读映射。
///
/// 已核实的根层字段（openai/codex @ ed9e5a26，与当前 main 逐字节相同）：
/// `ordinaryUsageAllowed: Option<bool>`（在**结果根层**，不在 snapshot 内，也不在 `account/read`；
/// 服务端在账号不匹配或 FedRAMP 时强制为 null）、`rateLimits`（legacy 单桶，恒存在）、
/// `rateLimitsByLimitId: Option<HashMap<limitId, snapshot>>`、`accountId`、`rateLimitResetCredits`、
/// `rateLimitUpsell`。固定版本的 snapshot 还存在但**本票不读取**的字段：`normalModelSlug`、
/// `individualLimit`、`spendControlReached`、`rateLimitReachedType`（额度耗尽与额外消费准入属 #17/#18/#25）。
fn parse_quota(result: &Value) -> QuotaEvidence {
    // 根层许可是权威值；没有任何桶内显式依据时才逐桶沿用，桶内显式的更严格取值优先（fail-closed）。
    let root_permission = root_ordinary_usage_allowed(result);
    let multi = result.get("rateLimitsByLimitId").and_then(Value::as_object).filter(|entries| !entries.is_empty());
    let (view, source, buckets, malformed_buckets) = if let Some(entries) = multi {
        // 稳定顺序：HashMap 迭代顺序随机，界面与测试都需要确定的多桶顺序。
        let mut keys: Vec<&String> = entries.keys().collect();
        keys.sort();
        let mut malformed = false;
        // 非对象条目解析不出桶：有意丢弃，不编造桶；若一个桶都解析不出来，下面如实记入顶层缺失。
        let buckets: Vec<QuotaBucket> = keys
            .into_iter()
            .filter_map(|key| match entries.get(key) {
                Some(snapshot) if snapshot.is_object() => Some(parse_bucket(key, snapshot, root_permission.as_ref())),
                _ => {
                    malformed = true;
                    None
                }
            })
            .collect();
        let malformed_buckets = malformed && buckets.is_empty();
        (QuotaView::RateLimitsByLimitId, QUOTA_SOURCE_BY_LIMIT_ID, buckets, malformed_buckets)
    } else if let Some(snapshot) = result.get("rateLimits").filter(|snapshot| snapshot.is_object()) {
        (QuotaView::RateLimits, QUOTA_SOURCE_RATE_LIMITS, vec![parse_bucket("", snapshot, root_permission.as_ref())], false,
        )
    } else {
        // 读取成功但既无多桶也无旧版单桶：证据为 Unknown，顶层缺失字段由下面补齐。
        (QuotaView::Unknown, QUOTA_SOURCE_BASE, Vec::new(), false)
    };
    let state = quota_state(&buckets);
    let missing_fields = quota_missing_fields(view, state, &buckets, malformed_buckets);
    QuotaEvidence {
        state,
        source: Some(source.to_owned()),
        observed_at: Some(observed_at_now()),
        view,
        buckets,
        missing_fields,
        history: false,
    }
}

/// 根层 `ordinaryUsageAllowed`：`Some(Ok(flag))` 为布尔，`Some(Err(text))` 为非布尔，`None` 为缺失/null。
fn root_ordinary_usage_allowed(root: &Value) -> Option<std::result::Result<bool, String>> {
    match root.get("ordinaryUsageAllowed") {
        Some(Value::Bool(flag)) => Some(Ok(*flag)),
        Some(Value::Null) | None => None,
        Some(raw) => Some(Err(format!("ordinaryUsageAllowed={raw}"))),
    }
}

/// 桶内许可解析：(许可, 记入 missing 的字段, 记入 invalid 的原始文本)。
///
/// fail-closed：桶内显式的 `false` 或非布尔**优先且更严格**，根层 `true` 不得把它覆盖成 Allowed。
/// 桶内为 `true`、缺失或 null 时才沿用根层值（固定版本只在根层有该字段，桶内键属防御性输入）：
/// 布尔 → Allowed/Denied；非布尔 → Unknown + invalid；缺失/null → Unknown + missing `ordinaryUsageAllowed`。
fn bucket_permission(snapshot: &Value, root: Option<&std::result::Result<bool, String>>) -> (QuotaPermission, Option<&'static str>, Option<String>) {
    match snapshot.get("ordinaryUsageAllowed") {
        Some(Value::Bool(false)) => return (QuotaPermission::Denied, None, None),
        Some(Value::Null) | Some(Value::Bool(true)) | None => {}
        Some(raw) => return (QuotaPermission::Unknown, None, Some(format!("ordinaryUsageAllowed={raw}"))),
    }
    match root {
        Some(Ok(true)) => (QuotaPermission::Allowed, None, None),
        Some(Ok(false)) => (QuotaPermission::Denied, None, None),
        Some(Err(raw)) => (QuotaPermission::Unknown, None, Some(raw.clone())),
        None => (QuotaPermission::Unknown, Some("ordinaryUsageAllowed"), None),
    }
}

/// 一个额度桶。窗口字段越界或类型不符时取值 `None` 并记下原始文本；缺失记入 `missing_fields`。
fn parse_bucket(fallback_key: &str, snapshot: &Value, root_permission: Option<&std::result::Result<bool, String>>) -> QuotaBucket {
    let mut missing_fields = Vec::new();
    let mut invalid_fields = Vec::new();
    let limit_id = match string_field(snapshot, "limitId").or_else(|| (!fallback_key.is_empty()).then(|| fallback_key.to_owned())) {
        Some(limit_id) => limit_id,
        None => {
            // 单桶视图没有 limitId 时只能用占位标识，并如实记下缺失。
            missing_fields.push("limitId".to_owned());
            "unknown".to_owned()
        }
    };
    let mut windows = Vec::new();
    for label in ["primary", "secondary"] {
        match snapshot.get(label).filter(|value| value.is_object()) {
            Some(window) => windows.push(parse_window(label, window)),
            None => missing_fields.push(label.to_owned()),
        }
    }
    // 旧版单桶兼容：快照本身直接带窗口字段时按 single 读；只有确实没有 primary/secondary 才这样。
    if windows.is_empty() && ["usedPercent", "windowDurationMins", "resetsAt"].iter().any(|key| snapshot.get(*key).is_some()) {
        windows.push(parse_window("single", snapshot));
        missing_fields.retain(|field| field != "primary" && field != "secondary");
    }
    let credits = match snapshot.get("credits") {
        Some(Value::Object(map)) => {
            let mut credits = QuotaCredits::default();
            credits.has_credits = match map.get("hasCredits") {
                Some(Value::Bool(flag)) => Some(*flag),
                _ => {
                    credits.missing_fields.push("hasCredits".to_owned());
                    None
                }
            };
            credits.unlimited = match map.get("unlimited") {
                Some(Value::Bool(flag)) => Some(*flag),
                _ => {
                    credits.missing_fields.push("unlimited".to_owned());
                    None
                }
            };
            // balance 原样保留字符串：不解析为金额、不推断单位。
            credits.balance = match map.get("balance").and_then(Value::as_str) {
                Some(balance) => Some(balance.to_owned()),
                None => {
                    credits.missing_fields.push("balance".to_owned());
                    None
                }
            };
            Some(credits)
        }
        _ => {
            missing_fields.push("credits".to_owned());
            None
        }
    };
    let (permission, permission_missing, permission_invalid) = bucket_permission(snapshot, root_permission);
    if let Some(field) = permission_missing {
        missing_fields.push(field.to_owned());
    }
    if let Some(text) = permission_invalid {
        invalid_fields.push(text);
    }
    QuotaBucket {
        limit_id,
        name: string_field(snapshot, "limitName"),
        plan_type: string_field(snapshot, "planType"),
        windows,
        credits,
        permission,
        missing_fields,
        invalid_fields,
    }
}

/// 一个额度窗口。`usedPercent` 只接受有限且 `0 <= x <= 100` 的数字（服务端为 i32）；
/// `windowDurationMins` 只接受整数 `>= 0`；`resetsAt` 只接受整数 `> 0`（Unix 秒）。
/// 越界/类型不符 → 取值 `None` + `invalid_fields` 记原始文本；缺失 → `missing_fields`。
fn parse_window(label: &str, snapshot: &Value) -> QuotaWindow {
    let mut window = QuotaWindow::new(label);
    window.used_percent = match snapshot.get("usedPercent") {
        None | Some(Value::Null) => {
            window.missing_fields.push("usedPercent".to_owned());
            None
        }
        Some(raw) => match raw.as_f64() {
            Some(percent) if percent.is_finite() && (0.0..=100.0).contains(&percent) => Some(percent),
            _ => {
                window.invalid_fields.push(format!("usedPercent={raw}"));
                None
            }
        },
    };
    window.window_minutes = match snapshot.get("windowDurationMins") {
        None | Some(Value::Null) => {
            window.missing_fields.push("windowDurationMins".to_owned());
            None
        }
        Some(raw) => match raw.as_i64() {
            Some(minutes) if minutes >= 0 => Some(minutes),
            _ => {
                window.invalid_fields.push(format!("windowDurationMins={raw}"));
                None
            }
        },
    };
    window.resets_at = match snapshot.get("resetsAt") {
        None | Some(Value::Null) => {
            window.missing_fields.push("resetsAt".to_owned());
            None
        }
        Some(raw) => match raw.as_i64() {
            Some(seconds) if seconds > 0 => Some(seconds),
            _ => {
                window.invalid_fields.push(format!("resetsAt={raw}"));
                None
            }
        },
    };
    window
}

/// 顶层缺失字段：没有视图时记下两个视图键；多桶形状不可用时说明无法解析出桶；
/// 有桶但状态未知时说明缺许可或缺 usedPercent。
fn quota_missing_fields(view: QuotaView, state: EvidenceState, buckets: &[QuotaBucket], malformed_buckets: bool) -> Vec<String> {
    if view == QuotaView::Unknown {
        return vec!["rateLimitsByLimitId".to_owned(), "rateLimits".to_owned()];
    }
    let mut missing_fields = Vec::new();
    if malformed_buckets {
        // 视图仍是多桶（map 存在且非空），但里面没有任何能解析成桶的形状：不是「没有视图」。
        missing_fields.push("rateLimitsByLimitId".to_owned());
    }
    if state == EvidenceState::Unknown {
        if buckets.iter().any(|bucket| bucket.permission == QuotaPermission::Unknown) {
            missing_fields.push("ordinaryUsageAllowed".to_owned());
        }
        if !buckets.iter().flat_map(|bucket| bucket.windows.iter()).any(|window| window.used_percent.is_some()) {
            missing_fields.push("usedPercent".to_owned());
        }
    }
    missing_fields
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn initialize_helper_self_report_is_display_only_and_version_requires_canonical_semver() {
        let fixture = json!({"userAgent":"codex_cli_rs/0.159.0 (Test OS; x86_64) rust"});
        let (identity, version) = helper_self_report(&fixture);
        assert_eq!(identity.as_deref(), Some("codex_cli_rs/0.159.0 (Test OS; x86_64) rust"));
        assert_eq!(version.as_deref(), Some("0.159.0"));

        for response in [
            json!({}),
            json!({"version":"9.9.9"}),
            json!({"userAgent": null}),
            json!({"userAgent": 7}),
        ] {
            let (identity, version) = helper_self_report(&response);
            assert!(identity.is_none());
            assert!(version.is_none());
        }

        for malformed in [
            "codex_cli_rs/not-a-version (Test OS)",
            "codex_cli_rs/0.159 (Test OS)",
            "codex_cli_rs/00.159.0 (Test OS)",
            "other_helper/0.159.0 (Test OS)",
            "codex_cli_rs/0.159.0.1 (Test OS)",
            "codex_cli_rs/0.159.0- (Test OS)",
            "codex_cli_rs/0.159.0-+build (Test OS)",
        ] {
            let (identity, version) = helper_self_report(&json!({"userAgent": malformed}));
            assert_eq!(identity.as_deref(), Some(malformed));
            assert!(version.is_none(), "malformed helper version must remain unknown: {malformed}");
        }
    }

    /// 只存在于 Rust 单测临时目录的假 Codex 辅助进程：不联网、不读任何真实凭据。
    /// 第一个参数是环境记录文件路径，同时也是只读场景队列的基名（`<base>.catalog` / `<base>.quota`，
    /// 每行一个场景名，依次消费；没有队列文件时走各方法的默认场景）。
    const FIXTURE_HELPER: &str = r#"#!/bin/sh
env_log="$1"
queue="$1"
helper_pid=$$
if [ -n "$env_log" ]; then
  {
    printf 'CODEX_HOME=%s\n' "$CODEX_HOME"
    printf 'OPENAI_API_KEY=%s\n' "${OPENAI_API_KEY:-<unset>}"
  } >> "$env_log"
fi
next_scenario() {
  file="$1"
  counter="$file.counter"
  n=0
  if [ -f "$counter" ]; then n=$(cat "$counter"); fi
  n=$((n+1))
  printf '%s' "$n" > "$counter"
  if [ -f "$file" ]; then sed -n "${n}p" "$file"; fi
}
while IFS= read -r line; do
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  if [ -z "$method" ]; then
    rpc_id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
    if [ -n "$rpc_id" ]; then printf '%s\n' "$line" >> "$queue.tool-responses"; fi
    continue
  fi
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "$id" ]; then continue; fi
  printf '%s\n' "$method" >> "$queue.calls"
  printf '%s\n' "$line" >> "$queue.rpc"
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"userAgent":"codex_cli_rs/0.159.0 (Test OS; x86_64) rust","codexHome":"%s","platformFamily":"unix","platformOs":"macos"}}\n' "$id" "$CODEX_HOME" ;;
    *'"method":"account/read"'*)
      scenario=$(next_scenario "$queue.account")
      case "$scenario" in
        signed-out) result='{"account":null,"requiresOpenaiAuth":true}' ;;
        incomplete) result='{"account":{"type":"chatgpt","email":null,"planType":"pro"},"requiresOpenaiAuth":true}' ;;
        api-key-email) result='{"account":{"type":"apiKey","email":"fixture@example.invalid"},"requiresOpenaiAuth":true}' ;;
        *) result='{"account":{"type":"chatgpt","email":"fixture@example.invalid","planType":"pro"},"requiresOpenaiAuth":true}' ;;
      esac
      printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result" ;;

    *'"method":"model/list"'*)
      scenario=""
      if [ -n "$queue" ]; then scenario=$(next_scenario "$queue.catalog"); fi
      case "$scenario" in
        missing) printf '{"jsonrpc":"2.0","id":%s,"result":{"data":[{"displayName":"no-identity"},{"id":"preset-ok","displayName":"OK"}],"nextCursor":null}}\n' "$id" ;;
        models) printf '{"jsonrpc":"2.0","id":%s,"result":{"models":[{"id":"legacy-model","displayName":"Legacy"}]}}\n' "$id" ;;
        fail) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"fixture catalog read failed"}}\n' "$id" ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"data":[{"id":"preset-a","model":"gpt-5-codex","displayName":"GPT-5 Codex"},{"model":"slug-only"},{"displayName":"no-identity"}],"nextCursor":null}}\n' "$id" ;;
      esac ;;
    *'"method":"account/rateLimits/read"'*)
      scenario=""
      if [ -n "$queue" ]; then scenario=$(next_scenario "$queue.quota"); fi
      case "$scenario" in
        single) printf '{"jsonrpc":"2.0","id":%s,"result":{"ordinaryUsageAllowed":true,"accountId":"fixture-account","rateLimits":{"limitId":"legacy-single","limitName":"Legacy","primary":{"usedPercent":7,"windowDurationMins":60,"resetsAt":1800000000},"credits":{"hasCredits":true,"unlimited":false,"balance":"3 credits"}}}}\n' "$id" ;;
        missing) printf '{"jsonrpc":"2.0","id":%s,"result":{"ordinaryUsageAllowed":true,"rateLimits":{"limitId":"legacy","primary":{"usedPercent":5}}}}\n' "$id" ;;
        invalid) printf '{"jsonrpc":"2.0","id":%s,"result":{"ordinaryUsageAllowed":true,"rateLimits":{"limitId":"legacy","primary":{"usedPercent":142,"windowDurationMins":-5,"resetsAt":0}}}}\n' "$id" ;;
        denied) printf '{"jsonrpc":"2.0","id":%s,"result":{"ordinaryUsageAllowed":false,"rateLimits":{"limitId":"legacy","primary":{"usedPercent":9,"windowDurationMins":60,"resetsAt":1800000000}}}}\n' "$id" ;;
        no-permission) printf '{"jsonrpc":"2.0","id":%s,"result":{"rateLimits":{"limitId":"legacy","primary":{"usedPercent":9,"windowDurationMins":60,"resetsAt":1800000000}}}}\n' "$id" ;;
        fail) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"fixture quota read failed"}}\n' "$id" ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"ordinaryUsageAllowed":true,"accountId":"fixture-account","rateLimits":{"limitId":"legacy-single","primary":{"usedPercent":1,"windowDurationMins":60,"resetsAt":1800000000}},"rateLimitsByLimitId":{"limit-b":{"limitId":"limit-b","limitName":"B","planType":"pro","primary":{"usedPercent":10,"windowDurationMins":300,"resetsAt":1800000100},"secondary":{"usedPercent":20,"windowDurationMins":10080,"resetsAt":1800600000},"credits":{"hasCredits":true,"unlimited":false,"balance":"12.5 credits"}},"limit-a":{"limitId":"limit-a","ordinaryUsageAllowed":false,"primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":1800000000}}}}}\n' "$id" ;;
      esac ;;
    *'"method":"account/login/start"'*)
      case "$line" in
        *'"params":{"type":"chatgpt"}'*)
          n=0; if [ -f "$1.counter" ]; then n=$(cat "$1.counter"); fi; n=$((n+1)); printf '%s' "$n" > "$1.counter"
          scenario=$(next_scenario "$queue.login")
          case "$scenario" in
            wrong-response-type) printf '{"jsonrpc":"2.0","id":%s,"result":{"type":"apiKey","loginId":"fixture-login-%s","authUrl":"https://example.invalid/auth"}}\n' "$id" "$n" ;;
            *) printf '{"jsonrpc":"2.0","id":%s,"result":{"type":"chatgpt","loginId":"fixture-login-%s","authUrl":"https://example.invalid/auth"}}\n' "$id" "$n" ;;
          esac
          case "$scenario" in
            missing-id) ( sleep 0.2; printf '{"jsonrpc":"2.0","method":"account/login/completed","params":{"success":true,"error":null,"onboardingEntrypoint":null}}\n' ) & ;;
            duplicate-idless) ( sleep 0.2; printf '{"jsonrpc":"2.0","method":"account/login/completed","params":{"loginId":"fixture-login-%s","success":true,"error":null,"onboardingEntrypoint":null}}\n' "$n"; sleep 0.05; printf '{"jsonrpc":"2.0","method":"account/login/completed","params":{"success":true,"error":null,"onboardingEntrypoint":null}}\n' ) & ;;
            pending) : ;;
            failed-sensitive) ( sleep 0.2; printf '{"jsonrpc":"2.0","method":"account/login/completed","params":{"loginId":"fixture-login-%s","success":false,"error":"authorization_code=SUPERSECRET1234567890 refresh_token=abcdef0123456789abcdef0123456789","onboardingEntrypoint":null}}\n' "$n" ) & ;;
            *) ( sleep 0.2; printf '{"jsonrpc":"2.0","method":"account/login/completed","params":{"loginId":"fixture-login-%s","success":true,"error":null,"onboardingEntrypoint":null}}\n' "$n" ) & ;;
          esac
          ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"Invalid request: missing field type"}}\n' "$id" ;;
      esac ;;
    *'"method":"account/login/cancel"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"cancelled":true}}\n' "$id" ;;
    *'"method":"account/logout"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"local":"cleared","remote":"revoked"}}\n' "$id" ;;
    *'"method":"thread/start"'*)
      if [ -f "$queue.active" ]; then
        active_pid=$(cat "$queue.active")
        if kill -0 "$active_pid" 2>/dev/null; then printf 'active helper %s\n' "$active_pid" >> "$queue.overlap"; else rm -f "$queue.active"; fi
      fi
      n=0; if [ -f "$queue.thread.counter" ]; then n=$(cat "$queue.thread.counter"); fi; n=$((n+1)); printf '%s' "$n" > "$queue.thread.counter"
      thread_id="fixture-thread-$n"
      case "$line" in *'"ephemeral":true'*) ephemeral=true ;; *) ephemeral=false ;; esac
      model=$(printf '%s' "$line" | sed -n 's/.*"model":"\([^"]*\)".*/\1/p')
      model_provider=$(printf '%s' "$line" | sed -n 's/.*"modelProvider":"\([^"]*\)".*/\1/p')
      sandbox=$(printf '%s' "$line" | sed -n 's/.*"sandbox":"\([^"]*\)".*/\1/p')
      printf 'thread ephemeral=%s model=%s sandbox=%s\n' "$ephemeral" "$model" "$sandbox" >> "$queue.generation-params"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"thread":{"id":"%s"},"modelProvider":"%s"}}\n' "$id" "$thread_id" "$model_provider" ;;
    *'"method":"turn/start"'*)
      thread_id=$(printf '%s' "$line" | sed -n 's/.*"threadId":"\([^"]*\)".*/\1/p')
      turn_id="$thread_id-turn"
      scenario=success; if [ -f "$queue.generation" ]; then scenario=$(sed -n '1p' "$queue.generation"); fi
      if [ "$scenario" = delayed-start-ack ] || [ "$scenario" = lost-start-ack ]; then
        printf 'accepted\n' > "$queue.turn-start-accepted"
      fi
      if [ "$scenario" = delayed-start-ack ]; then sleep 0.15; fi
      if [ "$scenario" = lost-start-ack ]; then
        while IFS= read -r ignored; do :; done
        continue
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"turn":{"id":"%s","status":"inProgress"}}}\n' "$id" "$turn_id"
      (
        sleep 0.05
        if [ "$scenario" = lifecycle ]; then
          printf '{"jsonrpc":"2.0","method":"item/started","params":{"threadId":"%s","turnId":"%s","startedAtMs":1,"item":{"type":"userMessage","id":"fixture-user","content":[{"type":"text","text":"hello"}]}}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"%s","turnId":"%s","completedAtMs":2,"item":{"type":"userMessage","id":"fixture-user","content":[{"type":"text","text":"hello"}]}}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/started","params":{"threadId":"%s","turnId":"%s","startedAtMs":3,"item":{"type":"reasoning","id":"fixture-reasoning","summary":[],"content":[]}}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/reasoning/summaryPartAdded","params":{"threadId":"%s","turnId":"%s","itemId":"fixture-reasoning","summaryIndex":0,"part":{"type":"summary_text","text":"private reasoning"}}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/reasoning/summaryTextDelta","params":{"threadId":"%s","turnId":"%s","itemId":"fixture-reasoning","summaryIndex":0,"delta":"private reasoning"}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/reasoning/textDelta","params":{"threadId":"%s","turnId":"%s","itemId":"fixture-reasoning","contentIndex":0,"delta":"private reasoning"}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"%s","turnId":"%s","completedAtMs":4,"item":{"type":"reasoning","id":"fixture-reasoning","summary":["private reasoning"],"content":[]}}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/started","params":{"threadId":"%s","turnId":"%s","startedAtMs":5,"item":{"type":"agentMessage","id":"fixture-item","text":""}}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"threadId":"%s","turnId":"%s","itemId":"fixture-item","delta":"fixture"}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"%s","turnId":"%s","completedAtMs":6,"item":{"type":"agentMessage","id":"fixture-item","text":"fixture"}}}\n' "$thread_id" "$turn_id"
        elif [ "$scenario" = tool-activity ]; then
          printf '{"jsonrpc":"2.0","method":"item/started","params":{"threadId":"%s","turnId":"%s","startedAtMs":1,"item":{"type":"commandExecution","id":"fixture-command","command":"echo blocked","cwd":"/tmp","commandActions":[],"status":"inProgress"}}}\n' "$thread_id" "$turn_id"
        elif [ "$scenario" = tool-interrupt-fails ]; then
          printf '%s\n' "$helper_pid" > "$queue.active"
          printf '{"jsonrpc":"2.0","method":"item/started","params":{"threadId":"%s","turnId":"%s","startedAtMs":1,"item":{"type":"commandExecution","id":"fixture-command","command":"echo blocked","cwd":"/tmp","commandActions":[],"status":"inProgress"}}}\n' "$thread_id" "$turn_id"
        elif [ "$scenario" = dynamic-tools ]; then
          printf '{"jsonrpc":"2.0","id":"tool-rpc-1","method":"item/tool/call","params":{"threadId":"%s","turnId":"%s","callId":"internal-1","tool":"lookup","arguments":{"key":"one"}}}\n' "$thread_id" "$turn_id"
          printf '{"jsonrpc":"2.0","id":"tool-rpc-2","method":"item/tool/call","params":{"threadId":"%s","turnId":"%s","callId":"internal-2","tool":"lookup","arguments":{"key":"two"}}}\n' "$thread_id" "$turn_id"
          while [ ! -f "$queue.tool-responses" ] || [ "$(wc -l < "$queue.tool-responses")" -lt 2 ]; do sleep 0.01; done
          printf '{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"threadId":"%s","turnId":"%s","itemId":"fixture-item","delta":"continued"}}\n' "$thread_id" "$turn_id"
        elif [ "$scenario" = partial-disconnect ]; then
          printf '{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"threadId":"%s","turnId":"%s","itemId":"fixture-item","delta":"partial"}}\n' "$thread_id" "$turn_id"
          sleep 0.05
          kill "$helper_pid"
          exit 0
        else
          printf '{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"threadId":"%s","turnId":"%s","itemId":"fixture-item","delta":"fixture"}}\n' "$thread_id" "$turn_id"
          if [ "$scenario" = hold ]; then printf 'started\n' > "$queue.event-consumption-started"; fi
        fi
        if [ "$scenario" = slow-success ]; then sleep 0.25; fi
        if [ "$scenario" = hold ]; then sleep 5; fi
        if [ "$scenario" = tool-interrupt-fails ]; then
          : # Keep this turn active until the app-owned helper is reaped.
        elif [ "$scenario" = partial-failure ]; then
          printf '{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"%s","turn":{"id":"%s","status":"failed","error":{"message":"fixture turn failure"}}}}\n' "$thread_id" "$turn_id"
        else
          status=completed
          if [ "$scenario" = hold ]; then status=interrupted; fi
          printf '{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"%s","turn":{"id":"%s","status":"%s"}}}\n' "$thread_id" "$turn_id" "$status"
          if [ "$scenario" = slow-success ]; then printf 'completed\n' >> "$queue.generation.done"; fi
        fi
      ) &
      generation_pid=$! ;;
    *'"method":"turn/interrupt"'*)
      thread_id=$(printf '%s' "$line" | sed -n 's/.*"threadId":"\([^"]*\)".*/\1/p')
      turn_id=$(printf '%s' "$line" | sed -n 's/.*"turnId":"\([^"]*\)".*/\1/p')
      if [ "$scenario" = "tool-interrupt-fails" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"fixture interrupt failed"}}\n' "$id"
      else
        if [ -n "$generation_pid" ]; then kill "$generation_pid" 2>/dev/null; fi
        printf '{"jsonrpc":"2.0","id":%s,"result":{"interrupted":true}}\n' "$id"
        printf '{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"%s","turn":{"id":"%s","status":"interrupted"}}}\n' "$thread_id" "$turn_id"
      fi ;;
    *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"no such method; authorization_code=SUPERSECRET1234567890 refresh_token=abcdef0123456789abcdef0123456789"}}\n' "$id" ;;
  esac
done
"#;

    fn fixture_launch(directory: &Path, env_log: &Path) -> HelperLaunch {
        let script = directory.join("fake-codex-app-server");
        std::fs::write(&script, FIXTURE_HELPER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        HelperLaunch { program: script, args: vec![env_log.to_string_lossy().into_owned()] }
    }

    #[derive(Clone, Debug)]
    struct ApiDispatchSeen {
        provider_id: String,
        model_id: String,
        protocol: Protocol,
        url: String,
        authorization: String,
        body: Value,
    }

    struct RecordingDispatcher(Arc<Mutex<Vec<ApiDispatchSeen>>>);

    impl crate::dispatch::Dispatcher for RecordingDispatcher {
        fn send<'a>(&'a self,
            target: crate::dispatch::Target<'a>, request: reqwest::RequestBuilder) -> futures_util::future::BoxFuture<'a, anyhow::Result<reqwest::Response>> {
            let seen = self.0.clone();
            let provider_id = target.provider.id.clone();
            let model_id = target.model_id.to_owned();
            let protocol = target.protocol;
            Box::pin(async move {
                let request = request.build()?;
                let authorization = request.headers().get(reqwest::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()).unwrap_or("").to_owned();
                let body = request.body().and_then(reqwest::Body::as_bytes)
                    .and_then(|bytes| serde_json::from_slice(bytes).ok()).unwrap_or(Value::Null);
                seen.lock().unwrap().push(ApiDispatchSeen {
                    provider_id, model_id, protocol, url: request.url().to_string(), authorization, body });
                anyhow::bail!("controlled dispatcher stopped at the API boundary")
            })
        }
    }

    fn admitted_gateway_store(
        database: &Path,
        adapter: Arc<CodexAdapter>,
        api_base_url: &str) -> Arc<crate::config::ConfigStore> {
        admitted_gateway_store_with_dispatcher(
            database,
            adapter,
            Arc::new(crate::dispatch::ApiDispatcher { loopback_only: false }),
            api_base_url)
    }

    fn admitted_gateway_store_with_dispatcher(
        database: &Path,
        adapter: Arc<CodexAdapter>,
        dispatcher: Arc<dyn crate::dispatch::Dispatcher>,
        api_base_url: &str,
    ) -> Arc<crate::config::ConfigStore> {
        use crate::subscription::{
            Capability, CapabilityStatus, CatalogEvidence, ConnectionState, Evidence, EvidenceState,
            QuotaBucket, QuotaCredits, QuotaEvidence, QuotaPermission, QuotaView, QuotaWindow,
        };
        let store = Arc::new(crate::config::ConfigStore::load_with_adapters(
            database.to_path_buf(),
            dispatcher,
            adapter).unwrap());
        store.update(|config| {
            config.port = 0;
            config.gateway.proxy_mode = "direct".into();
            config.providers[0].base_url = api_base_url.into();
            let provider = crate::config::Provider {
                preset: String::new(), api_type: String::new(), test_model: String::new(),
                id: "codex-fixture".into(), name: "Codex fixture".into(),
                kind: crate::config::ProviderKind::CodexSubscription,
                base_url: String::new(), enabled: true, has_api_key: false,
            };
            config.providers.push(provider.clone());
            let mut model = config.models[0].clone();
            model.id = "codex-fixture-binding".into();
            model.provider_id = provider.id.clone();
            model.model_id = "codex-fixture-model".into();
            model.name = "Codex fixture model".into();
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
                helper_version: Some("fixture-helper-1.0".into()),
                account_path: Some("isolated-fixture".into()),
                models: vec![],
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
                    view: QuotaView::RateLimits,
                    buckets: vec![QuotaBucket {
                        limit_id: "included".into(),
                        permission: QuotaPermission::Allowed,
                        windows: vec![QuotaWindow { label: "primary".into(), used_percent: Some(10.0), window_minutes: Some(60), ..Default::default() }],
                        credits: Some(QuotaCredits { permission: QuotaPermission::Denied, ..Default::default() }),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                catalog: CatalogEvidence { state: EvidenceState::Available, source: Some("controlled fixture".into()), ..Default::default() },
            });
            config.subscription_catalogs.insert(provider.id.clone(), crate::subscription_catalog::ProviderCatalog {
                entries: vec![crate::subscription_catalog::CatalogEntry {
                    model_id: model.model_id.clone(), name: Some(model.name.clone()), internal_id: model.id.clone(),
                    availability: crate::subscription_catalog::Availability::Available,
                    first_seen: Some("fixture".into()), last_confirmed: Some("fixture".into()),
                    confirmed_generation: Some(connection.generation), account: connection.identity.clone(),
                }],
            },
                );
        }).unwrap();
        store.write_secret("provider:openrouter", "fixture-api-key").unwrap();
        // Gateway integration fixtures represent a user-confirmed finite fake run. Production
        // configuration stays default-off; tests that need to reach later admission gates opt in here.
        store.update(|config| {
            crate::subscription::set_codex_real_generation_enabled(
                config,
                "codex-fixture",
                true,
                Some(crate::subscription::CODEX_REAL_GENERATION_MAX_CALLS),
            ).unwrap();
        }).unwrap();
        store
    }

    /// 写入只读场景队列：每个方法一行一个场景名，按顺序消费（没有文件时走默认场景）。
    fn scenario_launch(directory: &Path, env_log: &Path, catalog: &[&str], quota: &[&str]) -> HelperLaunch {
        let base = env_log.to_string_lossy().into_owned();
        if !catalog.is_empty() {
            std::fs::write(format!("{base}.catalog"), catalog.join("\n")).unwrap();
        }
        if !quota.is_empty() {
            std::fs::write(format!("{base}.quota"), quota.join("\n")).unwrap();
        }
        fixture_launch(directory, env_log)
    }

    // Runs the production adapter and refresh against an isolated stdio helper, never OAuth.
    fn refresh_fixture(accounts: &[&str]) -> (crate::config::ConfigStore, tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        std::fs::write(log.with_extension("log.account"), accounts.join("\n")).unwrap();
        let adapter = std::sync::Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let store = crate::config::ConfigStore::load_with_adapters(
            home.path().join("app.db"),
            std::sync::Arc::new(crate::dispatch::ApiDispatcher { loopback_only: true }), adapter,
        ).unwrap();
        store.update(|config| {
            let mut provider = config.providers[0].clone();
            provider.id = "codex".into();
            provider.kind = ProviderKind::CodexSubscription;
            provider.enabled = true;
            config.providers.push(provider);
            crate::subscription::sync_provider(config, "codex", &ProviderKind::CodexSubscription);
            let mut model = config.models[0].clone();
            model.provider_id = "codex".into();
            model.model_id = "preset-a".into();
            config.models.push(model);
        }).unwrap();
        (store, home, log)
    }

    #[tokio::test]
    async fn refresh_helper_disconnect_invalidates_connected_account_without_app_logout() {
        use crate::subscription::{admit_model, refresh};
        let (store, _home, log) = refresh_fixture(&["connected", "signed-out"]);
        let before = refresh(&store, "codex").await.unwrap();
        assert_eq!(before.state, ConnectionState::Connected);
        assert!(before.current_evidence().is_some());
        let _ = refresh(&store, "codex").await;
        let config = store.read();
        let after = &config.subscriptions["codex"];
        assert_eq!(after.state, ConnectionState::NotConnected);
        assert!(after.generation > before.generation);
        assert!(after.identity.is_none() && after.evidence.is_none());
        assert!(after.current_evidence().is_none());
        let provider = config.providers.iter().find(|p| p.id == "codex").unwrap();
        let model = config.models.iter().find(|m| m.provider_id == "codex").unwrap();
        assert_eq!(admit_model(&config, model, provider, crate::protocol::Protocol::Chat).unwrap_err().code, "not_connected");
        assert_eq!(std::fs::read_to_string(log.with_extension("log.catalog.counter")).unwrap(), "1");
    }

    #[tokio::test]
    async fn refresh_helper_incomplete_identity_retains_only_history_then_recovers_same_account() {
        use crate::subscription::{refresh, views, SessionState};
        let (store, _home, log) = refresh_fixture(&["connected", "incomplete", "connected"]);
        std::fs::write(log.with_extension("log.catalog"), "success\nfail").unwrap();
        std::fs::write(log.with_extension("log.quota"), "multi\nfail").unwrap();
        let before = refresh(&store, "codex").await.unwrap();
        let _ = refresh(&store, "codex").await;
        let after = store.read().subscriptions["codex"].clone();
        assert_ne!(after.state, ConnectionState::Connected);
        assert!(after.current_evidence().is_none());
        assert_eq!(after.identity, before.identity);
        let history = after.evidence.as_ref().unwrap();
        let old = before.evidence.as_ref().unwrap();
        assert_eq!(history.models, old.models);
        assert_eq!(history.catalog.observed_at, old.catalog.observed_at);
        assert_eq!(history.quota.buckets, old.quota.buckets);
        assert_eq!(history.quota.observed_at, old.quota.observed_at);
        let view = views(&store.read(), true, &SessionState::default()).into_iter().find(|v| v.provider_id == "codex").unwrap();
        assert_eq!(view.catalog.state, EvidenceState::Stale);
        assert_eq!(view.quota.state, EvidenceState::Failed);
        assert!(view.quota.history);
        let restored = refresh(&store, "codex").await.unwrap();
        assert_eq!(restored.state, ConnectionState::Connected);
        let restored_evidence = restored.current_evidence().unwrap();
        assert_eq!(restored_evidence.catalog.state, EvidenceState::Stale);
        assert_eq!(restored_evidence.models, old.models);
        assert_eq!(restored_evidence.catalog.observed_at, old.catalog.observed_at);
        assert_eq!(restored_evidence.quota.state, EvidenceState::Failed);
        assert!(restored_evidence.quota.history);
        assert_eq!(restored_evidence.quota.buckets, old.quota.buckets);
        assert_eq!(restored_evidence.quota.observed_at, old.quota.observed_at);
    }

    #[test]
    fn account_status_distinguishes_signed_out_from_incomplete_and_auth_requirement() {
        for (response, connected, incomplete) in [
            (json!({"account":{"type":"chatgpt","email":"A@example.invalid"},"requiresOpenaiAuth":true}), true, false,
            ),
            (json!({"account":null,"requiresOpenaiAuth":true}), false, false),
            (json!({"requiresAuth":true}), false, false),
            (json!({"account":{"type":"chatgpt","email":null},"requiresOpenaiAuth":true}), false, true),
            (json!({"account":{"type":"chatgpt","email":" "},"requiresOpenaiAuth":true}), false, true),
            (json!({"account":{"type":"apiKey"},"requiresOpenaiAuth":false}), false, true),
            (json!({}), false, true),
            (json!({"account":null}), false, true),
            (json!({"account":{"type":"chatgpt","email":"A@example.invalid"}}), false, true),
        ] {
            let (state, identity, actual_incomplete) = account_status(&response);
            assert_eq!(state == ConnectionState::Connected, connected, "{response}");
            assert_eq!(identity.is_some(), connected, "{response}");
            assert_eq!(actual_incomplete, incomplete, "{response}");
        }
    }

    #[test]
    fn helper_home_is_provider_scoped_and_sanitized() {
        let root = Path::new("/tmp/fixture-root");
        assert_eq!(helper_home_in(root, "codex"), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/codex"));
        assert_eq!(helper_home_in(root, "work/../home"), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/work____home"));
        assert_eq!(helper_home_in(root, ""), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/provider"));
        assert_eq!(helper_home_in(root, "codex-work_1"), PathBuf::from("/tmp/fixture-root/.autojev/helpers/codex/codex-work_1"));
        if let Some(home) = crate::runtime::home_dir() {
            let home_path = helper_home("codex");
            assert_eq!(home_path, home.join(".autojev/helpers/codex/codex"));
            assert_ne!(home_path, home.join(".codex"));
            assert!(!home_path.starts_with(home.join(".codex")));
        }
    }

    #[test]
    fn official_codex_resolution_uses_path_then_known_locations() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("codex");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([directory.path()]).unwrap();
        assert_eq!(resolve_official(Some(path.as_os_str()), &[]), Some(script.clone()));
        let empty = tempfile::tempdir().unwrap();
        let empty_path = std::env::join_paths([empty.path()]).unwrap();
        assert_eq!(resolve_official(Some(empty_path.as_os_str()), &[]), None);
        assert_eq!(resolve_official(None, &[directory.path().join("missing-codex")]), None);
        assert_eq!(resolve_official(None, &[script.clone()]), Some(script));
    }

    #[test]
    fn the_official_helper_is_started_as_a_stdio_app_server() {
        let launch = official_launch(PathBuf::from("/fixture/bin/codex"));
        assert_eq!(launch.program, PathBuf::from("/fixture/bin/codex"));
        assert_eq!(launch.args, vec!["app-server".to_owned()]);
        // 本机解析到官方 codex 时，真实启动描述也必须带同一子命令。
        if let Ok(resolved) = resolve_launch() {
            assert_eq!(resolved.args, vec!["app-server".to_owned()], "production must start codex app-server");
        }
    }

    #[cfg(not(feature = "isolation-check"))]
    #[test]
    fn production_has_no_switch_to_install_a_stub() {
        // 覆盖入口在非 isolation-check 构建里被编译成恒 None，也没有任何配置或环境开关。
        assert!(override_launch().is_none());
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("env.log");
        let launch = fixture_launch(directory.path(), &log);
        if let Ok(resolved) = resolve_launch() {
            assert_ne!(resolved.program, launch.program);
        }
    }

    #[test]
    fn secrets_are_never_written_verbatim() {
        let text = redact("login failed authorization_code=SUPERSECRET1234567890 refresh_token=abcdef0123456789abcdef0123456789");
        assert!(!text.contains("SUPERSECRET"), "{text}");
        assert!(!text.contains("abcdef0123456789"), "{text}");
        assert_eq!(redact("plain short words"), "plain short words");
        let value = redact_json(&json!({
            "error": "token=abcdef0123456789abcdef0123456789",
            "authorization_code": "SUPERSECRET1234567890",
            "code": -32601,
            "authorizationUrl": "https://example.invalid/auth",
        }));
        assert_eq!(value["authorization_code"], "[redacted]");
        assert_eq!(value["code"], -32601);
        assert!(!value["error"].as_str().unwrap().contains("abcdef"));
        assert_eq!(value["authorizationUrl"], "https://example.invalid/auth");
    }

    #[test]
    fn redaction_never_panics_on_multibyte_text() {
        // 单个无空格长词，每个汉字 3 字节：400 一定落在多字节字符中间（旧实现会 panic）。
        let long = "汉字混合文本".repeat(30);
        assert!(long.len() > 400 && !long.is_char_boundary(400));
        let redacted = redact(&long);
        assert!(redacted.ends_with('…'), "{redacted}");
        assert!(redacted.len() <= 403, "{}", redacted.len());
        // 输出仍是合法 UTF-8，且未把字符切坏。
        assert_eq!(redacted, String::from_utf8(redacted.clone().into_bytes()).unwrap());
        let mixed = format!("{long} authorization_code=SUPERSECRET1234567890");
        let redacted = redact(&mixed);
        assert!(!redacted.contains("SUPERSECRET"), "{redacted}");
        assert!(redacted.ends_with('…'));
    }

    #[tokio::test]
    async fn start_uses_a_dedicated_codex_home_and_strips_credentials() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut server = CodexAppServer::start_in(home.path(), "codex-fixture", fixture_launch(home.path(), &log)).await.unwrap();
        assert_eq!(server.version(), Some("0.159.0"));
        assert_eq!(server.user_agent(), Some("codex_cli_rs/0.159.0 (Test OS; x86_64) rust"));
        let expected = helper_home_in(home.path(), "codex-fixture");
        assert_eq!(server.auth_home(), expected.as_path());
        assert!(expected.starts_with(home.path().join(".autojev/helpers/codex")));
        assert_ne!(expected, crate::runtime::home_dir().unwrap_or_default().join(".codex"));
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(recorded.contains(&format!("CODEX_HOME={}", expected.display())), "{recorded}");
        assert!(recorded.contains("OPENAI_API_KEY=<unset>"), "credential variables must be stripped: {recorded}");
        server.shutdown();
    }

    #[tokio::test]
    async fn rpc_round_trip_routes_notifications_and_redacts_errors() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut server = CodexAppServer::start_in(home.path(), "codex-fixture", fixture_launch(home.path(), &log)).await.unwrap();
        let read = server.call("account/read", json!({})).await.unwrap();
        assert_eq!(read["account"]["email"], "fixture@example.invalid");
        let start = server.call("account/login/start", json!({"type": "chatgpt"})).await.unwrap();
        assert_eq!(start["type"], "chatgpt");
        assert_eq!(start["loginId"], "fixture-login-1");
        assert_eq!(start["authUrl"], "https://example.invalid/auth");
        let completed = server.next_notification().await.expect("a completion notification");
        assert_eq!(completed["method"], "account/login/completed");
        assert_eq!(completed["params"]["loginId"], "fixture-login-1");
        assert_eq!(completed["params"]["success"], true);
        assert!(completed["params"].get("account").is_none(), "the official completion notification carries no account identity");
        let error = server.call("fixture/unknown", json!({})).await.unwrap_err().to_string();
        assert!(!error.contains("SUPERSECRET"), "{error}");
        assert!(!error.contains("abcdef0123456789"), "{error}");
        assert!(error.contains("[redacted]"), "{error}");
        server.shutdown();
    }

    #[tokio::test]
    async fn shutdown_reaps_only_its_own_child() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut server = CodexAppServer::start_in(home.path(), "codex-fixture", fixture_launch(home.path(), &log)).await.unwrap();
        let pid = server.pid().expect("a live helper has a pid");
        let mut unrelated = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        server.shutdown();
        let status = std::process::Command::new("ps").arg("-p").arg(pid.to_string()).status().unwrap();
        assert!(!status.success(), "the owned helper must be terminated and reaped");
        assert!(unrelated.try_wait().unwrap().is_none(), "an unrelated process must not be touched");
        let _ = unrelated.kill();
        let _ = unrelated.wait();
    }

    #[tokio::test]
    async fn adapter_login_verify_logout_and_generates_text() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        assert!(adapter.available());
        assert!(adapter.helper_status().available);
        let start = adapter.start_login("codex-fixture", 1).await.unwrap();
        assert_eq!(start.login_id, "fixture-login-1");
        assert_eq!(start.authorization_url.as_deref(), Some("https://example.invalid/auth"));
        let status = adapter.helper_status();
        assert_eq!(status.user_agent.as_deref(), Some("codex_cli_rs/0.159.0 (Test OS; x86_64) rust"));
        assert_eq!(status.version.as_deref(), Some("0.159.0"));
        assert!(status.auth_home.as_deref().unwrap_or_default().contains("codex-fixture"));
        let result = adapter.login_result("codex-fixture", 1).await.unwrap();
        assert_eq!(
            result,
            Some(LoginResult::Completed {
                identity: "fixture@example.invalid".into()
            })
        );
        let read = adapter.status("codex-fixture", 1).await.unwrap();
        assert_eq!(read.state, ConnectionState::Connected);
        assert_eq!(read.identity.as_deref(), Some("fixture@example.invalid"));
        // A controlled app-server fixture drives the real adapter generation path.
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "codex-fixture",
                generation: 1,
                model_id: "fixture-model",
                protocol: crate::protocol::Protocol::Chat,
                body: json!({"model":"fixture-model","messages":[{"role":"user","content":"hello"}]}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = events.next().await {
            observed.push(event);
        }
        assert_eq!(observed, vec![
            crate::subscription::GenerationEvent::Started { generation: 1 },
            crate::subscription::GenerationEvent::Chunk("fixture".into()),
            crate::subscription::GenerationEvent::Finished { status: 200 },
        ]);
        let outcome = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(outcome.local_cleared);
        assert_eq!(outcome.remote, RemoteRevocation::Revoked);
        // 没有活动会话时也要清理：再次退出不报错，远端撤销如实记为失败。
        let again = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(again.local_cleared);
        assert_eq!(again.remote, RemoteRevocation::Failed);
    }

    #[tokio::test]
    async fn adapter_round_trips_parallel_client_tools_through_the_live_helper_turn() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("tools.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "dynamic-tools").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let tools = json!([{"type":"function","function":{"name":"lookup","description":"Lookup a value","parameters":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}}}]);
        let initial_body = json!({"model":"fixture-model","messages":[{"role":"user","content":"lookup"}],"tools":tools});
        let mut initial = adapter
            .generate(GenerationRequest {
                provider_id: "codex-tools",
                generation: 9,
                model_id: "fixture-model",
                protocol: Protocol::Chat,
                body: initial_body,
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert_eq!(initial.next().await, Some(GenerationEvent::Started { generation: 9 }));
        let calls = match initial.next().await {
            Some(GenerationEvent::ToolCalls { calls }) => calls,
            other => panic!("expected client-owned tool calls, got {other:?}"),
        };
        assert_eq!(calls.len(), 2, "parallel helper requests belong to the same API tool-call batch");
        assert!(calls.iter().all(|call| call.name == "lookup" && call.id.starts_with("call_ajv1_9_")));
        let call_by_key: HashMap<_, _> = calls
            .iter()
            .map(|call| {
                let arguments: Value = serde_json::from_str(&call.arguments).unwrap();
                (arguments["key"].as_str().unwrap().to_owned(), call.id.clone())
            })
            .collect();
        assert_eq!(call_by_key.len(), 2);

        let call_history: Vec<_> = calls
            .iter()
            .map(|call| {
                json!({
                    "id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}
                })
            })
            .collect();
        let mut messages = vec![
            json!({"role":"user","content":"lookup"}),
            json!({"role":"assistant","content":null,"tool_calls":call_history}),
        ];
        messages.push(json!({"role":"tool","tool_call_id":call_by_key["one"],"content":"one-result"}));
        messages.push(json!({"role":"tool","tool_call_id":call_by_key["two"],"content":"two-result"}));
        let mut followup = adapter
            .generate(GenerationRequest {
                provider_id: "codex-tools",
                generation: 9,
                model_id: "fixture-model",
                protocol: Protocol::Chat,
                body: json!({"model":"fixture-model","messages":messages,"tools":tools}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        let mut completed = Vec::new();
        while let Some(event) = followup.next().await {
            completed.push(event);
        }
        assert_eq!(
            completed,
            vec![
                GenerationEvent::Started { generation: 9 },
                GenerationEvent::Chunk("continued".into()),
                GenerationEvent::Finished { status: 200 },
            ]
        );

        let rpc_lines = std::fs::read_to_string(format!("{}.rpc", log.to_string_lossy())).unwrap();
        let rpc: Vec<Value> = rpc_lines.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let starts: Vec<_> = rpc.iter().filter(|value| value["method"] == "thread/start").collect();
        assert_eq!(starts.len(), 1, "the tool continuation must resume rather than replay a turn");
        assert_eq!(starts[0].pointer("/params/dynamicTools/0/name"), Some(&json!("lookup")));
        assert_eq!(rpc.iter().filter(|value| value["method"] == "turn/start").count(), 1);
        assert_eq!(adapter.pending_tool_turns.lock().unwrap().len(), 0);

        let responses = std::fs::read_to_string(format!("{}.tool-responses", log.to_string_lossy())).unwrap();
        let responses: Vec<Value> = responses.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(responses.len(), 2);
        let by_rpc_id: HashMap<_, _> = responses.iter().map(|response| (response["id"].as_str().unwrap(), &response["result"])).collect();
        for (rpc_id, expected) in [("tool-rpc-1", "one-result"), ("tool-rpc-2", "two-result")] {
            assert_eq!(by_rpc_id[rpc_id]["success"], true);
            assert_eq!(by_rpc_id[rpc_id]["contentItems"][0]["text"], expected);
        }
        assert!(
            rpc.iter().all(|value| value["method"] != "commandExecution" && value["method"] != "item/commandExecution"),
            "client functions stay client-owned; the helper must not execute local commands"
        );
        adapter.logout("codex-tools", 9).await.unwrap();
    }

    #[tokio::test]
    async fn adapter_discards_partial_tool_results_without_replaying_the_call() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("partial-tools.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "dynamic-tools").unwrap();
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let tools = json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"key":{"type":"string"}}}}}]);
        let mut initial = adapter
            .generate(GenerationRequest {
                provider_id: "codex-partial-tools",
                generation: 12,
                model_id: "fixture-model",
                protocol: Protocol::Chat,
                body: json!({"model":"fixture-model","messages":[{"role":"user","content":"lookup"}],"tools":tools.clone()}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert_eq!(initial.next().await, Some(GenerationEvent::Started { generation: 12 }));
        let calls = match initial.next().await {
            Some(GenerationEvent::ToolCalls { calls }) => calls,
            other => panic!("expected client-owned tool calls, got {other:?}"),
        };
        assert_eq!(calls.len(), 2);
        let history: Vec<_> = calls
            .iter()
            .map(|call| {
                json!({
                    "id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}
                })
            })
            .collect();
        let body = json!({"model":"fixture-model","messages":[
            {"role":"user","content":"lookup"},
            {"role":"assistant","content":null,"tool_calls":history},
            {"role":"tool","tool_call_id":calls[0].id,"content":"only one result"}
        ],"tools":tools});
        let partial = match adapter
            .generate(GenerationRequest {
                provider_id: "codex-partial-tools",
                generation: 12,
                model_id: "fixture-model",
                protocol: Protocol::Chat,
                body: body.clone(),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
        {
            Ok(_) => panic!("an incomplete tool-result batch must be rejected"),
            Err(error) => error,
        };
        assert!(partial.to_string().contains("incomplete"), "{partial}");
        assert!(adapter.pending_tool_turns.lock().unwrap().is_empty());

        let replay = match adapter
            .generate(GenerationRequest {
                provider_id: "codex-partial-tools",
                generation: 12,
                model_id: "fixture-model",
                protocol: Protocol::Chat,
                body,
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
        {
            Ok(_) => panic!("discarded client tool calls must never be replayed"),
            Err(error) => error,
        };
        assert!(replay.to_string().contains("matching live Codex turn"), "{replay}");
        let rpc_lines = std::fs::read_to_string(format!("{}.rpc", log.to_string_lossy())).unwrap();
        let rpc: Vec<Value> = rpc_lines.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(rpc.iter().filter(|value| value["method"] == "thread/start").count(), 1);
        assert_eq!(rpc.iter().filter(|value| value["method"] == "turn/start").count(), 1);
        assert_eq!(adapter.pending_tool_turns.lock().unwrap().len(), 0);
        adapter.logout("codex-partial-tools", 12).await.unwrap();
    }

    #[tokio::test]
    async fn generation_keeps_partial_failure_terminal() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "partial-failure").unwrap();
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let mut events = adapter.generate(GenerationRequest {
            provider_id: "codex-partial",
            generation: 4,
            model_id: "fixture-model",
            protocol: Protocol::Chat,
            body: json!({"model":"fixture-model","messages":[{"role":"user","content":"hello"}]}),
            pre_dispatch_check: Arc::new(|| Ok(())),
        }).await.unwrap();
        let mut observed = Vec::new();
        while let Some(event) = events.next().await { observed.push(event); }
        assert_eq!(observed, vec![
            GenerationEvent::Started { generation: 4 },
            GenerationEvent::Chunk("fixture".into()),
            GenerationEvent::Failed { message: "fixture turn failure".into() },
        ]);
    }

    #[tokio::test]
    async fn generation_accepts_text_and_reasoning_item_lifecycles_but_rejects_tool_items() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("lifecycle.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "lifecycle").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "codex-lifecycle",
                generation: 1,
                model_id: "fixture-model",
                protocol: Protocol::Responses,
                body: json!({"model":"fixture-model","input":"hello"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = events.next().await {
            observed.push(event);
        }
        assert_eq!(observed, vec![
            GenerationEvent::Started { generation: 1 },
            GenerationEvent::Chunk("fixture".into()),
            GenerationEvent::Finished { status: 200 },
        ], "ordinary message/reasoning lifecycle notifications must be ignored without leaking reasoning");

        std::fs::write(
            format!("{}.generation", log.to_string_lossy()),
            "tool-activity")
        .unwrap();
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "codex-tool-lifecycle",
                generation: 2,
                model_id: "fixture-model",
                protocol: Protocol::Responses,
                body: json!({"model":"fixture-model","input":"hello"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert_eq!(
            events.next().await,
            Some(GenerationEvent::Started { generation: 2 })
        );
        assert!(
            matches!(events.next().await, Some(GenerationEvent::Failed { message }) if message.contains("tool activity"))
        );
    }

    #[tokio::test]
    async fn queued_generation_rechecks_admission_before_starting_a_thread() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("queued-admission.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "hold").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(),
            fixture_launch(home.path(), &log)));
        let mut active = adapter
            .generate(GenerationRequest {
                provider_id: "codex-queued-admission",
                generation: 1,
                model_id: "fixture-model",
                protocol: Protocol::Responses,
                body: json!({"model":"fixture-model","input":"active"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert_eq!(
            active.next().await,
            Some(GenerationEvent::Started { generation: 1 })
        );
        assert_eq!(
            active.next().await,
            Some(GenerationEvent::Chunk("fixture".into()))
        );

        let admitted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queued = {
            let adapter = adapter.clone();
            let admitted = admitted.clone();
            let checks = checks.clone();
            tokio::spawn(async move {
                let result = adapter
                    .generate(GenerationRequest {
                        provider_id: "codex-queued-admission",
                        generation: 1,
                        model_id: "fixture-model",
                        protocol: Protocol::Responses,
                        body: json!({"model":"fixture-model","input":"queued"}),
                        pre_dispatch_check: Arc::new(move || {
                            checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            if admitted.load(std::sync::atomic::Ordering::SeqCst) {
                                Ok(())
                            } else {
                                Err("The live subscription admission changed".into())
                            }
                        }),
                    })
                    .await;
                let rejected = result.is_err();
                drop(result);
                rejected
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            checks.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the queued request must still be waiting on the active turn lock"
        );
        admitted.store(false, std::sync::atomic::Ordering::SeqCst);
        drop(active);

        let queued_result = tokio::time::timeout(std::time::Duration::from_secs(3), queued)
            .await
            .unwrap()
            .unwrap();
        assert!(
            queued_result,
            "stale queued admission must be rejected before dispatch"
        );
        assert!(checks.load(std::sync::atomic::Ordering::SeqCst) > 0);
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|method| *method == "thread/start")
                .count(),
            1,
            "the old queued request must not start a thread"
        );
        assert_eq!(
            calls
                .lines()
                .filter(|method| *method == "turn/start")
                .count(),
            1,
            "the old queued request must not start a turn"
        );
    }

    #[tokio::test]
    async fn logout_interrupts_an_active_backpressured_generation_before_revoking_the_helper() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("logout-generation-lock.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "hold").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(),
            fixture_launch(home.path(), &log)));
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "codex-logout-lock",
                generation: 1,
                model_id: "fixture-model",
                protocol: Protocol::Responses,
                body: json!({"model":"fixture-model","input":"active"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert_eq!(
            events.next().await,
            Some(GenerationEvent::Started { generation: 1 })
        );
        assert_eq!(
            events.next().await,
            Some(GenerationEvent::Chunk("fixture".into()))
        );

        let logout = {
            let adapter = adapter.clone();
            tokio::spawn(async move { adapter.logout("codex-logout-lock", 0).await })
        };
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(3), logout)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(outcome.local_cleared);
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert!(
            calls.lines().any(|method| method == "turn/interrupt"),
            "logout must actively interrupt the held stream without relying on downstream polling"
        );
        assert!(calls.lines().any(|method| method == "account/logout"));
        assert_eq!(
            events.next().await,
            Some(GenerationEvent::Cancelled),
            "the retained stream must observe logout cancellation after it is polled again"
        );
    }

    #[tokio::test]
    async fn delayed_turn_start_ack_is_supported_and_lost_ack_cancellation_reaps_helper() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("turn-start-ack.log");
        std::fs::write(
            format!("{}.generation", log.to_string_lossy()),
            "delayed-start-ack")
        .unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(),
            fixture_launch(home.path(), &log)));
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "codex-delayed-ack",
                generation: 1,
                model_id: "fixture-model",
                protocol: Protocol::Responses,
                body: json!({"model":"fixture-model","input":"delayed"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = events.next().await {
            observed.push(event);
        }
        assert_eq!(
            observed,
            vec![
                GenerationEvent::Started { generation: 1 },
                GenerationEvent::Chunk("fixture".into()),
                GenerationEvent::Finished { status: 200 },
            ]
        );

        let accepted = format!("{}.turn-start-accepted", log.to_string_lossy());
        let _ = std::fs::remove_file(&accepted);
        std::fs::write(
            format!("{}.generation", log.to_string_lossy()),
            "lost-start-ack")
        .unwrap();
        let (cleanup_started_tx, cleanup_started_rx) = tokio::sync::oneshot::channel();
        let (cleanup_release_tx, cleanup_release_rx) = tokio::sync::oneshot::channel();
        *adapter.cleanup_gate.lock().unwrap() = Some(CancellationCleanupGate {
            started: cleanup_started_tx,
            release: cleanup_release_rx });
        let adapter_for_call = adapter.clone();
        let request = GenerationRequest {
            provider_id: "codex-lost-ack",
            generation: 2,
            model_id: "fixture-model",
            protocol: Protocol::Responses,
            body: json!({"model":"fixture-model","input":"lost"}),
            pre_dispatch_check: Arc::new(|| Ok(())),
        };
        let pending = tokio::spawn(async move {
            let result = adapter_for_call.generate(request).await;
            drop(result);
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !std::path::Path::new(&accepted).exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture must record that turn/start was accepted before withholding its response");
        let helper_a_id = adapter.active_turns.lock().unwrap()["codex-lost-ack"]
            .helper_instance_id
            .clone()
            .expect("the cancellation guard must bind to the helper that accepted turn/start");
        pending.abort();
        let _ = pending.await;
        tokio::time::timeout(std::time::Duration::from_secs(3), cleanup_started_rx)
            .await
            .expect("cancellation cleanup must start")
            .expect("cleanup task must report that it is holding the generation lock");

        std::fs::write(format!("{}.generation", log.to_string_lossy()), "success").unwrap();
        let request_b = GenerationRequest {
            provider_id: "codex-lost-ack",
            generation: 3,
            model_id: "fixture-model",
            protocol: Protocol::Responses,
            body: json!({"model":"fixture-model","input":"B"}),
            pre_dispatch_check: Arc::new(|| Ok(())),
        };
        let b_start = adapter.generate(request_b);
        tokio::pin!(b_start);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut b_start)
                .await
                .is_err(),
            "B must remain queued while A's accepted but unacknowledged turn is being reaped"
        );
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|method| *method == "turn/start")
                .count(),
            2,
            "B must not be dispatched through A's still-owned helper"
        );
        cleanup_release_tx.send(()).unwrap();
        let mut b_events = tokio::time::timeout(std::time::Duration::from_secs(3), b_start)
            .await
            .expect("B must proceed once A's helper is reaped")
            .unwrap();
        let mut b_observed = Vec::new();
        while let Some(event) = b_events.next().await {
            b_observed.push(event);
        }
        assert_eq!(
            b_observed,
            vec![
                GenerationEvent::Started { generation: 3 },
                GenerationEvent::Chunk("fixture".into()),
                GenerationEvent::Finished { status: 200 },
            ]
        );
        let helper_b_id = adapter.servers.lock().await["codex-lost-ack"].instance_id().to_owned();
        assert_ne!(helper_a_id, helper_b_id, "B must use a new owned helper after A's reaper");
        assert_eq!(
            reap_owned_helper(&adapter.servers, "codex-lost-ack", &helper_a_id).await,
            ReapResult::DifferentInstance,
            "a late cleanup for A must not remove B's helper instance"
        );
        assert_eq!(
            adapter.servers.lock().await["codex-lost-ack"].instance_id(),
            helper_b_id
        );
    }

    #[tokio::test]
    async fn codex_internal_retries_are_zero_and_disconnect_fails_without_replaying_output() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("retry-policy.log");
        std::fs::write(
            format!("{}.generation", log.to_string_lossy()),
            "partial-disconnect")
        .unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let mut events = adapter
            .generate(GenerationRequest {
                provider_id: "codex-retry-policy",
                generation: 1,
                model_id: "fixture-model",
                protocol: Protocol::Responses,
                body: json!({"model":"fixture-model","input":"disconnect"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = events.next().await {
            observed.push(event);
        }
        assert_eq!(observed.len(), 3);
        assert_eq!(observed[0], GenerationEvent::Started { generation: 1 });
        assert_eq!(observed[1], GenerationEvent::Chunk("partial".into()));
        assert!(
            matches!(observed[2], GenerationEvent::Failed { .. }),
            "a disconnect after partial text must terminate this one turn"
        );

        let rpc: Vec<Value> = std::fs::read_to_string(format!("{}.rpc", log.to_string_lossy()))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let thread_start = rpc
            .iter()
            .find(|call| call["method"] == "thread/start")
            .unwrap();
        let provider_id = thread_start["params"]["modelProvider"]
            .as_str()
            .expect("thread must use its request-scoped no-retry provider");
        let provider = &thread_start["params"]["config"]["model_providers"][provider_id];
        assert_eq!(provider["request_max_retries"], 0);
        assert_eq!(provider["stream_max_retries"], 0);
        assert_eq!(provider["requires_openai_auth"], true);
        assert_eq!(provider["supports_websockets"], false);
        assert_eq!(thread_start["params"]["config"]["features"]["unbounded_connection_retries"], false);
        assert_eq!(thread_start["params"]["model"], "fixture-model");
        assert_eq!(
            rpc.iter()
                .filter(|call| call["method"] == "turn/start")
                .count(),
            1,
            "the controlled disconnect must not be replayed by the gateway adapter"
        );
    }

    #[tokio::test]
    async fn dropping_generation_stream_interrupts_its_owned_helper_turn() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "hold").unwrap();
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let mut events = adapter.generate(GenerationRequest {
            provider_id: "codex-cancel",
            generation: 9,
            model_id: "fixture-model",
            protocol: Protocol::Responses,
            body: json!({"model":"fixture-model","input":"hello"}),
            pre_dispatch_check: Arc::new(|| Ok(())),
        }).await.unwrap();
        assert_eq!(events.next().await, Some(GenerationEvent::Started { generation: 9 }));
        assert_eq!(events.next().await, Some(GenerationEvent::Chunk("fixture".into())));
        drop(events);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap_or_default();
                if calls.lines().any(|method| method == "turn/interrupt") { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("dropping the downstream stream must interrupt the helper turn");
    }

    #[tokio::test]
    async fn separate_subscription_accounts_use_separate_helper_homes_and_threads() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        for provider_id in ["codex-account-a", "codex-account-b"] {
            let mut events = adapter.generate(GenerationRequest {
                provider_id,
                generation: 1,
                model_id: "fixture-model",
                protocol: Protocol::Messages,
                body: json!({"model":"fixture-model","system":"system","messages":[{"role":"user","content":"hello"}]}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            }).await.unwrap();
            while events.next().await.is_some() {}
        }
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(recorded.contains(&format!("CODEX_HOME={}", helper_home_in(home.path(), "codex-account-a").display())));
        assert!(recorded.contains(&format!("CODEX_HOME={}", helper_home_in(home.path(), "codex-account-b").display())));
        let params = std::fs::read_to_string(format!("{}.generation-params", log.to_string_lossy())).unwrap();
        assert_eq!(params.lines().filter(|line| line.contains("thread ephemeral=true")).count(), 2);
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert_eq!(calls.lines().filter(|method| *method == "thread/start").count(), 2);
        assert_eq!(calls.lines().filter(|method| *method == "turn/start").count(), 2);
    }

    fn gateway_text_request(protocol: Protocol, model: &str, streaming: bool) -> Value {
        match protocol {
            Protocol::Chat => {
                json!({"model":model,"stream":streaming,"reasoning_effort":"medium","service_tier":"priority","messages":[
                {"role":"system","content":"chat system instruction"},
                {"role":"developer","content":"chat developer instruction"},
                {"role":"user","content":"chat user text"}
            ]})
            }
            Protocol::Responses => {
                json!({"model":model,"stream":streaming,"instructions":"responses developer instruction","service_tier":"priority","reasoning":{"effort":"high","summary":"auto"},"input":"responses user text"})
            }
            Protocol::Messages => {
                json!({"model":model,"stream":streaming,"system":"messages system instruction","messages":[{"role":"user","content":"messages user text"}]})
            }
        }
    }

    fn gateway_endpoint(protocol: Protocol) -> &'static str {
        crate::subscription::protocol_key(protocol)
    }

    #[tokio::test]
    async fn queued_gateway_request_is_rejected_after_logout_without_starting_a_turn() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("queued-gateway-logout.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "hold").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(),
            fixture_launch(home.path(), &log)));
        let mut active = adapter
            .generate(GenerationRequest {
                provider_id: "codex-fixture",
                generation: 1,
                model_id: "codex-fixture-model",
                protocol: Protocol::Responses,
                body: json!({"model":"codex-fixture-model","input":"active"}),
                pre_dispatch_check: Arc::new(|| Ok(())),
            })
            .await
            .unwrap();
        assert_eq!(active.next().await, Some(GenerationEvent::Started { generation: 1 }));
        assert_eq!(active.next().await, Some(GenerationEvent::Chunk("fixture".into())));

        let store = admitted_gateway_store(
            home.path().join("queued-gateway.db").as_path(),
            adapter.clone(),
            "http://127.0.0.1:9");
        let queued = {
            let store = store.clone();
            tokio::spawn(async move {
                crate::proxy::forward_test_request(
                    store,
                    json!({"model":"autojev/model/codex-fixture-binding","input":"queued"}),
                    "responses")
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let calls_path = format!("{}.calls", log.to_string_lossy());
        let calls = std::fs::read_to_string(&calls_path).unwrap();
        assert_eq!(calls.lines().filter(|method| *method == "turn/start").count(), 1,
            "the queued gateway request must wait behind the active account turn");

        let sessions = Arc::new(tokio::sync::Mutex::new(crate::subscription::SessionState::default()));
        let logout = {
            let store = store.clone();
            let sessions = sessions.clone();
            tokio::spawn(async move {
                crate::subscription::logout(&store, "codex-fixture", &sessions).await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if store.read().subscriptions["codex-fixture"].state
                    == crate::subscription::ConnectionState::NotConnected
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("logout invalidates the live connection before waiting for the generation lock");
        drop(active);

        let response = tokio::time::timeout(std::time::Duration::from_secs(3), queued)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY,
            "logout must invalidate a queued request before it starts a helper turn");
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), logout)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_ok());
        let calls = std::fs::read_to_string(&calls_path).unwrap();
        assert_eq!(calls.lines().filter(|method| *method == "turn/start").count(), 1,
            "the old queued request must not dispatch a turn after logout");
    }

    #[tokio::test]
    async fn gateway_bridges_text_and_incremental_sse_for_all_three_protocols() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("gateway-helper.log");
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let store = admitted_gateway_store(home.path().join("gateway.db").as_path(), adapter, "http://127.0.0.1:9");
        let model = "autojev/model/codex-fixture-binding";

        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            let reply = crate::proxy::forward_test_request(store.clone(), gateway_text_request(protocol, model, false), gateway_endpoint(protocol)).await;
            assert_eq!(reply.status(), axum::http::StatusCode::OK, "{protocol:?}");
            let bytes = axum::body::to_bytes(reply.into_body(), 1024 * 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let output = match protocol {
                Protocol::Chat => body.pointer("/choices/0/message/content"),
                Protocol::Responses => body.pointer("/output/0/content/0/text"),
                Protocol::Messages => body.pointer("/content/0/text"),
            }.and_then(Value::as_str);
            assert_eq!(output, Some("fixture"), "{protocol:?}: {body}");
        }

        std::fs::write(format!("{}.generation", log.to_string_lossy()), "slow-success").unwrap();
        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            let done = format!("{}.generation.done", log.to_string_lossy());
            let _ = std::fs::remove_file(&done);
            let reply = crate::proxy::forward_test_request(store.clone(), gateway_text_request(protocol, model, true), gateway_endpoint(protocol)).await;
            assert_eq!(reply.status(), axum::http::StatusCode::OK, "{protocol:?}");
            assert!(reply.headers().get("content-type").unwrap().to_str().unwrap().starts_with("text/event-stream"));
            let mut stream = reply.into_body().into_data_stream();
            let mut wire = String::new();
            loop {
                let chunk = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await
                    .expect("first output must arrive before terminal completion")
                    .expect("stream must contain a response frame").unwrap();
                wire.push_str(std::str::from_utf8(&chunk).unwrap());
                if wire.contains("fixture") { break; }
            }
            assert!(!std::path::Path::new(&done).exists(), "first text should be delivered before helper completion ({protocol:?})");
            while let Some(chunk) = stream.next().await { wire.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap()); }
            match protocol {
                Protocol::Chat => { assert!(wire.contains("\"finish_reason\":\"stop\""), "{wire}"); assert!(wire.contains("data: [DONE]"), "{wire}"); }
                Protocol::Responses => {
                    assert!(wire.contains("event: response.completed"), "{wire}")
                }
                Protocol::Messages => assert!(wire.contains("event: message_stop"), "{wire}"),
            }
        }

        let rpc: Vec<Value> = std::fs::read_to_string(format!("{}.rpc", log.to_string_lossy())).unwrap()
            .lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let threads: Vec<_> = rpc.iter().filter(|call| call["method"] == "thread/start").collect();
        let turns: Vec<_> = rpc.iter().filter(|call| call["method"] == "turn/start").collect();
        assert_eq!(threads.len(), 6);
        assert_eq!(turns.len(), 6);
        assert!(threads.iter().all(|call| call["params"]["ephemeral"] == true));
        assert!(threads.iter().all(|call| call["params"]["model"] == "codex-fixture-model"));
        assert!(threads.iter().all(|call| !call["params"]["cwd"].as_str().unwrap_or("").is_empty()));
        let workspaces: std::collections::HashSet<_> = threads
            .iter()
            .filter_map(|call| call["params"]["cwd"].as_str())
            .collect();
        assert_eq!(workspaces.len(), threads.len(), "every request must receive its own workspace");
        assert!(turns.iter().any(|call| call["params"]["input"][0]["text"] == "chat user text" && call["params"]["effort"] == "medium"));
        assert!(turns.iter().any(|call| call["params"]["input"][0]["text"] == "responses user text" && call["params"]["effort"] == "high" && call["params"]["summary"] == "auto"));
        assert!(turns.iter().any(|call| call["params"]["input"][0]["text"] == "messages user text"));
        assert!(threads.iter().any(|call| call["params"]["baseInstructions"] == "chat system instruction"));
        assert!(threads.iter().any(|call| call["params"]["developerInstructions"].as_str().unwrap_or("").contains("chat developer instruction")));
        assert!(threads.iter().any(|call| call["params"]["baseInstructions"] == "messages system instruction"));

        // Codex supports only the subset the app-server can preserve. The response limit and tools
        // are rejected before a helper turn is started, never silently discarded.
        let before = turns.len();
        for (protocol, body) in [
            (Protocol::Chat, json!({"model":model,"messages":[{"role":"user","content":"x"}],"temperature":0.2})),
            (Protocol::Responses, json!({"model":model,"max_output_tokens":20,"input":"x"})),
            (Protocol::Messages, json!({"model":model,"max_tokens":20,"system":"instructions","messages":[{"role":"user","content":"x"}]}),
            ),
            (Protocol::Chat, json!({"model":model,"tool_choice":"required","messages":[{"role":"user","content":"x"}]}),
            ),
            (Protocol::Chat, json!({"model":model,"messages":[{"role":"user","name":"Ada","content":"x"}]})),
            (Protocol::Responses, json!({"model":model,"input":[{"type":"message","role":"user","metadata":{"source":"x"},"content":"x"}]}),
            ),
            (Protocol::Messages, json!({"model":model,"messages":[{"role":"user","content":[{"type":"text","text":"x","annotations":[]}]}]}),
            ),
            (Protocol::Chat, json!({"model":model,"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]}),
            ),
            (Protocol::Responses, json!({"model":model,"previous_response_id":"resp_fixture","input":"x"})),
        ] {
            let body_for_assert = body.clone();
            let reply = crate::proxy::forward_test_request(store.clone(), body, gateway_endpoint(protocol)).await;
            assert_eq!(reply.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY, "{protocol:?}: {body_for_assert}");
            let _ = axum::body::to_bytes(reply.into_body(), 4096).await.unwrap();
        }
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert_eq!(calls.lines().filter(|method| *method == "turn/start").count(), before);
        let request_logs = store.request_logs("").unwrap();
        let streaming_logs: Vec<_> = request_logs.iter().filter(|entry| entry.streaming).collect();
        assert_eq!(streaming_logs.len(), 3);
        assert!(streaming_logs.iter().all(|entry| entry.status == "success"), "stream terminals must be understood by debug capture: {streaming_logs:?}");
    }

    #[tokio::test]
    async fn manual_speed_probe_uses_the_admitted_subscription_gateway_for_each_round() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("manual-speed-helper.log");
        let adapter = Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(),
            fixture_launch(home.path(), &log),
        ));
        let store = admitted_gateway_store(
            home.path().join("manual-speed.db").as_path(),
            adapter,
            "http://127.0.0.1:9",
        );
        let gateway = crate::proxy::start(store.clone()).await.unwrap();
        store.update(|config| config.port = gateway.port).unwrap();

        store.update(|config| {
            config.providers.iter_mut().find(|provider| provider.id == "codex-fixture").unwrap().test_model = "codex-fixture-model".into();
        }).unwrap();
        assert_eq!(
            crate::test_subscription_target(store.clone(), "codex-fixture", "codex-fixture-model").await.unwrap(),
            crate::SUBSCRIPTION_TEST_SUCCESS_MESSAGE
        );

        for _ in 0..3 {
            crate::performance::probe(&store, "codex-fixture-binding")
                .await
                .unwrap();
        }

        let rpc: Vec<Value> = std::fs::read_to_string(format!("{}.rpc", log.to_string_lossy()))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let turns: Vec<_> = rpc.iter().filter(|call| call["method"] == "turn/start").collect();
        assert_eq!(turns.len(), 4, "one helper turn must be counted for each test and measured request");
        assert!(turns[0]["params"]["input"][0]["text"].as_str().is_some_and(|text| text.contains("Say OK")));
        assert!(turns.iter().skip(1).all(|call| {
            call["params"]["input"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("1 through 8"))
        }));

        let samples = store.read().performance_samples["codex-fixture-binding"].clone();
        assert_eq!(samples.len(), 4);
        assert_eq!(samples.iter().filter(|sample| sample.probe).count(), 3);
        assert!(samples.iter().all(|sample| sample.success));
        let request_logs = store.request_logs("").unwrap();
        assert_eq!(request_logs.len(), 4);
        assert!(request_logs.iter().all(|request| request.requested_model == "autojev/model/codex-fixture-binding" && request.status == "success"));
        assert_eq!(request_logs.iter().filter(|request| request.streaming).count(), 3);

        store.update(|config| {
            config.subscriptions.get_mut("codex-fixture").unwrap().state = crate::subscription::ConnectionState::NotConnected;
        }).unwrap();
        assert!(crate::test_subscription_target(store.clone(), "codex-fixture", "codex-fixture-model").await.is_err());
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert_eq!(calls.lines().filter(|method| *method == "turn/start").count(), 4,
            "a denied manual test must not dispatch a helper turn");
        gateway.stop().await;
    }

    #[tokio::test]
    async fn unknown_extra_credit_permission_blocks_provider_and_speed_tests_before_helper_dispatch() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("unknown-credits-helper.log");
        let adapter = Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(),
            fixture_launch(home.path(), &log),
        ));
        let store = admitted_gateway_store(
            home.path().join("unknown-credits.db").as_path(),
            adapter.clone(),
            "http://127.0.0.1:9",
        );
        store.update(|config| {
            config.providers.iter_mut().find(|provider| provider.id == "codex-fixture").unwrap().test_model = "codex-fixture-model".into();
            config.subscriptions.get_mut("codex-fixture").unwrap().evidence.as_mut().unwrap().quota.buckets[0]
                .credits.as_mut().unwrap().permission = crate::subscription::QuotaPermission::Unknown;
        }).unwrap();

        let provider_test = crate::test_subscription_target(store.clone(), "codex-fixture", "codex-fixture-model").await.unwrap_err();
        assert!(provider_test.contains("whole call"), "{provider_test}");
        let speed_test = crate::performance::probe(&store, "codex-fixture-binding").await.unwrap_err().to_string();
        assert!(speed_test.contains("whole call"), "{speed_test}");

        let config = store.read();
        let provider = config.providers.iter().find(|provider| provider.id == "codex-fixture").unwrap().clone();
        let model = config.models.iter().find(|model| model.id == "codex-fixture-binding").unwrap().clone();
        let admission_store = store.clone();
        let admission_provider = provider.clone();
        let admission_model = model.clone();
        let generation = adapter.generate(GenerationRequest {
            provider_id: &provider.id,
            generation: config.subscriptions["codex-fixture"].generation,
            model_id: &model.model_id,
            protocol: Protocol::Chat,
            body: serde_json::json!({"model":model.model_id,"messages":[{"role":"user","content":"fixture"}]}),
            pre_dispatch_check: Arc::new(move || {
                let current = admission_store.read();
                crate::subscription::admit_model(&current, &admission_model, &admission_provider, Protocol::Chat)
                    .map(|_| ())
                    .map_err(|denial| denial.summary())
            }),
        }).await.err().expect("unknown credits permission must reject the direct helper generation before dispatch").to_string();
        assert!(generation.contains("whole call"), "{generation}");

        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap_or_default();
        assert!(!calls.lines().any(|method| method == "turn/start"), "denied tests must not start any helper turn: {calls}");
        assert!(store.request_logs("").unwrap().iter().all(|request| request.status != "success"));
    }

    #[tokio::test]
    async fn unsaved_subscription_provider_kind_change_is_rejected_before_helper_dispatch() {
        use tauri::Manager;

        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("provider-kind-helper.log");
        let adapter = Arc::new(CodexAdapter::with_launch(
            home.path().to_path_buf(),
            fixture_launch(home.path(), &log),
        ));
        let store = admitted_gateway_store(
            home.path().join("provider-kind.db").as_path(),
            adapter,
            "http://127.0.0.1:9",
        );
        let gateway = crate::proxy::start(store.clone()).await.unwrap();
        store.update(|config| config.port = gateway.port).unwrap();

        let saved = store
            .read()
            .providers
            .into_iter()
            .find(|provider| provider.id == "codex-fixture")
            .unwrap();
        let mut draft = saved.clone();
        draft.kind = crate::config::ProviderKind::GrokSubscription;
        draft.name = "Grok fixture".into();
        draft.test_model = "codex-fixture-model".into();

        let app = tauri::test::mock_app();
        app.manage(crate::AppState {
            performance: Arc::new(crate::performance::Runner::default()),
            store: store.clone(),
            proxy: Arc::new(tokio::sync::Mutex::new(Some(gateway))),
            sessions: Arc::new(tokio::sync::Mutex::new(crate::subscription::SessionState::default())),
        });
        let outcome = crate::test_provider_draft(app.state::<crate::AppState>(), draft, None).await;
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap_or_default();
        let turn_starts = calls.lines().filter(|method| *method == "turn/start").count();
        let error = outcome.expect_err("a draft Grok provider must not test through the saved Codex connection");
        assert!(error.contains("refresh its model catalog"), "the stale target should be rejected with recovery guidance: {error}");
        assert_eq!(turn_starts, 0, "the rejected draft must not start a Codex helper turn");

        app.state::<crate::AppState>().proxy.lock().await.take().unwrap().stop().await;
    }

    #[tokio::test]
    async fn codex_nonstreaming_event_wait_obeys_gateway_response_timeout() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("response-timeout-helper.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "hold").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        // Warm the controlled helper outside the request timeout. The regression under test is
        // consuming a started turn's events, not process startup or thread creation.
        adapter.status("codex-fixture", 0).await.unwrap();
        let store = admitted_gateway_store(home.path().join("gateway.db").as_path(), adapter, "http://127.0.0.1:9");
        store.update(|config| config.gateway.response_timeout_seconds = 2).unwrap();

        let store_for_request = store.clone();
        let request = tokio::spawn(async move {
            crate::proxy::forward_test_request(
                store_for_request,
                gateway_text_request(Protocol::Responses, "autojev/model/codex-fixture-binding", false),
                gateway_endpoint(Protocol::Responses),
            ).await
        });
        let event_started = format!("{}.event-consumption-started", log.to_string_lossy());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !std::path::Path::new(&event_started).exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("the fixture must start the turn and emit partial output before timing out");
        assert!(
            !request.is_finished(),
            "the gateway request must still be consuming the started turn after the fixture emits its first chunk"
        );

        let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await.expect("the event-consumption timeout must bound the non-streaming request")
            .expect("the gateway request task must not panic");
        assert_eq!(response.status(), axum::http::StatusCode::GATEWAY_TIMEOUT);
        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let error: Value = serde_json::from_slice(&body).expect("the timeout response must be JSON");
        assert_eq!(
            error.pointer("/error/message").and_then(Value::as_str),
            Some("Codex generation exceeded the gateway response timeout."),
            "this must be the event-consumption timeout, not a timeout starting the helper turn"
        );

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap_or_default();
                if calls.lines().any(|method| method == "turn/interrupt") { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("timing out the HTTP request must cancel its helper turn");
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert!(calls.lines().any(|method| method == "turn/start"), "the event timeout must happen after turn/start");
    }

    #[tokio::test]
    async fn codex_streaming_body_obeys_gateway_idle_timeout_after_partial_output() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("stream-idle-helper.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "hold").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let store = admitted_gateway_store(home.path().join("gateway.db").as_path(), adapter, "http://127.0.0.1:9");
        store.update(|config| config.gateway.stream_idle_seconds = 1).unwrap();
        let response = crate::proxy::forward_test_request(
            store,
            gateway_text_request(Protocol::Responses, "autojev/model/codex-fixture-binding", true),
            gateway_endpoint(Protocol::Responses),
        ).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let mut stream = response.into_body().into_data_stream();
        let mut saw_output = false;
        let timeout_error = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match stream.next().await {
                    Some(Ok(bytes)) => saw_output |= !bytes.is_empty(),
                    Some(Err(error)) => break error.to_string(),
                    None => {
                        panic!("the stalled Codex body must fail with the configured idle timeout")
                    }
                }
            }
        }).await.expect("the Codex HTTP body must apply stream_idle_seconds after partial output");
        assert!(saw_output, "the fixture must deliver partial SSE bytes before stalling");
        assert!(timeout_error.contains("Upstream idle timeout"), "{timeout_error}");
        drop(stream);

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap_or_default();
                if calls.lines().any(|method| method == "turn/interrupt") { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("an idle timeout must cancel the helper turn backing the body");
    }

    #[tokio::test]
    async fn failed_tool_interrupt_reaps_helper_before_releasing_generation_lock() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("failed-interrupt-helper.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "tool-interrupt-fails").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let mut active = adapter.generate(GenerationRequest {
            provider_id: "codex-failed-interrupt",
            generation: 1,
            model_id: "fixture-model",
            protocol: Protocol::Responses,
            body: json!({"model":"fixture-model","input":"A"}),
            pre_dispatch_check: Arc::new(|| Ok(())),
        }).await.unwrap();
        assert_eq!(active.next().await, Some(GenerationEvent::Started { generation: 1 }));
        let helper_a_id = adapter.active_turns.lock().unwrap()["codex-failed-interrupt"]
            .helper_instance_id.clone().expect("A must register its owned helper instance");

        let request_b = GenerationRequest {
            provider_id: "codex-failed-interrupt",
            generation: 2,
            model_id: "fixture-model",
            protocol: Protocol::Responses,
            body: json!({"model":"fixture-model","input":"B"}),
            pre_dispatch_check: Arc::new(|| Ok(())),
        };
        let mut b_start = adapter.generate(request_b);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(100), &mut b_start).await.is_err(),
            "B must queue while A owns the helper turn");

        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(3), active.next()).await.unwrap(),
            Some(GenerationEvent::Failed { message }) if message.contains("tool activity")
        ));
        drop(active);
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "success").unwrap();
        let mut b_events = tokio::time::timeout(std::time::Duration::from_secs(3), &mut b_start).await
            .expect("B should proceed after the failed A turn is stopped").unwrap();
        let helper_b_id = adapter.servers.lock().await["codex-failed-interrupt"].instance_id().to_owned();
        assert_ne!(helper_a_id, helper_b_id, "a failed interrupt must reap A's helper before B starts");
        assert!(!std::path::Path::new(&format!("{}.overlap", log.to_string_lossy())).exists(),
            "B must not start on an app-server that still owns A's incomplete turn");
        let mut b_observed = Vec::new();
        while let Some(event) = b_events.next().await { b_observed.push(event); }
        assert_eq!(b_observed, vec![
            GenerationEvent::Started { generation: 2 },
            GenerationEvent::Chunk("fixture".into()),
            GenerationEvent::Finished { status: 200 },
        ]);
    }

    #[tokio::test]
    async fn dropping_gateway_sse_interrupts_only_its_codex_turn() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("cancel-helper.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "hold").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let store = admitted_gateway_store(home.path().join("gateway.db").as_path(), adapter, "http://127.0.0.1:9");
        let response = crate::proxy::forward_test_request(store, gateway_text_request(Protocol::Responses, "autojev/model/codex-fixture-binding", true), gateway_endpoint(Protocol::Responses),
        ).await;
        let mut stream = response.into_body().into_data_stream();
        let mut text = String::new();
        loop {
            let chunk = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await.unwrap().unwrap().unwrap();
            text.push_str(std::str::from_utf8(&chunk).unwrap());
            if text.contains("fixture") { break; }
        }
        drop(stream);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap_or_default();
                if calls.lines().any(|method| method == "turn/interrupt") { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("dropping the gateway response must interrupt the helper turn");
    }

    #[tokio::test]
    async fn gemini_protocol_rejects_codex_subscription_before_helper_dispatch() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("gemini-helper.log");
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let store = admitted_gateway_store(home.path().join("gateway.db").as_path(), adapter, "http://127.0.0.1:9");
        let reply = crate::proxy::gemini_test_request(
            store,
            "autojev/model/codex-fixture-binding:generateContent",
            json!({"contents":[{"role":"user","parts":[{"text":"hello"}]}]}),
        ).await;
        assert_eq!(reply.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(reply.headers().get("x-should-retry").and_then(|value| value.to_str().ok()), Some("false"));
        let _ = axum::body::to_bytes(reply.into_body(), 4096).await.unwrap();
        assert!(!std::path::Path::new(&format!("{}.calls", log.to_string_lossy())).exists(),
            "unsupported Gemini subscription calls must be rejected before starting the helper");
    }

    #[tokio::test]
    async fn existing_api_requests_keep_their_http_dispatch_and_credentials() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("api-isolation-helper.log");
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = Arc::new(RecordingDispatcher(seen.clone()));
        let store = admitted_gateway_store_with_dispatcher(
            home.path().join("gateway.db").as_path(), adapter, dispatcher, "http://127.0.0.1:9");
        let api_model = store.read().models.iter().find(|model| model.provider_id == "openrouter").unwrap().clone();
        let response = crate::proxy::forward_test_request(
            store,
            json!({"model":format!("autojev/model/{}",api_model.id),"messages":[{"role":"user","content":"api isolation"}]}),
            gateway_endpoint(Protocol::Chat),
        ).await;
        assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
        let _ = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();

        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].provider_id, "openrouter");
        assert_eq!(calls[0].model_id, api_model.model_id);
        assert_eq!(calls[0].protocol, Protocol::Chat);
        assert_eq!(calls[0].url, "http://127.0.0.1:9/v1/chat/completions");
        assert_eq!(calls[0].authorization, "Bearer fixture-api-key");
        assert_eq!(calls[0].body["messages"][0]["content"], "api isolation");
        assert!(!std::path::Path::new(&format!("{}.calls", log.to_string_lossy())).exists(),
            "API requests must not start or borrow the Codex helper");
    }

    #[tokio::test]
    async fn partial_codex_failure_does_not_retry_into_an_api_model() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("partial-helper.log");
        std::fs::write(format!("{}.generation", log.to_string_lossy()), "partial-failure").unwrap();
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        let store = admitted_gateway_store(home.path().join("gateway.db").as_path(), adapter, "http://127.0.0.1:9");
        let (api_id, codex_id) = {
            let config = store.read();
            (
                config.models.iter().find(|model| model.provider_id == "openrouter").unwrap().id.clone(),
                config.models.iter().find(|model| model.provider_id == "codex-fixture").unwrap().id.clone(),
            )
        };
        store.update(|config| {
            config.gateway.max_attempts = 4;
            config.routes.push(crate::config::RouteRule {
                all_models: false,
                id: "codex-first".into(), name: "Codex first".into(), strategy: "round_robin".into(), enabled: true,
                model_ids: vec![codex_id.clone(), api_id.clone()],
                model_settings: std::collections::HashMap::from([
                    (codex_id.clone(), crate::config::RouteModelSettings { priority: 10, weight: 1 }),
                    (api_id.clone(), crate::config::RouteModelSettings { priority: 1, weight: 1 }),
                ]),
                automatic_policy: None,
            });
        }).unwrap();
        let reply = crate::proxy::forward_test_request(store.clone(), gateway_text_request(Protocol::Chat, "autojev/codex-first", false), gateway_endpoint(Protocol::Chat),
        ).await;
        assert_eq!(reply.status(), axum::http::StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(reply.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body.pointer("/error/message").and_then(Value::as_str).unwrap_or("").contains("Codex generation failed"), "{body}");
        let calls = std::fs::read_to_string(format!("{}.calls", log.to_string_lossy())).unwrap();
        assert_eq!(calls.lines().filter(|method| *method == "turn/start").count(), 1);
        assert_eq!(store.request_logs("").unwrap()[0].attempts.len(), 1);
        assert!(!store.read().events[0].success, "a failed non-stream Codex turn must not be recorded as a successful route");
    }

    #[test]
    fn codex_turn_request_preserves_text_instructions_and_rejects_unenforceable_fields() {
        let chat = codex_turn_request(
            crate::protocol::Protocol::Chat,
            &json!({"model":"requested","stream":true,"messages":[
                {"role":"system","content":"system prompt"},
                {"role":"developer","content":"developer prompt"},
                {"role":"user","content":"question"}
            ]}),
        ).unwrap();
        assert_eq!(chat.base_instructions.as_deref(), Some("system prompt"));
        assert_eq!(chat.developer_instructions.as_deref(), Some("developer prompt"));
        assert_eq!(chat.input, vec![json!({"type":"text","text":"question"})]);

        let responses = codex_turn_request(
            crate::protocol::Protocol::Responses,
            &json!({"model":"requested","stream":false,"instructions":"answer briefly","input":"question"}),
        ).unwrap();
        assert_eq!(responses.developer_instructions.as_deref(), Some("answer briefly"));
        assert_eq!(responses.input, vec![json!({"type":"text","text":"question"})]);

        let messages = codex_turn_request(
            crate::protocol::Protocol::Messages,
            &json!({"model":"requested","stream":false,"system":"instructions","messages":[{"role":"user","content":"question"}]}),
        ).unwrap();
        assert_eq!(messages.base_instructions.as_deref(), Some("instructions"));
        assert_eq!(messages.input, vec![json!({"type":"text","text":"question"})]);

        for (protocol, body) in [
            (crate::protocol::Protocol::Chat, json!({"max_tokens":16,"messages":[{"role":"user","content":"x"}]})),
            (crate::protocol::Protocol::Responses, json!({"max_output_tokens":16,"input":"x"})),
            (crate::protocol::Protocol::Messages, json!({"max_tokens":16,"messages":[{"role":"user","content":"x"}]}),
            ),
            (crate::protocol::Protocol::Chat, json!({"tool_choice":"required","messages":[{"role":"user","content":"x"}]}),
            ),
            (crate::protocol::Protocol::Responses, json!({"previous_response_id":"resp-old","input":"x"})),
        ] {
            assert!(codex_turn_request(protocol, &body).is_err(), "{protocol:?}: {body}");
        }
    }

    #[test]
    fn codex_turn_request_accepts_client_function_tools() {
        let request = codex_turn_request(crate::protocol::Protocol::Chat,
            &json!({
                "model":"requested",
                "messages":[{"role":"user","content":"look this up"}],
                "tools":[{"type":"function","function":{
                    "name":"lookup",
                    "description":"Look up a value",
                    "parameters":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}
                }}]
            }),
        );
        assert!(
            request.is_ok(),
            "a validated function schema must reach the Codex tool adapter: {:?}",
            request.err()
        );
    }

    #[test]
    fn all_codex_protocols_normalize_function_schemas_and_tool_result_pairs() {
        let generation = 7;
        let first = format!("call_ajv1_{generation}_first");
        let second = format!("call_ajv1_{generation}_second");
        let schema = json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]});
        let cases = [
            (
                Protocol::Chat,
                json!({"messages":[{"role":"user","content":"lookup"}],"tools":[{"type":"function","function":{"name":"lookup","parameters":schema}}]}),
                json!({"messages":[{"role":"user","content":"lookup"},{"role":"assistant","content":null,"tool_calls":[
                    {"id":first,"type":"function","function":{"name":"lookup","arguments":r#"{"key":"one"}"#}},
                    {"id":second,"type":"function","function":{"name":"lookup","arguments":r#"{"key":"two"}"#}}
                ]},{"role":"tool","tool_call_id":first,"content":"one-result"},{"role":"tool","tool_call_id":second,"content":"two-result"}],"tools":[{"type":"function","function":{"name":"lookup","parameters":schema}}]}),
            ),
            (
                Protocol::Responses,
                json!({"input":"lookup","tools":[{"type":"function","name":"lookup","parameters":schema}]}),
                json!({"input":[{"type":"message","role":"user","content":"lookup"},
                    {"type":"function_call","id":"fc_first","call_id":first,"name":"lookup","arguments":r#"{"key":"one"}"#,"status":"completed"},
                    {"type":"function_call","id":"fc_second","call_id":second,"name":"lookup","arguments":r#"{"key":"two"}"#,"status":"completed"},
                    {"type":"function_call_output","call_id":first,"output":"one-result"},
                    {"type":"function_call_output","call_id":second,"output":"two-result"}],"tools":[{"type":"function","name":"lookup","parameters":schema}]}),
            ),
            (
                Protocol::Messages, json!({"messages":[{"role":"user","content":"lookup"}],"tools":[{"name":"lookup","input_schema":schema}]}),
                json!({"messages":[{"role":"user","content":"lookup"},
                    {"role":"assistant","content":[{"type":"tool_use","id":first,"name":"lookup","input":{"key":"one"}},{"type":"tool_use","id":second,"name":"lookup","input":{"key":"two"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":first,"content":"one-result"},{"type":"tool_result","tool_use_id":second,"content":"two-result"}]}],"tools":[{"name":"lookup","input_schema":schema}]}),
            ),
        ];
        for (protocol, initial, followup) in cases {
            let initial = codex_turn_request(protocol, &initial).unwrap();
            assert_eq!(initial.dynamic_tools.len(), 1, "{protocol:?}");
            assert_eq!(initial.dynamic_tools[0]["type"], "function");
            assert_eq!(initial.dynamic_tools[0]["inputSchema"], schema);

            let followup = codex_turn_request(protocol, &followup).unwrap();
            assert!(followup.has_assistant_history, "{protocol:?}");
            assert_eq!(followup.tool_calls.len(), 2, "{protocol:?}");
            assert_eq!(followup.tool_outputs.len(), 2, "{protocol:?}");
            assert_eq!(followup.tool_calls[0].id, first);
            assert_eq!(followup.tool_calls[1].id, second);
            assert_eq!(followup.tool_outputs[0].id, first);
            assert_eq!(followup.tool_outputs[1].id, second);
        }
        let stale_id = "call_ajv1_6_old";
        for (protocol, body) in [
            (Protocol::Chat, json!({"messages":[{"role":"tool","tool_call_id":stale_id,"content":"old result"}]})),
            (
                Protocol::Responses,
                json!({"input":[{"type":"function_call_output","call_id":stale_id,"output":"old result"}]}),
            ),
            (
                Protocol::Messages,
                json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":stale_id,"content":"old result"}]}]}),
            ),
        ] {
            assert!(validate_generation_request(protocol, &body, generation).is_err(), "{protocol:?}: {body}");
        }
    }

    #[tokio::test]
    async fn adapter_logout_removes_only_its_own_auth_home() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        adapter.login_result("codex-fixture", 1).await.unwrap();
        let auth_home = helper_home_in(home.path(), "codex-fixture");
        assert!(auth_home.is_dir(), "the dedicated auth home is used while signed in");
        let outcome = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(outcome.local_cleared, "logout must report the dedicated directory as cleared");
        assert!(!auth_home.exists(), "the dedicated auth home must be removed");
        assert!(adapter.helper_status().auth_home.is_none(), "a deleted path must not be reported");
        // 没有活动会话时也删除：重启后残留的授权目录必须能被退出清掉。
        std::fs::create_dir_all(&auth_home).unwrap();
        let again = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(again.local_cleared);
        assert_eq!(again.remote, RemoteRevocation::Failed);
        assert!(!auth_home.exists());
        // 只删自有目录：用户的 ~/.codex 原样保留。
        let user_codex = home.path().join(".codex");
        std::fs::create_dir_all(&user_codex).unwrap();
        std::fs::write(user_codex.join("auth.json"), "fixture").unwrap();
        adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(user_codex.join("auth.json").is_file(), "the user's own ~/.codex must never be touched");
    }

    #[tokio::test]
    async fn rename_never_hot_migrates_a_running_helper() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        // 拉起 fixture 辅助进程：旧标识下存在活动 helper。
        adapter.start_login("codex-fixture", 1).await.unwrap();
        let old_home = helper_home_in(home.path(), "codex-fixture");
        let new_home = helper_home_in(home.path(), "codex-renamed");
        std::fs::write(old_home.join("fictional-auth.json"), "fixture").unwrap();

        let error = adapter.rename("codex-fixture", "codex-renamed").await.unwrap_err();
        assert!(error.to_string().contains("still running"), "{error}");
        // 绝不出现「磁盘已在新路径、子进程仍写旧 CODEX_HOME」。
        assert!(old_home.join("fictional-auth.json").is_file());
        assert!(!new_home.exists());

        // 回退与迁移失败一致：对旧标识退出（杀进程 + 清专用目录），新标识逼重登。
        let outcome = adapter.logout("codex-fixture", 1).await.unwrap();
        assert!(outcome.local_cleared);
        assert!(!old_home.exists());

        // 没有活动进程后才整体迁移：目录与内容跟随新标识。
        std::fs::create_dir_all(&old_home).unwrap();
        std::fs::write(old_home.join("fictional-auth.json"), "fixture").unwrap();
        adapter.rename("codex-fixture", "codex-renamed").await.unwrap();
        assert!(!old_home.exists());
        assert!(new_home.join("fictional-auth.json").is_file());
        // 清理跟随新标识：退出删的是新目录。
        let moved = adapter.logout("codex-renamed", 1).await.unwrap();
        assert!(moved.local_cleared);
        assert!(!new_home.exists());
    }

    #[tokio::test]
    async fn a_superseded_login_waiter_never_starves_the_new_attempt() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log)));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        // 真正让旧等待者先进去轮询，再用同世代发起第二次尝试。
        let stale = tokio::spawn({
            let adapter = adapter.clone();
            async move { adapter.login_result("codex-fixture", 1).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let second = adapter.start_login("codex-fixture", 1).await.unwrap();
        assert_eq!(second.login_id, "fixture-login-2");
        // 旧等待者最多只会拿到第一次尝试的结果（编排层按尝试号整体丢弃），并且必须自己退出。
        let stale_result = tokio::time::timeout(std::time::Duration::from_secs(3), stale)
            .await
            .expect("the superseded waiter must stop")
            .expect("the waiter task must not panic")
            .unwrap();
        if let Some(LoginResult::Completed { identity }) = &stale_result {
            assert_eq!(identity, "fixture@example.invalid");
        }
        // 关键性质：第二次尝试的完成通知不会被旧等待者吞掉。
        let current = tokio::time::timeout(std::time::Duration::from_secs(5), adapter.login_result("codex-fixture", 1))
            .await
            .expect("the current attempt still receives its completion")
            .unwrap();
        assert_eq!(current, Some(LoginResult::Completed { identity: "fixture@example.invalid".into() }));
    }

    #[tokio::test]
    async fn first_completion_without_login_id_is_safely_bound_to_its_only_attempt() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".login");
        std::fs::write(PathBuf::from(scenario), "missing-id\n").unwrap();
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            adapter.login_result("codex-fixture", 1),
        )
        .await
        .expect("the official optional loginId must be handled without a long timeout")
        .unwrap();
        assert_eq!(result, Some(LoginResult::Completed { identity: "fixture@example.invalid".into() }));
    }

    #[tokio::test]
    async fn login_start_rejects_a_response_for_another_account_type() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".login");
        std::fs::write(PathBuf::from(scenario), "wrong-response-type\n").unwrap();
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));

        let error = adapter.start_login("codex-fixture", 1).await.unwrap_err();
        assert!(
            error.to_string().contains("browser sign-in response"),
            "{error}"
        );
        assert_eq!(
            adapter.login_result("codex-fixture", 1).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn cancelled_attempt_late_idless_completion_is_ignored_until_replacement_completes() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".login");
        std::fs::write(PathBuf::from(scenario), "missing-id\ndefault\n").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        adapter.cancel_login("codex-fixture", 1).await.unwrap();
        adapter.start_login("codex-fixture", 1).await.unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            adapter.login_result("codex-fixture", 1),
        )
        .await
        .expect("the replacement must keep waiting after an uncorrelated old completion")
        .unwrap();
        assert_eq!(
            result,
            Some(LoginResult::Completed {
                identity: "fixture@example.invalid".into()
            })
        );
    }

    #[tokio::test]
    async fn completed_attempt_duplicate_idless_notification_does_not_fail_replacement() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".login");
        std::fs::write(PathBuf::from(scenario), "duplicate-idless\ndefault\n").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                adapter.login_result("codex-fixture", 1)
            )
            .await
            .expect("the first identified completion must finish")
            .unwrap(),
            Some(LoginResult::Completed {
                identity: "fixture@example.invalid".into()
            })
        );

        adapter.start_login("codex-fixture", 1).await.unwrap();
        let replacement = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            adapter.login_result("codex-fixture", 1),
        )
        .await
        .expect("a duplicate idless completion must not settle the replacement")
        .unwrap();
        assert_eq!(
            replacement,
            Some(LoginResult::Completed {
                identity: "fixture@example.invalid".into()
            })
        );
    }

    #[tokio::test]
    async fn replacement_failure_after_late_idless_notification_remains_terminal_and_redacted() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".login");
        std::fs::write(PathBuf::from(scenario), "missing-id\nfailed-sensitive\n").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        adapter.cancel_login("codex-fixture", 1).await.unwrap();
        adapter.start_login("codex-fixture", 1).await.unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            adapter.login_result("codex-fixture", 1),
        )
        .await
        .expect("the replacement must wait for its matching failure notification")
        .unwrap();
        let Some(LoginResult::Failed(message)) = result else {
            panic!("the replacement's matching failure must remain terminal: {result:?}");
        };
        assert!(message.contains("[redacted]"), "{message}");
        assert!(!message.contains("SUPERSECRET"), "{message}");
        assert_eq!(adapter.login_result("codex-fixture", 1).await.unwrap(), None);
    }

    #[tokio::test]
    async fn timed_out_login_waiter_can_be_cancelled_before_a_new_attempt() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".login");
        std::fs::write(PathBuf::from(scenario), "pending\ndefault\n").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();

        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                adapter.login_result("codex-fixture", 1)
            )
            .await
            .is_err(),
            "the pending fictional attempt should outlive the caller's bounded wait"
        );
        adapter.cancel_login("codex-fixture", 1).await.unwrap();
        adapter.start_login("codex-fixture", 1).await.unwrap();

        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                adapter.login_result("codex-fixture", 1)
            )
            .await
            .expect("the attempt after explicit cancellation should complete")
            .unwrap(),
            Some(LoginResult::Completed {
                identity: "fixture@example.invalid".into()
            })
        );
        let mut call_log = log.as_os_str().to_os_string();
        call_log.push(".calls");
        let calls = std::fs::read_to_string(PathBuf::from(call_log)).unwrap();
        assert!(
            calls.contains("account/login/cancel"),
            "the timed-out attempt must be cancelled explicitly"
        );
    }

    #[tokio::test]
    async fn official_login_failure_error_is_redacted() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".login");
        std::fs::write(PathBuf::from(scenario), "failed-sensitive\n").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            adapter.login_result("codex-fixture", 1),
        )
        .await
        .expect("the failed login notification must settle")
        .unwrap();
        let Some(LoginResult::Failed(message)) = result else {
            panic!("a failed helper notification cannot establish a connection: {result:?}");
        };
        assert!(message.contains("[redacted]"), "{message}");
        assert!(!message.contains("SUPERSECRET"), "{message}");
        assert!(!message.contains("abcdef0123456789"), "{message}");
    }

    #[tokio::test]
    async fn successful_notification_requires_a_chatgpt_account_read() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let mut scenario = log.as_os_str().to_os_string();
        scenario.push(".account");
        std::fs::write(PathBuf::from(scenario), "api-key-email\n").unwrap();
        let adapter =
            CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            adapter.login_result("codex-fixture", 1),
        )
        .await
        .expect("the official successful notification must trigger account/read")
        .unwrap();
        assert_eq!(
            result,
            Some(LoginResult::Failed(
                "The Codex helper did not confirm a signed-in account".into()
            ))
        );
    }

    #[test]
    fn completion_notifications_are_routed_by_login_id() {
        let completed = json!({"jsonrpc": "2.0", "method": "account/login/completed", "params": {"loginId": "a", "success": true}});
        let (login_id, params) = login_completion(&completed).unwrap();
        assert_eq!(login_id.as_deref(), Some("a"));
        assert_eq!(params["success"], true);
        let without_id = json!({"jsonrpc": "2.0", "method": "account/login/completed", "params": {"success": true}});
        assert_eq!(login_completion(&without_id).unwrap().0, None);
        assert!(login_completion(&json!({"jsonrpc": "2.0", "method": "account/rateLimits/read"})).is_none());
        let deferred = Mutex::new(HashMap::new());
        defer(&deferred, "codex", "b", json!({"loginId": "b"}));
        defer(&deferred, "codex", "a", json!({"loginId": "a"}));
        assert_eq!(take_deferred(&deferred, "codex", "a"), Some(json!({"loginId": "a"})));
        assert_eq!(take_deferred(&deferred, "codex", "a"), None);
        assert_eq!(take_deferred(&deferred, "codex", "b"), Some(json!({"loginId": "b"})));
        assert!(take_deferred(&deferred, "codex", "b").is_none());
    }

    #[tokio::test]
    async fn adapter_login_result_is_bound_to_its_generation() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        adapter.start_login("codex-fixture", 1).await.unwrap();
        assert_eq!(adapter.login_result("codex-fixture", 2).await.unwrap(), None);
        adapter.cancel_login("codex-fixture", 1).await.unwrap();
    }

    #[tokio::test]
    async fn adapter_reads_the_catalog_without_granting_eligibility() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let read = adapter.models("codex-fixture", 1).await.unwrap();
        assert_eq!(read.source.as_deref(), Some("codex-app-server:model/list"));
        let observed = read.observed_at.as_deref().expect("a successful read records its time");
        assert!(chrono::DateTime::parse_from_rfc3339(observed).is_ok(), "{observed}");
        // 界面会原样外露这个时间：规范 RFC3339（`Z` + 秒精度），不带纳秒与 +00:00。
        assert!(observed.ends_with('Z') && observed.len() == 20, "{observed}");
        // `displayName` 优先，缺失时回落 `name`；没有可用标识的条目只记缺失并跳过。
        assert_eq!(read.models.len(), 2);
        assert_eq!(read.models[0].model_id, "preset-a");
        assert_eq!(read.models[0].name.as_deref(), Some("GPT-5 Codex"));
        assert_eq!(read.models[1].model_id, "slug-only");
        assert!(read.models[1].name.is_none());
        assert_eq!(read.missing_fields, vec!["model.list[2].id".to_owned()]);
        // 发现 ≠ 资格：目录读取一律 false，资格由 #17 定义。
        assert!(read.models.iter().all(|model| !model.eligible), "discovery must never be eligibility");
    }

    #[tokio::test]
    async fn adapter_reads_multi_bucket_quota_with_the_root_permission() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), fixture_launch(home.path(), &log));
        let quota = adapter.quota("codex-fixture", 1).await.unwrap();
        assert_eq!(quota.view, QuotaView::RateLimitsByLimitId);
        // limit-a 桶内显式 false，根层是 true：fail-closed 取更严格的一方 → 该桶 denied，整体 Denied。
        // 固定版本只在根层有该字段（桶内键属防御性输入），这里覆盖的是防御性冲突，不是真实形状。
        assert_eq!(quota.state, EvidenceState::Denied);
        assert_eq!(quota.source.as_deref(), Some("codex-app-server:account/rateLimits/read#rateLimitsByLimitId"));
        assert!(quota.observed_at.as_deref().is_some_and(|at| chrono::DateTime::parse_from_rfc3339(at).is_ok()));
        assert!(!quota.history);
        assert!(quota.missing_fields.is_empty());
        // 多桶按 limitId 稳定排序；桶内 limitId 优先于映射键。
        assert_eq!(quota.buckets.len(), 2);
        assert_eq!(quota.buckets[0].limit_id, "limit-a");
        assert_eq!(quota.buckets[0].permission, QuotaPermission::Denied, "an explicit bucket-level false must never be overridden by the root true");
        let second = &quota.buckets[1];
        assert_eq!(second.limit_id, "limit-b");
        // 根层 true 且桶内没有该键 → Allowed（固定版本的唯一真实形状）。
        assert_eq!(second.permission, QuotaPermission::Allowed);
        assert_eq!(second.name.as_deref(), Some("B"));
        assert_eq!(second.plan_type.as_deref(), Some("pro"));
        assert_eq!(second.windows.len(), 2);
        assert_eq!(second.windows[0].label, "primary");
        assert_eq!(second.windows[0].used_percent, Some(10.0));
        assert_eq!(second.windows[0].window_minutes, Some(300));
        assert_eq!(second.windows[0].resets_at, Some(1_800_000_100));
        assert_eq!(second.windows[1].label, "secondary");
        // credits 原样保留字符串：不解析金额、不推断单位。
        let credits = second.credits.as_ref().expect("credits are mapped as-is");
        assert_eq!(credits.balance.as_deref(), Some("12.5 credits"));
        assert_eq!(credits.has_credits, Some(true));
        assert_eq!(credits.unlimited, Some(false));
        assert_eq!(credits.permission, QuotaPermission::Unknown, "the real-shaped balance is not a whole-call extra-credit prohibition");
        assert!(credits.missing_fields.is_empty());
    }

    #[tokio::test]
    async fn adapter_covers_single_missing_invalid_denied_and_unknown_quota() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let launch = scenario_launch(home.path(), &log, &[], &["single", "missing", "invalid", "denied", "no-permission", "fail"]);
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), launch);

        // single：只有旧版 rateLimits 时用单桶视图，来源与视图如实标记。
        let single = adapter.quota("codex-fixture", 1).await.unwrap();
        assert_eq!(single.view, QuotaView::RateLimits);
        assert_eq!(single.state, EvidenceState::Available);
        assert_eq!(single.source.as_deref(), Some("codex-app-server:account/rateLimits/read#rateLimits"));
        assert_eq!(single.buckets.len(), 1);
        assert_eq!(single.buckets[0].limit_id, "legacy-single");
        assert_eq!(single.buckets[0].windows[0].used_percent, Some(7.0));
        assert_eq!(single.buckets[0].credits.as_ref().unwrap().balance.as_deref(), Some("3 credits"));

        // missing：缺 resetsAt/windowDurationMins/credits 分别记入各自层级，不补默认值。
        let missing = adapter.quota("codex-fixture", 1).await.unwrap();
        let window = &missing.buckets[0].windows[0];
        assert_eq!(window.resets_at, None);
        assert_eq!(window.window_minutes, None);
        assert!(window.missing_fields.contains(&"resetsAt".to_owned()), "{:?}", window.missing_fields);
        assert!(window.missing_fields.contains(&"windowDurationMins".to_owned()));
        assert!(missing.buckets[0].missing_fields.contains(&"credits".to_owned()));
        assert!(missing.buckets[0].credits.is_none());
        assert_eq!(missing.state, EvidenceState::Available, "usedPercent alone is enough once permission is allowed");

        // invalid：越界/类型不符的原值记入 invalid_fields，取值一律 None，绝不截断成合法值。
        let invalid = adapter.quota("codex-fixture", 1).await.unwrap();
        let window = &invalid.buckets[0].windows[0];
        assert_eq!(window.used_percent, None);
        assert_eq!(window.window_minutes, None);
        assert_eq!(window.resets_at, None);
        assert!(window.invalid_fields.contains(&"usedPercent=142".to_owned()), "{:?}", window.invalid_fields);
        assert!(window.invalid_fields.contains(&"windowDurationMins=-5".to_owned()));
        assert!(window.invalid_fields.contains(&"resetsAt=0".to_owned()));
        assert_eq!(invalid.state, EvidenceState::Unknown);
        assert!(invalid.missing_fields.contains(&"usedPercent".to_owned()), "{:?}", invalid.missing_fields);

        // denied：明确拒绝就是 denied，不是 unknown。
        let denied = adapter.quota("codex-fixture", 1).await.unwrap();
        assert_eq!(denied.state, EvidenceState::Denied);
        assert_eq!(denied.buckets[0].permission, QuotaPermission::Denied);

        // no-permission：根层与桶内都没有该键 → unknown，并在顶层说明缺什么。
        let unknown = adapter.quota("codex-fixture", 1).await.unwrap();
        assert_eq!(unknown.state, EvidenceState::Unknown);
        assert_eq!(unknown.buckets[0].permission, QuotaPermission::Unknown);
        assert!(unknown.missing_fields.contains(&"ordinaryUsageAllowed".to_owned()), "{:?}", unknown.missing_fields);

        // fail：读取失败如实返回错误，不编造证据。
        let error = adapter.quota("codex-fixture", 1).await.unwrap_err().to_string();
        assert!(error.contains("fixture quota read failed"), "{error}");
    }

    #[tokio::test]
    async fn adapter_covers_catalog_missing_identity_legacy_key_and_failure() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("env.log");
        let launch = scenario_launch(home.path(), &log, &["missing", "models", "fail"], &[]);
        let adapter = CodexAdapter::with_launch(home.path().to_path_buf(), launch);

        let missing = adapter.models("codex-fixture", 1).await.unwrap();
        assert_eq!(missing.models.len(), 1);
        assert_eq!(missing.models[0].model_id, "preset-ok");
        assert_eq!(missing.missing_fields, vec!["model.list[0].id".to_owned()]);

        // 旧形状 `models[]` 仍然容忍。
        let legacy = adapter.models("codex-fixture", 1).await.unwrap();
        assert_eq!(legacy.models.len(), 1);
        assert_eq!(legacy.models[0].model_id, "legacy-model");
        assert!(!legacy.models[0].eligible);

        let error = adapter.models("codex-fixture", 1).await.unwrap_err().to_string();
        assert!(error.contains("fixture catalog read failed"), "{error}");
    }

    #[test]
    fn catalog_and_quota_parsers_reject_payloads_without_an_authoritative_shape() {
        // 既不是 data[] 也不是 models[]：按读取失败处理，不声称「目录为空」。
        assert!(parse_catalog(&json!({"nextCursor": null})).is_err());
        assert!(parse_catalog(&json!({"data": []})).unwrap().models.is_empty());
        // 额度：没有多桶也没有旧版单桶时仍是一次成功读取，但证据为 unknown 并记下两个视图键。
        let quota = parse_quota(&json!({"accountId": "fixture"}));
        assert_eq!(quota.view, QuotaView::Unknown);
        assert_eq!(quota.state, EvidenceState::Unknown);
        assert!(quota.observed_at.is_some());
        assert_eq!(quota.missing_fields, vec!["rateLimitsByLimitId".to_owned(), "rateLimits".to_owned()]);
        // 空的 rateLimitsByLimitId 回落到旧版单桶。
        let quota = parse_quota(&json!({
            "ordinaryUsageAllowed": true,
            "rateLimitsByLimitId": {},
            "rateLimits": {"limitId": "legacy", "primary": {"usedPercent": 3, "windowDurationMins": 60, "resetsAt": 1800000000}}
        }));
        assert_eq!(quota.view, QuotaView::RateLimits);
    }

    #[test]
    fn bucket_level_permission_is_fail_closed_against_the_root_value() {
        // 桶内显式 false + 根层 true：取更严格的一方 → 该桶 denied，整体 Denied。
        let quota = parse_quota(&json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": {"limitId": "legacy", "ordinaryUsageAllowed": false, "primary": {"usedPercent": 9, "windowDurationMins": 60, "resetsAt": 1800000000}}
        }));
        assert_eq!(quota.buckets[0].permission, QuotaPermission::Denied);
        assert_eq!(quota.state, EvidenceState::Denied);
        // 桶内非布尔 + 根层 true：unknown 并记下原始文本，绝不猜成 true。
        let quota = parse_quota(&json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": {"limitId": "legacy", "ordinaryUsageAllowed": "yes", "primary": {"usedPercent": 9, "windowDurationMins": 60, "resetsAt": 1800000000}}
        }));
        assert_eq!(quota.buckets[0].permission, QuotaPermission::Unknown);
        assert!(quota.buckets[0].invalid_fields.contains(&"ordinaryUsageAllowed=\"yes\"".to_owned()), "{:?}", quota.buckets[0].invalid_fields);
        assert_eq!(quota.state, EvidenceState::Unknown);
        assert!(quota.missing_fields.contains(&"ordinaryUsageAllowed".to_owned()));
        // 桶内 true/缺失/null 才沿用根层：根层 true → allowed。
        let quota = parse_quota(&json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": {"limitId": "legacy", "ordinaryUsageAllowed": null, "primary": {"usedPercent": 9, "windowDurationMins": 60, "resetsAt": 1800000000}}
        }));
        assert_eq!(quota.buckets[0].permission, QuotaPermission::Allowed);
        assert_eq!(quota.state, EvidenceState::Available);
    }

    #[test]
    fn a_multi_bucket_map_without_bucket_shapes_stays_labelled_as_multi() {
        // 非空 map 但值都不是对象：视图仍如实标多桶，一个桶都解析不出来，并记入顶层缺失。
        let quota = parse_quota(&json!({
            "ordinaryUsageAllowed": true,
            "rateLimitsByLimitId": {"limit-a": "nope", "limit-b": 42}
        }));
        assert_eq!(quota.view, QuotaView::RateLimitsByLimitId);
        assert_eq!(quota.source.as_deref(), Some("codex-app-server:account/rateLimits/read#rateLimitsByLimitId"));
        assert!(quota.buckets.is_empty());
        assert_eq!(quota.state, EvidenceState::Unknown);
        assert!(quota.missing_fields.contains(&"rateLimitsByLimitId".to_owned()), "{:?}", quota.missing_fields);
    }


fn review_pr40_body(protocol: Protocol, model: &str, streaming: bool) -> Value {
    let schema = json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]});
    match protocol {
        Protocol::Chat => json!({"model":model,"stream":streaming,"messages":[{"role":"user","content":"lookup"}],"tools":[{"type":"function","function":{"name":"lookup","parameters":schema}}]}),
        Protocol::Responses => json!({"model":model,"stream":streaming,"input":[{"role":"user","content":"lookup"}],"tools":[{"type":"function","name":"lookup","parameters":schema}]}),
        Protocol::Messages => json!({"model":model,"stream":streaming,"messages":[{"role":"user","content":"lookup"}],"tools":[{"name":"lookup","input_schema":schema}]}),
    }
}

fn review_pr40_followup(protocol: Protocol, mut body: Value, completion: &Value) -> Value {
    match protocol {
        Protocol::Chat => {
            let history = completion["choices"][0]["message"].clone();
            body["messages"].as_array_mut().unwrap().push(history.clone());
            for call in history["tool_calls"].as_array().unwrap() {
                let args: Value = serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
                body["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":call["id"],"content":format!("{}-result", args["key"].as_str().unwrap())}));
            }
        }
        Protocol::Responses => {
            let output = completion["output"].as_array().unwrap();
            body["input"].as_array_mut().unwrap().extend(output.iter().cloned());
            for call in output.iter().filter(|call| call["type"] == "function_call") {
                let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
                body["input"].as_array_mut().unwrap().push(json!({"type":"function_call_output","call_id":call["call_id"],"output":format!("{}-result", args["key"].as_str().unwrap())}));
            }
        }
        Protocol::Messages => {
            let content = completion["content"].clone();
            body["messages"].as_array_mut().unwrap().push(json!({"role":"assistant","content":content}));
            let results: Vec<_> = content.as_array().unwrap().iter().filter(|call| call["type"] == "tool_use").map(|call| json!({"type":"tool_result","tool_use_id":call["id"],"content":format!("{}-result", call["input"]["key"].as_str().unwrap())})).collect();
            body["messages"].as_array_mut().unwrap().push(json!({"role":"user","content":results}));
        }
    }
    body
}

fn review_pr40_decode_sse(protocol: Protocol, wire: &str) -> Value {
    let frames: Vec<Value> = wire.lines().filter_map(|line| line.strip_prefix("data: ")).filter_map(|data| serde_json::from_str(data).ok()).collect();
    match protocol {
        Protocol::Chat => {
            let calls: Vec<_> = frames.iter().filter_map(|frame| frame.pointer("/choices/0/delta/tool_calls").and_then(Value::as_array)).flatten().cloned().map(|mut call| { call.as_object_mut().unwrap().remove("index"); call }).collect();
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":calls}}]})
        }
        Protocol::Responses => frames.iter().find(|frame| frame["type"] == "response.completed").unwrap()["response"].clone(),
        Protocol::Messages => {
            let mut blocks = std::collections::BTreeMap::new();
            for frame in &frames {
                if frame["type"] == "content_block_start" { blocks.insert(frame["index"].as_u64().unwrap(), frame["content_block"].clone()); }
                if frame["type"] == "content_block_delta" && frame["delta"]["type"] == "input_json_delta" {
                    blocks.get_mut(&frame["index"].as_u64().unwrap()).unwrap()["input"] = serde_json::from_str(frame["delta"]["partial_json"].as_str().unwrap()).unwrap();
                }
            }
            json!({"content":blocks.into_values().collect::<Vec<_>>()})
        }
    }
}

async fn review_pr40_read(response: axum::response::Response) -> (u16, String) {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn review_pr40_fixture(text_before_tools: bool) -> (tempfile::TempDir, PathBuf, Arc<CodexAdapter>, Arc<crate::config::ConfigStore>) {
    review_pr40_fixture_impl(text_before_tools, true)
}

fn review_pr40_fixture_impl(text_before_tools: bool, wait_for_text: bool) -> (tempfile::TempDir, PathBuf, Arc<CodexAdapter>, Arc<crate::config::ConfigStore>) {
    let home = tempfile::tempdir().unwrap();
    let log = home.path().join("review-helper.log");
    let launch = fixture_launch(home.path(), &log);
    if text_before_tools {
        let before = r#"          printf '{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"threadId":"%s","turnId":"%s","delta":"before tools"}}\n' "$thread_id" "$turn_id"
          sleep 0.060
"#;
        let helper = FIXTURE_HELPER.replace("        elif [ \"$scenario\" = dynamic-tools ]; then\n", &format!("        elif [ \"$scenario\" = dynamic-tools ]; then\n{before}"));
        let helper = if wait_for_text { helper } else { helper.replace("          sleep 0.060\n", "") };
        assert_ne!(helper, FIXTURE_HELPER);
        std::fs::write(&launch.program, helper).unwrap();
    }
    std::fs::write(format!("{}.generation", log.to_string_lossy()), "dynamic-tools").unwrap();
    let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), launch));
    let store = admitted_gateway_store(&home.path().join("review.db"), adapter.clone(), "http://127.0.0.1:9");
    (home, log, adapter, store)
}

fn review_pr40_rpc(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(format!("{}.rpc", log.to_string_lossy())).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[tokio::test]
async fn review_pr40_gateway_three_protocols_json_and_sse_complete_client_tools() {
    for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
        for streaming in [false, true] {
            let (_home, log, adapter, store) = review_pr40_fixture(false);
            let body = review_pr40_body(protocol, "autojev/model/codex-fixture-binding", streaming);
            let initial = crate::proxy::forward_test_request(store.clone(), body.clone(), gateway_endpoint(protocol)).await;
            let (status, wire) = review_pr40_read(initial).await;
            assert_eq!(status, 200, "{protocol:?} stream={streaming}: {wire}");
            let completion: Value = if streaming { review_pr40_decode_sse(protocol, &wire) } else { serde_json::from_str(&wire).unwrap() };
            let next = review_pr40_followup(protocol, body, &completion);
            let (status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), next, gateway_endpoint(protocol)).await).await;
            assert_eq!(status, 200, "{protocol:?} stream={streaming}: {wire}");
            assert!(wire.contains("continued"), "{protocol:?} stream={streaming}: {wire}");
            let rpc = review_pr40_rpc(&log);
            assert_eq!(rpc.iter().filter(|call| call["method"] == "thread/start").count(), 1);
            assert_eq!(rpc.iter().filter(|call| call["method"] == "turn/start").count(), 1);
            let results = std::fs::read_to_string(format!("{}.tool-responses", log.to_string_lossy())).unwrap();
            assert!(results.contains("one-result") && results.contains("two-result"));
            assert!(!rpc.iter().any(|call| call["method"] == "commandExecution"));
            assert!(adapter.pending_tool_turns.lock().unwrap().is_empty());
            println!("CONTROL PASS {protocol:?} streaming={streaming}: schemas, 2 call IDs, matching results, next response, one helper turn");
            adapter.logout("codex-fixture", 1).await.unwrap();
        }
    }
}

#[tokio::test]
async fn review_pr40_model_swap_is_rejected_without_consuming_pending_turn() {
    let (_home, log, adapter, store) = review_pr40_fixture(false);
    store.update(|config| {
        let mut second = config.models.iter().find(|model| model.id == "codex-fixture-binding").unwrap().clone();
        second.id = "codex-second-binding".into();
        second.model_id = "second-model".into();
        config.models.push(second.clone());
        let connection = config.subscriptions.get_mut("codex-fixture").unwrap();
        connection.evidence.as_mut().unwrap().capabilities.push(crate::subscription::Capability {
            model_id: second.model_id.clone(), protocol: crate::subscription::protocol_key(Protocol::Chat).into(), status: crate::subscription::CapabilityStatus::Verified,
        });
        let catalog = config.subscription_catalogs.get_mut("codex-fixture").unwrap();
        let mut entry = catalog.entries[0].clone();
        entry.model_id = second.model_id;
        entry.internal_id = second.id;
        catalog.entries.push(entry);
    }).unwrap();
    let body = review_pr40_body(Protocol::Chat, "autojev/model/codex-fixture-binding", false);
    let (status, first) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), body.clone(), "chat/completions").await).await;
    assert_eq!(status, 200, "{first}");
    let completion: Value = serde_json::from_str(&first).unwrap();
    let mut wrong_model = review_pr40_followup(Protocol::Chat, body.clone(), &completion);
    wrong_model["model"] = "autojev/model/codex-second-binding".into();
    let (status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), wrong_model, "chat/completions").await).await;
    assert!(status >= 400, "a tool continuation cannot switch the model mid-turn: {wire}");
    assert_eq!(adapter.pending_tool_turns.lock().unwrap().len(), 1, "reject the mismatch while preserving the valid continuation");
    assert!(!review_pr40_rpc(&log).iter().any(|call| call["method"] == "turn/interrupt"));
    let next = review_pr40_followup(Protocol::Chat, body, &completion);
    let (status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), next, "chat/completions").await).await;
    assert_eq!(status, 200, "the original model can still continue: {wire}");
    assert!(wire.contains("continued"));
    assert!(adapter.pending_tool_turns.lock().unwrap().is_empty());
    let rpc = review_pr40_rpc(&log);
    assert_eq!(rpc.iter().filter(|call| call["method"] == "thread/start").count(), 1);
    let started = rpc.iter().find(|call| call["method"] == "thread/start").unwrap();
    assert_eq!(started["params"]["model"], "codex-fixture-model");
    println!("REGRESSION PASS model binding: a model swap is rejected before tool results dispatch; original continuation succeeds");
    adapter.logout("codex-fixture", 1).await.unwrap();
}
#[tokio::test]
async fn review_pr40_unrelated_request_preserves_pending_tool_turn() {
    let (_home, log, adapter, store) = review_pr40_fixture(false);
    let body = review_pr40_body(Protocol::Chat, "autojev/model/codex-fixture-binding", false);
    let (status, first) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), body.clone(), "chat/completions").await).await;
    assert_eq!(status, 200);
    let unrelated = json!({"model":"autojev/model/codex-fixture-binding","messages":[{"role":"user","content":"different client request"}]});
    let (other_status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), unrelated, "chat/completions").await).await;
    assert!(other_status >= 400, "the single pending Codex turn does not support another request: {wire}");
    assert_eq!(adapter.pending_tool_turns.lock().unwrap().len(), 1, "an unrelated client must not consume another continuation");
    assert!(!review_pr40_rpc(&log).iter().any(|call| call["method"] == "turn/interrupt"));
    let next = review_pr40_followup(Protocol::Chat, body, &serde_json::from_str(&first).unwrap());
    let (status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), next, "chat/completions").await).await;
    assert_eq!(status, 200, "the original tool continuation remains usable: {wire}");
    assert!(wire.contains("continued"));
    assert!(adapter.pending_tool_turns.lock().unwrap().is_empty());
    assert_eq!(review_pr40_rpc(&log).iter().filter(|call| call["method"] == "thread/start").count(), 1);
    println!("REGRESSION PASS pending identity: unrelated request rejected, original turn retained and continued");
    adapter.logout("codex-fixture", 1).await.unwrap();
}
#[tokio::test]
async fn review_pr40_responses_accepts_its_echoed_text_and_tool_items() {
    let (_home, log, adapter, store) = review_pr40_fixture(true);
    let body = review_pr40_body(Protocol::Responses, "autojev/model/codex-fixture-binding", false);
    let (status, first) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), body.clone(), "responses").await).await;
    assert_eq!(status, 200, "{first}");
    assert!(first.contains("before tools"));
    let completion: Value = serde_json::from_str(&first).unwrap();
    let next = review_pr40_followup(Protocol::Responses, body, &completion);
    let (status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), next, "responses").await).await;
    assert_eq!(status, 200, "the API's own id/status/annotations output history is valid input: {wire}");
    assert!(wire.contains("continued"), "the same live turn should resume: {wire}");
    assert!(adapter.pending_tool_turns.lock().unwrap().is_empty());
    let rpc = review_pr40_rpc(&log);
    assert_eq!(rpc.iter().filter(|call| call["method"] == "thread/start").count(), 1);
    assert_eq!(rpc.iter().filter(|call| call["method"] == "turn/start").count(), 1);
    let results = std::fs::read_to_string(format!("{}.tool-responses", log.to_string_lossy())).unwrap();
    assert!(results.contains("one-result") && results.contains("two-result"));
    println!("REGRESSION PASS Responses: complete echoed output items continue without caller-side field stripping");
    adapter.logout("codex-fixture", 1).await.unwrap();
}
#[tokio::test]
async fn review_pr40_sse_disconnect_before_terminal_discards_only_its_pending_turn() {
    let (_home, log, adapter, store) = review_pr40_fixture(false);
    let body = review_pr40_body(Protocol::Chat, "autojev/model/codex-fixture-binding", true);
    let response = crate::proxy::forward_test_request(store.clone(), body.clone(), "chat/completions").await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // Hold a second account's continuation at the same time. Dropping this HTTP body must only
    // abandon its own provider/generation/call ids.
    let other_body = json!({"model":"fixture-model","messages":[{"role":"user","content":"lookup"}],
        "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"key":{"type":"string"}}}}}]});
    let mut other_events = adapter.generate(GenerationRequest {
        provider_id: "codex-other",
        generation: 77,
        model_id: "fixture-model",
        protocol: Protocol::Chat,
        body: other_body,
        pre_dispatch_check: Arc::new(|| Ok(())),
    }).await.unwrap();
    assert_eq!(other_events.next().await, Some(GenerationEvent::Started { generation: 77 }));
    let other_calls = match other_events.next().await {
        Some(GenerationEvent::ToolCalls { calls }) => calls,
        other => panic!("expected the other account's pending tools, got {other:?}"),
    };
    drop(other_events);
    let (first_turn_id, other_turn_id) = {
        let pending = adapter.pending_tool_turns.lock().unwrap();
        assert_eq!(pending.len(), 2);
        (pending["codex-fixture"].turn_id.clone(), pending["codex-other"].turn_id.clone())
    };

    let mut chunks = response.into_body().into_data_stream();
    let mut observed = String::new();
    loop {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), chunks.next()).await.unwrap().unwrap().unwrap();
        observed.push_str(std::str::from_utf8(&chunk).unwrap());
        if review_pr40_decode_sse(Protocol::Chat, &observed)["choices"][0]["message"]["tool_calls"].as_array().unwrap().len() == 2 { break; }
    }
    assert!(!observed.contains("[DONE]") && !observed.contains("\"finish_reason\":\"tool_calls\""));
    drop(chunks);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let pending = adapter.pending_tool_turns.lock().unwrap();
            let own_pending_removed = !pending.contains_key("codex-fixture");
            let other_pending_preserved = pending.get("codex-other").is_some_and(|turn| turn.turn_id == other_turn_id);
            drop(pending);
            let own_interrupt_sent = review_pr40_rpc(&log).iter().any(|call|
                call["method"] == "turn/interrupt" && call["params"]["turnId"] == first_turn_id);
            if own_pending_removed && other_pending_preserved && own_interrupt_sent { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("body drop should interrupt only its associated pending turn");
    assert!(!review_pr40_rpc(&log).iter().any(|call|
        call["method"] == "turn/interrupt" && call["params"]["turnId"] == other_turn_id));
    adapter.abandon_client_tool_calls("codex-other", 77, &other_calls.iter().map(|call| call.id.clone()).collect::<Vec<_>>());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if !adapter.pending_tool_turns.lock().unwrap().contains_key("codex-other")
                && review_pr40_rpc(&log).iter().any(|call|
                    call["method"] == "turn/interrupt" && call["params"]["turnId"] == other_turn_id) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("the other fixture turn should be cleaned up after its assertions");
    let mut next = review_pr40_followup(Protocol::Chat, body, &review_pr40_decode_sse(Protocol::Chat, &observed));
    next["stream"] = false.into();
    let (status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), next, "chat/completions").await).await;
    assert!(status >= 400, "a client disconnected before the tool response terminal and cannot resume it: {wire}");
    println!("REGRESSION PASS SSE lifetime: early HTTP body drop interrupts and discards the matching pending turn");
    adapter.logout("codex-fixture", 1).await.unwrap();
}
#[tokio::test]
async fn review_pr40_helper_events_preserve_text_before_tool_requests() {
    for _ in 0..24 {
        let (_home, _log, adapter, store) = review_pr40_fixture_impl(true, false);
        let body = review_pr40_body(Protocol::Chat, "autojev/model/codex-fixture-binding", false);
        let (status, wire) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), body.clone(), "chat/completions").await).await;
        assert_eq!(status, 200, "{wire}");
        assert!(wire.contains("before tools"), "the first response must preserve the helper's event order: {wire}");
        let next = review_pr40_followup(Protocol::Chat, body, &serde_json::from_str(&wire).unwrap());
        let (status, continuation) = review_pr40_read(crate::proxy::forward_test_request(store.clone(), next, "chat/completions").await).await;
        assert_eq!(status, 200, "{continuation}");
        assert!(continuation.contains("continued"), "{continuation}");
        assert!(!continuation.contains("before tools"), "prior output must not leak to the next HTTP response: {continuation}");
        adapter.logout("codex-fixture", 1).await.unwrap();
    }
    println!("REGRESSION PASS event ordering: 24/24 helper text deltas precede tool-call terminal responses");
}
#[tokio::test]
async fn review400_cleanup_must_not_consume_the_next_turn_events() {
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("ordered-cleanup-helper.mjs");
    let log = home.path().join("ordered-cleanup.log");
    std::fs::write(&script, r#"
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';
const log = process.argv[2];
const record = (direction, value) => appendFileSync(log, JSON.stringify({direction, ...value})+'\n');
const emit = value => { record('out', value); process.stdout.write(JSON.stringify(value)+'\n'); };
const result = (id, value) => emit({jsonrpc:'2.0', id, result:value});
const notify = (method, params) => emit({jsonrpc:'2.0', method, params});
let nextThread = 0;
createInterface({ input:process.stdin }).on('line', line => {
  const request = JSON.parse(line); record('in', request);
  const p = request.params || {};
  if (request.method === 'initialize') result(request.id, {userAgent:'review-helper-1.0'});
  if (request.method === 'thread/start') {
    const id = `thread-review-${++nextThread}`;
    result(request.id, {thread:{id}, modelProvider:p.modelProvider});
  }
  if (request.method === 'turn/start') {
    const threadId = p.threadId;
    const turnId = threadId === 'thread-review-1' ? 'turn-A' : 'turn-B';
    result(request.id, {turn:{id:turnId}});
    if (turnId === 'turn-A') setTimeout(() => {
      for (const key of ['one','two']) emit({jsonrpc:'2.0', id:`rpc-${key}`, method:'item/tool/call', params:{threadId, turnId, callId:`internal-${key}`, tool:'lookup', arguments:{key}}});
    }, 15);
    else setTimeout(() => {
      notify('item/agentMessage/delta', {threadId, turnId, itemId:'message-B', delta:'second-text'});
      notify('turn/completed', {threadId, turn:{id:turnId, status:'completed'}});
    }, 40);
  }
  if (request.method === 'turn/interrupt') {
    result(request.id, {});
    setTimeout(() => notify('turn/completed', {threadId:p.threadId, turn:{id:p.turnId, status:'interrupted'}}), 180);
  }
  if (request.method === 'account/logout') result(request.id, {});
});
"#).unwrap();
    let node = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join("node"))
        .find(|path| path.is_file())
        .unwrap();
    let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_path_buf(), HelperLaunch {
        program: node,
        args: vec![script.to_string_lossy().into(), log.to_string_lossy().into()],
    }));
    let store = admitted_gateway_store(&home.path().join("cleanup.db"), adapter.clone(), "http://127.0.0.1:9");
    let first_body = review_pr40_body(Protocol::Chat, "autojev/model/codex-fixture-binding", true);
    let response = crate::proxy::forward_test_request(store.clone(), first_body, "chat/completions").await;
    assert_eq!(response.status().as_u16(), 200);
    let mut chunks = response.into_body().into_data_stream();
    let mut wire = String::new();
    loop {
        let chunk = chunks.next().await.unwrap().unwrap();
        wire.push_str(std::str::from_utf8(&chunk).unwrap());
        if review_pr40_decode_sse(Protocol::Chat, &wire)["choices"][0]["message"]["tool_calls"].as_array().unwrap().len() == 2 { break; }
    }
    assert!(!wire.contains("[DONE]"));
    drop(chunks);
    let next = tokio::time::timeout(std::time::Duration::from_millis(800), crate::proxy::forward_test_request(
        store.clone(),
        json!({"model":"autojev/model/codex-fixture-binding","messages":[{"role":"user","content":"independent next request"}]}),
        "chat/completions",
    )).await;
    let observed = match next {
        Ok(response) => Some(review_pr40_read(response).await),
        Err(_) => None,
    };
    let events: Vec<Value> = std::fs::read_to_string(&log).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    let second_started = events.iter().position(|event| event["direction"] == "in" && event["method"] == "turn/start" && event["params"]["threadId"] == "thread-review-2");
    let old_cleanup_done = events.iter().position(|event| event["direction"] == "out" && event["method"] == "turn/completed" && event["params"]["turn"]["id"] == "turn-A");
    let second_completed = events.iter().any(|event| event["direction"] == "out" && event["method"] == "turn/completed" && event["params"]["turn"]["id"] == "turn-B" && event["params"]["turn"]["status"] == "completed");
    adapter.logout("codex-fixture", 1).await.unwrap();
    assert!(second_started.is_some(), "the next admitted request should run after cleanup completes");
    assert!(old_cleanup_done.is_some_and(|old| second_started.is_some_and(|new| old < new)), "the old cleanup must finish before the next turn is dispatched");
    assert!(second_completed, "the helper should complete the next turn");
    assert!(observed.as_ref().is_some_and(|(status, body)| *status == 200 && body.contains("second-text")), "cleanup consumed or lost the next turn's events: {observed:?}");
}

#[tokio::test]
async fn review_pr40_gateway_loopback_http_three_protocols_tools() {
    for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
        for streaming in [false, true] {
            let (_home, log, adapter, store) = review_pr40_fixture(false);
            let gateway = crate::proxy::start(store.clone()).await.unwrap();
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let url = format!("http://127.0.0.1:{}{}", gateway.port, protocol.path());
            let body = review_pr40_body(protocol, "autojev/model/codex-fixture-binding", streaming);
            let initial = client.post(&url).json(&body).send().await.unwrap();
            assert_eq!(initial.status().as_u16(), 200);
            if streaming { assert!(initial.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream")); }
            let wire = initial.text().await.unwrap();
            let completion: Value = if streaming { review_pr40_decode_sse(protocol, &wire) } else { serde_json::from_str(&wire).unwrap() };
            let next = review_pr40_followup(protocol, body, &completion);
            let response = client.post(&url).json(&next).send().await.unwrap();
            assert_eq!(response.status().as_u16(), 200);
            let wire = response.text().await.unwrap();
            assert!(wire.contains("continued"), "{protocol:?} stream={streaming}: {wire}");
            if streaming {
                assert!(wire.contains(match protocol { Protocol::Chat => "[DONE]", Protocol::Responses => "response.completed", Protocol::Messages => "message_stop" }));
            }
            let rpc = review_pr40_rpc(&log);
            assert_eq!(rpc.iter().filter(|call| call["method"] == "thread/start").count(), 1);
            assert_eq!(rpc.iter().filter(|call| call["method"] == "turn/start").count(), 1);
            let thread = rpc.iter().find(|call| call["method"] == "thread/start").unwrap();
            assert_eq!(thread["params"]["dynamicTools"][0]["inputSchema"]["required"][0], "key");
            assert_eq!(thread["params"]["config"]["features"]["shell_tool"], false);
            let results = std::fs::read_to_string(format!("{}.tool-responses", log.to_string_lossy())).unwrap();
            assert!(results.contains("one-result") && results.contains("two-result"));
            println!("HTTP CONTROL PASS {protocol:?} streaming={streaming}: actual loopback /v1 endpoint, schema, client 2-tool execution/results, continued response and terminal");
            gateway.stop().await;
            adapter.logout("codex-fixture", 1).await.unwrap();
        }
    }
}

fn review_pr44_app_state(store: Arc<crate::config::ConfigStore>) -> crate::AppState {
    crate::AppState {
        performance: Arc::new(crate::performance::Runner::default()),
        store,
        proxy: Arc::new(tokio::sync::Mutex::new(None)),
        sessions: Arc::new(tokio::sync::Mutex::new(crate::subscription::SessionState::default())),
    }
}

fn review_pr44_budget(store: &crate::config::ConfigStore, limit: u32, used: u32) {
    store.update(|config| {
        config.codex_real_generation_grants.remove("codex-fixture");
        crate::subscription::set_codex_real_generation_enabled(config, "codex-fixture", true, Some(limit)).unwrap();
        for _ in 0..used {
            crate::subscription::reserve_codex_real_generation_call(config, "codex-fixture").unwrap();
        }
    }).unwrap();
}

#[tokio::test]
async fn review_pr44_rename_preserves_spent_budget_for_same_generation() {
    use tauri::Manager;
    let home = tempfile::tempdir().unwrap();
    let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_owned(), fixture_launch(home.path(), &home.path().join("rpc.log"))));
    let store = admitted_gateway_store(&home.path().join("config.db"), adapter, "http://127.0.0.1:9");
    review_pr44_budget(&store, 2, 1);
    let before = store.read().subscriptions["codex-fixture"].clone();
    let mut provider = store.read().providers.iter().find(|provider| provider.id == "codex-fixture").unwrap().clone();
    provider.id = "codex-renamed".into();
    let app = tauri::test::mock_app();
    app.manage(review_pr44_app_state(store.clone()));
    crate::save_provider(app.state::<crate::AppState>(), provider.clone(), None, None, Some("codex-fixture".into()), Some(false)).await.unwrap();
    let after = store.read().subscriptions["codex-renamed"].clone();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.identity, before.identity);
    crate::set_codex_real_generation_enabled(
        app.state::<crate::AppState>(), "codex-renamed".into(), true, Some(2), after.connection_instance_id.clone(), after.generation, after.identity.as_deref().unwrap().to_owned(),
    ).await.unwrap();
    let actual = crate::subscription::codex_real_generation_call_counts(&store.read(), &provider);
    assert_eq!(actual, Some((2, 1)), "renaming must not replenish spent requests for the same account and generation");
}

#[tokio::test]
async fn review_pr44_failed_login_does_not_replenish_exhausted_budget() {
    use tauri::Manager;
    let home = tempfile::tempdir().unwrap();
    let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_owned(), HelperLaunch { program: home.path().join("missing-fake-helper"), args: vec![] }));
    let store = admitted_gateway_store(&home.path().join("config.db"), adapter, "http://127.0.0.1:9");
    review_pr44_budget(&store, 1, 1);
    let before = store.read().subscriptions["codex-fixture"].clone();
    let app = tauri::test::mock_app();
    app.manage(review_pr44_app_state(store.clone()));
    assert!(crate::begin_subscription_login(app.state::<crate::AppState>(), "codex-fixture".into()).await.is_err());
    let after = store.read().subscriptions["codex-fixture"].clone();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.identity, before.identity);
    assert_eq!(after.state, crate::subscription::ConnectionState::Connected);
    let rearm = crate::set_codex_real_generation_enabled(
        app.state::<crate::AppState>(), "codex-fixture".into(), true, Some(1), after.connection_instance_id.clone(), after.generation, after.identity.as_deref().unwrap().to_owned(),
    ).await;
    assert!(rearm.is_err(), "a failed login attempt cannot renew an exhausted budget in the unchanged generation");
}

#[tokio::test]
async fn review_pr44_delete_and_recreate_does_not_inherit_consent() {
    use tauri::Manager;
    let home = tempfile::tempdir().unwrap();
    let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_owned(), fixture_launch(home.path(), &home.path().join("rpc.log"))));
    let store = admitted_gateway_store(&home.path().join("config.db"), adapter, "http://127.0.0.1:9");
    review_pr44_budget(&store, 2, 1);
    let provider = store.read().providers.iter().find(|provider| provider.id == "codex-fixture").unwrap().clone();
    let app = tauri::test::mock_app();
    app.manage(review_pr44_app_state(store.clone()));
    crate::delete_provider(app.state::<crate::AppState>(), provider.id.clone()).await.unwrap();
    crate::save_provider(app.state::<crate::AppState>(), provider.clone(), None, None, None, Some(true)).await.unwrap();
    // Local callback fixture: account B confirms identity without OAuth or model dispatch.
    store.update(|config| {
        let connection = config.subscriptions.get_mut(&provider.id).unwrap();
        connection.state = crate::subscription::ConnectionState::Connected;
        connection.identity = Some("new-account@example.invalid".into());
    }).unwrap();
    let config = store.read();
    assert!(!crate::subscription::codex_real_generation_enabled(&config, &provider), "a recreated provider must start default-off even if its generation number is reused");
    assert_eq!(crate::subscription::codex_real_generation_call_counts(&config, &provider), None, "a recreated provider must not display the previous account's budget");
}

#[tokio::test]
async fn review_pr44_delayed_old_confirmation_cannot_arm_new_generation() {
    use tauri::Manager;
    let home = tempfile::tempdir().unwrap();
    let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_owned(), fixture_launch(home.path(), &home.path().join("rpc.log"))));
    let store = admitted_gateway_store(&home.path().join("config.db"), adapter, "http://127.0.0.1:9");
    store.update(|config| { config.codex_real_generation_grants.clear(); }).unwrap();
    let confirmed = store.read().subscriptions["codex-fixture"].clone();
    store.update(|config| {
        let connection = config.subscriptions.get_mut("codex-fixture").unwrap();
        connection.generation += 1;
        connection.identity = Some("new-account@example.invalid".into());
        connection.evidence = None;
    }).unwrap();
    let app = tauri::test::mock_app();
    app.manage(review_pr44_app_state(store.clone()));
    let result = crate::set_codex_real_generation_enabled(
        app.state::<crate::AppState>(), "codex-fixture".into(), true, Some(3),
        confirmed.connection_instance_id.clone(), confirmed.generation, confirmed.identity.as_deref().unwrap().to_owned(),
    ).await;
    assert!(result.is_err(), "consent for the old snapshot must not arm a different connection generation");
}

#[tokio::test]
async fn review_pr44_recreated_connection_same_account_rejects_old_confirmation() {
    use tauri::Manager;
    let home = tempfile::tempdir().unwrap();
    let log = home.path().join("recreated-same-account.log");
    let adapter = Arc::new(CodexAdapter::with_launch(home.path().to_owned(), fixture_launch(home.path(), &log)));
    let store = admitted_gateway_store(&home.path().join("config.db"), adapter, "http://127.0.0.1:9");
    review_pr44_budget(&store, 1, 1);
    let confirmed = store.read().subscriptions["codex-fixture"].clone();
    let provider = store.read().providers.iter().find(|provider| provider.id == "codex-fixture").unwrap().clone();
    let app = tauri::test::mock_app();
    app.manage(review_pr44_app_state(store.clone()));
    crate::delete_provider(app.state::<crate::AppState>(), provider.id.clone()).await.unwrap();
    crate::save_provider(app.state::<crate::AppState>(), provider.clone(), None, None, None, Some(true)).await.unwrap();
    crate::begin_subscription_login(app.state::<crate::AppState>(), provider.id.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while store.read().subscriptions[&provider.id].state != crate::subscription::ConnectionState::Connected {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("the isolated helper must finish the recreated fictional login");
    let recreated = store.read().subscriptions[&provider.id].clone();
    assert_eq!(recreated.generation, confirmed.generation, "deleting/recreating the provider reuses numeric generation 1");
    assert_eq!(recreated.identity, confirmed.identity, "the fictional helper reconnects the same account identity");
    assert_ne!(recreated.connection_instance_id, confirmed.connection_instance_id, "deleting and recreating a connection must issue a fresh instance token");

    // Same account and numeric generation as before deletion: only the instance token distinguishes this connection.
    let result = crate::set_codex_real_generation_enabled(
        app.state::<crate::AppState>(),
        provider.id.clone(),
        true,
        Some(1),
        confirmed.connection_instance_id.clone(),
        confirmed.generation,
        confirmed.identity.clone().unwrap(),
    ).await;
    let config = store.read();
    assert!(result.is_err(), "a confirmation captured before deletion must not arm the recreated connection");
    assert_eq!(crate::subscription::codex_real_generation_call_counts(&config, &provider), None);
}
}
