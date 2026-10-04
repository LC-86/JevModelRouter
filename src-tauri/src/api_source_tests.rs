//! R2 public persistence and listening-gateway acceptance; fictional keys only.
use crate::config::{AppConfig, ConfigStore};
use axum::{
    extract::OriginalUri,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

pub(crate) fn finite_tool_response(mode: &str) -> axum::response::Response {
    let mut call=json!({"index":0,"id":"call_echo","type":"function","function":{"name":"echo","arguments":"{}"}});
    match mode {
        "invalid-single"|"invalid-split"=>call["function"]["arguments"]="{".into(),
        "missing-id"=>{call.as_object_mut().unwrap().remove("id");},
        "missing-name"=>{call["function"].as_object_mut().unwrap().remove("name");},
        "invalid-type"=>call["type"]="custom".into(),
        "valid-single"=>{},
        "valid-split"=>{call["id"]="call_".into();call["function"]["name"]="ec".into();call["function"]["arguments"]="{\"n\":".into();},
        _=>panic!("Unknown fictional response mode"),
    }
    let mut text=format!("data: {}\n\n",json!({"choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[call]},"finish_reason":null}]}));
    if mode=="valid-split" {text+=&format!("data: {}\n\n",json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"echo","function":{"name":"ho","arguments":"1}"}}]},"finish_reason":null}]}));}
    text+=&format!("data: {}\n\ndata: [DONE]\n\n",json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}));
    let size=if mode.ends_with("split"){17}else{text.len()};
    let chunks:std::collections::VecDeque<_>=text.as_bytes().chunks(size).map(axum::body::Bytes::copy_from_slice).collect();
    let stream=futures_util::stream::unfold(chunks,|mut chunks|async move {let chunk=chunks.pop_front()?;tokio::time::sleep(std::time::Duration::from_millis(2)).await;Some((Ok::<_,std::io::Error>(chunk),chunks))});
    ([("content-type","text/event-stream")],axum::body::Body::from_stream(stream)).into_response()
}

fn review_response_event(kind: &str, mut value: serde_json::Value) -> String {
    value["type"] = kind.into();
    format!("event: {kind}\ndata: {value}\n\n")
}

fn review_legacy_function_stream(arguments: &[String; 2]) -> String {
    let mut text = format!("data: {}\n\n", json!({"id":"legacy-fixture","choices":[
        {"index":1,"delta":{"role":"assistant","function_call":{"name":"echo","arguments":""}},"finish_reason":null},
        {"index":0,"delta":{"role":"assistant","function_call":{"name":"echo","arguments":""}},"finish_reason":null}
    ]}));
    let split = arguments[0].find("coding").unwrap();
    for (index, piece) in [(0, &arguments[0][..split]), (1, &arguments[1][..10]), (0, &arguments[0][split..]), (1, &arguments[1][10..])] {
        text.push_str(&format!("data: {}\n\n", json!({"id":"legacy-fixture","choices":[{"index":index,"delta":{"function_call":{"arguments":piece}},"finish_reason":null}]})));
    }
    text.push_str(&format!("data: {}\n\ndata: [DONE]\n\n", json!({"id":"legacy-fixture","choices":[{"index":0,"delta":{},"finish_reason":"function_call"},{"index":1,"delta":{},"finish_reason":"function_call"}]})));
    text
}

fn review_custom_input_stream(key: &str) -> String {
    let mut text = String::new();
    let mut items: Vec<_> = (0..2).map(|index| json!({"type":"custom_tool_call","id":format!("ctc_{index}"),"call_id":format!("call_{index}"),"name":"echo","input":"","status":"in_progress"})).collect();
    text.push_str(&review_response_event("response.created", json!({"response":{"id":"fixture","status":"in_progress","output":[]}})));
    for (index, item) in items.iter().enumerate() {
        text.push_str(&review_response_event("response.output_item.added", json!({"output_index":index,"item":item})));
    }
    let split = key.find("coding").unwrap();
    for (index, delta) in [(0, format!("你好 {}", &key[..split])), (1, "other raw input".into()), (0, format!("{} 完成", &key[split..])), (1, " retained".into())] {
        text.push_str(&review_response_event("response.custom_tool_call_input.delta", json!({"output_index":index,"item_id":format!("ctc_{index}"),"delta":delta})));
    }
    for (index, item) in items.iter_mut().enumerate() {
        item["input"] = if index == 0 { format!("你好 {key} 完成") } else { "other raw input retained".into() }.into();
        item["status"] = "completed".into();
        text.push_str(&review_response_event("response.custom_tool_call_input.done", json!({"output_index":index,"item_id":item["id"],"input":item["input"]})));
        text.push_str(&review_response_event("response.output_item.done", json!({"output_index":index,"item":item})));
    }
    text.push_str(&review_response_event("response.completed", json!({"response":{"id":"fixture","status":"completed","output":items}})));
    text
}

fn review_distinct_field_stream(mode: &str, key: &str) -> String {
    let (first, other) = match mode {
        "chat-content" => ("content", "refusal"),
        "chat-refusal" => ("refusal", "content"),
        "chat-reasoning_content" => ("reasoning_content", "reasoning"),
        "chat-reasoning" => ("reasoning", "reasoning_content"),
        "responses-text" => ("response.output_text.delta", "response.refusal.delta"),
        "responses-refusal" => ("response.refusal.delta", "response.output_text.delta"),
        _ => panic!("Unknown directed field fixture"),
    };
    let split = key.find("coding").unwrap();
    let mut text = String::new();
    let pieces = [(first, format!("first: {}", &key[..split])), (other, "other ".into()),
        (first, format!("{} done; ", &key[split..])), (first, key[..split].into()), (other, key[split..].into())];
    for (sequence, (field, delta)) in pieces.into_iter().enumerate() {
        if mode.starts_with("chat-") {
            let mut value = json!({}); value[field] = delta.into();
            text.push_str(&format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":value,"finish_reason":null}]})));
        } else {
            text.push_str(&review_response_event(field, json!({"sequence_number":sequence,"item_id":"msg_fields","output_index":0,"content_index":0,"delta":delta})));
        }
    }
    if mode.starts_with("chat-") {
        text.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n");
    } else {
        text.push_str(&review_response_event("response.completed", json!({"sequence_number":5,"response":{"id":"fixture-fields","status":"completed","output":[]}})));
    }
    text
}

fn review_choice_stream(field: &str, key: &str) -> String {
    let mut choices = Vec::new();
    if field == "tools" {
        choices.push(json!([
            {"index":1,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"echo","arguments":""}}]},"finish_reason":null},
            {"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_0","type":"function","function":{"name":"echo","arguments":""}}]},"finish_reason":null}
        ]));
        let arguments = [format!("{{\"note\":\"{key}\"}}"), r#"{"note":"other value"}"#.into()];
        let split = arguments[0].find("coding").unwrap();
        for (index, piece) in [(0, &arguments[0][..split]), (1, &arguments[1][..10]), (0, &arguments[0][split..]), (1, &arguments[1][10..])] {
            choices.push(json!([{"index":index,"delta":{"tool_calls":[{"index":0,"function":{"arguments":piece}}]},"finish_reason":null}]));
        }
    } else {
        let split = key.find("coding").unwrap();
        for (index, piece) in [(0, format!("你好 {}", &key[..split])), (1, "other choice".into()), (0, format!("{} 完成", &key[split..])), (1, " retained".into())] {
            let mut delta = json!({});
            delta[field] = piece.into();
            choices.push(json!([{"index":index,"delta":delta,"finish_reason":null}]));
        }
    }
    let reason = if field == "tools" { "tool_calls" } else { "stop" };
    choices.push(json!([{"index":1,"delta":{},"finish_reason":reason},{"index":0,"delta":{},"finish_reason":reason}]));
    let mut text = String::new();
    for choices in choices {
        text.push_str(&format!("data: {}\n\n", json!({"id":"review-choices","object":"chat.completion.chunk","created":0,"model":"same-model","choices":choices})));
    }
    text.push_str("data: [DONE]\n\n");
    text
}

