//! Portable text/image/function-tool subset of the three agent HTTP protocols.
//! Identical protocols bypass this module, preserving provider-specific extensions.
mod request;
mod response;
mod stream;
#[cfg(test)]
pub(crate) mod tests;

use crate::config::{Model, Provider};
use anyhow::{bail, Result};
pub use request::{convert_request, ToolMap};
pub use response::convert_response;
use serde_json::{json, Value};
#[cfg(test)]
pub use stream::converted_stream;
pub use stream::converted_stream_observed;
pub use stream::{collect_debug_stream, DebugProgress};
pub(crate) use stream::SseParser;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Chat,
    Responses,
    Messages,
}
impl Protocol {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "" | "chat_completions" | "chat/completions" => Ok(Self::Chat),
            "responses" => Ok(Self::Responses),
            "messages" => Ok(Self::Messages),
            _ => bail!("Unsupported API protocol: {value}"),
        }
    }
    pub fn upstream(model: &Model, provider: &Provider) -> Result<Self> {
        Self::parse(if model.api_type.is_empty() {
            &provider.api_type
        } else {
            &model.api_type
        })
    }
    pub fn path(self) -> &'static str {
        match self {
            Self::Chat => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
            Self::Messages => "/v1/messages",
        }
    }
    pub fn error(self, message: &str) -> Value {
        match self {
            Self::Messages => {
                json!({"type":"error","error":{"type":"api_error","message":message}})
            }
            _ => {
                json!({"error":{"type":"autojev_proxy_error","message":message,"code":"protocol_error"}})
            }
        }
    }
}

pub(super) fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
pub(super) fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
pub(super) fn id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
