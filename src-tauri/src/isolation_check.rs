//! Native-webview acceptance driver, compiled only for the explicit check build.
use anyhow::Context;
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
    // Optional login-lifecycle mode: the acceptance driver only rehearses one scenario per run.
    let login_mode = args
        .iter()
        .position(|s| s == "--autojev-login-check")
        .and_then(|index| args.get(index + 1))
        .cloned();
    if let Some(mode) = &login_mode {
        anyhow::ensure!(
            matches!(mode.as_str(), "success" | "late" | "failed" | "grok"),
            "Unknown login check mode: {mode}"
        );
    }
    let root = crate::runtime::home_dir()
        .context("Isolation requires a home directory")?
        .to_string_lossy()
        .to_string();
    let config = serde_json::json!({"base": url.to_string().trim_end_matches('/'), "reload": args.iter().any(|s| s == "--autojev-check-reload"), "loginMode": login_mode, "root": root});
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