fn review_indexed_stream(mode: &str) -> String {
    let mut text = String::new();
    if mode.starts_with("messages-") {
        let field = if mode == "messages-text" { "text" } else { "thinking" };
        text.push_str(&review_response_event("message_start", json!({"message":{"id":"fixture","role":"assistant","content":[],"usage":{}}})));
        for index in 0..2 {
            let (initial, delta) = if field == "thinking" {
                if index == 0 { ("fictional-", "coding") } else { ("other", " retained") }
            } else if index == 0 { ("fictional", "-") } else { ("coding", " 完成") };
            let mut block = json!({"type":field}); block[field] = initial.into();
            text.push_str(&review_response_event("content_block_start", json!({"index":index,"content_block":block})));
            let mut value = json!({"type":format!("{field}_delta")}); value[field] = delta.into();
            text.push_str(&review_response_event("content_block_delta", json!({"index":index,"delta":value})));
            if field == "thinking" { text.push_str(&review_response_event("content_block_delta", json!({"index":index,"delta":{"type":"signature_delta","signature":"fixture-signature"}}))); }
            text.push_str(&review_response_event("content_block_stop", json!({"index":index})));
        }
        text.push_str(&review_response_event("message_delta", json!({"delta":{"stop_reason":"end_turn"},"usage":{}})));
        text.push_str(&review_response_event("message_stop", json!({})));
    } else {
        let kind = if mode.starts_with("text") { "response.output_text.delta" } else if mode.starts_with("refusal") { "response.refusal.delta" } else if mode == "reasoning-text" { "response.reasoning_text.delta" } else { "response.reasoning_summary_text.delta" };
        let mut output = Vec::new();
        text.push_str(&review_response_event("response.created", json!({"response":{"id":"fixture","status":"in_progress","output":[]}})));
        for index in 0..2 {
            let output_index = if mode.ends_with("items") { index } else { 0 };
            let part_index = if mode.ends_with("items") { 0 } else { index };
            if mode.ends_with("items") || index == 0 {
                let item = if mode.starts_with("reasoning") { json!({"id":format!("item_{output_index}"),"type":"reasoning","summary":[]}) } else { json!({"id":format!("item_{output_index}"),"type":"message","role":"assistant","content":[],"status":"in_progress"}) };
                text.push_str(&review_response_event("response.output_item.added", json!({"output_index":output_index,"item":item})));
                output.push(item);
            }
            let delta = if index == 0 { "fictional-" } else { "coding 完成" };
            let mut value = json!({"item_id":format!("item_{output_index}"),"output_index":output_index,"delta":delta});
            value[if mode == "reasoning-summary" { "summary_index" } else { "content_index" }] = part_index.into();
            if mode.starts_with("text") || mode.starts_with("refusal") {
                let part = if mode.starts_with("text") { json!({"type":"output_text","text":"","annotations":[]}) } else { json!({"type":"refusal","refusal":""}) };
                text.push_str(&review_response_event("response.content_part.added", json!({"output_index":output_index,"content_index":part_index,"item_id":value["item_id"],"part":part})));
            }
            text.push_str(&review_response_event(kind, value));
            if mode.starts_with("text") || mode.starts_with("refusal") {
                let part = if mode.starts_with("text") { json!({"type":"output_text","text":delta,"annotations":[]}) } else { json!({"type":"refusal","refusal":delta}) };
                output[output_index]["content"].as_array_mut().unwrap().push(part.clone());
                let field = if mode.starts_with("text") { "text" } else { "refusal" };
                let mut done = json!({"output_index":output_index,"content_index":part_index,"item_id":format!("item_{output_index}")});
                done[field] = delta.into();
                text.push_str(&review_response_event(&kind.replace(".delta", ".done"), done));
                text.push_str(&review_response_event("response.content_part.done", json!({"output_index":output_index,"content_index":part_index,"item_id":format!("item_{output_index}"),"part":part})));
            } else {
                let field = if mode == "reasoning-summary" { "summary" } else { "content" };
                if output[output_index][field].is_null() { output[output_index][field] = json!([]); }
                output[output_index][field].as_array_mut().unwrap().push(json!({"type":if field == "summary" { "summary_text" } else { "reasoning_text" },"text":delta}));
            }
        }
        for (index, item) in output.iter_mut().enumerate() {
            if item["type"] == "message" { item["status"] = "completed".into(); }
            text.push_str(&review_response_event("response.output_item.done", json!({"output_index":index,"item":item})));
        }
        text.push_str(&review_response_event("response.completed", json!({"response":{"id":"fixture","status":"completed","output":output}})));
    }
    text
}

