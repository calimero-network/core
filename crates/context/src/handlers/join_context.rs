use actix::{Handler, Message, ResponseFuture};
use calimero_context_config::repr::ReprBytes;
use calimero_context_config::types::SignedOpenInvitation;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{JoinContextRequest, JoinContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types, Store};
use eyre::eyre;

use crate::{group_store, ContextManager};

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ResponseFuture<<JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest {
            invitation,
            new_member_public_key,
        }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_client = self.node_client.clone();
        let context_client = self.context_client.clone();
        let datastore = self.datastore.clone();

        let task = async move {
            let (context_id, invitee_id) = join_context(
                node_client,
                context_client,
                datastore,
                invitation,
                new_member_public_key,
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
    signed_invitation: SignedOpenInvitation,
    new_member_public_key: PublicKey,
) -> eyre::Result<(ContextId, PublicKey)> {
    let invitation = &signed_invitation.invitation;
    let context_id: ContextId = invitation.context_id.to_bytes().into();
    let inviter_id: PublicKey = {
        let bytes: [u8; DIGEST_SIZE] = invitation.inviter_identity.as_bytes();
        bytes.into()
    };
    let invitee_id = new_member_public_key;

    let app = context_client
        .get_context_application(&context_id)
        .await
        .ok();
    let invitation_app_id = app
        .as_ref()
        .map(|a| a.id)
        .or_else(|| {
            signed_invitation
                .application_id
                .map(calimero_primitives::application::ApplicationId::from)
        })
        .unwrap_or_else(|| {
            calimero_primitives::application::ApplicationId::from([0u8; DIGEST_SIZE])
        });
    let invitation_blob_id = app
        .as_ref()
        .map(|a| a.blob.bytecode)
        .or_else(|| {
            signed_invitation
                .blob_id
                .map(calimero_primitives::blobs::BlobId::from)
        })
        .unwrap_or_else(|| calimero_primitives::blobs::BlobId::from([0u8; DIGEST_SIZE]));

    let invitation_group_id = {
        let handle = datastore.handle();
        let ref_key = key::ContextGroupRef::new(context_id);
        handle.get(&ref_key)?.unwrap_or([0u8; DIGEST_SIZE])
    };

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

    let zero_blob = calimero_primitives::blobs::BlobId::from([0u8; 32]);
    if invitation_blob_id != zero_blob && !node_client.has_application(&invitation_app_id)? {
        let default_source = format!("calimero://context/{context_id}");
        let source = signed_invitation
            .source
            .as_deref()
            .unwrap_or(&default_source);

        let mut handle = datastore.handle();
        handle.put(
            &key::ApplicationMeta::new(invitation_app_id),
            &calimero_store::types::ApplicationMeta::new(
                key::BlobMeta::new(invitation_blob_id),
                0,
                source.to_owned().into_boxed_str(),
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
            %source,
            "wrote stub ApplicationMeta for blob sharing"
        );
    }

    let zero_group = [0u8; 32];
    if invitation_group_id != zero_group {
        let gid = calimero_context_config::types::ContextGroupId::from(invitation_group_id);

        group_store::register_context_in_group(&datastore, &gid, &context_id)?;

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

        let zero_pk = calimero_primitives::identity::PublicKey::from([0u8; 32]);
        if inviter_id != zero_pk
            && !group_store::check_group_membership(&datastore, &gid, &inviter_id)?
        {
            group_store::add_group_member(
                &datastore,
                &gid,
                &inviter_id,
                calimero_primitives::context::GroupMemberRole::Admin,
            )?;
        }
    }

    tracing::info!(%context_id, %invitee_id, "join_context: subscribing and syncing");
    node_client.subscribe(&context_id).await?;
    node_client.sync(Some(&context_id), None).await?;

    Ok((context_id, invitee_id))
}
