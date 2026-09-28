use serde_json::Value;

fn validate(status: reqwest::StatusCode, body: &[u8], key: Option<&str>) -> Result<(), String> {
    let parsed = serde_json::from_slice::<Value>(body).ok();
    let api_error = parsed.as_ref().is_some_and(|v| {
        v.get("error").is_some_and(|e| !e.is_null()) || v["type"] == "error" || v["status"] == "failed"
    });
    let text = |value: &Value| value.as_str().is_some_and(|text| !text.trim().is_empty());
    let content_text = |value: &Value| value.as_array().is_some_and(|parts| parts.iter().any(|part| text(&part["text"])));
    if status.is_success() && !api_error && parsed.as_ref().is_some_and(|v| {
        v["choices"].as_array().is_some_and(|choices| choices.iter().any(|choice| text(&choice["message"]["content"]) || content_text(&choice["message"]["content"])))
            || content_text(&v["content"])
            || v["output"].as_array().is_some_and(|items| items.iter().any(|item| item["type"] == "message" && content_text(&item["content"])))
    }) { return Ok(()); }
    let mut detail = match parsed {
        Some(v) => serde_json::to_string_pretty(v.get("error").filter(|e| !e.is_null()).unwrap_or(&v)).unwrap_or_default(),
        None => String::from_utf8_lossy(body).into_owned(),
    };
    if let Some(key) = key.map(str::trim).filter(|k| !k.is_empty()) { detail = detail.replace(key, "[REDACTED]"); }
    detail = detail.chars().take(4000).collect();
    if detail.trim().is_empty() { detail = "Empty response body".into(); }
    let summary = if status.is_success() { "Provider returned an API error or invalid model response".into() }
        else { format!("Provider returned HTTP {status}") };
    Err(format!("{summary}\n{detail}"))
}

pub async fn check_response(mut response: reqwest::Response, key: Option<&str>) -> Result<(), String> {
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| format!("Provider returned HTTP {status}\nFailed to read response body (connection interrupted or timed out)."))? {
        let remaining = (256 * 1024usize).saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() > remaining { return Err(format!("Provider returned HTTP {status}\nResponse body exceeds the test limit (256 KiB).")); }
    }
    validate(status, &body, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_error_details_and_redacts_key() {
        let error = validate(reqwest::StatusCode::TOO_MANY_REQUESTS, br#"{"error":{"message":"Quota exceeded secret-value","code":"rate_limit"}}"#, Some("secret-value")).unwrap_err();
        assert!(error.contains("429") && error.contains("Quota exceeded") && error.contains("rate_limit"));
        assert!(!error.contains("secret-value"));
    }
    #[test]
    fn rejects_errors_inside_http_success_and_invalid_bodies() {
        for body in [br#"{"error":{"code":522,"message":"timeout"}}"#.as_slice(), b"{}", br#"{"choices":[{"message":{"content":""}}]}"#, br#"{"output":[{"type":"message"}]}"#, b"<html>bad gateway</html>", b""] {
            assert!(validate(reqwest::StatusCode::OK, body, None).is_err());
        }
        for body in [br#"{"choices":[{"message":{"content":"OK"}}]}"#.as_slice(), br#"{"content":[{"type":"text","text":"OK"}]}"#, br#"{"output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}]}"#] {
            assert!(validate(reqwest::StatusCode::OK, body, None).is_ok());
        }
    }
}
