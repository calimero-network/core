//! Blob / application fetching for [`SyncManager`]: resolving a context's
//! blob id + application config, querying application size/source, and
//! installing a bundle after blob sharing. Extracted from the manager
//! god-file as an `impl SyncManager` fragment.

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

    /// Install the application after blob sharing completes, whether the blob
    /// holds a bundle or bare wasm.
    ///
    /// Updates `context.application_id` if the installed ApplicationId differs
    /// from the context's ApplicationId.
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

        // Get source from context config (use cached if available, otherwise fetch)
        let source = self
            .get_application_source(context_id, app_config_opt)
            .await?;

        let installed_app_id = self
            .node_client
            .install_application_from_blob(blob_id, &context.application_id, &source)
            .await
            .map_err(|e| {
                eyre::eyre!("Failed to install application from blob {}: {}", blob_id, e)
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
                    to_version: installed_application
                        .version
                        .as_ref()
                        .map(|v| v.as_str().to_owned()),
                }),
            });
            let _ = self.node_client.send_event(event);
        }

        // Use the verified installed application
        *application = Some(installed_application);

        // The application is runnable as of here: its bytecode blob is local
        // (checked at the top of this function) and its row is installed and
        // verified. Any state delta that arrived while it was missing was parked
        // rather than applied, so replay those now instead of leaving them for
        // the next namespace event or sync settle.
        self.drain_deltas_parked_on_application(context_id).await;

        Ok(())
    }

    /// Replay state deltas parked because this context's application was not
    /// runnable yet.
    ///
    /// The gossip apply path parks such a delta in the durable absorb buffer
    /// instead of applying it against bytecode this node cannot execute. This is
    /// the prompt trigger for the common case — the application just arrived by
    /// blob sync. Applications can also land by other routes (an operator
    /// installing one over the admin API), which is why the same drain is
    /// chained off every other absorb-drain hook as a safety net.
    async fn drain_deltas_parked_on_application(&self, context_id: &ContextId) {
        let drain_input = crate::handlers::state_delta::StateDeltaContext {
            node_clients: crate::state::NodeClients {
                context: self.context_client.clone(),
                node: self.node_client.clone(),
            },
            node_state: self.node_state.clone(),
            network_client: self.network_client.clone(),
            sync_timeout: self.sync_config.timeout,
        };
        crate::handlers::state_delta::drain_absorbed(&drain_input, context_id).await;
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
