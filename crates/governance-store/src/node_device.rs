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
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    NodeAccountRoot, NodeAccountRootValue, NodeDeviceIdentity, NodeDeviceIdentityValue,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use rand::Rng as _;

use crate::collect_keys_with_prefix;

/// Serializes the generate-once in [`NodeDeviceRepository::ensure_account_root`],
/// for the same reason as the device mint below: two callers could both observe an
/// absent row and both generate, and the second `put` would win — replacing the
/// root that already certified this node's devices, which is unrecoverable.
static ACCOUNT_ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// This node's account root — the one key that survives losing every device.
///
/// Node-level, not per-namespace: it is what certifies a replacement device after
/// total loss, so it cannot live in the state being replaced. Per-namespace account
/// ids stay distinct because the nonce is derived per namespace rather than shared,
/// which is what lets recovery and unlinkability hold at once.
// Deliberately NOT `Clone`: `PrivateKey` is not, and that is the right default for
// a recovery key — every copy is another place it can leak from, and none of them
// are covered by the original's wipe.
#[non_exhaustive]
pub struct AccountRoot {
    secret: PrivateKey,
}

impl std::fmt::Debug for AccountRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never derived: this is the recovery key.
        write!(f, "AccountRoot([redacted])")
    }
}

impl AccountRoot {
    /// The public half, which every genesis this root produces names.
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        self.secret.public_key()
    }

    /// The signing key, for minting device certificates and root-key handoffs.
    ///
    /// The only two things this key may ever do. It does not sign ops and does not
    /// receive data, which is what allows it to live offline.
    #[must_use]
    pub const fn signing_key(&self) -> &PrivateKey {
        &self.secret
    }

    /// This root's genesis for `namespace`.
    ///
    /// The nonce is derived from the root **secret** and the namespace id, so the
    /// account id is recomputable from the root alone — no stored nonce to lose —
    /// while remaining uncorrelatable across namespaces by anyone who does not hold
    /// the secret.
    #[must_use]
    pub fn genesis_for(&self, namespace: &ContextGroupId) -> AccountGenesis {
        AccountGenesis::new(
            self.public_key(),
            calimero_account::derive_account_nonce(self.secret.as_bytes(), &namespace.to_bytes()),
        )
    }

    /// The `AccountId` this root owns in `namespace`.
    #[must_use]
    pub fn account_for(&self, namespace: &ContextGroupId) -> AccountId {
        self.genesis_for(namespace).account_id()
    }
}

/// Serializes the read-check-write in
/// [`NodeDeviceRepository::ensure_enrolled`] across callers, making the
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
    /// stored nonce and the account root key that roots it.
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

