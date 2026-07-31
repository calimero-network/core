//! `PairDeviceCompleteRequest` handler — certify a device another node minted,
//! link it, and hand it the scope key.
//!
//! The second half of pairing, run on the device that already holds the account.
//! It is two published ops, and both are load-bearing:
//!
//! 1. **`AccountDeviceLinked`** — an encrypted `GroupOp` carrying the device's
//!    certificate, signed by the account root, plus a member endorsement. This
//!    is what confers authority.
//! 2. **`RootOp::KeyDelivery`** — the current scope key, wrapped to the device's
//!    agreement key. Without it the link lands and the new device still cannot
//!    read anything, which is the failure this half exists to prevent.
//!
//! **Delivery has to be a cleartext `RootOp`, and that is not incidental.** The
//! pairing device holds no scope key, so a device-addressed envelope carried
//! inside an *encrypted* `GroupOp` would be unreadable by its only recipient —
//! the same bootstrap deadlock that keeps the member-addressed envelope alive.
//! `KeyDelivery` being a root op is what breaks the cycle.
//!
//! **Only the current key is delivered.** Peers retain rotated-out keys, so
//! history *could* be handed back, but doing so would make every newly paired
//! device a full-history reader — a capability decision that deserves its own
//! change rather than riding in on pairing. The cost is that the paired device
//! converges on forward state and cannot decrypt ops sealed under retired
//! epochs.
//!
//! **Two keys, one use each.** The account root signs the certificate; the
//! namespace identity signs the endorsement, the ops, and the key wrap. Crossing
//! them is silent — see the certificate invariant test beside
//! `NodeDeviceRepository`.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::{
    pairing_confirmation_code, sign_account_endorsement, sign_device_cert, verify_pairing_statement,
};
use calimero_context_client::group::{PairDeviceCompleteRequest, PairDeviceCompleteResponse};
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp};
use calimero_crypto::X25519PublicKey;
use calimero_governance_store::{GroupKeyring, NodeDeviceRepository};
use calimero_primitives::identity::PrivateKey;
use tracing::{info, warn};

use crate::ContextManager;

impl Handler<PairDeviceCompleteRequest> for ContextManager {
    type Result = ActorResponse<Self, <PairDeviceCompleteRequest as Message>::Result>;

