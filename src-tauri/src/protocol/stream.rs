use super::response::{decode_response, validate_tools, Completion, Output};
use super::*;
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use std::collections::{HashMap, VecDeque};

const MAX_FRAME: usize = 2 * 1024 * 1024;
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
#[derive(Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
}
impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<(String, String)>> {
        let mut frames = vec![];
        for frame in self.push_raw(bytes)? {
            let frame = String::from_utf8(frame)?;
            let mut event = String::new();
            let mut data = vec![];
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("event:") { event = value.trim_start().into(); }
                if let Some(value) = line.strip_prefix("data:") { data.push(value.strip_prefix(' ').unwrap_or(value)); }
            }
            if !data.is_empty() { frames.push((event, data.join("\n"))); }
        }
        Ok(frames)
    }

    /// Preserve comments and provider extensions while sharing the same bounded framing.
    pub fn push_raw(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = vec![];
        loop {
            let lf = self
                .buffer
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|i| (i, 2));
            let crlf = self
                .buffer
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|i| (i, 4));
            let boundary = match (lf, crlf) {
                (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
                (a, b) => a.or(b),
            };
            let Some((end, length)) = boundary else {
                break;
            };
            if end > MAX_FRAME {
                bail!("Upstream SSE frame is too large");
            }
            frames.push(self.buffer.drain(..end + length).collect());
        }
        if self.buffer.len() > MAX_FRAME {
            bail!("Upstream SSE frame is too large");
        }
        Ok(frames)
    }
    pub fn clean_eof(&self) -> bool {
        self.buffer.iter().all(u8::is_ascii_whitespace)
    }
}

