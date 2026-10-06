//! Materialized account state for a group: which devices speak for which
//! account, which have been withdrawn, and each account's current root key.
//!
//! This is the governance-path counterpart of the account plane in
//! `calimero-projection`. The rules are the same — they have to be, or two
//! nodes would disagree about who may author — but they are enforced against
//! materialized rows rather than a fold, because that is how the shipping
//! governance wire works.
//!
//! Four rules earn their place here, and each one is a bug that
//! `crates/projection/tests/account_plane.rs` caught before this existed:
//!
//! 1. **Revocation is its own row family, and it is terminal.** A revocation
//!    that applies *before* the link it withdraws must still win, so every link
//!    consults the tombstone. Were revocation a flag on the binding, a
//!    revoke-then-link arrival order would silently resurrect the device.
//! 2. **The account's genesis is absorbed even when the link is refused.** The
//!    genesis is self-certifying, so recording it is safe regardless; recording
//!    it only on success made link-then-revoke and revoke-then-link produce
//!    different state.
//! 3. **Supersession is decided on read, not on apply.** Mid-stream, "has this
//!    root key been rotated past" reads only the rotations seen so far, which
//!    makes admission depend on delivery order. The signing epoch is stored and
//!    the question is answered once the account's current epoch is known.
//! 4. **Replica-seed uniqueness is decided on read too**, and for the same
//!    reason. Rejecting a link because an already-stored device with the same
//!    HLC seed had a lower id is order-dependent in the direction it does not
//!    check: high-then-low left both devices live. The rule is a filter over the
//!    stored set instead.

use calimero_account::{
    verify_device_cert, AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupAccountEndorser, GroupAccountKey, GroupAccountKeyValue, GroupDeviceBinding,
    GroupDeviceBindingValue, GroupDeviceScopeFloor, GroupRevokedDevice, GroupRevokedSigner,
    GroupSignerAccount, GroupSignerDevice,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error as ThisError;

use crate::collect_keys_with_prefix;

/// Scope epoch stamped on a binding a join wrote: the join credential carries no
/// scope statement, so the floor is what lets any later scope supersede it.
pub const JOIN_SCOPE_EPOCH: u32 = 0;

/// Why a credential was not recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ThisError)]
#[non_exhaustive]
pub enum BindingRejected {
    /// The certificate is not internally valid (bad anchor, chain, signature).
    #[error("device certificate is not internally valid: {0}")]
    CredentialInvalid(calimero_account::AccountError),
    /// The device was revoked; the id is spent for good.
    #[error("device was revoked and its id cannot be reused")]
    DeviceRevoked,
    /// A device may not be moved between accounts.
    #[error("device is already bound to a different account")]
    AccountReassignment,
    /// The account narrowed this device out of the group at or after this scope.
    #[error("device was narrowed out of this group at scope epoch {floor}; the link's scope is {offered}")]
    ScopeNarrowed {
        /// Scope epoch the incoming link was made under.
        offered: u32,
        /// Scope epoch of the latest narrowing recorded here.
        floor: u32,
    },
    /// The link does not advance the device's rotation epoch, so it would only
    /// let a retired certificate be replayed.
    #[error("device link at epoch {offered} does not supersede the stored {stored}")]
    EpochNotAdvanced {
        /// Epoch the incoming link offers.
        offered: u32,
        /// Epoch already recorded.
        stored: u32,
    },
    /// A rotation for an account this group has never learned.
    ///
    /// Distinct from [`Self::RotationNotContinuous`], which the two used to
    /// share: they send whoever reads one somewhere different. This means the
    /// group holds no key chain for the account at all — nothing has linked a
    /// device of it here — whereas a non-contiguous rotation means the account
    /// IS known and the handoff merely starts at the wrong epoch. The first is a
    /// relay of a stranger's rotation and is expected; the second is a stale or
    /// forked handoff and is worth looking at.
    #[error("no key chain known for account {account} in this group")]
    RotationAccountUnknown {
        /// The account the handoff would roll.
        account: AccountId,
    },
    /// A rotation that does not continue the chain in force for this account.
    #[error("key rotation starts at epoch {found}, but epoch {expected} is in force")]
    RotationNotContinuous {
        /// The epoch currently established for the account.
        expected: u32,
        /// The epoch the handoff declares it rolls from.
        found: u32,
    },
    /// A rotation not signed by the outgoing root key.
    #[error("key rotation is not signed by the outgoing root key")]
    RotationSignatureInvalid,
    /// The credential's handoff chain is longer than
    /// [`calimero_account::MAX_ROOT_KEY_HANDOFFS`]. Refused before any of its
    /// signatures are verified.
    #[error("handoff chain has {found} entries, over the {limit} cap")]
    ChainTooLong {
        /// Length of the supplied chain.
        found: usize,
        /// The cap.
        limit: usize,
    },
    /// The endorsement is about a different account than the credential.
    #[error("endorsement names a different account than the certificate")]
    EndorsementAccountMismatch,
    /// The endorsement is not validly signed by the member key it names.
    #[error("endorsement is not validly signed by the member it names")]
    EndorsementInvalid,
    /// The account is not a member of this group, so its devices may not link
    /// themselves in. Raised by the apply handler, not by the repository —
    /// membership is the caller's question.
    #[error("account is not a member of this group")]
    AccountNotMember,
}

impl BindingRejected {
    /// Whether no later op could ever make this credential bind a device.
    ///
    /// The question every caller that records an endorser has to answer first.
    /// An endorser row makes its member account-addressed, and the scope-key
    /// fan-out then delivers to `devices_of(account)` — which stays empty
    /// forever if no device of that account can ever bind. One malformed
    /// certificate would cost that member every future scope key, with no
    /// recovery path, despite a perfectly good membership.
    ///
    /// The permanent rejections are exactly those decided by the credential's
    /// own bytes, so every replica reaches the same verdict whatever it has
    /// folded. The rest depend on rows that later ops can change (a revocation,
    /// a rotation, an epoch that has not been reached yet), and making the
    /// endorser conditional on one of those would let two arrival orders leave
    /// different endorser sets behind.
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        matches!(
            self,
            Self::CredentialInvalid(_) | Self::ChainTooLong { .. } | Self::RotationSignatureInvalid
        )
    }
}

/// The account a **member key** speaks for in the namespace owning `group`.
///
/// The resolution the governance planes need when they stop naming keys and
/// start naming accounts. Distinct from a lookup by group in two
/// ways that both matter:
///
/// * **It resolves at the NAMESPACE, not at `group`.** Bindings are
///   namespace-keyed, so a member added to a subgroup was bound when it joined
///   the namespace — asking the subgroup would find nothing and refuse an
///   add that is perfectly legitimate.
/// * **It asks only whether the key is bound**, not whether some endorser is a
///   member of the decision group. Whether the account may be *added* here is
///   the caller's question; this one only answers *who* it is.
///
/// # `None` must fail closed
///
/// `None` means this namespace has never seen a credential for `member_key`, so
/// there is no account to name. Callers must refuse rather than fall back to a
/// key-derived stand-in: the stand-in is exactly the bridge the account plane
/// exists to remove, and a fallback would keep it alive at the one site where
/// it is hardest to notice — a grant that looks recorded and matches nothing.
///
/// Post-enrolment-at-join every member of a namespace is bound by construction,
/// so `None` is a real anomaly (an unenrolled straggler, or a key that never
/// joined) rather than an ordinary state to paper over.
///
/// # Errors
/// Propagates the namespace resolution or the binding scan.
pub fn member_account_in_namespace(
    store: &Store,
    group: &ContextGroupId,
    member_key: &PublicKey,
) -> EyreResult<Option<AccountId>> {
    let namespace = crate::NamespaceRepository::new(store).resolve(group)?;
    Ok(AccountBindingRepository::new(store)
        .binding_for_sign_pk(&namespace, member_key)?
        .map(|binding| binding.account))
}

/// The account `sign_pk` was ever certified for in the namespace owning `group`,
/// whether or not a live binding still speaks for it.
///
/// For judging state a key signed in the past, which
/// [`member_account_in_namespace`] cannot do: a revocation or a descope deletes
/// the live binding and a device-key rotation overwrites it, so a key that
/// legitimately signed an entry would resolve to nobody afterwards. Snapshot
/// apply needs exactly this, to check a leaf's signer against the account-keyed
/// owner or writer set it carries.
///
/// It is not an authorization: a revoked key still resolves here. Whether the key
/// may act *now* is [`member_account_in_namespace`]'s question.
///
/// `None` means no certificate for the key has verified here, either because the
/// key was never bound or because the link has not been folded yet.
///
/// # Errors
/// Propagates the namespace resolution or the store read.
pub fn signer_account_in_namespace(
    store: &Store,
    group: &ContextGroupId,
    sign_pk: &PublicKey,
) -> EyreResult<Option<AccountId>> {
    let namespace = crate::NamespaceRepository::new(store).resolve(group)?;
    AccountBindingRepository::new(store).signer_account(&namespace, sign_pk)
}

/// A device binding that is currently in force.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceBinding {
    /// The device this binding is for.
    pub device: DeviceId,
    /// The account it speaks for.
    pub account: AccountId,
    /// The key whose signature counts as this device's.
    pub sign_pk: PublicKey,
    /// Where wrapped scope keys are delivered.
    pub kem_pk: [u8; 32],
    /// Device key-rotation epoch.
    pub device_epoch: u32,
}

/// Reads and writes a group's account rows.
pub struct AccountBindingRepository<'a> {
    store: &'a Store,
}

