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
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    NodeAccountRoot, NodeAccountRootValue, NodeDeviceIdentity, NodeDeviceIdentityValue,
};
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use eyre::Result as EyreResult;
use rand::Rng as _;
use zeroize::Zeroizing;

use crate::{collect_keys_with_prefix, NamespaceRepository};

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

    /// This root's genesis.
    ///
    /// Content-addressed on the root key alone, so the account is recomputable
    /// from the root and nothing else — no stored salt to lose, and one account
    /// wherever this root speaks.
    #[must_use]
    pub fn genesis(&self) -> AccountGenesis {
        AccountGenesis::new(self.public_key())
    }

    /// The `AccountId` this root owns.
    #[must_use]
    pub fn account(&self) -> AccountId {
        self.genesis().account_id()
    }

    /// The root as a 24-word BIP-39 mnemonic — the backup an operator writes down.
    ///
    /// BIP-39 rather than hex for one reason: it is **checksummed**. A mistyped hex
    /// root is still a valid key, so recovery would succeed and silently produce a
    /// different `AccountId` — the operator would learn about it when the account
    /// they recovered turns out to be one nobody has ever heard of. A mistyped word
    /// fails the checksum at import instead. That it is also the format every
    /// hardware wallet and paper-backup habit already uses is a bonus.
    ///
    /// The secret is 32 bytes, which is exactly BIP-39's 256-bit entropy case, so
    /// this is a straight encoding of the key — no derivation, no passphrase, no
    /// BIP-32 tree. Recovering the words recovers the key itself.
    ///
    /// # Errors
    /// Only if the 32-byte secret is somehow rejected as entropy, which cannot
    /// happen for a fixed-size array — the `Result` exists to avoid a panic path in
    /// a function handling key material.
    pub fn to_mnemonic(&self) -> EyreResult<Zeroizing<String>> {
        let mnemonic = bip39::Mnemonic::from_entropy(self.secret.as_bytes())
            .map_err(|e| eyre::eyre!("failed to encode the account root as a mnemonic: {e}"))?;
        Ok(Zeroizing::new(mnemonic.to_string()))
    }

    /// Recover a root from the words [`to_mnemonic`](Self::to_mnemonic) produced.
    ///
    /// Whitespace between words is normalised, so an operator can retype a backup
    /// across lines without it mattering.
    ///
    /// # Errors
    /// If the phrase is not a valid 24-word BIP-39 mnemonic (bad word, bad
    /// checksum, wrong length) — which is the point of using one.
    pub fn from_mnemonic(phrase: &str) -> EyreResult<Self> {
        // Wiped on drop like every other copy of the words in this file: joining
        // the whitespace-split parts allocates a fresh String holding the whole
        // phrase, and a plain one would sit in freed heap until something reused
        // the pages.
        let normalised: Zeroizing<String> =
            Zeroizing::new(phrase.split_whitespace().collect::<Vec<_>>().join(" "));
        let mnemonic = bip39::Mnemonic::parse_normalized(&normalised).map_err(|e| {
            eyre::eyre!("not a valid BIP-39 mnemonic (check the words and their order): {e}")
        })?;
        let (entropy, len) = mnemonic.to_entropy_array();
        let bytes: [u8; 32] = entropy
            .get(..len)
            .and_then(|slice| <[u8; 32]>::try_from(slice).ok())
            .ok_or_else(|| {
                eyre::eyre!(
                    "expected a 24-word mnemonic (256 bits of entropy), got {len} bytes' worth"
                )
            })?;
        Ok(Self {
            secret: PrivateKey::from(bytes),
        })
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
#[derive(Clone)]
#[non_exhaustive]
pub struct DeviceSecret {
    /// The replica this node speaks as.
    pub device: DeviceId,
    /// The agreement secret matching the certificate's `kem_pk`.
    pub kem_secret: X25519SecretKey,
}

impl std::fmt::Debug for DeviceSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never derived, for the same reason as `AccountRoot`. `kem_secret` is the
        // only thing that opens scope keys addressed to this device, so one
        // `tracing::debug!(?secret, ..)` added later would hand every rotation this
        // device receives to whoever can read the logs. `NodeDevice` derives its
        // `Debug` and reaches this field through here, so redacting once covers both.
        f.debug_struct("DeviceSecret")
            .field("device", &self.device)
            .field("kem_secret", &"[redacted]")
            .finish()
    }
}