pub(super) struct Bridge {
    source: Protocol,
    target: Protocol,
    map: ToolMap,
    completion: Completion,
    keys: HashMap<String, usize>,
    sequence: u64,
    pub terminal: bool,
    finish_seen: bool,
    size: usize,
}
impl Bridge {
    pub fn new(source: Protocol, target: Protocol, model: &str, map: ToolMap) -> Self {
        Self {
            source,
            target,
            map,
            completion: Completion::new(model),
            keys: HashMap::new(),
            sequence: 0,
            terminal: false,
            finish_seen: false,
            size: 0,
        }
    }
    fn event(&mut self, kind: &str, mut data: Value) -> String {
        if self.target == Protocol::Responses {
            data["sequence_number"] = self.sequence.into();
            self.sequence += 1;
        }
        if self.target == Protocol::Chat {
            format!("data: {data}\n\n")
        } else {
            data["type"] = kind.into();
            format!("event: {kind}\ndata: {data}\n\n")
        }
    }
    fn chat(&mut self, delta: Value, reason: Value) -> String {
        let data = json!({"id":self.completion.id,"object":"chat.completion.chunk","created":self.completion.created,"model":self.completion.model,"choices":[{"index":0,"delta":delta,"finish_reason":reason}]});
        self.event("", data)
    }
    pub fn begin(&mut self) -> Result<String> {
        Ok(match self.target{
        Protocol::Chat=>self.chat(json!({"role":"assistant","content":""}),Value::Null),
        Protocol::Messages=>self.event("message_start",json!({"message":{"id":self.completion.id,"type":"message","role":"assistant","model":self.completion.model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}})),
        Protocol::Responses=>{
            let response=self.completion.value(self.target,&self.map,false)?;
            let mut events=self.event("response.created",json!({"response":response}));
            events+=&self.event("response.in_progress",json!({"response":response}));events
        }
    })
    }
    fn add(&mut self, key: &str, output: Output) -> Result<(usize, String)> {
        if let Some(index) = self.keys.get(key) {
            return Ok((*index, String::new()));
        }
        let index = self.completion.output.len();
        self.keys.insert(key.into(), index);
        self.completion.output.push(output);
        let event=match self.target {
            Protocol::Chat=>match &self.completion.output[index]{
                Output::Text(_)=>String::new(),
                Output::Tool{id,name,..}=>self.chat(json!({"tool_calls":[{"index":self.tool_index(index),"id":id,"type":"function","function":{"name":name,"arguments":""}}]}),Value::Null),
            },
            Protocol::Messages=>{
                let block=match &self.completion.output[index]{Output::Text(_)=>json!({"type":"text","text":""}),Output::Tool{id,name,..}=>json!({"type":"tool_use","id":id,"name":name,"input":{}})};
                self.event("content_block_start",json!({"index":index,"content_block":block}))
            }
            Protocol::Responses=>{
                let item=self.completion.item(index,&self.map,false)?;
                let mut events=self.event("response.output_item.added",json!({"output_index":index,"item":item}));
                if matches!(self.completion.output[index],Output::Text(_)){events+=&self.event("response.content_part.added",json!({"item_id":item["id"],"output_index":index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}}));}events
            }
        };
        Ok((index, event))
    }
    fn tool_index(&self, index: usize) -> usize {
        self.completion.output[..index]
            .iter()
            .filter(|p| matches!(p, Output::Tool { .. }))
            .count()
    }
    fn text(&mut self, key: &str, delta: &str) -> Result<String> {
        if delta.is_empty() {
            return Ok(String::new());
        }
        let (index, mut events) = self.add(key, Output::Text(String::new()))?;
        let Output::Text(text) = &mut self.completion.output[index] else {
            bail!("Conflicting upstream content indexes");
        };
        text.push_str(delta);
        events+=&match self.target{
            Protocol::Chat=>self.chat(json!({"content":delta}),Value::Null),
            Protocol::Messages=>self.event("content_block_delta",json!({"index":index,"delta":{"type":"text_delta","text":delta}})),
            Protocol::Responses=>self.event("response.output_text.delta",json!({"item_id":format!("msg_{}_{}",self.completion.id,index),"output_index":index,"content_index":0,"delta":delta,"logprobs":[]})),
        };
        Ok(events)
    }
    fn tool(&mut self, key: &str, call_id: &str, name: &str, delta: &str) -> Result<String> {
        if !self.keys.contains_key(key) && (call_id.is_empty() || name.is_empty()) {
            bail!("Tool arguments arrived before an upstream call ID and name");
        }
        let (index, mut events) = self.add(
            key,
            Output::Tool {
                id: call_id.into(),
                name: name.into(),
                arguments: String::new(),
            },
        )?;
        let Output::Tool {
            id: existing,
            name: existing_name,
            arguments,
        } = &mut self.completion.output[index]
        else {
            bail!("Conflicting upstream content indexes");
        };
        if (!call_id.is_empty() && call_id != existing)
            || (!name.is_empty() && name != existing_name)
        {
            bail!("Upstream changed a tool call ID or name mid-stream");
        }
        arguments.push_str(delta);
        if delta.is_empty() {
            return Ok(events);
        }
        let custom = self.map.custom.contains(existing_name);
        events+=&match self.target{
            Protocol::Chat=>self.chat(json!({"tool_calls":[{"index":self.tool_index(index),"function":{"arguments":delta}}]}),Value::Null),
            Protocol::Messages=>self.event("content_block_delta",json!({"index":index,"delta":{"type":"input_json_delta","partial_json":delta}})),
            // Custom tools accept raw strings; emit their decoded input once JSON is complete.
            Protocol::Responses if custom=>String::new(),
            Protocol::Responses=>self.event("response.function_call_arguments.delta",json!({"item_id":format!("fc_{}_{}",self.completion.id,index),"output_index":index,"delta":delta})),
        };
        Ok(events)
    }
    pub fn finish(&mut self) -> Result<String> {
        if self.terminal {
            return Ok(String::new());
        }
        if !self.finish_seen {
            bail!("Upstream stream ended before a completion event");
        }
        validate_tools(&self.completion)?;
        let final_value = self.completion.value(self.target, &self.map, true)?;
        let mut events = String::new();
        match self.target {
            Protocol::Chat => {
                events += &self.chat(json!({}), self.completion.chat_reason().into());
                events+=&self.event("",json!({"id":self.completion.id,"object":"chat.completion.chunk","created":self.completion.created,"model":self.completion.model,"choices":[],"usage":self.completion.usage.value(self.target)}));
                events += "data: [DONE]\n\n";
            }
            Protocol::Messages => {
                for index in 0..self.completion.output.len() {
                    events += &self.event("content_block_stop", json!({"index":index}));
                }
                events+=&self.event("message_delta",json!({"delta":{"stop_reason":self.completion.stop_reason(),"stop_sequence":null},"usage":self.completion.usage.value(self.target)}));
                events += &self.event("message_stop", json!({}));
            }
            Protocol::Responses => {
                for index in 0..self.completion.output.len() {
                    let item = self.completion.item(index, &self.map, true)?;
                    if matches!(self.completion.output[index], Output::Text(_)) {
                        let part = &item["content"][0];
                        events+=&self.event("response.output_text.done",json!({"output_index":index,"item_id":item["id"],"content_index":0,"text":part["text"],"logprobs":[]}));
                        events+=&self.event("response.content_part.done",json!({"output_index":index,"item_id":item["id"],"content_index":0,"part":part}));
                    } else if text(&item, "type") == "custom_tool_call" {
                        events+=&self.event("response.custom_tool_call_input.delta",json!({"output_index":index,"item_id":item["id"],"delta":item["input"]}));
                        events+=&self.event("response.custom_tool_call_input.done",json!({"output_index":index,"item_id":item["id"],"input":item["input"]}));
                    } else {
                        events+=&self.event("response.function_call_arguments.done",json!({"output_index":index,"item_id":item["id"],"name":item["name"],"arguments":item["arguments"]}));
                    }
                    events += &self.event(
                        "response.output_item.done",
                        json!({"output_index":index,"item":item}),
                    );
                }
                events += &self.event(
                    if self.completion.reason == "length"
                        || self.completion.reason == "content_filter"
                    {
                        "response.incomplete"
                    } else {
                        "response.completed"
                    },
                    json!({"response":final_value}),
                );
            }
        }
        self.terminal = true;
        Ok(events)
    }
    pub fn error(&mut self, message: &str) -> String {
        self.terminal = true;
        if self.target == Protocol::Responses {
            let mut response = self
                .completion
                .value(self.target, &self.map, false)
                .unwrap_or(json!({}));
            response["status"] = "failed".into();
            response["error"] = json!({"code":"upstream_error","message":message});
            self.event("response.failed", json!({"response":response}))
        } else {
            self.event("error", self.target.error(message))
        }
    }
    pub fn frame(&mut self, event: &str, data: &str) -> Result<String> {
        if self.terminal {
            return Ok(String::new());
        }
        self.size += data.len();
        if self.size > MAX_OUTPUT {
            bail!("Converted response exceeds the 16 MiB limit");
        }
        if data == "[DONE]" {
            return self.finish();
        }
        let mut value: Value = serde_json::from_str(data)?;
        if value["type"].is_null() && !event.is_empty() {
            value["type"] = event.into();
        }
        if !value["error"].is_null() {
            bail!("Upstream stream error: {}", value["error"]);
        }
        let mut events = String::new();
        match self.source {
            Protocol::Chat => {
                self.completion.usage.read(&value["usage"], self.source);
                for choice in array(&value, "choices") {
                    if choice["index"].as_u64().unwrap_or(0) != 0 {
                        bail!("Multiple upstream choices cannot be converted");
                    }
                    let delta = &choice["delta"];
                    for key in ["content", "refusal"] {
                        if let Some(s) = delta[key].as_str() {
                            events += &self.text("text", s)?;
                        }
                    }
                    for call in array(delta, "tool_calls") {
                        let key = format!("tool:{}", call["index"].as_u64().unwrap_or(0));
                        events += &self.tool(
                            &key,
                            text(call, "id"),
                            text(&call["function"], "name"),
                            text(&call["function"], "arguments"),
                        )?;
                    }
                    if let Some(reason) = choice["finish_reason"].as_str() {
                        self.completion.reason = reason.into();
                        self.finish_seen = true;
                    }
                }
            }
            Protocol::Messages => match text(&value, "type") {
                "message_start" => self
                    .completion
                    .usage
                    .read(&value["message"]["usage"], self.source),
                "content_block_start" => {
                    let key = format!("block:{}", value["index"]);
                    let block = &value["content_block"];
                    match text(block, "type") {
                        "text" => events += &self.text(&key, text(block, "text"))?,
                        "tool_use" => {
                            let arguments =
                                if block["input"].as_object().is_some_and(|o| !o.is_empty()) {
                                    block["input"].to_string()
                                } else {
                                    String::new()
                                };
                            events += &self.tool(
                                &key,
                                text(block, "id"),
                                text(block, "name"),
                                &arguments,
                            )?;
                        }
                        "thinking" | "redacted_thinking" => {}
                        kind => bail!("Unsupported upstream stream block: {kind}"),
                    }
                }
                "content_block_delta" => {
                    let key = format!("block:{}", value["index"]);
                    let delta = &value["delta"];
                    match text(delta, "type") {
                        "text_delta" => events += &self.text(&key, text(delta, "text"))?,
                        "input_json_delta" => {
                            events += &self.tool(&key, "", "", text(delta, "partial_json"))?
                        }
                        "thinking_delta" | "signature_delta" => {}
                        kind => bail!("Unsupported upstream stream delta: {kind}"),
                    }
                }
                "content_block_stop" => {
                    let key = format!("block:{}", value["index"]);
                    if let Some(i) = self.keys.get(&key).copied() {
                        if let Output::Tool { arguments, .. } = &self.completion.output[i] {
                            if arguments.is_empty() {
                                events += &self.tool(&key, "", "", "{}")?;
                            }
                        }
                    }
                }
                "message_delta" => {
                    self.completion.usage.read(&value["usage"], self.source);
                    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                        self.completion.reason = match reason {
                            "max_tokens" => "length",
                            "refusal" => "content_filter",
                            _ => "stop",
                        }
                        .into();
                        self.finish_seen = true;
                    }
                }
                "message_stop" => events += &self.finish()?,
                "ping" => {}
                kind => bail!("Unsupported upstream stream event: {kind}"),
            },
            Protocol::Responses => {
                let key = format!("item:{}", value["output_index"]);
                match text(&value, "type") {
                    "response.output_item.added" => {
                        let item = &value["item"];
                        match text(item, "type") {
                            "function_call" => {
                                events += &self.tool(
                                    &key,
                                    text(item, "call_id"),
                                    text(item, "name"),
                                    text(item, "arguments"),
                                )?
                            }
                            "message" | "reasoning" => {}
                            kind => bail!("Unsupported upstream output item: {kind}"),
                        }
                    }
                    "response.output_text.delta" | "response.refusal.delta" => {
                        events += &self.text(
                            &format!("{key}:{}", value["content_index"]),
                            text(&value, "delta"),
                        )?
                    }
                    "response.function_call_arguments.delta" => {
                        events += &self.tool(&key, "", "", text(&value, "delta"))?
                    }
                    "response.completed" | "response.incomplete" => {
                        let decoded = decode_response(
                            &value["response"],
                            self.source,
                            &self.completion.model,
                        )?;
                        if self.completion.output.is_empty() {
                            for (i, part) in decoded.output.iter().enumerate() {
                                let key = format!("final:{i}");
                                events += &match part {
                                    Output::Text(s) => self.text(&key, s)?,
                                    Output::Tool {
                                        id,
                                        name,
                                        arguments,
                                    } => self.tool(&key, id, name, arguments)?,
                                };
                            }
                        }
                        self.completion.usage = decoded.usage;
                        self.completion.reason = decoded.reason;
                        self.finish_seen = true;
                        events += &self.finish()?;
                    }
                    "response.failed" | "error" => bail!("Upstream Responses stream failed"),
                    "response.created"
                    | "response.in_progress"
                    | "response.content_part.added"
                    | "response.content_part.done"
                    | "response.output_text.done"
                    | "response.refusal.done"
                    | "response.function_call_arguments.done"
                    | "response.output_item.done" => {}
                    kind if kind.starts_with("response.reasoning") => {}
                    kind => bail!("Unsupported upstream stream event: {kind}"),
                }
            }
        }
        Ok(events)
    }
}

