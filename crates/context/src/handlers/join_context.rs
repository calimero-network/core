use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{JoinContextRequest, JoinContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{ContextConfigParams, ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::eyre;

use crate::ContextManager;

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ResponseFuture<<JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest { invitation_payload }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_client = self.node_client.clone();
        let context_client = self.context_client().clone();

        let task = async move {
            let (context_id, invitee_id) =
                join_context(node_client, context_client, invitation_payload).await?;

            Ok(JoinContextResponse {
                context_id,
                member_public_key: invitee_id,
            })
        };

        Box::pin(task)
    }
}

/// Join a context using an invitation.
///
/// **Idempotent**: Safe to call multiple times with same invitation.
///
/// **Flow**:
/// 1. Check if already fully joined (has full identity + state synced)
/// 2. If not, get private_key from pool (ContextId::zero)
/// 3. Sync blockchain config (creates ghost identities for all members)
/// 4. Upgrade our ghost to full identity (add private_key + sender_key)
/// 5. Subscribe to gossipsub and trigger sync
///
/// **Invariants Maintained**:
/// - Invitee has full identity (private_key + sender_key) after completion
/// - Sync requested (state will be initialized eventually)
/// - Pool identity cleaned up (no duplicates)
///
/// **Ghost Identities**: sync_context_config() creates ghosts for ALL members.
/// We immediately upgrade ours to full. Others remain ghosts until key exchange.
async fn join_context(
    node_client: NodeClient,
    context_client: ContextClient,
    invitation_payload: ContextInvitationPayload,
) -> eyre::Result<(ContextId, PublicKey)> {
    let (context_id, invitee_id, protocol, network_id, contract_id) = invitation_payload.parts()?;

    tracing::info!(%context_id, %invitee_id, "join_context: Starting join flow");

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 1: Check if already fully joined
    // ═══════════════════════════════════════════════════════════════════════════
    
    if let Some(identity) = context_client.get_identity(&context_id, &invitee_id)? {
        if identity.private_key.is_some() {
            // We have full identity - just ensure state is synced
            tracing::info!(
                %context_id,
                %invitee_id,
                has_sender_key = identity.sender_key.is_some(),
                "join_context: Already have full identity, checking state sync"
            );

            let context = context_client.get_context(&context_id)?;
            let needs_sync = context
                .map(|ctx| {
                    let is_empty = ctx.dag_heads.is_empty();
                    tracing::info!(
                        %context_id,
                        %invitee_id,
                        dag_heads_count = ctx.dag_heads.len(),
                        root_hash = %ctx.root_hash,
                        needs_sync = is_empty,
                        "join_context: State check - initialized = has dag_heads"
                    );
                    is_empty
                })
                .unwrap_or(true); // No context = definitely need sync

            if needs_sync {
                tracing::info!(%context_id, %invitee_id, "join_context: State not synced, waiting for sync to complete");
                node_client.subscribe(&context_id).await?;
                
                // CRITICAL: Wait for sync to actually complete before returning!
                // This ensures context is initialized when join_context returns.
                match node_client.sync_and_wait(Some(&context_id), None).await {
                    Ok(sync_result) => {
                        tracing::info!(
                            %context_id,
                            %invitee_id,
                            ?sync_result,
                            "Sync completed successfully - context is now initialized"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            %context_id,
                            %invitee_id,
                            error = %e,
                            "Sync FAILED - context will remain uninitialized!"
                        );
                        return Err(e.wrap_err("Failed to sync context state after join"));
                    }
                }
            }

            return Ok((context_id, invitee_id));
        }
        
        // Ghost identity exists (from previous sync_context_config call)
        // This is expected - we'll upgrade it below
        tracing::info!(
            %context_id,
            %invitee_id,
            "join_context: Found ghost identity, will upgrade to full identity"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 2: Get private_key from pool (ONLY source of private keys!)
    // ═══════════════════════════════════════════════════════════════════════════
    
    let stored_identity = context_client
        .get_identity(&ContextId::zero(), &invitee_id)?
        .ok_or_else(|| eyre!("Missing identity in pool (ContextId::zero) for: {}", invitee_id))?;

    let identity_secret = stored_identity
        .private_key
        .ok_or_else(|| eyre!("Pool identity '{}' missing private_key (invariant violation!)", invitee_id))?;

    // Sanity check: private_key matches public_key
    if identity_secret.public_key() != invitee_id {
        eyre::bail!("Identity mismatch: private_key doesn't match public_key");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 3: Sync blockchain config (creates ghosts for ALL members, including us)
    // ═══════════════════════════════════════════════════════════════════════════
    
    let config = if !context_client.has_context(&context_id)? {
        // Build external config from invitation parameters
        tracing::info!(%context_id, "join_context: Context doesn't exist locally, fetching from blockchain");
        
        let mut external_config = ContextConfigParams {
            protocol: protocol.into(),
            network_id: network_id.into(),
            contract_id: contract_id.into(),
            proxy_contract: "".into(),
            application_revision: 0,
            members_revision: 0,
        };

        let external_client = context_client.external_client(&context_id, &external_config)?;
        let config_client = external_client.config();
        let proxy_contract = config_client.get_proxy_contract().await?;

        external_config.proxy_contract = proxy_contract.into();
        
        Some(external_config)
    } else {
        None
    };

    tracing::info!(%context_id, %invitee_id, "join_context: Syncing blockchain config");
    let _ignored = context_client
        .sync_context_config(context_id, config)
        .await?;

    // At this point: Our identity might be a ghost (created by sync_context_config)

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 4: Verify membership (we should be in member list from blockchain)
    // ═══════════════════════════════════════════════════════════════════════════
    
    if !context_client.has_member(&context_id, &invitee_id)? {
        eyre::bail!("Unable to join context: not in member list on blockchain. Invalid invitation?");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 5: Upgrade from ghost to full identity (ATOMIC)
    // ═══════════════════════════════════════════════════════════════════════════
    
    tracing::info!(%context_id, %invitee_id, "join_context: Upgrading to full identity");
    
    let mut rng = rand::thread_rng();
    let sender_key = PrivateKey::random(&mut rng);

    context_client.update_identity(
        &context_id,
        &ContextIdentity {
            public_key: invitee_id,
            private_key: Some(identity_secret),
            sender_key: Some(sender_key),
        },
    )?;

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 6: Cleanup - remove from pool (no longer needed)
    // ═══════════════════════════════════════════════════════════════════════════
    
    tracing::info!(%context_id, %invitee_id, "join_context: Removing identity from pool");
    context_client.delete_identity(&ContextId::zero(), &invitee_id)?;

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 7: Subscribe and WAIT for initial sync (CRITICAL!)
    // ═══════════════════════════════════════════════════════════════════════════
    
    tracing::info!(%context_id, %invitee_id, "join_context: Subscribing and waiting for initial sync");
    node_client.subscribe(&context_id).await?;
    
    // CRITICAL: Wait for sync to complete before returning!
    // This ensures context is fully initialized when join_context returns.
    // Old behavior: Fire-and-forget sync → context appeared joined but couldn't execute.
    // New behavior: Block until sync completes → guaranteed initialized on return.
    match node_client.sync_and_wait(Some(&context_id), None).await {
        Ok(sync_result) => {
            tracing::info!(
                %context_id,
                %invitee_id,
                ?sync_result,
                "Initial sync completed successfully - context ready for use"
            );
        }
        Err(e) => {
            tracing::error!(
                %context_id,
                %invitee_id,
                error = %e,
                "Initial sync FAILED - returning error to prevent broken context"
            );
            return Err(e.wrap_err("Failed to complete initial sync after joining context"));
        }
    }
    
    tracing::info!(%context_id, %invitee_id, "join_context: Complete - context fully initialized");

    Ok((context_id, invitee_id))
}
