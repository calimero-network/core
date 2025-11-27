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
        let context_client = self.context_client.clone();

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

async fn join_context(
    node_client: NodeClient,
    context_client: ContextClient,
    invitation_payload: ContextInvitationPayload,
) -> eyre::Result<(ContextId, PublicKey)> {
    let (context_id, invitee_id, protocol, network_id, contract_id) = invitation_payload.parts()?;

    tracing::info!(%context_id, %invitee_id, "join_context: starting join flow");

    // Check if we already have a fully setup identity for this context
    let already_joined = context_client
        .get_identity(&context_id, &invitee_id)?
        .and_then(|i| i.private_key)
        .is_some();

    tracing::info!(%context_id, %invitee_id, already_joined, "join_context: checked if already joined");

    if already_joined {
        // Identity exists, but check if state is initialized
        // DAG heads being empty means no state has been synced yet
        // (even if root_hash != [0;32] from external config sync)
        let context = context_client.get_context(&context_id)?;
        let needs_sync = context
            .map(|ctx| {
                let empty = ctx.dag_heads.is_empty();
                tracing::info!(
                    %context_id,
                    %invitee_id,
                    dag_heads_count = ctx.dag_heads.len(),
                    root_hash = %ctx.root_hash,
                    needs_sync = empty,
                    "join_context: identity already exists, checking if sync needed"
                );
                empty
            })
            .unwrap_or(true); // If context doesn't exist, we definitely need sync

        if needs_sync {
            tracing::info!(%context_id, %invitee_id, "join_context: triggering sync for already-joined context with empty DAG heads");
            // State is uninitialized - subscribe and trigger sync
            node_client.subscribe(&context_id).await?;
            node_client.sync(Some(&context_id), None).await?;
        }

        return Ok((context_id, invitee_id));
    }

    let stored_identity = context_client
        .get_identity(&ContextId::zero(), &invitee_id)?
        .ok_or_else(|| eyre!("missing identity for public key: {}", invitee_id))?;

    let identity_secret = stored_identity
        .private_key
        .ok_or_else(|| eyre!("stored identity '{}' is missing private key", invitee_id))?;

    if identity_secret.public_key() != invitee_id {
        eyre::bail!("identity mismatch")
    }

    let mut config = None;

    if !context_client.has_context(&context_id)? {
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

        config = Some(external_config);
    };

    // Sync context config - allow partial failures during join
    // If member sync fails, we'll retry via periodic sync
    match context_client
        .sync_context_config(context_id, config)
        .await
    {
        Ok(_) => {
            // Sync succeeded
        }
        Err(e) => {
            // Sync had issues - log warning but continue
            // The periodic sync manager will retry member sync later
            tracing::warn!(
                %context_id,
                %invitee_id,
                error = ?e,
                "Context config sync had issues during join - periodic sync will retry"
            );
            // Give a brief moment for any partial sync to complete
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    // Check if we're a member (required for join)
    if !context_client.has_member(&context_id, &invitee_id)? {
        eyre::bail!("unable to join context: not a member, invalid invitation?")
    }

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

    // Delete the identity from the zero context (a.k.a. identity pool),
    // because we just assigned that identity to the new context.
    context_client.delete_identity(&ContextId::zero(), &invitee_id)?;

    // CRITICAL: Subscribe AFTER context is fully set up in database
    // This prevents "unknown context" warnings when peers see our subscription
    tracing::info!(%context_id, %invitee_id, "join_context: NEW join - calling subscribe and sync");
    
    // Small delay to ensure context is persisted before subscribing
    // This helps avoid race conditions where peers see subscription before context exists
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    
    node_client.subscribe(&context_id).await?;

    // Trigger sync - this will catch up on any missed deltas
    node_client.sync(Some(&context_id), None).await?;
    tracing::info!(%context_id, %invitee_id, "join_context: sync request sent successfully");

    Ok((context_id, invitee_id))
}
