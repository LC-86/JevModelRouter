use super::stream::{Bridge, SseParser};
use super::*;
const PROTOCOLS: [Protocol; 3] = [Protocol::Chat, Protocol::Responses, Protocol::Messages];

pub(crate) fn request(protocol: Protocol, stream: bool) -> Value {
    match protocol {
        Protocol::Chat => {
            json!({"model":"alias","stream":stream,"max_tokens":128,"messages":[{"role":"system","content":"Be helpful"},{"role":"user","content":"你好"},{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"city\":\"上海\"}"}}]},{"role":"tool","tool_call_id":"call_1","content":"sunny"}],"tools":[{"type":"function","function":{"name":"lookup","description":"Weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}],"tool_choice":{"type":"function","function":{"name":"lookup"}}})
        }
        Protocol::Responses => {
            json!({"model":"alias","stream":stream,"max_output_tokens":128,"instructions":"Be helpful","input":[{"role":"user","content":[{"type":"input_text","text":"你好"}]},{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"city\":\"上海\"}"},{"type":"function_call_output","call_id":"call_1","output":"sunny"}],"tools":[{"type":"function","name":"lookup","description":"Weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}],"tool_choice":{"type":"function","name":"lookup"}})
        }
        Protocol::Messages => {
            json!({"model":"alias","stream":stream,"max_tokens":128,"system":[{"type":"text","text":"Be helpful"}],"messages":[{"role":"user","content":"你好"},{"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"lookup","input":{"city":"上海"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"sunny"}]}],"tools":[{"name":"lookup","description":"Weather","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],"tool_choice":{"type":"tool","name":"lookup"}})
        }
    }
}
pub(crate) fn response(protocol: Protocol) -> Value {
    match protocol {
        Protocol::Chat => {
            json!({"id":"chatcmpl_1","choices":[{"index":0,"message":{"role":"assistant","content":"你好","tool_calls":[{"id":"call_2","type":"function","function":{"name":"lookup","arguments":"{\"city\":\"北京\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":12,"completion_tokens":7}})
        }
        Protocol::Responses => {
            json!({"id":"resp_1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"你好"}]},{"type":"function_call","call_id":"call_2","name":"lookup","arguments":"{\"city\":\"北京\"}"}],"usage":{"input_tokens":12,"output_tokens":7}})
        }
        Protocol::Messages => {
            json!({"id":"msg_1","content":[{"type":"text","text":"你好"},{"type":"tool_use","id":"call_2","name":"lookup","input":{"city":"北京"}}],"stop_reason":"tool_use","usage":{"input_tokens":12,"output_tokens":7}})
        }
    }
}
fn frames(protocol: Protocol) -> Vec<Value> {
    match protocol {
        Protocol::Chat => vec![
            json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"你"},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"content":"好","tool_calls":[{"index":0,"id":"call_2","type":"function","function":{"name":"lookup","arguments":"{\"city\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"北京\"}"}}]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7}}),
        ],
        Protocol::Messages => vec![
            json!({"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_2","name":"lookup","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"北京\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ],
        Protocol::Responses => vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","content":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"你好"}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_2","name":"lookup","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"city\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"\"北京\"}"}),
            json!({"type":"response.completed","response":response(protocol)}),
        ],
    }
}
pub(crate) fn wire(protocol: Protocol) -> String {
    let mut result = frames(protocol)
        .iter()
        .map(|v| format!("data: {v}\r\n\r\n"))
        .collect::<String>();
    if protocol == Protocol::Chat {
        result += "data: [DONE]\r\n\r\n";
    }
    result
}
#[test]
fn all_six_request_directions_preserve_history_and_tools() {
    for source in PROTOCOLS {
        for target in PROTOCOLS {
            if source == target {
                continue;
            }
            let (body, _) =
                convert_request(&request(source, false), source, target, "upstream-model").unwrap();
            assert_eq!(body["model"], "upstream-model");
            let repr = body.to_string();
            for expected in ["你好", "sunny", "call_1", "lookup", "Be helpful"] {
                assert!(
                    repr.contains(expected),
                    "{source:?}->{target:?}: {expected}"
                );
            }
            match target {
                Protocol::Chat => {
                    assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "call_1");
                    assert_eq!(body["messages"][3]["tool_call_id"], "call_1");
                    assert_eq!(body["tools"][0]["function"]["name"], "lookup");
                    assert!(body.get("input").is_none());
                }
                Protocol::Responses => {
                    assert!(body["input"].is_array());
                    assert_eq!(body["tools"][0]["type"], "function");
                    assert_eq!(body["max_output_tokens"], 128);
                    assert!(body.get("messages").is_none());
                }
                Protocol::Messages => {
                    assert_eq!(body["system"][0]["text"], "Be helpful");
                    assert_eq!(body["messages"][1]["content"][0]["input"]["city"], "上海");
                    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "call_1");
                    assert_eq!(body["max_tokens"], 128);
                }
            }
        }
    }
}
#[test]
fn all_six_response_directions_preserve_tool_ids_text_and_usage() {
    for source in PROTOCOLS {
        for target in PROTOCOLS {
            if source == target {
                continue;
            }
            let body = convert_response(
                &response(source),
                source,
                target,
                "model",
                &ToolMap::default(),
            )
            .unwrap();
            let repr = body.to_string();
            for s in ["你好", "call_2", "lookup"] {
                assert!(repr.contains(s));
            }
            let decoded = super::response::decode_response(&body, target, "model").unwrap();
            assert_eq!(decoded.usage.input, 12);
            assert_eq!(decoded.usage.output, 7);
            assert_eq!(decoded.output.len(), 2);
        }
    }
}
#[test]
fn six_stream_directions_handle_every_byte_boundary_and_complete_once() {
    for source in PROTOCOLS {
        for target in PROTOCOLS {
            if source == target {
                continue;
            }
            let mut bridge = Bridge::new(source, target, "model", ToolMap::default());
            let mut result = bridge.begin().unwrap();
            let mut parser = SseParser::default();
            for byte in wire(source).bytes() {
                for (event, data) in parser.push(&[byte]).unwrap() {
                    result += &bridge.frame(&event, &data).unwrap();
                }
            }
            assert!(bridge.terminal, "{source:?}->{target:?}");
            assert!(bridge.finish().unwrap().is_empty());
            let mut target_parser = SseParser::default();
            let parsed = target_parser.push(result.as_bytes()).unwrap();
            assert!(result.contains("lookup"));
            assert!(result.contains("call_2"));
            let terminal = match target {
                Protocol::Chat => "[DONE]",
                Protocol::Responses => "response.completed",
                Protocol::Messages => "message_stop",
            };
            assert_eq!(
                parsed
                    .iter()
                    .filter(|(event, data)| event == terminal || data == terminal)
                    .count(),
                1
            );
            if target == Protocol::Responses {
                let data: Value = serde_json::from_str(&parsed.last().unwrap().1).unwrap();
                assert_eq!(data["response"]["output"][0]["content"][0]["text"], "你好");
                assert_eq!(data["response"]["output"][1]["call_id"], "call_2");
                assert_eq!(
                    data["response"]["output"][1]["arguments"],
                    "{\"city\":\"北京\"}"
                );
                assert_eq!(data["response"]["usage"]["output_tokens"], 7);
                for (i, (_, data)) in parsed.iter().enumerate() {
                    assert_eq!(
                        serde_json::from_str::<Value>(data).unwrap()["sequence_number"],
                        i
                    );
                }
            }
            // Round-trip through the target's stream parser also reconstructs a final response.
            let mut back = Bridge::new(target, source, "model", ToolMap::default());
            back.begin().unwrap();
            for (event, data) in parsed {
                back.frame(&event, &data).unwrap();
            }
            assert!(back.terminal);
        }
    }
}
#[test]
fn native_requests_are_unchanged_except_model() {
    let mut body = json!({"model":"alias","input":"hello","previous_response_id":"resp_old","tools":[{"type":"web_search"}],"reasoning":{"effort":"high"}});
    let (out, _) =
        convert_request(&body, Protocol::Responses, Protocol::Responses, "actual").unwrap();
    body["model"] = "actual".into();
    assert_eq!(out, body);
}
#[test]
fn unsupported_semantics_fail_before_contacting_upstream() {
    for patch in [
        json!({"previous_response_id":"old"}),
        json!({"tools":[{"type":"web_search"}]}),
        json!({"input":[{"role":"user","content":[{"type":"input_audio"}]}]}),
        json!({"conversation":"old"}),
    ] {
        let mut body = request(Protocol::Responses, false);
        body.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(convert_request(&body, Protocol::Responses, Protocol::Chat, "m").is_err());
    }
}
#[test]
fn custom_codex_tools_round_trip_as_raw_input() {
    let req = json!({"input":"edit","tools":[{"type":"custom","name":"apply_patch","description":"Apply a patch","format":{"type":"grammar","syntax":"lark","definition":"..."}}]});
    let (body, map) = convert_request(&req, Protocol::Responses, Protocol::Chat, "m").unwrap();
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["input"]["type"],
        "string"
    );
    let upstream = json!({"choices":[{"message":{"tool_calls":[{"id":"call_p","type":"function","function":{"name":"apply_patch","arguments":"{\"input\":\"*** Begin Patch\\n*** End Patch\"}"}}]},"finish_reason":"tool_calls"}]});
    let out = convert_response(&upstream, Protocol::Chat, Protocol::Responses, "m", &map).unwrap();
    assert_eq!(out["output"][0]["type"], "custom_tool_call");
    assert_eq!(out["output"][0]["input"], "*** Begin Patch\n*** End Patch");
}
#[test]
fn images_keep_their_payload_in_all_directions() {
    let mut body = json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,YWJj"}}]}]});
    let (mut source, mut next) = (Protocol::Chat, Protocol::Messages);
    for _ in 0..3 {
        let (out, _) = convert_request(&body, source, next, "m").unwrap();
        assert!(out.to_string().contains("YWJj"));
        body = out;
        source = next;
        next = if source == Protocol::Messages {
            Protocol::Responses
        } else {
            Protocol::Chat
        };
    }
}
#[test]
fn broken_streams_never_emit_success() {
    for target in PROTOCOLS {
        let mut bridge = Bridge::new(Protocol::Chat, target, "m", ToolMap::default());
        bridge.begin().unwrap();
        bridge
            .frame("", r#"{"choices":[{"delta":{"content":"partial"}}]}"#)
            .unwrap();
        assert!(bridge.finish().is_err());
        let error = bridge.error("network interrupted");
        assert!(!error.contains("response.completed"));
        assert!(!error.contains("[DONE]"));
        assert!(!error.contains("message_stop"));
    }
    let mut parser = SseParser::default();
    parser.push(b"data: {\"truncated\"").unwrap();
    assert!(!parser.clean_eof());
}

#[tokio::test]
async fn real_http_stream_conversion_does_not_buffer_until_eof() {
    use axum::{body::Body, response::IntoResponse, routing::post, Router};
    use std::time::Duration;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/",post(||async{
        let stream=futures_util::stream::unfold(0,|i|async move{match i{0=>Some((Ok::<_,std::io::Error>(axum::body::Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"early\"}}]}\n\n")),1)),1=>{tokio::time::sleep(Duration::from_secs(30)).await;None},_=>None}});
        ([("content-type","text/event-stream")],Body::from_stream(stream)).into_response()
    }))).await.unwrap();
    });
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    let stream = converted_stream(
        response,
        Protocol::Chat,
        Protocol::Responses,
        "m".into(),
        ToolMap::default(),
    );
    use futures_util::StreamExt;
    futures_util::pin_mut!(stream);
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !output.contains("early") {
            output += std::str::from_utf8(&stream.next().await.unwrap().unwrap()).unwrap();
        }
    })
    .await
    .unwrap();
    assert!(!output.contains("response.completed"));
    server.abort();
}

