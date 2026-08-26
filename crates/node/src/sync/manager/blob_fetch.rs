//! Blob / application fetching for [`SyncManager`]: resolving a context's
//! blob id + application config, querying application size/source, and
//! installing a bundle after blob sharing. Extracted from the manager
//! god-file as an `impl SyncManager` fragment.

use calimero_app_downloader::{AppRequest, Outcome};
use calimero_context::handlers::upgrade_group::registry_coords;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_store::key;
use eyre::bail;

use super::SyncManager;

impl SyncManager {
    /// The bytecode blob this context's application row names.
    pub(super) async fn get_blob_info(
        &self,
        context_id: &ContextId,
        application: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<calimero_primitives::blobs::BlobId> {
        if let Some(ref app) = application {
            return Ok(app.blob.bytecode);
        }
        // No row under the context's application id, which is what
        // `get_context_application` resolves too - so this errors, and is the
        // one place that names why the session cannot proceed.
        Ok(self
            .context_client
            .get_context_application(context_id)
            .await?
            .blob
            .bytecode)
    }

    /// Acquire a context's application bytecode from the ONE source this node
    /// is configured with - never from peers behind an operator's back.
    ///
    /// `false` is "the source had nothing yet", never a fault: the caller skips
    /// what it was staging and the next access retries.
    pub(super) async fn acquire_context_bytecode(
        &self,
        context: &calimero_primitives::context::Context,
        application: &calimero_primitives::application::Application,
    ) -> bool {
        // Read off the row, not `Application`: the latter's version is
        // semver-validated, and a registry coordinate never has to be semver.
        let coords = self
            .context_client
            .datastore_handle()
            .get(&key::ApplicationMeta::new(context.application_id))
            .ok()
            .flatten()
            .and_then(|row| registry_coords(&row).ok());
        let (package, version) = coords.as_ref().map_or(("", ""), |(package, version)| {
            (package.as_str(), version.as_str())
        });
        let outcome = self
            .node_client
            .acquire_bytecode(&AppRequest {
                bytecode_id: Some(application.blob.bytecode),
                application_id: Some(context.application_id),
                package,
                version,
                context_id: Some(&context.id),
            })
            .await;
        outcome != Outcome::Unavailable
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
        context: &calimero_primitives::context::Context,
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

        let source = self
            .context_client
            .get_context_application(context_id)
            .await?
            .source;

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
            // Adopt the known id rather than re-deriving it: a raw-wasm id
            // hashes source+metadata, which vary per node.
            self.node_client.write_application_row(
                &context.application_id,
                blob_id,
                blob_bytes.len() as u64,
                &source,
                None,
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

        // The group named an application id; a bundle deriving a different one
        // is a different application, and repointing the context at it would
        // hand whoever served the bytes the power to swap a context's app.
        if installed_app_id != context.application_id {
            bail!(
                "bundle blob {blob_id} derives application {installed_app_id}, \
                 not the {} this context targets",
                context.application_id
            );
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
}
