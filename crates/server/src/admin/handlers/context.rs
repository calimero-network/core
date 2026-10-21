use calimero_primitives::context::ContextId;

use crate::AdminState;

/// The release `context_id` executes, when it differs from the application
/// row's version (the row names the latest install).
pub(crate) async fn executing_application_version(
    state: &AdminState,
    context_id: &ContextId,
) -> Option<String> {
    let blob = calimero_context::activation::executing_blob_off_row(
        state.ctx_client.datastore(),
        context_id,
    )?;
    state
        .node_client
        .blob_app_version(&calimero_primitives::blobs::BlobId::from(blob))
        .await
}

pub mod create_context;
pub mod create_context_intent;
pub mod delete_context;
mod founding_attestation;
pub mod get_context;
pub mod get_context_group;
pub mod get_context_identities;
pub mod get_context_ids;
pub mod get_context_storage;
pub mod get_contexts_for_application;
pub mod get_contexts_with_executors_for_application;
pub mod governance_intent;
pub mod intent_relay;
pub mod join_context;
pub mod leave_context;
pub mod perform_intent;
pub mod presence_intent;
pub mod query_context;
pub mod resync_context;
pub mod sync;
pub mod update_context_application;
