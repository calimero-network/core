//! Acquiring a context's bytecode, and the node-side half of the downloader:
//! [`calimero_app_downloader`] owns the routing, this owns the storage.

use std::sync::Arc;

use async_trait::async_trait;
use calimero_app_downloader::port::{ApplicationStore, InstalledApplication};
use calimero_app_downloader::registry::RegistryCoords;
use calimero_app_downloader::source::dht::PeerBlobs;
use calimero_app_downloader::{app_source, AppRequest, ApplicationDownloader, Outcome};
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_store::key;
use futures_util::io::Cursor;
use tracing::warn;

use crate::client::NodeClient;

impl NodeClient {
    /// Acquire the bytecode `req` names from the one source this node is
    /// configured with.
    ///
    /// On any outcome other than [`Outcome::Unavailable`] the application row
    /// for `req.application_id` names `req.bytecode_id` and that blob is local.
    /// Never errors: a fault is reported as `Unavailable`, which callers treat
    /// as "keep the current version, retry later".
    pub async fn acquire_bytecode(&self, req: &AppRequest<'_>) -> Outcome {
        let source = match app_source(&self.registry_config(), self.clone()) {
            Ok(source) => source,
            Err(err) => {
                warn!(%err, "no application source is configured");
                return Outcome::Unavailable;
            }
        };
        match ApplicationDownloader::new(self.clone(), source)
            .download(req)
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                warn!(bytecode_id = ?req.bytecode_id, %err, "bytecode acquisition failed");
                Outcome::Unavailable
            }
        }
    }
}

#[async_trait]
impl PeerBlobs for NodeClient {
    async fn fetch_bytecode_from_peers(
        &self,
        bytecode_id: &BlobId,
        context_id: &ContextId,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        self.get_blob_bytes(bytecode_id, Some(context_id)).await
    }
}

#[async_trait]
impl ApplicationStore for NodeClient {
    fn has_bytecode(&self, bytecode_id: &BlobId) -> eyre::Result<bool> {
        self.has_blob(bytecode_id)
    }

    fn installed_application(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<InstalledApplication>> {
        let row = self
            .datastore
            .handle()
            .get(&key::ApplicationMeta::new(*application_id))?;
        Ok(row.map(|row| InstalledApplication {
            bytecode_id: row.bytecode.blob_id(),
            source: String::from(row.source),
        }))
    }

    async fn read_local_bytecode(&self, bytecode_id: &BlobId) -> eyre::Result<Option<Arc<[u8]>>> {
        self.get_blob_bytes(bytecode_id, None).await
    }

    async fn store_bytecode(&self, bytes: &[u8]) -> eyre::Result<(BlobId, u64)> {
        // Never add_blob's expected_content_hash: it hashes content, not
        // chunk ids, and would reject every correct artifact.
        self.add_blob(Cursor::new(bytes), Some(bytes.len() as u64), None)
            .await
    }

    async fn release_bytecode(&self, bytecode_id: BlobId) -> eyre::Result<()> {
        let _deleted = self.delete_blob(bytecode_id).await?;
        Ok(())
    }

    async fn bind_application(
        &self,
        application_id: &ApplicationId,
        bytecode_id: BlobId,
        size: u64,
        source: &ApplicationSource,
        coords: Option<RegistryCoords<'_>>,
        bytes: &[u8],
    ) -> eyre::Result<()> {
        self.bind_application_row(application_id, bytecode_id, size, source, coords, bytes)
            .await
    }
}
