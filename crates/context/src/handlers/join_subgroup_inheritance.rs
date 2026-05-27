use crate::group_store::{GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository};
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
                if MetaRepository::new(&datastore).load(&group_id)?.is_none() {
                    return Err(JoinSubgroupInheritanceError::GroupNotFound.into());
                }

                // Resolve the joiner's namespace identity. `None` here means
                // the caller isn't a member of the parent namespace at all —
                // they can't inherit membership into any subgroup.
                let (joiner_identity, sk_bytes) = NamespaceRepository::new(&datastore)
                    .resolve_identity(&group_id)?
                    .map(|(pk, sk, _)| (pk, sk))
                    .ok_or(JoinSubgroupInheritanceError::NoNamespaceIdentity)?;

                let membership_path = MembershipRepository::new(&datastore)
                    .check_path(&group_id, &joiner_identity)?;

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

                let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;
                let signer_sk = PrivateKey::from(sk_bytes);

                // Two side-effects must succeed for the endpoint to honour
                // its deterministic contract: (1) local subgroup key
                // populated, (2) `MemberJoinedOpen` op on the namespace
                // DAG (so peers learn about the join and trigger their own
                // KeyDelivery flows for any subgroup-encrypted ops the
                // joiner authors next).
                //
                // Each side-effect is short-circuited independently — both
                // operations are idempotent (key-store is a put under a
                // stable key; namespace-op apply dedups on DAG delta-id at
                // `namespace_governance.rs:145`). A prior call that landed
                // (1) but failed (2) — e.g. transient publish error — must
                // be safe to retry without re-fetching the key, and must
                // still re-attempt the publish.
                let key_already_local = GroupKeyring::new(&datastore, group_id)
                    .load_current_key()?
                    .is_some();
                if !key_already_local {
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
                    let group_key = GroupKeyring::unwrap_for_recipient(&signer_sk, &envelope)?;
                    let _key_id = GroupKeyring::new(&datastore, group_id).store_key(&group_key)?;
                } else {
                    info!(
                        ?group_id,
                        %joiner_identity,
                        "join_subgroup_inheritance: group key already local, skipping fetch"
                    );
                }

                // Always publish `MemberJoinedOpen` — even when the key
                // was already local — so a prior call that succeeded in
                // fetching the key but failed mid-publish (publish-failure
                // race in unhappy-path retries) gets the membership op
                // onto the namespace DAG on the next attempt. Apply-side
                // dedups on DAG delta-id, so re-attempts on the happy
                // path are free.
                //
                // Hard-fail on publish error: the contract is "after this
                // returns success, both the key is local AND the
                // membership op is on the DAG." Silently swallowing a
                // publish failure would leave the joiner in a split state
                // where they can author group ops but peers can't tell
                // them apart from any other inherited member — caller
                // wouldn't know to retry.
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
                .await
                .map_err(|e| {
                    warn!(
                        ?group_id,
                        ?e,
                        "MemberJoinedOpen publish failed; subgroup key is local but \
                         peers may not see the membership op — caller should retry"
                    );
                    e
                })?;

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
