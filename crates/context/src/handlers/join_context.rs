use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{JoinContextRequest, JoinContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types, Store};
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

        let task = async move {
            let (context_id, invitee_id) =
                join_context(node_client, context_client, datastore, invitation_payload).await?;

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
    invitation_payload: calimero_primitives::context::ContextInvitationPayload,
) -> eyre::Result<(ContextId, PublicKey)> {
    let (
        context_id,
        invitee_id,
        invitation_app_id,
        _inviter_id,
        invitation_group_id,
        invitation_blob_id,
    ) = invitation_payload.parts()?;

    tracing::info!(%context_id, %invitee_id, %invitation_app_id, "join_context: starting join flow");

    let already_joined = context_client
        .get_identity(&context_id, &invitee_id)?
        .and_then(|i| i.private_key)
        .is_some();

    tracing::info!(%context_id, %invitee_id, already_joined, "join_context: checked if already joined");

    if already_joined {
        let context = context_client.get_context(&context_id)?;
        let needs_sync = context.map(|ctx| ctx.dag_heads.is_empty()).unwrap_or(true);

        if needs_sync {
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

    // Bootstrap context metadata if the context doesn't exist locally.
    let mut config = None;
    if !context_client.has_context(&context_id)? {
        let zero_app_id = calimero_primitives::application::ApplicationId::from([0u8; 32]);
        let app_id = if invitation_app_id != zero_app_id {
            Some(invitation_app_id)
        } else {
            group_store::get_group_for_context(&datastore, &context_id)?
                .and_then(|gid| group_store::load_group_meta(&datastore, &gid).ok()?)
                .map(|meta| meta.target_application_id)
        };

        config = Some(ContextConfigParams {
            application_id: app_id,
            application_revision: 0,
            members_revision: 0,
        });
    };

    let _ignored = context_client
        .sync_context_config(context_id, config)
        .await?;

    // Write ContextIdentity so the sync key-share can find keys for this context.
    let mut rng = rand::thread_rng();
    let sender_key = PrivateKey::random(&mut rng);

    {
        let mut handle = datastore.handle();
        handle.put(
            &key::ContextIdentity::new(context_id, invitee_id),
            &types::ContextIdentity {
                private_key: Some(*identity_secret),
                sender_key: Some(*sender_key),
            },
        )?;
    }

    context_client.delete_identity(&ContextId::zero(), &invitee_id)?;

    // Stub ApplicationMeta so the sync manager can get the blob_id for blob sharing.
    let zero_blob = calimero_primitives::blobs::BlobId::from([0u8; 32]);
    if invitation_blob_id != zero_blob && !node_client.has_application(&invitation_app_id)? {
        let mut handle = datastore.handle();
        handle.put(
            &key::ApplicationMeta::new(invitation_app_id),
            &calimero_store::types::ApplicationMeta::new(
                key::BlobMeta::new(invitation_blob_id),
                0,
                format!("calimero://context/{context_id}").into_boxed_str(),
                Box::default(),
                key::BlobMeta::new(calimero_primitives::blobs::BlobId::from([0u8; 32])),
                "unknown".to_owned().into_boxed_str(),
                "0.0.0".to_owned().into_boxed_str(),
                String::new().into_boxed_str(),
            ),
        )?;
        tracing::info!(
            %context_id,
            %invitation_app_id,
            %invitation_blob_id,
            "wrote stub ApplicationMeta for blob sharing"
        );
    }

    // Write group membership and context-group mapping so has_member()
    // can recognise both the inviter and invitee via the GroupMember fallback.
    let zero_group = [0u8; 32];
    if invitation_group_id != zero_group {
        let gid = calimero_context_config::types::ContextGroupId::from(invitation_group_id);

        // Ensure the context→group mapping exists on this node.
        group_store::register_context_in_group(&datastore, &gid, &context_id)?;

        // Write the invitee as a GroupMember with keys.
        if !group_store::check_group_membership(&datastore, &gid, &invitee_id)? {
            group_store::add_group_member_with_keys(
                &datastore,
                &gid,
                &invitee_id,
                calimero_primitives::context::GroupMemberRole::Member,
                Some(*identity_secret),
                Some(*sender_key),
            )?;
        }

        // Write the inviter as a GroupMember (no keys) so this node
        // recognises the inviter for sync.
        let zero_pk = calimero_primitives::identity::PublicKey::from([0u8; 32]);
        if _inviter_id != zero_pk
            && !group_store::check_group_membership(&datastore, &gid, &_inviter_id)?
        {
            group_store::add_group_member(
                &datastore,
                &gid,
                &_inviter_id,
                calimero_primitives::context::GroupMemberRole::Admin,
            )?;
        }
    }

    tracing::info!(%context_id, %invitee_id, "join_context: subscribing and syncing");
    node_client.subscribe(&context_id).await?;
    node_client.sync(Some(&context_id), None).await?;

    Ok((context_id, invitee_id))
}
