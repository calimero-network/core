use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{JoinContextRequest, JoinContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{ContextConfigParams, ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::{eyre, WrapErr};

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

/// Join a context using an invitation payload.
///
/// Sets up identity, syncs blockchain config, subscribes to gossipsub, and fetches historical state.
/// Returns when context is fully initialized and ready for execution.
async fn join_context(
    node_client: NodeClient,
    context_client: ContextClient,
    invitation_payload: ContextInvitationPayload,
) -> eyre::Result<(ContextId, PublicKey)> {
    let (context_id, invitee_id, protocol, network_id, contract_id) = invitation_payload.parts()?;

    tracing::info!(%context_id, %invitee_id, "Starting join flow");

    // Check if already joined
    if let Some(identity) = context_client.get_identity(&context_id, &invitee_id)? {
        if identity.private_key.is_some() {
            // Already joined - just check if state needs sync
            tracing::info!(%context_id, %invitee_id, "Already joined, checking state sync");

            let context = context_client.get_context(&context_id)?;
            let needs_sync = context.map(|ctx| ctx.dag_heads.is_empty()).unwrap_or(true);

            if needs_sync {
                node_client.subscribe(&context_id).await?;
                node_client
                    .sync_and_wait(Some(&context_id), None)
                    .await
                    .wrap_err("Failed to sync after join")?;
            }

            return Ok((context_id, invitee_id));
        }
    }

    // Get private_key from identity pool
    let stored_identity = context_client
        .get_identity(&ContextId::zero(), &invitee_id)?
        .ok_or_else(|| eyre!("Missing identity in pool for {}", invitee_id))?;

    let identity_secret = stored_identity
        .private_key
        .ok_or_else(|| eyre!("Pool identity missing private_key"))?;

    if identity_secret.public_key() != invitee_id {
        eyre::bail!("Identity mismatch");
    }

    // Fetch context config from blockchain if needed
    let config = if !context_client.has_context(&context_id)? {
        let mut external_config = ContextConfigParams {
            protocol: protocol.into(),
            network_id: network_id.into(),
            contract_id: contract_id.into(),
            proxy_contract: "".into(),
            application_revision: 0,
            members_revision: 0,
        };

        let external_client = context_client.external_client(&context_id, &external_config)?;
        let proxy_contract = external_client.config().get_proxy_contract().await?;
        external_config.proxy_contract = proxy_contract.into();

        Some(external_config)
    } else {
        None
    };

    // Sync blockchain config (creates member identities)
    context_client
        .sync_context_config(context_id, config)
        .await?;

    // Verify membership
    if !context_client.has_member(&context_id, &invitee_id)? {
        eyre::bail!("Not in member list - invalid invitation");
    }

    // Upgrade to full identity with sender_key
    let sender_key = PrivateKey::random(&mut rand::thread_rng());
    context_client.update_identity(
        &context_id,
        &ContextIdentity {
            public_key: invitee_id,
            private_key: Some(identity_secret),
            sender_key: Some(sender_key),
        },
    )?;

    // Remove from pool
    context_client.delete_identity(&ContextId::zero(), &invitee_id)?;

    // Subscribe and sync historical state
    // Gossipsub only delivers new deltas - must fetch history via P2P
    node_client.subscribe(&context_id).await?;
    node_client
        .sync_and_wait(Some(&context_id), None)
        .await
        .wrap_err("Failed to sync historical state")?;

    tracing::info!(%context_id, %invitee_id, "Join complete");

    Ok((context_id, invitee_id))
}
