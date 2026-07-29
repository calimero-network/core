//! This node's own device identity, per namespace.
//!
//! Every other account row in this crate is *replicated* state — what the group
//! collectively knows about who may author. This one is the opposite: it is the
//! secret half, node-local, never gossiped, and the only thing that can open a
//! scope key wrapped for this device.
//!
//! One row per namespace rather than one per node. A node is a distinct CRDT
//! replica in each namespace it joins, and reusing one agreement secret across
//! namespaces would let a peer in one namespace test whether a device in
//! another is the same machine — a correlation the per-namespace identity model
//! otherwise denies them.
//!
//! The `DeviceId` is stored alongside the secret instead of being recomputed
//! because it cannot be recomputed: it is `H(account ‖ nonce)` over a nonce
//! drawn once at enrollment. Losing it would orphan the device's replica
//! lineage — its counter slots and HLC seed — even though the machine and its
//! keys were unchanged.

use calimero_account::{AccountGenesis, AccountId, DeviceId, KemPublicKey};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::X25519SecretKey;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{NodeDeviceIdentity, NodeDeviceIdentityValue};
use calimero_store::Store;
use eyre::Result as EyreResult;
use rand::Rng as _;

/// Serializes the read-check-write in
/// [`NodeDeviceRepository::ensure_for_account`] across callers, making the
/// mint-if-absent atomic without a store-level compare-and-swap. Same pattern and
/// same reason as `GROUP_KEY_EPOCH_WRITE_LOCK`: two callers could otherwise both
/// observe an absent row and both mint, and the second `put` would win — handing
/// this machine a second `DeviceId` while the group's CRDT state still sat under
/// the first.
///
/// Sufficient without snapshot isolation because a base-`Store` `handle.put` is
/// write-through: the row is visible the instant `put` returns, before the lock is
/// released, so the next holder's `get` always observes it.
static NODE_DEVICE_MINT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The minimum needed to open a scope key addressed to this node's device.
///
/// Separate from [`NodeDevice`] because the unwrap paths need only these two
/// values, and requiring the account's root key just to read a secret would make
/// every receive path depend on resolving an identity it does not otherwise care
/// about.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeviceSecret {
    /// The replica this node speaks as.
    pub device: DeviceId,
    /// The agreement secret matching the certificate's `kem_pk`.
    pub kem_secret: X25519SecretKey,
}

/// This node's full enrollment for one namespace.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NodeDevice {
    /// The account this device speaks for.
    pub account: AccountId,
    /// The genesis that addresses [`Self::account`], reconstructed from the
    /// stored nonce and the namespace identity key that roots it.
    ///
    /// Carried because a device link has to put the genesis on the wire, and
    /// pairing a second device means publishing another link naming this same
    /// account.
    pub genesis: AccountGenesis,
    /// What opens scope keys addressed to this device.
    pub secret: DeviceSecret,
}

impl NodeDevice {
    /// The replica this node speaks as.
    #[must_use]
    pub const fn device(&self) -> DeviceId {
        self.secret.device
    }

    /// The public half to publish in this device's certificate.
    #[must_use]
    pub fn kem_public_key(&self) -> KemPublicKey {
        KemPublicKey::from(*self.secret.kem_secret.public_key().as_bytes())
    }
}

/// Reads and writes this node's per-namespace device identity.
pub struct NodeDeviceRepository<'a> {
    store: &'a Store,
}

