//! Typed, read-only JSON-RPC surface for the candidate Grok ACP extensions.
//!
//! This transport deliberately has no process launcher. The public-source mapping for the
//! installed CLI is not verified, so production must not start ACP or read local auth state.
//! Tests exercise the wire framing with an in-memory fake; only a future reviewed source pin
//! may connect this client to a helper process.

use std::io::{BufRead, Write};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

const MAX_RPC_LINE_BYTES: usize = 1024 * 1024;

/// These four candidate extension methods are the complete read-only request allowlist.
/// Callers cannot supply a free-form JSON-RPC method name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadMethod {
    AuthInfo,
    ModelsList,
    Billing,
    AutoTopupRule,
}

impl ReadMethod {
    fn wire_name(self) -> &'static str {
        match self {
            Self::AuthInfo => "x.ai/auth/info",
            Self::ModelsList => "x.ai/models/list",
            Self::Billing => "x.ai/billing",
            Self::AutoTopupRule => "x.ai/auto-topup-rule",
        }
    }
}

#[derive(Clone, Copy)]
enum RequestMethod {
    Initialize,
    Read(ReadMethod),
}

impl RequestMethod {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::Read(method) => method.wire_name(),
        }
    }

    fn params(self) -> Value {
        match self {
            Self::Initialize => json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "JevModelRouter",
                    "title": "JevModelRouter read-only adapter",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            Self::Read(_) => json!({}),
        }
    }
}

/// A bounded candidate result. Values stay transient and are never written to config or quota
/// evidence by this module.
#[derive(Debug, PartialEq)]
pub(crate) struct CandidateReadSnapshot {
    pub auth_info: Value,
    pub models: Value,
    pub billing: Value,
    pub auto_topup_rule: Value,
}

/// JSON-RPC line transport over an already-owned ACP stream. It cannot spawn, authenticate,
/// create sessions, prompt, or issue any request beyond `initialize` plus [`ReadMethod`].
pub(crate) struct JsonRpcReadOnly<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
    initialized: bool,
}

impl<R: BufRead, W: Write> JsonRpcReadOnly<R, W> {
    pub(crate) fn new(reader: R, writer: W) -> Self {
        Self { reader, writer, next_id: 0, initialized: false }
    }

    pub(crate) fn read_snapshot(&mut self) -> Result<CandidateReadSnapshot> {
        self.initialize()?;
        Ok(CandidateReadSnapshot {
            auth_info: self.call(ReadMethod::AuthInfo)?,
            models: self.call(ReadMethod::ModelsList)?,
            billing: self.call(ReadMethod::Billing)?,
            auto_topup_rule: self.call(ReadMethod::AutoTopupRule)?,
        })
    }

    fn initialize(&mut self) -> Result<()> {
        let result = self.request(RequestMethod::Initialize)?;
        if result.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            bail!("Grok ACP did not negotiate protocol version 1");
        }
        self.initialized = true;
        Ok(())
    }

    fn call(&mut self, method: ReadMethod) -> Result<Value> {
        if !self.initialized {
            bail!("Grok ACP read-only request before initialization");
        }
        self.request(RequestMethod::Read(method))
    }

    fn request(&mut self, method: RequestMethod) -> Result<Value> {
        self.next_id = self.next_id.checked_add(1).context("Grok ACP request id exhausted")?;
        let id = self.next_id;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method.wire_name(),
            "params": method.params(),
        });
        serde_json::to_writer(&mut self.writer, &request).context("Encode Grok ACP request")?;
        self.writer.write_all(b"\n").context("Write Grok ACP request")?;
        self.writer.flush().context("Flush Grok ACP request")?;

        let mut line = String::new();
        loop {
            line.clear();
            let count = self.reader.read_line(&mut line).context("Read Grok ACP response")?;
            if count == 0 {
                bail!("Grok ACP ended before completing `{}`", method.wire_name());
            }
            if count > MAX_RPC_LINE_BYTES {
                bail!("Grok ACP response exceeded the one-megabyte line limit");
            }
            let message: Value = serde_json::from_str(&line).context("Grok ACP returned invalid JSON-RPC")?;
            if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                bail!("Grok ACP returned a non-JSON-RPC 2.0 message");
            }
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if message.get("error").is_some() {
                    // Do not forward upstream error payloads; they may contain account data.
                    bail!("Grok ACP rejected `{}`", method.wire_name());
                }
                return message.get("result").cloned().context("Grok ACP response had no result");
            }
            if message.get("id").is_some() {
                // An unsolicited server request would require a handler; fail closed instead of
                // accidentally granting filesystem, terminal, or permission access.
                bail!("Grok ACP sent an unexpected request or response id");
            }
            // ACP notifications are one-way. Ignore them while awaiting our correlated response.
        }
    }
}

/// The release build deliberately contains no verified Grok source pin yet. UI state is derived
/// here, with no frontend parameter that could override the gate.
const VERIFIED_SOURCE_PIN: Option<SourcePin> = None;

#[derive(Clone, Copy)]
struct SourcePin {
    cli_version: &'static str,
    package_git_head: &'static str,
    public_source_commit: &'static str,
}

