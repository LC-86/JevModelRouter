use super::*;

#[derive(Clone, Debug)]
pub(super) enum Output {
    Text(String),
    Tool {
        id: String,
        name: String,
        arguments: String,
    },
}
#[derive(Clone, Default, Debug)]
pub(super) struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}
impl Usage {
    pub fn read(&mut self, value: &Value, source: Protocol) {
        let (input, output) = if source == Protocol::Chat {
            ("prompt_tokens", "completion_tokens")
        } else {
            ("input_tokens", "output_tokens")
        };
        if let Some(n) = value[input].as_u64() {
            self.input = n;
        }
        if let Some(n) = value[output].as_u64() {
            self.output = n;
        }
        self.cached = value
            .pointer("/prompt_tokens_details/cached_tokens")
            .or_else(|| value.pointer("/input_tokens_details/cached_tokens"))
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.cached);
        if source == Protocol::Messages && value.get("input_tokens").is_some() {
            self.input += self.cached + value["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        }
    }
    pub fn value(&self, target: Protocol) -> Value {
        match target {
            Protocol::Chat => {
                json!({"prompt_tokens":self.input,"completion_tokens":self.output,"total_tokens":self.input+self.output,"prompt_tokens_details":{"cached_tokens":self.cached}})
            }
            Protocol::Responses => {
                json!({"input_tokens":self.input,"output_tokens":self.output,"total_tokens":self.input+self.output,"input_tokens_details":{"cached_tokens":self.cached},"output_tokens_details":{"reasoning_tokens":0}})
            }
            Protocol::Messages => {
                json!({"input_tokens":self.input.saturating_sub(self.cached),"output_tokens":self.output,"cache_read_input_tokens":self.cached,"cache_creation_input_tokens":0})
            }
        }
    }
}
#[derive(Debug)]
pub(super) struct Completion {
    pub id: String,
    pub model: String,
    pub output: Vec<Output>,
    pub usage: Usage,
    pub reason: String,
    pub created: i64,
}
impl Completion {
    pub fn new(model: &str) -> Self {
        Self {
            id: id("resp"),
            model: model.into(),
            output: vec![],
            usage: Usage::default(),
            reason: "stop".into(),
            created: chrono::Utc::now().timestamp(),
        }
    }
    pub fn stop_reason(&self) -> &str {
        if self.reason == "length" {
            "max_tokens"
        } else if self.reason == "content_filter" {
            "refusal"
        } else if self.output.iter().any(|p| matches!(p, Output::Tool { .. })) {
            "tool_use"
        } else {
            "end_turn"
        }
    }
    pub fn item(&self, index: usize, map: &ToolMap, done: bool) -> Result<Value> {
        let status = if done { "completed" } else { "in_progress" };
        Ok(match &self.output[index] {
            Output::Text(text) => {
                json!({"id":format!("msg_{}_{}",self.id,index),"type":"message","role":"assistant","status":status,"content":if done{json!([{"type":"output_text","text":text,"annotations":[],"logprobs":[]}])}else{json!([])}})
            }
            Output::Tool {
                id,
                name,
                arguments,
            } => {
                let mut item = if map.custom.contains(name) {
                    let input = if done {
                        let args: Value = serde_json::from_str(arguments)?;
                        args["input"]
                            .as_str()
                            .ok_or_else(|| {
                                anyhow::anyhow!("Custom tool arguments must include a string input")
                            })?
                            .to_owned()
                    } else {
                        String::new()
                    };
                    json!({"type":"custom_tool_call","id":format!("ctc_{}_{}",self.id,index),"call_id":id,"name":name,"input":input,"status":status})
                } else {
                    json!({"type":"function_call","id":format!("fc_{}_{}",self.id,index),"call_id":id,"name":name,"arguments":if done{arguments.as_str()}else{""},"status":status})
                };
                if let Some((namespace, original)) = map.namespaces.get(name) {
                    item["namespace"] = namespace.clone().into();
                    item["name"] = original.clone().into();
                }
                item
            }
        })
    }
    pub fn value(&self, target: Protocol, map: &ToolMap, done: bool) -> Result<Value> {
        Ok(match target {
            Protocol::Chat => {
                let content = self
                    .output
                    .iter()
                    .filter_map(|p| {
                        if let Output::Text(s) = p {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let calls:Vec<_>=self.output.iter().filter_map(|p|if let Output::Tool{id,name,arguments}=p{Some(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}}))}else{None}).collect();
                let mut message = json!({"role":"assistant","content":if content.is_empty(){Value::Null}else{content.into()}});
                if !calls.is_empty() {
                    message["tool_calls"] = calls.into();
                }
                json!({"id":self.id,"object":"chat.completion","created":self.created,"model":self.model,"choices":[{"index":0,"message":message,"finish_reason":self.chat_reason()}],"usage":self.usage.value(target)})
            }
            Protocol::Messages => {
                let content=self.output.iter().map(|p|Ok(match p{Output::Text(s)=>json!({"type":"text","text":s}),Output::Tool{id,name,arguments}=>json!({"type":"tool_use","id":id,"name":name,"input":serde_json::from_str::<Value>(arguments)?})})).collect::<Result<Vec<_>>>()?;
                json!({"id":self.id,"type":"message","role":"assistant","model":self.model,"content":content,"stop_reason":self.stop_reason(),"stop_sequence":null,"usage":self.usage.value(target)})
            }
            Protocol::Responses => {
                json!({"id":self.id,"object":"response","created_at":self.created,"model":self.model,"status":if !done{"in_progress"}else if self.reason=="length"||self.reason=="content_filter"{"incomplete"}else{"completed"},"error":null,"incomplete_details":if self.reason=="length"{json!({"reason":"max_output_tokens"})}else if self.reason=="content_filter"{json!({"reason":"content_filter"})}else{Value::Null},"output":if done{self.output.iter().enumerate().map(|(i,_)|self.item(i,map,true)).collect::<Result<Vec<_>>>()?}else{vec![]},"usage":if done{self.usage.value(target)}else{Value::Null},"parallel_tool_calls":true,"tool_choice":"auto","tools":[],"store":false})
            }
        })
    }
    pub fn chat_reason(&self) -> &str {
        if self.reason == "length" {
            "length"
        } else if self.reason == "content_filter" {
            "content_filter"
        } else if self.output.iter().any(|p| matches!(p, Output::Tool { .. })) {
            "tool_calls"
        } else {
            "stop"
        }
    }
}

pub(super) fn decode_response(body: &Value, source: Protocol, model: &str) -> Result<Completion> {
    if !body["error"].is_null() {
        bail!("Upstream returned an error: {}", body["error"]);
    }
    let mut result = Completion::new(model);
    result.usage.read(&body["usage"], source);
    match source {
        Protocol::Chat => {
            let choice = body
                .pointer("/choices/0")
                .ok_or_else(|| anyhow::anyhow!("Upstream returned no choices"))?;
            let message = &choice["message"];
            if let Some(s) = message["content"].as_str() {
                result.output.push(Output::Text(s.into()));
            }
            for part in array(message, "content") {
                if text(part, "type") == "text" {
                    result.output.push(Output::Text(text(part, "text").into()));
                } else {
                    bail!("Unsupported upstream output content");
                }
            }
            if let Some(s) = message["refusal"].as_str() {
                result.output.push(Output::Text(s.into()));
            }
            for call in array(message, "tool_calls") {
                result.output.push(Output::Tool {
                    id: text(call, "id").into(),
                    name: text(&call["function"], "name").into(),
                    arguments: text(&call["function"], "arguments").into(),
                });
            }
            result.reason = text(choice, "finish_reason").into();
        }
        Protocol::Messages => {
            for block in array(body, "content") {
                match text(block, "type") {
                    "text" => result.output.push(Output::Text(text(block, "text").into())),
                    "tool_use" => result.output.push(Output::Tool {
                        id: text(block, "id").into(),
                        name: text(block, "name").into(),
                        arguments: block["input"].to_string(),
                    }),
                    "thinking" | "redacted_thinking" => {}
                    kind => bail!("Unsupported upstream content: {kind}"),
                }
            }
            result.reason = match text(body, "stop_reason") {
                "max_tokens" => "length",
                "refusal" => "content_filter",
                _ => "stop",
            }
            .into();
        }
        Protocol::Responses => {
            if text(body, "status") == "failed" {
                bail!("Upstream response failed");
            }
            for item in array(body, "output") {
                match text(item, "type") {
                    "message" => {
                        for part in array(item, "content") {
                            match text(part, "type") {
                                "output_text" => {
                                    result.output.push(Output::Text(text(part, "text").into()))
                                }
                                "refusal" => result
                                    .output
                                    .push(Output::Text(text(part, "refusal").into())),
                                kind => bail!("Unsupported upstream content: {kind}"),
                            }
                        }
                    }
                    "function_call" => result.output.push(Output::Tool {
                        id: text(item, "call_id").into(),
                        name: text(item, "name").into(),
                        arguments: text(item, "arguments").into(),
                    }),
                    "reasoning" => {}
                    kind => bail!("Unsupported upstream output: {kind}"),
                }
            }
            if text(body, "status") == "incomplete" {
                result.reason = if body.pointer("/incomplete_details/reason")
                    == Some(&json!("content_filter"))
                {
                    "content_filter"
                } else {
                    "length"
                }
                .into();
            }
        }
    }
    validate_tools(&result)?;
    Ok(result)
}
pub(super) fn validate_tools(result: &Completion) -> Result<()> {
    for part in &result.output {
        if let Output::Tool {
            id,
            name,
            arguments,
        } = part
        {
            if id.is_empty() || name.is_empty() {
                bail!("Upstream tool call is missing its ID or name");
            }
            serde_json::from_str::<Value>(arguments).map_err(|_| {
                anyhow::anyhow!("Upstream returned incomplete or invalid tool arguments")
            })?;
        }
    }
    Ok(())
}
pub fn convert_response(
    body: &Value,
    source: Protocol,
    target: Protocol,
    model: &str,
    map: &ToolMap,
) -> Result<Value> {
    decode_response(body, source, model)?.value(target, map, true)
}
