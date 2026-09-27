//! The namespace TEE key: one key a value only the TEE may read is sealed to.
//!
//! Sealing such a value to each TEE authority's own key leaves a TEE admitted
//! later unable to open it until a TEE that can writes it again, which it
//! cannot do if every such TEE is down. Sealing it to one namespace key instead,
//! and handing that key to each TEE authority as it arrives
//! ([`GroupOp::TeeVaultKeyDelivered`]), lets any TEE that holds the key open
//! everything sealed before it was admitted.
//!
//! Two TEEs may each create a key before either sees the other's. Nothing picks
//! a winner: each hands every key it holds to every TEE authority that lacks
//! it, so both end up holding both, a run opens with whichever key a value was
//! sealed to, and seals to the lowest ([`tee_vault_keys`] returns them in
//! order).

use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::SealedEnvelope;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};

use super::read_op_log_after;
use crate::tee::decode_group_op;
use crate::{MembershipRepository, NamespaceRepository};

/// One copy of a namespace TEE key on the root's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeeVaultDelivery {
    /// The public half of the key, which names it.
    pub vault_key: PublicKey,
    /// The attested key of the TEE the copy is for.
    pub recipient_key: PublicKey,
    /// The private half, sealed to `recipient_key`.
    pub envelope: Vec<u8>,
}

/// Every [`GroupOp::TeeVaultKeyDelivered`] on the namespace root's log whose
/// signer is a `ReadOnlyTee` member there, in log order.
///
/// Apply refuses any other signer already. The signer is checked again here
/// because a key an admin published would be one the admin could read
/// everything sealed to, and the check costs one lookup.
///
/// # Errors
/// Any governance store read error.
pub fn tee_vault_deliveries(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<TeeVaultDelivery>> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let membership = MembershipRepository::new(store);
    let mut deliveries = Vec::new();
    for (seq, bytes) in &read_op_log_after(store, &root, 0, usize::MAX)? {
        let Ok(signed) = decode_group_op(&root, *seq, bytes, "tee_vault_deliveries") else {
            continue;
        };
        let GroupOp::TeeVaultKeyDelivered {
            vault_key,
            recipient_key,
            envelope,
        } = signed.op
        else {
            continue;
        };
        let Some(signer) = crate::member_account_in_namespace(store, &root, &signed.signer)? else {
            continue;
        };
        if membership.role_of(&root, &signer)? != Some(GroupMemberRole::ReadOnlyTee) {
            continue;
        }
        deliveries.push(TeeVaultDelivery {
            vault_key,
            recipient_key,
            envelope,
        });
    }
    Ok(deliveries)
}

/// The namespace TEE keys delivered to the holder of `recipient`, lowest public
/// key first, each once.
///
/// A copy counts only if it opens with `recipient` to the private half of the
/// key it names; any other is ignored.
///
/// # Errors
/// Any governance store read error.
pub fn tee_vault_keys(
    store: &Store,
    group_id: &ContextGroupId,
    recipient: &PrivateKey,
) -> EyreResult<Vec<PrivateKey>> {
    let mine = recipient.public_key();
    let mut keys: Vec<PrivateKey> = Vec::new();
    for delivery in tee_vault_deliveries(store, group_id)? {
        if delivery.recipient_key != mine
            || keys.iter().any(|k| k.public_key() == delivery.vault_key)
        {
            continue;
        }
        if let Some(key) = open_vault_key(&delivery, recipient) {
            keys.push(key);
        }
    }
    keys.sort_by_key(|key| *key.public_key());
    Ok(keys)
}

/// The envelope for `vault` sealed to `recipient`, for a
/// [`GroupOp::TeeVaultKeyDelivered`].
///
/// # Errors
/// If `recipient` is not a usable key.
pub fn seal_tee_vault_key(vault: &PrivateKey, recipient: &PublicKey) -> EyreResult<Vec<u8>> {
    calimero_crypto::seal_to_root(&mut rand::rng(), recipient, vault.as_bytes().to_vec())
        .map(|envelope| envelope.to_bytes())
        .map_err(|err| eyre!("could not seal the namespace TEE key: {err:?}"))
}

