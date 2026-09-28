use std::{collections::HashMap, sync::{Mutex, OnceLock}};
use serde_json::{json, Value};
use tauri::State;
use crate::AppState;

type Cancellations = Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>;
fn cancellations() -> &'static Cancellations { static MAP: OnceLock<Cancellations> = OnceLock::new(); MAP.get_or_init(Default::default) }

#[tauri::command]
pub fn cancel_debug_curl(id: String) {
    if let Some(cancel) = cancellations().lock().unwrap().remove(&id) { let _ = cancel.send(()); }
}

#[tauri::command]
pub async fn debug_curl(state: State<'_, AppState>, id: String, endpoint: String, body: Value, headers: HashMap<String, String>, on_progress: tauri::ipc::Channel<Value>) -> Result<Value, String> {
    if !matches!(endpoint.as_str(), "chat/completions" | "responses" | "messages") || !body.is_object() { return Err("Invalid API request".into()); }
    if serde_json::to_vec(&body).map_err(|e|e.to_string())?.len() > 24 * 1024 * 1024 { return Err("Request exceeds 24 MB".into()); }
    if state.proxy.lock().await.is_none() { return Err("Start the local proxy before testing".into()); }
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    {
        let mut requests = cancellations().lock().unwrap();
        if requests.contains_key(&id) { return Err("Request is already running".into()); }
        requests.insert(id.clone(), cancel);
    }
    let _ = on_progress.send(json!({"started":true}));
    let operation = async {
        let start = std::time::Instant::now();
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(90)).build().map_err(|e|e.to_string())?;
        let mut request = client.post(format!("http://127.0.0.1:{}/v1/{endpoint}", state.store.read().port));
        for (name, value) in headers {
            if matches!(name.to_ascii_lowercase().as_str(), "host" | "content-length" | "transfer-encoding" | "connection") { return Err(format!("Unsupported header: {name}")); }
            request = request.header(name, value);
        }
        let mut response = request.header("user-agent", "AutoJev/Debug").json(&body).send().await.map_err(|e|e.to_string())?;
        let status = response.status().as_u16();
        let request_id = response.headers().get("x-autojev-request-id").and_then(|v|v.to_str().ok()).unwrap_or("").to_owned();
        let response_headers: HashMap<_,_> = response.headers().iter().map(|(k,v)|(k.to_string(),v.to_str().unwrap_or("").to_owned())).collect();
        let _ = on_progress.send(json!({"status":status,"headers":response_headers}));
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e|e.to_string())? {
            if bytes.len()+chunk.len() > 4*1024*1024 { return Err("Response exceeds 4 MB".into()); }
            let _ = on_progress.send(json!({"bytes":chunk.to_vec()}));
            bytes.extend_from_slice(&chunk);
        }
        let elapsed_ms = start.elapsed().as_millis();
        let store = state.store.clone();
        let telemetry = tauri::async_runtime::spawn_blocking(move || {
            for _ in 0..5 {
                if let Ok(Some(log)) = store.request_log(&request_id) { return Some(log); }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            None
        }).await.unwrap_or(None);
        Ok(json!({"status":status,"elapsed_ms":elapsed_ms,"body":String::from_utf8_lossy(&bytes),"headers":response_headers,"telemetry":telemetry}))
    };
    let result = tokio::select! { result = operation => result, _ = cancelled => Err("Request stopped".into()) };
    cancellations().lock().unwrap().remove(&id);
    result
}
