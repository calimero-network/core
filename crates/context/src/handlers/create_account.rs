//! `CreateAccountRequest` handler — enroll this node's device into a namespace
//! under a fresh account and publish the link.
//!
//! The first thing in the account feature that publishes an account op. Until
//! this exists, the whole account plane is unreachable at runtime: no bindings,
//! so `current_key_recipients` falls back to member addressing for everyone,
//! `live_bindings` is never called, and the apply handlers never fire.
//!
//! **Ordering constraint.** This must run after the node holds the namespace's
//! scope key, because `AccountDeviceLinked` travels as an *encrypted* `GroupOp`.
//! A node with no key cannot publish one — and that is exactly why `KeyEnvelope`
//! still addresses a member as well as a device. Calling this too early does not
//! fail cleanly, it deadlocks the very bootstrap it depends on, so the precondition
//! is checked explicitly below rather than left to fail deep in the publisher.
//!
//! **Why no admin approval.** The account is rooted at this node's offline
//! account root, which is a member nowhere by design, so the link carries a
//! *member endorsement*: the node's granted namespace identity signs the account
//! id, and the gate asks whether that endorser is a member at the op's cut. The
//! link therefore grants nothing the member did not already hold, which is what
//! makes it self-service.
//!
//! **Two keys, one use each.** The account root signs the certificate; the
//! namespace identity signs the endorsement and the op itself. Crossing them is
//! silent — a certificate signed by the wrong one still serializes, and is
//! refused by every peer while the local enrollment looks fine.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::sign_device_cert;
use calimero_context_client::group::{CreateAccountRequest, CreateAccountResponse};
use calimero_governance_store::{GroupKeyring, NodeDeviceRepository};
use calimero_primitives::identity::PrivateKey;
use tracing::info;

use crate::ContextManager;

impl Handler<CreateAccountRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateAccountRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateAccountRequest { namespace_id }: CreateAccountRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // The namespace identity signs the link op and the endorsement that
        // carries it past the gate. It is NOT the account's root key — that is
        // the offline account root, resolved below — and it is also the key
        // recorded as the device's `sign_pk`, because it is what actually signs
        // ops on the governance path.
        let Some((self_pk, signer_sk_bytes)) = self.node_signing_key(&namespace_id) else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node has no namespace identity for {namespace_id:?}; it cannot \
                 enroll a device there"
            )));
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);

        // Refuse before mutating anything if the scope key is absent. The link is
        // an encrypted GroupOp, so publishing without a key is impossible — and
        // failing here, with the reason, beats failing inside the publisher after
        // the device identity has already been minted and stored.
        let store = self.datastore.clone();
        match GroupKeyring::new(&store, namespace_id).holds_any_key() {
            Ok(true) => {}
            Ok(false) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node holds no scope key for {namespace_id:?} yet; a device link \
                     is an encrypted group op, so enrollment must follow key delivery"
                )));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        // Mint (or recover) this node's device identity. Idempotent, so a retried
        // request re-publishes the same link rather than minting a second replica
        // id and stranding the state written under the first.
        let device_repo = NodeDeviceRepository::new(&store);
        let enrolled = match device_repo.ensure_enrolled(&namespace_id) {
            Ok(enrolled) => enrolled,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // The certificate is signed by the ACCOUNT ROOT, not by the key that
        // signs ops. `ensure_enrolled` roots the account at the account root, so
        // that is the key `verify_device_cert` resolves from the genesis and
        // checks against — signing with the namespace identity instead produces a
        // certificate that verifies against a key which never signed it, and the
        // link is refused by every peer while the local enrollment looks fine.
        //
        // The two keys have exactly one use each and crossing them is silent,
        // which is why the invariant has its own test beside the repository.
        let account_root = match device_repo.ensure_account_root() {
            Ok(root) => root,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // The device's op-signing key is the namespace identity, because that is
        // what actually signs ops on the governance path. Recording anything else
        // would make `sign_pk` a claim no signature ever matches, and per-device
        // authorization resolves a signer THROUGH this field.
        let cert = match sign_device_cert(
            account_root.signing_key(),
            enrolled.account,
            enrolled.device(),
            &self_pk,
            &enrolled.kem_public_key(),
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

        let response =
            CreateAccountResponse::new(enrolled.account, enrolled.device(), enrolled.genesis);

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        // The endorsement is what makes the link admissible: the account root is a
        // member nowhere by design, so this node's granted namespace identity
        // vouches for the account instead. Only a member can sign this, and only the
        // root could sign the certificate above — the gate needs both.
        let endorsement =
            match calimero_account::sign_account_endorsement(&signer_sk, enrolled.account) {
                Ok(endorsement) => endorsement,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to sign the account endorsement: {err}"
                    )))
                }
            };

        let op = calimero_context_client::local_governance::GroupOp::AccountDeviceLinked {
            genesis: enrolled.genesis,
            // Epoch 0, so no handoffs to carry. A rotated account's chain is
            // supplied by whoever holds the newer root key.
            chain: vec![],
            cert,
            endorsement,
        };

        ActorResponse::r#async(
            async move {
                let report = calimero_governance_store::sign_apply_and_publish(
                    &store,
                    &node_client,
                    &ack_router,
                    &namespace_id,
                    &signer_sk,
                    op,
                )
                .await?;

                info!(
                    namespace_id = ?namespace_id,
                    account = %response.account,
                    device = %response.device,
                    published = report.is_some(),
                    "enrolled this node's device into a fresh account"
                );

                // A publish that collected no acks is not a failure: the local
                // apply already committed, and sync catches peers up. Reporting it
                // as an error would make a solo node unable to enroll at all.
                Ok(response)
            }
            .into_actor(self),
        )
    }
}
