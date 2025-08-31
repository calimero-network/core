use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::join_context::{
    JoinContextRequest, JoinContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{ContextConfigParams, ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::eyre;
use tracing::{debug, info, warn};

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
            info!("🔄 Starting context join process");
            let (context_id, invitee_id) =
                join_context(node_client, context_client, invitation_payload).await?;
            info!("✅ Context join process completed successfully");

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

    debug!(
        "🔍 Join context: context_id={}, invitee_id={}, protocol={}, network_id={}, contract_id={}",
        context_id, invitee_id, protocol, network_id, contract_id
    );

    // Check if already joined
    if context_client
        .get_identity(&context_id, &invitee_id)?
        .and_then(|i| i.private_key)
        .is_some()
    {
        debug!(
            "ℹ️  Already joined context: context_id={}, invitee_id={}",
            context_id, invitee_id
        );
        return Ok((context_id, invitee_id));
    }

    debug!(
        "🔑 Looking up stored identity for invitee_id={}",
        invitee_id
    );
    let stored_identity = context_client
        .get_identity(&ContextId::from([0u8; 32]), &invitee_id)?
        .ok_or_else(|| eyre!("missing identity for public key: {}", invitee_id))?;

    let identity_secret = stored_identity
        .private_key
        .ok_or_else(|| eyre!("stored identity '{}' is missing private key", invitee_id))?;

    if identity_secret.public_key() != invitee_id {
        eyre::bail!("identity mismatch")
    }

    debug!(
        "✅ Identity validation passed for invitee_id={}",
        invitee_id
    );

    let mut config = None;

    let has_context = context_client.has_context(&context_id)?;
    debug!(
        "🏗️  Context exists check: context_id={}, has_context={}",
        context_id, has_context
    );

    if !has_context {
        debug!(
            "🆕 Creating new context config for context_id={}",
            context_id
        );
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
        debug!("📋 Created external config: protocol={}, network_id={}, contract_id={}, proxy_contract={}", 
               external_config.protocol, external_config.network_id, external_config.contract_id, external_config.proxy_contract);
        config = Some(external_config);
    } else {
        debug!(
            "📋 Using existing context config for context_id={}",
            context_id
        );
    }

    debug!(
        "🔄 Starting context config sync for context_id={}",
        context_id
    );
    let _ignored = context_client
        .sync_context_config(context_id, config)
        .await?;
    debug!(
        "✅ Context config sync completed for context_id={}",
        context_id
    );

    let is_member = context_client.has_member(&context_id, &invitee_id)?;
    debug!(
        "👥 Member check: context_id={}, invitee_id={}, is_member={}",
        context_id, invitee_id, is_member
    );

    if !is_member {
        warn!(
            "❌ Failed to join context: invitee_id={} is not a member of context_id={}",
            invitee_id, context_id
        );
        eyre::bail!("unable to join context: not a member, invalid invitation?")
    }

    debug!("🔐 Generating sender key for invitee_id={}", invitee_id);
    let mut rng = rand::thread_rng();
    let sender_key = PrivateKey::random(&mut rng);

    debug!(
        "💾 Updating identity for context_id={}, invitee_id={}",
        context_id, invitee_id
    );
    context_client.update_identity(
        &context_id,
        &ContextIdentity {
            public_key: invitee_id,
            private_key: Some(identity_secret),
            sender_key: Some(sender_key),
        },
    )?;

    debug!(
        "🗑️  Deleting temporary identity for invitee_id={}",
        invitee_id
    );
    context_client.delete_identity(&ContextId::from([0u8; 32]), &invitee_id)?;

    debug!("📡 Subscribing to context_id={}", context_id);
    node_client.subscribe(&context_id).await?;

    debug!("🔄 Initiating node sync for context_id={}", context_id);
    node_client.sync(Some(&context_id), None).await?;
    debug!("✅ Node sync completed for context_id={}", context_id);

    info!(
        "🎉 Successfully joined context: context_id={}, invitee_id={}",
        context_id, invitee_id
    );
    Ok((context_id, invitee_id))
}
