//! Context configuration synchronization and application installation.
//!
//! This module handles syncing context configuration from external sources,
//! installing applications (both bundles and regular WASM), and managing
//! context metadata updates.

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use eyre::{eyre, WrapErr};
use std::collections::BTreeSet;
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};
use url::Url;

use super::external::ExternalClient;
use super::ContextClient;
use crate::messages::{ContextMessage, SyncRequest};

impl ContextClient {
    // Constants for application installation
    const DEFAULT_PACKAGE: &str = "unknown";
    const DEFAULT_VERSION: &str = "0.0.0";
    const MAX_BLOB_RETRIES: u32 = 20;
    const BLOB_RETRY_DELAY_MS: u64 = 1000;
    const MEMBERS_PAGE_SIZE: usize = 100;
    // Constants for member sync retry logic
    const MAX_MEMBER_SYNC_RETRIES: u32 = 3;
    const MEMBER_SYNC_RETRY_DELAY_MS: u64 = 500;

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

        // Fetch the LATEST revision from the blockchain with retry logic
        let external_client = self.external_client(&context_id, &config)?;
        let mut remote_members_revision = None;
        let mut last_error = None;
        
        // Retry fetching members revision
        for attempt in 1..=Self::MAX_MEMBER_SYNC_RETRIES {
            match external_client.config().members_revision().await {
                Ok(rev) => {
                    remote_members_revision = Some(rev);
                    break;
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < Self::MAX_MEMBER_SYNC_RETRIES {
                        warn!(
                            %context_id,
                            attempt,
                            error = ?last_error,
                            "Failed to fetch members revision, retrying..."
                        );
                        sleep(Duration::from_millis(
                            Self::MEMBER_SYNC_RETRY_DELAY_MS * attempt as u64
                        )).await;
                    }
                }
            }
        }
        
        let remote_members_revision = remote_members_revision.ok_or_else(|| {
            last_error.unwrap_or_else(|| eyre!("Failed to fetch members revision"))
        }).wrap_err(format!(
            "Failed to fetch members revision after {} retries",
            Self::MAX_MEMBER_SYNC_RETRIES
        ))?;

        // Check members revision and sync members, if needed
        if context.is_none() || remote_members_revision != config.members_revision {
            tracing::info!(
                %context_id,
                local_members_revision = config.members_revision,
                remote_members_revision,
                "Members revision changed, synchronizing member list...",
            );

            should_save_config = true;
            config.members_revision = remote_members_revision;

            // Perform the sync of the members with retry logic
            let external_client = self.external_client(&context_id, &config)?;
            // If sync_members fails, log warning but continue - periodic sync will retry
            if let Err(e) = self.sync_members(context_id, &external_client).await {
                warn!(
                    %context_id,
                    error = ?e,
                    "Failed to sync members during join - periodic sync will retry"
                );
                // Don't fail the entire join - allow it to continue
                // The periodic sync manager will retry member sync later
            }
        } else {
            debug!(
                %context_id,
                local_members_revision = config.members_revision,
                remote_members_revision,
                "Members revision was not changed, skipping sync",
            );
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
                let source: Url = application.source.into();
                let metadata = application.metadata.clone();
                let blob_id = application.blob.bytecode;

                let derived_application_id = {
                    // Try URL installation first (for HTTP/HTTPS sources)
                    if let Some(app_id) = self.try_install_from_url(&source, &metadata).await? {
                        app_id
                    } else {
                        // URL installation failed or not applicable
                        // Check if blob exists locally (might have been received via blob sharing)
                        if self.node_client.has_blob(&blob_id)? {
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
                                application.id,
                            )
                            .await?
                        }
                    }
                };

                if application.id != derived_application_id {
                    eyre::bail!(
                        "application mismatch: expected {}, got {}",
                        application.id,
                        derived_application_id
                    )
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

    /// Synchronizes the local member list with the authoritative state from the blockchain.
    ///
    /// These actions are performed:
    /// 1. Fetch the complete list of members from the external contract.
    /// 2. Adds any missing members to the local `datastore`.
    /// 3. Prunes (deletes) any local members that are no longer present in the external contract.
    async fn sync_members(
        &self,
        context_id: ContextId,
        external_client: &ExternalClient<'_>,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();
        let config_client = external_client.config();

        let mut external_members = BTreeSet::new();

        // Fetch ALL remote members with retry logic for resilience to connection failures
        for (offset, length) in (0..).map(|i| {
            (
                Self::MEMBERS_PAGE_SIZE.saturating_mul(i),
                Self::MEMBERS_PAGE_SIZE,
            )
        }) {
            // Retry logic for fetching members page
            let mut members = Vec::new();
            let mut last_error = None;
            
            for attempt in 1..=Self::MAX_MEMBER_SYNC_RETRIES {
                match config_client.members(offset, length).await {
                    Ok(page_members) => {
                        members = page_members;
                        last_error = None;
                        break;
                    }
                    Err(e) => {
                        last_error = Some(e);
                        if attempt < Self::MAX_MEMBER_SYNC_RETRIES {
                            warn!(
                                %context_id,
                                offset,
                                attempt,
                                error = ?last_error,
                                "Failed to fetch members page, retrying..."
                            );
                            sleep(Duration::from_millis(
                                Self::MEMBER_SYNC_RETRY_DELAY_MS * attempt as u64
                            )).await;
                        }
                    }
                }
            }
            
            // If all retries failed, return error
            if let Some(err) = last_error {
                return Err(err).wrap_err(format!(
                    "Failed to fetch members page after {} retries",
                    Self::MAX_MEMBER_SYNC_RETRIES
                ));
            }
            
            if members.is_empty() {
                break;
            }

            for member in members {
                external_members.insert(member);

                // Upsert: add to local DB if missing
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

        // PRUNING stage.
        // Identify members that exist locally but NOT remotely.
        let mut members_to_remove = Vec::new();

        // Create a scope for the iterator to avoid borrowing issues
        {
            if let Ok(mut iter) = handle.iter::<key::ContextIdentity>() {
                let start_key = key::ContextIdentity::new(context_id, [0u8; DIGEST_SIZE].into());

                // Capture the first element from seek() and chain with the rest to avoid skipping the first member
                let first = iter.seek(start_key).ok().flatten();

                // Iterate over the first item + the rest of the keys
                for k in first.into_iter().chain(iter.keys().flatten()) {
                    // Stop if we drifted to another context
                    if k.context_id() != context_id {
                        break;
                    }

                    // If local member is missing from external set -> Mark for removal
                    if !external_members.contains(&k.public_key()) {
                        members_to_remove.push(*k.public_key());
                    }
                }
            }
        }

        // Execute deletions of members that exist in the local DB, but don't exist remotely anymore
        for member in members_to_remove {
            let member_public_key = member.into();
            debug!(%context_id, %member_public_key, "Trying to prune member from local store (it was removed from the contract)");

            let key = key::ContextIdentity::new(context_id, member_public_key);
            handle.delete(&key)?;

            info!(%context_id, %member_public_key, "Pruned member from local store (it was removed from the contract)");
        }

        Ok(())
    }
}