/// What importing an account root did.
///
/// Returned rather than a bare `Option<AccountRoot>` because a forced import has
/// consequences beyond the root row, and an operator who cannot see them cannot
/// act on them: the devices this node held under the replaced root are gone, and
/// any it holds under somebody else's are not.
///
/// Not `Clone`, because [`AccountRoot`] is not: the fewer copies of a root secret
/// exist, the fewer there are to wipe.
#[derive(Debug)]
#[non_exhaustive]
pub struct ImportedRoot {
    /// The root that was replaced, if there was one. `None` on a fresh store,
    /// which is the ordinary recovery case and needs no `--force`.
    pub replaced: Option<AccountRoot>,
    /// Namespaces whose device row was dropped because it belonged to
    /// [`Self::replaced`]. Re-enrolling in each mints a fresh device under the
    /// imported root.
    pub released: Vec<ContextGroupId>,
    /// Namespaces whose device row was kept because it names an account this
    /// root never owned — a device paired into somebody else's account, which is
    /// unaffected by replacing this node's own root.
    pub retained: Vec<ContextGroupId>,
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

/// The account this node executes as inside `context` — what the guest reads as
/// `env::account_id()`.
///
/// **Always a real account, never a stand-in for the executing key.** The account
/// id is derived from this node's root plus a scope id and needs no ops at all, so
/// there is no state in which this node has no account to name: one that never ran
/// `account create` simply owns an account no peer has heard of yet. The
/// alternative — falling back to the identity key when nothing is enrolled — would
/// hand apps a device-shaped value through the account door and make
/// `Map<account_id, Vote>` silently one-vote-per-device again, which is the exact
/// failure this split exists to end.
///
/// The scope is the context's **namespace**, so all of a person's contexts in one
/// namespace agree on who they are, and their accounts in two namespaces stay
/// uncorrelatable. A context with no owning group has no namespace to resolve, so
/// it degenerates to being its own scope: still a real derived account, just one
/// scoped to that context.
///
/// # Errors
/// Propagates the store read or the account-root generation failure.
pub fn account_for_context(store: &Store, context_id: &ContextId) -> EyreResult<AccountId> {
    let scope = match crate::get_group_for_context(store, context_id)? {
        Some(group) => group,
        None => ContextGroupId::from(*context_id.as_ref()),
    };
    account_for_group(store, &scope)
}

/// The account this node executes as inside `group` — the same answer
/// [`account_for_context`] gives, resolved from the group directly.
///
/// **Use this wherever the group is known but the context→group row may not be
/// written yet.** `account_for_context` reads that row to find the namespace, and
/// falls back to scoping the account to the context itself when it is missing. For
/// a context whose row lands *later* — creation being exactly that case — the two
/// calls therefore return DIFFERENT accounts: `init` seeds a writer set under the
/// context-scoped account, every later call presents the namespace-scoped one, and
/// the creator is refused write access to the object it just created. The fallback
/// is correct only for a context that has no group at all, never for one whose row
/// has not been written yet.
///
/// # Errors
/// Propagates the namespace resolution or account-root generation failure.
pub fn account_for_group(store: &Store, group: &ContextGroupId) -> EyreResult<AccountId> {
    let namespace = NamespaceRepository::new(store).resolve(group)?;
    // The key this node signs with in that namespace. Its binding — if this node
    // has enrolled — names the real account; otherwise the key writes as its own
    // stand-in, which is the only value a PEER can derive for it.
    let (_, sign_pk, ..) = NamespaceRepository::new(store).get_or_create_identity(&namespace)?;
    let binding = crate::AccountBindingRepository::new(store)
        .binding_for_sign_pk(&namespace, &sign_pk)?
        .map(|binding| binding.account);
    Ok(calimero_op_adapter::writer_account(binding, &sign_pk))
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

    /// Import `root` as this node's account root, returning the one it replaced.
    ///
    /// The restore half of [`AccountRoot::to_mnemonic`]. Refuses to replace an
    /// existing root unless `force`, **in the repository rather than in the
    /// caller**: overwriting a root that has already certified devices is
    /// unrecoverable — there is no second copy, and every account it owned is
    /// stranded — so the check belongs where it cannot be skipped by forgetting to
    /// make it. An earlier version left it to `merod account import` and said
    /// "nothing else should call this", which is a convention, not a boundary.
    ///
    /// Reports what a forced replacement destroyed, so a caller cannot do it
    /// without being handed the consequences.
    ///
    /// # Errors
    /// If a root exists and `force` is false, or the store read/write fails.
    pub fn try_import_account_root(
        &self,
        root: &AccountRoot,
        force: bool,
    ) -> EyreResult<ImportedRoot> {
        let _guard = ACCOUNT_ROOT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let existing = self.account_root()?;
        if let Some(previous) = &existing {
            if !force {
                eyre::bail!(
                    "this node already has an account root ({}), and replacing it \
                     cannot be undone: a root that has already certified devices \
                     has no second copy",
                    previous.public_key()
                );
            }
        }

        // Re-importing the SAME root is a no-op, and must be treated as one. It
        // reaches here whenever an operator re-runs a restore, or passes `--force`
        // defensively — and because every device row would then match
        // `previous.account()`, the cleanup below would delete every
        // device this node holds while nothing about the root changed. Destroying
        // live enrolments to reinstall the key they already depend on is the
        // opposite of what the caller asked for.
        let same_root = existing
            .as_ref()
            .is_some_and(|previous| previous.public_key() == root.public_key());

        // A forced replacement invalidates every device the discarded root
        // certified, and the rows have to go with it.
        //
        // A device row is keyed by namespace alone, and
        // [`stored_identity_still_serves`](Self::stored_identity_still_serves)
        // refuses to replace a *linked* row that names a different account — right,
        // because that row holds the namespace's replica state. But after a forced
        // import every such row names an account derived from the key just
        // discarded, so enrolment under the new root was refused with "revoke the
        // existing device first": advice the operator cannot take, since revoking
        // needs the root they replaced. Recovering onto a machine that had not been
        // wiped locked it out of the namespaces it recovered the root *for*.
        //
        // Only the rows this root owned. A paired row names an account belonging to
        // another node's root; replacing this one says nothing about it, and
        // dropping it would strand a device that still opens scope keys wrapped for
        // it. Ownership is decided the only way it can be — by re-deriving the
        // account from the root being discarded.
        //
        // **Deliberately not holding `NODE_DEVICE_MINT_LOCK` here.**
        // `ensure_enrolled` takes that lock and then calls `ensure_account_root`,
        // which takes this one, so acquiring them in the opposite order is an ABBA
        // deadlock. Serializing against a concurrent enrolment is not worth it
        // anyway: this runs from a CLI that opens the datastore directly, which
        // requires the node to be stopped, so there is nothing to race.
        let mut released = Vec::new();
        let mut retained = Vec::new();
        let mut doomed = Vec::new();
        if let Some(previous) = &existing {
            if !same_root {
                for namespace in self.enrolled_namespaces()? {
                    let Some(row) = self.get(&namespace)? else {
                        continue;
                    };
                    if row.account == previous.account() {
                        doomed.push(namespace);
                    } else {
                        retained.push(namespace);
                    }
                }
            }
        }

        // One batch, so the new root and the removal of the rows it invalidates
        // either both land or neither does. Split across two writes there is a
        // window where a crash leaves the NEW root beside the OLD root's device
        // rows — precisely the state that makes `ensure_enrolled` refuse, and the
        // one this cleanup exists to prevent. It would be silently persistent
        // rather than transient, because nothing re-runs the cleanup afterwards.
        // Keys are declared before the transaction on purpose: `Transaction<'a>`
        // borrows them, so anything it references has to outlive it.
        let root_key = NodeAccountRoot::new();
        let doomed_keys: Vec<_> = doomed
            .iter()
            .map(|namespace| NodeDeviceIdentity::new(namespace.to_bytes()))
            .collect();
        let root_bytes: Slice<'_> = borsh::to_vec(&NodeAccountRootValue {
            root_secret: *root.signing_key().as_bytes(),
        })?
        .into();

