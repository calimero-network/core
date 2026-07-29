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

        // Mint a proof when this node holds the account root that owns the
        // device's account. That is what makes revocation available to the owner
        // without an admin — and it is also the only authority a non-admin has
        // here, so a node that is neither is refused before anything is signed.
        let device_repo = NodeDeviceRepository::new(&store);
        let (account, proof) = match device_repo.account_root() {
            Ok(Some(root)) => {
                let genesis = root.genesis_for(&namespace_id);
                let account = genesis.account_id();
                match sign_device_revocation(root.signing_key(), account, device, 0) {
                    Ok(revocation) => (
                        account,
                        Some(calimero_account::SignedDeviceRevocation {
                            genesis,
                            // Epoch 0: the account root has not rotated, so there
                            // are no handoffs for a verifier to walk.
                            chain: vec![],
                            revocation,
                        }),
                    ),
                    Err(err) => {
                        return ActorResponse::reply(Err(eyre::eyre!(
                            "failed to sign the revocation proof: {err}"
                        )))
                    }
                }
            }
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node has generated no account root, so it can neither own \
                     the device nor prove a revocation"
                )))
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // The proof only authorises devices of THIS node's account. Revoking
        // somebody else's device is the admin path, and nothing else.
        let owns_the_account = match device_repo.get(&namespace_id) {
            Ok(Some(enrolled)) => enrolled.account == account,
            Ok(None) => false,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if !is_admin && !owns_the_account {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node is neither an admin of {namespace_id:?} nor the holder of the \
                 account that owns {device}; revoking somebody else's device requires admin"
            )));
        }

        let op = GroupOp::AccountDeviceUnlinked {
            account,
            device,
            proof: if owns_the_account { proof } else { None },
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