async fn source_fixture() -> (
    tempfile::TempDir,
    Arc<ConfigStore>,
    Arc<Mutex<Vec<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let records = received.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(post(move |uri: OriginalUri, headers: HeaderMap, Json(body): Json<serde_json::Value>| {
            let records = records.clone();
            async move {
                let path = uri.0.path().to_string();
                records.lock().unwrap().push(json!({"path":path,"authorization":headers.get("authorization").and_then(|v|v.to_str().ok()),"model":body["model"],"n":body["n"]}));
                if serde_json::to_string(&body).unwrap().contains("review-wire-upper") || serde_json::to_string(&body).unwrap().contains("review-wire-cr") {
                    let upper = serde_json::to_string(&body).unwrap().contains("review-wire-upper");
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let split = key.find("coding").unwrap();
                    let mut chunks = std::collections::VecDeque::new();
                    for content in ["early", &key[..split], &key[split..]] {
                        chunks.push_back(format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"content":content},"finish_reason":null}]})));
                    }
                    chunks.push_back("data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into());
                    if !upper { for chunk in &mut chunks { *chunk = chunk.replace('\n', "\r"); } }
                    let stream = futures_util::stream::unfold((chunks, 0), |(mut chunks, index)| async move {
                        let chunk = chunks.pop_front()?;
                        if index == 1 { tokio::time::sleep(std::time::Duration::from_secs(2)).await; }
                        Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(chunk)), (chunks, index + 1)))
                    });
                    return axum::response::Response::builder().header("content-type", if upper { "Text/Event-Stream; charset=utf-8" } else { "text/event-stream" }).body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if body.pointer("/messages/0/content").and_then(|v| v.as_str()) == Some("review-legacy-function") {
                    assert_eq!(body["functions"][0]["name"], "echo");
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let arguments = [json!({"note":key}).to_string(), json!({"note":"other value"}).to_string()];
                    if body["stream"] != true {
                        return Json(json!({"id":"legacy-fixture","choices":arguments.iter().enumerate().map(|(index, arguments)| json!({"index":index,"message":{"role":"assistant","content":null,"function_call":{"name":"echo","arguments":arguments}},"finish_reason":"function_call"})).collect::<Vec<_>>()})).into_response();
                    }
                    let text = review_legacy_function_stream(&arguments);
                    let chunks: std::collections::VecDeque<_> = text.as_bytes().chunks(71).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move { let chunk = chunks.pop_front()?; tokio::task::yield_now().await; Some((Ok::<_, std::io::Error>(chunk), chunks)) });
                    return axum::response::Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if let Some(terminal) = body["input"].as_str().or_else(|| body.pointer("/messages/0/content").and_then(|v| v.as_str())).and_then(|v| v.strip_prefix("review-event-meta-")) {
                    let key = headers.get("authorization").or_else(|| headers.get("x-api-key")).unwrap().to_str().unwrap().trim_start_matches("Bearer ");
                    let split = key.find("coding").unwrap();
                    let mut text = "event: ping\ndata: {\"sequence\":1}\n\nevent: ping\ndata: {\"type\":\"provider.original\",\"sequence\":2}\n\nevent: ping\ndata: {\"type\":null,\"sequence\":3}\n\n".to_owned();
                    for delta in [format!("first: {}", &key[..split]), format!("{} done; ", &key[split..]), key[..split].to_owned()] {
                        let (event, value) = if terminal == "message_stop" {
                            ("content_block_delta", json!({"index":0,"delta":{"type":"text_delta","text":delta}}))
                        } else { ("response.output_text.delta", json!({"output_index":0,"content_index":0,"delta":delta})) };
                        text.push_str(&format!("event: {event}\ndata: {value}\n\n"));
                    }
                    text.push_str(&format!("event: {terminal}\ndata: {{}}\n\n"));
                    let stream = futures_util::stream::unfold(Some(text), |text| async move {
                        if let Some(text) = text { Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(text)), None)) }
                        else { tokio::time::sleep(std::time::Duration::from_secs(2)).await; None }
                    });
                    return axum::response::Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if let Some(mode) = body["input"].as_str().or_else(|| body.pointer("/messages/0/content").and_then(|v| v.as_str())).and_then(|v| v.strip_prefix("review-field-")) {
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    return ([("content-type", "text/event-stream")], review_distinct_field_stream(mode, key)).into_response();
                }
                if let Some(mode) = body["input"].as_str().or_else(|| body.pointer("/messages/0/content").and_then(|v| v.as_str())).and_then(|v| v.strip_prefix("review-indexed-")) {
                    return ([("content-type", "text/event-stream")], review_indexed_stream(mode)).into_response();
                }
                if body["input"] == "review-custom-input" {
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let text = review_custom_input_stream(key);
                    let chunks: std::collections::VecDeque<_> = text.as_bytes().chunks(71).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move { let chunk = chunks.pop_front()?; tokio::task::yield_now().await; Some((Ok::<_, std::io::Error>(chunk), chunks)) });
                    return axum::response::Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if let Some(field) = body.pointer("/messages/0/content").and_then(|v| v.as_str()).and_then(|v| v.strip_prefix("review-choice-")) {
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let text = review_choice_stream(field, key);
                    let chunks: std::collections::VecDeque<_> = text.as_bytes().chunks(71).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move { let chunk = chunks.pop_front()?; tokio::task::yield_now().await; Some((Ok::<_, std::io::Error>(chunk), chunks)) });
                    return axum::response::Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if let Some(style) = body.pointer("/messages/0/content").and_then(|v| v.as_str()).and_then(|v| v.strip_prefix("review-multiline-")) {
                    let text = ": keepalive\nevent: provider.extension\nid: extension-1\nretry: 1000\ndata: line one\ndata:\ndata: 第二行\ndata:\nprovider-extension: retained\n\ndata:\n\ndata: [DONE]\n\n";
                    return ([("content-type", "text/event-stream")], match style { "crlf" => text.replace('\n', "\r\n"), "cr" => text.replace('\n', "\r"), _ => text.into() }).into_response();
                }
                if body.pointer("/messages/0/content").and_then(|v| v.as_str()) == Some("split-secret") {
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let text = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"你好 {key} {key}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n");
                    let chunks: std::collections::VecDeque<_> = text.as_bytes().chunks(3).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move {
                        let chunk = chunks.pop_front()?;
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                        Some((Ok::<_, std::io::Error>(chunk), chunks))
                    });
                    return axum::response::Response::builder().header("content-type", "text/event-stream")
                        .header("x-request-id", key).body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if let Some(mode)=body.pointer("/messages/0/content").and_then(|v|v.as_str()).and_then(|v|v.strip_prefix("finite-tool-")) {
                    return finite_tool_response(mode);
                }
                if let Some(mode)=body.pointer("/messages/0/content").and_then(|v|v.as_str()).and_then(|v|v.strip_prefix("plan-stream-")) {
                    let mode=mode.to_owned();
                    let stream=futures_util::stream::unfold((mode,0),|(mode,n)|async move {
                        if n==0 {return Some((Ok::<_,std::io::Error>(axum::body::Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"fictional\"}}]}\n\n")),(mode,1)));}
                        if mode=="cancel" {tokio::time::sleep(std::time::Duration::from_secs(3)).await;}
                        if n==1 && mode=="done" {return Some((Ok(axum::body::Bytes::from("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")),(mode,2)));}
                        None
                    });
                    return axum::response::Response::builder().header("content-type","text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if let Some(status) = body.pointer("/messages/0/content").and_then(|v|v.as_str()).and_then(|v| v.strip_prefix("fail-")).and_then(|v| v.parse::<u16>().ok()) {
                    return (StatusCode::from_u16(status).unwrap(), Json(json!({"error":{"message":headers["authorization"].to_str().unwrap()}}))).into_response();
                }
                if serde_json::to_string(&body).unwrap().contains("escaped-secret") {
                    let key = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ");
                    let escaped = format!("\\u0066{}", &key[1..]);
                    if body["stream"] == true {
                        return ([("content-type", "text/event-stream")], format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{escaped}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n")).into_response();
                    }
                    return ([("content-type", "application/json")], format!("{{\"error\":{{\"message\":\"{escaped}\"}}}}")).into_response();
                }
                if serde_json::to_string(&body).unwrap().contains("delta-secret") {
                    let key = headers.get("authorization").or_else(|| headers.get("x-api-key")).unwrap().to_str().unwrap().trim_start_matches("Bearer ");
                    let (mut text, terminal) = if path.ends_with("/messages") {
                        ("event: message_start\ndata: {\"message\":{\"usage\":{}}}\n\nevent: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_string(), "event: content_block_stop\ndata: {\"index\":0}\n\nevent: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {}\n\n")
                    } else if path.ends_with("/responses") {
                        (String::new(), "event: response.completed\ndata: {\"response\":{\"id\":\"fixture\",\"status\":\"completed\",\"output\":[]}}\n\n")
                    } else {
                        (String::new(), "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                    };
                    for piece in key.chars().map(|c| c.to_string()) {
                        let frame = if path.ends_with("/messages") { format!("event: content_block_delta\ndata: {}\n\n", json!({"index":0,"delta":{"type":"text_delta","text":piece}})) }
                        else if path.ends_with("/responses") { format!("event: response.output_text.delta\ndata: {}\n\n", json!({"output_index":0,"content_index":0,"delta":piece})) }
                        else { format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"content":piece},"finish_reason":null}]})) };
                        text.push_str(&frame);
                    }
                    text.push_str(terminal);
                    let chunks: std::collections::VecDeque<_> = text.as_bytes().chunks(71).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move { let chunk = chunks.pop_front()?; tokio::task::yield_now().await; Some((Ok::<_, std::io::Error>(chunk), chunks)) });
                    return axum::response::Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if serde_json::to_string(&body).unwrap().contains("nested-secret") {
                    let arguments = r#"{"note":"\u0066ictional-coding"}"#;
                    if path.ends_with("/messages") {
                        if body["stream"] == true {
                            let mut text = "event: message_start\ndata: {\"message\":{\"usage\":{}}}\n\nevent: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_fixture\",\"name\":\"echo\",\"input\":{}}}\n\n".to_string();
                            for piece in arguments.chars().map(|c| c.to_string()) { text.push_str(&format!("event: content_block_delta\ndata: {}\n\n", json!({"index":0,"delta":{"type":"input_json_delta","partial_json":piece}}))); }
                            text.push_str("event: content_block_stop\ndata: {\"index\":0}\n\nevent: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}\n\nevent: message_stop\ndata: {}\n\n");
                            return ([("content-type", "text/event-stream")], text).into_response();
                        }
                        return Json(json!({"id":"fixture","content":[{"type":"tool_use","id":"call_fixture","name":"echo","input":serde_json::from_str::<serde_json::Value>(arguments).unwrap()}],"stop_reason":"tool_use"})).into_response();
                    }
                    if path.ends_with("/responses") {
                        let item = json!({"type":"function_call","call_id":"call_fixture","name":"echo","arguments":arguments});
                        let response = json!({"id":"fixture","status":"completed","output":[item]});
                        if body["stream"] == true {
                            let mut text = format!("event: response.output_item.added\ndata: {}\n\n", json!({"output_index":0,"item":{"type":"function_call","call_id":"call_fixture","name":"echo","arguments":""}}));
                            for piece in arguments.chars().map(|c| c.to_string()) { text.push_str(&format!("event: response.function_call_arguments.delta\ndata: {}\n\n", json!({"output_index":0,"delta":piece}))); }
                            text.push_str(&format!("event: response.completed\ndata: {}\n\n", json!({"response":response})));
                            return ([("content-type", "text/event-stream")], text).into_response();
                        }
                        return Json(response).into_response();
                    }
                    if body["stream"] == true {
                        let mut text = format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_fixture","type":"function","function":{"name":"echo","arguments":""}}]},"finish_reason":null}]}));
                        for piece in arguments.chars().map(|c| c.to_string()) {
                            text.push_str(&format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":piece}}]},"finish_reason":null}]})));
                        }
                        text.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n");
                        return ([("content-type", "text/event-stream")], text).into_response();
                    }
                    return Json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_fixture","type":"function","function":{"name":"echo","arguments":arguments}}]},"finish_reason":"tool_calls"}]})).into_response();
                }
                if serde_json::to_string(&body).unwrap().contains("inflight-secret") { tokio::time::sleep(std::time::Duration::from_millis(150)).await; }
                if serde_json::to_string(&body).unwrap().contains("slow-json") {
                    let bytes = serde_json::to_vec(&json!({"choices":[{"message":{"role":"assistant","content":"fictional slow response"},"finish_reason":"stop"}]})).unwrap();
                    let chunks: std::collections::VecDeque<_> = bytes.chunks(16).map(axum::body::Bytes::copy_from_slice).collect();
                    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move {
                        let chunk = chunks.pop_front()?;
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        Some((Ok::<_, std::io::Error>(chunk), chunks))
                    });
                    return axum::response::Response::builder().header("content-type", "application/json").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                if !["/official/v1/chat/completions","/third/api/v1/chat/completions","/coding/v4/chat/completions"].contains(&path.as_str()) {
                    return (StatusCode::NOT_FOUND, Json(json!({"error":{"message":"unsupported endpoint"}}))).into_response();
                }
                Json(json!({"id":"fixture","choices":[{"message":{"role":"assistant","content":path},"finish_reason":"stop"}]})).into_response()
            }
        }))).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::load(root.path().join("sources.db")).unwrap());
    store.update(|config| {
        let original_provider = config.providers[0].clone();
        let original_model = config.models[0].clone();
        config.providers.clear(); config.models.clear(); config.port = 0;
        config.gateway.proxy_mode = "direct".into();
        config.gateway.failure_threshold = 20;
        config.policy.use_jev_when_ambiguous = true;
        // The coding-labelled fixture now tests compatibility/secret handling only;
        // actual Coding Plan admission has a separate fail-closed test below.
        for (id, kind, endpoint) in [("official","official_api","/official/v1"),("third","third_party_api","/third/api/v1"),("coding","third_party_api","/coding/v4")] {
            let mut provider = original_provider.clone();
            provider.id = id.into(); provider.name = id.into(); provider.base_url = format!("http://{address}{endpoint}"); provider.api_type = "chat_completions".into();
            let mut model = original_model.clone(); model.id = format!("uuid-{id}"); model.provider_id = id.into(); model.model_id = "same-model".into();
            config.api_sources.insert(id.into(), serde_json::from_value(json!({"connection_instance_id":format!("instance-{id}"),"generation":1,"kind":kind,"endpoint":provider.base_url,"api_type":"chat_completions","credential_reference":format!("api-generation:instance-{id}"),"model_bindings":std::collections::HashMap::from([(model.id.clone(), model.model_id.clone())]),"account_label":null,"plan_label":null})).unwrap());
            config.providers.push(provider); config.models.push(model);
        }
    }).unwrap();
    for id in ["official", "third", "coding"] {
        store
            .write_secret(
                &format!("api-generation:instance-{id}"),
                &format!("fictional-{id}"),
            )
            .unwrap();
    }
    (root, store, received, task)
}

#[tokio::test]
async fn api_sources_same_model_hits_only_the_explicit_endpoint_and_generation_key() {
    let (_root, store, received, upstream) = source_fixture().await;
    // A decision key exists but must never be used for generation.
    store
        .write_secret("autojev-cloud", "fictional-decision-only")
        .unwrap();
    let gateway = crate::proxy::start(store).await.unwrap();
    for id in ["official", "third", "coding"] {
        // The pre-R4 provider/model reference remains usable through the same fixed UUID and admission path.
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
            .json(&json!({"model":format!("{id}/same-model"),"messages":[{"role":"user","content":"fictional request"}]})).send().await.unwrap();
        assert_eq!(reply.status(), 200, "{id}: {}", reply.text().await.unwrap());
    }
    let records = received.lock().unwrap().clone();
    assert_eq!(records.len(), 3);
    for (record, id) in records.iter().zip(["official", "third", "coding"]) {
        assert_eq!(record["authorization"], format!("Bearer fictional-{id}"));
        assert_eq!(record["model"], "same-model");
    }
    gateway.stop().await;
    upstream.abort();
}

