use super::*;
use std::collections::{HashSet, HashMap};
use sha2::{Digest, Sha256};

#[derive(Clone, Default, Debug)]
pub struct ToolMap {
    pub custom: HashSet<String>,
    pub namespaces: HashMap<String, (String, String)>,
}
fn namespace_alias(namespace: &str, name: &str, map: &mut ToolMap) -> String {
    let digest = Sha256::digest(format!("{namespace}\0{name}").as_bytes());
    let alias = format!("aj_{:x}", digest)[..63].to_owned();
    map.namespaces.insert(alias.clone(), (namespace.into(), name.into()));
    alias
}
fn flatten_namespaces(body: &Value, map: &mut ToolMap) -> Result<Value> {
    let mut result = body.clone();
    let mut tools = Vec::new();
    for tool in array(body, "tools") {
        if text(tool, "type") != "namespace" { tools.push(tool.clone()); continue; }
        let namespace = required(tool, "name")?;
        let children = tool["tools"].as_array().ok_or_else(|| anyhow::anyhow!("Namespace tools must be an array"))?;
        for child in children {
            if !matches!(text(child, "type"), "function" | "custom") { return Err(unsupported("nested namespace tool type")); }
            let name = required(child, "name")?;
            let mut child = child.clone();
            child["name"] = namespace_alias(&namespace, &name, map).into();
            child["description"] = format!("{namespace}.{name}: {}\n{}", text(tool, "description"), text(&child, "description")).into();
            tools.push(child);
        }
    }
    let mut names = HashSet::new();
    for tool in &tools { if !names.insert(text(tool, "name")) { bail!("Conflicting tool names after namespace conversion"); } }
    if body.get("tools").is_some() { result["tools"] = tools.into(); }
    if let Some(items) = result["input"].as_array_mut() {
        for item in items {
            if matches!(text(item, "type"), "function_call" | "custom_tool_call") && !text(item, "namespace").is_empty() {
                let namespace = required(item, "namespace")?;
                let name = required(item, "name")?;
                item["name"] = namespace_alias(&namespace, &name, map).into();
                item.as_object_mut().unwrap().remove("namespace");
            }
        }
    }
    if !text(&result["tool_choice"], "namespace").is_empty() {
        let namespace = required(&result["tool_choice"], "namespace")?;
        let name = required(&result["tool_choice"], "name")?;
        result["tool_choice"]["name"] = namespace_alias(&namespace, &name, map).into();
        result["tool_choice"].as_object_mut().unwrap().remove("namespace");
    }
    Ok(result)
}
#[derive(Clone, Debug)]
enum Part {
    Text(String),
    Image(String),
    Pdf { name: String, data: String },
    Call {
        id: String,
        name: String,
        arguments: String,
    },
    Result {
        id: String,
        content: Vec<Part>,
        error: bool,
    },
}
#[derive(Debug)]
struct Message {
    role: String,
    parts: Vec<Part>,
}
fn unsupported(kind: &str) -> anyhow::Error {
    anyhow::anyhow!("Cross-protocol conversion does not support {kind}. Use a model with the client's native API for this feature.")
}
fn required(v: &Value, key: &str) -> Result<String> {
    let s = text(v, key);
    if s.is_empty() {
        bail!("Missing {key} in protocol content");
    }
    Ok(s.into())
}
fn parts(value: &Value, source: Protocol) -> Result<Vec<Part>> {
    if let Some(s) = value.as_str() {
        return Ok(vec![Part::Text(s.into())]);
    }
    if value.is_null() {
        return Ok(vec![]);
    }
    let list = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Message content must be text or an array"))?;
    let mut out = vec![];
    for part in list {
        match text(part, "type") {
            "text" | "input_text" | "output_text" => {
                out.push(Part::Text(text(part, "text").into()))
            }
            "refusal" => out.push(Part::Text(text(part, "refusal").into())),
            "file" | "input_file" => {
                let file = if text(part, "type") == "file" { &part["file"] } else { part };
                let data = required(file, "file_data")?;
                if !data.starts_with("data:application/pdf;base64,") { return Err(unsupported("non-PDF file input")); }
                out.push(Part::Pdf { name: required(file, "filename")?, data });
            }
            "document" => {
                let source = &part["source"];
                if text(source, "type") != "base64" || text(source, "media_type") != "application/pdf" {
                    return Err(unsupported("non-base64 PDF documents"));
                }
                out.push(Part::Pdf { name: part["title"].as_str().unwrap_or("document.pdf").into(), data: format!("data:application/pdf;base64,{}", required(source, "data")?) });
            }
            "input_image" => out.push(Part::Image(required(part, "image_url")?)),
            "image_url" => out.push(Part::Image(required(&part["image_url"], "url")?)),
            "image" => {
                let image = &part["source"];
                let url = match text(image, "type") {
                    "url" => required(image, "url")?,
                    "base64" => format!(
                        "data:{};base64,{}",
                        required(image, "media_type")?,
                        required(image, "data")?
                    ),
                    kind => return Err(unsupported(kind)),
                };
                out.push(Part::Image(url));
            }
            "tool_use" if source == Protocol::Messages => out.push(Part::Call {
                id: required(part, "id")?,
                name: required(part, "name")?,
                arguments: part["input"].to_string(),
            }),
            "tool_result" if source == Protocol::Messages => out.push(Part::Result {
                id: required(part, "tool_use_id")?,
                content: parts(&part["content"], source)?,
                error: part["is_error"] == true,
            }),
            // Provider-bound thinking/signatures cannot be replayed to a different provider.
            "thinking" | "redacted_thinking" => {}
            kind => return Err(unsupported(kind)),
        }
    }
    Ok(out)
}
fn parse_messages(body: &Value, source: Protocol) -> Result<Vec<Message>> {
    let mut messages = vec![];
    if source == Protocol::Responses {
        for key in ["previous_response_id", "conversation"] {
            if !body[key].is_null() {
                return Err(unsupported(
                    "server-side conversation references; send the full input history",
                ));
            }
        }
        if let Some(s) = body["instructions"].as_str().filter(|s| !s.is_empty()) {
            messages.push(Message {
                role: "system".into(),
                parts: vec![Part::Text(s.into())],
            });
        }
        if let Some(s) = body["input"].as_str() {
            messages.push(Message {
                role: "user".into(),
                parts: vec![Part::Text(s.into())],
            });
            return Ok(messages);
        }
        for item in array(body, "input") {
            let kind = text(item, "type");
            match kind {
                "function_call" | "custom_tool_call" => {
                    let arguments = if kind == "custom_tool_call" {
                        json!({"input":text(item,"input")}).to_string()
                    } else {
                        text(item, "arguments").into()
                    };
                    messages.push(Message {
                        role: "assistant".into(),
                        parts: vec![Part::Call {
                            id: required(item, "call_id")?,
                            name: required(item, "name")?,
                            arguments,
                        }],
                    });
                }
                "function_call_output" | "custom_tool_call_output" => messages.push(Message {
                    role: "user".into(),
                    parts: vec![Part::Result {
                        id: required(item, "call_id")?,
                        content: parts(&item["output"], source)?,
                        error: false,
                    }],
                }),
                "reasoning" => {}
                "" | "message" if !text(item, "role").is_empty() => messages.push(Message {
                    role: required(item, "role")?,
                    parts: parts(&item["content"], source)?,
                }),
                kind => return Err(unsupported(kind)),
            }
        }
    } else {
        if source == Protocol::Messages && !body["system"].is_null() {
            messages.push(Message {
                role: "system".into(),
                parts: parts(&body["system"], source)?,
            });
        }
        for message in array(body, "messages") {
            let role = required(message, "role")?;
            let mut content = parts(&message["content"], source)?;
            if role == "tool" {
                content = vec![Part::Result {
                    id: required(message, "tool_call_id")?,
                    content,
                    error: false,
                }];
            }
            for call in array(message, "tool_calls") {
                if text(call, "type") != "function" {
                    return Err(unsupported("non-function tool calls"));
                }
                content.push(Part::Call {
                    id: required(call, "id")?,
                    name: required(&call["function"], "name")?,
                    arguments: text(&call["function"], "arguments").into(),
                });
            }
            if message.get("function_call").is_some() {
                return Err(unsupported("legacy function_call; use tool_calls"));
            }
            messages.push(Message {
                role: if role == "tool" { "user".into() } else { role },
                parts: content,
            });
        }
    }
    Ok(messages)
}
fn plain(parts: &[Part]) -> Result<String> {
    let mut result = vec![];
    for part in parts {
        if let Part::Text(s) = part {
            result.push(s.clone());
        } else {
            return Err(unsupported("non-text tool results for this upstream"));
        }
    }
    Ok(result.join("\n"))
}
fn image_content(url: &str, target: Protocol) -> Result<Value> {
    Ok(match target {
        Protocol::Chat => json!({"type":"image_url","image_url":{"url":url}}),
        Protocol::Responses => json!({"type":"input_image","image_url":url}),
        Protocol::Messages => {
            let source = if let Some(data) = url.strip_prefix("data:") {
                let (media, data) = data
                    .split_once(";base64,")
                    .ok_or_else(|| unsupported("non-base64 data URLs"))?;
                json!({"type":"base64","media_type":media,"data":data})
            } else {
                json!({"type":"url","url":url})
            };
            json!({"type":"image","source":source})
        }
    })
}
fn content(parts: &[Part], target: Protocol, role: &str) -> Result<Vec<Value>> {
    parts.iter().map(|part| Ok(match part {
        Part::Text(s) => json!({"type":if target == Protocol::Responses { if role == "assistant" {"output_text"} else {"input_text"} } else {"text"},"text":s}),
        Part::Image(url) => image_content(url,target)?,
        Part::Pdf { name, data } => match target {
            Protocol::Chat => json!({"type":"file","file":{"filename":name,"file_data":data}}),
            Protocol::Responses => json!({"type":"input_file","filename":name,"file_data":data}),
            Protocol::Messages => json!({"type":"document","title":name,"source":{"type":"base64","media_type":"application/pdf","data":data.strip_prefix("data:application/pdf;base64,").ok_or_else(|| unsupported("invalid PDF data"))?}}),
        },
        _ => return Err(unsupported("nested tool calls")),
    })).collect()
}
fn encode_messages(messages: &[Message], target: Protocol) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut out = vec![];
    let mut system = vec![];
    for message in messages {
        let role = if message.role == "developer" {
            "system"
        } else {
            &message.role
        };
        if !matches!(role, "system" | "user" | "assistant") {
            bail!("Unsupported role: {role}");
        }
        if target == Protocol::Messages && role == "system" {
            system.extend(content(&message.parts, target, role)?);
            continue;
        }
        let mut buffer = vec![];
        let flush = |buffer: &mut Vec<Part>, out: &mut Vec<Value>| -> Result<()> {
            if !buffer.is_empty() {
                out.push(json!({"role":role,"content":content(buffer,target,role)?}));
                buffer.clear();
            }
            Ok(())
        };
        if target == Protocol::Messages {
            let mut blocks = vec![];
            for part in &message.parts {
                blocks.push(match part {
                    Part::Call { id,name,arguments } => json!({"type":"tool_use","id":id,"name":name,"input":serde_json::from_str::<Value>(arguments)?}),
                    Part::Result { id,content:result,error } => json!({"type":"tool_result","tool_use_id":id,"content":content(result,target,"user")?,"is_error":error}),
                    part => content(std::slice::from_ref(part),target,role)?.remove(0),
                });
            }
            if !blocks.is_empty() {
                // Anthropic requires consecutive messages of the same role to be merged.
                if let Some(last) = out.last_mut().filter(|v| text(v, "role") == role) {
                    last["content"].as_array_mut().unwrap().extend(blocks);
                } else {
                    out.push(json!({"role":role,"content":blocks}));
                }
            }
        } else if target == Protocol::Chat {
            let mut calls = vec![];
            for part in &message.parts {
                match part {
                    Part::Call{id,name,arguments} => calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}})),
                    Part::Result{id,content,error} => { flush(&mut buffer,&mut out)?; let value = plain(content)?; out.push(json!({"role":"tool","tool_call_id":id,"content":if *error {format!("Tool error: {value}")} else {value}})); },
                    part => buffer.push(part.clone()),
                }
            }
            if !calls.is_empty() {
                out.push(json!({"role":"assistant","content":if buffer.is_empty(){Value::Null}else{Value::Array(content(&buffer,target,role)?)},"tool_calls":calls}));
            } else {
                flush(&mut buffer, &mut out)?;
            }
        } else {
            for part in &message.parts {
                match part {
                    Part::Call {
                        id,
                        name,
                        arguments,
                    } => {
                        flush(&mut buffer, &mut out)?;
                        out.push(json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments}));
                    }
                    Part::Result {
                        id,
                        content: result,
                        error,
                    } => {
                        flush(&mut buffer, &mut out)?;
                        out.push(json!({"type":"function_call_output","call_id":id,"output":if *error {Value::String(format!("Tool error: {}",plain(result)?))} else {Value::Array(content(result,target,"user")?)}}));
                    }
                    part => buffer.push(part.clone()),
                }
            }
            flush(&mut buffer, &mut out)?;
        }
    }
    Ok((out, system))
}

