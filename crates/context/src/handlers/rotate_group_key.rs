//! `RotateGroupKeyRequest` handler — pay off a forward-secrecy rotation a self-leave
//! left behind.
//!
//! The rotation for a member's departure is minted by whoever publishes the op that
//! triggers it. That works for an admin-initiated removal, where the publisher stays
//! in the group. It cannot work for a self-leave: the publisher IS the leaver, who
//! would have to mint the very key they are being cut off from — and would keep it.
//! Peers reject a rotation from a non-admin regardless.
//!
//! So `MemberLeft` records the debt as a replicated pending row, and a REMAINING
//! ADMIN discharges it here. This handler is what the rotation listener calls; the
//! eligibility checks live here (not only in the listener) so the invariant holds
//! however the request arrives.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::RotateGroupKeyRequest;
use calimero_governance_store::{PendingRotationRepository, PermissionChecker};
use tracing::{debug, info};

use crate::ContextManager;

impl Handler<RotateGroupKeyRequest> for ContextManager {
    type Result = ActorResponse<Self, <RotateGroupKeyRequest as Message>::Result>;

    fn handle(
        &mut self,
        RotateGroupKeyRequest { group_id, departed }: RotateGroupKeyRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Who are we in this namespace? This is the identity that will SIGN the
        // rotation, and therefore the identity peers check against the
        // authorized-rotator gate — so it is the identity whose admin-ness matters,
        // not the requester's.
        let Some((self_pk, signer_sk_bytes)) = self.node_namespace_identity(&group_id) else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node has no namespace identity for the namespace owning {group_id:?}; \
                 it cannot rotate that group's key"
            )));
        };
        let signer_sk = calimero_primitives::identity::PrivateKey::from(signer_sk_bytes);

        // Never rotate for our OWN departure. If this node is the leaver, it must not
        // mint the key it is being cut off from — that is precisely the forward-secrecy
        // hole this whole mechanism exists to close. (The membership row is already
        // gone, so the admin check below would normally catch it; this is explicit
        // because getting it wrong silently defeats the feature.)
        if departed == self_pk {
            return ActorResponse::reply(Err(eyre::eyre!(
                "refusing to rotate {group_id:?} for this node's own departure: a leaver \
                 cannot mint the key they are being cut off from"
            )));
        }

        // Nothing owed? Then some other admin already rotated and their
        // `GroupKeyRotated` cleared the row. Not an error — this is the expected
        // outcome of a race, and re-rotating would only put redundant envelopes on the
        // wire.
        match PendingRotationRepository::new(&self.datastore).is_pending(&group_id, &departed) {
            Ok(true) => {}
            Ok(false) => {
                debug!(
                    ?group_id,
                    %departed,
                    "no pending key rotation; another admin already discharged it"
                );
                return ActorResponse::reply(Ok(()));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        // Only an admin's rotation is accepted by peers. Check before publishing so a
        // non-admin node fails fast and locally rather than minting a key the network
        // will throw away.
        match PermissionChecker::new(&self.datastore, group_id).is_admin(&self_pk) {
            Ok(true) => {}
            Ok(false) => {
                debug!(
                    ?group_id,
                    %departed,
                    "this node is not an admin of the group; leaving the rotation to one that is"
                );
                return ActorResponse::reply(Ok(()));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                // Mints a fresh key, stamps it with the DAG sequence this op will
                // occupy, wraps it for every remaining member and for nobody who left,
                // and ships it as a sidecar on `GroupKeyRotated`. Applying that op
                // clears the pending row on every node.
                let report = calimero_governance_store::sign_apply_and_publish_rotation(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &signer_sk,
                    &departed,
                )
                .await?;

                info!(
                    ?group_id,
                    %departed,
                    acked = report.as_ref().map(|r| r.acked_by.len()).unwrap_or(0),
                    "rotated group key after member departure"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}