#[test]
fn api_source_identity_survives_reopen_without_projecting_a_secret() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sources.db");
    let store = ConfigStore::load(path.clone()).unwrap();
    let mut value = serde_json::to_value(store.read()).unwrap();
    value["api_sources"] = json!({"openrouter": {
        "connection_instance_id":"fixture-connection", "generation":1,
        "kind":"third_party_api", "endpoint":"https://example.invalid/api/v1",
        "api_type":"chat_completions", "credential_reference":"api-generation:fixture-connection",
        "account_label":null, "plan_label":null,
        "account_state":"unknown", "plan_state":"unknown"
    }});
    store
        .update(|config| *config = serde_json::from_value::<AppConfig>(value).unwrap())
        .unwrap();
    store
        .write_secret(
            "api-generation:fixture-connection",
            "fictional-generation-key",
        )
        .unwrap();
    drop(store);
    let reopened = ConfigStore::load(path).unwrap();
    let projection = serde_json::to_value(reopened.read()).unwrap();
    assert_eq!(
        projection["api_sources"]["openrouter"]["connection_instance_id"],
        "fixture-connection"
    );
    assert_eq!(
        projection["api_sources"]["openrouter"]["plan_state"],
        "unknown"
    );
    assert!(!projection.to_string().contains("fictional-generation-key"));
}

