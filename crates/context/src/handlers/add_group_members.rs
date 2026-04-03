use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::AddGroupMembersRequest;
use calimero_context_primitives::local_governance::{GroupOp, NamespaceOp, RootOp};
use tracing::{info, warn};

use crate::group_store;
use crate::ContextManager;

impl Handler<AddGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <AddGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        AddGroupMembersRequest {
            group_id,
            members,
            requester,
        }: AddGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let sk = preflight.signer_sk();
        let requester = preflight.requester;
        let members = members.clone();

        ActorResponse::r#async(
            async move {
                for (identity, role) in &members {
                    group_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &group_id,
                        &sk,
                        GroupOp::MemberAdded {
                            member: *identity,
                            role: role.clone(),
                        },
                    )
                    .await?;

                    if let Some((_key_id, group_key)) =
                        group_store::load_current_group_key(&datastore, &group_id)?
                    {
                        let ns_id = group_store::resolve_namespace(&datastore, &group_id)?;
                        match group_store::wrap_group_key_for_member(&sk, identity, &group_key) {
                            Ok(envelope) => {
                                let delivery_op = NamespaceOp::Root(RootOp::KeyDelivery {
                                    group_id: group_id.to_bytes(),
                                    envelope,
                                });
                                if let Err(e) = group_store::sign_and_publish_namespace_op(
                                    &datastore,
                                    &node_client,
                                    ns_id.to_bytes(),
                                    &sk,
                                    delivery_op,
                                )
                                .await
                                {
                                    warn!(?e, %identity, "failed to publish KeyDelivery for added member");
                                }
                            }
                            Err(e) => {
                                warn!(?e, %identity, "failed to wrap group key for added member");
                            }
                        }
                    }
                }
                info!(
                    ?group_id,
                    count = members.len(),
                    %requester,
                    "members added to group (local governance signed ops)"
                );
                Ok(())
            }
            .into_actor(self),
        )
    }
}
