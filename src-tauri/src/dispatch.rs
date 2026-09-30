//! The generation transport seam. Callers retain protocol conversion, capture and errors.
use crate::{config::Provider, protocol::Protocol};
use futures_util::future::BoxFuture;

pub struct Target<'a> {
    pub provider: &'a Provider,
    pub model_id: &'a str,
    pub protocol: Protocol,
}

pub trait Dispatcher: Send + Sync {
    fn send<'a>(
        &'a self,
        target: Target<'a>,
        request: reqwest::RequestBuilder,
    ) -> BoxFuture<'a, anyhow::Result<reqwest::Response>>;
}

pub struct ApiDispatcher {
    pub loopback_only: bool,
}

impl Dispatcher for ApiDispatcher {
    fn send<'a>(
        &'a self,
        target: Target<'a>,
        request: reqwest::RequestBuilder,
    ) -> BoxFuture<'a, anyhow::Result<reqwest::Response>> {
        Box::pin(async move {
            anyhow::ensure!(
                !target.provider.id.is_empty() && !target.model_id.is_empty(),
                "Missing dispatch target"
            );
            // All existing API protocols have already been prepared by the caller.
            let _protocol = target.protocol;
            send_http(request, self.loopback_only).await
        })
    }
}

// Also guards decision and local Debug traffic in an isolated desktop process.
// Deadlines must be on RequestBuilder: client defaults do not survive isolated rebuilds.
pub async fn send_http(
    request: reqwest::RequestBuilder,
    loopback_only: bool,
) -> anyhow::Result<reqwest::Response> {
    if !loopback_only {
        return Ok(request.send().await?);
    }
    let request = request.build()?;
    let url = request.url();
    ensure_loopback(url)?;
    crate::runtime::check_url(url)?;
    // Ignore all proxy environment/configuration and do not follow redirects.
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .execute(request)
        .await?)
}

pub fn ensure_loopback(url: &reqwest::Url) -> anyhow::Result<()> {
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    anyhow::ensure!(
        url.scheme() == "http"
            && host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
            && url.username().is_empty()
            && url.password().is_none(),
        "Isolated validation permits only literal loopback HTTP targets"
    );
    Ok(())
}
