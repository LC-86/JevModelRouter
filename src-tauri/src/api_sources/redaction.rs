/// Redact this request's credential before protocol parsing, logs or downstream output.
enum RedactionBuffer {
    Json(Vec<u8>),
    Sse(SseRedactor),
}

fn redact_value(value: &mut serde_json::Value, key: &str) {
    match value {
        serde_json::Value::String(text) => *text = text.replace(key, "[REDACTED]"),
        serde_json::Value::Array(items) => {
            for item in items {
                redact_value(item, key);
            }
        }
        serde_json::Value::Object(fields) => {
            let previous = std::mem::take(fields);
            for (name, mut value) in previous {
                redact_value(&mut value, key);
                fields.insert(name.replace(key, "[REDACTED]"), value);
            }
        }
        _ => {}
    }
}

fn redact_json(bytes: &[u8], key: &str) -> Vec<u8> {
    if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        redact_value(&mut value, key);
        serde_json::to_vec(&value).expect("JSON value serializes")
    } else {
        String::from_utf8_lossy(bytes)
            .replace(key, "[REDACTED]")
            .into_bytes()
    }
}

struct Frame {
    text: String,
    data: String,
    value: Option<serde_json::Value>,
}

impl Frame {
    fn parse(bytes: &[u8], key: &str) -> std::io::Result<Self> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| std::io::Error::other("Invalid upstream event stream"))?;
        let data = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .map(|value| value.strip_prefix(' ').unwrap_or(value))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut value: Option<serde_json::Value> = serde_json::from_str(&data).ok();
        if let Some(value) = &mut value {
            if value["type"].is_null() && value.is_object() {
                if let Some(event) = text.lines().find_map(|line| line.strip_prefix("event:")) {
                    value["type"] = event.trim_start().into();
                }
            }
            redact_value(value, key);
        }
        Ok(Self {
            text: text.into(),
            data,
            value,
        })
    }

    fn encode(&self, key: &str) -> Vec<u8> {
        let safe = self
            .value
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| self.data.replace(key, "[REDACTED]"));
        let mut output = String::new();
        let mut wrote_data = false;
        for line in self.text.lines().filter(|line| !line.is_empty()) {
            if line.starts_with("data:") {
                if wrote_data {
                    continue;
                }
                output.push_str("data: ");
                output.push_str(&safe);
                wrote_data = true;
            } else {
                output.push_str(&line.replace(key, "[REDACTED]"));
            }
            output.push('\n');
        }
        output.push('\n');
        output.into_bytes()
    }

    fn terminal(&self) -> bool {
        self.data == "[DONE]"
            || self.value.as_ref().is_some_and(|value| {
                matches!(
                    value["type"].as_str(),
                    Some(
                        "message_stop"
                            | "response.completed"
                            | "response.incomplete"
                            | "response.failed"
                    )
                )
            })
            || self
                .text
                .lines()
                .any(|line| line.trim() == "event: message_stop")
    }
}

#[derive(Default)]
struct SseRedactor {
    parser: crate::protocol::SseParser,
    pending: Vec<Frame>,
}

struct Span {
    frame: usize,
    pointer: String,
    start: usize,
    end: usize,
}
#[derive(Default)]
struct Channel {
    text: String,
    spans: Vec<Span>,
}

/// Match the append-only fields consumed by the existing protocol bridge and Debug.
fn delta_fields(value: &serde_json::Value) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    if let Some(choices) = value["choices"].as_array() {
        for (i, choice) in choices.iter().enumerate() {
            for name in ["content", "refusal", "reasoning_content", "reasoning"] {
                let channel = if name.starts_with("reasoning") {
                    "reasoning"
                } else {
                    "text"
                };
                fields.push((channel.into(), format!("/choices/{i}/delta/{name}")));
            }
            if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
                for (j, call) in calls.iter().enumerate() {
                    fields.push((
                        format!("tool:{}", call["index"]),
                        format!("/choices/{i}/delta/tool_calls/{j}/function/arguments"),
                    ));
                }
            }
        }
    }
    match value["type"].as_str().unwrap_or("") {
        "content_block_start" if value["content_block"]["type"] == "text" => {
            fields.push(("text".into(), "/content_block/text".into()))
        }
        "content_block_delta" => {
            for (name, channel) in [
                ("text", "text".into()),
                ("thinking", "reasoning".into()),
                ("partial_json", format!("tool:{}", value["index"])),
            ] {
                fields.push((channel, format!("/delta/{name}")));
            }
        }
        "response.output_text.delta" | "response.refusal.delta" => {
            fields.push(("text".into(), "/delta".into()))
        }
        "response.function_call_arguments.delta" => {
            fields.push((format!("tool:{}", value["output_index"]), "/delta".into()))
        }
        kind if kind.starts_with("response.reasoning") => {
            fields.push(("reasoning".into(), "/delta".into()))
        }
        _ => {}
    }
    fields
}

fn channels(frames: &[Frame]) -> std::collections::BTreeMap<String, Channel> {
    let mut channels: std::collections::BTreeMap<String, Channel> = Default::default();
    for (frame, item) in frames.iter().enumerate() {
        let Some(value) = &item.value else { continue };
        for (name, pointer) in delta_fields(value) {
            let Some(text) = value.pointer(&pointer).and_then(|v| v.as_str()) else {
                continue;
            };
            let channel = channels.entry(name).or_default();
            let start = channel.text.len();
            channel.text.push_str(text);
            channel.spans.push(Span {
                frame,
                pointer,
                start,
                end: channel.text.len(),
            });
        }
    }
    channels
}