#[tokio::test]
async fn api_source_route_failure_never_retries_another_paid_target() {
    let mut failures = Vec::new();
    for strategy in ["round_robin", "jev"] {
        for status in [429, 500, 503] {
            let (_root, store, received, upstream) = source_fixture().await;
            let route_id = format!("review-{}", uuid::Uuid::new_v4().simple());
            store.update(|config| {
                config.policy.use_jev_when_ambiguous = false;
                config.gateway.max_attempts = 3;
                config.routes = vec![serde_json::from_value(json!({
                    "id":route_id,"name":"Fictional review route","strategy":strategy,
                    "enabled":true,"all_models":false,"model_ids":config.models.iter().map(|m|m.id.clone()).collect::<Vec<_>>(),
                    "model_settings":{},"automatic_policy":null
                })).unwrap()];
            }).unwrap();
            let gateway = crate::proxy::start(store).await.unwrap();
            let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
                .json(&json!({"model":format!("autojev/{route_id}"),"messages":[{"role":"user","content":format!("fail-{status}")}]})).send().await.unwrap();
            let actual_status = reply.status().as_u16();
            let body = reply.text().await.unwrap();
            let records = received.lock().unwrap().clone();
            if actual_status != status || records.len() != 1 {
                failures.push(format!(
                    "{strategy}/{status}: response={actual_status}, targets={:?}",
                    records
                        .iter()
                        .map(|r| r["path"].clone())
                        .collect::<Vec<_>>()
                ));
            }
            assert!(!body.contains("fictional-"));
            gateway.stop().await;
            upstream.abort();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn api_source_changed_endpoint_is_rejected_without_any_upstream_request() {
    let (_root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| config.providers[2].base_url = config.providers[0].base_url.clone())
        .unwrap();
    let gateway = crate::proxy::start(store).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"fixture"}]})).send().await.unwrap();
    assert!(!reply.status().is_success());
    assert!(reply
        .text()
        .await
        .unwrap()
        .contains("source_target_changed"));
    assert!(received.lock().unwrap().is_empty());
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_upstream_failure_never_projects_its_generation_secret() {
    let (_root, store, received, upstream) = source_fixture().await;
    for status in [200, 401, 429, 500] {
        // Each error starts with a healthy fixed target; 401 otherwise cools down the next request.
        let gateway = crate::proxy::start(store.clone()).await.unwrap();
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
            .json(&json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":format!("fail-{status}")}]})).send().await.unwrap();
        assert_eq!(
            reply.status().as_u16(),
            if status == 200 { 502 } else { status }
        );
        let body = reply.text().await.unwrap();
        assert!(!body.contains("fictional-coding"));
        assert!(body.contains("[REDACTED]"));
        gateway.stop().await;
    }
    assert_eq!(received.lock().unwrap().len(), 4);
    assert!(received
        .lock()
        .unwrap()
        .iter()
        .all(|record| record["path"] == "/coding/v4/chat/completions"));
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    upstream.abort();
}

#[tokio::test]
async fn api_sources_all_refused_targets_dispatch_zero_to_every_other_source() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let original = store.read();
    for id in ["official", "third", "coding"] {
        for condition in ["missing", "disabled", "removed", "protocol"] {
            store
                .update(|config| {
                    *config = original.clone();
                    if condition == "disabled" {
                        config
                            .providers
                            .iter_mut()
                            .find(|p| p.id == id)
                            .unwrap()
                            .enabled = false;
                    }
                    if condition == "removed" {
                        config.models.retain(|m| m.provider_id != id);
                    }
                    if condition == "protocol" {
                        config
                            .models
                            .iter_mut()
                            .find(|m| m.provider_id == id)
                            .unwrap()
                            .api_type = "responses".into();
                    }
                })
                .unwrap();
            if condition == "missing" {
                store
                    .delete_secret(&format!("api-generation:instance-{id}"))
                    .unwrap();
            }
            // A legacy valid alias still resolves to its old fixed source target and cannot bypass denial.
            let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
                .json(&json!({"model":format!("{id}/same-model"),"messages":[{"role":"user","content":"fixture"}]})).send().await.unwrap();
            assert!(!reply.status().is_success(), "{id}/{condition}");
            let body = reply.text().await.unwrap();
            assert!(
                body.contains(match condition {
                    "missing" => "source_credential_missing",
                    "disabled" => "source_disabled",
                    "removed" => "source_model_retired",
                    _ => "source_protocol_unsupported",
                }),
                "{id}/{condition}: {body}"
            );
            assert!(
                received.lock().unwrap().is_empty(),
                "{id}/{condition} dispatched"
            );
            store
                .write_secret(
                    &format!("api-generation:instance-{id}"),
                    &format!("fictional-{id}"),
                )
                .unwrap();
        }
    }
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_same_protocol_legacy_functions_preserve_choice_arguments() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store).await.unwrap();
    let mut failures = Vec::new();
    for stream in [false, true] {
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
            .json(&json!({"model":"autojev/model/uuid-coding","stream":stream,"n":2,"messages":[{"role":"user","content":"review-legacy-function"}],"functions":[{"name":"echo","parameters":{"type":"object","properties":{"note":{"type":"string"}}}}],"function_call":"auto"})).send().await.unwrap();
        assert_eq!(reply.status(), 200);
        let bytes = reply.bytes().await.unwrap();
        let mut arguments = [String::new(), String::new()];
        let mut names = [String::new(), String::new()];
        let mut finished = [false; 2];
        let values: Vec<serde_json::Value> = if stream {
            let events = crate::protocol::SseParser::default().push(&bytes).unwrap();
            assert_eq!(events.last().unwrap().1, "[DONE]");
            events.into_iter().filter(|(_, data)| data != "[DONE]").map(|(_, data)| serde_json::from_str(&data).unwrap()).collect()
        } else { vec![serde_json::from_slice(&bytes).unwrap()] };
        for value in values {
            for choice in value["choices"].as_array().unwrap() {
                let index = choice["index"].as_u64().unwrap() as usize;
                let call = &choice[if stream { "delta" } else { "message" }]["function_call"];
                if let Some(name) = call["name"].as_str() { names[index] = name.into(); }
                if let Some(piece) = call["arguments"].as_str() { arguments[index].push_str(piece); }
                finished[index] |= choice["finish_reason"] == "function_call";
            }
        }
        assert_eq!(names, ["echo", "echo"]);
        assert_eq!(finished, [true, true]);
        for (index, text) in arguments.iter().enumerate() {
            let input: serde_json::Value = serde_json::from_str(text).unwrap();
            let expected = if index == 0 { "[REDACTED]" } else { "other value" };
            if input["note"] != expected { failures.push(format!("stream={stream}/choice={index}: {input}")); }
        }
    }
    assert_eq!(received.lock().unwrap().len(), 2);
    assert!(received.lock().unwrap().iter().all(|r| r["path"] == "/coding/v4/chat/completions" && r["n"] == 2));
    gateway.stop().await;
    upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn api_source_existing_delta_fields_preserve_block_item_and_part_boundaries() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let mut failures = Vec::new();
    for mode in ["messages-text", "messages-thinking", "text-items", "text-parts", "refusal-items", "refusal-parts", "reasoning-text", "reasoning-summary"] {
        let messages = mode.starts_with("messages-");
        set_review_source_protocol(&store, if messages { "messages" } else { "responses" });
        let endpoint = if messages { "messages" } else { "responses" };
        let mut request = json!({"model":"autojev/model/uuid-coding","stream":true,"max_tokens":16});
        let prompt = format!("review-indexed-{mode}");
        if messages { request["messages"] = json!([{"role":"user","content":prompt}]); } else { request["input"] = prompt.into(); }
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/{endpoint}", gateway.port)).json(&request).send().await.unwrap();
        assert_eq!(reply.status(), 200);
        let bytes = reply.bytes().await.unwrap();
        let mut output = [String::new(), String::new()];
        let mut terminal = false;
        for (_, data) in crate::protocol::SseParser::default().push(&bytes).unwrap() {
            let value: serde_json::Value = serde_json::from_str(&data).unwrap();
            let kind = value["type"].as_str().unwrap();
            terminal |= kind == "message_stop" || kind == "response.completed";
            if messages {
                let field = if mode == "messages-text" { "text" } else { "thinking" };
                let piece = if kind == "content_block_start" { value["content_block"][field].as_str() } else if kind == "content_block_delta" { value["delta"][field].as_str() } else { None };
                if let Some(piece) = piece { output[value["index"].as_u64().unwrap() as usize].push_str(piece); }
                if value["delta"]["type"] == "signature_delta" { assert_eq!(value["delta"]["signature"], "fixture-signature"); }
            } else if kind.ends_with(".delta") {
                let index = if mode.ends_with("items") { &value["output_index"] } else if mode == "reasoning-summary" { &value["summary_index"] } else { &value["content_index"] };
                output[index.as_u64().unwrap() as usize].push_str(value["delta"].as_str().unwrap());
            }
        }
        assert!(terminal);
        let expected = if mode == "messages-thinking" { ["[REDACTED]", "other retained"] } else { ["fictional-", "coding 完成"] };
        if output != expected { failures.push(format!("{mode}: {output:?}")); }
    }
    assert_eq!(received.lock().unwrap().len(), 8);
    assert!(received.lock().unwrap().iter().all(|r| r["path"].as_str().unwrap().starts_with("/coding/v4/")));
    gateway.stop().await;
    upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn api_source_responses_custom_input_preserves_raw_text_and_item_identity() {
    let (_root, store, received, upstream) = source_fixture().await;
    set_review_source_protocol(&store, "responses");
    let gateway = crate::proxy::start(store).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/responses", gateway.port))
        .json(&json!({"model":"autojev/model/uuid-coding","stream":true,"input":"review-custom-input","tools":[{"type":"custom","name":"echo"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    let bytes = reply.bytes().await.unwrap();
    let mut input = [String::new(), String::new()];
    let mut done = [String::new(), String::new()];
    let mut final_output = serde_json::Value::Null;
    let mut item_ids = [String::new(), String::new()];
    for (_, data) in crate::protocol::SseParser::default().push(&bytes).unwrap() {
        let value: serde_json::Value = serde_json::from_str(&data).unwrap();
        match value["type"].as_str().unwrap() {
            "response.output_item.added" => {
                let index = value["output_index"].as_u64().unwrap() as usize;
                assert_eq!(value["item"]["type"], "custom_tool_call");
                item_ids[index] = value["item"]["id"].as_str().unwrap().into();
            }
            "response.custom_tool_call_input.delta" => {
                let index = value["output_index"].as_u64().unwrap() as usize;
                assert_eq!(value["item_id"], item_ids[index]);
                input[index].push_str(value["delta"].as_str().unwrap());
            }
            "response.custom_tool_call_input.done" => {
                let index = value["output_index"].as_u64().unwrap() as usize;
                done[index] = value["input"].as_str().unwrap().into();
            }
            "response.completed" => final_output = value["response"]["output"].clone(),
            _ => {}
        }
    }
    assert_eq!(item_ids, ["ctc_0", "ctc_1"]);
    assert_eq!(done, ["你好 [REDACTED] 完成", "other raw input retained"]);
    assert_eq!(final_output[0]["input"], done[0]);
    assert_eq!(final_output[1]["input"], done[1]);
    assert_eq!(received.lock().unwrap().len(), 1);
    assert_eq!(received.lock().unwrap()[0]["path"], "/coding/v4/responses");
    gateway.stop().await;
    upstream.abort();
    assert_eq!(input, done, "Raw custom input deltas must match each item's done value");
}

fn set_review_source_protocol(store: &ConfigStore, protocol: &str) {
    store.update(|config| {
        config.providers.iter_mut().find(|p| p.id == "coding").unwrap().api_type = protocol.into();
        config.models.iter_mut().find(|m| m.id == "uuid-coding").unwrap().api_type.clear();
        config.api_sources.get_mut("coding").unwrap().api_type = protocol.into();
    }).unwrap();
}

#[tokio::test]
async fn api_source_distinct_delta_fields_keep_interleaved_text_separate() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let mut failures = Vec::new();
    for mode in ["chat-content", "chat-refusal", "chat-reasoning_content", "chat-reasoning", "responses-text", "responses-refusal"] {
        let responses = mode.starts_with("responses-");
        set_review_source_protocol(&store, if responses { "responses" } else { "chat_completions" });
        let mut request = json!({"model":"autojev/model/uuid-coding","stream":true});
        let prompt = format!("review-field-{mode}");
        if responses { request["input"] = prompt.into(); } else { request["messages"] = json!([{"role":"user","content":prompt}]); }
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/{}", gateway.port, if responses { "responses" } else { "chat/completions" }))
            .json(&request).send().await.unwrap();
        assert_eq!(reply.status(), 200);
        let mut fields = std::collections::BTreeMap::<String, String>::new();
        let mut terminal = false;
        for (event, data) in crate::protocol::SseParser::default().push(&reply.bytes().await.unwrap()).unwrap() {
            if data == "[DONE]" { terminal = true; continue; }
            let value: serde_json::Value = serde_json::from_str(&data).unwrap();
            if responses {
                if event.ends_with(".delta") { fields.entry(event).or_default().push_str(value["delta"].as_str().unwrap()); }
                terminal |= value["type"] == "response.completed";
            } else if let Some(delta) = value.pointer("/choices/0/delta").and_then(|v| v.as_object()) {
                for (field, piece) in delta { if let Some(piece) = piece.as_str() { fields.entry(field.clone()).or_default().push_str(piece); } }
            }
        }
        let first = match mode { "responses-text" => "response.output_text.delta", "responses-refusal" => "response.refusal.delta", _ => mode.strip_prefix("chat-").unwrap() };
        if fields.get(first).map(String::as_str) != Some("first: [REDACTED] done; fictional-") || !fields.values().any(|v| v == "other coding") || !terminal {
            failures.push(format!("{mode}: {fields:?}, terminal={terminal}"));
        }
    }
    let records = received.lock().unwrap().clone();
    assert_eq!(records.len(), 6);
    assert!(records.iter().all(|r| r["path"].as_str().unwrap().starts_with("/coding/v4/")));
    gateway.stop().await; upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn api_source_chat_choices_keep_separate_text_reasoning_and_tools() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store).await.unwrap();
    let mut failures = Vec::new();
    for field in ["content", "refusal", "reasoning_content", "reasoning", "tools"] {
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
            .json(&json!({"model":"autojev/model/uuid-coding","n":2,"stream":true,"messages":[{"role":"user","content":format!("review-choice-{field}")}]})).send().await.unwrap();
        assert_eq!(reply.status(), 200);
        let bytes = match reply.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => { failures.push(format!("{field}: stream terminated: {error}")); continue; }
        };
        let events = crate::protocol::SseParser::default().push(&bytes).unwrap();
        assert_eq!(events.last().unwrap().1, "[DONE]");
        let mut output = [String::new(), String::new()];
        let mut calls = [String::new(), String::new()];
        let mut finished = [false; 2];
        for (_, data) in events.into_iter().filter(|(_, data)| data != "[DONE]") {
            let value: serde_json::Value = serde_json::from_str(&data).unwrap();
            for choice in value["choices"].as_array().unwrap() {
                let index = choice["index"].as_u64().unwrap() as usize;
                if let Some(piece) = choice["delta"][field].as_str() { output[index].push_str(piece); }
                if let Some(tools) = choice["delta"]["tool_calls"].as_array() {
                    for call in tools {
                        assert_eq!(call["index"], 0);
                        if let Some(id) = call["id"].as_str() { calls[index] = id.into(); }
                        output[index].push_str(call["function"]["arguments"].as_str().unwrap());
                    }
                }
                finished[index] |= choice["finish_reason"].is_string();
            }
        }
        assert_eq!(finished, [true, true]);
        let expected = if field == "tools" {
            assert_eq!(calls, ["call_0", "call_1"]);
            for (index, text) in output.iter().enumerate() {
                match serde_json::from_str::<serde_json::Value>(text) {
                    Ok(value) => {
                        let expected = if index == 0 { "[REDACTED]" } else { "other value" };
                        if value["note"] != expected { failures.push(format!("{field}/{index}: {value}")); }
                    }
                    Err(error) => failures.push(format!("{field}/{index}: invalid arguments: {error}")),
                }
            }
            continue;
        } else { ["你好 [REDACTED] 完成", "other choice retained"] };
        if output != expected { failures.push(format!("{field}: {output:?}")); }
    }
    let records = received.lock().unwrap().clone();
    assert_eq!(records.len(), 5);
    assert!(records.iter().all(|r| r["path"] == "/coding/v4/chat/completions" && r["n"] == 2));
    gateway.stop().await;
    upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

async fn check_api_source_wire_stream(marker: &str) {
    use std::time::Duration;
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store).await.unwrap();
    let mut failures = Vec::new();
    for protocol in [crate::protocol::Protocol::Chat, crate::protocol::Protocol::Responses] {
        let mut body = json!({"model":"autojev/model/uuid-coding","stream":true});
        if protocol == crate::protocol::Protocol::Chat {
            body["messages"] = json!([{"role":"user","content":marker}]);
        } else {
            body["input"] = json!([{"role":"user","content":marker}]);
        }
        let start = tokio::time::Instant::now();
        let mut reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}{}", gateway.port, protocol.path()))
            .timeout(Duration::from_secs(8)).json(&body).send().await.unwrap();
        let status = reply.status();
        let mut bytes = Vec::new();
        let early = tokio::time::timeout_at(start + Duration::from_secs(1), async {
            while !String::from_utf8_lossy(&bytes).contains("early") {
                bytes.extend_from_slice(&reply.chunk().await?.ok_or_else(|| anyhow::anyhow!("EOF before early content"))?);
            }
            Ok::<_, anyhow::Error>(())
        }).await;
        if !matches!(early, Ok(Ok(()))) { failures.push(format!("{marker}/{protocol:?}: no incremental early content: {early:?}")); }
        loop {
            match reply.chunk().await {
                Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(error) => { failures.push(format!("{marker}/{protocol:?}: body error: {error}")); break; }
            }
        }
        if status != 200 { failures.push(format!("{marker}/{protocol:?}: status {status}")); }
        let events = crate::protocol::SseParser::default().push(&bytes).unwrap();
        let mut text = String::new();
        let mut terminal = false;
        for (event, data) in events {
            if data == "[DONE]" { terminal = true; continue; }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else { continue; };
            let delta = if protocol == crate::protocol::Protocol::Chat {
                value.pointer("/choices/0/delta/content").and_then(|v| v.as_str())
            } else if event == "response.output_text.delta" { value["delta"].as_str() } else { None };
            if let Some(delta) = delta { text.push_str(delta); }
            terminal |= event == "response.completed";
        }
        if text != "early[REDACTED]" || !terminal {
            failures.push(format!("{marker}/{protocol:?}: text={text:?}, terminal={terminal}"));
        }
    }
    let records = received.lock().unwrap().clone();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|r| r["path"] == "/coding/v4/chat/completions"));
    gateway.stop().await;
    upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn api_source_mixed_case_sse_media_type_remains_incremental_and_redacted() {
    check_api_source_wire_stream("review-wire-upper").await;
}

