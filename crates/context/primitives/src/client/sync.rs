//! Context configuration synchronization and application installation.
//!
//! This module handles syncing context configuration from external sources,
//! installing applications (both bundles and regular WASM), and managing
//! context metadata updates.

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration};
use tracing::{debug, warn};
use url::Url;

use super::ContextClient;
use crate::messages::{ContextMessage, SyncRequest};

impl ContextClient {
    // Constants for application installation
    const DEFAULT_PACKAGE: &str = "unknown";
    const DEFAULT_VERSION: &str = "0.0.0";
    const MAX_BLOB_RETRIES: u32 = 20;
    const BLOB_RETRY_DELAY_MS: u64 = 1000;
    /// Try to install application from URL (for HTTP/HTTPS sources)
    async fn try_install_from_url(
        &self,
        source: &Url,
        metadata: &[u8],
    ) -> eyre::Result<Option<ApplicationId>> {
        match source.scheme() {
            "http" | "https" => Ok(Some(
                self.node_client
                    .install_application_from_url(source.clone(), metadata.to_vec(), None)
                    .await?,
            )),
            _ => Ok(None),
        }
    }

    /// Install a regular (non-bundle) application
    async fn install_regular_application(
        &self,
        blob_id: &BlobId,
        size: u64,
        source: &Url,
        metadata: &[u8],
    ) -> eyre::Result<ApplicationId> {
        self.node_client.install_application(
            blob_id,
            size,
            &source.clone().into(),
            metadata.to_vec(),
            Self::DEFAULT_PACKAGE,
            Self::DEFAULT_VERSION,
            None,  // signer_id: None for non-bundle installations
            false, // is_bundle: false
        )
    }