fn open_vault_key(delivery: &TeeVaultDelivery, recipient: &PrivateKey) -> Option<PrivateKey> {
    let envelope = SealedEnvelope::from_bytes(&delivery.envelope)?;
    let bytes: [u8; 32] = calimero_crypto::open_sealed(recipient, &envelope)
        .ok()?
        .try_into()
        .ok()?;
    let key = PrivateKey::from(bytes);
    (key.public_key() == delivery.vault_key).then_some(key)
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::Store;

    use super::{seal_tee_vault_key, tee_vault_deliveries, tee_vault_keys};
    use crate::test_fixtures::{enrol_member, nest_for_test, test_store};
    use crate::{apply_local_signed_group_op, MembershipRepository};

    /// A namespace root with one admin and two admitted TEEs.
    struct Namespace {
        store: Store,
        root: ContextGroupId,
        admin: PrivateKey,
        tee: PrivateKey,
        other_tee: PrivateKey,
        nonce: std::cell::Cell<u64>,
    }

    impl Namespace {
        fn new() -> Self {
            let store = test_store();
            let root = ContextGroupId::from([0xB7; 32]);
            let members = MembershipRepository::new(&store);
            let key = || PrivateKey::random(&mut rand::rng());
            let (admin, tee, other_tee) = (key(), key(), key());
            for (sk, role) in [
                (&admin, GroupMemberRole::Admin),
                (&tee, GroupMemberRole::ReadOnlyTee),
                (&other_tee, GroupMemberRole::ReadOnlyTee),
            ] {
                let account = enrol_member(&store, &root, &sk.public_key());
                members.add_member(&root, &account, role).unwrap();
            }
            Self {
                store,
                root,
                admin,
                tee,
                other_tee,
                nonce: std::cell::Cell::new(0),
            }
        }

        /// Deliver `vault` to `recipient`, signed by `signer`, through apply.
        fn deliver(
            &self,
            group: ContextGroupId,
            signer: &PrivateKey,
            vault: &PrivateKey,
            recipient: &PublicKey,
        ) -> eyre::Result<()> {
            self.deliver_named(group, signer, vault.public_key(), vault, recipient)
        }

        fn deliver_named(
            &self,
            group: ContextGroupId,
            signer: &PrivateKey,
            vault_key: PublicKey,
            vault: &PrivateKey,
            recipient: &PublicKey,
        ) -> eyre::Result<()> {
            self.nonce.set(self.nonce.get() + 1);
            let op = SignedGroupOp::sign(
                signer,
                group.to_bytes().into(),
                vec![],
                self.nonce.get(),
                GroupOp::TeeVaultKeyDelivered {
                    vault_key,
                    recipient_key: *recipient,
                    envelope: seal_tee_vault_key(vault, recipient).unwrap(),
                },
            )
            .unwrap();
            apply_local_signed_group_op(&self.store, &op)
        }

        fn keys_of(&self, holder: &PrivateKey) -> Vec<PublicKey> {
            tee_vault_keys(&self.store, &self.root, holder)
                .unwrap()
                .iter()
                .map(PrivateKey::public_key)
                .collect()
        }
    }

    /// The copy a TEE was handed opens for it, and for nobody else.
    #[test]
    fn a_tee_holds_the_key_delivered_to_it() {
        let ns = Namespace::new();
        let vault = PrivateKey::random(&mut rand::rng());
        ns.deliver(ns.root, &ns.tee, &vault, &ns.tee.public_key())
            .unwrap();

        assert_eq!(ns.keys_of(&ns.tee), vec![vault.public_key()]);
        assert!(ns.keys_of(&ns.other_tee).is_empty());

        ns.deliver(ns.root, &ns.tee, &vault, &ns.other_tee.public_key())
            .unwrap();
        assert_eq!(ns.keys_of(&ns.other_tee), vec![vault.public_key()]);
    }

    /// An admin may not hand the TEEs a key: it would hold the key itself and
    /// read everything sealed to it.
    #[test]
    fn only_an_admitted_tee_may_deliver_the_key() {
        let ns = Namespace::new();
        let vault = PrivateKey::random(&mut rand::rng());
        assert!(ns
            .deliver(ns.root, &ns.admin, &vault, &ns.tee.public_key())
            .is_err());
        assert!(tee_vault_deliveries(&ns.store, &ns.root)
            .unwrap()
            .is_empty());
        assert!(ns.keys_of(&ns.tee).is_empty());
    }

    #[test]
    fn the_key_lives_on_the_namespace_root() {
        let ns = Namespace::new();
        let child = ContextGroupId::from([0xB8; 32]);
        nest_for_test(&ns.store, &ns.root, &child);
        let vault = PrivateKey::random(&mut rand::rng());
        assert!(ns
            .deliver(child, &ns.tee, &vault, &ns.tee.public_key())
            .is_err());
    }

    /// A copy that opens to some other key than the one it names is ignored,
    /// so a delivery cannot pass off one key under another's name.
    #[test]
    fn a_copy_that_opens_to_another_key_is_ignored() {
        let ns = Namespace::new();
        let named = PrivateKey::random(&mut rand::rng()).public_key();
        let sealed = PrivateKey::random(&mut rand::rng());
        ns.deliver_named(ns.root, &ns.tee, named, &sealed, &ns.tee.public_key())
            .unwrap();
        assert!(ns.keys_of(&ns.tee).is_empty());
    }

    /// Two keys created concurrently are both held, lowest first, which is the
    /// one a run seals to.
    #[test]
    fn every_key_delivered_is_held_lowest_first() {
        let ns = Namespace::new();
        let (a, b) = (
            PrivateKey::random(&mut rand::rng()),
            PrivateKey::random(&mut rand::rng()),
        );
        for vault in [&a, &b, &a] {
            ns.deliver(ns.root, &ns.other_tee, vault, &ns.tee.public_key())
                .unwrap();
        }
        let mut expected = vec![a.public_key(), b.public_key()];
        expected.sort();
        assert_eq!(ns.keys_of(&ns.tee), expected);
    }
}
