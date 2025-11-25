//! Application installation helpers for sync manager.
//!
//! This module provides helper functions for retrieving application information
//! and installing bundle applications after blob sharing completes.

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::Context;
use calimero_primitives::context::ContextId;
use eyre::{bail, eyre};

use super::SyncManager;

impl SyncManager {
    /// Get blob ID and application config from application or context config
    pub(super) async fn get_blob_info(
        &self,
        context_id: &ContextId,
        application: &Option<Application>,
    ) -> eyre::Result<(BlobId, Option<Application>)> {
        if let Some(ref app) = application {
            Ok((app.blob.bytecode, None))
        } else {
            // Application not found - get blob_id from context config
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok((app_config.blob.bytecode, Some(app_config)))
        }
    }

    /// Get application size from application, cached config, or context config
    pub(super) async fn get_application_size(
        &self,
        context_id: &ContextId,
        application: &Option<Application>,
        app_config_opt: &Option<Application>,
    ) -> eyre::Result<u64> {
        if let Some(ref app) = application {
            Ok(app.size)
        } else if let Some(ref app_config) = app_config_opt {
            Ok(app_config.size)
        } else {
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok(app_config.size)
        }
    }

    /// Get application source from cached config or context config
    pub(super) async fn get_application_source(
        &self,
        context_id: &ContextId,
        app_config_opt: &Option<Application>,
    ) -> eyre::Result<ApplicationSource> {
        if let Some(ref app_config) = app_config_opt {
            Ok(app_config.source.clone())
        } else {
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok(app_config.source.clone())
        }
    }

    /// Install bundle application after blob sharing completes.
    ///
    /// Returns `Some((installed_application, installed_app_id))` if a bundle was installed,
    /// `None` otherwise. The `installed_app_id` may differ from `context.application_id` if
    /// the context config has a stale ApplicationId. The caller should persist this change
    /// to the database if they differ.
    pub(super) async fn install_bundle_after_blob_sharing(
        &self,
        context_id: &ContextId,
        blob_id: &BlobId,
        app_config_opt: &Option<Application>,
        context: &Context,
    ) -> eyre::Result<Option<(Application, ApplicationId)>> {
        // Only proceed if blob is now available locally
        if !self.node_client.has_blob(blob_id)? {
            return Ok(None);
        }

        // Check if blob is a bundle
        let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
            return Ok(None);
        };

        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let blob_bytes_clone = blob_bytes.clone();
        let is_bundle =
            tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&blob_bytes_clone))
                .await?;

        if !is_bundle {
            return Ok(None);
        }

        // Get source from context config (use cached if available, otherwise fetch)
        let source = self
            .get_application_source(context_id, app_config_opt)
            .await?;

        // Install bundle
        let installed_app_id = self
            .node_client
            .install_application_from_bundle_blob(blob_id, &source.into())
            .await
            .map_err(|e| {
                eyre::eyre!(
                    "Failed to install bundle application from blob {}: {}",
                    blob_id,
                    e
                )
            })?;

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
            use tracing::warn;
            warn!(
                installed_app_id = %installed_app_id,
                context_app_id = %context.application_id,
                "Installed application ID does not match context application ID, will persist update"
            );
        }

        Ok(Some((installed_application, installed_app_id)))
    }
}