pub fn convert_request(
    body: &Value,
    source: Protocol,
    target: Protocol,
    model: &str,
) -> Result<(Value, ToolMap)> {
    let mut map = ToolMap::default();
    if source == target {
        let mut result = body.clone();
        result["model"] = model.into();
        return Ok((result, map));
    }
    let normalized;
    let body = if source == Protocol::Responses { normalized = flatten_namespaces(body, &mut map)?; &normalized } else { body };
    if !body
        .pointer("/output_config/format")
        .unwrap_or(&Value::Null)
        .is_null()
    {
        return Err(unsupported("Messages structured output constraints"));
    }
    // A reasoning setting has provider-specific semantics; this portable subset
    // cannot silently replace it with an ordinary text request.
    for key in ["background", "audio", "prediction", "reasoning_effort"] {
        if !body[key].is_null() && body[key] != false {
            return Err(unsupported(key));
        }
    }
    if body
        .get("n")
        .and_then(Value::as_u64)
        .is_some_and(|n| n != 1)
    {
        return Err(unsupported("multiple choices (n > 1)"));
    }
    let messages = parse_messages(body, source)?;
    let (messages, system) = encode_messages(&messages, target)?;
    let mut out = json!({"model":model,"stream":body["stream"].as_bool().unwrap_or(false)});
    out[if target == Protocol::Responses {
        "input"
    } else {
        "messages"
    }] = messages.into();
    if !system.is_empty() {
        out["system"] = system.into();
    }
    for key in ["temperature", "top_p"] {
        if !body[key].is_null() {
            out[key] = body[key].clone();
        }
    }
    let tokens = ["max_output_tokens", "max_completion_tokens", "max_tokens"]
        .iter()
        .find_map(|key| body.get(key).filter(|v| !v.is_null()));
    if let Some(tokens) = tokens {
        out[if target == Protocol::Responses {
            "max_output_tokens"
        } else {
            "max_tokens"
        }] = tokens.clone();
    }
    let stop = body.get("stop_sequences").or_else(|| body.get("stop"));
    if let Some(stop) = stop.filter(|v| !v.is_null()) {
        if target == Protocol::Responses {
            return Err(unsupported("stop sequences on a Responses upstream"));
        }
        out[if target == Protocol::Messages {
            "stop_sequences"
        } else {
            "stop"
        }] = if stop.is_string() {
            json!([stop])
        } else {
            stop.clone()
        };
    }
    let mut tools = vec![];
    for tool in array(body, "tools") {
        let kind = text(tool, "type");
        let custom = source == Protocol::Responses && kind == "custom";
        let function = if source == Protocol::Chat {
            if kind != "function" {
                return Err(unsupported(kind));
            }
            &tool["function"]
        } else {
            tool
        };
        if source == Protocol::Responses && !matches!(kind, "function" | "custom") {
            return Err(unsupported(kind));
        }
        if source == Protocol::Messages && !matches!(kind, "" | "custom") {
            return Err(unsupported(kind));
        }
        let name = required(function, "name")?;
        if custom {
            map.custom.insert(name.clone());
        }
        let schema = if custom {
            json!({"type":"object","properties":{"input":{"type":"string"}},"required":["input"],"additionalProperties":false})
        } else {
            function
                .get(if source == Protocol::Messages {
                    "input_schema"
                } else {
                    "parameters"
                })
                .cloned()
                .unwrap_or(json!({"type":"object","properties":{}}))
        };
        let mut definition = json!({"name":name,"description":function.get("description").and_then(Value::as_str).unwrap_or(""),"parameters":schema});
        if let Some(strict) = function.get("strict") {
            definition["strict"] = strict.clone();
        }
        tools.push(match target {
            Protocol::Chat=>json!({"type":"function","function":definition}),
            Protocol::Responses=>{definition["type"]="function".into();definition},
            Protocol::Messages=>json!({"name":definition["name"],"description":definition["description"],"input_schema":definition["parameters"]}),
        });
    }
    if !tools.is_empty() {
        out["tools"] = tools.into();
    }
    if let Some(choice) = body.get("tool_choice").filter(|v| !v.is_null()) {
        let mode = choice.as_str().unwrap_or_else(|| text(choice, "type"));
        let name = if source == Protocol::Chat {
            text(&choice["function"], "name")
        } else {
            text(choice, "name")
        };
        let mode = if mode == "any" { "required" } else { mode };
        if !name.is_empty() {
            out["tool_choice"] = match target {
                Protocol::Chat => json!({"type":"function","function":{"name":name}}),
                Protocol::Responses => {
                    json!({"type":if map.custom.contains(name){"custom"}else{"function"},"name":name})
                }
                Protocol::Messages => json!({"type":"tool","name":name}),
            };
        } else if matches!(mode, "auto" | "none" | "required") {
            out["tool_choice"] = if target == Protocol::Messages {
                json!({"type":if mode=="required"{"any"}else{mode}})
            } else {
                mode.into()
            };
        } else {
            return Err(unsupported("this tool_choice"));
        }
    }
    if let Some(parallel) = body.get("parallel_tool_calls").and_then(Value::as_bool) {
        if target == Protocol::Messages {
            if out["tool_choice"].is_null() {
                out["tool_choice"] = json!({"type":"auto"});
            }
            out["tool_choice"]["disable_parallel_tool_use"] = (!parallel).into();
        } else {
            out["parallel_tool_calls"] = parallel.into();
        }
    } else if let Some(disabled) = body
        .pointer("/tool_choice/disable_parallel_tool_use")
        .and_then(Value::as_bool)
    {
        out["parallel_tool_calls"] = (!disabled).into();
    }
    // Portable structured output settings. Do not silently drop a schema constraint.
    let format = if source == Protocol::Responses {
        body.pointer("/text/format").cloned()
    } else {
        body.get("response_format").cloned()
    };
    if let Some(mut format) = format.filter(|f| text(f, "type") != "text") {
        if target == Protocol::Messages {
            return Err(unsupported(
                "structured output constraints on a Messages upstream",
            ));
        }
        if target == Protocol::Responses {
            if text(&format, "type") == "json_schema" {
                let mut f = format["json_schema"].clone();
                f["type"] = "json_schema".into();
                format = f;
            }
            out["text"] = json!({"format":format});
        } else {
            if text(&format, "type") == "json_schema" {
                let mut f = format.clone();
                f.as_object_mut().unwrap().remove("type");
                format = json!({"type":"json_schema","json_schema":f});
            }
            out["response_format"] = format;
        }
    }
    if target == Protocol::Responses {
        out["store"] = false.into();
    }
    if target == Protocol::Chat && out["stream"] == true {
        out["stream_options"] = json!({"include_usage":true});
    }
    Ok((out, map))
}