#[test]
fn interleaved_parallel_tools_keep_arguments_separate() {
    for target in [Protocol::Responses, Protocol::Messages] {
        let mut bridge = Bridge::new(Protocol::Chat, target, "m", ToolMap::default());
        bridge.begin().unwrap();
        let chunks = [
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"first","arguments":"{\"a\":"}},{"index":1,"id":"call_b","function":{"name":"second","arguments":"{\"b\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"2}"}},{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}),
        ];
        for chunk in chunks {
            bridge.frame("", &chunk.to_string()).unwrap();
        }
        let result = bridge.frame("", "[DONE]").unwrap();
        if target == Protocol::Responses {
            let mut parser = SseParser::default();
            let frames = parser.push(result.as_bytes()).unwrap();
            let value: Value = serde_json::from_str(&frames.last().unwrap().1).unwrap();
            assert_eq!(value["response"]["output"][0]["arguments"], "{\"a\":1}");
            assert_eq!(value["response"]["output"][1]["arguments"], "{\"b\":2}");
        }
        assert!(bridge.terminal);
    }
}

#[test]
fn token_limit_is_incomplete_and_invalid_tool_json_is_rejected() {
    let body = json!({"choices":[{"message":{"content":"partial"},"finish_reason":"length"}]});
    let result = convert_response(
        &body,
        Protocol::Chat,
        Protocol::Responses,
        "m",
        &ToolMap::default(),
    )
    .unwrap();
    assert_eq!(result["status"], "incomplete");
    assert_eq!(result["incomplete_details"]["reason"], "max_output_tokens");
    let mut bridge = Bridge::new(Protocol::Chat, Protocol::Responses, "m", ToolMap::default());
    bridge.begin().unwrap();
    bridge.frame("", &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"lookup","arguments":"{"}}]},"finish_reason":"length"}]}).to_string()).unwrap();
    assert!(bridge.frame("", "[DONE]").is_err());
    assert!(!bridge.terminal);
}