impl<'a> NodeDeviceRepository<'a> {
    /// Bind to `store`.
    #[must_use]
    pub const fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// This node's device identity for `namespace`, if it has enrolled one.
    ///
    /// `None` means this node has no device in the namespace, which is not an
    /// error: it is the state of every node that has not yet enrolled, and the
    /// unwrap path uses it to decide that a device-addressed envelope cannot be
    /// for us.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn get(&self, namespace: &ContextGroupId) -> EyreResult<Option<NodeDevice>> {
        let key = NodeDeviceIdentity::new(namespace.to_bytes());
        Ok(self
            .store
            .handle()
            .get(&key)?
            .map(|value: NodeDeviceIdentityValue| {
                let genesis = AccountGenesis::new(
                    PublicKey::from(value.account_root_pk),
                    value.account_nonce,
                );
                NodeDevice {
                    account: genesis.account_id(),
                    genesis,
                    secret: DeviceSecret {
                        device: DeviceId::from(value.device_id),
                        kem_secret: X25519SecretKey::from(value.kem_secret),
                    },
                }
            }))
    }

    /// Just what the unwrap paths need: this node's device id and agreement
    /// secret, without resolving the account that owns them.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn device_secret(&self, namespace: &ContextGroupId) -> EyreResult<Option<DeviceSecret>> {
        let key = NodeDeviceIdentity::new(namespace.to_bytes());
        Ok(self
            .store
            .handle()
            .get(&key)?
            .map(|value: NodeDeviceIdentityValue| DeviceSecret {
                device: DeviceId::from(value.device_id),
                kem_secret: X25519SecretKey::from(value.kem_secret),
            }))
    }

    /// This node's device identity for `namespace`, minting one for `account` if
    /// absent.
    ///
    /// Idempotent, and that matters more than it looks: minting twice would mint
    /// two `DeviceId`s for one machine, and the second would start with empty
    /// counter slots and a fresh HLC lineage while the first still held the
    /// group's history under the old id. Callers may therefore invoke this on
    /// every enrollment attempt without checking first.
    ///
    /// The read-check-write is serialized by [`NODE_DEVICE_MINT_LOCK`], so
    /// idempotence holds against concurrent callers and not merely sequential
    /// ones — two threads could otherwise both see an absent row and both mint.
    ///
    /// A stored identity is returned **as is**, even when it was minted for a
    /// different account. Re-minting would be the wrong repair: the old device
    /// id is already the replica id in this namespace's CRDT state, and
    /// silently replacing it would strand that state. Moving a machine to
    /// another account is a fresh enrollment after the old device is revoked,
    /// which is what makes the revocation tombstone terminal.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn ensure_enrolled(
        &self,
        namespace: &ContextGroupId,
        root_sign_pk: &PublicKey,
    ) -> EyreResult<NodeDevice> {
        // Recover a poisoned lock: the guarded state lives in the store, not the
        // guard, so a prior panic leaves nothing inconsistent behind.
        let _guard = NODE_DEVICE_MINT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let genesis = AccountGenesis::new(*root_sign_pk, rand::thread_rng().gen::<[u8; 16]>());
        self.enroll_locked(namespace, genesis)
    }

    /// This node's device identity for `namespace`, enrolling it into an
    /// **existing** account if absent.
    ///
    /// The pairing counterpart of [`ensure_enrolled`](Self::ensure_enrolled). The
    /// genesis arrives from the device that already holds the account, and it has
    /// to: `DeviceId` is `H(account ‖ nonce)`, so this node cannot mint its own id
    /// until it knows the account, while the account holder cannot sign this
    /// device's certificate until it knows the id and KEM key. Pairing is therefore
    /// a two-way exchange, and this is its first half — the half that produces the
    /// values the certificate will name.
    ///
    /// Idempotent on the same terms as `ensure_enrolled`, including that a stored
    /// identity wins even when a *different* account asks: re-minting would strand
    /// the replica state already written under the old device id.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn ensure_enrolled_into(
        &self,
        namespace: &ContextGroupId,
        genesis: AccountGenesis,
    ) -> EyreResult<NodeDevice> {
        let _guard = NODE_DEVICE_MINT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.enroll_locked(namespace, genesis)
    }

    /// Mint-if-absent for a decided genesis. Callers hold
    /// [`NODE_DEVICE_MINT_LOCK`].
    fn enroll_locked(
        &self,
        namespace: &ContextGroupId,
        genesis: AccountGenesis,
    ) -> EyreResult<NodeDevice> {
        if let Some(existing) = self.get(namespace)? {
            return Ok(existing);
        }

        let mut rng = rand::thread_rng();
        let account = genesis.account_id();
        let device = DeviceId::mint(account, rng.gen::<[u8; 16]>());
        let kem_secret = X25519SecretKey::random(&mut rng);

        let key = NodeDeviceIdentity::new(namespace.to_bytes());
        self.store.handle().put(
            &key,
            &NodeDeviceIdentityValue {
                account_root_pk: *AsRef::<[u8; 32]>::as_ref(&genesis.root_sign_pk),
                account_nonce: genesis.nonce,
                device_id: *device.as_bytes(),
                kem_secret: *kem_secret.as_bytes(),
            },
        )?;

        Ok(NodeDevice {
            account,
            genesis,
            secret: DeviceSecret { device, kem_secret },
        })
    }

    /// Drop this node's device identity for `namespace`. Idempotent.
    ///
    /// Called by the namespace teardown for the same reason the group keyring is
    /// dropped there: the secret is the only thing that can open scope keys
    /// wrapped for this device, so leaving it behind after the node has left
    /// keeps a decryption capability alive for state it is no longer entitled
    /// to.
    ///
    /// # Errors
    /// Propagates the store write failure.
    pub fn delete(&self, namespace: &ContextGroupId) -> EyreResult<()> {
        let key = NodeDeviceIdentity::new(namespace.to_bytes());
        self.store.handle().delete(&key)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};
    use calimero_account::AccountGenesis;
    use calimero_crypto::SharedKey;
    use calimero_primitives::identity::PrivateKey;

    fn root(seed: u8) -> PublicKey {
        PrivateKey::from([seed; 32]).public_key()
    }

    #[test]
    fn a_namespace_with_no_enrolled_device_reports_none() {
        let store = test_store();
        assert!(NodeDeviceRepository::new(&store)
            .get(&test_group_id())
            .expect("read")
            .is_none());
    }

    #[test]
    fn ensure_is_idempotent_and_keeps_the_first_device_id() {
        // The invariant the whole repository exists for. A second mint would
        // hand this machine a new replica id and strand the CRDT state held
        // under the old one.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let first = repo.ensure_enrolled(&ns, &root(1)).expect("mint");
        let second = repo.ensure_enrolled(&ns, &root(1)).expect("mint");

        assert_eq!(first.device(), second.device());
        assert_eq!(
            first.secret.kem_secret.as_bytes(),
            second.secret.kem_secret.as_bytes()
        );
    }

    #[test]
    fn a_stored_identity_survives_a_different_account_asking() {
        // Re-minting for a new account would strand replica state, so the
        // stored identity wins and re-enrollment is an explicit revoke-then-add.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let original = repo.ensure_enrolled(&ns, &root(1)).expect("mint");
        let asked_again = repo.ensure_enrolled(&ns, &root(2)).expect("mint");
        assert_eq!(original.device(), asked_again.device());
    }

    #[test]
    fn the_enrolled_account_is_reconstructible_from_the_stored_nonce() {
        // The reason the nonce is stored rather than the AccountId: pairing a
        // second device means publishing another link naming this same account,
        // and a link has to carry the GENESIS on the wire. An id is a one-way
        // hash, so storing it would leave the genesis unrecoverable and make the
        // account un-pairable after a restart.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let enrolled = repo.ensure_enrolled(&ns, &root(1)).expect("enroll");
        let reloaded = repo.get(&ns).expect("read").expect("present");

        assert_eq!(enrolled.account, reloaded.account);
        assert_eq!(enrolled.genesis, reloaded.genesis);
        assert_eq!(
            reloaded.genesis.account_id(),
            reloaded.account,
            "the reconstructed genesis must address the account it claims"
        );
        assert_eq!(
            reloaded.genesis.root_sign_pk,
            root(1),
            "the account is rooted at the namespace identity that enrolled it"
        );
    }

    #[test]
    fn a_paired_device_adopts_an_account_it_did_not_mint() {
        // The pairing half. node-B enrolls into an account rooted at node-A's key,
        // and the row has to be self-describing afterwards: nothing on node-B knows
        // whose account it is, so reconstructing the genesis cannot depend on being
        // told the root key.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        // node-A's account, as it would arrive over a pairing exchange.
        let alice_root = root(1);
        let alice = AccountGenesis::new(alice_root, [0xABu8; 16]);

        let paired = repo
            .ensure_enrolled_into(&ns, alice)
            .expect("adopt the account");
        assert_eq!(
            paired.account,
            alice.account_id(),
            "the paired device must speak for the account it was given, not a new one"
        );

        // Reloaded with no external input at all.
        let reloaded = repo.get(&ns).expect("read").expect("present");
        assert_eq!(reloaded.account, alice.account_id());
        assert_eq!(reloaded.genesis, alice);
        assert_eq!(
            reloaded.genesis.root_sign_pk, alice_root,
            "the account stays rooted at the pairing device's key, not this node's"
        );

        // Distinct replica from whatever node-A holds: same account, different id.
        let other = test_store();
        let node_a = NodeDeviceRepository::new(&other)
            .ensure_enrolled(&ns, &alice_root)
            .expect("enroll");
        assert_ne!(
            paired.device(),
            node_a.device(),
            "one account, two devices — the replica ids must differ"
        );
    }

    #[test]
    fn an_adopted_account_does_not_displace_an_existing_enrollment() {
        // Same terms as ensure_enrolled: re-minting would strand the replica state
        // already written under the stored device id.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let mine = repo.ensure_enrolled(&ns, &root(1)).expect("enroll");
        let asked = repo
            .ensure_enrolled_into(&ns, AccountGenesis::new(root(2), [0xCDu8; 16]))
            .expect("adopt");
        assert_eq!(mine.device(), asked.device());
        assert_eq!(mine.account, asked.account);
    }

    #[test]
    fn each_namespace_gets_its_own_device_and_secret() {
        // Cross-namespace correlation guard: one machine must not present the
        // same replica id or the same agreement key in two namespaces.
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);
        let acct = root(1);

        let a = repo
            .ensure_enrolled(&ContextGroupId::from([0xAAu8; 32]), &acct)
            .expect("mint");
        let b = repo
            .ensure_enrolled(&ContextGroupId::from([0xBBu8; 32]), &acct)
            .expect("mint");

        assert_ne!(a.device(), b.device());
        assert_ne!(
            a.secret.kem_secret.as_bytes(),
            b.secret.kem_secret.as_bytes()
        );
    }

    #[test]
    fn the_stored_secret_agrees_with_the_published_public_key() {
        // The reason the secret is stored at all: it must reproduce the exact
        // agreement a sender performed against the certificate's `kem_pk`.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let minted = repo.ensure_enrolled(&ns, &root(1)).expect("mint");
        let published = minted.kem_public_key();

        // A sender wraps to the published key...
        let sender = X25519SecretKey::random(&mut rand::thread_rng());
        let sender_side = SharedKey::from_x25519(
            &sender,
            &calimero_crypto::X25519PublicKey::from(*published.as_bytes()),
        )
        .expect("agree");

        // ...and the identity reloaded from the store opens it.
        let reloaded = repo.get(&ns).expect("read").expect("present");
        let device_side = SharedKey::from_x25519(&reloaded.secret.kem_secret, &sender.public_key())
            .expect("agree");

        let (nonce, ciphertext) = sender_side.encrypt(b"scope key".to_vec()).expect("seal");
        assert_eq!(
            device_side.decrypt(ciphertext, nonce).expect("open"),
            b"scope key".to_vec()
        );
    }

    #[test]
    fn concurrent_mints_agree_on_one_device_id() {
        // The read-check-write is not atomic in the store, so without the lock two
        // callers both observe an absent row, both mint, and the second `put` wins
        // — handing this machine a second DeviceId while the group's CRDT state
        // still sits under the first.
        use std::sync::Arc;

        let store = test_store();
        let ns = test_group_id();
        let acct = root(1);

        let observed: Vec<DeviceId> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let store = Arc::new(&store);
                    scope.spawn(move || {
                        NodeDeviceRepository::new(&store)
                            .ensure_enrolled(&ns, &acct)
                            .expect("mint")
                            .device()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("thread"))
                .collect()
        });

        let first = observed[0];
        assert!(
            observed.iter().all(|d| *d == first),
            "every concurrent caller must get the same device id, got {observed:?}"
        );
        assert_eq!(
            NodeDeviceRepository::new(&store)
                .get(&ns)
                .expect("read")
                .expect("present")
                .device(),
            first,
            "the stored identity must be the one every caller was handed"
        );
    }

    #[test]
    fn delete_removes_the_secret_and_is_idempotent() {
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let _ = repo.ensure_enrolled(&ns, &root(1)).expect("mint");
        repo.delete(&ns).expect("delete");
        assert!(repo.get(&ns).expect("read").is_none());
        repo.delete(&ns).expect("delete again");
    }
}
