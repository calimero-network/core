use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{JoinGroupContextRequest, JoinGroupContextResponse};
use calimero_primitives::context::ContextConfigParams;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinGroupContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupContextRequest {
            group_id,
            context_id,
        }: JoinGroupContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Resolve joiner identity from node group identity.
        let (joiner_identity, _effective_signing_key) = match self.node_group_identity() {
            Some((pk, sk)) => (pk, Some(sk)),
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "joiner_identity not provided and node has no configured group identity"
                )));
            }
        };

        // Validate: group exists, joiner is a member, and has permission to join this context.
        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group not found");
            }
            if !group_store::check_group_membership(&self.datastore, &group_id, &joiner_identity)? {
                bail!("identity is not a member of the group");
            }
            match group_store::get_context_visibility(&self.datastore, &group_id, &context_id)? {
                Some((0, _)) => {
                    if !group_store::is_group_admin_or_has_capability(
                        &self.datastore,
                        &group_id,
                        &joiner_identity,
                        calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_CONTEXTS,
                    )? {
                        bail!(
                            "identity lacks permission to join open context '{context_id:?}' \
                             (not an admin and CAN_JOIN_OPEN_CONTEXTS is not set)"
                        );
                    }
                }
                Some((1, _)) => {
                    let is_admin =
                        group_store::is_group_admin(&self.datastore, &group_id, &joiner_identity)?;
                    let on_allowlist = group_store::check_context_allowlist(
                        &self.datastore,
                        &group_id,
                        &context_id,
                        &joiner_identity,
                    )?;
                    if !is_admin && !on_allowlist {
                        bail!(
                            "identity is not permitted to join restricted context '{context_id:?}' \
                             (not an admin and not on the context allowlist)"
                        );
                    }
                }
                Some((mode, _)) => bail!("unknown context visibility mode: {mode}"),
                None => {
                    if !group_store::is_group_admin(&self.datastore, &group_id, &joiner_identity)? {
                        bail!(
                            "context visibility not found for '{context_id:?}'; \
                             only admins may join"
                        );
                    }
                }
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        let effective_signing_key =
            group_store::get_group_signing_key(&self.datastore, &group_id, &joiner_identity)
                .ok()
                .flatten()
                .or_else(|| self.node_group_identity().map(|(_, sk_bytes)| sk_bytes));

        ActorResponse::r#async(
            async move {
                // Use the group member's existing keys (reused across all contexts).
                let group_member_val =
                    group_store::get_group_member_value(&datastore, &group_id, &joiner_identity)?
                        .ok_or_else(|| eyre::eyre!("group member value not found"))?;

                let config = if !context_client.has_context(&context_id)? {
                    let app_id = group_store::load_group_meta(&datastore, &group_id)?
                        .map(|meta| meta.target_application_id);

                    Some(ContextConfigParams {
                        application_id: app_id,
                        application_revision: 0,
                        members_revision: 0,
                    })
                } else {
                    None
                };

                let _ignored = context_client
                    .sync_context_config(context_id, config)
                    .await?;

                // Write ContextIdentity from group member keys so the sync
                // key-share can find identity + keys for this context.
                {
                    let mut handle = datastore.handle();
                    handle.put(
                        &calimero_store::key::ContextIdentity::new(context_id, joiner_identity),
                        &calimero_store::types::ContextIdentity {
                            private_key: group_member_val.private_key,
                            sender_key: group_member_val.sender_key,
                        },
                    )?;
                }

                node_client.subscribe(&context_id).await?;
                node_client.sync(Some(&context_id), None).await?;

                info!(
                    ?group_id,
                    ?context_id,
                    %joiner_identity,
                    "joined context via group membership"
                );

                Ok(JoinGroupContextResponse {
                    context_id,
                    member_public_key: joiner_identity,
                })
            }
            .into_actor(self),
        )
    }
}
