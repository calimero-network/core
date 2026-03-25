use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_context_primitives::messages::{JoinContextRequest, JoinContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::eyre;

use crate::{group_store, ContextManager};

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ResponseFuture<<JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest { invitation_payload }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_client = self.node_client.clone();
        let context_client = self.context_client.clone();
        let datastore = self.datastore.clone();
        let group_signing_identity = self
            .node_group_identity()
            .map(|(pk, sk)| (pk, PrivateKey::from(sk)));

        let task = async move {
            let (context_id, invitee_id) = join_context(
                node_client,
                context_client,
                datastore,
                group_signing_identity,
                invitation_payload,
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
    datastore: Store,
    group_signing_identity: Option<(PublicKey, PrivateKey)>,
    invitation_payload: calimero_primitives::context::ContextInvitationPayload,
) -> eyre::Result<(ContextId, PublicKey)> {
    let (context_id, invitee_id) = invitation_payload.parts()?;

    tracing::info!(%context_id, %invitee_id, "join_context: starting join flow");

    let already_joined = context_client
        .get_identity(&context_id, &invitee_id)?
        .and_then(|i| i.private_key)
        .is_some();

    tracing::info!(%context_id, %invitee_id, already_joined, "join_context: checked if already joined");

    if already_joined {
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
            .unwrap_or(true);

        if needs_sync {
            tracing::info!(%context_id, %invitee_id, "join_context: triggering sync for already-joined context with empty DAG heads");
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
        let external_config = ContextConfigParams {
            application_revision: 0,
            members_revision: 0,
        };

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
            private_key: Some(identity_secret),
            sender_key: Some(sender_key),
        },
    )?;

    context_client.delete_identity(&ContextId::zero(), &invitee_id)?;

    // Publish MemberJoinedContext governance op so other nodes learn about this join.
    // Signed by the node's group admin identity if available.
    if let Ok(Some(group_id)) = group_store::get_group_for_context(&datastore, &context_id) {
        if let Some((_admin_pk, admin_sk)) = group_signing_identity {
            if let Err(e) = group_store::sign_apply_and_publish(
                &datastore,
                &node_client,
                &group_id,
                &admin_sk,
                GroupOp::MemberJoinedContext {
                    member: invitee_id,
                    context_id,
                    context_identity: *invitee_id.as_ref(),
                },
            )
            .await
            {
                tracing::warn!(
                    %context_id,
                    %invitee_id,
                    ?e,
                    "failed to publish MemberJoinedContext governance op during join"
                );
            }
        }
    }

    tracing::info!(%context_id, %invitee_id, "join_context: NEW join - calling subscribe and sync");
    node_client.subscribe(&context_id).await?;

    node_client.sync(Some(&context_id), None).await?;
    tracing::info!(%context_id, %invitee_id, "join_context: sync request sent successfully");

    Ok((context_id, invitee_id))
}