    fn handle(
        &mut self,
        PairDeviceCompleteRequest {
            namespace_id,
            device,
            kem_pk,
            sign_pk,
            statement,
        }: PairDeviceCompleteRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // The namespace identity signs the endorsement, both ops, and the key
        // wrap. It must be a granted member: the endorsement is what carries the
        // link past the apply gate, and an endorsement from a non-member is
        // refused.
        let Some((self_pk, signer_sk_bytes)) = self.node_namespace_identity(&namespace_id) else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node has no namespace identity for {namespace_id:?}; it cannot \
                 certify a device there"
            )));
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);

        let store = self.datastore.clone();
        let device_repo = NodeDeviceRepository::new(&store);

        // The account root is what certifies the device, and it is also what
        // decides *which* account this node can pair into: the genesis is
        // derived from the root secret and the namespace id, so this node can
        // only ever certify devices for the account it owns here.
        let account_root = match device_repo.ensure_account_root() {
            Ok(root) => root,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let genesis = account_root.genesis_for(&namespace_id);
        let account = genesis.account_id();

        // Check the key material before anything is signed over it. The
        // certificate minted below is what makes these keys a trusted device of
        // this account, and until this point they are three values a caller
        // supplied: an attacker who can alter the pairing payload substitutes its
        // own keys under a captured `DeviceId` and receives the scope-key
        // fan-out. The statement is the pairing device's own signature over
        // exactly what is being certified, so it can only be produced by
        // whoever holds the signing key it names.
        //
        // It does not cover a substitution that replaces both keys and re-signs
        // — nothing here has a prior commitment to the genuine ones, and binding
        // them into the `DeviceId` is ruled out because the id must survive key
        // rotation. The confirmation code returned below is what closes that,
        // out of band and by a person.
        if let Err(err) = verify_pairing_statement(account, device, &kem_pk, &sign_pk, &statement) {
            return ActorResponse::reply(Err(eyre::eyre!(
                "refusing to certify device {device}: {err}. The key material does not \
                 come with a valid signature from the device that minted it — re-run \
                 `account pair-init` and carry its statement across unaltered"
            )));
        }

        // Derived from the same values the statement covers, and reported so the
        // operator can compare it with what `pair-init` printed on the other
        // machine. Computed before the publishes so a failure there still leaves
        // the operator able to tell whether they certified the right device.
        let confirmation_code = pairing_confirmation_code(account, device, &kem_pk, &sign_pk);

        // Refuse if this node is not itself a device of that account. A node
        // that paired INTO somebody else's account holds a device whose account
        // its own root cannot name, so it has no standing to certify a second
        // one — it would mint a certificate for an account it does not hold and
        // the link would be refused downstream.
        match device_repo.get(&namespace_id) {
            Ok(Some(enrolled)) if enrolled.account == account => {}
            Ok(Some(enrolled)) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node's device in {namespace_id:?} belongs to account {}, not to {account} \
                     which its own root owns; a paired device cannot certify further devices — run \
                     this on the node that holds the account",
                    enrolled.account,
                )));
            }
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node has enrolled no device in {namespace_id:?}; enroll one with \
                     `account create` before pairing a second"
                )));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        // One precondition covers both ops: the link is an encrypted group op so
        // publishing it needs the current key, and the delivery is that same key
        // wrapped for the new device. Checking it here, before anything is
        // signed, beats failing deep inside the publisher.
        let group_key = match GroupKeyring::new(&store, namespace_id).load_current_key() {
            Ok(Some((_key_id, group_key))) => group_key,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node holds no current scope key for {namespace_id:?}; pairing both \
                     publishes an encrypted group op and delivers that key, so neither is \
                     possible yet"
                )));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Epoch 0 on both counts: the account root has not rotated (rotation is
        // not implemented yet), so there are no handoffs to carry and the
        // certifying key is the genesis key itself.
        let cert = match sign_device_cert(
            account_root.signing_key(),
            account,
            device,
            &sign_pk,
            &kem_pk,
            0,
            0,
        ) {
            Ok(cert) => cert,
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to sign the device certificate: {err}"
                )))
            }
        };

        // Only a member can endorse and only the root can certify; the gate needs
        // both. `self_pk` is the endorser and is inside the signed payload.
        let endorsement = match sign_account_endorsement(&signer_sk, account) {
            Ok(endorsement) => endorsement,
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to sign the account endorsement: {err}"
                )))
            }
        };
        // A hard check, not a `debug_assert`. The endorsement is what makes this
        // link self-service, and the two keys in play here (account root vs
        // namespace identity) are trivially crossed — that mistake produces a link
        // every peer refuses while looking perfectly healthy locally. Compiled out
        // in release, this would publish the mismatch instead of refusing it.
        if endorsement.member != self_pk {
            return ActorResponse::reply(Err(eyre::eyre!(
                "endorsement names {} but this node signs as {self_pk}; refusing to \
                 publish a link no peer can admit",
                endorsement.member,
            )));
        }

        let link = GroupOp::AccountDeviceLinked {
            genesis,
            chain: vec![],
            cert,
            endorsement,
        };

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                // Publish the link first. If it fails nothing else should happen
                // — handing the scope key to a device the group has not been told
                // about would give it read access with no recorded authority.
                let report = calimero_governance_store::sign_apply_and_publish(
                    &store,
                    &node_client,
                    &ack_router,
                    &namespace_id,
                    &signer_sk,
                    link,
                )
                .await?;

                info!(
                    namespace_id = ?namespace_id,
                    %account,
                    %device,
                    published = report.is_some(),
                    "linked a paired device"
                );

                // Wrap under the KEM key we were handed rather than re-reading it
                // from the folded binding, so the delivery does not depend on this
                // node having already folded the link it just published.
                let envelope = GroupKeyring::wrap_for_device(
                    &signer_sk,
                    device,
                    &X25519PublicKey::from(*kem_pk.as_bytes()),
                    &namespace_id.to_bytes(),
                    &group_key,
                )?;

                // A cleartext root op, so the keyless recipient can actually read
                // it. `required_signers` is None because the paired device is not
                // a member and so is not among the acking set — its receipt shows
                // up as the device being able to read, not as an ack.
                let delivery = NamespaceOp::Root(RootOp::KeyDelivery {
                    group_id: namespace_id.to_bytes().into(),
                    envelope,
                });

                let key_delivered = match calimero_governance_store::sign_and_publish_namespace_op(
                    &store,
                    &node_client,
                    &ack_router,
                    namespace_id.to_bytes().into(),
                    &signer_sk,
                    delivery,
                    None,
                )
                .await
                {
                    Ok(_) => true,
                    Err(err) => {
                        // Not fatal: the link already conferred authority, and the
                        // device's own sync pull re-requests the key it lacks. Say
                        // so rather than reporting a flat success, because until
                        // that pull lands the device cannot read.
                        warn!(
                            ?err,
                            namespace_id = ?namespace_id,
                            %device,
                            "device linked but the scope-key delivery failed to publish; \
                             the device's sync pull is the durable retry"
                        );
                        false
                    }
                };

                Ok(PairDeviceCompleteResponse::new(
                    account,
                    device,
                    key_delivered,
                    confirmation_code,
                ))
            }
            .into_actor(self),
        )
    }
}
