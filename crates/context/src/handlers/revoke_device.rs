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
use calimero_context_client::group::{
    RevocationOutcome, RevokeDeviceRequest, RevokeDeviceResponse,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_governance_store::{
    GroupGovernancePublisher, MembershipRepository, NamespaceRepository, NodeDeviceRepository,
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
            proof: supplied_proof,
        }: RevokeDeviceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let Some((self_pk, signer_sk_bytes)) = self.node_signing_key(&namespace_id) else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node has no namespace identity for {namespace_id:?}; it cannot \
                 revoke a device there"
            )));
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);
        let store = self.datastore.clone();

        let self_account = match crate::member_account::require(&store, &namespace_id, &self_pk) {
            Ok(account) => account,
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

        // A proof minted elsewhere is verified HERE, before anything is published,
        // against the account the group's own binding names. Refusing beats
        // publishing: the apply path treats an unverifiable proof as a deterministic
        // refusal that records nothing and returns `Ok`, so a bad one would leave the
        // operator with a successful-looking call, no revocation on any replica, and
        // nothing anywhere saying why.
        //
        // Verifying against `target.account` rather than the account inside the proof
        // is what stops a proof for one account authorising a device bound to
        // another — the same tie the apply path enforces, checked early so the error
        // reaches whoever can act on it.
        let supplied_proof = match supplied_proof {
            Some(proof) => match proof.authorises(account, device) {
                Ok(()) => Some(proof),
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "the supplied revocation proof does not authorise withdrawing \
                         {device} from {account}: {err}. A proof is only valid for the \
                         one account and device it names, and {namespace_id:?} has this \
                         device bound to {account}"
                    )))
                }
            },
            None => None,
        };

        // Two ways to be authorized, and both are the ACCOUNT's. Revoking a device
        // is not a group-administration act: an admin governs who is a member, not
        // which installations another person runs. An admin who wants somebody out
        // removes their account, which is strictly stronger and already exists.
        //
        // `is_admin` still matters below, but as a capability rather than an
        // authorization — only an admin may rotate the scope key that rides along.
        if supplied_proof.is_none() && !target.self_service {
            return ActorResponse::reply(Err(eyre::eyre!(
                "this node does not hold the account that owns {device}, and no \
                 revocation proof was supplied. Revoking a device is the account's \
                 authority, not an admin's: run this from a node of that account, or \
                 mint a proof from its root (`merod account revoke-proof`) and pass it \
                 here. To remove the person rather than one of their devices, remove \
                 the account from the group"
            )));
        }

        // A supplied proof is used as given — re-minting would need the root, which
        // is the thing this path exists to do without.
        //
        // Otherwise mint one only on the self-service path, which is the only one
        // that needs it: an admin revokes on the group's authority and may hold no
        // account root at all, so consulting one unconditionally refused every
        // admin that had enrolled nowhere itself.
        let proof = if let Some(proof) = supplied_proof {
            Some(proof)
        } else if target.self_service {
            match device_repo.account_root() {
                Ok(Some(root)) => {
                    let genesis = root.genesis();
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
                // A device belongs to an account, not to a scope, so the revocation
                // goes everywhere the account holds this device — not only the
                // namespace the caller named. The proof is minted once above and
                // reused: it names `{account, device}` and nothing about it is
                // namespace-scoped, so the same bytes verify wherever they land.
                //
                // Publication stays per-DAG. Wider validity is not wider reach: the
                // op takes effect in a namespace when it is published there, which
                // is what this loop does, one namespace at a time.
                let namespaces = NamespaceRepository::new(&store).participating_namespaces()?;
                let mut revoked_in = Vec::new();

                for ns in namespaces {
                    // A namespace that never linked this device has nothing to
                    // withdraw, and publishing there would name an account its
                    // bindings do not agree with.
                    match NodeDeviceRepository::new(&store).revocation_target(&ns, device) {
                        Ok(Some(_)) => {}
                        Ok(None) => continue,
                        Err(err) => {
                            warn!(namespace_id = ?ns, %device, %err,
                                  "revocation: could not read the binding; skipping this namespace");
                            continue;
                        }
                    }

                    // Admin HERE, not where the caller started. The rotation is a
                    // group act — peers accept one only from an admin at the cut —
                    // so it rides along in the namespaces this node governs and is
                    // left owed in the rest.
                    let is_admin_here = MembershipRepository::new(&store)
                        .is_admin(&ns, &self_account)
                        .unwrap_or(false);

                    let published = if is_admin_here {
                        GroupGovernancePublisher::new(&store, &node_client, ns)
                            .sign_apply_and_publish_device_revocation(
                                &ack_router,
                                &signer_sk,
                                op.clone(),
                            )
                            .await
                    } else {
                        calimero_governance_store::sign_apply_and_publish(
                            &store,
                            &node_client,
                            &ack_router,
                            &ns,
                            &signer_sk,
                            op.clone(),
                        )
                        .await
                    };

                    match published {
                        Ok(report) => {
                            if !is_admin_here {
                                warn!(
                                    namespace_id = ?ns,
                                    %device,
                                    "revoked without a key rotation: this node is not an \
                                     admin here, so the device loses the right to write \
                                     immediately but keeps the key it already holds until \
                                     an admin rotates"
                                );
                            }
                            info!(
                                namespace_id = ?ns,
                                %account,
                                %device,
                                published = report.is_some(),
                                key_rotated = is_admin_here,
                                "device revoked"
                            );
                            revoked_in.push(RevocationOutcome::new(ns, is_admin_here));
                        }
                        // One namespace failing must not withhold the revocation
                        // from the rest — a device half-withdrawn is worse than one
                        // withdrawn everywhere it could be. The caller sees which
                        // namespaces landed.
                        Err(err) => warn!(
                            namespace_id = ?ns, %device, %err,
                            "revocation: publishing failed for this namespace; others continue"
                        ),
                    }
                }

                if revoked_in.is_empty() {
                    eyre::bail!(
                        "the revocation of {device} reached no namespace. Nothing was \
                         published, so the device is still linked wherever it was"
                    );
                }

                Ok(RevokeDeviceResponse::new(account, device, revoked_in))
            }
            .into_actor(self),
        )
    }
}
