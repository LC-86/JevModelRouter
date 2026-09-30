//! Native-webview acceptance driver, compiled only for the explicit check build.
use tauri::Manager;

pub fn script() -> anyhow::Result<Option<String>> {
    let args: Vec<_> = std::env::args().collect();
    let Some(index) = args.iter().position(|s| s == "--autojev-ui-check") else {
        return Ok(None);
    };
    anyhow::ensure!(
        crate::runtime::isolated(),
        "Desktop check requires --autojev-isolated"
    );
    let url = reqwest::Url::parse(
        args.get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("Missing loopback fixture URL"))?,
    )?;
    crate::dispatch::ensure_loopback(&url)?;
    let config = serde_json::json!({"base": url.to_string().trim_end_matches('/'), "reload": args.iter().any(|s| s == "--autojev-check-reload")});
    Ok(Some(format!(
        "window.__ISOLATION_CHECK__ = {config};\n{}",
        include_str!("isolation-check.js")
    )))
}

#[tauri::command]
pub async fn isolation_check_report(
    app: tauri::AppHandle,
    report: serde_json::Value,
) -> Result<(), String> {
    if !crate::runtime::isolated() {
        return Err("Desktop check requires isolation".into());
    }
    let root = crate::runtime::home_dir().unwrap();
    let state = app.state::<crate::AppState>();
    crate::safe_stop(&state).await?;
    std::fs::write(
        root.join("isolation-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    crate::EXIT_READY.store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(if report["ok"] == true { 0 } else { 1 });
    Ok(())
}