#[test]
fn messages_output_schema_is_not_silently_discarded() {
    let body = json!({"messages":[{"role":"user","content":"hello"}],"output_config":{"format":{"type":"json_schema","schema":{"type":"object"}}}});
    assert!(convert_request(&body, Protocol::Messages, Protocol::Chat, "m").is_err());
    assert!(convert_request(&body, Protocol::Messages, Protocol::Messages, "m").is_ok());
}

#[test]
fn output_limits_are_never_injected_and_explicit_limits_are_preserved() {
    for source in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
        for target in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            let body = if source == Protocol::Responses { json!({"model":"alias","input":"hello"}) }
                else { json!({"model":"alias","messages":[{"role":"user","content":"hello"}]}) };
            let (out, _) = convert_request(&body, source, target, "actual").unwrap();
            for key in ["max_tokens", "max_output_tokens", "max_completion_tokens"] {
                assert!(out.get(key).is_none(), "{source:?} -> {target:?}: {key}");
            }
            let mut explicit = body;
            let key = if source == Protocol::Responses { "max_output_tokens" } else { "max_tokens" };
            explicit[key] = 8192.into();
            let (out, _) = convert_request(&explicit, source, target, "actual").unwrap();
            assert_eq!(out[if target == Protocol::Responses { "max_output_tokens" } else { "max_tokens" }], 8192);
        }
    }
}