        let mut tx = Transaction::default();
        tx.put(&root_key, root_bytes);
        for key in &doomed_keys {
            tx.delete(key);
        }
        self.store.apply(&tx)?;
        released.extend(doomed);

        Ok(ImportedRoot {
            replaced: existing,
            released,
            retained,
        })
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
    /// The device this node can still be addressed by in `namespace`, if any.
    ///
    /// For callers that need *a* device to be reachable at rather than one of
    /// their own account: a paired device speaks for somebody else's account and
    /// is perfectly usable, so reaching for [`Self::ensure_enrolled`] instead
    /// would release the slot and mint a replacement — discarding the device any
    /// key already in flight is addressed to.
    ///
    /// `None` means "nothing usable here, mint one": either no device, or one
    /// this node has revoked. A revoked id is spent for good, so asking as it
    /// asks for keys the revocation exists to withhold.
    ///
    /// A revocation read that FAILS reports the device as usable rather than
    /// hiding it. Re-minting on a transient store error destroys a paired device
    /// permanently, while asking as a revoked one costs nothing — the responder
    /// checks revocation before serving a key, so that is the enforcement and
    /// this is only politeness.
    pub fn reusable_device(&self, namespace: &ContextGroupId) -> EyreResult<Option<NodeDevice>> {
        let Some(held) = self.get(namespace)? else {
            return Ok(None);
        };
        let revoked = crate::AccountBindingRepository::new(self.store)
            .is_revoked(namespace, held.device())
            .unwrap_or(false);
        Ok((!revoked).then_some(held))
    }

