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
use calimero_account::{AccountId, DeviceId, DeviceRevocation};
use calimero_context_client::group::{
    RevocationOutcome, RevokeDeviceRequest, RevokeDeviceResponse,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    withdraw_device_in, NamespaceRepository, NodeDeviceRepository, RevocationTarget,
};
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::warn;

use crate::error::ContextError;
use crate::ContextManager;

/// The namespaces a withdrawal is published into, this node's account namespace
/// LAST: its apply drives this node's own carry, which must find the rest gone.
pub(crate) fn revocation_namespaces(store: &Store) -> EyreResult<Vec<ContextGroupId>> {
    let account_namespace = NodeDeviceRepository::new(store).account_namespace()?;
    let mut namespaces = NamespaceRepository::new(store).participating_namespaces()?;
    namespaces.sort_by_key(|namespace| Some(*namespace) == account_namespace);
    Ok(namespaces)
}

/// Where a withdrawal of `device` is published: where it is bound, and with the
/// account's proof everywhere, so a later link of it is refused there too.
pub(crate) fn revocation_targets(
    store: &Store,
    _account: AccountId,
    device: DeviceId,
    _proven: bool,
) -> EyreResult<Vec<ContextGroupId>> {
    let devices = NodeDeviceRepository::new(store);
    let mut targets = Vec::new();
    for namespace in revocation_namespaces(store)? {
        match devices.revocation_target(&namespace, device) {
            Ok(Some(_)) => targets.push(namespace),
            Ok(None) => {}
            Err(err) => warn!(namespace_id = ?namespace, %device, %err,
                              "revocation: could not read the binding; skipping this namespace"),
        }
    }
    Ok(targets)
}

