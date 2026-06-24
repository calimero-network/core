//! Self-leave from a namespace (root group).
//!
//! See `architecture/membership-and-leave.html` § 6 for the design.
//! This handler is a thin wrapper over `leave_group`: it publishes
//! `GroupOp::MemberLeft { member: signer }` at the namespace root.
//! The apply path in `calimero_governance_store` detects "this group has no
//! parent" and cascades through every descendant where the leaver has
//! a direct row — owner + last-admin checks across all of them
//! upfront, then row-removal cascade.
//!
//! No key rotation. Same forward-secrecy caveat as `leave_group`.
//!
//! Local cleanup: the local apply of `MemberLeft` always emits
//! `OpEvent::MemberRemoved` (per descendant group where the leaver had
//! a direct row, plus once at the namespace root). It additionally
//! emits `OpEvent::TeeMemberRemoved` for each of those, gated
//! per-group on whether the leaver's stored role in THAT group was
//! `ReadOnlyTee`. The [`crate::self_purge`] listener only reacts to
//! the latter, so for a regular `Admin`/`Member`/`Observer`
//! self-leave the listener stays dormant and the local rows
//! (namespace identity + signing keys) are preserved as soft-leave
//! residue — leave-then-rejoin-via-inheritance and similar workflows
//! depend on this. For a `ReadOnlyTee` self-leave the listener
//! cascade-purges every group's local rows (signing keys included)
//! and drops namespace-level state. The
//! `node_client.unsubscribe_namespace` call below is still issued
//! synchronously by this handler so the unsubscribe is ordered with
//! the user-visible handler response; the listener also issues an
//! unsubscribe (idempotent) as part of its TEE-eviction cleanup. See
//! ADR 0002 (`docs/adr/0002-fleet-tee-leave-protocol.md`).

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{LeaveNamespaceRequest, LeaveNamespaceResponse};
use tracing::info;

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::observe_handler_delivery;
use calimero_governance_store::{MembershipRepository, MetaRepository, NamespaceRepository};

impl Handler<LeaveNamespaceRequest> for ContextManager {
    type Result = ActorResponse<Self, <LeaveNamespaceRequest as Message>::Result>;

    fn handle(
        &mut self,
        LeaveNamespaceRequest { namespace_id }: LeaveNamespaceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let self_identity = match self.node_namespace_identity(&namespace_id) {
            Some((pk, sk_bytes)) => (pk, sk_bytes),
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node has no namespace identity for {:?}; \
                     not a member, nothing to leave",
                    namespace_id
                )))
            }
        };
        let (member_public_key, signer_sk_bytes) = self_identity;
        let signer_sk = calimero_primitives::identity::PrivateKey::from(signer_sk_bytes);

        // Verify this is actually a namespace (no parent). If a non-root
        // group_id was passed by mistake, route the user to leave_group.
        let resolved = match NamespaceRepository::new(&self.datastore).resolve(&namespace_id) {
            Ok(g) => g,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        if resolved != namespace_id {
            return ActorResponse::reply(Err(eyre::eyre!(
                "{:?} is not a namespace (root group); use leave_group for subgroups",
                namespace_id
            )));
        }

        // Direct-row check at the root. Apply re-validates everything.
        match MembershipRepository::new(&self.datastore).role_of(&namespace_id, &member_public_key)
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node is not a direct member of namespace {:?}",
                    namespace_id
                )))
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        // Sign-time hash precomputation, mirroring `MemberRemoved`. The
        // leaver simulates the post-leave state so receivers can detect
        // divergence. See `compute_group_state_hash_after_remove` and
        // `snapshot_context_state_hashes`.
        let expected_group_state_hash = match MetaRepository::new(&self.datastore)
            .compute_state_hash_after_remove(&namespace_id, &member_public_key)
        {
            Ok(h) => h,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let expected_context_state_hashes = match MetaRepository::new(&self.datastore)
            .snapshot_context_state_hashes(&namespace_id)
        {
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

                // The apply path detects "this group is the namespace" and
                // performs the multi-scope owner / last-admin checks +
                // cascade across descendants. If any check fails, the error
                // surfaces here as the user-facing failure.
                let report = crate::sign_apply_and_publish_group_op(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &namespace_id,
                    &signer_sk,
                    op,
                )
                .await?;

                if let Some(report) = report.as_ref() {
                    observe_handler_delivery("leave_namespace", "MemberLeft", report);
                }

                let _ = node_client
                    .unsubscribe_namespace(namespace_id.to_bytes())
                    .await;

                info!(
                    ?namespace_id,
                    %member_public_key,
                    "left namespace voluntarily — cascade complete; key rotation deferred"
                );

                Ok(LeaveNamespaceResponse {
                    namespace_id,
                    member_public_key,
                })
            }
            .into_actor(self),
        )
    }
}
