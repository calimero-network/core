//! Handlers for the owner-level ops that need the account root's proof:
//! transferring a group, repointing a namespace's admin, and the owner-only group
//! deletion. Each builds the bare op, wraps it in its `RootGuarded` form with a
//! proof (see [`crate::root_guard`]), then signs, applies and publishes it like
//! any other governance op.
//!
//! The local apply runs before the publish, so a refusal (not the owner, a
//! target who is not an admin, a stale counter) reaches the caller and nothing
//! is published.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{
    ChangeNamespaceAdminRequest, OwnerDeleteGroupRequest, TransferOwnershipRequest,
};
use calimero_context_client::local_governance::{GroupOp, RootOp};
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::NamespaceRepository;
use calimero_primitives::identity::PrivateKey;
use tracing::info;

use crate::ContextManager;

impl ContextManager {
    /// Wrap `op` for `group_id` with its root proof, then sign, apply and
    /// publish it. `label` names the op in metrics and logs.
    fn publish_guarded_group_op(
        &mut self,
        group_id: calimero_context_config::types::ContextGroupId,
        op: GroupOp,
        root_proof: Option<calimero_account::SignedOwnerOp>,
        label: &'static str,
    ) -> ActorResponse<Self, eyre::Result<()>> {
        // No admin precheck: ownership is the apply's to decide, and the local
        // apply below runs before anything is published.
        let preflight = match self.governance_preflight(&group_id, false) {
            Ok(preflight) => preflight,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let op = match crate::root_guard::guard_group_op(
            &preflight.datastore,
            &group_id,
            &preflight.signer,
            op,
            root_proof,
        ) {
            Ok(op) => op,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let sk = preflight.signing_key;
        let datastore = preflight.datastore;
        let node_client = preflight.node_client;
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    op,
                )
                .await?;
                report.observe(label, "RootGuarded");
                info!(?group_id, op = label, "published a root-guarded owner op");
                Ok(())
            }
            .into_actor(self),
        )
    }
}

impl Handler<TransferOwnershipRequest> for ContextManager {
    type Result = ActorResponse<Self, <TransferOwnershipRequest as Message>::Result>;

    fn handle(
        &mut self,
        TransferOwnershipRequest {
            group_id,
            new_owner,
            root_proof,
        }: TransferOwnershipRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.publish_guarded_group_op(
            group_id,
            GroupOp::TransferOwnership { new_owner },
            root_proof,
            "transfer_ownership",
        )
    }
}

impl Handler<OwnerDeleteGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <OwnerDeleteGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        OwnerDeleteGroupRequest {
            group_id,
            root_proof,
        }: OwnerDeleteGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.publish_guarded_group_op(
            group_id,
            GroupOp::GroupDelete,
            root_proof,
            "owner_delete_group",
        )
    }
}

impl Handler<ChangeNamespaceAdminRequest> for ContextManager {
    type Result = ActorResponse<Self, <ChangeNamespaceAdminRequest as Message>::Result>;

    fn handle(
        &mut self,
        ChangeNamespaceAdminRequest {
            namespace_id,
            new_admin,
            root_proof,
        }: ChangeNamespaceAdminRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();
        let prepared = (|| -> eyre::Result<(RootOp, [u8; 32])> {
            let resolved = NamespaceRepository::new(&store).resolve(&namespace_id)?;
            if resolved != namespace_id {
                return Err(crate::error::ContextError::NamespaceNotFound {
                    namespace_id: namespace_id.to_string(),
                }
                .into());
            }
            let Some((signer, sk)) =
                NamespaceRepository::new(&store).resolve_identity(&namespace_id)?
            else {
                eyre::bail!(
                    "no local namespace identity for '{namespace_id:?}': cannot sign the admin change"
                );
            };
            let op = crate::root_guard::guard_root_op(
                &store,
                &namespace_id,
                &signer,
                RootOp::AdminChanged { new_admin },
                root_proof,
            )?;
            Ok((op, sk))
        })();
        let (op, sk_bytes) = match prepared {
            Ok(prepared) => prepared,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                let signer_sk = PrivateKey::from(sk_bytes);
                // Sealed: a root op whose publisher holds the namespace key, like
                // every admin-published root op (`root_op_is_sealable`).
                let op = calimero_governance_store::seal_root_op_for_publish(
                    &store,
                    namespace_id.to_bytes().into(),
                    op,
                )?;
                let report = calimero_governance_store::sign_apply_and_publish_namespace_op(
                    &store,
                    &node_client,
                    &ack_router,
                    namespace_id.to_bytes().into(),
                    &signer_sk,
                    op,
                )
                .await?;
                report.observe("change_namespace_admin", "RootGuarded");
                info!(?namespace_id, %new_admin, "namespace admin changed");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
