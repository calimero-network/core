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
//! **The scope is chosen by application, not by namespace.** A namespace is an
//! implementation unit nobody outside this crate named, so asking a caller to
//! pick one left the rest of the outcome undecided; an application list settles
//! which namespaces receive the binding and which scope keys are delivered, and
//! it is a question the person doing the pairing can actually answer. Naming no
//! application means all of them, which is what the fan-out did unconditionally
//! before.
//!
//! **Two checks before anything is signed.** The pairing device's statement
//! proves the key material came from the device that minted it, and the
//! confirmation code the caller must supply proves the account holder is
//! certifying the keys they were actually read. The first is a signature over
//! the payload, so an attacker holding both keys can always make it agree with
//! itself; the second is the value it cannot produce, because it arrives from the
//! other device by a channel it does not control. Neither is a substitute for the
//! other, and if the code travels beside the keys it describes it proves nothing
//! — that part is the operator's channel, not this handler's.
//!
//! **Two keys, one use each.** The account root signs the certificate; the
//! namespace identity signs the endorsement, the ops, and the key wrap. Crossing
//! them is silent — see the certificate invariant test beside
//! `NodeDeviceRepository`.

use std::collections::BTreeSet;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::{AccountMemberEndorsement, DeviceCert, PairingOffer};
use calimero_context_client::group::{
    PairDeviceCompleteRequest, PairDeviceCompleteResponse, PairingScope,
};
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::X25519PublicKey;
use calimero_governance_store::{GroupKeyring, NamespaceRepository, NodeDeviceRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{info, warn};

use crate::handlers::list_namespaces::namespace_rows_for_applications;
use crate::ContextManager;

/// Where the link is published: everywhere this node takes part, narrowed to the
/// namespaces targeting one of `applications`.
///
/// Participation is the base set rather than the application resolution, because
/// publishing needs this node's identity and scope key — a namespace it merely
/// knows the metadata of is one it cannot author in.
///
/// An empty list is every namespace, which is what a caller who names no
/// application asks for and what the fan-out did unconditionally before.
fn namespaces_in_scope(
    store: &Store,
    applications: &[ApplicationId],
) -> EyreResult<Vec<ContextGroupId>> {
    let participating = NamespaceRepository::new(store).participating_namespaces()?;
    if applications.is_empty() {
        return Ok(participating);
    }

    let scoped: BTreeSet<[u8; 32]> = namespace_rows_for_applications(store, applications)?
        .into_iter()
        .map(|(group_id, _meta)| group_id)
        .collect();

    Ok(participating
        .into_iter()
        .filter(|namespace| scoped.contains(&namespace.to_bytes()))
        .collect())
}

impl Handler<PairDeviceCompleteRequest> for ContextManager {
    type Result = ActorResponse<Self, <PairDeviceCompleteRequest as Message>::Result>;

    fn handle(
        &mut self,
        PairDeviceCompleteRequest {
            scope,
            device,
            kem_pk,
            sign_pk,
            statement,
            confirmation_code,
        }: PairDeviceCompleteRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();

        // Resolved before any check, because the scope is what the checks are
        // about: which namespaces have to hold an identity and a key for this
        // pairing to be able to do anything.
        let resolved = (|| -> EyreResult<(Vec<ContextGroupId>, Vec<ContextGroupId>)> {
            Ok(match &scope {
                // The namespace-scoped route checks the namespace it was handed
                // and still publishes into every one. Left as it was: narrowing
                // its fan-out would silently change what an existing caller gets,
                // and the route is on its way out.
                PairingScope::Namespace(namespace_id) => {
                    (vec![*namespace_id], namespaces_in_scope(&store, &[])?)
                }
                // No namespace to name, so the resolved set answers for both.
                PairingScope::Applications(applications) => {
                    let targets = namespaces_in_scope(&store, applications)?;
                    (targets.clone(), targets)
                }
            })
        })();
        let (gated_on, targets) = match resolved {
            Ok(resolved) => resolved,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // The namespace identity signs the endorsement, both ops, and the key
        // wrap. It must be a granted member: the endorsement is what carries the
        // link past the apply gate, and an endorsement from a non-member is
        // refused.
        //
        // One key serves every namespace — it is node-level — so the first that
        // resolves is the same key as any other. What the scan actually decides is
        // whether this node takes part in the scope at all: an empty answer means
        // either it is a stranger to the namespaces named, or the applications
        // resolved to nothing here.
        let Some((self_pk, signer_sk_bytes)) =
            gated_on.iter().find_map(|ns| self.node_signing_key(ns))
        else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node takes part in none of the namespaces this pairing covers \
                 ({gated_on:?}); it has no identity to sign with and cannot certify a \
                 device there"
            )));
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);

        let device_repo = NodeDeviceRepository::new(&store);

        // The account root is what certifies the device, and it is also what
        // decides *which* account this node can pair into: the genesis is the
        // content address of this node's root key, so it can only ever certify
        // devices for the one account that root owns.
        let account_root = match device_repo.require_account_root() {
            Ok(root) => root,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let genesis = account_root.genesis();
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
        let offer = PairingOffer::new(account, device, kem_pk, sign_pk);
        if let Err(err) = offer.verify_statement(&statement) {
            // Logged, not just returned: this is the security-relevant event the
            // check exists for, and the error otherwise reaches only whoever made
            // the request — possibly the attacker rather than an operator reading
            // logs. Ids only; no key material.
            warn!(
                namespaces = gated_on.len(),
                %account,
                %device,
                %err,
                "refusing to certify device: pairing statement invalid"
            );
            return ActorResponse::reply(Err(eyre::eyre!(
                "refusing to certify device {device}: {err}. The key material does not \
                 come with a valid signature from the device that minted it — re-run \
                 `account pair-init` and carry its statement across unaltered"
            )));
        }

        // The statement proves the keys and the signature agree with each other,
        // which an attacker holding both can arrange. The code is the value it
        // cannot produce: the account holder was read it from the pairing
        // device's own output, so it describes the keys that device minted, and
        // here it is checked against the keys that actually arrived.
        if !offer.code_matches(&confirmation_code) {
            // Deliberately does not echo the expected code: an attacker that can
            // drive this endpoint would otherwise learn the value it needs.
            warn!(
                namespaces = gated_on.len(),
                %account,
                %device,
                "refusing to certify device: confirmation code does not match the \
                 key material offered"
            );
            return ActorResponse::reply(Err(eyre::eyre!(
                "refusing to certify device {device}: the confirmation code does not \
                 match the key material in this request. Either it was mistyped, or \
                 the payload was altered between `account pair-init` and here — in \
                 which case do not retry with the code this side computes, get it \
                 from the pairing device again"
            )));
        }

        // Refuse if this node is not itself a device of that account. A node
        // that paired INTO somebody else's account holds a device whose account
        // its own root cannot name, so it has no standing to certify a second
        // one — it would mint a certificate for an account it does not hold and
        // the link would be refused downstream.
        match device_repo.get() {
            Ok(Some(enrolled)) if enrolled.account == account => {}
            Ok(Some(enrolled)) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node's device belongs to account {}, not to {account} which its \
                     own root owns; a paired device cannot certify further devices — run \
                     this on the node that holds the account",
                    enrolled.account,
                )));
            }
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node has enrolled no device; joining a namespace enrols one, \
                     and it has to happen before pairing a second"
                )));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        // One precondition covers both ops: the link is an encrypted group op so
        // publishing it needs the current key, and the delivery is that same key
        // wrapped for the new device. Checking it here, before anything is
        // signed, beats failing deep inside the publisher.
        //
        // Only the namespaces this pairing is gated on are a precondition, and one
        // of them holding a key is enough. The fan-out below reaches the others
        // too, but a missing key there is a reason to skip that namespace rather
        // than to refuse the pairing the caller asked for.
        let mut holds_a_key = false;
        for namespace_id in &gated_on {
            match GroupKeyring::new(&store, *namespace_id).load_current_key() {
                Ok(Some(_)) => {
                    holds_a_key = true;
                    break;
                }
                Ok(None) => {}
                Err(err) => return ActorResponse::reply(Err(err)),
            }
        }
        if !holds_a_key {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node holds no current scope key in any of {gated_on:?}; pairing \
                 both publishes an encrypted group op and delivers that key, so neither \
                 is possible yet"
            )));
        }

        // Epoch 0 on both counts: the account root has not rotated (rotation is
        // not implemented yet), so there are no handoffs to carry and the
        // certifying key is the genesis key itself.
        let cert = match DeviceCert::sign(
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
        let endorsement = match AccountMemberEndorsement::sign(&signer_sk, account) {
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

        // Kept before the op takes ownership: the device this certifies needs the
        // proof to present itself, and cannot read it off the DAG until it is a
        // member somewhere — which for a thin client is never.
        let credential = Box::new(calimero_account::AccountProof {
            genesis,
            chain: vec![],
            statement: cert,
        });

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
                // A device belongs to an account, not to a scope, so both
                // credentials the link carries are account-scoped: the certificate
                // is signed by the account root, and the endorsement by this node's
                // signing key, which is node-level. Neither says anything about a
                // namespace.
                //
                // What does vary per namespace is the scope key, so each one gets
                // its own delivery wrapped under the same device KEM key.
                //
                // The set is the caller's applications resolved, or every namespace
                // this node takes part in when they named none. It does not have to
                // agree with what the device subscribed to at pair-init: a binding
                // published somewhere the device is not listening still lands on
                // that namespace's DAG and is picked up whenever it does subscribe,
                // and a subscription this never reaches costs the device nothing.
                // Only prompt delivery depends on the two overlapping.
                let mut linked_in = Vec::new();
                let mut key_delivered_everywhere = true;

                for ns in targets {
                    // No key here means this node cannot publish an encrypted group
                    // op for the namespace, let alone deliver one. Skip rather than
                    // fail: the scope the caller asked about is checked above, and
                    // the others are a bonus this pairing is extending.
                    let Ok(Some((_key_id, ns_key))) =
                        GroupKeyring::new(&store, ns).load_current_key()
                    else {
                        continue;
                    };

                    let published = calimero_governance_store::sign_apply_and_publish(
                        &store,
                        &node_client,
                        &ack_router,
                        &ns,
                        &signer_sk,
                        link.clone(),
                    )
                    .await;

                    match published {
                        Ok(report) => info!(
                            namespace_id = ?ns,
                            %account,
                            %device,
                            published = report.is_some(),
                            "linked a paired device"
                        ),
                        // One namespace failing must not withhold the device from
                        // the rest. The caller sees which ones landed.
                        Err(err) => {
                            warn!(namespace_id = ?ns, %device, %err,
                                  "pairing: publishing the link failed here; others continue");
                            continue;
                        }
                    }

                    // Wrap under the KEM key we were handed rather than re-reading
                    // it from the folded binding, so the delivery does not depend
                    // on this node having already folded the link it just
                    // published.
                    let envelope = GroupKeyring::wrap_for_device(
                        &signer_sk,
                        device,
                        &X25519PublicKey::from(*kem_pk.as_bytes()),
                        &ns.to_bytes(),
                        &ns_key,
                    )?;

                    // A cleartext root op, so the keyless recipient can actually
                    // read it. `required_signers` is None because the paired device
                    // is not a member and so is not among the acking set — its
                    // receipt shows up as the device being able to read, not as an
                    // ack.
                    let delivery = NamespaceOp::Root(RootOp::KeyDelivery {
                        group_id: ns.to_bytes().into(),
                        envelope,
                    });

                    if let Err(err) = calimero_governance_store::sign_and_publish_namespace_op(
                        &store,
                        &node_client,
                        &ack_router,
                        ns.to_bytes().into(),
                        &signer_sk,
                        delivery,
                        None,
                    )
                    .await
                    {
                        // Not fatal: the link already conferred authority, and the
                        // device's own sync pull re-requests the key it lacks. Say
                        // so rather than reporting a flat success, because until
                        // that pull lands the device cannot read there.
                        warn!(
                            ?err,
                            namespace_id = ?ns,
                            %device,
                            "device linked but the scope-key delivery failed to publish; \
                             the device's sync pull is the durable retry"
                        );
                        key_delivered_everywhere = false;
                    }

                    linked_in.push(ns);
                }

                if linked_in.is_empty() {
                    eyre::bail!(
                        "the link for {device} reached no namespace, so it is paired \
                         nowhere and holds no scope key"
                    );
                }

                let key_delivered = key_delivered_everywhere;

                Ok(PairDeviceCompleteResponse::new(
                    account,
                    device,
                    key_delivered,
                    confirmation_code,
                    credential,
                ))
            }
            .into_actor(self),
        )
    }
}
