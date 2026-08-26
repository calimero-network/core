//! The registry route: one GET against the node's own configured base.

use async_trait::async_trait;
use eyre::bail;
use reqwest::{Client, StatusCode, Url};
use tracing::info;

use crate::http::{read_body_capped, registry_client, MAX_ARTIFACT_BYTES};
use crate::registry::RegistryCoords;
use crate::source::{AppRequest, AppSource, Bytes};

/// The node's own configured registry, addressed by `package@version`.
#[derive(Clone, Debug)]
pub struct HttpRegistry {
    base: Url,
    client: Client,
}

impl HttpRegistry {
    /// Fails only when the HTTP client itself cannot be built.
    pub fn new(base: Url) -> eyre::Result<Self> {
        Ok(Self {
            base,
            client: registry_client()?,
        })
    }
}

#[async_trait]
impl AppSource for HttpRegistry {
    async fn fetch(&self, req: &AppRequest<'_>) -> eyre::Result<Option<Bytes>> {
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
        if !matches!(url.scheme(), "http" | "https") {
            bail!(
                "unsupported registry URL scheme '{}'; only http and https are allowed",
                url.scheme()
            );
        }
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
        if let Some(len) = response.content_length() {
            if len > MAX_ARTIFACT_BYTES {
                bail!("registry artifact too large: {len} bytes exceeds {MAX_ARTIFACT_BYTES}");
            }
        }

        let bytes = read_body_capped(response, MAX_ARTIFACT_BYTES).await?;
        Ok(Some(bytes.into()))
    }
}