fn source_matches(pin: Option<SourcePin>, cli_version: Option<&str>, package_git_head: Option<&str>) -> bool {
    pin.is_some_and(|pin| {
        cli_version == Some(pin.cli_version)
            && package_git_head == Some(pin.package_git_head)
            && pin.package_git_head == pin.public_source_commit
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiStatus {
    pub state: &'static str,
    pub source_verified: bool,
    pub source_version: Option<&'static str>,
    pub source_commit: Option<&'static str>,
    pub auth_info: &'static str,
    pub model_catalog: &'static str,
    pub billing: &'static str,
    pub auto_topup_rule: &'static str,
    pub extra_usage_permission: &'static str,
    pub real_generation_enabled: bool,
}

pub(crate) fn ui_status() -> UiStatus {
    let source_verified = source_matches(VERIFIED_SOURCE_PIN, None, None);
    UiStatus {
        state: if source_verified { "verified_source" } else { "blocked_unverified_source" },
        source_verified,
        source_version: VERIFIED_SOURCE_PIN.map(|pin| pin.cli_version),
        source_commit: VERIFIED_SOURCE_PIN.map(|pin| pin.public_source_commit),
        auth_info: "unknown",
        model_catalog: "unknown",
        billing: "unknown",
        auto_topup_rule: "unknown",
        extra_usage_permission: "unknown",
        real_generation_enabled: false,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use serde_json::Value;

    use super::{source_matches, ui_status, JsonRpcReadOnly, SourcePin};

    #[test]
    fn fake_acp_exercises_only_initialize_and_four_fixed_read_methods() {
        let responses = [
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"status":"current_or_expired"}}"#,
            r#"{"jsonrpc":"2.0","id":3,"result":{"models":[{"id":"fixture-model"}]}}"#,
            r#"{"jsonrpc":"2.0","id":4,"result":{"subscription_tier":"fixture"}}"#,
            r#"{"jsonrpc":"2.0","id":5,"result":{"rule":{"enabled":false}}}"#,
        ].join("\n");
        let mut client = JsonRpcReadOnly::new(BufReader::new(Cursor::new(responses)), Vec::new());

        let snapshot = client.read_snapshot().expect("fake ACP should return all four candidate methods");
        let sent: Vec<Value> = String::from_utf8(client.writer)
            .expect("requests are UTF-8 JSON lines")
            .lines()
            .map(|line| serde_json::from_str(line).expect("each request is JSON"))
            .collect();
        let methods: Vec<&str> = sent.iter().map(|request| request["method"].as_str().unwrap()).collect();

        assert_eq!(methods, ["initialize", "x.ai/auth/info", "x.ai/models/list", "x.ai/billing", "x.ai/auto-topup-rule"]);
        assert!(sent.iter().all(|request| request["jsonrpc"] == "2.0"));
        assert!(sent.iter().all(|request| request.get("params").is_some()));
        assert_eq!(snapshot.auth_info["status"], "current_or_expired");
        assert_eq!(snapshot.models["models"][0]["id"], "fixture-model");
        assert_eq!(snapshot.billing["subscription_tier"], "fixture");
        assert_eq!(snapshot.auto_topup_rule["rule"]["enabled"], false);
    }

    #[test]
    fn errors_and_protocol_version_mismatch_fail_closed() {
        let mut version_mismatch = JsonRpcReadOnly::new(
            BufReader::new(Cursor::new(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2}}"#)),
            Vec::new(),
        );
        assert!(version_mismatch.read_snapshot().is_err());

        let responses = [
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#,
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"private account detail"}}"#,
        ].join("\n");
        let mut rejected = JsonRpcReadOnly::new(BufReader::new(Cursor::new(responses)), Vec::new());
        let error = rejected.read_snapshot().expect_err("ACP errors must not become empty success values").to_string();
        assert!(!error.contains("private account detail"), "upstream error data must not be surfaced");
    }

    #[test]
    fn production_status_is_unknown_and_cannot_be_opened_by_a_ui_toggle() {
        let status = ui_status();
        assert_eq!(status.state, "blocked_unverified_source");
        assert!(!status.source_verified);
        assert!(status.source_version.is_none());
        assert!(status.source_commit.is_none());
        assert_eq!(status.auth_info, "unknown");
        assert_eq!(status.model_catalog, "unknown");
        assert_eq!(status.billing, "unknown");
        assert_eq!(status.auto_topup_rule, "unknown");
        assert_eq!(status.extra_usage_permission, "unknown");
        assert!(!status.real_generation_enabled);
    }

    #[test]
    fn a_source_pin_requires_exact_version_and_package_commit_match() {
        let pin = SourcePin {
            cli_version: "1.0.45",
            package_git_head: "abc123",
            public_source_commit: "abc123",
        };
        assert!(source_matches(Some(pin), Some("1.0.45"), Some("abc123")));
        assert!(!source_matches(Some(pin), Some("1.0.44"), Some("abc123")));
        assert!(!source_matches(Some(pin), Some("1.0.45"), Some("different")));
        assert!(!source_matches(None, Some("1.0.45"), Some("abc123")));
    }
}