impl<'a> AccountBindingRepository<'a> {
    /// Bind to `store`.
    #[must_use]
    pub const fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// The account's current root key and epoch, if this group knows the
    /// account at all.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn account_key(
        &self,
        group: &ContextGroupId,
        account: AccountId,
    ) -> EyreResult<Option<(u32, PublicKey)>> {
        let key = GroupAccountKey::new(group.to_bytes(), *account.as_bytes());
        Ok(self
            .store
            .handle()
            .get(&key)?
            .map(|value| (value.epoch, PublicKey::from(value.root_pk))))
    }

    /// Is this device's id spent? `group` must be the namespace: tombstones are
    /// recorded under it, so most callers want [`Self::device_is_withdrawn`].
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn is_revoked(&self, group: &ContextGroupId, device: DeviceId) -> EyreResult<bool> {
        let key = GroupRevokedDevice::new(group.to_bytes(), *device.as_bytes());
        Ok(self.store.handle().has(&key)?)
    }

    /// Has the namespace owning `group` withdrawn `device` of `account`: revoked
    /// it, or narrowed it out and not widened it since? Bindings and tombstones
    /// are keyed by the namespace, so a subgroup is asked through its root.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn device_is_withdrawn(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
    ) -> EyreResult<bool> {
        let namespace = crate::NamespaceRepository::new(self.store).resolve(group)?;
        if self.is_revoked(&namespace, device)? {
            return Ok(true);
        }
        // A floor with no binding row left for the account: narrowed out, not widened since.
        Ok(self.scope_floor(&namespace, account, device)?.is_some()
            && self
                .raw_binding(&namespace, device)?
                .is_none_or(|bound| bound.account != *account.as_bytes()))
    }

    /// Do the rows withdraw `account`'s `device` past a link at `link_epoch`: a revocation, or a
    /// floor at or above it? Unlike [`Self::device_is_withdrawn`], a later widening does not count.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn device_withdrawn_past(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
        link_epoch: u32,
    ) -> EyreResult<bool> {
        let namespace = crate::NamespaceRepository::new(self.store).resolve(group)?;
        Ok(self.is_revoked(&namespace, device)?
            || self
                .scope_floor(&namespace, account, device)?
                .is_some_and(|floor| floor >= link_epoch))
    }

    /// Is `device` bound to `account` in `group`'s namespace at an epoch past
    /// `device_epoch`, so a certificate at that epoch names a key it rotated out?
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn device_epoch_superseded(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
        device_epoch: u32,
    ) -> EyreResult<bool> {
        let namespace = crate::NamespaceRepository::new(self.store).resolve(group)?;
        Ok(self.raw_binding(&namespace, device)?.is_some_and(|bound| {
            bound.account == *account.as_bytes() && device_epoch < bound.device_epoch
        }))
    }

    /// Is `account`'s `device` spent in the namespace owning `group`: revoked, or
    /// withdrawn by the account's own root? Unlike a narrowing, neither is undone.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn is_spent(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
    ) -> EyreResult<bool> {
        let namespace = crate::NamespaceRepository::new(self.store).resolve(group)?;
        Ok(self.is_revoked(&namespace, device)?
            || self.is_withdrawn_for_account(&namespace, account, device)?)
    }

    /// Is `account`'s withdrawal of `device` still owed in `namespace`? Not where it
    /// is spent already, nor where the same id is bound to another account.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn withdrawal_owed(
        &self,
        namespace: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
    ) -> EyreResult<bool> {
        if self.is_spent(namespace, account, device)? {
            return Ok(false);
        }
        Ok(self
            .raw_binding(namespace, device)?
            .is_none_or(|bound| bound.account == *account.as_bytes()))
    }

    /// Did `sign_pk` sign for a device that was revoked or narrowed out in `group`?
    ///
    /// Recorded by [`apply_revocation`](Self::apply_revocation) and
    /// [`narrow`](Self::narrow) from the binding each deletes, and by
    /// [`apply_link`](Self::apply_link) for a link either refuses. On its own this
    /// does not mean the key is withdrawn: a re-paired node keeps its namespace
    /// identity under a fresh device, so a caller must first ask whether a live
    /// binding speaks for the key, as
    /// [`crate::DenyListRepository::is_author_denied_for_context`] does.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn is_signer_revoked(
        &self,
        group: &ContextGroupId,
        sign_pk: &PublicKey,
    ) -> EyreResult<bool> {
        let key = GroupRevokedSigner::new(group.to_bytes(), *AsRef::<[u8; 32]>::as_ref(sign_pk));
        Ok(self.store.handle().has(&key)?)
    }

    /// Record that `sign_pk` signed for a device withdrawn from `group`; see
    /// [`is_signer_revoked`](Self::is_signer_revoked).
    fn record_withdrawn_signer(
        &self,
        group: &ContextGroupId,
        sign_pk: &PublicKey,
    ) -> EyreResult<()> {
        let key = GroupRevokedSigner::new(group.to_bytes(), *AsRef::<[u8; 32]>::as_ref(sign_pk));
        Ok(self.store.handle().put(&key, &())?)
    }

    /// The account `sign_pk` was certified for in `group`, if any certificate for
    /// it has verified here. See [`signer_account_in_namespace`].
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn signer_account(
        &self,
        group: &ContextGroupId,
        sign_pk: &PublicKey,
    ) -> EyreResult<Option<AccountId>> {
        let key = GroupSignerAccount::new(group.to_bytes(), *AsRef::<[u8; 32]>::as_ref(sign_pk));
        Ok(self.store.handle().get(&key)?.map(AccountId::from))
    }

    /// Record that `account` certified `sign_pk`, unless the key already has a row.
    ///
    /// The first certificate wins. A key is one node's namespace identity, which a
    /// re-paired node keeps under a fresh device of the same account, so a second
    /// account for the same key is not an expected state; keeping the first
    /// avoids letting a later certificate re-attribute state already signed.
    fn record_signer_account(
        &self,
        group: &ContextGroupId,
        sign_pk: &PublicKey,
        account: AccountId,
    ) -> EyreResult<()> {
        let key = GroupSignerAccount::new(group.to_bytes(), *AsRef::<[u8; 32]>::as_ref(sign_pk));
        let mut handle = self.store.handle();
        if !handle.has(&key)? {
            handle.put(&key, account.as_bytes())?;
        }
        Ok(())
    }

    /// The raw stored binding for `device`, superseded or not.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn raw_binding(
        &self,
        group: &ContextGroupId,
        device: DeviceId,
    ) -> EyreResult<Option<GroupDeviceBindingValue>> {
        let key = GroupDeviceBinding::new(group.to_bytes(), *device.as_bytes());
        Ok(self.store.handle().get(&key)?)
    }

    /// Every binding in `group` that is **in force** — stored, not revoked, and
    /// not signed by a root key the account has since rotated past.
    ///
    /// The supersession filter lives here rather than at apply time on purpose;
    /// see the module docs. This is the list scope-key delivery fans out over,
    /// so a device the account has rotated away from receives no key.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn live_bindings(&self, group: &ContextGroupId) -> EyreResult<Vec<DeviceBinding>> {
        let gid = group.to_bytes();

        // The tombstones as a set, from one sequential pass. Every binding is
        // tested against the same rows, so a point read per binding re-walked the
        // column N times for a set that fits in memory — and this runs on the
        // per-op authorization path, not just at delivery time.
        let revoked: BTreeSet<[u8; 32]> = collect_keys_with_prefix(
            self.store,
            GroupRevokedDevice::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_REVOKED_DEVICE_PREFIX,
            |k| k.group_id() == gid,
        )?
        .into_iter()
        .map(|k| k.device_id())
        .collect();

        // Keys first, then one `get` each — deliberately, and NOT the cursor's own
        // `entries()`, which would save those reads. `entries()` decodes the value
        // of every row it steps over, and `GroupRevokedDevice` lives in this column
        // with a key of the same 65 bytes, so the typed iterator's size-mismatch
        // skip does not filter it: the scan reaches a tombstone and fails decoding
        // it as a binding before the prefix check can stop the loop. Reading values
        // by key is what keeps the two families independent.
        let keys = collect_keys_with_prefix(
            self.store,
            GroupDeviceBinding::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_DEVICE_BINDING_PREFIX,
            |k| k.group_id() == gid,
        )?;

        // Memoized for the duration of this call only. One account's epoch is the
        // same for every device it owns, and a member with several devices asked
        // for it once per device. Deliberately not cached across calls: the epoch
        // changes under a rotation, and a cache outliving the read would need an
        // invalidation path that nothing here would remember to call.
        let handle = self.store.handle();
        let mut epochs: BTreeMap<AccountId, Option<u32>> = BTreeMap::new();
        let mut out = Vec::new();
        for key in keys {
            let Some(value) = handle.get(&key)? else {
                continue;
            };
            let raw_device = key.device_id();
            if revoked.contains(&raw_device) {
                continue;
            }
            let account = AccountId::from(value.account);
            let epoch = match epochs.get(&account) {
                Some(epoch) => *epoch,
                None => {
                    let epoch = self.account_key(group, account)?.map(|(epoch, _)| epoch);
                    let _ = epochs.insert(account, epoch);
                    epoch
                }
            };
            if epoch.is_some_and(|epoch| value.key_epoch < epoch) {
                continue;
            }
            out.push(DeviceBinding {
                device: DeviceId::from(raw_device),
                account,
                sign_pk: PublicKey::from(value.sign_pk),
                kem_pk: value.kem_pk,
                device_epoch: value.device_epoch,
            });
        }

        // Replica-seed uniqueness, resolved HERE rather than at apply time, for
        // the same reason supersession is (rule 3 in the module docs). Two
        // devices sharing an HLC seed mint colliding RGA ids and lose characters
        // silently, so at most one of a colliding pair may be live — and which
        // one cannot be decided as each link arrives.
        //
        // The apply-time version rejected an incoming device only when an
        // already-stored one had a *lower* id, which is order-dependent in the
        // direction it does not check: low-then-high left one device live, but
        // high-then-low left BOTH, because the stored high id does not compare
        // lower than the incoming low one. As a filter over the stored set the
        // rule is a function of the set, so every replica reaches the same live
        // view no matter what order the links arrived in.
        let mut by_seed: BTreeMap<[u8; 16], DeviceBinding> = BTreeMap::new();
        for binding in out {
            by_seed
                .entry(binding.device.hlc_seed())
                .and_modify(|kept| {
                    if binding.device < kept.device {
                        *kept = binding;
                    }
                })
                .or_insert(binding);
        }
        Ok(by_seed.into_values().collect())
    }

    /// Record that `member` vouched for `account` in this group.
    ///
    /// Called after the endorsement has been verified — signature checked and
    /// bound to this account — so a stored row always means a real vouch.
    ///
    /// A grow-only set, and deliberately not "the" endorser: two links for one
    /// account may legitimately name different members, and collapsing them to
    /// one field would make the stored value depend on which link folded last.
    /// Idempotent, because the key carries the whole fact.
    ///
    /// # Errors
    /// Propagates the store write failure.
    pub fn record_endorser(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        member: &AccountId,
    ) -> EyreResult<()> {
        let key = GroupAccountEndorser::new(group.to_bytes(), *account.as_bytes(), *member);
        let mut handle = self.store.handle();
        if handle.has(&key)? {
            return Ok(());
        }
        handle.put(&key, &())?;
        Ok(())
    }

    /// Every account this group knows, grouped by the member key that endorsed
    /// it.
    ///
    /// The member→account direction the scope-key fan-out and per-device
    /// authorization both need. Reads the endorser rows rather than the account
    /// row's genesis key: since the account root became a dedicated offline key
    /// it is a member nowhere, so matching on the genesis key matches nothing
    /// and every member silently falls back to identity addressing — handing the
    /// scope key straight to a node running a revoked device.
    ///
    /// Re-derived per call, never cached: `AccountId` is a one-way hash, so a
    /// reverse map could only be populated while decoding ops, and would come back
    /// empty after a projection rebuild — silently reverting every member to
    /// identity addressing and undoing revocation.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn accounts_by_endorsing_member(
        &self,
        group: &ContextGroupId,
    ) -> EyreResult<BTreeMap<AccountId, Vec<AccountId>>> {
        let gid = group.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupAccountEndorser::new(gid, [0u8; 32], AccountId::from([0u8; 32])),
            calimero_store::key::GROUP_ACCOUNT_ENDORSER_PREFIX,
            |k| k.group_id() == gid,
        )?;

        let mut out: BTreeMap<AccountId, Vec<AccountId>> = BTreeMap::new();
        for key in keys {
            out.entry(key.member())
                .or_default()
                .push(AccountId::from(key.account_id()));
        }
        for accounts in out.values_mut() {
            accounts.sort_unstable();
            accounts.dedup();
        }
        Ok(out)
    }

    /// The live device binding whose certified signing key is `sign_pk`, if any.
    ///
    /// The device→account direction per-device authorization needs: a paired
    /// device signs with its own namespace identity, which is a member of
    /// nothing, so the only way it can author is through the account its
    /// certificate binds it to.
    ///
    /// Reads [`Self::live_bindings`], so a revoked or superseded device resolves
    /// to `None` — revocation therefore withdraws the right to author, not only
    /// the right to receive keys.
    ///
    /// Answered from the [`GroupSignerDevice`] index: the devices whose binding
    /// names `sign_pk`, each checked with [`Self::live_binding`]. It runs once
    /// per gossip message, for any validly signed message including a
    /// stranger's, so it must not scan the group's bindings. When several of the
    /// key's devices are live the lowest device id wins, which is the one a
    /// search of [`Self::live_bindings`] finds first.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn binding_for_sign_pk(
        &self,
        group: &ContextGroupId,
        sign_pk: &PublicKey,
    ) -> EyreResult<Option<DeviceBinding>> {
        let gid = group.to_bytes();
        let pk = *AsRef::<[u8; 32]>::as_ref(sign_pk);
        let devices = collect_keys_with_prefix(
            self.store,
            GroupSignerDevice::new(gid, pk, [0u8; 32]),
            calimero_store::key::GROUP_SIGNER_DEVICE_PREFIX,
            |k| k.group_id() == gid && k.sign_pk() == pk,
        )?;
        for key in devices {
            if let Some(binding) = self.live_binding(group, DeviceId::from(key.device_id()))? {
                if binding.sign_pk == *sign_pk {
                    return Ok(Some(binding));
                }
            }
        }
        Ok(None)
    }

    /// `device`'s binding, if it is in force: the rules of
    /// [`Self::live_bindings`] for one device, from point reads and a scan of
    /// only the bindings that share its replica seed.
    ///
    /// Stored, not revoked, not signed by a root key the account has rotated
    /// past, and not losing a replica-seed collision: no other device with the
    /// same seed that passes the first three checks has a lower id. Bindings are
    /// keyed by device id and the seed is its first 16 bytes, so the colliding
    /// devices are one contiguous run of keys.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn live_binding(
        &self,
        group: &ContextGroupId,
        device: DeviceId,
    ) -> EyreResult<Option<DeviceBinding>> {
        let Some(value) = self.raw_binding(group, device)? else {
            return Ok(None);
        };
        if !self.in_force(group, device, &value)? {
            return Ok(None);
        }

        let gid = group.to_bytes();
        let seed = device.hlc_seed();
        let mut first = [0u8; 32];
        first[..16].copy_from_slice(&seed);
        let same_seed = collect_keys_with_prefix(
            self.store,
            GroupDeviceBinding::new(gid, first),
            calimero_store::key::GROUP_DEVICE_BINDING_PREFIX,
            |k| k.group_id() == gid && k.device_id()[..16] == seed,
        )?;
        let handle = self.store.handle();
        for key in same_seed {
            let other = DeviceId::from(key.device_id());
            if other >= device {
                break;
            }
            let Some(other_value) = handle.get(&key)? else {
                continue;
            };
            if self.in_force(group, other, &other_value)? {
                return Ok(None);
            }
        }

        Ok(Some(DeviceBinding {
            device,
            account: AccountId::from(value.account),
            sign_pk: PublicKey::from(value.sign_pk),
            kem_pk: value.kem_pk,
            device_epoch: value.device_epoch,
        }))
    }

    /// Whether a stored binding passes the per-device rules of
    /// [`Self::live_bindings`]: its device is not revoked, and its account has not
    /// rotated past the root key that signed it.
    fn in_force(
        &self,
        group: &ContextGroupId,
        device: DeviceId,
        value: &GroupDeviceBindingValue,
    ) -> EyreResult<bool> {
        if self.is_revoked(group, device)? {
            return Ok(false);
        }
        let epoch = self
            .account_key(group, AccountId::from(value.account))?
            .map(|(epoch, _)| epoch);
        Ok(!epoch.is_some_and(|epoch| value.key_epoch < epoch))
    }

    /// Whether `device` has a live binding in `group`.
    ///
    /// Answers the one question that decides whether this node's stored device
    /// identity may be replaced: a device that was never linked holds no replica
    /// state, so re-minting strands nothing.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn is_device_linked(&self, group: &ContextGroupId, device: DeviceId) -> EyreResult<bool> {
        Ok(self.live_binding(group, device)?.is_some())
    }

    /// Every live device of `account` in `group` — the scope-key fan-out unit.
    ///
    /// For one account only. A caller resolving *several* accounts must use
    /// [`live_devices_by_account`](Self::live_devices_by_account) instead: this
    /// filters a full scan, so calling it in a loop rescans the column once per
    /// account and makes the fan-out quadratic in a group's size.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn devices_of(
        &self,
        group: &ContextGroupId,
        account: AccountId,
    ) -> EyreResult<Vec<DeviceBinding>> {
        Ok(self
            .live_bindings(group)?
            .into_iter()
            .filter(|binding| binding.account == account)
            .collect())
    }

    /// [`live_bindings`](Self::live_bindings) grouped by the account each device
    /// speaks for — one scan for every account in the group.
    ///
    /// What the scope-key fan-out and the pull responder both actually want. They
    /// previously called [`devices_of`](Self::devices_of) per account inside a
    /// per-member loop, so a group of *m* members with *a* accounts each rescanned
    /// the binding column *m × a* times to answer one delivery.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn live_devices_by_account(
        &self,
        group: &ContextGroupId,
    ) -> EyreResult<BTreeMap<AccountId, Vec<DeviceBinding>>> {
        let mut out: BTreeMap<AccountId, Vec<DeviceBinding>> = BTreeMap::new();
        for binding in self.live_bindings(group)? {
            out.entry(binding.account).or_default().push(binding);
        }
        Ok(out)
    }

    /// [`live_bindings`](Self::live_bindings) keyed by each device's certified
    /// signing key — one scan for every signer in the group.
    ///
    /// The batch form of
    /// [`binding_for_sign_pk`](Self::binding_for_sign_pk), for the callers that
    /// attribute *many* signatures against one group: the projection backfill
    /// resolves a signer per governance op, and the ACL shadow resolves one per
    /// rotation-log entry. Each of those searched a fresh scan, so a walk of *n*
    /// ops over a group with *d* devices read the binding column *n × d* times to
    /// answer questions that share a single answer set.
    ///
    /// Built from the filtered list rather than from the raw rows, so the
    /// read-time rules — revocation, root-key supersession, and the replica-seed
    /// reduction that is a function of the whole set — hold exactly as they do for
    /// a single lookup. This is also why there is no reverse *key family* here: a
    /// point index from `sign_pk` could not answer whether the device it names
    /// survives a seed collision without reading the devices it collides with.
    ///
    /// Nothing constrains two devices to distinct signing keys, so a duplicate
    /// resolves to the **first** binding in scan order — the same one
    /// `binding_for_sign_pk`'s search returns, which is what makes this
    /// substitutable for it.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn live_bindings_by_sign_pk(
        &self,
        group: &ContextGroupId,
    ) -> EyreResult<BTreeMap<PublicKey, DeviceBinding>> {
        let mut out: BTreeMap<PublicKey, DeviceBinding> = BTreeMap::new();
        for binding in self.live_bindings(group)? {
            let _ = out.entry(binding.sign_pk).or_insert(binding);
        }
        Ok(out)
    }

    /// Record the account root a credential carries.
    ///
    /// Called **unconditionally** whenever a credential names an account,
    /// before deciding whether the device link itself is admissible. The
    /// genesis hashes to the id it claims, so accepting it needs no trust, and
    /// making it conditional on the link succeeding is what previously made the
    /// result depend on op order.
    ///
    /// # Errors
    /// Propagates the store write failure.
    pub fn absorb_genesis(
        &self,
        group: &ContextGroupId,
        genesis: &AccountGenesis,
    ) -> EyreResult<()> {
        let account = genesis.account_id();
        let key = GroupAccountKey::new(group.to_bytes(), *account.as_bytes());
        let mut handle = self.store.handle();
        if handle.has(&key)? {
            return Ok(());
        }
        handle.put(
            &key,
            &GroupAccountKeyValue {
                epoch: 0,
                root_pk: *AsRef::<[u8; 32]>::as_ref(&genesis.root_sign_pk),
            },
        )?;
        Ok(())
    }

    /// Apply a root-key rotation.
    ///
    /// # Errors
    /// [`BindingRejected`] when the handoff does not continue the chain or is
    /// not signed by the outgoing key; otherwise propagates the store failure.
    pub fn apply_rotation(
        &self,
        group: &ContextGroupId,
        handoff: &RootKeyHandoff,
    ) -> EyreResult<Result<(), BindingRejected>> {
        let key = GroupAccountKey::new(group.to_bytes(), *handoff.account.as_bytes());
        let Some(current): Option<GroupAccountKeyValue> = self.store.handle().get(&key)? else {
            return Ok(Err(BindingRejected::RotationAccountUnknown {
                account: handoff.account,
            }));
        };
        let (epoch, root_pk) = (current.epoch, PublicKey::from(current.root_pk));
        if handoff.from_epoch != epoch {
            return Ok(Err(BindingRejected::RotationNotContinuous {
                expected: epoch,
                found: handoff.from_epoch,
            }));
        }
        if root_pk
            .verify_raw_signature(&handoff.payload(), &handoff.signature)
            .is_err()
        {
            return Ok(Err(BindingRejected::RotationSignatureInvalid));
        }

        self.store.handle().put(
            &key,
            &GroupAccountKeyValue {
                epoch: epoch.saturating_add(1),
                root_pk: *AsRef::<[u8; 32]>::as_ref(&handoff.new_root_sign_pk),
            },
        )?;
        Ok(Ok(()))
    }

    /// Apply a device link.
    ///
    /// Absorbs the genesis and any handoffs the credential carries first, then
    /// decides the link. The order matters: the account must be learned even
    /// when the link is refused (see the module docs).
    ///
    /// Deliberately does **not** check group membership — that is an
    /// authorization question for the caller, which knows the group's member
    /// set. Keeping it out means this function stays a pure statement about
    /// credential admissibility.
    ///
    /// # Errors
    /// [`BindingRejected`] for an inadmissible credential; otherwise propagates
    /// the store failure.
    pub fn apply_link(
        &self,
        group: &ContextGroupId,
        genesis: &AccountGenesis,
        chain: &[RootKeyHandoff],
        cert: &DeviceCert,
        scope_epoch: u32,
    ) -> EyreResult<Result<DeviceBinding, BindingRejected>> {
        // Cap before the loop below, not just inside `verify_device_cert`. Each
        // `apply_rotation` costs an Ed25519 verification, and this runs first, so
        // the cap living only in `root_key_at_epoch` left the expensive part
        // unguarded — the same one-call-too-deep miss as the projection fold had.
        if chain.len() > calimero_account::MAX_ROOT_KEY_HANDOFFS {
            return Ok(Err(BindingRejected::ChainTooLong {
                found: chain.len(),
                limit: calimero_account::MAX_ROOT_KEY_HANDOFFS,
            }));
        }

        if genesis.account_id() == cert.account {
            self.absorb_genesis(group, genesis)?;
            for handoff in chain {
                // A handoff that does not continue the chain is simply not
                // absorbed; the credential's own verification below is what
                // decides whether the certificate stands.
                let _outcome = self.apply_rotation(group, handoff)?;
            }
        }

        let verified = match verify_device_cert(cert.account, genesis, chain, cert) {
            Ok(verified) => verified,
            Err(e) => return Ok(Err(BindingRejected::CredentialInvalid(e))),
        };

        // Before any refusal below: those decide whether the device may act
        // now, but the certificate has already proved the account signed for
        // this key. A revocation folded before its link is the case that needs
        // it, since that link is never stored as a binding at all.
        self.record_signer_account(group, &verified.sign_pk, verified.account)?;

        if self.is_revoked(group, verified.device)? {
            self.record_withdrawn_signer(group, &verified.sign_pk)?;
            return Ok(Err(BindingRejected::DeviceRevoked));
        }
        // Consulted before the binding, like the tombstone: a narrowing that
        // arrives before the stale link it outranks must still win.
        if let Some(floor) = self.scope_floor(group, verified.account, verified.device)? {
            if scope_epoch <= floor {
                self.record_withdrawn_signer(group, &verified.sign_pk)?;
                return Ok(Err(BindingRejected::ScopeNarrowed {
                    offered: scope_epoch,
                    floor,
                }));
            }
        }

        match self.raw_binding(group, verified.device)? {
            Some(existing) => {
                if existing.account != *verified.account.as_bytes() {
                    return Ok(Err(BindingRejected::AccountReassignment));
                }
                // A credential that re-states EXACTLY what is already stored is a
                // replay, not a stale offer, and re-applying it must succeed.
                // Every apply handler re-runs its mutation before the op-log
                // dedup fires, so an op reaching this code a second time is
                // ordinary — re-gossip, DAG replay, a crash between the nonce
                // write and the log append. Answering `EpochNotAdvanced` there
                // reports a refusal for a binding this group holds and agrees
                // with, which is how a join came to log a rejection on every
                // replica on every replay.
                //
                // Narrow on purpose: same device, same account, same keys, same
                // epochs. Anything else at an unadvanced epoch — a different
                // signing key, a different KEM key — is a fork of a spent epoch
                // and still refused.
                if existing.sign_pk == *AsRef::<[u8; 32]>::as_ref(&verified.sign_pk)
                    && existing.kem_pk == *verified.kem_pk.as_bytes()
                    && existing.device_epoch == verified.device_epoch
                    && existing.key_epoch == verified.key_epoch
                {
                    // The scope stamp still moves: a re-link under a newer scope
                    // is what retires the descopes signed before it.
                    if scope_epoch > existing.scope_epoch {
                        self.put_binding(
                            group,
                            verified.device,
                            &GroupDeviceBindingValue {
                                scope_epoch,
                                ..existing
                            },
                        )?;
                    }
                    return Ok(Ok(DeviceBinding {
                        device: verified.device,
                        account: verified.account,
                        sign_pk: verified.sign_pk,
                        kem_pk: *verified.kem_pk.as_bytes(),
                        device_epoch: verified.device_epoch,
                    }));
                }
                if verified.device_epoch <= existing.device_epoch {
                    return Ok(Err(BindingRejected::EpochNotAdvanced {
                        offered: verified.device_epoch,
                        stored: existing.device_epoch,
                    }));
                }
            }
            None => {
                // No first-link check. Replica-seed uniqueness is decided by
                // `live_bindings` over the stored set, because "which of two
                // colliding devices is live" cannot be answered as each link
                // arrives — see the comment there. Storing the loser costs one
                // row and keeps the verdict a function of the op set; rejecting
                // it here made the live view depend on arrival order.
                //
                // Dropping the check also removes a full `live_bindings` scan
                // from every link apply, which was O(devices) store reads on a
                // path any member can drive.
            }
        }

        self.put_binding(
            group,
            verified.device,
            &GroupDeviceBindingValue {
                account: *verified.account.as_bytes(),
                sign_pk: *AsRef::<[u8; 32]>::as_ref(&verified.sign_pk),
                kem_pk: *verified.kem_pk.as_bytes(),
                device_epoch: verified.device_epoch,
                key_epoch: verified.key_epoch,
                scope_epoch,
            },
        )?;

        Ok(Ok(DeviceBinding {
            device: verified.device,
            account: verified.account,
            sign_pk: verified.sign_pk,
            kem_pk: *verified.kem_pk.as_bytes(),
            device_epoch: verified.device_epoch,
        }))
    }

    /// Remove every account row under `group` — bindings, revocation
    /// tombstones and the signing keys they withdrew, per-account root keys, and
    /// the scope floors.
    ///
    /// Used by the group teardown so the account plane does not outlive the group
    /// it describes. The tombstones matter most: they are **terminal**, so a group
    /// later recreated under the same id would otherwise inherit a set of device
    /// ids it can never enroll, with nothing in the new group's history to explain
    /// why. Stale bindings are the milder half of the same problem — they would
    /// make the fan-out wrap scope keys for devices of the previous occupants.
    ///
    /// # Errors
    /// Propagates the store scan or delete failure.
    pub fn clear_all_for_group(&self, group: &ContextGroupId) -> EyreResult<()> {
        let gid = group.to_bytes();
        let bindings = collect_keys_with_prefix(
            self.store,
            GroupDeviceBinding::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_DEVICE_BINDING_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let revoked = collect_keys_with_prefix(
            self.store,
            GroupRevokedDevice::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_REVOKED_DEVICE_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let revoked_signers = collect_keys_with_prefix(
            self.store,
            GroupRevokedSigner::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_REVOKED_SIGNER_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let signer_accounts = collect_keys_with_prefix(
            self.store,
            GroupSignerAccount::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_SIGNER_ACCOUNT_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let signer_devices = collect_keys_with_prefix(
            self.store,
            GroupSignerDevice::new(gid, [0u8; 32], [0u8; 32]),
            calimero_store::key::GROUP_SIGNER_DEVICE_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let accounts = collect_keys_with_prefix(
            self.store,
            GroupAccountKey::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_ACCOUNT_KEY_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let floors = collect_keys_with_prefix(
            self.store,
            GroupDeviceScopeFloor::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_DEVICE_SCOPE_FLOOR_PREFIX,
            |k| k.group_id() == gid,
        )?;

        let mut handle = self.store.handle();
        for key in bindings {
            handle.delete(&key)?;
        }
        for key in revoked {
            handle.delete(&key)?;
        }
        for key in revoked_signers {
            handle.delete(&key)?;
        }
        for key in signer_accounts {
            handle.delete(&key)?;
        }
        for key in signer_devices {
            handle.delete(&key)?;
        }
        for key in accounts {
            handle.delete(&key)?;
        }
        for key in floors {
            handle.delete(&key)?;
        }
        Ok(())
    }

    /// The scope epoch `device` was last narrowed out of `group` at, if ever.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn scope_floor(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
    ) -> EyreResult<Option<u32>> {
        Ok(self
            .store
            .handle()
            .get(&floor_key(group, account, device))?)
    }

    /// Narrow `device` out of `group` at `scope_epoch`: raise the floor whatever
    /// is bound, and drop a binding made under an older scope. No device
    /// tombstone, so a widening re-enables it; the dropped binding's key is
    /// recorded as in [`apply_revocation`](Self::apply_revocation).
    ///
    /// # Errors
    /// Propagates the store failure.
    pub fn narrow(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
        scope_epoch: u32,
    ) -> EyreResult<bool> {
        let key = floor_key(group, account, device);
        let mut handle = self.store.handle();
        if handle.get(&key)?.is_none_or(|floor| scope_epoch > floor) {
            handle.put(&key, &scope_epoch)?;
        }
        match self.raw_binding(group, device)? {
            Some(bound)
                if bound.account == *account.as_bytes() && bound.scope_epoch < scope_epoch =>
            {
                self.record_withdrawn_signer(group, &PublicKey::from(bound.sign_pk))?;
                self.delete_binding(group, device)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Record that `account`'s own root withdrew `device` here, when this group
    /// holds no binding that ties the two.
    ///
    /// The device-wide tombstone ([`apply_revocation`](Self::apply_revocation))
    /// cannot be written from a root-signed proof alone: the proof names a
    /// `DeviceId` its signer chose, so honouring it would let any account spend
    /// any other account's device for good. This records the withdrawal in the
    /// slot keyed by account AND device instead — the scope floor, raised to
    /// [`WITHDRAWN_SCOPE_FLOOR`], which no scope epoch outranks. A proof from
    /// another account names another slot, so it withdraws nothing of anyone
    /// else's. A later link of this device for this account is refused by the
    /// floor check in [`apply_link`](Self::apply_link), whatever order the two
    /// arrive in.
    ///
    /// # Errors
    /// Propagates the store failure.
    pub fn withdraw_for_account(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
    ) -> EyreResult<()> {
        let _dropped = self.narrow(group, account, device, WITHDRAWN_SCOPE_FLOOR)?;
        Ok(())
    }

    /// Did `account`'s own root withdraw `device` in `group`?
    ///
    /// True after [`withdraw_for_account`](Self::withdraw_for_account). A device
    /// the group revoked outright reads [`is_revoked`](Self::is_revoked) instead;
    /// a caller deciding whether a device may still act for its account asks both.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn is_withdrawn_for_account(
        &self,
        group: &ContextGroupId,
        account: AccountId,
        device: DeviceId,
    ) -> EyreResult<bool> {
        Ok(self.scope_floor(group, account, device)? == Some(WITHDRAWN_SCOPE_FLOOR))
    }

    /// Withdraw a device.
    ///
    /// Writes the tombstone **unconditionally**, even for a device this group
    /// has never seen linked: a revocation that arrives before its link must
    /// still win, and the tombstone is what the link consults. Dropping an
    /// early revocation would make the outcome depend on arrival order.
    ///
    /// Also records the signing key the deleted binding named, since that is how a
    /// state delta names its author (see [`GroupRevokedSigner`]). A revocation that
    /// arrives before its link has no binding to read; the link it refuses records
    /// the key instead, so the verdict does not depend on arrival order.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn apply_revocation(&self, group: &ContextGroupId, device: DeviceId) -> EyreResult<()> {
        if let Some(bound) = self.raw_binding(group, device)? {
            self.record_withdrawn_signer(group, &PublicKey::from(bound.sign_pk))?;
        }
        self.store.handle().put(
            &GroupRevokedDevice::new(group.to_bytes(), *device.as_bytes()),
            &(),
        )?;
        self.delete_binding(group, device)
    }

    /// Store `device`'s binding and keep the [`GroupSignerDevice`] index in step:
    /// the row for the binding's key is written, and the one for the key it
    /// replaces, if any, is removed. Every binding write goes through here.
    fn put_binding(
        &self,
        group: &ContextGroupId,
        device: DeviceId,
        value: &GroupDeviceBindingValue,
    ) -> EyreResult<()> {
        let gid = group.to_bytes();
        let mut handle = self.store.handle();
        if let Some(previous) = self.raw_binding(group, device)? {
            if previous.sign_pk != value.sign_pk {
                handle.delete(&GroupSignerDevice::new(
                    gid,
                    previous.sign_pk,
                    *device.as_bytes(),
                ))?;
            }
        }
        handle.put(&GroupDeviceBinding::new(gid, *device.as_bytes()), value)?;
        handle.put(
            &GroupSignerDevice::new(gid, value.sign_pk, *device.as_bytes()),
            &(),
        )?;
        Ok(())
    }

    /// Delete `device`'s binding and its [`GroupSignerDevice`] index row. Every
    /// binding delete goes through here.
    fn delete_binding(&self, group: &ContextGroupId, device: DeviceId) -> EyreResult<()> {
        let gid = group.to_bytes();
        let mut handle = self.store.handle();
        if let Some(previous) = self.raw_binding(group, device)? {
            handle.delete(&GroupSignerDevice::new(
                gid,
                previous.sign_pk,
                *device.as_bytes(),
            ))?;
        }
        handle.delete(&GroupDeviceBinding::new(gid, *device.as_bytes()))?;
        Ok(())
    }
}

/// The scope floor a device's own account leaves it at when it withdraws it:
/// no scope epoch outranks it, so the withdrawal is terminal for that account.
/// See [`AccountBindingRepository::withdraw_for_account`].
pub const WITHDRAWN_SCOPE_FLOOR: u32 = u32::MAX;

/// Account and device hashed into one slot: a statement from another account's
/// root names a different slot, so it cannot raise this device's floor.
fn floor_key(
    group: &ContextGroupId,
    account: AccountId,
    device: DeviceId,
) -> GroupDeviceScopeFloor {
    let mut hasher = Sha256::new();
    hasher.update(account.as_bytes());
    hasher.update(device.as_bytes());
    GroupDeviceScopeFloor::new(group.to_bytes(), hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};
    use calimero_account::{DeviceCert, KemPublicKey, RootKeyHandoff};
    use calimero_primitives::identity::PrivateKey;

    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    fn genesis_for(seed: u8) -> AccountGenesis {
        AccountGenesis::new(key(seed).public_key())
    }

    fn cert_for(
        genesis: &AccountGenesis,
        signer: &PrivateKey,
        device_seed: u8,
        key_epoch: u32,
        device_epoch: u32,
    ) -> DeviceCert {
        let account = genesis.account_id();
        DeviceCert::sign(
            signer,
            account,
            DeviceId::mint(account, [device_seed; 16]),
            &key(device_seed).public_key(),
            &KemPublicKey::from([device_seed; 32]),
            key_epoch,
            device_epoch,
        )
        .expect("sign")
    }

    /// The point reads must answer exactly what a search of `live_bindings` does:
    /// `binding_for_sign_pk` for each key seed, `is_device_linked` and
    /// `live_binding` for each device.
    fn assert_point_reads_agree(
        repo: &AccountBindingRepository<'_>,
        gid: &ContextGroupId,
        key_seeds: &[u8],
        devices: &[DeviceId],
    ) {
        let live = repo.live_bindings(gid).expect("read");
        for seed in key_seeds {
            let sign_pk = key(*seed).public_key();
            assert_eq!(
                repo.binding_for_sign_pk(gid, &sign_pk).expect("read"),
                live.iter().find(|b| b.sign_pk == sign_pk).copied(),
                "binding_for_sign_pk disagrees for key {seed}"
            );
        }
        for device in devices {
            let expected = live.iter().find(|b| b.device == *device).copied();
            assert_eq!(repo.live_binding(gid, *device).expect("read"), expected);
            assert_eq!(
                repo.is_device_linked(gid, *device).expect("read"),
                expected.is_some()
            );
        }
    }

    #[test]
    fn is_device_linked_distinguishes_a_bound_device_from_a_merely_minted_one() {
        // Decides whether this node's stored device identity may be replaced. A
        // device that was never linked holds no replica state, so re-minting
        // strands nothing — which is what keeps a one-shot device row from
        // becoming a trap when a pairing is attempted with the wrong account.
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        let never_linked = DeviceId::mint(g.account_id(), [0x77; 16]);
        assert!(!repo.is_device_linked(&gid, never_linked).expect("query"));

        let bound = repo
            .apply_link(&gid, &g, &[], &cert, 0)
            .expect("store")
            .expect("admitted");
        assert!(repo.is_device_linked(&gid, bound.device).expect("query"));

        // A revoked device is not live, so its id is not "linked" either — but it
        // must NOT become re-mintable, because the tombstone is terminal and the
        // id is spent. That is enforced by the revocation check on link, not here.
        repo.apply_revocation(&gid, bound.device).expect("revoke");
        assert!(!repo.is_device_linked(&gid, bound.device).expect("query"));
    }

    /// A namespace with one subgroup and a device of an account bound in the
    /// namespace, which is where bindings and revocations are recorded.
    fn bound_in_a_namespace() -> (
        Store,
        ContextGroupId,
        ContextGroupId,
        AccountGenesis,
        DeviceCert,
    ) {
        let store = test_store();
        let ns = test_group_id();
        let sub = ContextGroupId::from([0x7B; 32]);
        crate::test_fixtures::nest_for_test(&store, &ns, &sub);
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);
        AccountBindingRepository::new(&store)
            .apply_link(&ns, &g, &[], &cert, 0)
            .expect("store")
            .expect("admitted");
        (store, ns, sub, g, cert)
    }

    #[test]
    fn a_live_device_is_not_withdrawn_anywhere_in_its_namespace() {
        let (store, ns, sub, g, cert) = bound_in_a_namespace();
        let repo = AccountBindingRepository::new(&store);
        for group in [&ns, &sub] {
            assert!(!repo
                .device_is_withdrawn(group, g.account_id(), cert.device)
                .expect("read"));
        }
        let unknown = DeviceId::mint(g.account_id(), [0x77; 16]);
        assert!(
            !repo
                .device_is_withdrawn(&sub, g.account_id(), unknown)
                .expect("read"),
            "a device no row names has not been withdrawn"
        );
    }

    #[test]
    fn a_revocation_made_for_the_namespace_withdraws_the_device_in_a_subgroup() {
        let (store, ns, sub, g, cert) = bound_in_a_namespace();
        let repo = AccountBindingRepository::new(&store);
        repo.apply_revocation(&ns, cert.device).expect("revoke");

        for group in [&ns, &sub] {
            assert!(
                repo.device_is_withdrawn(group, g.account_id(), cert.device)
                    .expect("read"),
                "the tombstone is keyed by the namespace and covers every group in it"
            );
        }
        let elsewhere = ContextGroupId::from([0x7C; 32]);
        assert!(
            !repo
                .device_is_withdrawn(&elsewhere, g.account_id(), cert.device)
                .expect("read"),
            "a revocation in one namespace does not reach another"
        );
    }

    #[test]
    fn a_device_narrowed_out_is_withdrawn_until_a_newer_scope_binds_it_again() {
        let (store, ns, sub, g, cert) = bound_in_a_namespace();
        let repo = AccountBindingRepository::new(&store);
        let account = g.account_id();
        repo.narrow(&ns, account, cert.device, 1).expect("narrow");

        assert!(repo
            .device_is_withdrawn(&sub, account, cert.device)
            .expect("read"));
        assert_point_reads_agree(&repo, &ns, &[5], &[cert.device]);
        assert_eq!(
            repo.binding_for_sign_pk(&ns, &key(5).public_key())
                .expect("read"),
            None,
            "a narrowed-out device's key resolves to nobody"
        );

        repo.apply_link(&ns, &g, &[], &cert, 2)
            .expect("store")
            .expect("a link under a newer scope is admitted");
        assert!(
            !repo
                .device_is_withdrawn(&sub, account, cert.device)
                .expect("read"),
            "widened again, the device is bound and acts"
        );
        assert_point_reads_agree(&repo, &ns, &[5], &[cert.device]);
    }

    /// A re-paired node keeps its signing key under a fresh device, so one key
    /// can name several bindings. The index holds one row per device, and the
    /// lookup answers what a search of `live_bindings` does whichever of them is
    /// live: the lower device id while both are, the other once it is revoked.
    #[test]
    fn a_signing_key_bound_to_two_devices_resolves_to_its_live_one() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let account = g.account_id();
        let cert_on = |nonce: u8| {
            DeviceCert::sign(
                &key(1),
                account,
                DeviceId::mint(account, [nonce; 16]),
                &key(5).public_key(),
                &KemPublicKey::from([nonce; 32]),
                0,
                0,
            )
            .expect("sign")
        };
        let (first, second) = (cert_on(0x10), cert_on(0x20));
        for cert in [&first, &second] {
            let _ = repo
                .apply_link(&gid, &g, &[], cert, 0)
                .expect("store")
                .expect("admitted");
        }
        let devices = [first.device, second.device];
        assert_point_reads_agree(&repo, &gid, &[5], &devices);

        let lower = first.device.min(second.device);
        let higher = first.device.max(second.device);
        repo.apply_revocation(&gid, lower).expect("revoke");
        assert_point_reads_agree(&repo, &gid, &[5], &devices);
        assert_eq!(
            repo.binding_for_sign_pk(&gid, &key(5).public_key())
                .expect("read")
                .map(|b| b.device),
            Some(higher)
        );
    }

    #[test]
    fn a_valid_link_is_recorded_and_reported_live() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        let bound = repo
            .apply_link(&gid, &g, &[], &cert, 0)
            .expect("store")
            .expect("admitted");
        assert_eq!(bound.account, g.account_id());
        assert_eq!(
            repo.devices_of(&gid, g.account_id()).expect("read").len(),
            1
        );
    }

    #[test]
    fn two_devices_of_one_account_are_both_live_and_distinct() {
        // The property the whole feature exists for: one identity, two
        // replicas.
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);

        for seed in [5u8, 6] {
            let _ = repo
                .apply_link(&gid, &g, &[], &cert_for(&g, &key(1), seed, 0, 0), 0)
                .expect("store")
                .expect("admitted");
        }

        let devices = repo.devices_of(&gid, g.account_id()).expect("read");
        assert_eq!(devices.len(), 2);
        assert_ne!(devices[0].device, devices[1].device);
        assert_eq!(devices[0].account, devices[1].account);
    }

    #[test]
    fn a_revocation_applied_before_its_link_still_wins() {
        // The ordering hazard the separate tombstone family exists for. As a
        // flag on the binding, this order would resurrect the device.
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        repo.apply_revocation(&gid, cert.device).expect("revoke");
        assert_eq!(
            repo.apply_link(&gid, &g, &[], &cert, 0).expect("store"),
            Err(BindingRejected::DeviceRevoked)
        );
        assert!(repo.live_bindings(&gid).expect("read").is_empty());
    }

    #[test]
    fn a_signing_key_keeps_its_account_through_revocation_in_either_order() {
        // Snapshot apply checks a leaf's signer against the account-keyed owner
        // or writer set, for state signed before the device was revoked. The live
        // binding is gone by then, so the key must still name its account, and
        // it must do so whichever of the two ops this replica folded first.
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);
        let sign_pk = key(5).public_key();

        for revoke_first in [false, true] {
            let store = test_store();
            let gid = test_group_id();
            let repo = AccountBindingRepository::new(&store);
            if revoke_first {
                repo.apply_revocation(&gid, cert.device).expect("revoke");
                let _ = repo.apply_link(&gid, &g, &[], &cert, 0).expect("store");
            } else {
                let _ = repo.apply_link(&gid, &g, &[], &cert, 0).expect("store");
                repo.apply_revocation(&gid, cert.device).expect("revoke");
            }
            assert!(repo
                .binding_for_sign_pk(&gid, &sign_pk)
                .expect("read")
                .is_none());
            assert_eq!(
                repo.signer_account(&gid, &sign_pk).expect("read"),
                Some(g.account_id()),
                "revoke_first = {revoke_first}"
            );
        }
    }

    #[test]
    fn a_key_no_certificate_named_has_no_signer_account() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        assert_eq!(
            repo.signer_account(&gid, &key(9).public_key())
                .expect("read"),
            None
        );
    }

    #[test]
    fn revocation_after_a_link_removes_the_binding() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        let _ = repo.apply_link(&gid, &g, &[], &cert, 0).expect("store");
        repo.apply_revocation(&gid, cert.device).expect("revoke");
        assert!(repo.live_bindings(&gid).expect("read").is_empty());
    }

    #[test]
    fn both_revocation_orders_reach_the_same_state() {
        // Convergence, stated directly: apply the same two facts either way
        // round and the group must end up in the same place.
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        let link_then_revoke = {
            let store = test_store();
            let gid = test_group_id();
            let repo = AccountBindingRepository::new(&store);
            let _ = repo.apply_link(&gid, &g, &[], &cert, 0).expect("store");
            repo.apply_revocation(&gid, cert.device).expect("revoke");
            (
                repo.live_bindings(&gid).expect("read"),
                repo.account_key(&gid, g.account_id()).expect("read"),
            )
        };
        let revoke_then_link = {
            let store = test_store();
            let gid = test_group_id();
            let repo = AccountBindingRepository::new(&store);
            repo.apply_revocation(&gid, cert.device).expect("revoke");
            let _ = repo.apply_link(&gid, &g, &[], &cert, 0).expect("store");
            (
                repo.live_bindings(&gid).expect("read"),
                repo.account_key(&gid, g.account_id()).expect("read"),
            )
        };

        assert_eq!(link_then_revoke.0, revoke_then_link.0, "bindings diverged");
        assert_eq!(
            link_then_revoke.1, revoke_then_link.1,
            "account key diverged — the genesis must be absorbed even when the \
             link it arrived on is refused"
        );
    }

    #[test]
    fn two_devices_sharing_a_replica_seed_converge_on_the_lower_id_either_order() {
        // The seed rule has to be a function of the stored SET, not of arrival
        // order. Rejecting the newcomer only when an existing device has a lower
        // id is order-dependent: low-then-high leaves one device live, but
        // high-then-low leaves BOTH live, because the existing high id does not
        // compare lower than the incoming low one. Two replicas sharing an HLC
        // seed mint colliding RGA ids and lose characters silently, which is the
        // whole reason the rule exists.
        let g = genesis_for(1);
        let account = g.account_id();

        // Two certs whose device ids share an hlc_seed. The seed is the id's
        // first 16 bytes, so forge the ids directly rather than hunting for a
        // `mint` nonce collision.
        let mut low = [0u8; 32];
        low[..16].copy_from_slice(&[0xAA; 16]);
        let mut high = low;
        high[31] = 0xFF;
        let (low, high) = (DeviceId::from(low), DeviceId::from(high));
        assert_eq!(low.hlc_seed(), high.hlc_seed());
        assert!(low < high);

        let cert_for_device = |device: DeviceId, seed: u8| {
            DeviceCert::sign(
                &key(1),
                account,
                device,
                &key(seed).public_key(),
                &KemPublicKey::from([seed; 32]),
                0,
                0,
            )
            .expect("sign")
        };
        let low_cert = cert_for_device(low, 5);
        let high_cert = cert_for_device(high, 6);

        let live_after = |order: [&DeviceCert; 2]| {
            let store = test_store();
            let gid = test_group_id();
            let repo = AccountBindingRepository::new(&store);
            for cert in order {
                let _ = repo.apply_link(&gid, &g, &[], cert, 0).expect("store");
            }
            assert_point_reads_agree(&repo, &gid, &[5, 6], &[low, high]);
            let mut live: Vec<DeviceId> = repo
                .live_bindings(&gid)
                .expect("read")
                .into_iter()
                .map(|b| b.device)
                .collect();
            live.sort_unstable();
            live
        };

        let low_first = live_after([&low_cert, &high_cert]);
        let high_first = live_after([&high_cert, &low_cert]);

        assert_eq!(
            low_first, high_first,
            "the live set must not depend on which link applied first"
        );
        assert_eq!(
            low_first,
            vec![low],
            "the lower device id must be the one left live"
        );
    }

    #[test]
    fn the_adversarial_workload_reaches_the_same_state_in_every_order() {
        // The governance plane's counterpart to
        // `the_adversarial_account_workload_converges` in the projection. Same
        // reasoning: every order-dependence bug here came from a rule that read
        // "whatever has been applied so far", and a workload of mutually
        // consistent ops cannot expose one. So this applies the shapes that broke
        // — a seed-colliding pair, a revocation, and a rotation that supersedes —
        // in all 24 orders and requires identical materialized state.
        //
        // When a new order-dependence bug is found, add its shape here.
        let g = genesis_for(1);
        let account = g.account_id();

        let mut low = [0u8; 32];
        low[..16].copy_from_slice(&[0xAA; 16]);
        let mut high = low;
        high[31] = 0xFF;
        let (low, high) = (DeviceId::from(low), DeviceId::from(high));

        let cert_at = |device: DeviceId, seed: u8, key_epoch: u32| {
            DeviceCert::sign(
                &key(if key_epoch == 0 { 1 } else { 2 }),
                account,
                device,
                &key(seed).public_key(),
                &KemPublicKey::from([seed; 32]),
                key_epoch,
                0,
            )
            .expect("sign")
        };
        let handoff =
            RootKeyHandoff::sign(&key(1), account, 0, &key(2).public_key()).expect("sign");

        // Certified at epoch 1 so the rotation does not supersede them and mask
        // the collision property.
        let low_cert = cert_at(low, 5, 1);
        let high_cert = cert_at(high, 6, 1);
        let doomed = cert_at(DeviceId::mint(account, [7u8; 16]), 7, 1);

        #[derive(Clone, Copy)]
        enum Step {
            LinkLow,
            LinkHigh,
            LinkDoomed,
            Revoke,
        }

        let run = |order: &[Step]| {
            let store = test_store();
            let gid = test_group_id();
            let repo = AccountBindingRepository::new(&store);
            // The rotation is applied first in every run: it is what establishes
            // the epoch the certificates above claim, and `apply_rotation` needs
            // the account already learned, so its position is not free to vary.
            let _ = repo.apply_link(&gid, &g, &[], &low_cert, 0).expect("store");
            repo.apply_rotation(&gid, &handoff)
                .expect("store")
                .expect("rotated");
            for step in order {
                match step {
                    Step::LinkLow => {
                        let _ = repo
                            .apply_link(&gid, &g, &[handoff], &low_cert, 0)
                            .expect("s");
                    }
                    Step::LinkHigh => {
                        let _ = repo
                            .apply_link(&gid, &g, &[handoff], &high_cert, 0)
                            .expect("s");
                    }
                    Step::LinkDoomed => {
                        let _ = repo
                            .apply_link(&gid, &g, &[handoff], &doomed, 0)
                            .expect("s");
                    }
                    Step::Revoke => repo.apply_revocation(&gid, doomed.device).expect("revoke"),
                }
            }
            assert_point_reads_agree(&repo, &gid, &[1, 2, 5, 6, 7], &[low, high, doomed.device]);
            let mut live: Vec<DeviceId> = repo
                .live_bindings(&gid)
                .expect("read")
                .into_iter()
                .map(|b| b.device)
                .collect();
            live.sort_unstable();
            (live, repo.account_key(&gid, account).expect("read"))
        };

        let steps = [
            Step::LinkLow,
            Step::LinkHigh,
            Step::LinkDoomed,
            Step::Revoke,
        ];
        let mut orders: Vec<Vec<Step>> = Vec::new();
        for a in 0..4 {
            for b in 0..4 {
                for c in 0..4 {
                    for d in 0..4 {
                        let idx = [a, b, c, d];
                        let mut seen = [false; 4];
                        for i in idx {
                            seen[i] = true;
                        }
                        if seen.iter().all(|s| *s) {
                            orders.push(idx.iter().map(|i| steps[*i]).collect());
                        }
                    }
                }
            }
        }
        assert_eq!(orders.len(), 24, "all 4! orders should be enumerated");

        let expected = run(&orders[0]);
        for order in &orders[1..] {
            assert_eq!(
                run(order),
                expected,
                "an application order produced different materialized state"
            );
        }

        // Convergence alone would also hold if everything were dropped.
        let (live, key_state) = expected;
        assert_eq!(live, vec![low], "only the lower colliding id may be live");
        assert_eq!(key_state.map(|r| r.0), Some(1), "the rotation took effect");
    }

    #[test]
    fn a_rotation_supersedes_certificates_from_the_old_key() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let account = g.account_id();

        // Learn the account, then rotate its root key.
        let _ = repo
            .apply_link(&gid, &g, &[], &cert_for(&g, &key(1), 5, 0, 0), 0)
            .expect("store")
            .expect("admitted");
        let handoff =
            RootKeyHandoff::sign(&key(1), account, 0, &key(2).public_key()).expect("sign");
        repo.apply_rotation(&gid, &handoff)
            .expect("store")
            .expect("rotated");

        assert_eq!(
            repo.account_key(&gid, account).expect("read").map(|r| r.0),
            Some(1)
        );

        // The device certified under epoch 0 is no longer in force...
        assert!(
            repo.live_bindings(&gid).expect("read").is_empty(),
            "a binding signed by a superseded key must drop out on read"
        );
        // ...and a fresh certificate from the new key is.
        let _ = repo
            .apply_link(&gid, &g, &[handoff], &cert_for(&g, &key(2), 6, 1, 0), 0)
            .expect("store")
            .expect("admitted");
        assert_eq!(repo.live_bindings(&gid).expect("read").len(), 1);
    }

    #[test]
    fn clearing_a_group_removes_every_account_row_and_only_that_group_s() {
        // The tombstones are the reason this matters: they are terminal, so a
        // group recreated under the same id would inherit device ids it can never
        // enroll, with nothing in its own history to explain why.
        let store = test_store();
        let gid = test_group_id();
        let other = ContextGroupId::from([0x99u8; 32]);
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);

        let live = cert_for(&g, &key(1), 5, 0, 0);
        let doomed = cert_for(&g, &key(1), 6, 0, 0);
        for cert in [&live, &doomed] {
            let _ = repo.apply_link(&gid, &g, &[], cert, 0).expect("store");
            let _ = repo.apply_link(&other, &g, &[], cert, 0).expect("store");
        }
        repo.apply_revocation(&gid, doomed.device).expect("revoke");
        repo.apply_revocation(&other, doomed.device)
            .expect("revoke");

        // A floor outliving the group would keep a device out of a group later
        // recreated under the same id, with nothing in its history to explain why.
        repo.narrow(&gid, g.account_id(), live.device, 3)
            .expect("narrow");
        repo.narrow(&other, g.account_id(), doomed.device, 3)
            .expect("narrow");

        repo.clear_all_for_group(&gid).expect("clear");

        assert_eq!(
            repo.scope_floor(&gid, g.account_id(), live.device)
                .expect("read"),
            None
        );
        assert!(repo.live_bindings(&gid).expect("read").is_empty());
        assert_eq!(
            repo.binding_for_sign_pk(&gid, &key(5).public_key())
                .expect("read"),
            None
        );
        assert!(
            !repo.is_revoked(&gid, doomed.device).expect("read"),
            "a terminal tombstone must not outlive the group it describes"
        );
        assert!(repo
            .account_key(&gid, g.account_id())
            .expect("read")
            .is_none());

        // The other group is untouched.
        assert_eq!(repo.live_bindings(&other).expect("read").len(), 1);
        assert_point_reads_agree(&repo, &other, &[5, 6], &[live.device, doomed.device]);
        assert!(repo.is_revoked(&other, doomed.device).expect("read"));
        assert!(repo
            .account_key(&other, g.account_id())
            .expect("read")
            .is_some());
        assert_eq!(
            repo.scope_floor(&other, g.account_id(), doomed.device)
                .expect("read"),
            Some(3)
        );

        // Idempotent.
        repo.clear_all_for_group(&gid).expect("clear again");
    }

    /// A link AT the floor is refused; a narrowing AT the binding's stamp leaves
    /// it bound.
    #[test]
    fn an_equal_scope_epoch_neither_links_nor_unbinds() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let account = g.account_id();
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        let _ = repo.apply_link(&gid, &g, &[], &cert, 2).expect("store");
        assert!(
            !repo.narrow(&gid, account, cert.device, 2).expect("narrow"),
            "a narrowing at the epoch the binding was made under does not unbind it"
        );
        assert!(repo.is_device_linked(&gid, cert.device).expect("read"));

        // A second device, so the floor raised for it is the one its link meets.
        let sibling = cert_for(&g, &key(1), 6, 0, 0);
        let _ = repo
            .narrow(&gid, account, sibling.device, 4)
            .expect("narrow");
        assert!(
            matches!(
                repo.apply_link(&gid, &g, &[], &sibling, 4).expect("store"),
                Err(BindingRejected::ScopeNarrowed { .. })
            ),
            "a link AT the floor is under it, not above it"
        );
        assert!(repo
            .apply_link(&gid, &g, &[], &sibling, 5)
            .expect("store")
            .is_ok());
    }

    #[test]
    fn a_rotation_for_an_unknown_account_is_refused() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let handoff =
            RootKeyHandoff::sign(&key(1), g.account_id(), 0, &key(2).public_key()).expect("sign");
        assert_eq!(
            repo.apply_rotation(&gid, &handoff).expect("store"),
            Err(BindingRejected::RotationAccountUnknown {
                account: g.account_id()
            }),
            "an account this group never learned must not be reported as a \
             non-contiguous chain — there is no chain to be discontinuous with"
        );
    }

    /// The other half of the split: a KNOWN account whose handoff starts at the
    /// wrong epoch. Previously untested — the branch existed, but nothing pinned
    /// which epochs it reports, and the fields are the whole point of separating
    /// it from `RotationAccountUnknown`.
    #[test]
    fn a_rotation_from_the_wrong_epoch_names_both_epochs() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let account = g.account_id();

        // Learn the account and roll it to epoch 1.
        let _ = repo
            .apply_link(&gid, &g, &[], &cert_for(&g, &key(1), 5, 0, 0), 0)
            .expect("store")
            .expect("admitted");
        let first = RootKeyHandoff::sign(&key(1), account, 0, &key(2).public_key()).expect("sign");
        repo.apply_rotation(&gid, &first)
            .expect("store")
            .expect("rotated");

        // Replaying the epoch-0 handoff is stale, not unknown.
        assert_eq!(
            repo.apply_rotation(&gid, &first).expect("store"),
            Err(BindingRejected::RotationNotContinuous {
                expected: 1,
                found: 0
            }),
            "a replayed handoff must name the epoch in force and the one it \
             offers, so a stale relay is distinguishable from a forked chain"
        );

        // ...and so is a handoff that skips ahead.
        let skipped =
            RootKeyHandoff::sign(&key(2), account, 3, &key(4).public_key()).expect("sign");
        assert_eq!(
            repo.apply_rotation(&gid, &skipped).expect("store"),
            Err(BindingRejected::RotationNotContinuous {
                expected: 1,
                found: 3
            }),
        );
    }

    #[test]
    fn a_forged_certificate_is_refused() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        // Signed by a key that was never this account's root.
        let cert = cert_for(&g, &key(99), 5, 0, 0);
        assert!(matches!(
            repo.apply_link(&gid, &g, &[], &cert, 0).expect("store"),
            Err(BindingRejected::CredentialInvalid(_))
        ));
    }

    #[test]
    fn a_stale_certificate_cannot_reinstate_a_retired_device_key() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);

        let v0 = cert_for(&g, &key(1), 5, 0, 0);
        let _ = repo.apply_link(&gid, &g, &[], &v0, 0).expect("store");

        // Same device id, fresh keypair, higher epoch — accepted.
        let v1 = DeviceCert::sign(
            &key(1),
            g.account_id(),
            v0.device,
            &key(7).public_key(),
            &KemPublicKey::from([7u8; 32]),
            0,
            1,
        )
        .expect("sign");
        let _ = repo.apply_link(&gid, &g, &[], &v1, 0).expect("store");

        // Replaying the epoch-0 certificate does not roll it back.
        assert_eq!(
            repo.apply_link(&gid, &g, &[], &v0, 0).expect("store"),
            Err(BindingRejected::EpochNotAdvanced {
                offered: 0,
                stored: 1
            })
        );
        let live = repo.live_bindings(&gid).expect("read");
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].sign_pk, key(7).public_key());

        // The rotation moved the signer index with the binding: the retired key
        // resolves to nothing, the new one to the device.
        assert_point_reads_agree(&repo, &gid, &[5, 7], &[v0.device]);
        assert_eq!(
            repo.binding_for_sign_pk(&gid, &key(5).public_key())
                .expect("read"),
            None
        );
    }

    #[test]
    fn a_device_cannot_be_moved_between_accounts() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let alice = genesis_for(1);
        let mallory = genesis_for(2);

        let cert = cert_for(&alice, &key(1), 5, 0, 0);
        let _ = repo.apply_link(&gid, &alice, &[], &cert, 0).expect("store");

        // Mallory certifies the same device id under his own account.
        let hijack = DeviceCert::sign(
            &key(2),
            mallory.account_id(),
            cert.device,
            &key(9).public_key(),
            &KemPublicKey::from([9u8; 32]),
            0,
            1,
        )
        .expect("sign");
        assert_eq!(
            repo.apply_link(&gid, &mallory, &[], &hijack, 0)
                .expect("store"),
            Err(BindingRejected::AccountReassignment)
        );
    }

    #[test]
    fn the_sign_pk_map_answers_every_lookup_the_single_search_does() {
        // The substitutability the batch form exists for. It replaces
        // `binding_for_sign_pk` at the loop call sites, so the two have to agree
        // on every state a signing key can be in — live, revoked, superseded, and
        // never linked at all. Building the map from the raw rows instead of from
        // the filtered list would pass a "live device resolves" check and quietly
        // resurrect the other three.
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);

        // Two accounts, so the map has to key on the signing key rather than
        // collapse to one account's devices.
        let a = genesis_for(1);
        let b = genesis_for(2);
        let _ = repo
            .apply_link(&gid, &a, &[], &cert_for(&a, &key(1), 5, 0, 0), 0)
            .expect("store")
            .expect("admitted");
        let revoked = cert_for(&a, &key(1), 6, 0, 0);
        let _ = repo
            .apply_link(&gid, &a, &[], &revoked, 0)
            .expect("store")
            .expect("admitted");
        repo.apply_revocation(&gid, revoked.device).expect("revoke");
        let _ = repo
            .apply_link(&gid, &b, &[], &cert_for(&b, &key(2), 7, 0, 0), 0)
            .expect("store")
            .expect("admitted");

        // A third account whose root rotates, superseding its epoch-0 device.
        let c = genesis_for(3);
        let _ = repo
            .apply_link(&gid, &c, &[], &cert_for(&c, &key(3), 8, 0, 0), 0)
            .expect("store")
            .expect("admitted");
        let handoff = RootKeyHandoff::sign(&key(3), c.account_id(), 0, &key(4).public_key())
            .expect("sign handoff");
        repo.apply_rotation(&gid, &handoff)
            .expect("store")
            .expect("rotated");

        let map = repo.live_bindings_by_sign_pk(&gid).expect("map");
        for seed in [5u8, 6, 7, 8, 99] {
            let sign_pk = key(seed).public_key();
            assert_eq!(
                map.get(&sign_pk).copied(),
                repo.binding_for_sign_pk(&gid, &sign_pk).expect("search"),
                "the two forms disagree about the device signing with key({seed})"
            );
        }
        // Live: the two unrevoked, unsuperseded devices — not the other three keys.
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn a_device_dropped_by_the_seed_reduction_is_absent_from_the_sign_pk_map() {
        // Why the batch form is a map built over the filtered list and NOT a
        // reverse `sign_pk -> device` key family. The seed rule is a function of
        // the whole stored set: whether this device is live depends on the OTHER
        // devices sharing its HLC seed. A point index could return the loser's
        // row without ever reading the row that beats it, so it would hand
        // authorship to a device the live view excludes.
        let g = genesis_for(1);
        let account = g.account_id();

        // Forge two ids sharing a seed, as
        // `two_devices_sharing_a_replica_seed_converge_on_the_lower_id_either_order`
        // does — the seed is the id's first 16 bytes.
        let mut low = [0u8; 32];
        low[..16].copy_from_slice(&[0xAA; 16]);
        let mut high = low;
        high[31] = 0xFF;
        let (low, high) = (DeviceId::from(low), DeviceId::from(high));
        assert!(low < high);

        let cert_for_device = |device: DeviceId, seed: u8| {
            DeviceCert::sign(
                &key(1),
                account,
                device,
                &key(seed).public_key(),
                &KemPublicKey::from([seed; 32]),
                0,
                0,
            )
            .expect("sign")
        };

        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let _ = repo
            .apply_link(&gid, &g, &[], &cert_for_device(low, 5), 0)
            .expect("store");
        let _ = repo
            .apply_link(&gid, &g, &[], &cert_for_device(high, 6), 0)
            .expect("store");

        let map = repo.live_bindings_by_sign_pk(&gid).expect("map");
        let (winner, loser) = (key(5).public_key(), key(6).public_key());
        assert_eq!(
            map.get(&winner).map(|b| b.device),
            Some(low),
            "the surviving device must be reachable by its signing key"
        );
        assert_eq!(
            map.get(&loser),
            None,
            "the device the seed reduction dropped must not be reachable at all"
        );
        // And the single-lookup form says the same, which is the invariant the
        // hoisted call sites depend on.
        assert_eq!(
            repo.binding_for_sign_pk(&gid, &loser).expect("search"),
            None
        );
    }
}
