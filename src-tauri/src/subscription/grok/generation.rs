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
use std::collections::HashSet;
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
use super::{CompletedClientToolExchange, GrokSubscriptionAdapter, PendingClientToolCall, PendingClientToolTurn};
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

#[derive(Debug, Clone, PartialEq)]
struct FunctionTool {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolChoice {
    Auto,
    None,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedToolCall {
    id: String,
    name: String,
    arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedToolResult {
    id: String,
    output: String,
    is_error: bool,
}

#[derive(Debug, Clone)]
struct TextTurn {
    text: Vec<String>,
    max_tokens: Option<u64>,
    tools: Vec<FunctionTool>,
    tool_choice: ToolChoice,
    allow_parallel: bool,
    tool_calls: Vec<ParsedToolCall>,
    tool_call_batch_sizes: Vec<usize>,
    tool_results: Vec<ParsedToolResult>,
    assistant_text: Vec<String>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct AcpClientToolCall {
    id: String,
    name: String,
    arguments: Value,
}

/// Keep the text-only contract for requests without client function tools or tool history.
fn parse_plain_text_turn(protocol: Protocol, body: &Value, model_id: &str) -> Result<TextTurn> {
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
    Ok(TextTurn {
        text,
        max_tokens,
        tools: Vec::new(),
        tool_choice: ToolChoice::Auto,
        allow_parallel: true,
        tool_calls: Vec::new(),
        tool_call_batch_sizes: Vec::new(),
        tool_results: Vec::new(),
        assistant_text: Vec::new(),
    })
}

fn parse_text_turn(protocol: Protocol, body: &Value, model_id: &str) -> Result<TextTurn> {
    if body.get("tools").is_some()
        || body.get("tool_choice").is_some()
        || body.get("parallel_tool_calls").is_some()
        || request_has_tool_history(protocol, body)
    {
        return parse_client_tool_turn(protocol, body, model_id);
    }
    parse_plain_text_turn(protocol, body, model_id)
}

fn request_has_tool_history(protocol: Protocol, body: &Value) -> bool {
    match protocol {
        Protocol::Chat => body.get("messages").and_then(Value::as_array).is_some_and(|messages| {
            messages.iter().any(|message| {
                matches!(message.get("role").and_then(Value::as_str), Some("tool"))
                    || message.get("tool_calls").is_some()
            })
        }),
        Protocol::Responses => body.get("input").and_then(Value::as_array).is_some_and(|items| {
            items.iter().any(|item| matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call" | "function_call_output")
            ))
        }),
        Protocol::Messages => body.get("messages").and_then(Value::as_array).is_some_and(|messages| {
            messages.iter().any(|message| {
                message.get("content").and_then(Value::as_array).is_some_and(|blocks| {
                    blocks.iter().any(|block| matches!(
                        block.get("type").and_then(Value::as_str),
                        Some("tool_use" | "tool_result")
                    ))
                })
            })
        }),
    }
}

fn parse_client_tool_turn(protocol: Protocol, body: &Value, model_id: &str) -> Result<TextTurn> {
    ensure!(!model_id.trim().is_empty() && model_id.len() <= 256, "A bounded model id is required");
    let allowed: &[&str] = match protocol {
        Protocol::Chat => &["model", "stream", "messages", "max_tokens", "max_completion_tokens", "tools", "tool_choice", "parallel_tool_calls"],
        Protocol::Responses => &["model", "stream", "input", "max_output_tokens", "tools", "tool_choice", "parallel_tool_calls"],
        Protocol::Messages => &["model", "stream", "messages", "max_tokens", "tools", "tool_choice"],
    };
    reject_fields(body, allowed, "Grok generation request")?;
    if let Some(value) = body.get("model") {
        let requested = value.as_str().context("model must be a string")?;
        ensure!(!requested.trim().is_empty() && requested.len() <= 256, "model must be a bounded non-empty string");
    }
    if body.get("stream").is_some_and(|value| !value.is_boolean()) {
        bail!("stream must be a boolean");
    }
    let max_tokens = request_max_tokens(protocol, body)?;
    if protocol == Protocol::Messages {
        ensure!(max_tokens.is_some(), "Messages requires a positive integer max_tokens");
    }
    let tools = parse_function_tools(protocol, body)?;
    let (tool_choice, allow_parallel) = parse_tool_choice(protocol, body)?;
    let mut text = Vec::new();
    let mut assistant_text = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_call_batch_sizes = Vec::new();
    let mut tool_results = Vec::new();

    match protocol {
        Protocol::Chat => {
            let messages = body.get("messages").and_then(Value::as_array).context("Chat Completions requires a messages array")?;
            for message in messages {
                reject_fields(message, &["role", "content", "tool_calls", "tool_call_id"], "Chat message")?;
                match message.get("role").and_then(Value::as_str) {
                    Some("user") => text.extend(text_value(message.get("content").context("A user message requires content")?)?),
                    Some("assistant") => {
                        assistant_text.extend(optional_text_value(message.get("content"), "assistant content")?);
                        let batch_start = tool_calls.len();
                        for call in message.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                            reject_fields(call, &["id", "type", "function"], "Chat tool call")?;
                            ensure!(call.get("type").and_then(Value::as_str) == Some("function"), "Only function tool calls are supported");
                            let function = call.get("function").context("A Chat tool call requires function")?;
                            reject_fields(function, &["name", "arguments"], "Chat function call")?;
                            let name = required_string(function.get("name"), "Chat tool name")?;
                            let raw = function.get("arguments").and_then(Value::as_str).context("Function arguments must be a JSON string")?;
                            let arguments: Value = serde_json::from_str(raw).context("Function arguments must contain valid JSON")?;
                            tool_calls.push(parse_call(call.get("id"), name, arguments)?);
                        }
                        let batch_size = tool_calls.len() - batch_start;
                        if batch_size > 0 {
                            tool_call_batch_sizes.push(batch_size);
                        }
                    }
                    Some("tool") => {
                        let id = required_string(message.get("tool_call_id"), "tool_call_id")?;
                        let output = parse_tool_output(message.get("content").context("A tool result requires content")?)?;
                        tool_results.push(ParsedToolResult { id, output, is_error: false });
                    }
                    Some(role) => bail!("Grok ACP cannot preserve Chat message role {role}"),
                    None => bail!("Each Chat message requires a string role"),
                }
            }
        }
        Protocol::Responses => {
            let input = body.get("input").context("Responses requires input")?;
            let mut response_call_batch_size = 0;
            match input {
                Value::String(value) => text.push(value.clone()),
                Value::Array(items) => for item in items {
                    match item.get("type").and_then(Value::as_str).unwrap_or("message") {
                        "message" => match required_string(item.get("role"), "Responses message role")?.as_str() {
                            "user" => text.extend(text_value(item.get("content").context("A Responses message requires content")?)?),
                            "assistant" => {
                                reject_fields(item, &["type", "id", "status", "role", "content"], "Responses assistant message")?;
                                ensure!(item.get("id").and_then(Value::as_str).is_some_and(|id| id.starts_with("msg_")), "Responses assistant history requires its emitted message ID");
                                ensure!(item.get("status").and_then(Value::as_str) == Some("completed"), "Only completed Responses assistant history is supported");
                                assistant_text.extend(responses_assistant_text(item.get("content").context("Assistant output requires content")?)?);
                            }
                            _ => bail!("Grok ACP supports user and assistant Responses messages only"),
                        },
                        "function_call" => {
                            reject_fields(item, &["type", "id", "call_id", "name", "arguments", "status"], "Responses function call")?;
                            ensure!(item.get("id").and_then(Value::as_str).is_some_and(|id| id.starts_with("fc_")), "Responses function-call history requires its emitted item ID");
                            ensure!(item.get("status").and_then(Value::as_str) == Some("completed"), "Only completed Responses function-call history is supported");
                            let name = required_string(item.get("name"), "Responses tool name")?;
                            let raw = item.get("arguments").and_then(Value::as_str).context("Responses function arguments must be a JSON string")?;
                            let arguments: Value = serde_json::from_str(raw).context("Responses function arguments must contain valid JSON")?;
                            tool_calls.push(parse_call(item.get("call_id"), name, arguments)?);
                            response_call_batch_size += 1;
                        }
                        "function_call_output" => {
                            if response_call_batch_size > 0 {
                                tool_call_batch_sizes.push(response_call_batch_size);
                                response_call_batch_size = 0;
                            }
                            reject_fields(item, &["type", "id", "call_id", "output"], "Responses function result")?;
                            let id = required_string(item.get("call_id"), "Responses call_id")?;
                            let output = parse_tool_output(item.get("output").context("A function result requires output")?)?;
                            tool_results.push(ParsedToolResult { id, output, is_error: false });
                        }
                        kind => bail!("Unsupported Grok Responses input type {kind}"),
                    }
                },
                _ => bail!("Responses requires string input or an input array"),
            }
            if response_call_batch_size > 0 {
                tool_call_batch_sizes.push(response_call_batch_size);
            }
        }
        Protocol::Messages => {
            ensure!(body.get("system").map_or(true, Value::is_null), "Grok ACP cannot preserve Messages system instructions");
            let messages = body.get("messages").and_then(Value::as_array).context("Messages requires a messages array")?;
            for message in messages {
                reject_fields(message, &["role", "content"], "Messages message")?;
                match required_string(message.get("role"), "Messages message role")?.as_str() {
                    "user" => {
                        let content = message.get("content").context("A user message requires content")?;
                        if let Some(value) = content.as_str() {
                            text.push(value.to_owned());
                        } else {
                            for block in content.as_array().context("Messages content must be text or an array of blocks")? {
                                match block.get("type").and_then(Value::as_str) {
                                    Some("text") => {
                                        reject_fields(block, &["type", "text"], "Messages text block")?;
                                        text.push(required_string(block.get("text"), "Messages text")?);
                                    }
                                    Some("tool_result") => {
                                        reject_fields(block, &["type", "tool_use_id", "content", "is_error"], "Messages tool result")?;
                                        let id = required_string(block.get("tool_use_id"), "tool_use_id")?;
                                        let output = parse_tool_output(block.get("content").context("A tool result requires content")?)?;
                                        let is_error = block.get("is_error").map(|value| value.as_bool().context("is_error must be a boolean")).transpose()?.unwrap_or(false);
                                        tool_results.push(ParsedToolResult { id, output, is_error });
                                    }
                                    Some(kind) => bail!("Unsupported Grok Messages user block {kind}"),
                                    None => bail!("Messages content blocks require a string type"),
                                }
                            }
                        }
                    }
                    "assistant" => {
                        let content = message.get("content").context("An assistant message requires content")?;
                        let batch_start = tool_calls.len();
                        if let Some(value) = content.as_str() {
                            assistant_text.push(value.to_owned());
                        } else {
                            for block in content.as_array().context("Messages assistant content must be text or an array of blocks")? {
                                match block.get("type").and_then(Value::as_str) {
                                    Some("text") => {
                                        reject_fields(block, &["type", "text"], "Messages assistant text block")?;
                                        assistant_text.push(required_text(block.get("text"), "Messages assistant text")?);
                                    }
                                    Some("tool_use") => {
                                        reject_fields(block, &["type", "id", "name", "input"], "Messages tool use")?;
                                        let name = required_string(block.get("name"), "Messages tool name")?;
                                        let arguments = block.get("input").context("Messages tool use requires input")?.clone();
                                        tool_calls.push(parse_call(block.get("id"), name, arguments)?);
                                    }
                                    Some(kind) => bail!("Unsupported Grok Messages assistant block {kind}"),
                                    None => bail!("Messages assistant blocks require a string type"),
                                }
                            }
                        }
                        let batch_size = tool_calls.len() - batch_start;
                        if batch_size > 0 {
                            tool_call_batch_sizes.push(batch_size);
                        }
                    }
                    _ => bail!("Grok ACP supports user and assistant Messages history only"),
                }
            }
        }
    }

    ensure!(tool_calls.len() <= 64 && tool_results.len() <= 64, "A Grok tool exchange supports at most 64 calls");
    validate_client_calls(&tool_calls, &tool_call_batch_sizes, &tools, tool_choice, allow_parallel)?;
    let bytes = text.iter().chain(assistant_text.iter()).fold(0usize, |sum, part| sum.saturating_add(part.len()));
    ensure!(bytes <= MAX_INPUT_BYTES, "Grok input exceeded the 64 KiB text limit");
    let result_bytes = tool_results.iter().fold(0usize, |sum, result| sum.saturating_add(result.output.len()));
    ensure!(result_bytes <= 32 * 1024, "Grok tool results exceeded the 32 KiB request limit");
    ensure!(text.iter().any(|part| !part.trim().is_empty()) || !tool_calls.is_empty() || !tool_results.is_empty(),
        "Grok ACP requires non-empty user text or a client tool result");
    Ok(TextTurn { text, max_tokens, tools, tool_choice, allow_parallel, tool_calls, tool_call_batch_sizes, tool_results, assistant_text })
}

fn reject_fields(value: &Value, allowed: &[&str], label: &str) -> Result<()> {
    let object = value.as_object().context("Protocol items must be objects")?;
    if let Some(field) = object.keys().find(|field| !allowed.contains(&field.as_str())) {
        bail!("Unsupported {label} field {field}");
    }
    Ok(())
}

fn required_string(value: Option<&Value>, label: &str) -> Result<String> {
    let value = value.and_then(Value::as_str).context(format!("{label} must be a string"))?;
    ensure!(!value.trim().is_empty() && value.len() <= 256, "{label} must be a bounded non-empty string");
    Ok(value.to_owned())
}

fn required_text(value: Option<&Value>, label: &str) -> Result<String> {
    let value = value.and_then(Value::as_str).context(format!("{label} must be a string"))?;
    ensure!(!value.trim().is_empty(), "{label} must be non-empty");
    Ok(value.to_owned())
}

fn optional_text_value(value: Option<&Value>, label: &str) -> Result<Vec<String>> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => text_value(value).with_context(|| format!("Unsupported {label}")),
    }
}