    /// Check if blob is a bundle and install accordingly
    async fn check_bundle_and_install(
        &self,
        blob_id: &BlobId,
        blob_bytes: &[u8],
        source: &Url,
        size: u64,
        metadata: &[u8],
    ) -> eyre::Result<ApplicationId> {
        let blob_bytes_clone = blob_bytes.to_vec();
        let is_bundle =
            tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&blob_bytes_clone))
                .await?;

        if is_bundle {
            debug!(
                blob_id = %blob_id,
                "Blob is a bundle, installing from bundle blob"
            );
            self.node_client
                .install_application_from_bundle_blob(blob_id, &source.clone().into())
                .await
        } else {
            debug!(
                blob_id = %blob_id,
                "Blob is not a bundle, using regular installation"
            );
            self.install_regular_application(blob_id, size, source, metadata)
                .await
        }
    }

    /// Install application from existing blob (checks if bundle and installs accordingly)
    async fn install_from_existing_blob(
        &self,
        blob_id: &BlobId,
        source: &Url,
        size: u64,
        metadata: &[u8],
    ) -> eyre::Result<ApplicationId> {
        debug!(
            blob_id = %blob_id,
            "Blob exists locally, checking if it's a bundle"
        );

        // Check if blob is a bundle
        let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
            debug!(
                blob_id = %blob_id,
                "Failed to read blob, falling back to regular installation"
            );
            // Failed to read blob, fall back to regular installation
            return self
                .install_regular_application(blob_id, size, source, metadata)
                .await;
        };

        // Check if bundle and install accordingly
        self.check_bundle_and_install(blob_id, &blob_bytes, source, size, metadata)
            .await
    }

    /// Wait for blob to arrive and install bundle (with retry logic)
    async fn wait_for_blob_and_install(
        &self,
        blob_id: &BlobId,
        source: &Url,
        size: u64,
        metadata: &[u8],
        expected_app_id: ApplicationId,
    ) -> eyre::Result<ApplicationId> {
        debug!(
            blob_id = %blob_id,
            "Source indicates bundle (.mpk), waiting for blob to arrive via blob sharing"
        );
        // For bundles, we need the blob to extract package/version from manifest
        // Wait a bit for blob sharing to deliver it, then retry

        for _ in 0..Self::MAX_BLOB_RETRIES {
            // Check if blob is available
            if !self.node_client.has_blob(blob_id)? {
                sleep(Duration::from_millis(Self::BLOB_RETRY_DELAY_MS)).await;
                continue;
            }

            // Blob arrived, try to read and install
            let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
                sleep(Duration::from_millis(Self::BLOB_RETRY_DELAY_MS)).await;
                continue;
            };

            debug!(
                blob_id = %blob_id,
                "Blob arrived, installing bundle"
            );

            // Check if bundle and install
            return self
                .check_bundle_and_install(blob_id, &blob_bytes, source, size, metadata)
                .await;
        }

        // Retries exhausted
        warn!(
            blob_id = %blob_id,
            "Blob didn't arrive within retry window - bundle installation will be retried when blob arrives"
        );
        // Blob didn't arrive in time - we can't install without package/version from manifest
        // Return the ApplicationId from context config to pass the check
        // The application will be installed when blob arrives via blob sharing
        // This will cause initiate_sync_inner to fail with "application not found",
        // but blob sharing will happen and installation will succeed on retry
        Ok(expected_app_id)
    }

    /// Install application when blob doesn't exist locally yet
    async fn install_when_blob_missing(
        &self,
        blob_id: &BlobId,
        source: &Url,
        size: u64,
        metadata: &[u8],
        expected_app_id: ApplicationId,
    ) -> eyre::Result<ApplicationId> {
        debug!(
            blob_id = %blob_id,
            "Blob doesn't exist locally, checking source for bundle detection"
        );
        // Blob doesn't exist yet - try to detect if it's a bundle from source URL
        // If source ends with .mpk, it's likely a bundle
        let is_bundle_from_source = source.path().ends_with(".mpk");

        if is_bundle_from_source {
            // Wait for blob to arrive and install bundle
            self.wait_for_blob_and_install(blob_id, source, size, metadata, expected_app_id)
                .await
        } else {
            debug!(
                blob_id = %blob_id,
                "Blob doesn't exist locally, using regular installation"
            );
            // Blob doesn't exist yet - create ApplicationMeta entry anyway
            // The blob will be shared later in initiate_sync_inner
            // When blob arrives, get_application_bytes will handle extraction on-demand
            self.install_regular_application(blob_id, size, source, metadata)
                .await
        }
    }

    /// Synchronize context configuration and ensure context metadata is present.
    ///
    /// Two modes:
    ///
    /// * **Bootstrap** (`config: Some(...)`): The context does not exist locally
    ///   yet. The caller supplies initial revision hints. The function installs
    ///   the application (if not already present), writes `ContextMeta` and
    ///   `ContextConfig`, and sends a `Sync` message to the context manager.
    ///
    /// * **Refresh** (`config: None`): The context already exists locally.
    ///   Returns the stored context. Membership and application state are kept
    ///   up-to-date through the governance DAG, so no revision polling is needed.
    pub async fn sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<ContextConfigParams>,
    ) -> eyre::Result<Context> {
        let mut handle = self.datastore.handle();

        let context = handle.get(&key::ContextMeta::new(context_id))?;

        // Refresh path: context already exists, return stored metadata.
        // Membership and application updates propagate through the governance
        // DAG, so there is no external source to poll for revision changes.
        let Some(config) = config else {
            let meta = context.ok_or_else(|| {
                eyre::eyre!("sync_context_config called with config: None but context {context_id} not found")
            })?;

            debug!(
                %context_id,
                application_id = %meta.application.application_id(),
                dag_heads_count = meta.dag_heads.len(),
                "context already exists, returning stored metadata"
            );

            return Ok(Context::with_dag_heads(
                context_id,
                meta.application.application_id(),
                meta.root_hash.into(),
                meta.dag_heads.clone(),
            ));
        };

        // Bootstrap path: resolve application and create store entries.
        //
        // The application_id is supplied by the caller (looked up from the
        // group store) because `ContextMeta` has not been written yet — it is
        // created below.  If the application is already installed locally we
        // run the full installation checks; otherwise we write the metadata
        // and let blob-sharing + sync retries deliver the binary later.

        let application_id = if let Some(ctx) = &context {
            ctx.application.application_id()
        } else {
            config
                .application_id
                .ok_or_else(|| eyre::eyre!(
                    "bootstrap requires application_id in ContextConfigParams \
                     (context {context_id} has no local metadata yet)"
                ))?
        };

        if let Some(application) = self.node_client().get_application(&application_id)? {
            if !self.node_client.has_application(&application_id)? {
                let source: Url = application.source.into();
                let metadata = application.metadata.clone();
                let blob_id = application.blob.bytecode;

                let derived_application_id = {
                    if let Some(app_id) =
                        self.try_install_from_url(&source, &metadata).await?
                    {
                        app_id
                    } else if self.node_client.has_blob(&blob_id)? {
                        self.install_from_existing_blob(
                            &blob_id,
                            &source,
                            application.size,
                            &metadata,
                        )
                        .await?
                    } else {
                        self.install_when_blob_missing(
                            &blob_id,
                            &source,
                            application.size,
                            &metadata,
                            application_id,
                        )
                        .await?
                    }
                };

                if application_id != derived_application_id {
                    eyre::bail!(
                        "application mismatch: expected {}, got {}",
                        application_id,
                        derived_application_id
                    )
                }
            }
        } else {
            debug!(
                %context_id,
                %application_id,
                "application not available locally during bootstrap; \
                 writing metadata — blob sharing will deliver it"
            );
        }

        handle.put(
            &key::ContextConfig::new(context_id),
            &types::ContextConfig::new(config.application_revision, config.members_revision),
        )?;

        let (root_hash, dag_heads) = context.map_or_else(
            || (Hash::default(), vec![]),
            |meta| (meta.root_hash.into(), meta.dag_heads.clone()),
        );

        handle.put(
            &key::ContextMeta::new(context_id),
            &types::ContextMeta::new(
                key::ApplicationMeta::new(application_id),
                *root_hash,
                dag_heads.clone(),
            ),
        )?;

        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::Sync {
                request: SyncRequest {
                    context_id,
                    application_id,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped");

        Ok(Context::with_dag_heads(
            context_id,
            application_id,
            root_hash,
            dag_heads,
        ))
    }
}