#[tokio::test]
async fn api_source_cr_only_sse_remains_incremental_and_redacted() {
    check_api_source_wire_stream("review-wire-cr").await;
}

#[tokio::test]
async fn api_source_event_names_classify_without_mutating_json_payloads() {
    use std::time::Duration;
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let mut failures = Vec::new();
    for terminal in ["response.completed", "response.incomplete", "response.failed", "message_stop"] {
        let messages = terminal == "message_stop";
        set_review_source_protocol(&store, if messages { "messages" } else { "responses" });
        let mut body = json!({"model":"autojev/model/uuid-coding","stream":true});
        let prompt = format!("review-event-meta-{terminal}");
        if messages { body["messages"] = json!([{"role":"user","content":prompt}]); body["max_tokens"] = 8.into(); }
        else { body["input"] = prompt.into(); }
        let start = tokio::time::Instant::now();
        let mut reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/{}", gateway.port, if messages { "messages" } else { "responses" })).timeout(Duration::from_secs(8)).json(&body).send().await.unwrap();
        assert_eq!(reply.status(), 200);
        let mut bytes = Vec::new();
        let marker = format!("event: {terminal}\n");
        let early = tokio::time::timeout_at(start + Duration::from_secs(1), async {
            while !String::from_utf8_lossy(&bytes).contains(&marker) {
                bytes.extend_from_slice(&reply.chunk().await?.ok_or_else(|| anyhow::anyhow!("EOF before terminal"))?);
            }
            Ok::<_, anyhow::Error>(())
        }).await;
        if !matches!(early, Ok(Ok(()))) { failures.push(format!("{terminal}: terminal not flushed before delayed EOF: {early:?}")); }
        while let Some(chunk) = reply.chunk().await.unwrap() { bytes.extend_from_slice(&chunk); }
        let mut text = String::new();
        let mut pings = Vec::new();
        for (event, data) in crate::protocol::SseParser::default().push(&bytes).unwrap() {
            let value: serde_json::Value = serde_json::from_str(&data).unwrap();
            if event == "ping" { pings.push(value); }
            else {
                if value.get("type").is_some() { failures.push(format!("{terminal}/{event}: synthetic type changed payload")); }
                if let Some(delta) = value["delta"].as_str().or_else(|| value.pointer("/delta/text").and_then(|v| v.as_str())) { text.push_str(delta); }
            }
        }
        if pings != vec![json!({"sequence":1}), json!({"type":"provider.original","sequence":2}), json!({"type":null,"sequence":3})] || text != "first: [REDACTED] done; fictional-" {
            failures.push(format!("{terminal}: pings={pings:?}, text={text:?}"));
        }
    }
    assert_eq!(received.lock().unwrap().len(), 4);
    assert!(received.lock().unwrap().iter().all(|r| r["path"].as_str().unwrap().starts_with("/coding/v4/")));
    gateway.stop().await; upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn api_source_multiline_sse_keeps_every_data_line_and_event_metadata() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store).await.unwrap();
    let mut failures = Vec::new();
    for style in ["lf", "crlf", "cr"] {
        let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
            .json(&json!({"model":"autojev/model/uuid-coding","stream":true,"messages":[{"role":"user","content":format!("review-multiline-{style}")}]})).send().await.unwrap();
        assert_eq!(reply.status(), 200);
        let bytes = reply.bytes().await.unwrap();
        let events = crate::protocol::SseParser::default().push(&bytes).unwrap();
        let expected = vec![("provider.extension".into(), "line one\n\n第二行\n".into()), (String::new(), String::new()), (String::new(), "[DONE]".into())];
        if events != expected { failures.push(format!("{style}: {events:?}")); }
        let text = std::str::from_utf8(&bytes).unwrap();
        for metadata in [": keepalive", "id: extension-1", "retry: 1000", "provider-extension: retained"] {
            assert!(text.contains(metadata), "{style}: missing {metadata}");
        }
    }
    assert_eq!(received.lock().unwrap().len(), 3);
    gateway.stop().await;
    upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn api_source_split_stream_secret_never_reaches_output_headers_or_logs() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":"autojev/model/uuid-coding","stream":true,"messages":[{"role":"user","content":"split-secret"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert!(!reply.headers().contains_key("x-request-id"));
    let body = reply.text().await.unwrap();
    assert!(body.contains("你好 [REDACTED] [REDACTED]"), "{body}");
    assert!(body.contains("data: [DONE]"));
    assert!(!body.contains("fictional-coding"));
    assert_eq!(received.lock().unwrap().len(), 1);
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_escaped_json_secret_never_reaches_text_or_stream_output() {
    let (_root, store, _received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    for endpoint in ["chat/completions", "responses", "messages"] {
        for streaming in [false, true] {
            let mut body = json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"escaped-secret"}],"max_tokens":8,"stream":streaming});
            if endpoint == "responses" {
                body["input"] = json!("escaped-secret");
                body.as_object_mut().unwrap().remove("messages");
            }
            let reply = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/v1/{endpoint}", gateway.port))
                .json(&body)
                .send()
                .await
                .unwrap();
            let output = reply.text().await.unwrap();
            assert!(!output.contains("fictional-coding"), "{endpoint}: {output}");
            for data in output
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
            {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                    assert!(
                        !value.to_string().contains("fictional-coding"),
                        "Decoded SSE leaked a credential"
                    );
                }
            }
            assert!(output.contains("[REDACTED]"), "{endpoint}: {output}");
        }
    }
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_legacy_database_copy_preserves_existing_uuid_and_credential_reference() {
    let (root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| {
            config.api_sources.clear();
            config.providers.truncate(1);
            config.models.truncate(1);
        })
        .unwrap();
    store
        .write_secret("provider:official", "fictional-official")
        .unwrap();
    let expected = store.read().models[0].clone();
    let mut legacy = serde_json::to_value(store.read()).unwrap();
    legacy.as_object_mut().unwrap().remove("api_sources");
    drop(store);
    let original = root.path().join("sources.db");
    let database = rusqlite::Connection::open(&original).unwrap();
    database
        .execute(
            "UPDATE app_meta SET value = ?1 WHERE key = 'config'",
            [legacy.to_string()],
        )
        .unwrap();
    drop(database);
    let original_bytes = std::fs::read(&original).unwrap();
    let copy = root.path().join("isolated-copy.db");
    std::fs::copy(&original, &copy).unwrap();
    let reopened = Arc::new(ConfigStore::load(copy).unwrap());
    let config = reopened.read();
    assert!(config.api_sources.is_empty());
    assert_eq!(config.models[0].id, expected.id);
    assert_eq!(config.models[0].provider_id, expected.provider_id);
    let gateway = crate::proxy::start(reopened).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":format!("autojev/model/{}", expected.id),"messages":[{"role":"user","content":"fixture"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert_eq!(
        received.lock().unwrap()[0]["authorization"],
        "Bearer fictional-official"
    );
    assert_eq!(
        std::fs::read(original).unwrap(),
        original_bytes,
        "Only the isolated copy may be changed"
    );
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_nested_tool_json_secret_never_reappears_after_conversion() {
    let (_root, store, _received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let mut failures = Vec::new();
    for source_protocol in ["chat_completions", "responses", "messages"] {
        store
            .update(|config| {
                config
                    .providers
                    .iter_mut()
                    .find(|p| p.id == "coding")
                    .unwrap()
                    .api_type = source_protocol.into();
                config
                    .models
                    .iter_mut()
                    .find(|m| m.id == "uuid-coding")
                    .unwrap()
                    .api_type = String::new();
                config.api_sources.get_mut("coding").unwrap().api_type = source_protocol.into();
            })
            .unwrap();
        for endpoint in ["chat/completions", "responses", "messages"] {
            for stream in [false, true] {
                let schema = json!({"type":"object","properties":{"note":{"type":"string"}}});
                let tool = match endpoint {
                    "messages" => json!({"name":"echo","input_schema":schema}),
                    "responses" => json!({"type":"function","name":"echo","parameters":schema}),
                    _ => json!({"type":"function","function":{"name":"echo","parameters":schema}}),
                };
                let mut request = json!({"model":"autojev/model/uuid-coding","max_tokens":16,"stream":stream,"tools":[tool]});
                if endpoint == "responses" {
                    request["input"] = "nested-secret".into();
                } else {
                    request["messages"] = json!([{"role":"user","content":"nested-secret"}]);
                }
                let reply = reqwest::Client::new()
                    .post(format!("http://127.0.0.1:{}/v1/{endpoint}", gateway.port))
                    .json(&request)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(reply.status(), 200);
                let body = reply.bytes().await.unwrap();
                let value = if stream {
                    crate::protocol::collect_debug_stream(
                        &body,
                        crate::protocol::Protocol::parse(endpoint).unwrap(),
                        "fixture",
                    )
                    .unwrap()
                } else {
                    serde_json::from_slice(&body).unwrap()
                };
                let input = match endpoint {
                    "messages" => value["content"][0]["input"].clone(),
                    "responses" => {
                        serde_json::from_str(value["output"][0]["arguments"].as_str().unwrap())
                            .unwrap()
                    }
                    _ => serde_json::from_str(
                        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap(),
                };
                if input["note"] != "[REDACTED]" {
                    failures.push(format!(
                        "{source_protocol}/{endpoint}/stream={stream}: {value}"
                    ));
                }
            }
        }
    }
    gateway.stop().await;
    upstream.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
}

#[tokio::test]
async fn api_source_redaction_preserves_incomplete_prefix_and_sse_block_order() {
    use futures_util::StreamExt;
    let frames = [
        ": keepalive\nprovider-extension: retained\n\n",
        "event: message_start\ndata: {\"message\":{\"usage\":{}}}\n\n",
        "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你好 fictional-\"}}\n\n",
        "event: content_block_stop\ndata: {\"index\":0}\n\n",
        "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
        "event: message_stop\ndata: {}\n\n",
    ];
    let input =
        futures_util::stream::iter(frames.into_iter().map(|text| {
            Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(text.as_bytes()))
        }));
    let safe = crate::api_sources::redacted_stream(input, Some("fictional-coding".into()), true);
    futures_util::pin_mut!(safe);
    let mut bytes = Vec::new();
    while let Some(chunk) = safe.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains(": keepalive\nprovider-extension: retained"));
    assert!(
        text.find("你好 fictional-").unwrap() < text.find("event: content_block_stop").unwrap()
    );
    let collected = crate::protocol::collect_debug_stream(
        &bytes,
        crate::protocol::Protocol::Messages,
        "fixture",
    )
    .unwrap();
    assert!(
        collected.to_string().contains("你好 fictional-"),
        "An incomplete prefix is legitimate output"
    );
}

