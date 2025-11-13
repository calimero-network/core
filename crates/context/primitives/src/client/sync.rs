use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use tokio::sync::oneshot;
use tracing::{debug, warn};
use url::Url;

use super::ContextClient;
use crate::messages::{ContextMessage, SyncRequest};

/// Error message constant for deferred installation due to missing blob
const DEFERRED_INSTALLATION_MSG: &str = "blob doesn't exist and will be shared later";

impl ContextClient {
    pub async fn sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<ContextConfigParams<'_>>,
    ) -> eyre::Result<Context> {
        let mut handle = self.datastore.handle();

        let context = handle.get(&key::ContextMeta::new(context_id))?;

        let (mut config, mut should_save_config) = config.map_or_else(
            || {
                let Some(config) = handle.get(&key::ContextConfig::new(context_id))? else {
                    eyre::bail!("context config not found")
                };

                let config = ContextConfigParams {
                    protocol: config.protocol.into_string().into(),
                    network_id: config.network.into_string().into(),
                    contract_id: config.contract.into_string().into(),
                    proxy_contract: config.proxy_contract.into_string().into(),
                    application_revision: config.application_revision,
                    members_revision: config.members_revision,
                };

                Ok((config, false))
            },
            |config| Ok((config, true)),
        )?;

        let members_revision = {
            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            config_client.members_revision().await?
        };

        if context.is_none() || members_revision != config.members_revision {
            should_save_config = true;
            config.members_revision = members_revision;

            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            for (offset, length) in (0..).map(|i| (100_usize.saturating_mul(i), 100)) {
                let members = config_client.members(offset, length).await?;

                if members.is_empty() {
                    break;
                }

                for member in members {
                    let key = key::ContextIdentity::new(context_id, member);

                    if !handle.has(&key)? {
                        handle.put(
                            &key,
                            &types::ContextIdentity {
                                private_key: None,
                                sender_key: None,
                            },
                        )?;
                    }
                }
            }
        }

        let application_revision = {
            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            config_client.application_revision().await?
        };

        let mut application_id = None;

        if context.is_none() || application_revision != config.application_revision {
            should_save_config = true;
            config.application_revision = application_revision;

            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            let application = config_client.application().await?;

            application_id = Some(application.id);

            if !self.node_client.has_application(&application.id)? {
                let source: Url = application.source.clone().into();
                let metadata = application.metadata.clone();
                let blob_id = application.blob.bytecode;

                match self
                    .install_application_from_config(&application, &source, &metadata, &blob_id)
                    .await
                {
                    Ok(derived_application_id) => {
                        if application.id != derived_application_id {
                            eyre::bail!(
                                "application mismatch: expected {}, got {}",
                                application.id,
                                derived_application_id
                            )
                        }
                    }
                    Err(e) if e.to_string().contains(DEFERRED_INSTALLATION_MSG) => {
                        // Installation deferred - use application_id from context config
                        // The application will be installed when blob arrives via blob sharing
                        warn!(
                            blob_id = %blob_id,
                            application_id = %application.id,
                            "Application installation deferred until blob arrives"
                        );
                        // Continue with application.id from context config
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        if should_save_config {
            // todo! we shouldn't be reallocating here
            // todo! but store requires ContextConfig: 'static
            let config = config.clone();

            handle.put(
                &key::ContextConfig::new(context_id),
                &types::ContextConfig::new(
                    config.protocol.into_owned().into_boxed_str(),
                    config.network_id.into_owned().into_boxed_str(),
                    config.contract_id.into_owned().into_boxed_str(),
                    config.proxy_contract.into_owned().into_boxed_str(),
                    config.application_revision,
                    config.members_revision,
                ),
            )?;
        }

        let (should_save, application_id, root_hash, dag_heads) = context.map_or_else(
            || {
                (
                    true,
                    application_id.expect("must've been defined if context doesn't exist"),
                    Hash::default(),
                    vec![],
                )
            },
            |meta| {
                (
                    application_id.is_some(),
                    application_id.unwrap_or_else(|| meta.application.application_id()),
                    meta.root_hash.into(),
                    meta.dag_heads.clone(),
                )
            },
        );

        if should_save {
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
        }

        let context = Context::with_dag_heads(context_id, application_id, root_hash, dag_heads);

        Ok(context)
    }

    /// Install application from context config, handling URL, bundle, and blob-based installation
    async fn install_application_from_config(
        &self,
        application: &calimero_primitives::application::Application,
        source: &Url,
        metadata: &[u8],
        blob_id: &calimero_primitives::blobs::BlobId,
    ) -> eyre::Result<calimero_primitives::application::ApplicationId> {
        // Try URL installation first (for HTTP/HTTPS sources)
        if let Some(app_id) = self.try_url_installation(source, metadata).await? {
            return Ok(app_id);
        }

        // URL installation failed or not applicable
        // Check if blob exists locally (might have been received via blob sharing)
        if self.node_client.has_blob(blob_id)? {
            return self
                .install_from_local_blob(application, source, metadata, blob_id)
                .await;
        }

        // Blob doesn't exist yet - try to detect if it's a bundle from source URL
        let is_bundle_from_source = source.path().ends_with(".mpk");

        if is_bundle_from_source {
            self.wait_and_install_bundle(application, source, blob_id)
                .await
        } else {
            // For non-bundle applications, defer installation if blob doesn't exist
            // This prevents creating dangling ApplicationMeta entries
            self.install_application_fallback(application, source, metadata, blob_id)?
                .ok_or_else(|| {
                    eyre::eyre!(
                        "Cannot install application: blob {} {}. \
                         Installation will be retried when blob arrives via blob sharing.",
                        blob_id,
                        DEFERRED_INSTALLATION_MSG
                    )
                })
        }
    }

    /// Try to install application from URL (HTTP/HTTPS sources)
    async fn try_url_installation(
        &self,
        source: &Url,
        metadata: &[u8],
    ) -> eyre::Result<Option<calimero_primitives::application::ApplicationId>> {
        match source.scheme() {
            "http" | "https" => self
                .node_client
                .install_application_from_url(source.clone(), metadata.to_vec(), None)
                .await
                .ok()
                .map(Ok)
                .transpose(),
            _ => Ok(None),
        }
    }

    /// Install application from local blob, detecting bundle vs regular WASM
    async fn install_from_local_blob(
        &self,
        application: &calimero_primitives::application::Application,
        source: &Url,
        metadata: &[u8],
        blob_id: &calimero_primitives::blobs::BlobId,
    ) -> eyre::Result<calimero_primitives::application::ApplicationId> {
        debug!(
            blob_id = %blob_id,
            "Blob exists locally, checking if it's a bundle"
        );

        // Blob exists, check if it's a bundle
        let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
            debug!(
                blob_id = %blob_id,
                "Failed to read blob, falling back to regular installation"
            );
            // Blob should exist since we checked has_blob, but handle Option just in case
            return self
                .install_application_fallback(application, source, metadata, blob_id)?
                .ok_or_else(|| eyre::eyre!("Blob {} unexpectedly missing", blob_id));
        };

        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let blob_bytes_clone = blob_bytes.clone();
        let is_bundle =
            tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&blob_bytes_clone))
                .await?;

        if is_bundle {
            debug!(
                blob_id = %blob_id,
                "Blob is a bundle, installing from bundle blob"
            );
            // Install from bundle blob
            self.node_client
                .install_application_from_bundle_blob(blob_id, &source.clone().into())
                .await
        } else {
            debug!(
                blob_id = %blob_id,
                "Blob is not a bundle, using regular installation"
            );
            // Blob should exist since we checked has_blob, but handle Option just in case
            self.install_application_fallback(application, source, metadata, blob_id)?
                .ok_or_else(|| eyre::eyre!("Blob {} unexpectedly missing", blob_id))
        }
    }

    /// Wait for blob to arrive and install bundle, with retry logic
    async fn wait_and_install_bundle(
        &self,
        application: &calimero_primitives::application::Application,
        source: &Url,
        blob_id: &calimero_primitives::blobs::BlobId,
    ) -> eyre::Result<calimero_primitives::application::ApplicationId> {
        debug!(
            blob_id = %blob_id,
            "Source indicates bundle (.mpk), waiting for blob to arrive via blob sharing"
        );

        // For bundles, we need the blob to extract package/version from manifest
        // Wait a bit for blob sharing to deliver it, then retry
        use tokio::time::{sleep, Duration};
        let mut retries = 20; // Increased retries for blob sharing
        let mut installed = false;
        let mut app_id_result = application.id;

        while retries > 0 && !installed {
            // Check first, then sleep only if blob doesn't exist
            // This avoids wasting time if blob is already available
            if !self.node_client.has_blob(blob_id)? {
                retries -= 1;
                // Sleep after failed check, before next retry
                if retries > 0 {
                    sleep(Duration::from_millis(1000)).await;
                }
                continue;
            }

            debug!(
                blob_id = %blob_id,
                "Blob arrived, installing bundle"
            );

            // Blob arrived, install it
            let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
                retries -= 1;
                // Sleep after failed check, before next retry
                if retries > 0 {
                    sleep(Duration::from_millis(1000)).await;
                }
                continue;
            };

            // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
            let blob_bytes_clone = blob_bytes.clone();
            let is_bundle =
                tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&blob_bytes_clone))
                    .await?;

            if is_bundle {
                app_id_result = self
                    .node_client
                    .install_application_from_bundle_blob(blob_id, &source.clone().into())
                    .await?;
                installed = true;
            } else {
                // Not a bundle after all, use regular installation
                app_id_result = self.node_client.install_application(
                    blob_id,
                    application.size,
                    &source.clone().into(),
                    application.metadata.clone(),
                    "unknown",
                    "0.0.0",
                    false, // is_bundle: false
                )?;
                installed = true;
            }
        }

        if !installed {
            warn!(
                blob_id = %blob_id,
                "Blob didn't arrive within retry window - bundle installation will be retried when blob arrives"
            );
            // Blob didn't arrive in time - we can't install without package/version from manifest
            // Return the ApplicationId from context config to pass the check
            // The application will be installed when blob arrives via blob sharing
            // This will cause initiate_sync_inner to fail with "application not found",
            // but blob sharing will happen and installation will succeed on retry
            Ok(application.id)
        } else {
            Ok(app_id_result)
        }
    }

    /// Install application using fallback method (regular WASM installation).
    ///
    /// Only creates ApplicationMeta if blob exists locally. This prevents dangling
    /// references that would cause failures when accessing the application before
    /// the blob arrives. Returns `None` if blob doesn't exist yet (installation
    /// will be deferred until blob arrives via blob sharing).
    fn install_application_fallback(
        &self,
        application: &calimero_primitives::application::Application,
        source: &Url,
        metadata: &[u8],
        blob_id: &calimero_primitives::blobs::BlobId,
    ) -> eyre::Result<Option<calimero_primitives::application::ApplicationId>> {
        debug!(
            blob_id = %blob_id,
            "Checking if blob exists before creating ApplicationMeta"
        );

        // Only create ApplicationMeta if blob exists locally
        // This prevents dangling references that would cause failures
        if !self.node_client.has_blob(blob_id)? {
            debug!(
                blob_id = %blob_id,
                "Blob doesn't exist yet, deferring ApplicationMeta creation until blob arrives"
            );
            // Return None to indicate installation should be deferred
            // The application will be installed when blob arrives via blob sharing
            return Ok(None);
        }

        debug!(
            blob_id = %blob_id,
            "Blob exists, creating ApplicationMeta entry"
        );

        // Blob exists, safe to create ApplicationMeta entry
        Ok(Some(self.node_client.install_application(
            blob_id,
            application.size,
            &source.clone().into(),
            metadata.to_vec(),
            "unknown",
            "0.0.0",
            false, // is_bundle: false
        )?))
    }
}