    pub fn get(&self, namespace: &ContextGroupId) -> EyreResult<Option<NodeDevice>> {
        let key = NodeDeviceIdentity::new(namespace.to_bytes());
        Ok(self
            .store
            .handle()
            .get(&key)?
            .map(|value: NodeDeviceIdentityValue| {
                let genesis = AccountGenesis::new(PublicKey::from(value.account_root_pk));
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
        let genesis = self.ensure_account_root()?.genesis();
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
            .is_some_and(|root| root.account() == binding);

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
        let alice = AccountGenesis::new(alice_sk.public_key());
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

        let squatter = AccountGenesis::new(root(1));
        let squatted = repo.ensure_enrolled_into(&ns, squatter).expect("adopt");
        assert_eq!(squatted.account, squatter.account_id());

        let mine = repo.ensure_enrolled(&ns).expect("enroll");
        assert_eq!(
            mine.account,
            repo.account_root()
                .expect("read")
                .expect("present")
                .account(),
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

    /// The three answers `reusable_device` owes a caller that needs to be
    /// addressable rather than to be itself. Each was a live bug in the sync
    /// path's key-recovery pull before the rule had a name to call.
    #[test]
    fn a_paired_device_is_reusable_and_is_not_re_minted_over() {
        // The bug this pins: the pull called `ensure_enrolled`, which releases a
        // row belonging to another account and mints a replacement. A paired
        // device IS such a row, so the first pull after pairing destroyed it —
        // and the scope key already in flight named the device it destroyed.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let paired = repo
            .ensure_enrolled_into(&ns, AccountGenesis::new(root(1)))
            .expect("adopt somebody else's account");

        let reusable = repo
            .reusable_device(&ns)
            .expect("read")
            .expect("a paired device is still a device this node can be addressed at");
        assert_eq!(
            reusable.device(),
            paired.device(),
            "reusing means the SAME id — a different one is the destruction this avoids"
        );
        assert_eq!(
            repo.get(&ns).expect("read").expect("present").device(),
            paired.device(),
            "and reading must not have mutated the slot"
        );
    }

    #[test]
    fn a_revoked_device_is_not_reusable() {
        // The mirror bug, introduced while fixing the one above: reading the held
        // device before minting skipped the revocation check, so a node came back
        // as the device it had just revoked. `None` here is what sends the caller
        // to `ensure_enrolled`, which releases the spent row and mints.
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let spent = repo.ensure_enrolled(&ns).expect("enroll");
        AccountBindingRepository::new(&store)
            .apply_revocation(&ns, spent.device())
            .expect("tombstone the device");

        assert!(
            repo.reusable_device(&ns).expect("read").is_none(),
            "a spent id must not be handed back — asking as it asks for the keys the \
             revocation withheld"
        );
    }

    #[test]
    fn a_namespace_with_no_device_has_nothing_to_reuse() {
        let store = test_store();
        let ns = test_group_id();
        assert!(
            NodeDeviceRepository::new(&store)
                .reusable_device(&ns)
                .expect("read")
                .is_none(),
            "nothing held means nothing to reuse, which is the caller's cue to mint"
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
                .account(),
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
    fn one_root_yields_one_account_everywhere() {
        // The property the recovery model rests on: one key to back up, and an
        // account recomputable from it alone — no per-scope salt to lose, and the
        // same answer in every namespace this root speaks in.
        let store = test_store();
        let root = NodeDeviceRepository::new(&store)
            .ensure_account_root()
            .expect("generate");

        assert_eq!(
            root.account(),
            root.genesis().account_id(),
            "the account must be exactly the content address of the genesis"
        );
        assert_eq!(
            root.genesis().root_sign_pk,
            root.public_key(),
            "and the genesis must name the root that backs it up, or a recovered \
             mnemonic names an account nobody has heard of"
        );
    }

    /// **Replacing a root is refused by the REPOSITORY, not by whoever calls it.**
    ///
    /// The check used to live in `merod account import`, with a doc comment
    /// saying nothing else should call the raw setter. That is a convention, not
    /// a boundary — and the same "precondition enforced in a different layer than
    /// the invariant depending on it" shape this codebase has been burned by
    /// before. Any future caller (meroctl, an RPC handler, a test helper) would
    /// have silently destroyed an unrecoverable key.
    #[test]
    fn importing_over_an_existing_root_is_refused_unless_forced() {
        let store = test_store();
        let repo = NodeDeviceRepository::new(&store);
        let original = repo.ensure_account_root().expect("generate");
        let original_pk = original.public_key();

        let incoming = AccountRoot::from_mnemonic(
            &NodeDeviceRepository::new(&test_store())
                .ensure_account_root()
                .expect("generate")
                .to_mnemonic()
                .expect("export"),
        )
        .expect("import");

        let refused = repo.try_import_account_root(&incoming, false);
        assert!(
            refused.is_err(),
            "an unforced import over an existing root must be refused"
        );
        assert_eq!(
            repo.account_root()
                .expect("read")
                .expect("present")
                .public_key(),
            original_pk,
            "and the refusal must leave the original root in place — a partial \
             overwrite here strands every account it owned"
        );

        // Forced, it goes through AND hands back what it destroyed, so a caller
        // cannot lose track of the fact that a replacement happened.
        let replaced = repo
            .try_import_account_root(&incoming, true)
            .expect("forced import");
        assert_eq!(
            replaced.replaced.map(|r| r.public_key()),
            Some(original_pk),
            "the replaced root is returned, not silently dropped"
        );
        assert_eq!(
            repo.account_root()
                .expect("read")
                .expect("present")
                .public_key(),
            incoming.public_key()
        );
    }

    /// **The backup is the phrase and nothing else.**
    ///
    /// The account is the content address of the root key, so a recovered root
    /// names the account it always named — with no stored salt, and no list of
    /// namespaces to keep beside the words.
    #[test]
    fn an_exported_root_recovers_the_same_account() {
        let original_store = test_store();
        let original = NodeDeviceRepository::new(&original_store)
            .ensure_account_root()
            .expect("generate");
        let before = original.account();
        let backup = original.to_mnemonic().expect("export");
        assert_eq!(
            backup.split_whitespace().count(),
            24,
            "a 32-byte root is BIP-39's 256-bit case, and the word count is what an \
             operator checks before trusting a transcription"
        );

        // The disk is gone. A fresh store shares nothing with the old one.
        drop(original);
        let recovered_store = test_store();
        assert!(
            NodeDeviceRepository::new(&recovered_store)
                .account_root()
                .expect("read")
                .is_none(),
            "precondition: the new node has no root, so recovery cannot be reading \
             a leftover"
        );

        let recovered = AccountRoot::from_mnemonic(&backup).expect("import");
        assert_eq!(
            recovered.account(),
            before,
            "the recovered root must present the same account — this is the whole \
             recovery story, and nothing but the words crossed over"
        );
        assert_eq!(
            recovered.public_key(),
            original_public(&original_store),
            "and it is the same key, not merely one that agrees on the id"
        );
    }

    /// Read the stored root's public half, so the assertion above compares keys
    /// rather than trusting three derived ids to imply key equality.
    fn original_public(store: &Store) -> calimero_primitives::identity::PublicKey {
        NodeDeviceRepository::new(store)
            .account_root()
            .expect("read")
            .expect("present")
            .public_key()
    }

    /// **A mistyped backup is refused, not silently recovered into a stranger.**
    ///
    /// The reason this is BIP-39 and not hex. Every 32-byte string is a valid
    /// key, so a hex backup with one wrong character recovers *a* root — a
    /// different one — and the operator finds out when the account they restored
    /// turns out to be an account nobody has heard of, with no grants and no
    /// history. The checksum turns that into an error at import.
    /// A forced import is the "recover onto a machine that is already running"
    /// case, and it used to leave the node unable to use the root it just
    /// recovered.
    ///
    /// The device row for a namespace is keyed by namespace alone, and
    /// `stored_identity_still_serves` refuses to replace a **linked** row naming a
    /// different account — correctly, since that row holds the namespace's replica
    /// state. After a forced import every such row names an account derived from
    /// the *discarded* root, so enrolment under the new one was refused with
    /// "revoke the existing device first" — advice the operator cannot take,
    /// because revoking needs the root they just replaced. A lockout, reachable by
    /// following the documented recovery procedure on a node that had not been
    /// wiped.
    #[test]
    fn a_forced_import_releases_the_replaced_roots_device_slots() {
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        // Enrol under this node's own root, and LINK it: an unlinked row yields
        // anyway, so only a linked one exercises the refusal.
        let discarded = repo.ensure_account_root().expect("generate");
        let mine = repo.ensure_enrolled(&ns).expect("enroll");
        let cert = calimero_account::sign_device_cert(
            discarded.signing_key(),
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
        assert!(
            AccountBindingRepository::new(&store)
                .is_device_linked(&ns, mine.device())
                .expect("read"),
            "the row has to be linked for this test to mean anything"
        );

        let incoming = AccountRoot::from_mnemonic(
            &NodeDeviceRepository::new(&test_store())
                .ensure_account_root()
                .expect("generate")
                .to_mnemonic()
                .expect("export"),
        )
        .expect("import");

        let replaced = repo
            .try_import_account_root(&incoming, true)
            .expect("a forced import must succeed");
        assert_eq!(
            replaced
                .replaced
                .as_ref()
                .expect("must report the root it replaced")
                .public_key(),
            discarded.public_key()
        );
        assert_eq!(
            replaced.released,
            vec![ns],
            "the namespace whose device belonged to the discarded root must be released"
        );
        assert!(replaced.retained.is_empty());

        // The whole point: the recovered root has to be usable here.
        let after = repo
            .ensure_enrolled(&ns)
            .expect("enrolling under the freshly imported root must not be refused");
        assert_eq!(
            after.account,
            incoming.account(),
            "the new device must speak for the imported root's account"
        );
        assert_ne!(
            after.device(),
            mine.device(),
            "and it must be a fresh replica id, not the one bound to the old account"
        );
    }

    /// Re-importing the root that is already installed must change nothing.
    ///
    /// The regression this pins was introduced by the fix above it: the cleanup
    /// deletes rows whose account re-derives from the root being replaced, and when
    /// the "replaced" root IS the incoming one, that matches every row this node
    /// holds. So a defensive `--force` on a re-run — or simply restoring the same
    /// backup twice — destroyed every live enrolment in order to reinstall the key
    /// those enrolments already depend on.
    #[test]
    fn re_importing_the_same_root_keeps_every_device() {
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let root = repo.ensure_account_root().expect("generate");
        let mine = repo.ensure_enrolled(&ns).expect("enroll");

        // Round-tripped through the mnemonic, because that is how an operator
        // re-supplies it: same key, different `AccountRoot` value.
        let same = AccountRoot::from_mnemonic(&root.to_mnemonic().expect("export"))
            .expect("import the same root");

        let outcome = repo
            .try_import_account_root(&same, true)
            .expect("a forced re-import of the same root must succeed");

        assert!(
            outcome.released.is_empty(),
            "re-importing the same root released {:?} — nothing changed, so nothing \
             may be destroyed",
            outcome.released
        );
        assert_eq!(
            repo.get(&ns).expect("read").expect("present").device(),
            mine.device(),
            "the device must survive untouched"
        );
    }

    /// The other half, and the reason the slots cannot just be cleared wholesale: a
    /// **paired** row names an account belonging to somebody else's root. Replacing
    /// this node's root says nothing about it, and dropping it would strand a
    /// working device — the node would stop being able to open scope keys wrapped
    /// for it.
    #[test]
    fn a_forced_import_keeps_device_rows_belonging_to_another_root() {
        let store = test_store();
        let ns = test_group_id();
        let repo = NodeDeviceRepository::new(&store);

        let _discarded = repo.ensure_account_root().expect("generate");

        // Adopted into an account this node's root does not own, as pairing does.
        let elsewhere = AccountGenesis::new(root(3));
        let paired = repo
            .ensure_enrolled_into(&ns, elsewhere)
            .expect("adopt the foreign account");

        let incoming = AccountRoot::from_mnemonic(
            &NodeDeviceRepository::new(&test_store())
                .ensure_account_root()
                .expect("generate")
                .to_mnemonic()
                .expect("export"),
        )
        .expect("import");
        let _replaced = repo
            .try_import_account_root(&incoming, true)
            .expect("a forced import must succeed");

        let still_there = repo
            .get(&ns)
            .expect("read")
            .expect("a row for another root's account must survive the import");
        assert_eq!(
            still_there.device(),
            paired.device(),
            "a paired device is not this root's to discard"
        );
    }

    #[test]
    fn a_corrupted_backup_fails_the_checksum_instead_of_recovering_a_stranger() {
        let store = test_store();
        let root = NodeDeviceRepository::new(&store)
            .ensure_account_root()
            .expect("generate");
        let backup = root.to_mnemonic().expect("export");

        let mut words: Vec<&str> = backup.split_whitespace().collect();
        // Swap the first word for another valid BIP-39 word: still a real word, so
        // only the checksum can catch it.
        let replacement = if words[0] == "zoo" { "abandon" } else { "zoo" };
        words[0] = replacement;
        let corrupted = words.join(" ");

        assert!(
            AccountRoot::from_mnemonic(&corrupted).is_err(),
            "a single wrong word must fail the checksum — recovering a different \
             root here would be indistinguishable from a successful recovery until \
             far too late"
        );
    }

    /// Whitespace is normalised, because a backup on paper gets retyped across
    /// lines and an operator should not lose an account to a line break.
    #[test]
    fn a_retyped_backup_survives_ragged_whitespace() {
        let store = test_store();
        let root = NodeDeviceRepository::new(&store)
            .ensure_account_root()
            .expect("generate");
        let backup = root.to_mnemonic().expect("export");
        let ragged = backup
            .split_whitespace()
            .collect::<Vec<_>>()
            .chunks(4)
            .map(|line| line.join("  "))
            .collect::<Vec<_>>()
            .join("\n  ");

        assert_eq!(
            AccountRoot::from_mnemonic(&ragged)
                .expect("import")
                .public_key(),
            root.public_key()
        );
    }

    #[test]
    fn the_creation_time_resolver_trap_is_gone() {
        // This used to be the trap that made `account_for_group` necessary.
        // `account_for_context` finds the namespace through the context→group row
        // and, when that row is absent, falls back to scoping the account to the
        // CONTEXT. During creation the row lands after `init`, so the two resolvers
        // answered differently for the same context: `init` seeded a writer set
        // under the context-scoped account, every later call presented the
        // namespace-scoped one, and the creator was locked out of the object it had
        // just created.
        //
        // An account is no longer scoped to anything, so "which scope did you
        // resolve against" has stopped being a question with two answers. The
        // fallback cannot name a stranger, and the ordering hazard cannot recur.
        // The previous test asserted the two DISAGREED and said in its own comment
        // that a change making the fallback safe should replace it deliberately —
        // this is that replacement.
        let store = test_store();
        let namespace = ContextGroupId::from([0x77u8; 32]);
        let context = ContextId::from([0x99u8; 32]);

        // No context→group row written yet — exactly the state `init` runs in.
        let during_creation = account_for_context(&store, &context).expect("resolve by context");
        let from_the_group = account_for_group(&store, &namespace).expect("resolve by group");

        assert_eq!(
            during_creation, from_the_group,
            "the fallback must name the same account as the group resolver, or a \
             creator can still be locked out of what it just created"
        );

        // And it keeps agreeing once the row lands, so nothing about the account
        // moved when the mapping appeared.
        crate::context_tree::ContextTreeService::new(&store, namespace)
            .register_context(&context)
            .expect("write the context→group row");
        assert_eq!(
            account_for_context(&store, &context).expect("resolve by context"),
            from_the_group,
            "and the answer does not change when the row appears"
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
        let alice = AccountGenesis::new(alice_root);

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
            .ensure_enrolled_into(&ns, AccountGenesis::new(root(2)))
            .is_err());
        let reloaded = repo.get(&ns).expect("read").expect("present");
        assert_eq!(reloaded.device(), mine.device());
        assert_eq!(reloaded.account, mine.account);
    }

    /// Link `device` of `genesis`'s account into `ns`, signed by `root_sk`.
    fn link(store: &Store, ns: &ContextGroupId, root_sk: &PrivateKey, nonce: [u8; 16]) -> DeviceId {
        let genesis = AccountGenesis::new(root_sk.public_key());
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
            AccountGenesis::new(bob_sk.public_key()).account_id(),
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
        let alice = AccountGenesis::new(alice_sk.public_key());
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
            .ensure_enrolled_into(&paired, AccountGenesis::new(root(9)))
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
                &AccountGenesis::new(root(3)),
            )
            .expect("absorb");

        assert_eq!(repo.enrolled_namespaces().expect("scan"), vec![highest]);
    }
}
