// Gemini CLI ingress translated to Chat Completions. Streaming responses are buffered
// as one SSE event so function arguments remain valid JSON.
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
pub fn request(model: &str, body: &Value) -> Result<Value> {
    if body.get("cachedContent").is_some() {return Err(anyhow!("Gemini cached content is not supported by this bridge"));}
    let mut messages=Vec::new();
    if let Some(parts)=body.pointer("/systemInstruction/parts").and_then(Value::as_array) {
        messages.push(json!({"role":"system","content":parts.iter().filter_map(|p|p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")}));
    }
    let mut calls:HashMap<String,VecDeque<String>>=HashMap::new();
    for (turn,content) in body.get("contents").and_then(Value::as_array).ok_or_else(||anyhow!("Missing Gemini contents"))?.iter().enumerate() {
        let role=if content.get("role").and_then(Value::as_str)==Some("model") {"assistant"}else{"user"};
        let mut parts=Vec::new(); let mut tools=Vec::new(); let mut results=Vec::new();
        for (i,p) in content.get("parts").and_then(Value::as_array).ok_or_else(||anyhow!("Missing Gemini parts"))?.iter().enumerate() {
            if let Some(text)=p.get("text") {parts.push(json!({"type":"text","text":text}));}
            else if let Some(call)=p.get("functionCall") {
                let name=call.get("name").and_then(Value::as_str).ok_or_else(||anyhow!("Invalid function call"))?;
                let id=call.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(||format!("call_{turn}_{i}"));
                calls.entry(name.into()).or_default().push_back(id.clone());
                tools.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":call.get("args").unwrap_or(&json!({})).to_string()}}));
            } else if let Some(result)=p.get("functionResponse") {
                let name=result.get("name").and_then(Value::as_str).ok_or_else(||anyhow!("Invalid function response"))?;
                let matched=calls.get_mut(name).and_then(VecDeque::pop_front);
                let id=result.get("id").and_then(Value::as_str).map(str::to_owned).or(matched).ok_or_else(||anyhow!("Unmatched function response"))?;
                results.push(json!({"role":"tool","tool_call_id":id,"content":result.get("response").unwrap_or(&Value::Null).to_string()}));
            } else if let Some(data)=p.get("inlineData") {
                let mime=data.get("mimeType").and_then(Value::as_str).unwrap_or("");
                if !mime.starts_with("image/"){return Err(anyhow!("Only inline images are supported by this bridge"));}
                parts.push(json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{}",data.get("data").and_then(Value::as_str).unwrap_or(""))}}));
            } else {return Err(anyhow!("Unsupported Gemini content part"));}
        }
        // Tool results must precede the next user message.
        messages.extend(results);
        if !parts.is_empty() || !tools.is_empty() {
            let mut message=json!({"role":role,"content":parts});
            if !tools.is_empty(){message["tool_calls"]=json!(tools);}
            messages.push(message);
        }
    }
    let mut output=json!({"model":model,"messages":messages,"stream":false});
    if let Some(tools)=body.get("tools").and_then(Value::as_array) {
        let mut functions=Vec::new();
        for tool in tools {
            let declarations=tool.get("functionDeclarations").and_then(Value::as_array).ok_or_else(||anyhow!("Only Gemini function declaration tools are supported"))?;
            for f in declarations {
                let mut function=f.clone();
                if let Some(schema)=function.get("parametersJsonSchema").cloned(){function["parameters"]=schema;}
                if let Some(obj)=function.as_object_mut(){obj.remove("parametersJsonSchema");}
                normalize_schema(&mut function);
                functions.push(json!({"type":"function","function":function}));
            }
        }
        if !functions.is_empty(){output["tools"]=json!(functions);}
    }
    if let Some(config)=body.get("generationConfig") {
        for (src,dst) in [("temperature","temperature"),("topP","top_p"),("maxOutputTokens","max_tokens"),("stopSequences","stop")] {
            if let Some(v)=config.get(src){output[dst]=v.clone();}
        }
        if config.get("responseMimeType").and_then(Value::as_str)==Some("application/json") {
            output["response_format"]=json!({"type":"json_object"});
        }
    }
    if let Some(mode)=body.pointer("/toolConfig/functionCallingConfig/mode").and_then(Value::as_str) {
        output["tool_choice"]=json!(match mode {"NONE"=>"none","ANY"=>"required",_=>"auto"});
    }
    Ok(output)
}
fn normalize_schema(v:&mut Value) {
    match v {
        Value::Object(o)=>{for (k,v) in o {if k=="type" {if let Some(s)=v.as_str(){*v=json!(s.to_lowercase());}} else {normalize_schema(v);}}},
        Value::Array(a)=>for v in a{normalize_schema(v);},
        _=>{}
    }
}
pub fn response(body: &Value) -> Result<Value> {
    let choice=body.pointer("/choices/0").ok_or_else(||anyhow!("Upstream returned no completion"))?;
    let message=&choice["message"];
    let mut parts=Vec::new();
    if let Some(text)=message.get("content").and_then(Value::as_str){parts.push(json!({"text":text}));}
    if let Some(calls)=message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let args:Value=serde_json::from_str(call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}"))?;
            parts.push(json!({"functionCall":{"id":call["id"],"name":call["function"]["name"],"args":args}}));
        }
    }
    let usage=&body["usage"];
    Ok(json!({"candidates":[{"index":0,"content":{"role":"model","parts":parts},
        "finishReason":match choice["finish_reason"].as_str(){Some("length")=>"MAX_TOKENS",Some("content_filter")=>"SAFETY",_=>"STOP"}}],
        "usageMetadata":{"promptTokenCount":usage["prompt_tokens"],"candidatesTokenCount":usage["completion_tokens"],"totalTokenCount":usage["total_tokens"]}}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn roundtrip_tools() {
        let body=json!({"contents":[{"role":"model","parts":[{"functionCall":{"name":"ls","args":{}}}]},
            {"role":"user","parts":[{"functionResponse":{"name":"ls","response":{"files":[]}}}]}]});
        let mapped=request("autojev/test",&body).unwrap();
        assert_eq!(mapped["messages"][0]["tool_calls"][0]["id"],mapped["messages"][1]["tool_call_id"]);
        let reply=response(&json!({"choices":[{"message":{"tool_calls":[{"id":"call1","function":{"name":"ls","arguments":"{}"}}]},"finish_reason":"tool_calls"}]})).unwrap();
        assert_eq!(reply["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],"ls");
    }
    #[test] fn rejects_unsupported_parts() {
        assert!(request("x",&json!({"contents":[{"parts":[{"fileData":{"fileUri":"gs://secret"}}]}]})).is_err());
    }
}