#[tokio::test]
async fn api_source_cross_delta_secret_cannot_reappear_in_converted_output_or_logs() {
    let (_root, store, _received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    for source_protocol in ["chat_completions", "responses", "messages"] {
        store
            .update(|config| {
                config
                    .providers
                    .iter_mut()
                    .find(|p| p.id == "coding")
                    .unwrap()
                    .api_type = source_protocol.into();
                config
                    .models
                    .iter_mut()
                    .find(|m| m.id == "uuid-coding")
                    .unwrap()
                    .api_type = String::new();
                config.api_sources.get_mut("coding").unwrap().api_type = source_protocol.into();
            })
            .unwrap();
        for (endpoint, terminal) in [
            ("chat/completions", "[DONE]"),
            ("responses", "response.completed"),
            ("messages", "message_stop"),
        ] {
            let request = if endpoint == "responses" {
                json!({"model":"autojev/model/uuid-coding","input":"delta-secret","stream":true})
            } else {
                json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"delta-secret"}],"max_tokens":16,"stream":true})
            };
            let reply = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/v1/{endpoint}", gateway.port))
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), 200);
            let body = reply.text().await.unwrap();
            assert!(
                body.contains(terminal),
                "{source_protocol}/{endpoint}: {body}"
            );
            assert!(
                !body.contains("fictional-coding"),
                "{source_protocol}/{endpoint}: {body}"
            );
            assert!(
                body.contains("[REDACTED]"),
                "{source_protocol}/{endpoint}: {body}"
            );
        }
    }
    assert!(!serde_json::to_string(&store.request_logs("").unwrap())
        .unwrap()
        .contains("fictional-coding"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_retired_alias_and_uuid_cannot_be_captured_as_another_sources_bare_id() {
    let (_root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| {
            config.api_sources.get_mut("official").unwrap().retired = true;
            config.providers.retain(|p| p.id != "official");
            config.models.retain(|m| m.provider_id != "official");
            config.models[0].model_id = "official/same-model".into();
            config
                .api_sources
                .get_mut("third")
                .unwrap()
                .model_bindings
                .insert("uuid-third".into(), "official/same-model".into());
        })
        .unwrap();
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    for reference in ["official/same-model", "uuid-official"] {
        if reference == "uuid-official" {
            store
                .update(|config| {
                    config.models[0].model_id = reference.into();
                    config
                        .api_sources
                        .get_mut("third")
                        .unwrap()
                        .model_bindings
                        .insert("uuid-third".into(), reference.into());
                })
                .unwrap();
        }
        let reply = reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/v1/chat/completions",
                gateway.port
            ))
            .json(&json!({"model":reference,"messages":[{"role":"user","content":"fixture"}]}))
            .send()
            .await
            .unwrap();
        assert!(
            !reply.status().is_success(),
            "Retired reference dispatched: {reference}"
        );
        assert!(
            received.lock().unwrap().is_empty(),
            "Retired reference hit another source"
        );
    }
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_redaction_preserves_json_progress_for_idle_timeout() {
    let (_root, store, _received, upstream) = source_fixture().await;
    store
        .update(|config| config.gateway.stream_idle_seconds = 1)
        .unwrap();
    let gateway = crate::proxy::start(store).await.unwrap();
    let reply = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port))
        .json(&json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"slow-json"}]})).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert!(reply
        .text()
        .await
        .unwrap()
        .contains("fictional slow response"));
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_source_inflight_generation_never_uses_replaced_legacy_or_decision_credentials() {
    let (_root, store, received, upstream) = source_fixture().await;
    let gateway = crate::proxy::start(store.clone()).await.unwrap();
    let request = json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"inflight-secret"}]});
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", gateway.port);
    let inflight = tokio::spawn(client.post(&url).json(&request).send());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while received.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    store
        .write_secret("provider:coding", "fictional-replacement-account")
        .unwrap();
    store
        .write_secret("autojev-cloud", "fictional-replacement-decision")
        .unwrap();
    let second = client.post(url).json(&request).send().await.unwrap();
    assert_eq!(second.status(), 200);
    assert_eq!(inflight.await.unwrap().unwrap().status(), 200);
    let records = received.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(
        |record| record["authorization"] == "Bearer fictional-coding"
            && record["path"] == "/coding/v4/chat/completions"
    ));
    drop(records);
    gateway.stop().await;
    upstream.abort();
}

#[tokio::test]
async fn api_sources_background_schedule_never_generates_or_speed_tests() {
    let (_root, store, received, upstream) = source_fixture().await;
    store
        .update(|config| config.performance_settings.enabled = true)
        .unwrap();
    let task = tokio::spawn(crate::performance::schedule(
        store,
        Arc::new(crate::performance::Runner::default()),
    ));
    tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    assert!(received.lock().unwrap().is_empty());
    task.abort();
    upstream.abort();
}

#[tokio::test]
async fn unreviewed_coding_plan_keeps_its_fixed_target_but_never_dispatches() {
    let (_root,store,received,upstream)=source_fixture().await;
    store.update(|c|c.api_sources.get_mut("coding").unwrap().kind=crate::api_sources::SourceKind::CodingPlan).unwrap();
    let gateway=crate::proxy::start(store.clone()).await.unwrap();
    for model in ["coding/same-model","autojev/model/uuid-coding"] {
        let reply=reqwest::Client::builder().no_proxy().build().unwrap().post(format!("http://127.0.0.1:{}/v1/chat/completions",gateway.port)).json(&json!({"model":model,"messages":[{"role":"user","content":"fictional coding plan request"}],"max_tokens":32})).send().await.unwrap();
        assert!(!reply.status().is_success());assert!(reply.text().await.unwrap().contains("coding_plan_unverified"));
    }
    assert!(received.lock().unwrap().is_empty());assert_eq!(store.read().api_sources["coding"].endpoint.ends_with("/coding/v4"),true);
    gateway.stop().await;upstream.abort();
}

fn coding_proof(store:&ConfigStore)->crate::coding_hand_run::Proof {
    use crate::hand_run_policy::{EvidenceKind,ReviewedEvidence,Policy};
    use crate::subscription::{EvidenceState,QuotaEvidence,QuotaView,QuotaBucket,QuotaWindow,QuotaCredits,QuotaPermission};
    let c=store.read();let source=&c.api_sources["coding"];let now=chrono::Utc::now();let authority="https://127.0.0.1/fictional-plan-receipt";
    crate::coding_hand_run::Proof{plan_id:"5872ea21-53ba-45c4-a427-01a4190833e9".into(),binding:crate::coding_hand_run::Binding{provider_id:"coding".into(),connection_instance_id:source.connection_instance_id.clone(),generation:source.generation,endpoint:source.endpoint.clone(),credential_reference:source.credential_reference.clone(),model_id:"same-model".into(),model_uuid:"uuid-coding".into(),account:"fictional-plan-account".into(),plan:"fictional-plan".into()},jev_artifact_sha256:crate::runtime::artifact_sha().unwrap(),policy:Policy{
        evidence:[EvidenceKind::Identity,EvidenceKind::Plan,EvidenceKind::Eligibility,EvidenceKind::Protocol,EvidenceKind::Quota,EvidenceKind::WholeCallCost].into_iter().map(|kind|ReviewedEvidence{kind,state:EvidenceState::Available,authority:authority.into(),observed_at:now.timestamp(),valid_until:now.timestamp()+300,receipt_sha256:"b".repeat(64),reviewed:true}).collect(),
        quota:QuotaEvidence{state:EvidenceState::Available,source:Some(authority.into()),observed_at:Some(now.to_rfc3339()),view:QuotaView::RateLimits,buckets:vec![QuotaBucket{limit_id:"fictional-plan-quota".into(),permission:QuotaPermission::Allowed,windows:vec![QuotaWindow{label:"fictional".into(),used_percent:Some(1.0),resets_at:Some(now.timestamp()+300),..Default::default()}],credits:Some(QuotaCredits{permission:QuotaPermission::Denied,..Default::default()}),..Default::default()}],..Default::default()},subscription_only:true,streaming:false,function_tools:false,max_requests:1,max_input_bytes:2048,max_output_tokens:32,max_auxiliary_requests:0,max_tool_rounds:0}}
}

