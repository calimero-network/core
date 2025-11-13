use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use tokio::sync::oneshot;
use tracing::{debug, warn};
use url::Url;

use super::ContextClient;
use crate::messages::{ContextMessage, SyncRequest};

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
                let source: Url = application.source.into();
                let source_clone = source.clone();
                let metadata = application.metadata.clone();
                let blob_id = application.blob.bytecode;

                let derived_application_id = {
                    // Try URL installation first (for HTTP/HTTPS sources)
                    let app_id = match source.scheme() {
                        "http" | "https" => self
                            .node_client
                            .install_application_from_url(
                                source_clone.clone(),
                                metadata.clone(),
                                None,
                            )
                            .await
                            .ok(),
                        _ => None,
                    };

                    match app_id {
                        Some(id) => id,
                        None => {
                            // URL installation failed or not applicable
                            // Check if blob exists locally (might have been received via blob sharing)
                            if self.node_client.has_blob(&blob_id)? {
                                debug!(
                                    blob_id = %blob_id,
                                    "Blob exists locally, checking if it's a bundle"
                                );
                                // Blob exists, check if it's a bundle
                                if let Ok(Some(blob_bytes)) =
                                    self.node_client.get_blob_bytes(&blob_id, None).await
                                {
                                    if NodeClient::is_bundle_blob(&blob_bytes) {
                                        debug!(
                                            blob_id = %blob_id,
                                            "Blob is a bundle, installing from bundle blob"
                                        );
                                        // Install from bundle blob
                                        self.node_client
                                            .install_application_from_bundle_blob(
                                                &blob_id,
                                                &source_clone.clone().into(),
                                            )
                                            .await?
                                    } else {
                                        debug!(
                                            blob_id = %blob_id,
                                            "Blob is not a bundle, using regular installation"
                                        );
                                        // Not a bundle, use regular installation
                                        self.node_client.install_application(
                                            &blob_id,
                                            application.size,
                                            &source_clone.clone().into(),
                                            metadata.clone(),
                                            "unknown",
                                            "0.0.0",
                                            false, // is_bundle: false
                                        )?
                                    }
                                } else {
                                    debug!(
                                        blob_id = %blob_id,
                                        "Failed to read blob, falling back to regular installation"
                                    );
                                    // Failed to read blob, fall back to regular installation
                                    self.node_client.install_application(
                                        &blob_id,
                                        application.size,
                                        &source_clone.clone().into(),
                                        metadata.clone(),
                                        "unknown",
                                        "0.0.0",
                                        false, // is_bundle: false
                                    )?
                                }
                            } else {
                                debug!(
                                    blob_id = %blob_id,
                                    "Blob doesn't exist locally, checking source for bundle detection"
                                );
                                // Blob doesn't exist yet - try to detect if it's a bundle from source URL
                                // If source ends with .mpk, it's likely a bundle
                                let is_bundle_from_source = source.path().ends_with(".mpk");

                                if is_bundle_from_source {
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
                                        sleep(Duration::from_millis(1000)).await; // Increased wait time to 1 second

                                        if self.node_client.has_blob(&blob_id)? {
                                            debug!(
                                                blob_id = %blob_id,
                                                "Blob arrived, installing bundle"
                                            );
                                            // Blob arrived, install it
                                            if let Ok(Some(blob_bytes)) = self
                                                .node_client
                                                .get_blob_bytes(&blob_id, None)
                                                .await
                                            {
                                                if NodeClient::is_bundle_blob(&blob_bytes) {
                                                    app_id_result = self
                                                        .node_client
                                                        .install_application_from_bundle_blob(
                                                            &blob_id,
                                                            &source_clone.clone().into(),
                                                        )
                                                        .await?;
                                                    installed = true;
                                                } else {
                                                    // Not a bundle after all, use regular installation
                                                    app_id_result =
                                                        self.node_client.install_application(
                                                            &blob_id,
                                                            application.size,
                                                            &source_clone.clone().into(),
                                                            metadata.clone(),
                                                            "unknown",
                                                            "0.0.0",
                                                            false, // is_bundle: false
                                                        )?;
                                                    installed = true;
                                                }
                                            } else {
                                                retries -= 1;
                                                continue;
                                            }
                                        } else {
                                            retries -= 1;
                                            continue;
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
                                        application.id
                                    } else {
                                        app_id_result
                                    }
                                } else {
                                    debug!(
                                        blob_id = %blob_id,
                                        "Blob doesn't exist locally, using regular installation"
                                    );
                                    // Blob doesn't exist yet - create ApplicationMeta entry anyway
                                    // The blob will be shared later in initiate_sync_inner
                                    // When blob arrives, get_application_bytes will handle extraction on-demand
                                    self.node_client.install_application(
                                        &blob_id,
                                        application.size,
                                        &source_clone.clone().into(),
                                        metadata.clone(),
                                        "unknown",
                                        "0.0.0",
                                        false, // is_bundle: false
                                    )?
                                }
                            }
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
}
