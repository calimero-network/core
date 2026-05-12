//! Self-leave from a namespace (root group).
//!
//! See `architecture/membership-and-leave.html` § 6 for the design.
//! This handler is a thin wrapper over `leave_group`: it publishes
//! `GroupOp::MemberLeft { member: signer }` at the namespace root.
//! The apply path in `crate::group_store` detects "this group has no
//! parent" and cascades through every descendant where the leaver has
//! a direct row — owner + last-admin checks across all of them
//! upfront, then row-removal cascade.
//!
//! No key rotation. Same forward-secrecy caveat as `leave_group`.
//! See architecture doc § 6 for the soft-vs-hard local-cleanup choice
//! (left as a follow-up — current behavior is "soft": no purge,
//! membership rows removed but encrypted blobs and keys remain on the
//! local node).

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{LeaveNamespaceRequest, LeaveNamespaceResponse};
use tracing::info;

use crate::governance_broadcast::observe_handler_delivery;
use crate::group_store;
use crate::ContextManager;

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
        let resolved = match group_store::resolve_namespace(&self.datastore, &namespace_id) {
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
        match group_store::get_group_member_role(&self.datastore, &namespace_id, &member_public_key)
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
        let cut = match group_store::build_governance_cut(&self.datastore, &namespace_id) {
            Ok(c) => c,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let expected_group_state_hash = match group_store::compute_group_state_hash_after_remove(
            &self.datastore,
            &namespace_id,
            &member_public_key,
        ) {
            Ok(h) => h,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let expected_context_state_hashes =
            match group_store::snapshot_context_state_hashes(&self.datastore, &namespace_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        ActorResponse::r#async(
            async move {
                let op = calimero_context_client::local_governance::GroupOp::MemberLeft {
                    member: member_public_key,
                    cut,
                    expected_group_state_hash,
                    expected_context_state_hashes,
                };

                // The apply path detects "this group is the namespace" and
                // performs the multi-scope owner / last-admin checks +
                // cascade across descendants. If any check fails, the error
                // surfaces here as the user-facing failure.
                let report = group_store::sign_apply_and_publish(
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
