use std::sync::Arc;
use std::time::Instant;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{
    JoinSubgroupInheritanceError, JoinSubgroupInheritanceRequest, JoinSubgroupInheritanceResponse,
};
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_primitives::identity::PrivateKey;
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

use crate::op_events::{subscribe as subscribe_op_events, OpEvent};
use crate::{group_store, ContextManager};

impl Handler<JoinSubgroupInheritanceRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinSubgroupInheritanceRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinSubgroupInheritanceRequest { group_id }: JoinSubgroupInheritanceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let key_delivery_fallback_wait = self.config.key_delivery_fallback_wait;

        ActorResponse::r#async(
            async move {
                // The group must already be visible to this node — without
                // local meta we can't resolve the namespace or know what we'd
                // be joining. Caller's recovery is to sync the namespace
                // (`POST /namespaces/:id/sync`) first.
                if group_store::load_group_meta(&datastore, &group_id)?.is_none() {
                    return Err(JoinSubgroupInheritanceError::GroupNotFound.into());
                }

                // Resolve the joiner's namespace identity. `None` here means
                // the caller isn't a member of the parent namespace at all —
                // they can't inherit membership into any subgroup.
                let (joiner_identity, sk_bytes) =
                    group_store::resolve_namespace_identity(&datastore, &group_id)?
                        .map(|(pk, sk, _)| (pk, sk))
                        .ok_or(JoinSubgroupInheritanceError::NoNamespaceIdentity)?;

                let membership_path = group_store::check_group_membership_path(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                )?;

                match membership_path {
                    group_store::MembershipPath::Direct => {
                        info!(
                            ?group_id,
                            %joiner_identity,
                            "join_subgroup_inheritance: caller is already a direct member, no-op"
                        );
                        return Ok(JoinSubgroupInheritanceResponse {
                            group_id,
                            member_public_key: joiner_identity,
                            was_inherited: false,
                        });
                    }
                    group_store::MembershipPath::Inherited { anchor, via_admin } => {
                        info!(
                            target: "calimero::audit::group_membership",
                            subgroup_id = %hex::encode(group_id.to_bytes()),
                            anchor_parent = %hex::encode(anchor.to_bytes()),
                            %joiner_identity,
                            via_admin,
                            "self-join via inherited Open-subgroup membership"
                        );
                    }
                    group_store::MembershipPath::None => {
                        return Err(JoinSubgroupInheritanceError::NotEligible.into());
                    }
                }

                // If a prior `join_context` (or earlier call to this endpoint)
                // already materialised the inheritance, the group key is local
                // and there's nothing useful for `MemberJoinedOpen` to trigger
                // — publishing it again would just be a redundant DAG op that
                // no peer needs to act on. Short-circuit to the same response
                // shape so clients can't tell which path ran.
                if group_store::load_current_group_key(&datastore, &group_id)?.is_some() {
                    info!(
                        ?group_id,
                        %joiner_identity,
                        "join_subgroup_inheritance: group key already local, skipping publish"
                    );
                    return Ok(JoinSubgroupInheritanceResponse {
                        group_id,
                        member_public_key: joiner_identity,
                        was_inherited: true,
                    });
                }

                let ns_id = group_store::resolve_namespace(&datastore, &group_id)?;

                // Subscribe BEFORE publishing so we cannot miss a
                // `GroupKeyDelivered` that fires between publish and wait
                // setup — the receiver could race us through the broadcast.
                let mut op_event_rx = subscribe_op_events();

                let signer_sk = PrivateKey::from(sk_bytes);
                let op = NamespaceOp::Root(RootOp::MemberJoinedOpen {
                    member: joiner_identity,
                    group_id: group_id.to_bytes(),
                });
                group_store::sign_apply_and_publish_namespace_op(
                    &datastore,
                    &node_client,
                    &ack_router,
                    ns_id.to_bytes(),
                    &signer_sk,
                    op,
                )
                .await?;

                let deadline = Instant::now() + key_delivery_fallback_wait;
                loop {
                    match group_store::load_current_group_key(&datastore, &group_id) {
                        Ok(Some(_)) => {
                            info!(
                                ?group_id,
                                "group key acquired via gossip KeyDelivery fallback"
                            );
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!(
                                ?group_id,
                                ?e,
                                "transient store error during KeyDelivery wait — retrying"
                            );
                        }
                    }
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(eyre::eyre!(
                            "KeyDelivery timed out for group {group_id:?}: \
                             no group key arrived within {}ms via the gossip fallback path; \
                             join cannot proceed without a usable group key",
                            key_delivery_fallback_wait.as_millis()
                        ));
                    }
                    let remaining = deadline - now;
                    match tokio::time::timeout(remaining, op_event_rx.recv()).await {
                        Ok(Ok(OpEvent::GroupKeyDelivered {
                            group_id: g,
                            recipient,
                        })) if g == group_id.to_bytes() && recipient == joiner_identity => {
                            // `continue` (not `break`) is intentional: the
                            // emitter publishes the event AFTER `store_group_key`
                            // returns, so we re-read the store on the next
                            // iteration where the unwrap is guaranteed to hit
                            // and exit cleanly. Mirrors join_group.rs's loop.
                            continue;
                        }
                        Ok(Ok(_)) => continue,
                        Ok(Err(RecvError::Lagged(_))) => continue,
                        Ok(Err(RecvError::Closed)) => {
                            return Err(eyre::eyre!(
                                "KeyDelivery channel closed before group key arrived for \
                                 {group_id:?}: join cannot proceed without a usable group key"
                            ));
                        }
                        Err(_) => continue,
                    }
                }

                Ok(JoinSubgroupInheritanceResponse {
                    group_id,
                    member_public_key: joiner_identity,
                    was_inherited: true,
                })
            }
            .into_actor(self),
        )
    }
}