fn responses_assistant_text(value: &Value) -> Result<Vec<String>> {
    let blocks = value.as_array().context("Responses assistant output content must be an array")?;
    let mut result = Vec::new();
    for block in blocks {
        reject_fields(block, &["type", "text", "annotations"], "Responses assistant content")?;
        ensure!(block.get("type").and_then(Value::as_str) == Some("output_text"), "Only Responses output_text history is supported");
        if let Some(annotations) = block.get("annotations") {
            ensure!(annotations.as_array().is_some_and(Vec::is_empty), "Responses text annotations are unsupported");
        }
        result.push(required_text(block.get("text"), "Responses output_text")?);
    }
    Ok(result)
}

fn parse_tool_output(value: &Value) -> Result<String> {
    let output = match value {
        Value::String(value) => value.clone(),
        Value::Array(blocks) => {
            let mut output = String::new();
            for block in blocks {
                reject_fields(block, &["type", "text"], "tool-result text block")?;
                ensure!(block.get("type").and_then(Value::as_str) == Some("text"), "Grok ACP accepts text tool results only");
                output.push_str(block.get("text").and_then(Value::as_str).context("Tool-result blocks require string text")?);
            }
            output
        }
        _ => bail!("Grok ACP accepts text tool results only"),
    };
    ensure!(output.len() <= 32 * 1024, "A Grok tool result exceeds the 32 KiB request limit");
    Ok(output)
}

fn parse_call(id: Option<&Value>, name: String, arguments: Value) -> Result<ParsedToolCall> {
    let id = required_string(id, "tool call ID")?;
    ensure!(arguments.is_object(), "Function arguments must be a JSON object");
    ensure!(arguments.to_string().len() <= 32 * 1024, "Function arguments exceed the 32 KiB request limit");
    Ok(ParsedToolCall { id, name, arguments })
}

fn validate_client_calls(
    calls: &[ParsedToolCall],
    batch_sizes: &[usize],
    tools: &[FunctionTool],
    choice: ToolChoice,
    allow_parallel: bool,
) -> Result<()> {
    ensure!(calls.is_empty() || choice == ToolChoice::Auto, "Tool calls are incompatible with tool_choice=none");
    ensure!(calls.is_empty() || !tools.is_empty(), "The request contains tool-call history without client function schemas");
    ensure!(
        batch_sizes.iter().all(|size| *size > 0) && batch_sizes.iter().sum::<usize>() == calls.len(),
        "Grok tool-call history has invalid batch boundaries"
    );
    ensure!(allow_parallel || batch_sizes.iter().all(|size| *size <= 1),
        "The request contains a parallel call batch while parallel tool calls are disabled");
    for call in calls {
        let tool = tools.iter().find(|tool| tool.name == call.name)
            .with_context(|| format!("Tool call names undeclared client function `{}`", call.name))?;
        validate_schema_value(&call.arguments, &tool.parameters, 0)
            .with_context(|| format!("Arguments for client function `{}` do not match its schema", call.name))?;
    }
    Ok(())
}

fn parse_function_tools(protocol: Protocol, body: &Value) -> Result<Vec<FunctionTool>> {
    let tools = match body.get("tools") {
        None => return Ok(Vec::new()),
        Some(value) => value.as_array().context("tools must be an array")?,
    };
    ensure!(tools.len() <= 64, "Grok supports at most 64 client function tools");
    let mut parsed = Vec::with_capacity(tools.len());
    let mut names = HashSet::new();
    let mut bytes = 0usize;
    for tool in tools {
        let (name, description, parameters) = match protocol {
            Protocol::Chat => {
                reject_fields(tool, &["type", "function"], "Chat tool")?;
                ensure!(tool.get("type").and_then(Value::as_str) == Some("function"), "Only client function tools are supported");
                let function = tool.get("function").context("A Chat function tool requires function")?;
                reject_fields(function, &["name", "description", "parameters", "strict"], "Chat function tool")?;
                if let Some(strict) = function.get("strict") {
                    ensure!(strict.as_bool() == Some(false), "Grok ACP cannot guarantee strict function schemas");
                }
                (
                    required_string(function.get("name"), "Chat tool name")?,
                    optional_description(function.get("description"))?,
                    function.get("parameters").context("A Chat function tool requires parameters")?.clone(),
                )
            }
            Protocol::Responses => {
                reject_fields(tool, &["type", "name", "description", "parameters", "strict"], "Responses tool")?;
                ensure!(tool.get("type").and_then(Value::as_str) == Some("function"), "Only Responses function tools are supported");
                if let Some(strict) = tool.get("strict") {
                    ensure!(strict.as_bool() == Some(false), "Grok ACP cannot guarantee strict function schemas");
                }
                (
                    required_string(tool.get("name"), "Responses tool name")?,
                    optional_description(tool.get("description"))?,
                    tool.get("parameters").context("A Responses function tool requires parameters")?.clone(),
                )
            }
            Protocol::Messages => {
                reject_fields(tool, &["name", "description", "input_schema"], "Messages tool")?;
                (
                    required_string(tool.get("name"), "Messages tool name")?,
                    optional_description(tool.get("description"))?,
                    tool.get("input_schema").context("A Messages tool requires input_schema")?.clone(),
                )
            }
        };
        ensure!(name.len() <= 64 && name.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')),
            "Tool names must use ASCII letters, digits, underscore, or hyphen and be at most 64 bytes");
        ensure!(names.insert(name.clone()), "Grok function tool names must be unique");
        ensure!(parameters.is_object() && parameters.get("type").and_then(Value::as_str) == Some("object"),
            "Function parameters must use an object JSON Schema");
        validate_schema_definition(&parameters, 0)?;
        let parsed_tool = FunctionTool { name, description, parameters };
        bytes = bytes.saturating_add(parsed_tool.name.len())
            .saturating_add(parsed_tool.description.len())
            .saturating_add(parsed_tool.parameters.to_string().len());
        parsed.push(parsed_tool);
    }
    ensure!(bytes <= 32 * 1024, "Grok function schemas exceed the 32 KiB request limit");
    Ok(parsed)
}

