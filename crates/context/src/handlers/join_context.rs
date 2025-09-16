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

    if context_client
        .get_identity(&context_id, &invitee_id)?
        .and_then(|i| i.keypair_ref)
        .is_some()
    {
        return Ok((context_id, invitee_id));
    }

    let stored_identity = context_client
        .get_identity(&ContextId::from([0u8; 32]), &invitee_id)?
        .ok_or_else(|| eyre!("missing identity for public key: {}", invitee_id))?;

    let identity_secret = stored_identity
        .private_key(&context_client)?
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

    let _ignored = context_client
        .sync_context_config(context_id, config)
        .await?;

    if !context_client.has_member(&context_id, &invitee_id)? {
        eyre::bail!("unable to join context: not a member, invalid invitation?")
    }

    let mut rng = rand::thread_rng();

    let sender_key = PrivateKey::random(&mut rng);

    context_client.update_identity(
        &context_id,
        &ContextIdentity {
            public_key: invitee_id,
            keypair_ref: Some((*identity_secret.public_key()).into()),
            sender_key: Some(sender_key),
        },
    )?;

    context_client.delete_identity(&ContextId::from([0u8; 32]), &invitee_id)?;

    node_client.subscribe(&context_id).await?;

    node_client.sync(Some(&context_id), None).await?;

    Ok((context_id, invitee_id))
}