#[test]
fn pdf_attachments_round_trip_across_protocols() {
    let data = "data:application/pdf;base64,YWJj";
    let body = json!({"messages":[{"role":"user","content":[{"type":"file","file":{"filename":"report.pdf","file_data":data}}]}]});
    let (messages, _) = convert_request(&body, Protocol::Chat, Protocol::Messages, "m").unwrap();
    assert_eq!(messages["messages"][0]["content"][0]["source"]["data"], "YWJj");
    let (responses, _) = convert_request(&messages, Protocol::Messages, Protocol::Responses, "m").unwrap();
    assert_eq!(responses["input"][0]["content"][0]["file_data"], data);
    let (chat, _) = convert_request(&responses, Protocol::Responses, Protocol::Chat, "m").unwrap();
    assert_eq!(chat["messages"][0]["content"][0]["file"]["filename"], "report.pdf");
    assert_eq!(chat["messages"][0]["content"][0]["file"]["file_data"], data);
}

#[test]
fn namespaced_codex_tools_preserve_identity_history_and_custom_calls() {
    let request = json!({"input":[{"type":"function_call","call_id":"old","namespace":"files","name":"read","arguments":"{}"},{"type":"function_call_output","call_id":"old","output":"ok"}],
        "tools":[{"type":"namespace","name":"files","tools":[{"type":"function","name":"read","parameters":{"type":"object"}},{"type":"custom","name":"patch"}]},{"type":"namespace","name":"other","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]}],
        "tool_choice":{"type":"function","namespace":"files","name":"read"}});
    for target in [Protocol::Chat, Protocol::Messages] {
        let (body,map) = convert_request(&request, Protocol::Responses,target,"m").unwrap();
        let path = if target == Protocol::Chat { "/function/name" } else { "/name" };
        let names:Vec<_> = body["tools"].as_array().unwrap().iter().map(|t|t.pointer(path).unwrap().as_str().unwrap()).collect();
        assert_ne!(names[0],names[2]);
        assert!(names.iter().all(|n|n.len()<=64));
        assert!(body.to_string().contains(names[0]));
        let upstream=json!({"choices":[{"message":{"tool_calls":[{"id":"call1","type":"function","function":{"name":names[0],"arguments":"{}"}},{"id":"call2","type":"function","function":{"name":names[1],"arguments":"{\"input\":\"patch contents\"}"}}]},"finish_reason":"tool_calls"}]});
        let response=convert_response(&upstream,Protocol::Chat,Protocol::Responses,"m",&map).unwrap();
        assert_eq!(response["output"][0]["namespace"],"files");
        assert_eq!(response["output"][0]["name"],"read");
        assert_eq!(response["output"][1]["namespace"],"files");
        assert_eq!(response["output"][1]["type"],"custom_tool_call");
        assert_eq!(response["output"][1]["input"],"patch contents");
        let history=&body["messages"][0];
        assert!(history.to_string().contains(names[0]));
        assert!(body["tool_choice"].to_string().contains(names[0]));
    }
}

#[test]
fn namespaced_stream_restores_names_on_added_done_and_completed_items() {
    let request=json!({"tools":[{"type":"namespace","name":"functions","tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]}],"input":"hello"});
    let (body,map)=convert_request(&request,Protocol::Responses,Protocol::Chat,"m").unwrap();
    let alias=body["tools"][0]["function"]["name"].as_str().unwrap();
    let mut bridge=Bridge::new(Protocol::Chat,Protocol::Responses,"m",map);
    let mut output=bridge.begin().unwrap();
    for frame in frames(Protocol::Chat) {
        output+=&bridge.frame("",&frame.to_string().replace("lookup",alias)).unwrap();
    }
    output+=&bridge.frame("","[DONE]").unwrap();
    let mut parser=SseParser::default();
    let events=parser.push(output.as_bytes()).unwrap();
    for kind in ["response.output_item.added","response.output_item.done","response.completed"] {
        let items:Vec<Value>=events.iter().filter(|(event,_)| event==kind).map(|(_,data)|serde_json::from_str(data).unwrap()).collect();
        assert!(items.iter().any(|item|item.to_string().contains("\"namespace\":\"functions\"")),"{kind}");
        assert!(!items.iter().any(|item|item.to_string().contains(alias)));
    }
}

#[test]
fn sse_line_endings_work_across_every_http_split() {
    let source = "event: message\ndata: 你好\ndata:\n\ndata: [DONE]\n\n";
    let expected = vec![("message".into(), "你好\n".into()), (String::new(), "[DONE]".into())];
    let mixed = "event: message\r\ndata: 你好\rdata:\n\rdata: [DONE]\r\n\n";
    for wire in [source.into(), source.replace('\n', "\r\n"), source.replace('\n', "\r"), mixed.into()] {
        for split in 0..=wire.len() {
            let mut parser = SseParser::default();
            let mut frames = parser.push(&wire.as_bytes()[..split]).unwrap();
            frames.extend(parser.push(&wire.as_bytes()[split..]).unwrap());
            assert_eq!(frames, expected, "wire={wire:?}, split={split}");
            assert!(parser.clean_eof());
        }
    }
}
