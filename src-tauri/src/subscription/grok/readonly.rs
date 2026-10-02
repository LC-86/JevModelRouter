//! Manual account observation through the official CLI. This has no admission or persistence hooks.
use super::super::helper;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::watch,
};

const MAX_LINE: usize = 256 * 1024;
const MAX_LINES: usize = 256;
const DEADLINE: Duration = Duration::from_secs(75);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Field<T> {
    pub state: &'static str,
    pub value: Option<T>,
}
impl<T> Field<T> {
    fn unknown(state: &'static str) -> Self {
        Self { state, value: None }
    }
    fn available(value: T) -> Self {
        Self {
            state: "available",
            value: Some(value),
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Observation {
    pub observed_at: String,
    pub models: Field<Vec<String>>,
    pub current_model: Field<String>,
    pub usage_percent: Field<f64>,
    pub remaining_percent: Field<f64>,
    pub subscription_tier: Field<String>,
    pub period_type: Field<String>,
    pub period_start: Field<String>,
    pub period_end: Field<String>,
    pub billing_period_end: Field<String>,
    pub period_conflict: bool,
    pub real_generation_enabled: bool,
}
fn field<T>(parent: &Value, key: &str, parse: impl FnOnce(&Value) -> Option<T>) -> Field<T> {
    if !parent.is_object() {
        return Field::unknown("invalid");
    }
    match parent.get(key) {
        None => Field::unknown("missing"),
        Some(Value::Null) => Field::unknown("null"),
        Some(value) => parse(value)
            .map(Field::available)
            .unwrap_or_else(|| Field::unknown("invalid")),
    }
}
fn safe_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.len() <= 160 && !v.chars().any(char::is_control))
        .map(helper::redact)
}
fn timestamp(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.to_rfc3339())
}
fn project(models: &Value, billing: &Value) -> Observation {
    // The model method adds one extra result envelope; no other account fields cross the boundary.
    let model_result = &models["result"];
    let config = &billing["config"];
    let period = &config["currentPeriod"];
    let mut usage = field(config, "creditUsagePercent", Value::as_f64);
    if usage
        .value
        .is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
    {
        usage = Field::unknown("out_of_range");
    }
    let remaining = Field {
        state: usage.state,
        value: usage.value.map(|v| 100.0 - v),
    };
    let mut end = field(period, "end", timestamp);
    let billing_end = field(config, "billingPeriodEnd", timestamp);
    let conflict = match (&end.value, &billing_end.value) {
        (Some(a), Some(b)) => {
            chrono::DateTime::parse_from_rfc3339(a).ok()
                != chrono::DateTime::parse_from_rfc3339(b).ok()
        }
        _ => false,
    };
    if conflict {
        end = Field::unknown("conflict");
    }
    Observation {
        observed_at: chrono::Utc::now().to_rfc3339(),
        models: field(model_result, "availableModels", |value| {
            let entries = value.as_array()?;
            if entries.len() > 512 {
                return None;
            }
            // A malformed entry invalidates the catalog instead of presenting a partial whitelist.
            entries
                .iter()
                .map(|entry| safe_text(&entry["modelId"]))
                .collect()
        }),
        current_model: field(model_result, "currentModelId", safe_text),
        usage_percent: usage,
        remaining_percent: remaining,
        subscription_tier: field(billing, "subscription_tier", safe_text),
        period_type: field(period, "type", safe_text),
        period_start: field(period, "start", timestamp),
        period_end: end,
        billing_period_end: billing_end,
        period_conflict: conflict,
        real_generation_enabled: false,
    }
}

