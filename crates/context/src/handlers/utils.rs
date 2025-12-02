use tracing::{debug, error, info, warn};

use calimero_context_primitives::client::ContextClient;
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::{ContextHost, ContextMutation};
use calimero_store::{key, Store};

// A bridge implementation that exposes context information from the `calimero-store`
/// to the runtime via the `ContextHost` trait.
#[derive(Debug)]
pub struct StoreContextHost {
    pub store: Store,
    pub context_id: ContextId,
}

impl ContextHost for StoreContextHost {
    fn is_member(&self, public_key: &[u8; DIGEST_SIZE]) -> bool {
        let key = key::ContextIdentity::new(self.context_id, (*public_key).into());
        self.store.handle().has(&key).unwrap_or(false)
    }

    fn members(&self) -> Vec<[u8; DIGEST_SIZE]> {
        let handle = self.store.handle();
        // We can use the iterator from ContextClient::get_context_members logic
        // or access the store directly via iterator
        let mut members = Vec::new();

        // Create iterator for ContextIdentity prefix
        // Note: Error handling in traits is tricky if the trait doesn't return Result.
        // For runtime host functions, it's often cleaner to return empty or log error.
        if let Ok(mut iter) = handle.iter::<key::ContextIdentity>() {
            let start_key = key::ContextIdentity::new(self.context_id, [0u8; DIGEST_SIZE].into());

            // Seek to the start of this context.
            // `seek` returns the key at the cursor if found (Result<Option<K>>).
            let first = iter.seek(start_key).ok().flatten();

            // Chain the first key (if any) with the rest of the iterator (.keys())
            // We flatten the iterator results to skip database read errors for simplicity in this host function.
            for k in first.into_iter().chain(iter.keys().flatten()) {
                // Stop iteration if we drift into the next context ID scope
                if k.context_id() != self.context_id {
                    break;
                }
                members.push(*k.public_key());
            }
        }
        members
    }
}

/// Processes context configuration mutations requested by the WASM runtime.
///
/// This iterates over the `context_mutations` returned in the execution outcome
/// and performs the necessary external calls (e.g., on-chain transactions) via
/// the `ContextClient`.
pub async fn process_context_mutations(
    context_client: &ContextClient,
    context_id: ContextId,
    executor: PublicKey,
    mutations: &[ContextMutation],
) {
    if mutations.is_empty() {
        return;
    }

    info!(%context_id, count = mutations.len(), "Processing context mutations from WASM");

    for mutation in mutations {
        match mutation {
            ContextMutation::AddMember { public_key } => {
                let new_member = PublicKey::from(*public_key);
                info!(%context_id, member = %new_member, "WASM requested AddMember");

                match context_client
                    .invite_member(&context_id, &executor, &new_member)
                    .await
                {
                    Ok(_) => {
                        debug!(%context_id, %new_member, "Member invited successfully, syncing config...");
                        // Sync local state so `is_member` returns true immediately
                        if let Err(e) = context_client.sync_context_config(context_id, None).await {
                            warn!(%context_id, error = ?e, "Failed to sync context config after adding member");
                        }
                    }
                    Err(e) => {
                        error!(%context_id, %new_member, error = ?e, "Failed to process AddMember request")
                    }
                }
            }
            ContextMutation::RemoveMember { public_key } => {
                let member = PublicKey::from(*public_key);
                info!(%context_id, member = %member, "WASM requested RemoveMember");

                // We need to fetch the config to get the external client
                match context_client.context_config(&context_id) {
                    Ok(Some(config)) => {
                        match context_client.external_client(&context_id, &config) {
                            Ok(ext) => {
                                match ext.config().remove_members(&executor, &[member]).await {
                                    Ok(_) => {
                                        debug!(%context_id, %member, "Member removed successfully, syncing config...");
                                        // Sync local state after the deletion is complete
                                        if let Err(e) = context_client
                                            .sync_context_config(context_id, None)
                                            .await
                                        {
                                            warn!(%context_id, error = ?e, "Failed to sync context config after removing member");
                                        }
                                    }
                                    Err(e) => {
                                        error!(%context_id, %member, error = ?e, "Failed to execute RemoveMember")
                                    }
                                }
                            }
                            Err(e) => {
                                error!(%context_id, error = ?e, "Failed to create external client")
                            }
                        }
                    }
                    Ok(None) => error!(%context_id, "Context config not found for RemoveMember"),
                    Err(e) => error!(%context_id, error = ?e, "Failed to fetch context config"),
                }
            }
        }
    }
}
