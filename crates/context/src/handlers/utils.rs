use tracing::{debug, error, info, warn};

use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
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
    node_client: &NodeClient,
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
            ContextMutation::CreateContext {
                protocol,
                application_id,
                init_args,
                alias,
            } => {
                info!(%context_id, %protocol, alias=?alias, "WASM requested CreateContext");

                let app_id = calimero_primitives::application::ApplicationId::from(*application_id);

                // We don't pass identity_secret, so a new identity is generated for the new context owner.
                // The seed is also not passed as it should not be transferred via host function
                // and should be generated instead.
                match context_client
                    .create_context(protocol.clone(), &app_id, None, init_args.clone(), None)
                    .await
                {
                    Ok(response) => {
                        let new_context_id = response.context_id;
                        info!(
                            parent_context=%context_id,
                            new_context=%new_context_id,
                            "Context created successfully via WASM host function"
                        );

                        // If an alias was provided, register it in the node
                        if let Some(alias_str) = alias {
                            let alias: Alias<PublicKey> = Alias::new(alias_str);

                            // Map the alias to the new ContextId.
                            let new_context_id_as_key = PublicKey::from(*new_context_id);

                            match node_client.create_alias(
                                alias,
                                // Scope to parent context that created that mutation.
                                Some(context_id),
                                new_context_id_as_key,
                            ) {
                                Ok(_) => {
                                    info!(%context_id, alias=%alias_str, child_context=%new_context_id, "Alias registered for child context")
                                }
                                Err(e) => error!(%context_id, error=?e, "Failed to register alias"),
                            }
                        }
                    }
                    Err(e) => {
                        error!(%context_id, error=?e, "Failed to process CreateContext request");
                    }
                }
            }
            ContextMutation::DeleteContext {
                context_id: target_ctx_bytes,
            } => {
                let target_ctx = ContextId::from(*target_ctx_bytes);
                info!(%context_id, target=%target_ctx, "WASM requested DeleteContext");

                match context_client.delete_context(&target_ctx).await {
                    Ok(_) => {
                        info!(%context_id, target=%target_ctx, "Context deleted successfully via WASM host function");
                    }
                    Err(e) => {
                        error!(%context_id, target=%target_ctx, error=?e, "Failed to process DeleteContext request");
                    }
                }
            }
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