/// What a revocation of one device is about, resolved from the group's own
/// bindings rather than from anything this node derives for itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RevocationTarget {
    /// The account the device actually speaks for.
    pub account: AccountId,
    /// Whether this node holds the account root that owns it, and can therefore
    /// mint the self-certifying proof that needs no admin.
    pub self_service: bool,
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

    /// This node's account root, generating it once if absent.
    ///
    /// Idempotent, and the lock matters more here than anywhere else in this file:
    /// replacing a root that has already certified devices cannot be undone, and
    /// there is no second copy to recover from.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn ensure_account_root(&self) -> EyreResult<AccountRoot> {
        let _guard = ACCOUNT_ROOT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(existing) = self.account_root()? {
            return Ok(existing);
        }

        let secret = PrivateKey::random(&mut rand::thread_rng());
        self.store.handle().put(
            &NodeAccountRoot::new(),
            &NodeAccountRootValue {
                root_secret: *secret.as_bytes(),
            },
        )?;
        Ok(AccountRoot { secret })
    }

    /// This node's account root, if one has been generated.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn account_root(&self) -> EyreResult<Option<AccountRoot>> {
        Ok(self
            .store
            .handle()
            .get(&NodeAccountRoot::new())?
            .map(|value: NodeAccountRootValue| AccountRoot {
                secret: PrivateKey::from(value.root_secret),
            }))
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
    /// A stored identity is returned as is whenever it still serves; see
    /// [`stored_identity_still_serves`](Self::stored_identity_still_serves) for
    /// the two cases where it cannot and is replaced instead.
    ///
    /// # Errors
    /// Propagates the store read or write failure, or refuses when the stored
    /// device is linked to a different account.
    pub fn ensure_enrolled(&self, namespace: &ContextGroupId) -> EyreResult<NodeDevice> {
        // Recover a poisoned lock: the guarded state lives in the store, not the
        // guard, so a prior panic leaves nothing inconsistent behind.
        let _guard = NODE_DEVICE_MINT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Rooted at the node's account root, not its namespace identity. The
        // namespace identity dies with the node, which is the case recovery exists
        // for; the root is kept offline precisely so it does not.
        //
        // The nonce is DERIVED rather than generated, so a node holding only the
        // root can recompute this exact account without the row below. The row is a
        // read cache, not the source of truth — and it has to exist anyway, because
        // a paired device's genesis belongs to another node's root and cannot be
        // derived here at all.
        let genesis = self.ensure_account_root()?.genesis_for(namespace);
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
    /// Idempotent on the same terms as `ensure_enrolled`, and it applies the same
    /// rule to a stored row that names a different account.
    ///
    /// # Errors
    /// Propagates the store read or write failure, or refuses when the stored
    /// device is linked to a different account.
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

    /// May the row already stored for `namespace` be handed back to an enrolment
    /// into `account`?
    ///
    /// A node holds at most one device per namespace, so this one row is the whole
    /// slot. `Ok(true)` keeps it, `Ok(false)` releases it to be re-minted, and an
    /// error refuses the enrolment outright.
    ///
    /// **The default is to keep it**, because the device id is already the replica
    /// id in this namespace's CRDT state: counter slots and an HLC lineage are
    /// held under it, and silently minting a second id would strand all of that
    /// while the group's history still sat under the first. Two cases defeat that
    /// reasoning, and in both the row is worthless rather than load-bearing:
    ///
    /// - **The device is revoked.** A tombstone is terminal, so the id can never
    ///   be linked again — in this account or any other. Keeping it locks the node
    ///   out of the namespace with its own revocation: enrolment keeps minting
    ///   certificates for a spent id and every peer refuses them, with nothing to
    ///   say why. Nothing else releases the slot, so "re-enrolling mints a fresh
    ///   one" is only true if this does it. (A node that never received the
    ///   revocation has no tombstone to read and cannot know; that is inherent to
    ///   causal revocation, not something this can repair.)
    /// - **The device names another account and was never linked.** The row is
    ///   minted BEFORE anyone certifies it, so one `pair-init` with a mistyped
    ///   nonce — or one issued by anyone who can reach this node's admin API —
    ///   claims the slot for an account nobody here controls. An unlinked device
    ///   holds no replica state, so replacing it strands nothing, while refusing
    ///   to leaves the node unable to enroll here again short of leaving the
    ///   namespace.
    ///
    /// A *linked* row for another account is the one refusal: its replica state is
    /// real, so moving the machine between accounts has to be an explicit
    /// revoke-then-enroll rather than a silent overwrite.
    fn stored_identity_still_serves(
        &self,
        namespace: &ContextGroupId,
        existing: &NodeDevice,
        account: AccountId,
    ) -> EyreResult<bool> {
        let bindings = crate::AccountBindingRepository::new(self.store);

        if bindings.is_revoked(namespace, existing.device())? {
            return Ok(false);
        }
        if existing.account == account {
            return Ok(true);
        }
        if bindings.is_device_linked(namespace, existing.device())? {
            eyre::bail!(
                "this node already holds device {} for account {} in {namespace:?}, and it \
                 is linked — its replica state is held under that id. Moving a machine \
                 between accounts means revoking the existing device first",
                existing.device(),
                existing.account,
            );
        }
        Ok(false)
    }

    /// Mint-if-absent for a decided genesis. Callers hold
    /// [`NODE_DEVICE_MINT_LOCK`].
    fn enroll_locked(
        &self,
        namespace: &ContextGroupId,
        genesis: AccountGenesis,
    ) -> EyreResult<NodeDevice> {
        let account = genesis.account_id();
        if let Some(existing) = self.get(namespace)? {
            if self.stored_identity_still_serves(namespace, &existing, account)? {
                return Ok(existing);
            }
            // Deleting takes the KEM secret with it, which is only safe because
            // both replacement cases leave nothing addressed to it: a revoked
            // device is rotated away from, and an unlinked one was never a
            // recipient at all.
            self.delete(namespace)?;
        }

        let mut rng = rand::thread_rng();
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

    /// Who owns `device` in `namespace`, and whether this node can prove it.
    ///
    /// `None` means this namespace holds no binding for the device — it was never
    /// linked here, or the link has not synced yet. That is not a refusal on the
    /// apply side (a revocation deliberately beats the link it withdraws), but it
    /// does mean a *caller* cannot honestly name the account, so the revoke path
    /// treats it as one.
    ///
    /// **The account is read from the binding, never derived locally.** Deriving it
    /// from this node's own root answers a different question — "which account do
    /// *I* own here" — and the two only coincide when revoking your own device. An
    /// admin ejecting somebody else's device would name its own account in the op
    /// it publishes and report that account back to the operator.
    ///
    /// **Ownership is a fact about the root, not about the stored row.** A paired
    /// device's row names the account it adopted, which belongs to *another* node's
    /// root — so a row match would let a paired device claim the self-service path
    /// and sign a proof its root cannot back, which every peer then refuses. The
    /// only proof of ownership is that this node's root re-derives the same account.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn revocation_target(
        &self,
        namespace: &ContextGroupId,
        device: DeviceId,
    ) -> EyreResult<Option<RevocationTarget>> {
        let Some(binding) = crate::AccountBindingRepository::new(self.store)
            .raw_binding(namespace, device)?
            .map(|value| AccountId::from(value.account))
        else {
            return Ok(None);
        };

        let self_service = self
            .account_root()?
            .is_some_and(|root| root.account_for(namespace) == binding);

        Ok(Some(RevocationTarget {
            account: binding,
            self_service,
        }))
    }

    /// Every namespace this node holds a device identity in.
    ///
    /// This is the node's own participation set, and it is deliberately not a
    /// membership query. A paired device is a device of someone else's account
    /// and a member of nothing, so every membership-derived listing omits it —
    /// including `list_all_groups`, which the startup subscription rehydration
    /// iterates. Without this the paired device comes back from a restart
    /// subscribed to no gossip topic at all: no error, no log line, it simply
    /// stops receiving ops.
    ///
    /// The row family is the right source precisely because it is written by
    /// enrollment rather than by joining — one row per namespace this node can
    /// speak in, whether or not it is a member there.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn enrolled_namespaces(&self) -> EyreResult<Vec<ContextGroupId>> {
        let keys = collect_keys_with_prefix(
            self.store,
            NodeDeviceIdentity::new([0u8; 32]),
            calimero_store::key::NODE_DEVICE_IDENTITY_PREFIX,
            |_| true,
        )?;
        Ok(keys
            .into_iter()
            .map(|key| ContextGroupId::from(key.namespace_id()))
            .collect())
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
    use crate::AccountBindingRepository;
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

        let first = repo.ensure_enrolled(&ns).expect("mint");
        let second = repo.ensure_enrolled(&ns).expect("mint");

        assert_eq!(first.device(), second.device());
        assert_eq!(
            first.secret.kem_secret.as_bytes(),
            second.secret.kem_secret.as_bytes()
        );
    }

    #[test]
    fn a_linked_row_for_another_account_is_refused_rather_than_replaced() {
        // Re-minting over a LINKED device would strand the replica state already
        // written under its id, so the stored identity wins and moving the machine
        // between accounts stays an explicit revoke-then-enroll.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let alice_sk = PrivateKey::from([1u8; 32]);
        let alice = AccountGenesis::new(alice_sk.public_key(), [0xABu8; 16]);
        let paired = repo.ensure_enrolled_into(&ns, alice).expect("adopt");

        let cert = calimero_account::sign_device_cert(
            &alice_sk,
            paired.account,
            paired.device(),
            &root(9),
            &paired.kem_public_key(),
            0,
            0,
        )
        .expect("sign the certificate");
        let _binding = AccountBindingRepository::new(&store)
            .apply_link(&ns, &alice, &[], &cert)
            .expect("store")
            .expect("the credential must be admissible");

        assert!(
            repo.ensure_enrolled(&ns).is_err(),
            "a linked device holds this namespace's replica state; enrolling over it \
             has to be refused rather than silently strand that state"
        );
        assert_eq!(
            repo.get(&ns).expect("read").expect("present").device(),
            paired.device(),
            "and the refusal must leave the stored row untouched"
        );
    }

    #[test]
    fn an_unlinked_row_for_another_account_yields_to_this_nodes_own_enrolment() {
        // The device row is minted BEFORE anyone certifies it, so a single
        // `pair-init` with a mistyped nonce — or one issued by anyone who can reach
        // this node's admin API — claims the namespace's only device slot for an
        // account nobody here controls. An unlinked device holds no replica state,
        // so nothing is stranded by replacing it, and refusing to would leave the
        // node unable to enroll here again short of leaving the namespace.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let squatter = AccountGenesis::new(root(1), [0xABu8; 16]);
        let squatted = repo.ensure_enrolled_into(&ns, squatter).expect("adopt");
        assert_eq!(squatted.account, squatter.account_id());

        let mine = repo.ensure_enrolled(&ns).expect("enroll");
        assert_eq!(
            mine.account,
            repo.account_root()
                .expect("read")
                .expect("present")
                .account_for(&ns),
            "the node must end up in the account its OWN root owns — a certificate \
             signed for anything else verifies against a key that never signed it"
        );
        assert_ne!(
            mine.device(),
            squatted.device(),
            "and under a fresh device id, since the squatted one addresses another \
             account"
        );
    }

    #[test]
    fn a_revoked_device_is_reminted_rather_than_handed_back() {
        // Revocation is terminal: the id is spent for good. So the tombstone has to
        // release this node's device slot too, or `account create` afterwards keeps
        // certifying an id every peer refuses — the node is locked out of the
        // namespace by its own revocation, with nothing in the logs to say why.
        // "Re-enrolling mints a fresh one" is only true if something mints it.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let spent = repo.ensure_enrolled(&ns).expect("enroll");
        AccountBindingRepository::new(&store)
            .apply_revocation(&ns, spent.device())
            .expect("tombstone the device");

        let fresh = repo.ensure_enrolled(&ns).expect("re-enroll");
        assert_ne!(
            fresh.device(),
            spent.device(),
            "a spent device id can never be linked again, so handing it back leaves \
             the node permanently unable to enroll"
        );
        assert_eq!(
            fresh.account, spent.account,
            "but the account is unchanged: it is derived from the root, which the \
             revocation did not touch"
        );
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

        let enrolled = repo.ensure_enrolled(&ns).expect("enroll");
        let reloaded = repo.get(&ns).expect("read").expect("present");

        assert_eq!(enrolled.account, reloaded.account);
        assert_eq!(enrolled.genesis, reloaded.genesis);
        assert_eq!(
            reloaded.genesis.account_id(),
            reloaded.account,
            "the reconstructed genesis must address the account it claims"
        );
        // Rooted at the node's ACCOUNT ROOT, not its namespace identity — which is
        // the whole point: the namespace identity dies with the node, and this key
        // is what certifies a replacement device afterwards.
        assert_eq!(
            reloaded.genesis.root_sign_pk,
            repo.account_root()
                .expect("read")
                .expect("present")
                .public_key(),
        );
        // And it is recomputable from the root alone, with no row at all — the row
        // is a read cache, not the source of truth.
        assert_eq!(
            repo.account_root()
                .expect("read")
                .expect("present")
                .account_for(&ns),
            reloaded.account,
            "a node holding only the root must name the same account"
        );
    }

    #[test]
    fn the_account_root_is_generated_once_and_survives_reads() {
        // Replacing a root that has already certified devices is unrecoverable —
        // there is no second copy — so generate-once is the whole contract.
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);

        assert!(repo.account_root().expect("read").is_none());

        let first = repo.ensure_account_root().expect("generate");
        let second = repo.ensure_account_root().expect("generate");
        assert_eq!(first.public_key(), second.public_key());
        assert_eq!(
            repo.account_root()
                .expect("read")
                .expect("present")
                .public_key(),
            first.public_key()
        );
    }

    #[test]
    fn one_root_yields_a_distinct_account_per_namespace() {
        // The property the whole recovery model rests on: one key to back up, one
        // account per namespace, and no way to correlate them without the secret.
        let store = test_store();
        let root = NodeDeviceRepository::new(&store)
            .ensure_account_root()
            .expect("generate");

        let ns_a = ContextGroupId::from([0xAAu8; 32]);
        let ns_b = ContextGroupId::from([0xBBu8; 32]);

        assert_ne!(
            root.account_for(&ns_a),
            root.account_for(&ns_b),
            "the same root must not present the same account id in two namespaces"
        );
        assert_eq!(
            root.genesis_for(&ns_a).root_sign_pk,
            root.genesis_for(&ns_b).root_sign_pk,
            "but both genesis records name the same root, which is what makes one \
             backed-up key able to recover either"
        );
        // Recomputable from the root alone — nothing per-namespace to lose.
        assert_eq!(
            root.account_for(&ns_a),
            root.genesis_for(&ns_a).account_id()
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
            .ensure_enrolled(&ns)
            .expect("enroll");
        assert_ne!(
            paired.device(),
            node_a.device(),
            "one account, two devices — the replica ids must differ"
        );
    }

    #[test]
    fn an_adopted_account_does_not_displace_a_linked_enrollment() {
        // Same rule as `ensure_enrolled`, reached through the pairing entry point:
        // a linked device's replica state is real, so adopting another account over
        // it is refused rather than stranding that state. Both entry points have to
        // agree — the rule lives in one place precisely so they cannot drift.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let mine = repo.ensure_enrolled(&ns).expect("enroll");
        let cert = calimero_account::sign_device_cert(
            repo.account_root()
                .expect("read")
                .expect("present")
                .signing_key(),
            mine.account,
            mine.device(),
            &root(9),
            &mine.kem_public_key(),
            0,
            0,
        )
        .expect("sign the certificate");
        let _binding = AccountBindingRepository::new(&store)
            .apply_link(&ns, &mine.genesis, &[], &cert)
            .expect("store")
            .expect("the credential must be admissible");

        assert!(repo
            .ensure_enrolled_into(&ns, AccountGenesis::new(root(2), [0xCDu8; 16]))
            .is_err());
        let reloaded = repo.get(&ns).expect("read").expect("present");
        assert_eq!(reloaded.device(), mine.device());
        assert_eq!(reloaded.account, mine.account);
    }

    /// Link `device` of `genesis`'s account into `ns`, signed by `root_sk`.
    fn link(store: &Store, ns: &ContextGroupId, root_sk: &PrivateKey, nonce: [u8; 16]) -> DeviceId {
        let genesis = AccountGenesis::new(root_sk.public_key(), nonce);
        let device = DeviceId::mint(genesis.account_id(), nonce);
        let cert = calimero_account::sign_device_cert(
            root_sk,
            genesis.account_id(),
            device,
            &root(9),
            &KemPublicKey::from([9u8; 32]),
            0,
            0,
        )
        .expect("sign the certificate");
        let _binding = AccountBindingRepository::new(store)
            .apply_link(ns, &genesis, &[], &cert)
            .expect("store")
            .expect("the credential must be admissible");
        device
    }

    #[test]
    fn a_revocation_names_the_account_that_owns_the_device() {
        // The admin path. Deriving the account from this node's own root answers a
        // different question — "which account do I own here" — so an admin ejecting
        // somebody else's device published its OWN account id in the op and reported
        // that account back to the operator.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let mine = repo.ensure_enrolled(&ns).expect("enroll");
        let bob_sk = PrivateKey::from([7u8; 32]);
        let bob_device = link(&store, &ns, &bob_sk, [0x11u8; 16]);

        let target = repo
            .revocation_target(&ns, bob_device)
            .expect("resolve")
            .expect("the group knows this binding");

        assert_eq!(
            target.account,
            AccountGenesis::new(bob_sk.public_key(), [0x11u8; 16]).account_id(),
        );
        assert_ne!(
            target.account, mine.account,
            "the revocation must name the device's account, not the revoker's"
        );
        assert!(
            !target.self_service,
            "this node does not hold bob's root, so it can prove nothing about his \
             device and has only the admin path"
        );
    }

    #[test]
    fn an_admin_holding_no_account_root_can_still_resolve_a_revocation() {
        // Ejecting a device is an admin's job and needs no account of their own.
        // Requiring a root here refused the whole admin path on any node that had
        // never run `account create`.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);
        assert!(repo.account_root().expect("read").is_none());

        let device = link(&store, &ns, &PrivateKey::from([7u8; 32]), [0x11u8; 16]);

        let target = repo
            .revocation_target(&ns, device)
            .expect("resolve")
            .expect("the group knows this binding");
        assert!(!target.self_service);
        assert!(
            repo.account_root().expect("read").is_none(),
            "and resolving must not mint a root as a side effect — generating one \
             here would burn the singleton on a node that never asked for an account"
        );
    }

    #[test]
    fn revoking_this_nodes_own_device_is_self_service() {
        // The lost-laptop case: the owner may be the only person who knows, so it
        // must not need an admin.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let mine = repo.ensure_enrolled(&ns).expect("enroll");
        let cert = calimero_account::sign_device_cert(
            repo.account_root()
                .expect("read")
                .expect("present")
                .signing_key(),
            mine.account,
            mine.device(),
            &root(9),
            &mine.kem_public_key(),
            0,
            0,
        )
        .expect("sign the certificate");
        let _binding = AccountBindingRepository::new(&store)
            .apply_link(&ns, &mine.genesis, &[], &cert)
            .expect("store")
            .expect("the credential must be admissible");

        let target = repo
            .revocation_target(&ns, mine.device())
            .expect("resolve")
            .expect("the group knows this binding");
        assert_eq!(target.account, mine.account);
        assert!(target.self_service);
    }

    #[test]
    fn a_paired_device_cannot_claim_self_service_over_the_account_it_adopted() {
        // The stored row names the account this device speaks for, which belongs to
        // ANOTHER node's root. Reading ownership off that row would let this node
        // sign a revocation proof its root cannot back — a proof every peer refuses,
        // so the revocation records nothing and there is no admin fallback because
        // the node claimed it did not need one.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let alice_sk = PrivateKey::from([1u8; 32]);
        let alice = AccountGenesis::new(alice_sk.public_key(), [0xABu8; 16]);
        let paired = repo.ensure_enrolled_into(&ns, alice).expect("adopt");
        let cert = calimero_account::sign_device_cert(
            &alice_sk,
            paired.account,
            paired.device(),
            &root(9),
            &paired.kem_public_key(),
            0,
            0,
        )
        .expect("sign the certificate");
        let _binding = AccountBindingRepository::new(&store)
            .apply_link(&ns, &alice, &[], &cert)
            .expect("store")
            .expect("the credential must be admissible");

        // This node has a root of its own — it just is not alice's.
        let _own_root = repo.ensure_account_root().expect("generate");

        let target = repo
            .revocation_target(&ns, paired.device())
            .expect("resolve")
            .expect("the group knows this binding");
        assert_eq!(
            target.account, paired.account,
            "the account is still read from the binding"
        );
        assert!(
            !target.self_service,
            "but ownership is a fact about the root, and this node does not hold the \
             one that owns the account its own row names"
        );
    }

    #[test]
    fn a_device_this_namespace_never_linked_has_no_revocation_target() {
        let store = test_store();
        let ns = test_group_id();
        assert!(NodeDeviceRepository::new(&store)
            .revocation_target(&ns, DeviceId::from([0x33u8; 32]))
            .expect("resolve")
            .is_none());
    }

    #[test]
    fn each_namespace_gets_its_own_device_and_secret() {
        // Cross-namespace correlation guard: one machine must not present the
        // same replica id or the same agreement key in two namespaces.
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);

        let a = repo
            .ensure_enrolled(&ContextGroupId::from([0xAAu8; 32]))
            .expect("mint");
        let b = repo
            .ensure_enrolled(&ContextGroupId::from([0xBBu8; 32]))
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

        let minted = repo.ensure_enrolled(&ns).expect("mint");
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

        let observed: Vec<DeviceId> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let store = Arc::new(&store);
                    scope.spawn(move || {
                        NodeDeviceRepository::new(&store)
                            .ensure_enrolled(&ns)
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

        let _ = repo.ensure_enrolled(&ns).expect("mint");
        repo.delete(&ns).expect("delete");
        assert!(repo.get(&ns).expect("read").is_none());
        repo.delete(&ns).expect("delete again");
    }

    #[test]
    fn an_enrolled_device_is_certifiable_only_by_the_account_root() {
        // The two keys a node holds have one correct use each, and crossing them
        // is silent: the namespace identity signs ops, the account root signs
        // certificates. Since `ensure_enrolled` roots the account at the account
        // root, a certificate signed by the namespace identity is verified
        // against a key that never signed it — so the link is refused by every
        // peer while the enrollment looks locally fine.
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);
        let ns = test_group_id();

        let enrolled = repo.ensure_enrolled(&ns).expect("enroll");
        let namespace_identity = PrivateKey::random(&mut rand::thread_rng());

        let wrong = calimero_account::sign_device_cert(
            &namespace_identity,
            enrolled.account,
            enrolled.device(),
            &namespace_identity.public_key(),
            &enrolled.kem_public_key(),
            0,
            0,
        )
        .expect("sign");
        assert!(
            calimero_account::verify_device_cert(enrolled.account, &enrolled.genesis, &[], &wrong)
                .is_err(),
            "a cert signed by the namespace identity must not verify against the account root"
        );

        let right = calimero_account::sign_device_cert(
            repo.ensure_account_root().expect("root").signing_key(),
            enrolled.account,
            enrolled.device(),
            &namespace_identity.public_key(),
            &enrolled.kem_public_key(),
            0,
            0,
        )
        .expect("sign");
        assert!(
            calimero_account::verify_device_cert(enrolled.account, &enrolled.genesis, &[], &right)
                .is_ok(),
            "a cert signed by the account root must verify"
        );
    }

    #[test]
    fn a_node_with_no_enrollment_lists_no_namespaces() {
        let store = test_store();
        assert!(NodeDeviceRepository::new(&store)
            .enrolled_namespaces()
            .expect("scan")
            .is_empty());
    }

    #[test]
    fn every_enrolled_namespace_is_listed_whether_or_not_this_node_is_a_member() {
        // The reason this scan exists. A paired device is a device of someone
        // else's account and a member of nothing, so the membership-derived
        // listings the startup subscription rehydration walks all skip it. This
        // row family is written by enrollment, so it sees both cases alike.
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);

        let mine = ContextGroupId::from([0xAAu8; 32]);
        let paired = ContextGroupId::from([0xBBu8; 32]);
        let _ = repo.ensure_enrolled(&mine).expect("mint");
        let _ = repo
            .ensure_enrolled_into(&paired, AccountGenesis::new(root(9), [0x11u8; 16]))
            .expect("adopt");

        let mut listed = repo.enrolled_namespaces().expect("scan");
        listed.sort_unstable();
        assert_eq!(listed, vec![mine, paired]);
    }

    #[test]
    fn a_deleted_enrollment_leaves_the_listing() {
        // Namespace teardown drops the device row, and the subscription must go
        // with it — a node that keeps re-subscribing to a namespace it left is
        // both noise and a capability it should no longer have.
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);

        let kept = ContextGroupId::from([0xAAu8; 32]);
        let dropped = ContextGroupId::from([0xBBu8; 32]);
        let _ = repo.ensure_enrolled(&kept).expect("mint");
        let _ = repo.ensure_enrolled(&dropped).expect("mint");
        repo.delete(&dropped).expect("delete");

        assert_eq!(repo.enrolled_namespaces().expect("scan"), vec![kept]);
    }

    #[test]
    fn the_scan_stops_at_the_neighbouring_row_families() {
        // The device rows sit at prefix 0x44, between the per-group account keys
        // (0x43) and this node's account root (0x45) in the same column. A scan
        // that failed to bound itself would read a neighbour's bytes as a
        // namespace id and hand the node a subscription to a topic that does not
        // exist — so bracket the range from both sides and pin the upper edge
        // with the largest possible namespace id, which sorts immediately before
        // the account root.
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);

        let highest = ContextGroupId::from([0xFFu8; 32]);
        let _ = repo.ensure_enrolled(&highest).expect("mint");
        // `ensure_enrolled` already wrote the 0x45 root above; add a 0x43
        // neighbour below the range.
        crate::AccountBindingRepository::new(&store)
            .absorb_genesis(
                &ContextGroupId::from([0x01u8; 32]),
                &AccountGenesis::new(root(3), [0x22u8; 16]),
            )
            .expect("absorb");

        assert_eq!(repo.enrolled_namespaces().expect("scan"), vec![highest]);
    }
}
