//! `RevokeDeviceRequest` handler — withdraw a device and rotate the scope key.
//!
//! Revocation is terminal: the `DeviceId` is spent for good, so re-enrolling the
//! machine mints a fresh one. That permanence is what keeps a replica id from
//! ever being reused, so the CRDT planes hold their one-writer-per-replica
//! invariant across a revoke/re-add cycle.
//!
//! **Two authorities, and this handler can offer both.** A group admin may
//! revoke any device — the path that ejects a device whose account holder is
//! unreachable. The account holder may revoke its own device by attaching a
//! root-signed proof, which is the lost-laptop case where the owner may be the
//! only person who knows. The proof is self-certifying, so it needs no admin and
//! no folded state.
//!
//! **Cutting off authorship is not enough on its own.** A revoked device already
//! holds the current scope key, so without a rotation it stops writing and goes
//! on reading everything the group publishes — a silent reader, which is the
//! failure the whole feature exists to prevent. The rotation therefore rides on
//! the same op.
//!
//! Rotating is admin-only, because peers accept a rotation sidecar only from an
//! admin at the op's cut. A self-service revocation therefore locks the device
//! out of writing immediately and leaves the key rotation owed to an admin. That
//! asymmetry is reported back rather than hidden, because until the rotation
//! lands the device can still read.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::sign_device_revocation;
use calimero_context_client::group::{RevokeDeviceRequest, RevokeDeviceResponse};
use calimero_context_client::local_governance::GroupOp;
use calimero_governance_store::{
    GroupGovernancePublisher, MembershipRepository, NodeDeviceRepository,
};
use calimero_primitives::identity::PrivateKey;
use tracing::{info, warn};

use crate::ContextManager;

impl Handler<RevokeDeviceRequest> for ContextManager {
    type Result = ActorResponse<Self, <RevokeDeviceRequest as Message>::Result>;

    fn handle(
        &mut self,
        RevokeDeviceRequest {
            namespace_id,
            device,
        }: RevokeDeviceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let Some((self_pk, signer_sk_bytes)) = self.node_namespace_identity(&namespace_id) else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node has no namespace identity for {namespace_id:?}; it cannot \
                 revoke a device there"
            )));
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);
        let store = self.datastore.clone();

        let is_admin = match MembershipRepository::new(&store).is_admin(&namespace_id, &self_pk) {
            Ok(is_admin) => is_admin,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Whose device this is, and whether this node can prove it owns the
        // account, both come from the group's own binding. Deriving the account
        // from this node's root instead answers a different question — "which
        // account do I own here" — so an admin ejecting somebody else's device
        // named its own account in the op and reported it back to the operator.
        let device_repo = NodeDeviceRepository::new(&store);
        let target = match device_repo.revocation_target(&namespace_id, device) {
            Ok(Some(target)) => target,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "{namespace_id:?} holds no binding for {device}, so there is no account \
                     to name in the revocation. Either it was never linked here, or its \
                     link has not synced to this node yet"
                )))
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let account = target.account;

        if !is_admin && !target.self_service {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node is neither an admin of {namespace_id:?} nor the holder of the \
                 account that owns {device}; revoking somebody else's device requires admin"
            )));
        }

        // Mint the proof only on the self-service path, which is the only one that
        // needs it — an admin revokes on the group's authority and may hold no
        // account root at all, so consulting one unconditionally refused every
        // admin that had never run `account create`.
        let proof = if target.self_service {
            match device_repo.account_root() {
                Ok(Some(root)) => {
                    let genesis = root.genesis_for(&namespace_id);
                    match sign_device_revocation(root.signing_key(), account, device, 0) {
                        Ok(revocation) => Some(calimero_account::SignedDeviceRevocation {
                            genesis,
                            // Epoch 0: the account root has not rotated, so there
                            // are no handoffs for a verifier to walk.
                            chain: vec![],
                            revocation,
                        }),
                        Err(err) => {
                            return ActorResponse::reply(Err(eyre::eyre!(
                                "failed to sign the revocation proof: {err}"
                            )))
                        }
                    }
                }
                // Unreachable: `self_service` is true only because the root
                // re-derived this account. Refusing beats signing nothing silently.
                Ok(None) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "the account root that owns {account} vanished between resolving \
                         the revocation and signing its proof"
                    )))
                }
                Err(err) => return ActorResponse::reply(Err(err)),
            }
        } else {
            None
        };

        let op = GroupOp::AccountDeviceUnlinked {
            account,
            device,
            proof,
        };

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                // An admin publishes the revocation with the key rotation riding
                // on the same op. A non-admin owner cannot: peers accept a
                // rotation only from an admin at the cut, and minting one anyway
                // would leave this node holding a key nobody else adopts.
                let (report, key_rotated) = if is_admin {
                    let report = GroupGovernancePublisher::new(&store, &node_client, namespace_id)
                        .sign_apply_and_publish_device_revocation(&ack_router, &signer_sk, op)
                        .await?;
                    (report, true)
                } else {
                    warn!(
                        namespace_id = ?namespace_id,
                        %device,
                        "revoking without a key rotation: this node is not an admin, so the \
                         device loses the right to write immediately but keeps the key it \
                         already holds until an admin rotates"
                    );
                    let report = calimero_governance_store::sign_apply_and_publish(
                        &store,
                        &node_client,
                        &ack_router,
                        &namespace_id,
                        &signer_sk,
                        op,
                    )
                    .await?;
                    (report, false)
                };

                info!(
                    namespace_id = ?namespace_id,
                    %account,
                    %device,
                    published = report.is_some(),
                    key_rotated,
                    "device revoked"
                );

                Ok(RevokeDeviceResponse::new(account, device, key_rotated))
            }
            .into_actor(self),
        )
    }
}