impl SseRedactor {
    fn push(&mut self, bytes: &[u8], key: &str) -> std::io::Result<Vec<u8>> {
        let frames = self
            .parser
            .push_raw(bytes)
            .map_err(|_| std::io::Error::other("Invalid upstream event stream"))?;
        for bytes in frames {
            self.pending.push(Frame::parse(&bytes, key)?);
        }
        if self
            .pending
            .iter()
            .map(|frame| frame.text.len())
            .sum::<usize>()
            > 16 * 1024 * 1024
        {
            return Err(std::io::Error::other("Upstream event stream is too large"));
        }
        // Keep original frame order: a held text prefix must precede its block stop.
        let mut edits: std::collections::BTreeMap<(usize, String), Vec<(usize, usize, String)>> =
            Default::default();
        for channel in channels(&self.pending).values() {
            for (start, _) in channel.text.match_indices(key) {
                let end = start + key.len();
                let mut first = true;
                for span in &channel.spans {
                    if span.start < end && span.end > start {
                        edits
                            .entry((span.frame, span.pointer.clone()))
                            .or_default()
                            .push((
                                start.saturating_sub(span.start),
                                end.min(span.end) - span.start,
                                if first {
                                    "[REDACTED]".into()
                                } else {
                                    String::new()
                                },
                            ));
                        first = false;
                    }
                }
            }
        }
        for ((frame, pointer), edits) in edits {
            let value = self.pending[frame]
                .value
                .as_mut()
                .unwrap()
                .pointer_mut(&pointer)
                .unwrap();
            let mut text = value.as_str().unwrap().to_owned();
            for (start, end, safe) in edits.into_iter().rev() {
                text.replace_range(start..end, &safe);
            }
            *value = text.into();
        }
        let terminal = self.pending.iter().any(Frame::terminal);
        let mut ready = self.pending.len();
        if !terminal {
            for channel in channels(&self.pending).values() {
                let suffix = (1..key.len().min(channel.text.len() + 1))
                    .rev()
                    .find(|&length| {
                        channel.text.as_bytes()[channel.text.len() - length..]
                            == key.as_bytes()[..length]
                    })
                    .unwrap_or(0);
                if suffix > 0 {
                    let start = channel.text.len() - suffix;
                    if let Some(span) = channel.spans.iter().find(|span| span.end > start) {
                        ready = ready.min(span.frame);
                    }
                }
            }
        }
        Ok(self.drain(ready, key))
    }

    fn drain(&mut self, count: usize, key: &str) -> Vec<u8> {
        self.pending
            .drain(..count)
            .flat_map(|frame| frame.encode(key))
            .collect()
    }

    fn finish(&mut self, key: &str) -> std::io::Result<Vec<u8>> {
        if !self.parser.clean_eof() {
            return Err(std::io::Error::other("Incomplete upstream event stream"));
        }
        Ok(self.drain(self.pending.len(), key))
    }
}

/// Redact this request's credential before protocol parsing, logs or downstream output.
/// Decode JSON before redaction so escaped credentials cannot reappear after parsing.
pub fn redacted_stream<S, E>(
    input: S,
    secret: Option<String>,
    is_sse: bool,
) -> impl futures_util::Stream<Item = std::result::Result<axum::body::Bytes, std::io::Error>> + Send
where
    S: futures_util::Stream<Item = std::result::Result<axum::body::Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    use futures_util::StreamExt;
    let key = secret.unwrap_or_default();
    let buffer = if is_sse {
        RedactionBuffer::Sse(Default::default())
    } else {
        RedactionBuffer::Json(Vec::new())
    };
    futures_util::stream::unfold(
        (Box::pin(input), key, buffer, false),
        |(mut stream, key, mut buffer, mut ended)| async move {
            loop {
                if ended {
                    return None;
                }
                let next = stream.next().await;
                if key.is_empty() {
                    return next.map(|chunk| {
                        (
                            chunk.map_err(|_| {
                                std::io::Error::other("Upstream connection interrupted")
                            }),
                            (stream, key, buffer, false),
                        )
                    });
                }
                let output: std::io::Result<Option<Vec<u8>>> = match next {
                    Some(Err(_)) => Err(std::io::Error::other("Upstream connection interrupted")),
                    Some(Ok(chunk)) => match &mut buffer {
                        RedactionBuffer::Json(bytes) => {
                            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                                Err(std::io::Error::other("Upstream JSON is too large"))
                            } else {
                                bytes.extend_from_slice(&chunk);
                                Ok(Some(Vec::new()))
                            }
                        }
                        RedactionBuffer::Sse(redactor) => redactor.push(&chunk, &key).map(Some),
                    },
                    None => {
                        ended = true;
                        match &mut buffer {
                            RedactionBuffer::Json(bytes) => Ok(Some(redact_json(bytes, &key))),
                            RedactionBuffer::Sse(redactor) => redactor.finish(&key).map(Some),
                        }
                    }
                };
                match output {
                    Err(error) => return Some((Err(error), (stream, key, buffer, true))),
                    Ok(Some(bytes)) => {
                        return Some((
                            Ok(axum::body::Bytes::from(bytes)),
                            (stream, key, buffer, ended),
                        ))
                    }
                    _ => {}
                }
            }
        },
    )
}
