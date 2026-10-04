//! Curated fixed-v8 HTTP operations; no credential download or arbitrary api-call.
use anyhow::{ensure, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;

const COMMIT: &str = "e2bff0107bb307337aaa19018ccddd55f64253d5";
#[derive(PartialEq, Eq)]
pub enum SessionStatus {
    Waiting,
    Complete,
    Expired,
}
pub struct Client {
    base: reqwest::Url,
    key: String,
    http: reqwest::Client,
    fictional_authorization: bool,
}
#[derive(Deserialize)]
pub struct Credential {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub id_token: Claims,
    #[serde(default)]
    pub account_type: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
}
#[derive(Default, Deserialize)]
pub struct Claims {
    #[serde(default)]
    pub chatgpt_account_id: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
}
#[derive(Deserialize)]
struct Files {
    files: Vec<Credential>,
}
#[derive(Deserialize)]
struct Models {
    models: Vec<DirectoryModel>,
}
#[derive(Deserialize)]
struct DirectoryModel {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

impl Credential {
    pub fn identity(&self)->Option<String> {
        if self.provider=="codex" {return self.id_token.chatgpt_account_id.clone();}
        if self.provider=="xai" && self.account_type.as_deref()==Some("oauth") {return self.account.clone();}
        None // In particular, never project an api_key account value.
    }
}
impl Client {
    #[cfg(test)]
    pub fn model_base(&self)->String {self.base.as_str().trim_end_matches('/').into()}
    pub fn owned_service(base: String, key: String) -> Result<Self> {
        let base = reqwest::Url::parse(&base)?;
        crate::dispatch::ensure_loopback(&base)?;
        ensure!(base.path() == "/" && base.query().is_none() && base.fragment().is_none()
            && base.username().is_empty() && base.password().is_none(), "Use a dedicated owned service origin");
        ensure!(!key.trim().is_empty(), "Owned management authentication is missing");
        Ok(Self { base, key, fictional_authorization: false,
            http: reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(3)).build()? })
    }
    #[cfg(any(test, feature = "isolation-check"))]
    pub fn owned_fixture(base: String) -> Result<Self> {
        let base = reqwest::Url::parse(&base)?;
        crate::dispatch::ensure_loopback(&base)?;
        ensure!(
            base.path() == "/" && base.query().is_none() && base.fragment().is_none(),
            "Use a dedicated fixture origin"
        );
        ensure!(
            !matches!(base.port(), Some(9526 | 9527 | 11434)),
            "Use a dedicated fixture port"
        );
        Ok(Self {
            base,
            key: "fictional-management-r3".into(),
            fictional_authorization: true,
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(3))
                .build()?,
        })
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Value> {
        let url = self.base.join(&format!("v8/management/{path}"))?;
        let response = self
            .http
            .request(method, url)
            .query(query)
            .bearer_auth(&self.key)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("CPA management request failed"))?;
        ensure!(
            response.status().is_success(),
            "CPA management returned HTTP {}",
            response.status().as_u16()
        );
        ensure!(
            response
                .headers()
                .get("x-cpa-commit")
                .and_then(|h| h.to_str().ok())
                == Some(COMMIT),
            "CPA service does not match the pinned contract"
        );
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| anyhow::anyhow!("CPA management response failed"))?;
            ensure!(
                bytes.len() + chunk.len() <= 262144,
                "CPA management response is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid CPA management response"))
    }
    pub async fn credentials(&self) -> Result<Vec<Credential>> {
        let result: Files = serde_json::from_value(
            self.request(reqwest::Method::GET, "credentials", &[])
                .await?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid CPA credential metadata"))?;
        ensure!(
            result.files.iter().all(|f| !f.name.is_empty()
                && f.name.len() <= 200
                && !f.name.contains(['/', '\\'])
                && !f.name.starts_with('.')),
            "Invalid credential reference"
        );
        Ok(result.files)
    }
    pub async fn begin(&self, provider: &str) -> Result<(String, String)> {
        let value = self
            .request(
                reqwest::Method::GET,
                "oauth/auth-url",
                &[("provider", provider), ("is_webui", "true")],
            )
            .await?;
        let state = value["state"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or_else(|| anyhow::anyhow!("CPA did not return an authorization session"))?
            .to_owned();
        let url = value["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("CPA did not return an authorization URL"))?;
        let parsed =
            reqwest::Url::parse(url).map_err(|_| anyhow::anyhow!("Invalid authorization URL"))?;
        let accepted_host = parsed.host_str().is_some_and(|host| {
            if self.fictional_authorization { return host.ends_with(".example.invalid"); }
            match provider {
                "codex" => host == "auth.openai.com",
                "xai" => matches!(host, "auth.x.ai" | "accounts.x.ai"),
                _ => false,
            }
        });
        ensure!(
            parsed.scheme() == "https" && accepted_host
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "CPA returned an authorization URL outside this provider's contract"
        );
        Ok((state, url.into()))
    }
    pub async fn status(&self, state: &str) -> Result<SessionStatus> {
        let value = self
            .request(reqwest::Method::GET, "oauth/status", &[("state", state)])
            .await?;
        match value["status"].as_str() {
            Some("ok") => Ok(SessionStatus::Complete),
            Some("wait") => Ok(SessionStatus::Waiting),
            Some("error") if value["error"] == "unknown or expired state" => Ok(SessionStatus::Expired),
            _ => anyhow::bail!("CPA authorization failed or expired"),
        }
    }
    pub async fn cancel(&self, state: &str) -> Result<bool> {
        let result = self
            .request(
                reqwest::Method::DELETE,
                "oauth/session",
                &[("state", state)],
            )
            .await?;
        ensure!(
            result["status"] == "ok",
            "CPA did not acknowledge cancellation"
        );
        result["cancelled"]
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("CPA returned an unknown cancellation result"))
    }
    pub async fn delete(&self, name: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, "credentials", &[("name", name)])
            .await?;
        Ok(())
    }
    pub async fn models(&self, name: &str) -> Result<Vec<super::Discovered>> {
        let result: Models = serde_json::from_value(
            self.request(
                reqwest::Method::GET,
                "credentials/models",
                &[("name", name)],
            )
            .await?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid CPA directory"))?;
        let mut ids = std::collections::HashSet::new();
        ensure!(
            result.models.len() <= 2000
                && result
                    .models
                    .iter()
                    .all(|m| !m.id.trim().is_empty() && m.id.len() <= 200 && ids.insert(&m.id)),
            "Incomplete or ambiguous CPA directory"
        );
        Ok(result
            .models
            .into_iter()
            .map(|m| super::Discovered {
                name: m.display_name.unwrap_or_else(|| m.id.clone()),
                model_id: m.id,
            })
            .collect())
    }
}
