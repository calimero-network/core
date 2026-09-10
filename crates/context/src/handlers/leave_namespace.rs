//! Self-leave from a namespace (root group).
//!
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
//! unsubscribe (idempotent) as part of its TEE-eviction cleanup.

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
        let self_identity = match self.node_signing_key(&namespace_id) {
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
        // The leaver signs with its identity key; the row it is leaving names
        // the account that key acts as.
        let member_account = match crate::member_account::require(
            &self.datastore,
            &namespace_id,
            &member_public_key,
        ) {
            Ok(account) => account,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        match MembershipRepository::new(&self.datastore).role_of(&namespace_id, &member_account) {
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
            .compute_state_hash_after_remove(&namespace_id, &member_account)
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
                    member: member_account,
                    expected_group_state_hash,
                    expected_context_state_hashes,
                };

                // The apply path detects "this group is the namespace" and
                // performs the multi-scope owner / last-admin checks +
                // cascade across descendants. If any check fails, the error
                // surfaces here as the user-facing failure.
                let report = calimero_governance_store::sign_apply_and_publish(
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

                // The account left, so every other device of it has to stop
                // following the namespace too.
                let _recorded = crate::account_namespace::announce(
                    &datastore,
                    &node_client,
                    &ack_router,
                    namespace_id,
                    crate::account_namespace::AccountNamespaceChange::Left,
                )
                .await;

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_client::group::{CreateGroupRequest, EnsureAccountNamespaceRequest};
    use calimero_governance_store::{
        AccountNamespaceSet, MembershipRepository, MetaRepository, NodeDeviceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::*;
    use crate::test_support::actor;

    const GROUP: [u8; 32] = [0xF1; 32];

    /// Leaving is the ACCOUNT leaving, so a device that goes on following the
    /// namespace is following one it may no longer read.
    ///
    /// The namespace is created app-less and then handed to a second admin: the
    /// owner and the last admin are both refused a self-leave, and neither
    /// refusal is what this test is about.
    #[actix::test]
    async fn leaving_a_namespace_drops_it_from_the_account_namespace() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the holder's root");

        let harness = actor::over(store.clone()).await;
        let account_namespace = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("the ensure runs")
            .expect("the holder creates its account namespace");
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: None,
                name: None,
                parent_group_id: None,
                restricted: true,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");
        assert!(
            AccountNamespaceSet::new(&store, account_namespace)
                .contains(created.group_id)
                .expect("read the set")
                .is_some(),
            "the creation put it in the set, which is what the leave has to undo"
        );

        let other =
            crate::test_support::enrol(&store, &created.group_id, &PublicKey::from([0xF2; 32]));
        MembershipRepository::new(&store)
            .add_member(&created.group_id, &other, GroupMemberRole::Admin)
            .expect("a second admin");
        let meta = MetaRepository::new(&store);
        let mut row = meta
            .load(&created.group_id)
            .expect("read the meta")
            .expect("the namespace has one");
        row.owner_identity = other;
        meta.save(&created.group_id, &row).expect("hand it over");

        let _left = harness
            .manager
            .send(LeaveNamespaceRequest {
                namespace_id: created.group_id,
            })
            .await
            .expect("the manager answers")
            .expect("the leave runs");

        assert_eq!(
            AccountNamespaceSet::new(&store, account_namespace)
                .contains(created.group_id)
                .expect("read the set"),
            None,
            "a namespace the account left is no longer one of its own"
        );
    }
}