fn optional_description(value: Option<&Value>) -> Result<String> {
    match value {
        None => Ok(String::new()),
        Some(value) => {
            let description = value.as_str().context("Tool descriptions must be strings")?;
            ensure!(description.len() <= 4 * 1024, "Tool descriptions exceed the 4 KiB limit");
            Ok(description.to_owned())
        }
    }
}

#[cfg(test)]
impl FunctionTool {
    fn to_value(&self) -> Value {
        json!({"name":self.name,"description":self.description,"parameters":self.parameters})
    }
}

fn parse_tool_choice(protocol: Protocol, body: &Value) -> Result<(ToolChoice, bool)> {
    match protocol {
        Protocol::Messages => {
            let Some(choice) = body.get("tool_choice") else { return Ok((ToolChoice::Auto, true)); };
            reject_fields(choice, &["type", "disable_parallel_tool_use"], "Messages tool_choice")?;
            let kind = choice.get("type").and_then(Value::as_str).context("Messages tool_choice requires string type")?;
            ensure!(kind == "auto", "Grok ACP cannot enforce Messages tool_choice {kind}");
            let disabled = choice.get("disable_parallel_tool_use")
                .map(|value| value.as_bool().context("disable_parallel_tool_use must be a boolean"))
                .transpose()?.unwrap_or(false);
            Ok((ToolChoice::Auto, !disabled))
        }
        Protocol::Chat | Protocol::Responses => {
            let requested = match body.get("tool_choice") {
                None => "auto",
                Some(Value::String(value)) => value.as_str(),
                Some(_) => bail!("Grok ACP supports string tool_choice values only"),
            };
            let choice = match requested {
                "auto" => ToolChoice::Auto,
                "none" => ToolChoice::None,
                other => bail!("Grok ACP cannot enforce tool_choice {other}"),
            };
            let parallel = body.get("parallel_tool_calls")
                .map(|value| value.as_bool().context("parallel_tool_calls must be a boolean"))
                .transpose()?.unwrap_or(true);
            Ok((choice, parallel))
        }
    }
}

fn validate_schema_definition(schema: &Value, depth: usize) -> Result<()> {
    ensure!(depth <= 8, "Function schemas may be nested at most 8 levels");
    let object = schema.as_object().context("Function schema nodes must be objects")?;
    let kind = object.get("type").and_then(Value::as_str).context("Function schema nodes require a string type")?;
    let allowed: &[&str] = match kind {
        "object" => &["type", "properties", "required", "additionalProperties", "enum", "title", "description"],
        "array" => &["type", "items", "enum", "title", "description"],
        "string" | "number" | "integer" | "boolean" | "null" => &["type", "enum", "title", "description"],
        other => bail!("Grok ACP cannot enforce JSON Schema type {other}"),
    };
    reject_fields(schema, allowed, "function schema")?;
    if let Some(values) = object.get("enum") {
        ensure!(values.as_array().is_some_and(|values| !values.is_empty()), "Function-schema enum must be a non-empty array");
    }
    match kind {
        "object" => {
            ensure!(object.get("additionalProperties").and_then(Value::as_bool) == Some(false),
                "Grok ACP requires additionalProperties=false for every function object schema");
            if let Some(properties) = object.get("properties") {
                for (name, property) in properties.as_object().context("Function-schema properties must be an object")? {
                    ensure!(!name.trim().is_empty(), "Function-schema property names cannot be empty");
                    validate_schema_definition(property, depth + 1)?;
                }
            }
            if let Some(required) = object.get("required") {
                let required = required.as_array().context("Function-schema required must be an array")?;
                let mut names = HashSet::new();
                for name in required {
                    let name = name.as_str().context("Function-schema required entries must be strings")?;
                    ensure!(names.insert(name), "Function-schema required contains a duplicate property");
                    ensure!(object.get("properties").and_then(Value::as_object).is_some_and(|properties| properties.contains_key(name)),
                        "Function-schema required names must reference declared properties");
                }
            }
        }
        "array" => validate_schema_definition(object.get("items").context("Array schemas require an items schema")?, depth + 1)?,
        _ => {}
    }
    if let Some(title) = object.get("title") {
        ensure!(title.as_str().is_some_and(|title| title.len() <= 128), "Function schema title must be a short string");
    }
    if let Some(description) = object.get("description") {
        ensure!(description.as_str().is_some_and(|description| description.len() <= 1024), "Function schema description must be a bounded string");
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        let type_matches = |value: &Value| match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.as_f64().is_some(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => false,
        };
        ensure!(values.iter().all(type_matches), "Function-schema enum values must match their declared type");
    }
    Ok(())
}