/// Pull-based streaming: cancellation drops the upstream body, with no detached task.
#[cfg(test)]
pub fn converted_stream(
    response: reqwest::Response,
    source: Protocol,
    target: Protocol,
    model: String,
    map: ToolMap,
) -> impl Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send {
    converted_stream_observed(response.bytes_stream(), source, target, model, map, || {})
}

pub fn converted_stream_observed<S, F, E>(
    stream: S,
    source: Protocol,
    target: Protocol,
    model: String,
    map: ToolMap,
    on_error: F,
) -> impl Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send
where
    S: Stream<Item = std::result::Result<Bytes, E>> + Send,
    E: Send,
    F: Fn() + Send,
{
    let upstream = Box::pin(stream);
    let mut bridge = Bridge::new(source, target, &model, map);
    let mut pending = VecDeque::new();
    match bridge.begin() {
        Ok(events) => pending.push_back(Bytes::from(events)),
        Err(e) => { on_error(); pending.push_back(Bytes::from(bridge.error(&e.to_string()))); },
    }
    futures_util::stream::unfold(
        (upstream, SseParser::default(), bridge, pending, on_error),
        |(mut upstream, mut parser, mut bridge, mut pending, on_error)| async move {
            loop {
                if let Some(bytes) = pending.pop_front() {
                    return Some((Ok(bytes), (upstream, parser, bridge, pending, on_error)));
                }
                if bridge.terminal {
                    return None;
                }
                let result =
                    Ok::<_,std::io::Error>(upstream.next().await);
                let events = match result {
                    Ok(Some(Ok(bytes))) => parser.push(&bytes).and_then(|frames| {
                        let mut out = String::new();
                        for (event, data) in frames {
                            out += &bridge.frame(&event, &data)?;
                        }
                        Ok(out)
                    }),
                    Ok(Some(Err(_))) => Err(anyhow::anyhow!("Upstream connection interrupted")),
                    Err(_) => Err(anyhow::anyhow!("Upstream stream timed out")),
                    Ok(None) => {
                        if parser.clean_eof() {
                            bridge.finish()
                        } else {
                            Err(anyhow::anyhow!("Upstream stream ended inside an SSE frame"))
                        }
                    }
                };
                match events {
                    Ok(events) if !events.is_empty() => pending.push_back(Bytes::from(events)),
                    Ok(_) => {}
                    Err(e) => { on_error(); pending.push_back(Bytes::from(bridge.error(&e.to_string()))); },
                }
            }
        },
    )
}

