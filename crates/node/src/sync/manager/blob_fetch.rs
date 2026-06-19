//! Blob / application fetching for [`SyncManager`]: resolving a context's
//! blob id + application config, querying application size/source, and
//! installing a bundle after blob sharing. Extracted from the manager
//! god-file as an `impl SyncManager` fragment.

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{
    AppVersionChangedPayload, ContextEvent, ContextEventPayload, NodeEvent,
};
use eyre::bail;
use tracing::{debug, warn};

use super::SyncManager;

impl SyncManager {
    /// Get blob ID and application config from application or context config
    pub(super) async fn get_blob_info(
        &self,
        context_id: &ContextId,
        application: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<(
        calimero_primitives::blobs::BlobId,
        Option<calimero_primitives::application::Application>,
    )> {
        if let Some(ref app) = application {
            Ok((app.blob.bytecode, None))
        } else {
            // Application not found - get blob_id from context config
            let app_config = self
                .context_client
                .get_context_application(context_id)
                .await?;
            Ok((app_config.blob.bytecode, Some(app_config)))
        }
    }

    /// Get application size from application, cached config, or context config
    pub(super) async fn get_application_size(
        &self,
        context_id: &ContextId,
        application: &Option<calimero_primitives::application::Application>,
        app_config_opt: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<u64> {
        if let Some(ref app) = application {
            Ok(app.size)
        } else if let Some(ref app_config) = app_config_opt {
            Ok(app_config.size)
        } else {
            let app_config = self
                .context_client
                .get_context_application(context_id)
                .await?;
            Ok(app_config.size)
        }
    }

    /// Get application source from cached config or context config
    async fn get_application_source(
        &self,
        context_id: &ContextId,
        app_config_opt: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<calimero_primitives::application::ApplicationSource> {
        if let Some(ref app_config) = app_config_opt {
            Ok(app_config.source.clone())
        } else {
            let app_config = self
                .context_client
                .get_context_application(context_id)
                .await?;
            Ok(app_config.source.clone())
        }
    }

    /// Install bundle application after blob sharing completes.
    ///
    /// Returns `Some(installed_application)` if a bundle was installed,
    /// `None` otherwise. Updates `context.application_id` if the installed
    /// ApplicationId differs from the context's ApplicationId.
    pub(crate) async fn install_bundle_after_blob_sharing(
        &self,
        context_id: &ContextId,
        blob_id: &calimero_primitives::blobs::BlobId,
        app_config_opt: &Option<calimero_primitives::application::Application>,
        context: &mut calimero_primitives::context::Context,
        application: &mut Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<()> {
        // Only proceed if blob is now available locally
        if !self.node_client.has_blob(blob_id)? {
            return Ok(());
        }

        // Check if blob is a bundle
        let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
            return Ok(());
        };

        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let blob_bytes_clone = blob_bytes.clone();
        let is_bundle =
            tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&blob_bytes_clone))
                .await?;

        // Get source from context config (use cached if available, otherwise fetch)
        let source = self
            .get_application_source(context_id, app_config_opt)
            .await?;

        let installed_app_id = if is_bundle {
            self.node_client
                .install_application_from_bundle_blob(blob_id, &source)
                .await
                .map_err(|e| {
                    eyre::eyre!(
                        "Failed to install bundle application from blob {}: {}",
                        blob_id,
                        e
                    )
                })?
        } else {
            // For non-bundle apps, write ApplicationMeta directly under the
            // known application_id rather than re-deriving it via
            // install_application (which hashes source+metadata and would
            // produce a different ID than the original installer used).
            let size = blob_bytes.len() as u64;
            let mut handle = self.context_client.datastore_handle();
            handle.put(
                &calimero_store::key::ApplicationMeta::new(context.application_id),
                &calimero_store::types::ApplicationMeta::new(
                    calimero_store::key::BlobMeta::new(*blob_id),
                    size,
                    source.to_string().into_boxed_str(),
                    Box::default(),
                    calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from(
                        [0u8; 32],
                    )),
                    calimero_store::types::PackageInfo {
                        package: "unknown".to_owned().into_boxed_str(),
                        version: "0.0.0".to_owned().into_boxed_str(),
                        signer_id: String::new().into_boxed_str(),
                    },
                ),
            )?;
            context.application_id
        };

        // Verify installation succeeded by fetching the installed application
        let installed_application = self
            .node_client
            .get_application(&installed_app_id)
            .map_err(|e| {
                eyre::eyre!(
                    "Failed to verify bundle installation for application {}: {}",
                    installed_app_id,
                    e
                )
            })?;

        let Some(installed_application) = installed_application else {
            bail!(
                "Bundle installation reported success but application {} is not retrievable",
                installed_app_id
            );
        };

        // Check if the installed ApplicationId matches the context's ApplicationId
        if installed_app_id != context.application_id {
            warn!(
                installed_app_id = %installed_app_id,
                context_app_id = %context.application_id,
                "Installed application ID does not match context application ID, updating to installed ID"
            );
            // Capture the pre-flip id for the AppVersionChanged emit below; this
            // is a durable application flip (this node just learned, via blob
            // sync, that its context's app changed), so it must notify
            // subscribers like the update_application workers do.
            let old_app_id = context.application_id;

            // Update context with the installed application ID for consistency
            context.application_id = installed_app_id;

            // Persist the ApplicationId change to the database
            // This is critical: if we don't persist, the old ApplicationId will be
            // used on node restart, causing application lookup failures
            self.context_client
                .update_context_application_id(context_id, installed_app_id)
                .map_err(|e| {
                    eyre::eyre!(
                        "Failed to persist ApplicationId update for context {}: {}",
                        context_id,
                        e
                    )
                })?;

            debug!(
                %context_id,
                installed_app_id = %installed_app_id,
                "Persisted ApplicationId update to database"
            );

            // Notify subscribers of the version flip (skew #2). Best-effort, like
            // the update_application emit. The guard above is the dedup (only a
            // genuine id change reaches here). to_version comes straight off the
            // installed Application; from_version resolves the old app row.
            let event = NodeEvent::Context(ContextEvent {
                context_id: *context_id,
                payload: ContextEventPayload::AppVersionChanged(AppVersionChangedPayload {
                    from_version: self.application_version(old_app_id),
                    to_version: Some(installed_application.version.clone())
                        .filter(|v| !v.is_empty()),
                }),
            });
            let _ = self.node_client.send_event(event);
        }

        // Use the verified installed application
        *application = Some(installed_application);

        Ok(())
    }

    /// Resolves an application's semver from its `ApplicationMeta` row via the
    /// context store; `None` when the row is absent. Labels the from-version of
    /// the blob-sync `AppVersionChanged` emit (mirrors the context-handler
    /// `application_version` helper).
    fn application_version(&self, application_id: ApplicationId) -> Option<String> {
        self.context_client
            .datastore_handle()
            .get(&calimero_store::key::ApplicationMeta::new(application_id))
            .ok()
            .flatten()
            .map(|meta| meta.version.to_string())
    }
}