fn validate_schema_value(value: &Value, schema: &Value, depth: usize) -> Result<()> {
    ensure!(depth <= 8, "Function arguments exceed the supported schema depth");
    let schema = schema.as_object().context("Function schema nodes must be objects")?;
    let kind = schema.get("type").and_then(Value::as_str).context("Function schema nodes require a string type")?;
    let matches = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.as_f64().is_some(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    ensure!(matches, "Grok ACP tool arguments do not match declared type {kind}");
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        ensure!(values.contains(value), "Grok ACP tool arguments do not match the declared enum");
    }
    match kind {
        "object" => {
            let object = value.as_object().expect("type checked above");
            let properties = schema.get("properties").and_then(Value::as_object);
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    ensure!(object.contains_key(name), "Grok ACP tool arguments omit required property {name}");
                }
            }
            for (name, property) in object {
                match properties.and_then(|properties| properties.get(name)) {
                    Some(property_schema) => validate_schema_value(property, property_schema, depth + 1)?,
                    None => ensure!(schema.get("additionalProperties").and_then(Value::as_bool).unwrap_or(true),
                        "Grok ACP tool arguments contain undeclared property {name}"),
                }
            }
        }
        "array" => {
            let items = schema.get("items").context("Array schemas require an items schema")?;
            for item in value.as_array().expect("type checked above") {
                validate_schema_value(item, items, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
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
    generation: u64,
) -> Result<()> {
    let turn = parse_text_turn(protocol, body, model_id)?;
    let prefix = format!("call_ajv1_grok_{generation}_");
    for id in turn.tool_calls.iter().map(|call| &call.id).chain(turn.tool_results.iter().map(|result| &result.id)) {
        ensure!(id.starts_with(&prefix), "Tool-call history or result belongs to a stale Grok connection generation");
    }
    Ok(())
}

/// Bind every tool result to the exact prior Grok handoff. A partial follow-up consumes the
/// pending batch before it is refused, so a client cannot accidentally execute it twice.
#[cfg(test)]
fn resolve_tool_followup(
    adapter: &GrokSubscriptionAdapter,
    provider_id: &str,
    generation: u64,
    protocol: Protocol,
    model_id: &str,
    turn: &TextTurn,
) -> Result<Option<PendingClientToolTurn>> {
    if turn.tool_calls.is_empty() && turn.tool_results.is_empty() {
        return Ok(None);
    }

    let ids: HashSet<String> = turn.tool_calls.iter().map(|call| call.id.clone())
        .chain(turn.tool_results.iter().map(|result| result.id.clone())).collect();
    let mut pending = adapter.pending_tool_turns.lock().unwrap();
    let mut batches: HashMap<Vec<String>, PendingClientToolTurn> = HashMap::new();
    for id in &ids {
        if let Some(candidate) = pending.get(&(provider_id.to_owned(), generation, id.clone())) {
            let mut batch_ids: Vec<_> = candidate.calls.iter().map(|call| call.call.id.clone()).collect();
            batch_ids.sort();
            batches.entry(batch_ids).or_insert_with(|| candidate.clone());
        }
    }
    // If a malformed request refers to more than one outstanding batch, revoke every referenced
    // batch. The client may already have run some of those tools; none may be replayed.
    let matched_batches: Vec<_> = batches.into_iter().collect();
    for (batch_ids, _) in &matched_batches {
        for id in batch_ids {
            pending.remove(&(provider_id.to_owned(), generation, id.clone()));
        }
    }
    drop(pending);

    ensure!(matched_batches.len() <= 1, "A tool follow-up cannot combine separate pending Grok tool batches");
    let active = matched_batches.into_iter().next().map(|(_, turn)| turn);
    let call_by_id: std::collections::HashMap<_, _> = turn.tool_calls.iter().map(|call| (call.id.as_str(), call)).collect();
    let result_by_id: std::collections::HashMap<_, _> = turn.tool_results.iter().map(|result| (result.id.as_str(), result)).collect();
    ensure!(call_by_id.len() == turn.tool_calls.len(), "Grok tool-call history contains duplicate IDs");
    ensure!(result_by_id.len() == turn.tool_results.len(), "Grok tool results contain duplicate IDs");
    let call_ids: HashSet<_> = call_by_id.keys().copied().collect();
    let result_ids: HashSet<_> = result_by_id.keys().copied().collect();
    ensure!(call_ids == result_ids, "Every Grok tool call in a follow-up must have exactly one client result");

    let mut completed = adapter.completed_tool_exchanges.lock().unwrap();
    let active_ids: HashSet<String> = active.as_ref().into_iter().flat_map(|active| active.calls.iter())
        .map(|call| call.call.id.clone()).collect();

    if let Some(active) = &active {
        ensure!(active.protocol == protocol, "A Grok tool result was submitted under a different protocol");
        ensure!(active.model_id == model_id, "A Grok tool result was submitted under a different model");
        let expected_tools: Vec<_> = active.tools.clone();
        let actual_tools: Vec<_> = turn.tools.iter().map(FunctionTool::to_value).collect();
        ensure!(expected_tools == actual_tools, "The client tool schemas changed during a Grok tool exchange");
        ensure!(active.allow_parallel == turn.allow_parallel, "The parallel tool policy changed during a Grok tool exchange");
        ensure!(active.user_text == turn.text, "The user conversation changed before the Grok tool results were returned");
        ensure!(active.assistant_history == turn.assistant_text, "The assistant history changed before the Grok tool results were returned");
        for expected in &active.calls {
            let supplied = call_by_id.get(expected.call.id.as_str()).context("A Grok tool follow-up omitted part of the pending call batch")?;
            ensure!(supplied.name == expected.call.name && supplied.arguments == expected.arguments,
                "The client changed a pending Grok tool call");
            ensure!(result_by_id.contains_key(expected.call.id.as_str()), "A Grok tool follow-up returned only part of its pending result batch");
        }
    }

    for call in &turn.tool_calls {
        if active_ids.contains(&call.id) {
            let expected = active.as_ref().unwrap().calls.iter().find(|expected| expected.call.id == call.id).unwrap();
            ensure!(expected.call.name == call.name && expected.arguments == call.arguments, "The client changed a pending Grok tool call");
        } else {
            let previous = completed.get(&(provider_id.to_owned(), generation, call.id.clone()))
                .context("Grok tool history has no matching completed or pending client call")?;
            ensure!(previous.name == call.name && previous.arguments == call.arguments, "Grok tool history changed a completed client call");
        }
    }
    for result in &turn.tool_results {
        if active_ids.contains(&result.id) {
            continue;
        }
        let previous = completed.get(&(provider_id.to_owned(), generation, result.id.clone()))
            .context("Grok tool result has no matching completed or pending client call")?;
        ensure!(previous.output == result.output && previous.is_error == result.is_error,
            "Grok tool history changed a completed client result");
    }

    if let Some(active) = active {
        let existing = completed.keys().filter(|(provider, current, _)| provider == provider_id && *current == generation).count();
        ensure!(existing.saturating_add(active.calls.len()) <= 512, "Grok client tool history exceeded its per-generation limit");
        for call in &active.calls {
            let result = result_by_id.get(call.call.id.as_str()).expect("complete batch checked above");
            completed.insert((provider_id.to_owned(), generation, call.call.id.clone()), CompletedClientToolExchange {
                name: call.call.name.clone(),
                arguments: call.arguments.clone(),
                output: result.output.clone(),
                is_error: result.is_error,
            });
        }
        Ok(Some(active))
    } else {
        ensure!(!turn.tool_calls.is_empty(), "Grok tool result history omitted its client tool call");
        Ok(None)
    }
}

#[cfg(test)]
fn acp_prompt(turn: &TextTurn) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    for text in &turn.text {
        blocks.push(json!({"type":"text","text":text}));
    }
    for text in &turn.assistant_text {
        blocks.push(json!({"type":"text","text":format!("Prior assistant response:\n{text}")}));
    }
    for call in &turn.tool_calls {
        blocks.push(json!({"type":"text","text":format!("Prior client tool call {} ({}): {}", call.id, call.name, call.arguments)}));
    }
    for result in &turn.tool_results {
        let state = if result.is_error { "failed" } else { "completed" };
        blocks.push(json!({"type":"text","text":format!("Client tool result ({state}) for {}:\n{}", result.id, result.output)}));
    }
    if !turn.tools.is_empty() && turn.tool_choice == ToolChoice::Auto {
        let descriptions = turn.tools.iter().map(FunctionTool::to_value).collect::<Vec<_>>();
        blocks.push(json!({"type":"text","text":format!(
            "Client-owned function tools are available; the API client performs every tool action. Do not use helper tools. If a function is needed, report a pending ACP tool_call with name and rawInput matching one of these schemas: {}",
            serde_json::to_string(&descriptions)?
        )}));
    }
    let byte_count = blocks.iter().filter_map(|block| block.get("text").and_then(Value::as_str))
        .fold(0usize, |sum, text| sum.saturating_add(text.len()));
    ensure!(byte_count <= MAX_INPUT_BYTES, "Grok conversation and client tool context exceeded the 64 KiB prompt limit");
    Ok(blocks)
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
    validate_generation_request(request.protocol, &request.body, request.model_id, request.generation)?;
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
    resolve_tool_followup(
        adapter,
        request.provider_id,
        generation,
        request.protocol,
        request.model_id,
        &turn,
    )?;
    let prompt_content = acp_prompt(&turn)?;

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
    let protocol = request.protocol;
    let tools = turn.tools.clone();
    let tool_choice = turn.tool_choice;
    let allow_parallel = turn.allow_parallel;
    let user_text = turn.text.clone();
    let previous_assistant_history = turn.assistant_text.clone();
    let tool_schemas: Vec<_> = turn.tools.iter().map(FunctionTool::to_value).collect();
    let pending_tool_turns = adapter.pending_tool_turns.clone();
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
            tools,
            tool_choice,
            allow_parallel,
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
            PromptOutcome::ToolCalls { calls, assistant_text } => {
                let client_calls: Vec<_> = calls.iter().map(|call| crate::codex_helper::ClientToolCall {
                    id: format!("call_ajv1_grok_{generation}_{}", uuid::Uuid::new_v4().simple()),
                    name: call.name.clone(),
                    arguments: call.arguments.to_string(),
                }).collect();
                let mut assistant_history = previous_assistant_history;
                if !assistant_text.is_empty() {
                    assistant_history.push(assistant_text);
                }
                let pending = PendingClientToolTurn {
                    protocol,
                    model_id,
                    tools: tool_schemas,
                    allow_parallel,
                    user_text,
                    assistant_history,
                    calls: calls.into_iter().zip(client_calls.iter().cloned()).map(|(acp, call)| PendingClientToolCall {
                        call,
                        arguments: acp.arguments,
                    }).collect(),
                };
                let call_ids: Vec<_> = client_calls.iter().map(|call| call.id.clone()).collect();
                let mut pending_turns = pending_tool_turns.lock().unwrap();
                let held = pending_turns.keys().filter(|(provider, current, _)| provider == &provider_id && *current == generation).count();
                if held.saturating_add(call_ids.len()) > 512 {
                    drop(pending_turns);
                    let _ = tx.send(GenerationEvent::Failed { message: "Grok client tool handoffs exceeded the per-generation limit".into() });
                    return;
                }
                for call_id in &call_ids {
                    pending_turns.insert((provider_id.clone(), generation, call_id.clone()), pending.clone());
                }
                drop(pending_turns);
                if tx.send(GenerationEvent::ToolCalls { calls: client_calls }).is_err() {
                    let mut pending_turns = pending_tool_turns.lock().unwrap();
                    for call_id in &call_ids {
                        pending_turns.remove(&(provider_id.clone(), generation, call_id.clone()));
                    }
                }
            }
            PromptOutcome::Failed(message) => {
                let _ = tx.send(GenerationEvent::Failed { message });
            }
        }
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
    ToolCalls { calls: Vec<AcpClientToolCall>, assistant_text: String },
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
    tools: Vec<FunctionTool>,
    tool_choice: ToolChoice,
    allow_parallel: bool,
    deadline: tokio::time::Instant,
) -> PromptOutcome {
    let mut output_bytes = 0usize;
    let mut assistant_text = String::new();
    let mut client_calls = Vec::new();
    let mut call_ids = HashSet::new();
    let mut cancelling_for_client_tools = false;
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
                Some("end_turn") if client_calls.is_empty() => return PromptOutcome::Finished,
                Some("end_turn") => return PromptOutcome::Failed("Grok ACP completed while a client tool call was still pending".into()),
                Some("cancelled") if cancelling_for_client_tools && !client_calls.is_empty() => {
                    return PromptOutcome::ToolCalls { calls: client_calls, assistant_text };
                }
                Some("cancelled") => return PromptOutcome::Cancelled,
                Some("max_tokens") if client_calls.is_empty() => return PromptOutcome::MaxTokens,
                Some("max_tokens") => return PromptOutcome::Failed("Grok ACP stopped before cancelling its pending client tool call".into()),
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
                assistant_text.push_str(text);
                if !text.is_empty() && tx.send(GenerationEvent::Chunk(text.to_owned())).is_err() {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Cancelled;
                }
            }
            Some("tool_call") => {
                if pre_dispatch_check().is_err() {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Cancelled;
                }
                let parsed = parse_acp_client_tool_call(update, &tools, tool_choice, allow_parallel, &call_ids);
                let call = match parsed {
                    Ok(call) => call,
                    Err(_) => {
                        notify_session_cancel(rpc, session_id).await;
                        return PromptOutcome::Failed("Grok ACP requested a tool that the client did not declare or that its schema cannot safely constrain".into());
                    }
                };
                if client_calls.len() >= 64 {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Failed("Grok ACP exceeded the 64-call client tool limit".into());
                }
                if !call_ids.insert(call.id.clone()) {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Failed("Grok ACP repeated a client tool call ID".into());
                }
                client_calls.push(call);
                if !cancelling_for_client_tools {
                    cancelling_for_client_tools = true;
                    if !notify_session_cancel(rpc, session_id).await {
                        return PromptOutcome::Failed("Grok ACP could not be stopped before client tool handoff".into());
                    }
                }
            }
            Some("tool_call_update") => {
                if validate_acp_client_tool_update(update, &client_calls, cancelling_for_client_tools).is_err() {
                    notify_session_cancel(rpc, session_id).await;
                    return PromptOutcome::Failed("Grok ACP advanced or changed a tool operation before client handoff".into());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
fn parse_acp_client_tool_call(
    update: &Value,
    tools: &[FunctionTool],
    choice: ToolChoice,
    allow_parallel: bool,
    seen_ids: &HashSet<String>,
) -> Result<AcpClientToolCall> {
    ensure!(choice == ToolChoice::Auto && !tools.is_empty(), "Client tools are disabled for this request");
    ensure!(allow_parallel || seen_ids.is_empty(), "Parallel client tools are disabled for this request");
    ensure_no_acp_tool_output(update)?;
    let status = update.get("status").and_then(Value::as_str).unwrap_or("pending");
    ensure!(status == "pending", "ACP tool calls must be pending before handoff");
    let id = required_string(update.get("toolCallId"), "ACP toolCallId")?;
    ensure!(!seen_ids.contains(&id), "ACP repeated a toolCallId");
    let name = required_string(update.get("name"), "ACP client tool name")?;
    let tool = tools.iter().find(|tool| tool.name == name)
        .with_context(|| format!("ACP named undeclared client tool {name}"))?;
    let arguments = update.get("rawInput").context("ACP client tool calls require rawInput")?.clone();
    ensure!(arguments.is_object(), "ACP client tool arguments must be an object");
    validate_schema_value(&arguments, &tool.parameters, 0)?;
    ensure!(arguments.to_string().len() <= 32 * 1024, "ACP client tool arguments exceed the 32 KiB limit");
    Ok(AcpClientToolCall { id, name, arguments })
}

#[cfg(test)]
fn ensure_no_acp_tool_output(update: &Value) -> Result<()> {
    ensure!(update.get("rawOutput").is_none_or(Value::is_null), "ACP reported helper-side tool output");
    ensure!(update.get("content").is_none_or(|value| value.is_null() || value.as_array().is_some_and(Vec::is_empty)),
        "ACP reported helper-side tool output");
    Ok(())
}

#[cfg(test)]
fn validate_acp_client_tool_update(
    update: &Value,
    calls: &[AcpClientToolCall],
    cancelling: bool,
) -> Result<()> {
    let id = update.get("toolCallId").and_then(Value::as_str)
        .context("ACP tool updates require a known toolCallId")?;
    let original = calls.iter().find(|call| call.id == id)
        .context("ACP updated an unknown tool call")?;
    let status = update.get("status").and_then(Value::as_str);
    ensure!(status.is_none() || status == Some("pending") || (cancelling && status == Some("cancelled")),
        "ACP changed a pending tool call's execution status");
    ensure!(update.get("name").is_none_or(|value| value.is_null() || value.as_str() == Some(original.name.as_str())),
        "ACP changed a pending tool call's function name");
    ensure!(update.get("rawInput").is_none_or(|value| value.is_null() || value == &original.arguments),
        "ACP changed a pending tool call's arguments");
    ensure_no_acp_tool_output(update)?;
    Ok(())
}

#[cfg(test)]
fn value_id(value: &Value) -> Option<u64> {
    value.get("id").and_then(Value::as_u64)
}

#[cfg(test)]
async fn notify_session_cancel(rpc: &mut RpcProcess, session_id: &str) -> bool {
    let deadline = tokio::time::Instant::now() + CANCEL_NOTIFY_TIMEOUT;
    rpc.notify_until("session/cancel", json!({"sessionId":session_id}), deadline).await.is_ok()
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
                config.grok_real_generation_grants.insert(
                    provider.id.clone(),
                    crate::config::SubscriptionRealGenerationGrant {
                        connection_instance_id: connection.connection_instance_id.clone(),
                        generation: connection.generation,
                        identity: connection.identity.clone().unwrap(),
                        max_calls: crate::subscription::GROK_REAL_GENERATION_MAX_CALLS,
                        used_calls: 0,
                        enabled: true,
                    },
                );
            })
            .unwrap();
        store
    }

    fn client_tool_schema(protocol: Protocol) -> Value {
        let parameters = json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query"],
            "additionalProperties":false
        });
        match protocol {
            Protocol::Chat => json!([{"type":"function","function":{"name":"lookup","description":"Look up a value","parameters":parameters}}]),
            Protocol::Responses => json!([{"type":"function","name":"lookup","description":"Look up a value","parameters":parameters}]),
            Protocol::Messages => json!([{"name":"lookup","description":"Look up a value","input_schema":parameters}]),
        }
    }

    fn client_tool_initial_request(protocol: Protocol, model: &str, streaming: bool) -> Value {
        let tools = client_tool_schema(protocol);
        match protocol {
            Protocol::Chat => json!({"model":model,"stream":streaming,"tools":tools,"parallel_tool_calls":true,"messages":[{"role":"user","content":"find x"}]}),
            Protocol::Responses => json!({"model":model,"stream":streaming,"tools":tools,"parallel_tool_calls":true,"input":"find x"}),
            Protocol::Messages => json!({"model":model,"stream":streaming,"max_tokens":64,"tools":tools,"messages":[{"role":"user","content":"find x"}]}),
        }
    }

    fn client_tool_followup_request(protocol: Protocol, model: &str, streaming: bool, ids: &[String], partial: bool) -> Value {
        let tools = client_tool_schema(protocol);
        let result_count = if partial { 1 } else { ids.len() };
        match protocol {
            Protocol::Chat => {
                let tool_calls: Vec<_> = ids.iter().take(result_count).enumerate().map(|(index, id)| json!({
                    "id":id,"type":"function","function":{"name":"lookup","arguments":json!({"query":if index == 0 {"x"} else {"y"}}).to_string()}
                })).collect();
                let results: Vec<_> = ids.iter().take(result_count).map(|id| json!({"role":"tool","tool_call_id":id,"content":"found"})).collect();
                let mut messages = vec![json!({"role":"user","content":"find x"}),json!({"role":"assistant","content":"checking","tool_calls":tool_calls})];
                messages.extend(results);
                json!({"model":model,"stream":streaming,"tools":tools,"parallel_tool_calls":true,"messages":messages})
            }
            Protocol::Responses => {
                let mut input = vec![
                    json!({"type":"message","role":"user","content":"find x"}),
                    json!({"type":"message","id":"msg_fixture","status":"completed","role":"assistant","content":[{"type":"output_text","text":"checking","annotations":[]}]}),
                ];
                for (index, id) in ids.iter().take(result_count).enumerate() {
                    input.push(json!({"type":"function_call","id":format!("fc_fixture_{index}"),"call_id":id,"name":"lookup","arguments":json!({"query":if index == 0 {"x"} else {"y"}}).to_string(),"status":"completed"}));
                }
                for id in ids.iter().take(result_count) {
                    input.push(json!({"type":"function_call_output","call_id":id,"output":"found"}));
                }
                json!({"model":model,"stream":streaming,"tools":tools,"parallel_tool_calls":true,"input":input})
            }
            Protocol::Messages => {
                let mut content = vec![json!({"type":"text","text":"checking"})];
                content.extend(ids.iter().take(result_count).enumerate().map(|(index, id)| json!({
                    "type":"tool_use","id":id,"name":"lookup","input":{"query":if index == 0 {"x"} else {"y"}}
                })));
                let results: Vec<_> = ids.iter().take(result_count).enumerate().map(|(index, id)| json!({
                    "type":"tool_result","tool_use_id":id,"content":"found","is_error":index == 1
                })).collect();
                json!({"model":model,"stream":streaming,"max_tokens":64,"tools":tools,"messages":[
                    {"role":"user","content":"find x"},{"role":"assistant","content":content},{"role":"user","content":results}
                ]})
            }
        }
    }

    fn fake_client_tool_acp(directory: &Path) -> PathBuf {
        install_fake_acp(directory, r##"#!/usr/bin/env python3
import json, sys
session = "client-tool-session"
prompt_id = None
for line in sys.stdin:
    req = json.loads(line); method = req.get("method"); ident = req.get("id")
    if method == "initialize": result = {"protocolVersion":1,"authMethods":[{"id":"cached_token"}]}
    elif method == "authenticate": result = {}
    elif method == "session/new": result = {"sessionId":session,"configOptions":[{"id":"max_tokens","type":"select","currentValue":"4096","options":[{"value":"64","name":"64"},{"value":"4096","name":"4096"}]}]}
    elif method == "session/set_config_option": result = {"configOptions":[{"id":"max_tokens","type":"select","currentValue":req["params"]["value"],"options":[{"value":"64","name":"64"},{"value":"4096","name":"4096"}]}]}
    elif method == "session/prompt":
        prompt_id = ident
        text = "\n".join(part.get("text", "") for part in req["params"]["prompt"])
        if "Client-owned function tools are available" in text and "Client tool result (" not in text:
            chunk = {"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"checking"}}
            sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":chunk}}) + "\n")
            for index, query in enumerate(["x", "y"]):
                update = {"sessionUpdate":"tool_call","toolCallId":"acp-call-"+str(index),"name":"lookup","title":"Lookup","kind":"fetch","status":"pending","rawInput":{"query":query}}
                sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":update}}) + "\n")
            sys.stdout.flush()
            continue
        update = {"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"resolved"}}
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":update}}) + "\n")
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":{"stopReason":"end_turn"}}) + "\n")
        sys.stdout.flush()
        break
    elif method == "session/cancel":
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":prompt_id,"result":{"stopReason":"cancelled"}}) + "\n")
        sys.stdout.flush()
        break
    else: result = {}
    if ident is not None:
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":result}) + "\n"); sys.stdout.flush()
"##)
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

    #[test]
    fn validates_client_function_schemas_and_result_pairs_for_all_protocols() {
        let call_id = "call_ajv1_grok_7_fixture";
        let schema = json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query"],
            "additionalProperties":false
        });
        let cases = [
            (
                Protocol::Chat,
                json!({
                    "model":"grok-test",
                    "tools":[{"type":"function","function":{"name":"lookup","description":"Look up a value","parameters":schema}}],
                    "messages":[{"role":"user","content":"look this up"}]
                }),
                json!({
                    "model":"grok-test",
                    "tools":[{"type":"function","function":{"name":"lookup","description":"Look up a value","parameters":schema}}],
                    "messages":[
                        {"role":"user","content":"look this up"},
                        {"role":"assistant","content":null,"tool_calls":[{"id":call_id,"type":"function","function":{"name":"lookup","arguments":json!({"query":"x"}).to_string()}}]},
                        {"role":"tool","tool_call_id":call_id,"content":"found it"}
                    ]
                }),
            ),
            (
                Protocol::Responses,
                json!({
                    "model":"grok-test",
                    "tools":[{"type":"function","name":"lookup","description":"Look up a value","parameters":schema}],
                    "input":"look this up"
                }),
                json!({
                    "model":"grok-test",
                    "tools":[{"type":"function","name":"lookup","description":"Look up a value","parameters":schema}],
                    "input":[
                        {"type":"message","role":"user","content":"look this up"},
                        {"type":"function_call","id":"fc_fixture","call_id":call_id,"name":"lookup","arguments":json!({"query":"x"}).to_string(),"status":"completed"},
                        {"type":"function_call_output","call_id":call_id,"output":"found it"}
                    ]
                }),
            ),
            (
                Protocol::Messages,
                json!({
                    "model":"grok-test","max_tokens":64,
                    "tools":[{"name":"lookup","description":"Look up a value","input_schema":schema}],
                    "messages":[{"role":"user","content":"look this up"}]
                }),
                json!({
                    "model":"grok-test","max_tokens":64,
                    "tools":[{"name":"lookup","description":"Look up a value","input_schema":schema}],
                    "messages":[
                        {"role":"user","content":"look this up"},
                        {"role":"assistant","content":[{"type":"tool_use","id":call_id,"name":"lookup","input":{"query":"x"}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":call_id,"content":"found it"}]}
                    ]
                }),
            ),
        ];
        for (protocol, initial, followup) in cases {
            assert!(
                validate_generation_request(protocol, &initial, "grok-test", 7).is_ok(),
                "initial {protocol:?} tool request should be accepted: {initial}"
            );
            assert!(
                validate_generation_request(protocol, &followup, "grok-test", 7).is_ok(),
                "follow-up {protocol:?} tool results should be accepted: {followup}"
            );
        }
    }

    #[test]
    fn rejects_unconstrained_tool_schemas_and_tool_choices() {
        let cases = [
            (
                Protocol::Chat,
                json!({"model":"grok-test","messages":[{"role":"user","content":"x"}],"tools":[{"type":"web_search"}]}),
            ),
            (
                Protocol::Chat,
                json!({"model":"grok-test","messages":[{"role":"user","content":"x"}],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"},"strict":true}}]}),
            ),
            (
                Protocol::Responses,
                json!({"model":"grok-test","input":"x","tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],"tool_choice":"required"}),
            ),
            (
                Protocol::Messages,
                json!({"model":"grok-test","max_tokens":64,"messages":[{"role":"user","content":"x"}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}],"tool_choice":{"type":"tool","name":"lookup"}}),
            ),
        ];
        for (protocol, body) in cases {
            assert!(
                validate_generation_request(protocol, &body, "grok-test", 7).is_err(),
                "unconstrained {protocol:?} request must be refused: {body}"
            );
        }
    }

    #[test]
    fn rejects_unmatched_calls_invalid_arguments_and_old_generation_ids() {
        let id = "call_ajv1_grok_7_fixture";
        let schema = json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query"],
            "additionalProperties":false
        });
        let tools = json!([{"type":"function","function":{"name":"lookup","parameters":schema}}]);
        let valid = json!({
            "model":"grok-test","tools":tools,
            "messages":[
                {"role":"user","content":"find x"},
                {"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":"lookup","arguments":"{\"query\":\"x\"}"}}]},
                {"role":"tool","tool_call_id":id,"content":"found"}
            ]
        });
        assert!(validate_generation_request(Protocol::Chat, &valid, "grok-test", 7).is_ok());
        assert!(validate_generation_request(Protocol::Chat, &valid, "grok-test", 8).is_err(), "old generation IDs must be rejected");

        let invalid_cases = [
            json!({"model":"grok-test","tools":tools,"messages":[
                {"role":"user","content":"find x"},
                {"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":"shell","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":id,"content":"done"}
            ]}),
            json!({"model":"grok-test","tools":tools,"messages":[
                {"role":"user","content":"find x"},
                {"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":"lookup","arguments":"{\"query\":1}"}}]},
                {"role":"tool","tool_call_id":id,"content":"done"}
            ]}),
            json!({"model":"grok-test","tools":tools,"tool_choice":"none","messages":[
                {"role":"user","content":"find x"},
                {"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":"lookup","arguments":"{\"query\":\"x\"}"}}]},
                {"role":"tool","tool_call_id":id,"content":"done"}
            ]}),
            json!({"model":"grok-test","tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{}}}}],"messages":[{"role":"user","content":"find x"}]}),
        ];
        for body in invalid_cases {
            assert!(validate_generation_request(Protocol::Chat, &body, "grok-test", 7).is_err(), "unconstrained call must be refused: {body}");
        }
    }

    #[test]
    fn original_257_byte_assistant_history_roundtrips_for_all_protocols() {
        let text = "a".repeat(257);
        let id = "call_ajv1_grok_7_fixture";
        let bodies = [
            (
                Protocol::Chat,
                json!({
                    "model":"grok-test",
                    "tools":client_tool_schema(Protocol::Chat),
                    "messages":[
                        {"role":"user","content":"find x"},
                        {"role":"assistant","content":text,"tool_calls":[{"id":id,"type":"function","function":{"name":"lookup","arguments":"{\"query\":\"x\"}"}}]},
                        {"role":"tool","tool_call_id":id,"content":"found"}
                    ]
                }),
            ),
            (
                Protocol::Responses,
                json!({
                    "model":"grok-test",
                    "tools":client_tool_schema(Protocol::Responses),
                    "input":[
                        {"type":"message","role":"user","content":"find x"},
                        {"type":"message","id":"msg_fixture","status":"completed","role":"assistant","content":[{"type":"output_text","text":text,"annotations":[]}]},
                        {"type":"function_call","id":"fc_fixture","call_id":id,"name":"lookup","arguments":"{\"query\":\"x\"}","status":"completed"},
                        {"type":"function_call_output","call_id":id,"output":"found"}
                    ]
                }),
            ),
            (
                Protocol::Messages,
                json!({
                    "model":"grok-test","max_tokens":64,
                    "tools":client_tool_schema(Protocol::Messages),
                    "messages":[
                        {"role":"user","content":"find x"},
                        {"role":"assistant","content":[{"type":"text","text":text},{"type":"tool_use","id":id,"name":"lookup","input":{"query":"x"}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"found"}]}
                    ]
                }),
            ),
        ];
        for (protocol, body) in bodies {
            let result = validate_generation_request(protocol, &body, "grok-test", 7);
            assert!(result.is_ok(), "257-byte assistant history must roundtrip for {protocol:?}: {result:?}");
        }
    }

    #[test]
    fn parallel_tool_policy_is_checked_per_call_batch_not_across_history() {
        let first = "call_ajv1_grok_7_first";
        let second = "call_ajv1_grok_7_second";
        let call = |id: &str, query: &str| json!({
            "role":"assistant","content":null,
            "tool_calls":[{"id":id,"type":"function","function":{"name":"lookup","arguments":json!({"query":query}).to_string()}}]
        });
        let result = |id: &str| json!({"role":"tool","tool_call_id":id,"content":"found"});
        let base = json!({
            "model":"grok-test","parallel_tool_calls":false,
            "tools":client_tool_schema(Protocol::Chat),
            "messages":[{"role":"user","content":"find x"},call(first,"x"),result(first),call(second,"y"),result(second)]
        });
        let parsed = validate_generation_request(Protocol::Chat, &base, "grok-test", 7);
        assert!(parsed.is_ok(), "single calls in successive turns are not parallel: {parsed:?}");

        let parallel = json!({
            "model":"grok-test","parallel_tool_calls":false,
            "tools":client_tool_schema(Protocol::Chat),
            "messages":[
                {"role":"user","content":"find x and y"},
                {"role":"assistant","content":null,"tool_calls":[
                    {"id":first,"type":"function","function":{"name":"lookup","arguments":"{\"query\":\"x\"}"}},
                    {"id":second,"type":"function","function":{"name":"lookup","arguments":"{\"query\":\"y\"}"}}
                ]},
                result(first),result(second)
            ]
        });
        assert!(validate_generation_request(Protocol::Chat, &parallel, "grok-test", 7).is_err(),
            "multiple calls in one batch must remain disabled");
    }

    #[test]
    fn acp_followup_prompt_preserves_call_id_name_arguments_and_result_association() {
        let id_x = "call_ajv1_grok_7_x";
        let id_y = "call_ajv1_grok_7_y";
        let history = |swapped: bool| {
            let (call_x_id, call_y_id) = if swapped { (id_y, id_x) } else { (id_x, id_y) };
            json!({
                "model":"grok-test","tools":client_tool_schema(Protocol::Chat),
                "messages":[
                    {"role":"user","content":"Compare x and y"},
                    {"role":"assistant","content":"checking","tool_calls":[
                        {"id":call_x_id,"type":"function","function":{"name":"lookup","arguments":"{\"query\":\"x\"}"}},
                        {"id":call_y_id,"type":"function","function":{"name":"lookup","arguments":"{\"query\":\"y\"}"}}
                    ]},
                    {"role":"tool","tool_call_id":id_x,"content":"value=10"},
                    {"role":"tool","tool_call_id":id_y,"content":"value=20"}
                ]
            })
        };
        let first = acp_prompt(&parse_text_turn(Protocol::Chat, &history(false), "grok-test").unwrap()).unwrap();
        let swapped = acp_prompt(&parse_text_turn(Protocol::Chat, &history(true), "grok-test").unwrap()).unwrap();
        assert_ne!(first, swapped, "opposite call-to-result associations must yield different ACP context");
        let prompt = first.iter().filter_map(|block| block.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n");
        assert!(prompt.contains(&format!("Prior client tool call {id_x} (lookup): {{\"query\":\"x\"}}")));
        assert!(prompt.contains(&format!("Prior client tool call {id_y} (lookup): {{\"query\":\"y\"}}")));
        assert!(prompt.contains(&format!("Client tool result (completed) for {id_x}:\nvalue=10")));
        assert!(prompt.contains(&format!("Client tool result (completed) for {id_y}:\nvalue=20")));
    }

    #[test]
    fn acp_builtin_or_already_running_tools_cannot_be_forwarded_to_the_api_client() {
        let request = json!({"tools":client_tool_schema(Protocol::Chat)});
        let tools = parse_function_tools(Protocol::Chat, &request).unwrap();
        let no_ids = HashSet::new();
        let helper_tool = json!({"toolCallId":"helper-shell-1","name":"shell","status":"pending","rawInput":{"query":"x"}});
        let already_running = json!({"toolCallId":"client-lookup-1","name":"lookup","status":"in_progress","rawInput":{"query":"x"}});
        assert!(parse_acp_client_tool_call(&helper_tool, &tools, ToolChoice::Auto, true, &no_ids).is_err());
        assert!(parse_acp_client_tool_call(&already_running, &tools, ToolChoice::Auto, true, &no_ids).is_err());
        let helper_raw_output = json!({"toolCallId":"client-lookup-1","name":"lookup","status":"pending","rawInput":{"query":"x"},"rawOutput":"already executed"});
        let helper_content = json!({"toolCallId":"client-lookup-1","name":"lookup","status":"pending","rawInput":{"query":"x"},"content":[{"type":"content","content":{"type":"text","text":"already executed"}}]});
        assert!(parse_acp_client_tool_call(&helper_raw_output, &tools, ToolChoice::Auto, true, &no_ids).is_err(),
            "an initial tool_call with rawOutput must not be handed to the API client");
        assert!(parse_acp_client_tool_call(&helper_content, &tools, ToolChoice::Auto, true, &no_ids).is_err(),
            "an initial tool_call with content output must not be handed to the API client");

        let pending = AcpClientToolCall { id:"client-lookup-1".into(), name:"lookup".into(), arguments:json!({"query":"x"}) };
        let unchanged = json!({"toolCallId":"client-lookup-1","status":"pending","rawInput":{"query":"x"}});
        let mutated = json!({"toolCallId":"client-lookup-1","status":"pending","rawInput":{"query":"other"}});
        let helper_output = json!({"toolCallId":"client-lookup-1","status":"cancelled","rawOutput":"already executed"});
        assert!(validate_acp_client_tool_update(&unchanged, std::slice::from_ref(&pending), true).is_ok());
        assert!(validate_acp_client_tool_update(&mutated, std::slice::from_ref(&pending), true).is_err());
        assert!(validate_acp_client_tool_update(&helper_output, std::slice::from_ref(&pending), true).is_err());
    }

    #[tokio::test]
    async fn acp_tool_calls_with_initial_helper_output_never_become_client_handoffs() {
        for output_marker in [
            json!({"rawOutput":"already executed"}),
            json!({"content":[{"type":"content","content":{"type":"text","text":"already executed"}}]}),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let output_literal = serde_json::to_string(&output_marker).unwrap();
            let script = r##"#!/usr/bin/env python3
import json, sys
session = "initial-output-session"
prompt_id = None
for line in sys.stdin:
    req = json.loads(line); method = req.get("method"); ident = req.get("id")
    if method == "initialize": result = {"protocolVersion":1,"authMethods":[{"id":"cached_token"}]}
    elif method == "authenticate": result = {}
    elif method == "session/new": result = {"sessionId":session}
    elif method == "session/prompt":
        prompt_id = ident
        update = {"sessionUpdate":"tool_call","toolCallId":"client-lookup-1","name":"lookup","status":"pending","rawInput":{"query":"x"}}
        update.update(OUTPUT_MARKER)
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":update}}) + "\n")
        sys.stdout.flush()
        continue
    elif method == "session/cancel":
        sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":prompt_id,"result":{"stopReason":"cancelled"}}) + "\n")
        sys.stdout.flush()
        break
    else: result = {}
    sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":ident,"result":result}) + "\n"); sys.stdout.flush()
"##.replace("OUTPUT_MARKER", &output_literal);
            let helper = install_fake_acp(temp.path(), &script);
            let adapter = GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper);
            let mut events = adapter.generate(GenerationRequest {
                provider_id: "grok-initial-helper-output",
                generation: 7,
                model_id: "grok-test",
                protocol: Protocol::Chat,
                body: client_tool_initial_request(Protocol::Chat, "grok-test", false),
                pre_dispatch_check: Arc::new(|| Ok(())),
            }).await.unwrap();
            let mut saw_tool_calls = false;
            let mut saw_failure = false;
            while let Some(event) = events.next().await {
                match event {
                    GenerationEvent::ToolCalls { .. } => saw_tool_calls = true,
                    GenerationEvent::Failed { .. } => saw_failure = true,
                    _ => {}
                }
            }
            assert!(saw_failure, "helper output must fail closed before handoff: {output_marker}");
            assert!(!saw_tool_calls, "a helper-executed operation must never reach the client: {output_marker}");
            assert!(adapter.pending_tool_turns.lock().unwrap().is_empty(),
                "a helper-executed operation must not be registered as pending: {output_marker}");
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
                assert_eq!(
                    store.read().grok_real_generation_grants["grok-fixture"].used_calls as usize,
                    calls,
                    "each accepted helper turn consumes one finite API dispatch slot"
                );
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
                assert_eq!(
                    store.read().grok_real_generation_grants["grok-fixture"].used_calls as usize,
                    calls,
                    "each accepted helper turn consumes one finite API dispatch slot"
                );
            }
        }

        let rejected = crate::proxy::forward_test_request(
            store,
            json!({
                "model":alias,"messages":[{"role":"user","content":"hello"}],"tools":[{"type":"web_search"}]
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

    fn grok_tool_call_ids(text: &str) -> Vec<String> {
        let prefix = "call_ajv1_grok_";
        let mut rest = text;
        let mut result = Vec::new();
        while let Some(index) = rest.find(prefix) {
            let candidate = &rest[index..];
            let id: String = candidate.chars().take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_').collect();
            if !result.contains(&id) {
                result.push(id);
            }
            rest = &candidate[prefix.len().min(candidate.len())..];
            if rest.is_empty() { break; }
        }
        result
    }

    #[tokio::test]
    async fn grok_client_tools_roundtrip_all_protocols_and_stream_modes() {
        let temp = tempfile::tempdir().unwrap();
        let helper = fake_client_tool_acp(temp.path());
        let adapter = Arc::new(GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper));
        let store = admitted_grok_store(temp.path(), adapter.clone());
        let alias = "autojev/model/grok-fixture-binding";
        let generation = store.read().subscriptions.get("grok-fixture").unwrap().generation;

        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            for streaming in [false, true] {
                let initial = crate::proxy::forward_test_request(
                    store.clone(), client_tool_initial_request(protocol, alias, streaming),
                    crate::subscription::protocol_key(protocol),
                ).await;
                assert_eq!(initial.status(), axum::http::StatusCode::OK, "{protocol:?}/{streaming}");
                let bytes = axum::body::to_bytes(initial.into_body(), 1024 * 1024).await.unwrap();
                let wire = String::from_utf8(bytes.to_vec()).unwrap();
                let ids = grok_tool_call_ids(&wire);
                assert_eq!(ids.len(), 2, "{protocol:?}/{streaming}: {wire}");
                match protocol {
                    Protocol::Chat => assert!(wire.contains("\"finish_reason\":\"tool_calls\""), "{wire}"),
                    Protocol::Responses if streaming => assert!(wire.contains("response.function_call_arguments.done") && wire.contains("response.completed"), "{wire}"),
                    Protocol::Responses => assert!(wire.contains("\"status\":\"completed\"") && wire.contains("\"type\":\"function_call\""), "{wire}"),
                    Protocol::Messages if streaming => assert!(wire.contains("\"stop_reason\":\"tool_use\"") && wire.contains("message_stop"), "{wire}"),
                    Protocol::Messages => assert!(wire.contains("\"stop_reason\":\"tool_use\"") && wire.contains("\"type\":\"tool_use\""), "{wire}"),
                }
                assert!(wire.find("checking").unwrap() < wire.find(&ids[0]).unwrap(), "assistant text must precede tool calls: {protocol:?}/{streaming}: {wire}");

                // A separate ordinary request for this account must leave the other client-owned
                // handoff intact while it runs and completes.
                if protocol == Protocol::Chat && !streaming {
                    let unrelated = crate::proxy::forward_test_request(
                        store.clone(),
                        json!({"model":alias,"messages":[{"role":"user","content":"independent"}]}),
                        crate::subscription::protocol_key(Protocol::Chat),
                    ).await;
                    let _ = axum::body::to_bytes(unrelated.into_body(), 1024 * 1024).await.unwrap();
                    let held = adapter.pending_tool_turns.lock().unwrap().keys()
                        .filter(|(provider, current, _)| provider == "grok-fixture" && *current == generation).count();
                    assert_eq!(held, 2, "an unrelated request consumed another request's pending tool batch");
                }

                let followup = crate::proxy::forward_test_request(
                    store.clone(),
                    client_tool_followup_request(protocol, alias, streaming, &ids, false),
                    crate::subscription::protocol_key(protocol),
                ).await;
                assert_eq!(followup.status(), axum::http::StatusCode::OK, "{protocol:?}/{streaming}");
                let bytes = axum::body::to_bytes(followup.into_body(), 1024 * 1024).await.unwrap();
                let wire = String::from_utf8(bytes.to_vec()).unwrap();
                assert!(wire.contains("resolved"), "{protocol:?}/{streaming}: {wire}");
                if streaming {
                    match protocol {
                        Protocol::Chat => assert!(wire.contains("data: [DONE]"), "{wire}"),
                        Protocol::Responses => assert!(wire.contains("event: response.completed"), "{wire}"),
                        Protocol::Messages => assert!(wire.contains("event: message_stop"), "{wire}"),
                    }
                } else {
                    let body: Value = serde_json::from_str(&wire).unwrap();
                    let text = match protocol {
                        Protocol::Chat => body.pointer("/choices/0/message/content").and_then(Value::as_str),
                        Protocol::Responses => body.pointer("/output/0/content/0/text").and_then(Value::as_str),
                        Protocol::Messages => body.pointer("/content/0/text").and_then(Value::as_str),
                    };
                    assert_eq!(text, Some("resolved"), "{protocol:?}: {body}");
                }
            }
        }
    }

    #[tokio::test]
    async fn grok_partial_client_tool_results_revoke_the_whole_batch_without_replay() {
        let temp = tempfile::tempdir().unwrap();
        let helper = fake_client_tool_acp(temp.path());
        let adapter = GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper);
        let provider_id = "grok-partial-tools";
        let mut events = adapter.generate(GenerationRequest {
            provider_id, generation: 5, model_id: "grok-test", protocol: Protocol::Chat,
            body: client_tool_initial_request(Protocol::Chat, "grok-test", false),
            pre_dispatch_check: Arc::new(|| Ok(())),
        }).await.unwrap();
        let mut calls = Vec::new();
        while let Some(event) = events.next().await {
            if let GenerationEvent::ToolCalls { calls: batch } = event { calls = batch; }
        }
        assert_eq!(calls.len(), 2);
        let ids: Vec<_> = calls.iter().map(|call| call.id.clone()).collect();
        assert_eq!(adapter.pending_tool_turns.lock().unwrap().len(), 2);

        let partial = client_tool_followup_request(Protocol::Chat, "grok-test", false, &ids, true);
        let result = adapter.generate(GenerationRequest {
            provider_id, generation: 5, model_id: "grok-test", protocol: Protocol::Chat,
            body: partial,
            pre_dispatch_check: Arc::new(|| Ok(())),
        }).await;
        assert!(result.is_err(), "a partial result batch must be refused");
        assert!(adapter.pending_tool_turns.lock().unwrap().is_empty(), "the remaining call must not be re-emitted");
        assert!(adapter.completed_tool_exchanges.lock().unwrap().is_empty(), "partial results must not be recorded as a completed batch");

        let replay = adapter.generate(GenerationRequest {
            provider_id, generation: 5, model_id: "grok-test", protocol: Protocol::Chat,
            body: client_tool_followup_request(Protocol::Chat, "grok-test", false, &ids, false),
            pre_dispatch_check: Arc::new(|| Ok(())),
        }).await;
        assert!(replay.is_err(), "a revoked partial batch must not be replayed");
    }

    #[tokio::test]
    async fn grok_generation_cancel_clears_pending_tools_and_rejects_old_ids() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), fake_client_tool_acp(temp.path()));
        let provider_id = "grok-cancel-tools";
        let mut events = adapter.generate(GenerationRequest {
            provider_id, generation: 12, model_id: "grok-test", protocol: Protocol::Chat,
            body: client_tool_initial_request(Protocol::Chat, "grok-test", false),
            pre_dispatch_check: Arc::new(|| Ok(())),
        }).await.unwrap();
        let mut calls = Vec::new();
        while let Some(event) = events.next().await {
            if let GenerationEvent::ToolCalls { calls: batch } = event { calls = batch; }
        }
        assert_eq!(calls.len(), 2);
        adapter.cancel_generation(provider_id, 12);
        assert!(adapter.pending_tool_turns.lock().unwrap().is_empty());
        assert!(adapter.completed_tool_exchanges.lock().unwrap().is_empty());

        let ids: Vec<_> = calls.iter().map(|call| call.id.clone()).collect();
        let stale = client_tool_followup_request(Protocol::Chat, "grok-test", false, &ids, false);
        assert!(validate_generation_request(Protocol::Chat, &stale, "grok-test", 13).is_err());
    }

    #[tokio::test]
    async fn grok_client_tool_handoffs_are_revoked_when_json_or_sse_bodies_are_abandoned() {
        let temp = tempfile::tempdir().unwrap();
        let helper = fake_client_tool_acp(temp.path());
        let adapter = Arc::new(GrokSubscriptionAdapter::with_test_helper(temp.path().to_path_buf(), helper));
        let store = admitted_grok_store(temp.path(), adapter.clone());
        let alias = "autojev/model/grok-fixture-binding";
        let generation = store.read().subscriptions.get("grok-fixture").unwrap().generation;

        for streaming in [false, true] {
            let response = crate::proxy::forward_test_request(
                store.clone(), client_tool_initial_request(Protocol::Chat, alias, streaming),
                crate::subscription::protocol_key(Protocol::Chat),
            ).await;
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            drop(response);
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let pending = adapter.pending_tool_turns.lock().unwrap().keys().any(|(provider, current, _)| {
                        provider == "grok-fixture" && *current == generation
                    });
                    let active = adapter.active_generations.lock().unwrap().keys().any(|(provider, current)| {
                        provider == "grok-fixture" && *current == generation
                    });
                    if !pending && !active { break; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("abandoned client tool response must cancel and revoke its handoff");
        }
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