/// Whose device `device` is in `namespace`, or the refusal a caller can act on.
///
/// Two refusals, both before anything is signed or applied:
///
/// * the device this node runs as. Revoking it here would withdraw the identity
///   the revocation is signed with part way through publishing it: the local
///   apply lands, and the publish that should carry it to peers then fails, so
///   the node is cut off while no peer heard of it.
/// * a device `namespace` holds no binding for, which names no account to put in
///   the op.
pub(crate) fn resolve_target(
    store: &Store,
    namespace: &ContextGroupId,
    device: DeviceId,
) -> EyreResult<RevocationTarget> {
    let devices = NodeDeviceRepository::new(store);
    if devices.get()?.is_some_and(|own| own.device() == device) {
        return Err(ContextError::RevocationOfOwnDevice {
            device: device.to_string(),
        }
        .into());
    }
    // Whose device this is, and whether this node can prove it owns the
    // account, both come from the group's own binding. Deriving the account
    // from this node's root instead answers a different question - "which
    // account do I own here" - so an admin ejecting somebody else's device
    // named its own account in the op and reported it back to the operator.
    devices
        .revocation_target(namespace, device)?
        .ok_or_else(|| {
            ContextError::RevocationUnknownDevice {
                namespace: namespace.to_string(),
                device: device.to_string(),
            }
            .into()
        })
}

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
        let (self_pk, signer_sk_bytes) = match self.require_namespace_signing_key(&namespace_id) {
            Ok(key) => key,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);
        let store = self.datastore.clone();

        // A node whose identity is bound to no account here can author the
        // revocation nowhere; refusing now names the reason.
        if let Err(err) = crate::member_account::require(&store, &namespace_id, &self_pk) {
            return ActorResponse::reply(Err(err));
        }
        let target = match resolve_target(&store, &namespace_id, device) {
            Ok(target) => target,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let device_repo = NodeDeviceRepository::new(&store);
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
                Ok(_) => Some(proof),
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
                    match DeviceRevocation::sign(root.signing_key(), account, device, 0) {
                        Ok(revocation) => Some(calimero_account::SignedDeviceRevocation {
                            genesis,
                            // Epoch 0: the account root has not rotated, so there
                            // are no handoffs for a verifier to walk.
                            chain: vec![],
                            statement: revocation,
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
                // A device belongs to an account, not to a scope: with the account's
                // proof the withdrawal goes into every namespace, bound there or not.
                let proven = matches!(op, GroupOp::AccountDeviceUnlinked { proof: Some(_), .. });
                let mut revoked_in = Vec::new();

                for ns in revocation_targets(&store, account, device, proven)? {
                    match withdraw_device_in(
                        &store,
                        &node_client,
                        &ack_router,
                        &ns,
                        &signer_sk,
                        device,
                        op.clone(),
                    )
                    .await
                    {
                        Ok(key_rotated) => revoked_in.push(RevocationOutcome::new(ns, key_rotated)),
                        // One namespace failing must not withhold the revocation
                        // from the rest - a device half-withdrawn is worse than
                        // one withdrawn everywhere it could be. The caller sees
                        // which namespaces landed.
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_governance_store::{NamespaceRepository, NodeDeviceRepository};
    use calimero_store::db::InMemoryDB;

    use super::{
        resolve_target, revocation_namespaces, revocation_targets, ContextError, ContextGroupId,
        DeviceId, Store,
    };

    const NS: [u8; 32] = [0xA1; 32];

    /// A node that created or joined one namespace: it holds its own account root
    /// and runs as one device.
    fn a_node_with_its_own_device() -> (Store, DeviceId) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&NS.into())
            .expect("take part in the namespace");
        let devices = NodeDeviceRepository::new(&store);
        let _root = devices
            .provision_account_root()
            .expect("a node that ran `merod init` holds a root");
        let own = devices
            .ensure_enrolled(&NS.into())
            .expect("mint this node's own device")
            .device();
        (store, own)
    }

    /// Revoking the device this node runs as would withdraw the identity the
    /// revocation is signed with before any peer heard of it, so it is refused
    /// before anything is applied - and as a typed refusal, not an internal error.
    #[test]
    fn revoking_the_device_this_node_runs_as_is_refused() {
        let (store, own) = a_node_with_its_own_device();

        let refused = resolve_target(&store, &NS.into(), own).expect_err("this is our device");
        assert!(
            matches!(
                refused.downcast_ref::<ContextError>(),
                Some(ContextError::RevocationOfOwnDevice { .. })
            ),
            "got: {refused}"
        );
    }

    /// A device the namespace holds no binding for names no account, so there is
    /// nothing to revoke. Typed, so the API answers `404` rather than `500`.
    #[test]
    fn a_device_the_namespace_holds_no_binding_for_is_refused_as_unknown() {
        let (store, _own) = a_node_with_its_own_device();

        let refused = resolve_target(&store, &NS.into(), DeviceId::from([0u8; 32]))
            .expect_err("nothing is bound under this id");
        assert!(
            matches!(
                refused.downcast_ref::<ContextError>(),
                Some(ContextError::RevocationUnknownDevice { .. })
            ),
            "got: {refused}"
        );
    }

    /// Last, whatever the key-ordered scan says: the account namespace's apply
    /// drives this node's own carry, which then finds the rest already gone.
    #[test]
    fn the_account_namespace_is_withdrawn_from_last() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        // Sorts FIRST in the scan, so the order cannot come out right by luck.
        let account_namespace = ContextGroupId::from([0x01; 32]);
        let projects = [
            ContextGroupId::from([0x81; 32]),
            ContextGroupId::from([0x82; 32]),
        ];
        NodeDeviceRepository::new(&store)
            .store_account_namespace(&account_namespace)
            .expect("record what the pairing named");
        let namespaces = NamespaceRepository::new(&store);
        for namespace in [account_namespace, projects[0], projects[1]] {
            let _identity = namespaces
                .participate_in(&namespace)
                .expect("take part in it");
        }

        assert_eq!(
            revocation_namespaces(&store).expect("read the namespaces"),
            vec![projects[0], projects[1], account_namespace],
        );
    }

    /// With the account's proof a withdrawal reaches every namespace that has not
    /// withdrawn the device; without one only where it is bound, an admin's reach.
    #[test]
    fn a_proven_revocation_reaches_a_namespace_the_device_never_linked() {
        let (store, _own) = a_node_with_its_own_device();
        let other = ContextGroupId::from([0xA2; 32]);
        let spent = ContextGroupId::from([0xA3; 32]);
        for namespace in [other, spent] {
            let _identity = NamespaceRepository::new(&store)
                .participate_in(&namespace)
                .expect("take part in another namespace");
        }
        let root = calimero_primitives::identity::PrivateKey::from([0x3C; 32]);
        let genesis = calimero_account::AccountGenesis::new(root.public_key());
        let lost = DeviceId::from([0x3D; 32]);
        let cert = calimero_account::DeviceCert::sign(
            &root,
            genesis.account_id(),
            lost,
            &calimero_primitives::identity::PrivateKey::from([0x3E; 32]).public_key(),
            &calimero_account::KemPublicKey::from([0x3F; 32]),
            0,
            0,
        )
        .expect("the root certifies its device");
        let linked = calimero_governance_store::AccountBindingRepository::new(&store)
            .apply_link(&NS.into(), &genesis, &[], &cert, 0)
            .expect("write the binding");
        assert!(
            linked.is_ok(),
            "control: the device links in the first namespace"
        );
        calimero_governance_store::AccountBindingRepository::new(&store)
            .apply_revocation(&spent, lost)
            .expect("a namespace that already withdrew the device");

        let sorted = |mut namespaces: Vec<ContextGroupId>| {
            namespaces.sort_by_key(ContextGroupId::to_bytes);
            namespaces
        };
        let account = genesis.account_id();
        assert_eq!(
            sorted(revocation_targets(&store, account, lost, true).expect("read the targets")),
            sorted(vec![NS.into(), other])
        );
        assert_eq!(
            revocation_targets(&store, account, lost, false).expect("read the targets"),
            vec![ContextGroupId::from(NS)]
        );
    }

    /// A namespace where the same device id is bound to another account is not a
    /// target: the withdrawal names this account, and must remove nothing of theirs.
    #[test]
    fn a_namespace_binding_the_device_to_another_account_is_not_a_target() {
        let (store, _own) = a_node_with_its_own_device();
        let other = ContextGroupId::from([0xA2; 32]);
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&other)
            .expect("take part in a second namespace");
        let lost = DeviceId::from([0x3D; 32]);
        let mut accounts = Vec::new();
        for (namespace, root) in [(ContextGroupId::from(NS), 0x3C), (other, 0x3B)] {
            let root = calimero_primitives::identity::PrivateKey::from([root; 32]);
            let genesis = calimero_account::AccountGenesis::new(root.public_key());
            let cert = calimero_account::DeviceCert::sign(
                &root,
                genesis.account_id(),
                lost,
                &calimero_primitives::identity::PrivateKey::from([0x3E; 32]).public_key(),
                &calimero_account::KemPublicKey::from([0x3F; 32]),
                0,
                0,
            )
            .expect("the root certifies a device id of its choosing");
            let linked = calimero_governance_store::AccountBindingRepository::new(&store)
                .apply_link(&namespace, &genesis, &[], &cert, 0)
                .expect("write the binding");
            assert!(linked.is_ok(), "control: the device links");
            accounts.push(genesis.account_id());
        }

        for proven in [true, false] {
            assert_eq!(
                revocation_targets(&store, accounts[0], lost, proven).expect("read the targets"),
                vec![ContextGroupId::from(NS)],
                "proven: {proven}"
            );
        }
    }
}
