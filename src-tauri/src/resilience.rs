//! Endpoint health is scoped to one gateway instance. No prompts or credentials are retained.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub connect_timeout_seconds: u64,
    pub response_timeout_seconds: u64,
    pub stream_idle_seconds: u64,
    pub max_attempts: usize,
    pub failure_threshold: u32,
    pub cooldown_seconds: u64,
    pub proxy_mode: String,
    pub proxy_url: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            connect_timeout_seconds: 10,
            response_timeout_seconds: 60,
            stream_idle_seconds: 120,
            max_attempts: 4,
            failure_threshold: 3,
            cooldown_seconds: 30,
            proxy_mode: "system".into(),
            proxy_url: String::new(),
        }
    }
}
impl Settings {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=120).contains(&self.connect_timeout_seconds)
                && (1..=600).contains(&self.response_timeout_seconds)
                && (1..=1800).contains(&self.stream_idle_seconds),
            "Timeout settings are out of range"
        );
        anyhow::ensure!(
            (1..=16).contains(&self.max_attempts)
                && (1..=20).contains(&self.failure_threshold)
                && (1..=3600).contains(&self.cooldown_seconds),
            "Retry or circuit settings are out of range"
        );
        anyhow::ensure!(
            matches!(self.proxy_mode.as_str(), "system" | "direct" | "custom"),
            "Invalid proxy mode"
        );
        if self.proxy_mode == "custom" {
            let url = reqwest::Url::parse(&self.proxy_url)?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none(),
                "Proxy must be an HTTP(S) URL without embedded credentials"
            );
        }
        Ok(())
    }
    pub fn client(&self) -> anyhow::Result<reqwest::Client> {
        self.validate()?;
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(self.connect_timeout_seconds))
            .redirect(reqwest::redirect::Policy::none());
        if self.proxy_mode == "direct" {
            builder = builder.no_proxy();
        }
        if self.proxy_mode == "custom" {
            builder = builder.proxy(
                reqwest::Proxy::all(&self.proxy_url)?
                    .no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.1,::1")),
            );
        }
        Ok(builder.build()?)
    }
}
#[derive(Default)]
struct Circuit {
    failures: u32,
    until: Option<Instant>,
    probing: bool,
    last_status: u16,
    generation: u64,
}
#[derive(Clone, Default)]
pub struct Health(Arc<Mutex<HashMap<String, Circuit>>>);
#[derive(Serialize)]
pub struct Status {
    pub model_id: String,
    pub failures: u32,
    pub state: String,
    pub retry_after_seconds: u64,
    pub last_status: u16,
}
impl Health {
    pub fn available(&self, id: &str) -> bool {
        let guard = self.0.lock().unwrap();
        guard
            .get(id)
            .is_none_or(|c| !c.probing && c.until.is_none_or(|t| t <= Instant::now()))
    }
    /// Only one request is allowed to probe a recovered circuit at a time.
    pub fn acquire(&self, id: &str) -> Option<Lease> {
        let mut guard = self.0.lock().unwrap();
        let c = guard.entry(id.into()).or_default();
        if c.probing || c.until.is_some_and(|t| t > Instant::now()) {
            return None;
        }
        let probe = c.until.is_some();
        if probe {
            c.probing = true;
        }
        Some(Lease {
            health: self.clone(),
            id: id.into(),
            probe,
            generation: c.generation,
            finished: false,
        })
    }
    pub fn statuses(&self) -> Vec<Status> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(id, c)| Status {
                model_id: id.clone(),
                failures: c.failures,
                state: if c.probing {
                    "probing"
                } else if c.until.is_some_and(|t| t > Instant::now()) {
                    "cooldown"
                } else if c.until.is_some() {
                    "ready_to_probe"
                } else {
                    "healthy"
                }
                .into(),
                retry_after_seconds: c
                    .until
                    .map_or(0, |t| t.saturating_duration_since(Instant::now()).as_secs()),
                last_status: c.last_status,
            })
            .collect()
    }
    pub fn reset(&self) {
        for circuit in self.0.lock().unwrap().values_mut() {
            *circuit = Circuit { generation: circuit.generation.wrapping_add(1), ..Default::default() };
        }
    }
    pub fn reset_model(&self, id: &str) {
        if let Some(circuit) = self.0.lock().unwrap().get_mut(id) {
            *circuit = Circuit { generation: circuit.generation.wrapping_add(1), ..Default::default() };
        }
    }
}
pub struct Lease {
    health: Health,
    id: String,
    probe: bool,
    generation: u64,
    finished: bool,
}
impl Lease {
    pub fn complete(mut self, status: u16, retry_after: Option<u64>, settings: &Settings) {
        let mut guard = self.health.0.lock().unwrap();
        let c = guard.entry(self.id.clone()).or_default();
        if self.generation != c.generation {
            self.finished = true;
            return;
        }
        c.probing = false;
        c.last_status = status;
        if retryable(status) {
            c.failures = c.failures.saturating_add(1);
            if matches!(status, 401 | 402 | 403 | 429) || self.probe || c.failures >= settings.failure_threshold {
                c.generation = c.generation.wrapping_add(1);
                c.until = Some(
                    Instant::now()
                        + Duration::from_secs(
                            retry_after
                                .unwrap_or(settings.cooldown_seconds)
                                .clamp(1, 3600),
                        ),
                );
            }
        } else if (200..400).contains(&status) {
            c.failures = 0;
            c.until = None;
        } else {
            c.until = None;
        } // Request/auth errors don't mark a model unhealthy.
        self.finished = true;
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.finished && self.probe {
            if let Some(c) = self.health.0.lock().unwrap().get_mut(&self.id) {
                if c.generation == self.generation {
                    c.probing = false;
                }
            }
        }
    }
}
pub fn retryable(status: u16) -> bool {
    matches!(status, 401 | 402 | 403 | 408 | 429 | 500..=599)
}
pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get("retry-after")?.to_str().ok()?;
    value.parse::<u64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc2822(value)
            .ok()
            .map(|date| (date.timestamp() - chrono::Utc::now().timestamp()).max(1) as u64)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rate_limits_cool_down_and_only_one_probe_is_allowed() {
        let h = Health::default();
        let s = Settings::default();
        h.acquire("m").unwrap().complete(429, Some(60), &s);
        assert!(!h.available("m"));
        assert!(h.acquire("m").is_none());
        h.0.lock().unwrap().get_mut("m").unwrap().until = Some(Instant::now());
        let probe = h.acquire("m").unwrap();
        assert!(h.acquire("m").is_none());
        drop(probe);
        h.acquire("m").unwrap().complete(200, None, &s);
        assert!(h.available("m"));
    }
    #[test]
    fn threshold_and_request_errors() {
        let h = Health::default();
        let s = Settings::default();
        for _ in 0..2 {
            h.acquire("m").unwrap().complete(503, None, &s);
            assert!(h.available("m"));
        }
        h.acquire("m").unwrap().complete(503, None, &s);
        assert!(!h.available("m"));
        h.acquire("other").unwrap().complete(400, None, &s);
        assert!(h.available("other"));
    }
}

#[cfg(test)]
mod concurrent_health_tests {
    #[test]
    fn late_success_cannot_close_a_newer_circuit() {
        let health = super::Health::default();
        let settings = super::Settings::default();
        let slow = health.acquire("m").unwrap();
        let limited = health.acquire("m").unwrap();
        limited.complete(429, None, &settings);
        slow.complete(200, None, &settings);
        assert!(!health.available("m"));
        assert_eq!(health.statuses()[0].last_status, 429);
    }
}

#[cfg(test)]
mod targeted_reset_tests {
    use super::*;
    #[test]
    fn reset_one_model_preserves_other_cooldowns_and_ignores_old_requests() {
        let health = Health::default();
        let settings = Settings::default();
        health.acquire("a").unwrap().complete(429, Some(60), &settings);
        health.acquire("b").unwrap().complete(429, Some(60), &settings);
        health.reset_model("a");
        assert!(health.available("a"));
        assert!(!health.available("b"));
        let old = health.acquire("a").unwrap();
        health.reset_model("a");
        old.complete(429, Some(60), &settings);
        assert!(health.available("a"));
        assert!(!health.available("b"));
        assert_eq!(health.statuses().iter().find(|s|s.model_id=="a").unwrap().state,"healthy");
    }
}