// One manual read at a time. Cancellation can arrive before its refresh is registered.
const CANCEL_RETENTION: Duration = Duration::from_secs(90);
const MAX_CANCELLED_IDS: usize = 128;
#[derive(Default)]
struct ReadState {
    active: Option<(String, watch::Sender<bool>)>,
    cancelled: std::collections::VecDeque<(String, std::time::Instant)>,
    overflow_until: Option<std::time::Instant>,
}
impl ReadState {
    fn prune(&mut self) {
        self.cancelled
            .retain(|(_, at)| at.elapsed() < CANCEL_RETENTION);
    }
    fn is_cancelled(&mut self, id: &str) -> bool {
        self.prune();
        self.overflow_until
            .is_some_and(|until| std::time::Instant::now() < until)
            || self.cancelled.iter().any(|(cancelled, _)| cancelled == id)
    }
    fn remember_cancel(&mut self, id: &str) {
        self.prune();
        self.cancelled.retain(|(cancelled, _)| cancelled != id);
        if self.cancelled.len() == MAX_CANCELLED_IDS {
            // Bounded storage must not revive an evicted cancellation. Refuse reads until expiry.
            self.overflow_until = Some(std::time::Instant::now() + CANCEL_RETENTION);
            self.cancelled.pop_front();
        }
        self.cancelled
            .push_back((id.into(), std::time::Instant::now()));
    }
}
static ACTIVE: OnceLock<Mutex<ReadState>> = OnceLock::new();
fn active() -> &'static Mutex<ReadState> {
    ACTIVE.get_or_init(|| Mutex::new(ReadState::default()))
}
struct Registration(String);
impl Drop for Registration {
    fn drop(&mut self) {
        let mut slot = active().lock().unwrap();
        if slot.active.as_ref().is_some_and(|(id, _)| id == &self.0) {
            slot.active = None;
        }
    }
}
pub(crate) async fn cancel(request_id: &str) {
    if request_id.is_empty() || request_id.len() > 80 {
        return;
    }
    {
        let mut slot = active().lock().unwrap();
        // The same lock protects cancellation and registration: neither can miss the other.
        slot.remember_cancel(request_id);
        if let Some((id, sender)) = slot.active.as_ref() {
            if id == request_id {
                let _ = sender.send(true);
            }
        }
    }
    // An unregistered request retains its cancellation; an owned process must finish cleanup.
    for _ in 0..700 {
        if !active()
            .lock()
            .unwrap()
            .active
            .as_ref()
            .is_some_and(|(id, _)| id == request_id)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
fn program() -> Result<PathBuf, String> {
    if crate::runtime::isolated() {
        return crate::runtime::grok_helper_override()
            .map(Path::to_path_buf)
            .ok_or_else(|| "helper_isolated: explicit pinned read-only fixture required".into());
    }
    // No command/env overrides or appended arguments. The installed CLI owns its cached auth.
    let home = dirs::home_dir().ok_or("helper_missing: user home unavailable")?;
    let installed = home.join(".local/bin/grok");
    if installed.is_file() {
        return Ok(installed);
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("grok"))
                .find(|p| p.is_file())
        })
        .ok_or_else(|| "helper_missing: install the official Grok CLI".into())
}
// Thread-local test resolver keeps the public command path offline without exposing overrides.
#[cfg(test)]
thread_local! {
    static TEST_RESOLVER: std::cell::RefCell<Option<Box<dyn FnOnce() -> Result<(PathBuf, PathBuf), String>>>> = Default::default();
}
fn resolve_helper() -> Result<(PathBuf, PathBuf), String> {
    #[cfg(test)]
    if let Some(resolver) = TEST_RESOLVER.with(|slot| slot.borrow_mut().take()) {
        return resolver();
    }
    let program = program()?;
    let home = if crate::runtime::isolated() {
        crate::runtime::home_dir()
    } else {
        dirs::home_dir()
    }
    .ok_or("helper_missing: home unavailable")?;
    Ok((program, home))
}
pub(crate) async fn refresh(request_id: String) -> Result<Observation, String> {
    if request_id.len() > 80 || request_id.is_empty() {
        return Err("invalid_request_id".into());
    }
    let entered = std::time::Instant::now();
    if active().lock().unwrap().is_cancelled(&request_id) {
        return Err("read_cancelled: Grok observation cancelled".into());
    }
    let (program, home) = resolve_helper()?;
    let (sender, receiver) = watch::channel(false);
    {
        let mut slot = active().lock().unwrap();
        if slot.is_cancelled(&request_id) {
            return Err("read_cancelled: Grok observation cancelled".into());
        }
        if slot.active.is_some() {
            return Err("read_busy: a Grok observation is already running".into());
        }
        slot.active = Some((request_id.clone(), sender));
    }
    let _registration = Registration(request_id);
    // An expired pre-registration read cannot outlive cancellation retention. Reserve cleanup.
    let budget = CANCEL_RETENTION
        .checked_sub(entered.elapsed() + Duration::from_secs(12))
        .filter(|budget| !budget.is_zero())
        .ok_or("helper_timeout: Grok read deadline exceeded")?;
    run(&program, &home, receiver, DEADLINE.min(budget)).await
}
async fn line(reader: &mut BufReader<tokio::process::ChildStdout>) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    loop {
        let bytes = reader
            .fill_buf()
            .await
            .map_err(|_| "helper_io: stdout read failed")?;
        if bytes.is_empty() {
            return Err("helper_exited: response stream ended".into());
        }
        let count = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(bytes.len());
        if output.len() + count > MAX_LINE {
            return Err("schema_error: response line too large".into());
        }
        output.extend_from_slice(&bytes[..count]);
        reader.consume(count);
        if output.last() == Some(&b'\n') {
            return Ok(output);
        }
    }
}
async fn rpc(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut BufReader<tokio::process::ChildStdout>,
    id: u32,
    method: &'static str,
    params: Value,
) -> Result<Value, String> {
    let mut request =
        serde_json::to_vec(&json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .unwrap();
    request.push(b'\n');
    stdin
        .write_all(&request)
        .await
        .map_err(|_| "helper_io: request write failed")?;
    stdin
        .flush()
        .await
        .map_err(|_| "helper_io: request flush failed")?;
    for _ in 0..MAX_LINES {
        let value: Value = serde_json::from_slice(&line(stdout).await?)
            .map_err(|_| "schema_error: invalid JSON-RPC")?;
        if value.get("id").and_then(Value::as_u64) != Some(id as u64) {
            // Notifications are ignored. Server requests and mismatched responses cannot be answered.
            if value.get("id").is_some() {
                return Err("schema_error: unexpected JSON-RPC id".into());
            }
            continue;
        }
        if value["jsonrpc"] != "2.0" {
            return Err("schema_error: invalid JSON-RPC version".into());
        }
        if let Some(error) = value.get("error") {
            // Only typed code and sanitized bounded message are retained; opaque data is discarded.
            let code = error["code"]
                .as_i64()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".into());
            // Messages/data may embed private account payloads. Expose only a stable category.
            let category = match code.as_str() {
                "401" => "authentication required in the official Grok CLI",
                "403" => "read access denied",
                "-32601" => "read method unavailable in this CLI",
                _ => "Grok read failed; check CLI authentication or network",
            };
            return Err(format!("rpc_error {code}: {category}"));
        }
        return value
            .get("result")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| "schema_error: missing object result".into());
    }
    Err("schema_error: too many notifications".into())
}
async fn cleanup(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // SIGTERM is sent only to the owned child, with its handle retained until wait completes.
        unsafe {
            kill(pid as i32, 15);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    }
}
async fn run(
    program: &Path,
    home: &Path,
    mut cancellation: watch::Receiver<bool>,
    deadline: Duration,
) -> Result<Observation, String> {
    if *cancellation.borrow() {
        return Err("read_cancelled: Grok observation cancelled".into());
    }
    let mut command = Command::new(program);
    command
        .args(["agent", "--no-leader", "stdio"])
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("HOME", home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    for key in ["PATH", "TMPDIR", "LANG"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|_| "helper_spawn: unable to start Grok CLI")?;
    let mut stdin = child.stdin.take().ok_or("helper_io: stdin unavailable")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("helper_io: stdout unavailable")?);
    let query = async {
        let initialized = rpc(&mut stdin, &mut stdout, 1, "initialize", json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"readonly-account-query","version":"1"}})).await?;
        if initialized["protocolVersion"] != 1 {
            return Err("schema_error: unsupported ACP protocol".into());
        }
        rpc(
            &mut stdin,
            &mut stdout,
            2,
            "authenticate",
            json!({"methodId":"cached_token"}),
        )
        .await?;
        let models = rpc(&mut stdin, &mut stdout, 3, "_x.ai/models/list", json!({})).await;
        // An explicit RPC error ends the read without retries or additional methods.
        let models = models?;
        let billing = rpc(&mut stdin, &mut stdout, 4, "_x.ai/billing", json!({})).await?;
        Ok(project(&models, &billing))
    };
    let result = tokio::select! {
        biased;
        _ = cancellation.changed() => Err("read_cancelled: Grok observation cancelled".into()),
        result = tokio::time::timeout(deadline, query) => result.unwrap_or_else(|_| Err("helper_timeout: Grok read deadline exceeded".into())),
    };
    drop(stdin);
    drop(stdout);
    cleanup(&mut child).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_is_minimal_and_never_grants_generation() {
        let result = project(
            &json!({"result":{"currentModelId":"grok-example", "availableModels":[{"modelId":"grok-example","secret":"discard"}]}}),
            &json!({"subscription_tier":"ExamplePlan", "config":{"creditUsagePercent":25,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2030-01-01T00:00:00Z","end":"2030-01-08T00:00:00Z"},"billingPeriodEnd":"2030-01-08T01:00:00+01:00","history":["discard"],"onDemandCap":{"val":50}}}),
        );
        assert_eq!(result.usage_percent.value, Some(25.0));
        assert_eq!(result.remaining_percent.value, Some(75.0));
        assert!(!result.period_conflict);
        assert!(!result.real_generation_enabled);
        let output = serde_json::to_string(&result).unwrap();
        assert!(!output.contains("discard"));
        assert!(!output.contains("onDemand"));
    }
    #[test]
    fn sparse_invalid_and_conflicting_values_stay_unknown() {
        for (raw, state) in [
            (json!({}), "missing"),
            (json!({"creditUsagePercent":null}), "null"),
            (json!({"creditUsagePercent":"25"}), "invalid"),
            (json!({"creditUsagePercent":101}), "out_of_range"),
        ] {
            let result = project(&json!({}), &json!({"config":raw}));
            assert_eq!(result.usage_percent.state, state);
            assert_eq!(result.remaining_percent.value, None);
        }
        let result = project(
            &json!({"result":{"availableModels":[{"modelId":null}]}}),
            &json!({"config":{"creditUsagePercent":0,"currentPeriod":{"end":"2030-01-08T00:00:00Z"},"billingPeriodEnd":"2030-02-01T00:00:00Z"}}),
        );
        assert_eq!(result.models.state, "invalid");
        assert_eq!(result.usage_percent.value, Some(0.0));
        assert!(result.period_conflict);
        assert_eq!(result.period_end.state, "conflict");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn public_refresh_cancelled_during_resolution_never_starts_child_or_rpc() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Barrier};
        // This is the only public-command fixture; restore its process-global cancellation cache.
        struct ResetState;
        impl Drop for ResetState {
            fn drop(&mut self) {
                *active().lock().unwrap() = ReadState::default();
            }
        }
        let _reset = ResetState;
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("fake-grok");
        let home = directory.path().to_path_buf();
        std::fs::write(
            &helper,
            r#"#!/usr/bin/env python3
import json,os,sys
open(os.path.join(os.environ['HOME'],'started'),'w').write('synthetic child')
for line in sys.stdin:
 r=json.loads(line)
 with open(os.path.join(os.environ['HOME'],'rpc'),'a') as f: f.write(r['method']+'\n')
 value={'protocolVersion':1} if r['method']=='initialize' else {}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':value}),flush=True)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let entered = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let id = "offline-public-precancel";
        let thread = {
            let entered = entered.clone();
            let resume = resume.clone();
            std::thread::spawn(move || {
                TEST_RESOLVER.with(|slot| {
                    *slot.borrow_mut() = Some(Box::new(move || {
                        entered.wait();
                        resume.wait();
                        Ok((helper, home))
                    }))
                });
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(refresh(id.into()))
            })
        };
        entered.wait();
        cancel(id).await;
        assert!(
            !directory.path().join("started").exists(),
            "no child at cancellation acknowledgement"
        );
        resume.wait();
        let result = tokio::task::spawn_blocking(move || thread.join().unwrap())
            .await
            .unwrap();
        assert!(
            result
                .as_ref()
                .is_err_and(|error| error.contains("read_cancelled")),
            "cancelled public refresh resumed: {result:?}; RPCs: {:?}",
            std::fs::read_to_string(directory.path().join("rpc"))
        );
        assert!(
            !directory.path().join("started").exists(),
            "cancelled refresh must not spawn"
        );
        assert!(
            !directory.path().join("rpc").exists(),
            "cancelled refresh must send no RPC"
        );
        let before_entry_id = "offline-public-cancel-before-entry";
        cancel(before_entry_id).await;
        TEST_RESOLVER.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|| {
                panic!("pre-cancelled read must not resolve the CLI")
            }))
        });
        let result = refresh(before_entry_id.into()).await;
        TEST_RESOLVER.with(|slot| *slot.borrow_mut() = None);
        assert!(result.is_err_and(|error| error.contains("read_cancelled")));
        assert!(!directory.path().join("started").exists());
        assert!(!directory.path().join("rpc").exists());
        // Saturating the bounded cache cannot make an evicted request readable again.
        for index in 0..=MAX_CANCELLED_IDS {
            cancel(&format!("offline-cancel-{index}")).await;
        }
        let result = refresh("offline-cancel-0".into()).await;
        assert!(result.is_err_and(|error| error.contains("read_cancelled")));
        assert!(!directory.path().join("started").exists());
        assert!(!directory.path().join("rpc").exists());
    }
    #[cfg(unix)]
    async fn fixture(
        mode: &str,
        timeout: Duration,
        cancel: bool,
    ) -> (Result<Observation, String>, tempfile::TempDir) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fake-grok");
        // Synthetic stdio server. No Grok binary or network is used.
        let script = format!(
            r#"#!/usr/bin/env python3
import json,sys,os,time
assert sys.argv[1:] == ['agent','--no-leader','stdio']
open(os.path.join(os.environ['HOME'],'pid'),'w').write(str(os.getpid()))
for line in sys.stdin:
 r=json.loads(line); m=r['method']; i=r['id']
 assert m in ['initialize','authenticate','_x.ai/models/list','_x.ai/billing']
 if m=='initialize': value={{'protocolVersion':2 if '{mode}'=='schema' else 1}}
 elif m=='authenticate':
  assert r['params']=={{'methodId':'cached_token'}}
  value={{}}
 elif m=='_x.ai/models/list':
  if '{mode}'=='timeout': time.sleep(30)
  if '{mode}' in ['401','network']:
   print(json.dumps({{'jsonrpc':'2.0','id':i,'error':{{'code':401 if '{mode}'=='401' else -32000,'message':'Unauthorized token=fixture-secret','data':{{'secret':'discard'}}}}}}),flush=True);continue
  value={{'result':{{'currentModelId':'grok-example','availableModels':[{{'modelId':'grok-example'}}]}}}}
 else: value={{'config':{{'creditUsagePercent':25}},'subscription_tier':'ExamplePlan'}}
 print(json.dumps({{'jsonrpc':'2.0','id':i,'result':value}}),flush=True)
"#
        );
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (sender, receiver) = watch::channel(false);
        if cancel {
            let ready = dir.path().join("pid");
            tokio::spawn(async move {
                for _ in 0..1000 {
                    if ready.exists() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let _ = sender.send(true);
            });
        }
        // Keep the sender alive for successful/non-cancelled reads.
        else {
            let result = run(&file, dir.path(), receiver, timeout).await;
            drop(sender);
            return (result, dir);
        }
        (run(&file, dir.path(), receiver, timeout).await, dir)
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn offline_wire_success_401_timeout_cancel_and_child_cleanup() {
        for (mode, cancel, expected) in [
            ("ok", false, ""),
            ("401", false, "rpc_error 401"),
            ("network", false, "rpc_error -32000"),
            ("schema", false, "schema_error"),
            ("timeout", false, "helper_timeout"),
            ("timeout", true, "read_cancelled"),
        ] {
            let (result, dir) = fixture(
                mode,
                Duration::from_secs(if mode == "timeout" { 12 } else { 5 }),
                cancel,
            )
            .await;
            if expected.is_empty() {
                assert_eq!(result.unwrap().usage_percent.value, Some(25.0));
            } else {
                let err = result.unwrap_err();
                assert!(err.contains(expected), "{err}");
                assert!(!err.contains("fixture-secret"));
                assert!(!err.contains("discard"));
            }
            let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap_or_else(|error| {
                panic!("mode={mode}, cancel={cancel}, pid missing: {error}")
            });
            let status = std::process::Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "owned fixture child must be reaped");
        }
    }
}
