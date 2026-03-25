use std::collections::BTreeSet;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::RemoveGroupMembersRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<RemoveGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <RemoveGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        RemoveGroupMembersRequest {
            group_id,
            members,
            requester,
        }: RemoveGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        // Resolve requester: use provided value or fall back to node group identity
        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        // Resolve signing_key from node identity key
        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        // Sync validation
        if let Err(err) = (|| -> eyre::Result<()> {
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;
            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            let admin_count = group_store::count_group_admins(&self.datastore, &group_id)?;
            let mut unique_admins_being_removed: BTreeSet<PublicKey> = BTreeSet::new();
            for id in &members {
                let role = group_store::get_group_member_role(&self.datastore, &group_id, id)?;
                if role == Some(GroupMemberRole::Admin) {
                    unique_admins_being_removed.insert(*id);
                }
            }

            if admin_count <= unique_admins_being_removed.len() {
                bail!("cannot remove all admins from group '{group_id:?}': at least one admin must remain");
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        // Auto-store signing key for future use
        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
        }

        let self_identity = self.node_group_identity().map(|(pk, _)| pk);
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let context_client = self.context_client.clone();
        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &requester)
                .ok()
                .flatten()
        });
        let members = members.clone();

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!("local group governance requires a signing key for the requester")
                })?);
                for identity in &members {
                    group_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &group_id,
                        &sk,
                        GroupOp::MemberRemoved { member: *identity },
                    )
                    .await?;
                }
                info!(
                    ?group_id,
                    count = members.len(),
                    %requester,
                    "members removed from group (local governance signed ops)"
                );

                // Unsubscribe if this node's identity was removed
                if let Some(self_pk) = self_identity {
                    if members.iter().any(|pk| *pk == self_pk) {
                        let _ = node_client.unsubscribe_group(group_id.to_bytes()).await;
                    }
                }

                let contexts =
                    group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)?;

                for context_id in &contexts {
                    if let Err(err) = context_client.sync_context_config(*context_id, None).await {
                        tracing::warn!(
                            ?group_id,
                            %context_id,
                            ?err,
                            "failed to sync context after group member removal"
                        );
                    }
                }

                Ok(())
            }
            .into_actor(self),
        )
    }
}
