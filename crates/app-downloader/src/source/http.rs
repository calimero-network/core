//! The registry route: one GET against the node's own configured base.

use std::sync::Arc;

use async_trait::async_trait;
use eyre::bail;
use reqwest::{Client, StatusCode, Url};
use tracing::info;

use crate::registry::RegistryCoords;
use crate::source::{AppRequest, AppSource};

const TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300); // generous for large bundles, bounded against a slowloris host
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15); // connect timeout for an artifact fetch
/// Maximum size of a fetched application artifact; bounds memory against a hostile or lying `Content-Length`.
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// The one artifact client. An artifact lives at its coordinates on the one
/// configured registry, so a redirect is refused rather than followed to
/// wherever a server names - which is the whole of this client's SSRF surface.
fn registry_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Read an HTTP response body into memory, enforcing a hard byte cap so a
/// missing or lying `Content-Length` can't grow the buffer without bound.
async fn read_body_capped(mut response: reqwest::Response, max: u64) -> eyre::Result<Vec<u8>> {
    let mut buf = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        // `> max` means a body of exactly `max` bytes is accepted (`max` is an
        // inclusive limit) while anything larger is rejected before it is
        // buffered — the running total never exceeds `max`.
        if buf.len() as u64 + chunk.len() as u64 > max {
            bail!("application artifact exceeded size limit of {max} bytes");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// The node's own configured registry, addressed by `package@version`.
#[derive(Clone, Debug)]
pub struct HttpRegistry {
    base: Url,
    client: Client,
}

impl HttpRegistry {
    /// The scheme is `base`'s and never varies per fetch, so it is checked here.
    pub fn new(base: Url) -> eyre::Result<Self> {
        if !matches!(base.scheme(), "http" | "https") {
            bail!(
                "unsupported registry URL scheme '{}'; only http and https are allowed",
                base.scheme()
            );
        }
        Ok(Self {
            base,
            client: registry_client()?,
        })
    }
}

#[async_trait]
impl AppSource for HttpRegistry {
    async fn fetch(&self, req: &AppRequest<'_>) -> eyre::Result<Option<Arc<[u8]>>> {
        // Unaddressable is a rejection, not an absence: reporting it as
        // "nothing published" would describe a fetch that never ran.
        let coords = RegistryCoords::new(req.package, req.version);
        let Some(url) = coords.artifact_url(&self.base) else {
            if self.base.cannot_be_a_base() {
                bail!("[registry] base_url {} cannot address artifacts", self.base);
            }
            bail!(
                "registry coordinates {}@{} cannot address an artifact",
                req.package,
                req.version
            );
        };
        info!(%url, application_id = ?req.application_id, "fetching application from registry");

        // No host guard, redirects included: `url` is this node's own
        // configured base, which is routinely private or air-gapped.
        let response = self.client.get(url.clone()).send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            // Not published here yet, which the caller retries - not a fault.
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!("registry returned HTTP {} for {url}", response.status());
        }
        let bytes = read_body_capped(response, MAX_ARTIFACT_BYTES).await?;
        Ok(Some(bytes.into()))
    }
}
