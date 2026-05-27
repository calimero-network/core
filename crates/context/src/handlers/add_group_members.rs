use calimero_governance_store::{GroupKeyring, NamespaceRepository};
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::AddGroupMembersRequest;
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp};
use tracing::{info, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

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
        let ack_router = Arc::clone(&self.ack_router);
        let sk = preflight.signer_sk();
        let requester = preflight.requester;
        let members = members.clone();

        ActorResponse::r#async(
            async move {
                for (identity, role) in &members {
                    let report = calimero_governance_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &ack_router,
                        &group_id,
                        &sk,
                        GroupOp::MemberAdded {
                            member: *identity,
                            role: role.clone(),
                        },
                    )
                    .await?;
                    report.observe("add_group_members", "MemberAdded");

                    if let Some((_key_id, group_key)) =
                        GroupKeyring::new(&datastore, group_id).load_current_key()?
                    {
                        let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;
                        match GroupKeyring::wrap_for_member(&sk, identity, &group_key) {
                            Ok(envelope) => {
                                let delivery_op = NamespaceOp::Root(RootOp::KeyDelivery {
                                    group_id: group_id.to_bytes(),
                                    envelope,
                                });
                                // KeyDelivery is recipient-specific: the
                                // only ack that proves successful delivery
                                // is from the added member themselves.
                                // Pass `required_signers = Some([identity])`
                                // so non-recipient acks are filtered out
                                // and the report's `acked_by` cleanly
                                // signals whether the recipient applied
                                // and acked.
                                if let Err(e) = calimero_governance_store::sign_and_publish_namespace_op(
                                    &datastore,
                                    &node_client,
                                    &ack_router,
                                    ns_id.to_bytes(),
                                    &sk,
                                    delivery_op,
                                    Some(vec![*identity]),
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