#[tokio::test]
async fn reviewed_coding_plan_uses_its_plan_endpoint_with_a_finite_independent_budget() {
    let (_root,store,received,upstream)=source_fixture().await;
    store.update(|c|c.api_sources.get_mut("coding").unwrap().kind=crate::api_sources::SourceKind::CodingPlan).unwrap();
    let proof=coding_proof(&store);
    crate::coding_hand_run::enable(&store,"coding",proof.clone()).unwrap();let gateway=crate::proxy::start(store.clone()).await.unwrap();
    let request=json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"fictional plan"}],"max_tokens":16});
    let reply=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&request).unwrap().send().await.unwrap();assert_eq!(reply.status(),200);assert_eq!(reply.json::<serde_json::Value>().await.unwrap()["choices"][0]["message"]["content"],"/coding/v4/chat/completions");
    assert_eq!(received.lock().unwrap().len(),1);
    crate::coding_hand_run::disable(&store,"coding").unwrap();crate::coding_hand_run::enable(&store,"coding",proof).unwrap();
    let reply=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&request).unwrap().send().await.unwrap();assert!(!reply.status().is_success());assert_eq!(received.lock().unwrap().len(),1);
    gateway.stop().await;upstream.abort();
}

#[tokio::test]
async fn coding_plan_evidence_failures_restart_and_first_http_failure_stay_locked() {
    let (root,store,received,upstream)=source_fixture().await;
    store.update(|c|c.api_sources.get_mut("coding").unwrap().kind=crate::api_sources::SourceKind::CodingPlan).unwrap();
    let proof=coding_proof(&store);
    for bad in 0..6 {
        let mut p=proof.clone();
        match bad {
            0=>p.policy.subscription_only=false,
            1=>p.binding.endpoint.push_str("/other"),
            2=>p.policy.evidence[0].valid_until=chrono::Utc::now().timestamp()-1,
            3=>p.policy.evidence[5].state=crate::subscription::EvidenceState::Unknown,
            4=>p.policy.quota.buckets[0].windows[0].used_percent=Some(100.0),
            _=>p.jev_artifact_sha256="0".repeat(64),
        }
        assert!(crate::coding_hand_run::enable(&store,"coding",p).is_err());
    }
    assert!(received.lock().unwrap().is_empty());
    let mut proof=proof;proof.policy.max_requests=2;
    crate::coding_hand_run::enable(&store,"coding",proof.clone()).unwrap();
    let mut expanded=proof.clone();expanded.policy.max_output_tokens+=1;
    assert!(crate::coding_hand_run::enable(&store,"coding",expanded).is_err(),"same plan cannot expand another budget");
    let gateway=crate::proxy::start(store.clone()).await.unwrap();
    let req=json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"fail-429"}],"max_tokens":16});
    let reply=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&req).unwrap().send().await.unwrap();
    assert_eq!(reply.status(),429);let text=reply.text().await.unwrap();assert!(!text.contains("fictional-coding"));
    assert!(store.read().api_sources["coding"].hand_run_ledger[&proof.plan_id].stopped);
    assert!(crate::coding_hand_run::enable(&store,"coding",proof.clone()).is_err());
    let reopened=ConfigStore::load(root.path().join("sources.db")).unwrap();
    assert!(reopened.read().api_sources["coding"].hand_run.is_none());
    assert!(crate::coding_hand_run::enable(&reopened,"coding",proof).is_err());
    assert_eq!(received.lock().unwrap().len(),1);gateway.stop().await;upstream.abort();
}

#[tokio::test]
async fn coding_plan_public_stream_cancel_and_missing_terminal_lock_only_the_original_plan() {
    for mode in ["done","cancel","missing"] {
        let (_root,store,received,upstream)=source_fixture().await;
        store.update(|c|c.api_sources.get_mut("coding").unwrap().kind=crate::api_sources::SourceKind::CodingPlan).unwrap();
        let mut proof=coding_proof(&store);proof.policy.streaming=true;proof.policy.max_requests=2;
        crate::coding_hand_run::enable(&store,"coding",proof.clone()).unwrap();
        let gateway=crate::proxy::start(store.clone()).await.unwrap();
        let req=json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":format!("plan-stream-{mode}")}],"max_tokens":16,"stream":true});
        let mut reply=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&req).unwrap().send().await.unwrap();assert_eq!(reply.status(),200);
        if mode=="cancel" {assert!(reply.chunk().await.unwrap().is_some());drop(reply);}else{let _=reply.text().await;}
        if mode!="done" {tokio::time::timeout(std::time::Duration::from_secs(5),async{while !store.read().api_sources["coding"].hand_run_ledger[&proof.plan_id].stopped{tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();assert!(crate::coding_hand_run::enable(&store,"coding",proof.clone()).is_err());}
        else {assert!(!store.read().api_sources["coding"].hand_run_ledger[&proof.plan_id].stopped);}
        assert_eq!(received.lock().unwrap().len(),1);gateway.stop().await;upstream.abort();
    }
}

#[tokio::test]
async fn finite_coding_plan_streamed_tools_fail_closed_or_complete() {
    for mode in ["invalid-single","invalid-split","missing-id","missing-name","invalid-type","valid-split"] {
        let (_root,store,received,upstream)=source_fixture().await;
        store.update(|c|c.api_sources.get_mut("coding").unwrap().kind=crate::api_sources::SourceKind::CodingPlan).unwrap();
        let mut proof=coding_proof(&store);proof.policy.streaming=true;proof.policy.function_tools=true;proof.policy.max_requests=2;
        crate::coding_hand_run::enable(&store,"coding",proof.clone()).unwrap();let gateway=crate::proxy::start(store.clone()).await.unwrap();
        let request=json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":format!("finite-tool-{mode}")}],"max_tokens":16,"stream":true});
        let valid=mode=="valid-split";
        let first=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&request).unwrap().send().await;
        if valid {assert!(first.unwrap().text().await.unwrap().contains("[DONE]"));}
        else if let Ok(first)=first {assert!(first.text().await.is_err(),"invalid assembled tool must fail: {mode}");}
        let second=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&request).unwrap().send().await;
        assert_eq!(received.lock().unwrap().len(),if valid {2}else{1},"fixed source dispatch count: {mode}");
        if valid {assert!(second.unwrap().text().await.unwrap().contains("[DONE]"));}
        else {assert!(!second.unwrap().status().is_success());assert!(crate::coding_hand_run::enable(&store,"coding",proof).is_err());}
        gateway.stop().await;upstream.abort();
    }
}

#[tokio::test]
async fn finite_coding_plan_outer_parser_failure_locks_before_a_second_dispatch() {
    let (_root,store,received,upstream)=source_fixture().await;
    store.update(|c|c.api_sources.get_mut("coding").unwrap().kind=crate::api_sources::SourceKind::CodingPlan).unwrap();
    let mut proof=coding_proof(&store);proof.policy.streaming=true;proof.policy.function_tools=true;proof.policy.max_requests=2;
    crate::coding_hand_run::enable(&store,"coding",proof.clone()).unwrap();let gateway=crate::proxy::start(store.clone()).await.unwrap();
    let request=json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"finite-tool-valid-single"}],"max_tokens":16,"stream":false});
    let first=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&request).unwrap().send().await.unwrap();
    assert_eq!(first.status(),StatusCode::BAD_GATEWAY);assert!(first.text().await.unwrap().contains("invalid JSON"));
    let second=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&request).unwrap().send().await;
    assert_eq!(received.lock().unwrap().len(),1,"outer parser failure must never dispatch again");
    assert!(!second.unwrap().status().is_success());assert!(crate::coding_hand_run::enable(&store,"coding",proof).is_err());
    gateway.stop().await;upstream.abort();
}

#[tokio::test]
async fn coding_plan_first_fixed_admission_failure_cannot_resume_the_plan() {
    let (_root,store,received,upstream)=source_fixture().await;
    store.update(|c|c.api_sources.get_mut("coding").unwrap().kind=crate::api_sources::SourceKind::CodingPlan).unwrap();
    let proof=coding_proof(&store);crate::coding_hand_run::enable(&store,"coding",proof.clone()).unwrap();
    store.update(|c|c.models.iter_mut().find(|m|m.id=="uuid-coding").unwrap().enabled=false).unwrap();
    let gateway=crate::proxy::start(store.clone()).await.unwrap();
    let req=json!({"model":"autojev/model/uuid-coding","messages":[{"role":"user","content":"fictional plan"}],"max_tokens":16});
    let reply=crate::dispatch::local_gateway_request(gateway.port,crate::protocol::Protocol::Chat,&req).unwrap().send().await.unwrap();assert!(!reply.status().is_success());
    store.update(|c|c.models.iter_mut().find(|m|m.id=="uuid-coding").unwrap().enabled=true).unwrap();
    assert!(crate::coding_hand_run::enable(&store,"coding",proof).is_err(),"first fixed-target refusal must stop its finite plan");
    assert!(received.lock().unwrap().is_empty());gateway.stop().await;upstream.abort();
}