/// Parse incremental text without exposing reasoning as the final answer.
#[derive(Default)]
pub struct DebugProgress { parser: SseParser }
impl DebugProgress {
    pub fn push(&mut self, bytes: &[u8], protocol: Protocol) -> Result<Vec<Value>> {
        let mut output = Vec::new();
        for (_, data) in self.parser.push(bytes)? {
            if data == "[DONE]" { continue; }
            let value: Value = serde_json::from_str(&data)?;
            let (text, reasoning) = match protocol {
                Protocol::Chat => {
                    let delta = &value["choices"][0]["delta"];
                    (delta["content"].as_str().unwrap_or(""), delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()).is_some_and(|s| !s.is_empty()))
                },
                Protocol::Messages => (
                    value["delta"]["text"].as_str().or(value["content_block"]["text"].as_str()).unwrap_or(""),
                    value["delta"]["type"] == "thinking_delta" || value["content_block"]["type"] == "thinking"
                ),
                Protocol::Responses => (
                    if value["type"] == "response.output_text.delta" || value["type"] == "response.refusal.delta" { value["delta"].as_str().unwrap_or("") } else { "" },
                    value["type"].as_str().is_some_and(|s| s.starts_with("response.reasoning"))
                ),
            };
            if !text.is_empty() || reasoning { output.push(json!({"text":text,"reasoning":reasoning})); }
        }
        Ok(output)
    }
}

