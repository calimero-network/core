use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{
    JoinSubgroupInheritanceError, JoinSubgroupInheritanceRequest, JoinSubgroupInheritanceResponse,
};
use calimero_context_client::local_governance::{KeyEnvelope, NamespaceOp, RootOp};
use calimero_primitives::identity::PrivateKey;
use tracing::{info, warn};

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

                // Short-circuit if the subgroup key is already local (e.g.
                // a prior `join_context` ran the inheritance materialisation).
                if group_store::load_current_group_key(&datastore, &group_id)?.is_some() {
                    info!(
                        ?group_id,
                        %joiner_identity,
                        "join_subgroup_inheritance: group key already local, skipping fetch"
                    );
                    return Ok(JoinSubgroupInheritanceResponse {
                        group_id,
                        member_public_key: joiner_identity,
                        was_inherited: true,
                    });
                }

                let ns_id = group_store::resolve_namespace(&datastore, &group_id)?;

                // Direct-stream key fetch: ask any peer holding the
                // subgroup key for it via the dedicated
                // `OpenSubgroupJoinRequest` protocol (#2357). Mirrors
                // `request_namespace_join` from #2270 Phase 9.1 — the
                // deterministic primary path for the inherited
                // self-join, instead of the gossip-only
                // `MemberJoinedOpen` → `KeyDelivery` round-trip that
                // can't reliably complete in small clusters where the
                // gossipsub mesh stays empty.
                let envelope_bytes = node_client
                    .request_open_subgroup_join(
                        ns_id.to_bytes(),
                        group_id.to_bytes(),
                        joiner_identity,
                    )
                    .await?;
                let envelope: KeyEnvelope = borsh::from_slice(&envelope_bytes)
                    .map_err(|e| eyre::eyre!("decode KeyEnvelope from peer response: {e}"))?;
                let signer_sk = PrivateKey::from(sk_bytes);
                let group_key = group_store::unwrap_group_key(&signer_sk, &envelope)?;
                let _key_id = group_store::store_group_key(&datastore, &group_id, &group_key)?;

                // Publish `MemberJoinedOpen` so other peers (not just the
                // responder) record our direct membership row and the
                // governance DAG reflects the join. The publish is
                // idempotent on apply (DAG delta-id dedup at
                // `namespace_governance.rs:145`); on the unhappy path
                // where gossip drops the op, this becomes a no-op for
                // peers that don't see it — but our local key is already
                // deterministic via the direct stream above.
                let op = NamespaceOp::Root(RootOp::MemberJoinedOpen {
                    member: joiner_identity,
                    group_id: group_id.to_bytes(),
                });
                if let Err(e) = group_store::sign_apply_and_publish_namespace_op(
                    &datastore,
                    &node_client,
                    &ack_router,
                    ns_id.to_bytes(),
                    &signer_sk,
                    op,
                )
                .await
                {
                    warn!(
                        ?group_id,
                        ?e,
                        "MemberJoinedOpen publish failed after direct-stream key fetch \
                         succeeded — subgroup key is local but other peers may not see \
                         the membership row until the next gossip sweep"
                    );
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
