//! Self-leave from a single group.
//!
//! See `architecture/membership-and-leave.html` § 5 for the full design.
//! This handler publishes `GroupOp::MemberLeft { member: signer }` via
//! the existing local-governance pipeline. All apply-side validation
//! (direct-row, owner, last-admin) lives in
//! `calimero_governance_store::apply_group_op_mutations` so peers reject the
//! op identically to the publisher.
//!
//! No key rotation is attached. See the apply-arm comment for the
//! forward-secrecy rationale (briefly: the leaver cannot generate the
//! new key without retaining it; proper two-phase rotation is a
//! deferred follow-up).
//!
//! Local cleanup: the local apply of `MemberLeft` emits
//! `OpEvent::MemberRemoved`, which the [`crate::self_purge`] listener
//! reacts to by dropping the subgroup's local rows (signing keys
//! included). The listener deliberately does NOT unsubscribe from the
//! namespace gossipsub topic for a subgroup-only leave — other
//! memberships under the same namespace still need it. See ADR 0002
//! (`docs/adr/0002-fleet-tee-leave-protocol.md`).

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{LeaveGroupRequest, LeaveGroupResponse};
use tracing::info;

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::observe_handler_delivery;
use calimero_governance_store::{MembershipRepository, MetaRepository, NamespaceRepository};

impl Handler<LeaveGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <LeaveGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        LeaveGroupRequest { group_id }: LeaveGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Resolve this node's identity for the group's namespace. If the
        // node has no namespace identity, it can't be a member of any
        // subgroup under it — nothing to leave.
        // Reject if the caller passed a namespace (root group) here.
        // Although the apply path's `MemberLeft` arm correctly cascades
        // for root-group leaves, this handler does NOT call
        // `unsubscribe_namespace` (it can't, because the leaver may still
        // be in other groups under the same namespace). For a namespace
        // leave, the proper handler is `leave_namespace`, which both
        // applies the cascade AND unsubscribes from gossipsub.
        match NamespaceRepository::new(&self.datastore).resolve(&group_id) {
            Ok(ns) if ns == group_id => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "{:?} is a namespace (root group); use leave_namespace, \
                     which also unsubscribes from the namespace gossipsub topic",
                    group_id
                )))
            }
            Ok(_) => {}
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        let self_identity = match self.node_namespace_identity(&group_id) {
            Some((pk, sk_bytes)) => (pk, sk_bytes),
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node has no namespace identity for the namespace owning {:?}; \
                     not a member, nothing to leave",
                    group_id
                )))
            }
        };
        let (member_public_key, signer_sk_bytes) = self_identity;
        let signer_sk = calimero_primitives::identity::PrivateKey::from(signer_sk_bytes);

        // Pre-flight direct-row check so we surface a friendly error
        // instead of going through publish-then-apply-fail. Apply will
        // re-validate this anyway on every receiver.
        match MembershipRepository::new(&self.datastore).role_of(&group_id, &member_public_key) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node is not a direct member of {:?}; \
                     leave the parent group where the membership anchor lives instead",
                    group_id
                )))
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        // Sign-time hash precomputation, mirroring `MemberRemoved`. The
        // leaver simulates the post-leave state so receivers can detect
        // divergence.
        let expected_group_state_hash = match MetaRepository::new(&self.datastore)
            .compute_state_hash_after_remove(&group_id, &member_public_key)
        {
            Ok(h) => h,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let expected_context_state_hashes =
            match MetaRepository::new(&self.datastore).snapshot_context_state_hashes(&group_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        ActorResponse::r#async(
            async move {
                let op = calimero_context_client::local_governance::GroupOp::MemberLeft {
                    member: member_public_key,
                    expected_group_state_hash,
                    expected_context_state_hashes,
                };

                // sign_apply_and_publish runs the apply path locally
                // (which enforces owner / last-admin / direct-row checks
                // again) and broadcasts to peers. Errors at apply bubble
                // up here as the user-facing failure.
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &signer_sk,
                    op,
                )
                .await?;

                if let Some(report) = report.as_ref() {
                    observe_handler_delivery("leave_group", "MemberLeft", report);
                }

                // Note: we deliberately do NOT call
                // `node_client.unsubscribe_namespace` here. Namespace
                // subscription tracks namespace-level governance traffic
                // and a member who leaves a single subgroup can still be
                // a member of other groups under the same namespace —
                // unsubscribing prematurely would cut them off from
                // governance ops they still need. The namespace topic is
                // only safe to unsubscribe in `leave_namespace`, where
                // the member is leaving the whole subtree.

                info!(
                    ?group_id,
                    %member_public_key,
                    "left group voluntarily (MemberLeft published, peers will see the leave; \
                     no key rotation attached — admin follow-up required for forward secrecy)"
                );

                Ok(LeaveGroupResponse {
                    group_id,
                    member_public_key,
                })
            }
            .into_actor(self),
        )
    }
}