/// Reconstruct a completed debug response while retaining the original SSE separately.
pub fn collect_debug_stream(bytes: &[u8], protocol: Protocol, model: &str) -> Result<Value> {
    let mut parser = SseParser::default();
    let mut bridge = Bridge::new(protocol, protocol, model, ToolMap::default());
    for (event, data) in parser.push(bytes)? {
        bridge.frame(&event, &data)?;
    }
    if !parser.clean_eof() { bail!("Upstream stream ended inside an SSE frame"); }
    bridge.finish()?;
    bridge.completion.value(protocol, &bridge.map, true)
}

#[cfg(test)]
mod debug_stream_tests {
    use super::*;
    #[test]
    fn debug_progress_handles_split_frames_in_all_protocols() {
        let cases = [
            (Protocol::Chat, "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n"),
            (Protocol::Messages, "event: content_block_delta\ndata: {\"delta\":{\"type\":\"text_delta\",\"text\":\"你好\"}}\n\n"),
            (Protocol::Responses, "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\n\n"),
        ];
        for (protocol, data) in cases {
            let mut progress = DebugProgress::default();
            let mut events = Vec::new();
            for byte in data.as_bytes() { events.extend(progress.push(&[*byte], protocol).unwrap()); }
            assert_eq!(events, vec![json!({"text":"你好","reasoning":false})]);
        }
        let events = DebugProgress::default().push(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"private\"}}]}\n\n", Protocol::Chat).unwrap();
        assert_eq!(events, vec![json!({"text":"","reasoning":true})]);
    }

    #[test]
    fn collects_text_and_rejects_incomplete_streams() {
        let bytes = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let value = collect_debug_stream(bytes, Protocol::Chat, "test").unwrap();
        assert_eq!(value["choices"][0]["message"]["content"], "Hello world");
        assert!(collect_debug_stream(b"data: {\"choices\":[]}\n\n", Protocol::Chat, "test").is_err());
    }
}
