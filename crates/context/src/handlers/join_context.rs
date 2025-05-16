use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::join_context::{
    JoinContextRequest, JoinContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{ContextConfigParams, ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};

use crate::ContextManager;

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ResponseFuture<<JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest {
            identity_secret,
            invitation_payload,
        }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_client = self.node_client.clone();
        let context_client = self.context_client.clone();

        let task = async move {
            let (context_id, invitee_id) = join_context(
                node_client,
                context_client,
                invitation_payload,
                identity_secret,
            )
            .await?;

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
    identity_secret: PrivateKey,
) -> eyre::Result<(ContextId, PublicKey)> {
    let (context_id, invitee_id, protocol, network_id, contract_id) = invitation_payload.parts()?;

    if identity_secret.public_key() != invitee_id {
        eyre::bail!("identity mismatch")
    }

    if context_client.has_member(&context_id, &invitee_id)? {
        return Ok((context_id, invitee_id));
    }

    let Some(external_config) = context_client.context_config(&context_id)? else {
        eyre::bail!("context not found");
    };

    let external_client = context_client.external_client(&context_id, &external_config)?;

    let mut config = None;

    if !context_client.has_context(&context_id)? {
        let config_client = external_client.config();

        let proxy_contract = config_client.get_proxy_contract().await?;

        config = Some(ContextConfigParams {
            protocol: protocol.into(),
            network_id: network_id.into(),
            contract_id: contract_id.into(),
            proxy_contract: proxy_contract.into(),
            application_revision: 0,
            members_revision: 0,
        });
    };

    let _ignored = context_client
        .sync_context_config(context_id, config.as_mut())
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
            private_key: Some(identity_secret),
            sender_key: Some(sender_key),
        },
    )?;

    node_client.subscribe(&context_id).await?;

    Ok((context_id, invitee_id))
}
