//! Materialized account state for a group: which devices speak for which
//! account, which have been withdrawn, and each account's current root key.
//!
//! This is the governance-path counterpart of the account plane in
//! `calimero-projection`. The rules are the same — they have to be, or two
//! nodes would disagree about who may author — but they are enforced against
//! materialized rows rather than a fold, because that is how the shipping
//! governance wire works.
//!
//! Three rules earn their place here, and each one is a bug that
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
    GroupDeviceBindingValue, GroupRevokedDevice,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use std::collections::BTreeMap;
use thiserror::Error as ThisError;

use crate::collect_keys_with_prefix;

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
    /// The link does not advance the device's rotation epoch, so it would only
    /// let a retired certificate be replayed.
    #[error("device link at epoch {offered} does not supersede the stored {stored}")]
    EpochNotAdvanced {
        /// Epoch the incoming link offers.
        offered: u32,
        /// Epoch already recorded.
        stored: u32,
    },
    /// A rotation for an account this group has never seen, or one that does
    /// not continue the established chain.
    #[error("key rotation does not continue this account's chain")]
    RotationNotContinuous,
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

    /// Is this device's id spent?
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn is_revoked(&self, group: &ContextGroupId, device: DeviceId) -> EyreResult<bool> {
        let key = GroupRevokedDevice::new(group.to_bytes(), *device.as_bytes());
        Ok(self.store.handle().has(&key)?)
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
        let keys = collect_keys_with_prefix(
            self.store,
            GroupDeviceBinding::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_DEVICE_BINDING_PREFIX,
            |k| k.group_id() == gid,
        )?;

        let handle = self.store.handle();
        let mut out = Vec::new();
        for key in keys {
            let Some(value) = handle.get(&key)? else {
                continue;
            };
            let device = DeviceId::from(key.device_id());
            if self.is_revoked(group, device)? {
                continue;
            }
            let account = AccountId::from(value.account);
            if let Some((epoch, _)) = self.account_key(group, account)? {
                if value.key_epoch < epoch {
                    continue;
                }
            }
            out.push(DeviceBinding {
                device,
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
        member: &PublicKey,
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
    /// Re-derived per call, never cached, for the reason in
    /// [`Self::accounts_endorsed_by`]'s sibling below.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn accounts_by_endorsing_member(
        &self,
        group: &ContextGroupId,
    ) -> EyreResult<BTreeMap<PublicKey, Vec<AccountId>>> {
        let gid = group.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupAccountEndorser::new(gid, [0u8; 32], PublicKey::from([0u8; 32])),
            calimero_store::key::GROUP_ACCOUNT_ENDORSER_PREFIX,
            |k| k.group_id() == gid,
        )?;

        let mut out: BTreeMap<PublicKey, Vec<AccountId>> = BTreeMap::new();
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

    /// Every live device of `account` in `group` — the scope-key fan-out unit.
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
                genesis_root_pk: *AsRef::<[u8; 32]>::as_ref(&genesis.root_sign_pk),
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
            return Ok(Err(BindingRejected::RotationNotContinuous));
        };
        let (epoch, root_pk) = (current.epoch, PublicKey::from(current.root_pk));
        if handoff.from_epoch != epoch {
            return Ok(Err(BindingRejected::RotationNotContinuous));
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
                // Carried through untouched: the genesis key is what ties this
                // account to a group member, and a rotation changes the account's
                // signing authority, not who it belongs to.
                genesis_root_pk: current.genesis_root_pk,
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
    ) -> EyreResult<Result<DeviceBinding, BindingRejected>> {
        // Cap before the loop below, not just inside `verify_device_cert`. Each
        // `apply_rotation` costs an Ed25519 verification, and this runs first, so
        // the cap living only in `resolve_root_keys` left the expensive part
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

        if self.is_revoked(group, verified.device)? {
            return Ok(Err(BindingRejected::DeviceRevoked));
        }

        match self.raw_binding(group, verified.device)? {
            Some(existing) => {
                if existing.account != *verified.account.as_bytes() {
                    return Ok(Err(BindingRejected::AccountReassignment));
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

        let key = GroupDeviceBinding::new(group.to_bytes(), *verified.device.as_bytes());
        self.store.handle().put(
            &key,
            &GroupDeviceBindingValue {
                account: *verified.account.as_bytes(),
                sign_pk: *AsRef::<[u8; 32]>::as_ref(&verified.sign_pk),
                kem_pk: *verified.kem_pk.as_bytes(),
                device_epoch: verified.device_epoch,
                key_epoch: verified.key_epoch,
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
    /// tombstones, and per-account root keys.
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
        let accounts = collect_keys_with_prefix(
            self.store,
            GroupAccountKey::new(gid, [0u8; 32]),
            calimero_store::key::GROUP_ACCOUNT_KEY_PREFIX,
            |k| k.group_id() == gid,
        )?;

        let mut handle = self.store.handle();
        for key in bindings {
            handle.delete(&key)?;
        }
        for key in revoked {
            handle.delete(&key)?;
        }
        for key in accounts {
            handle.delete(&key)?;
        }
        Ok(())
    }

    /// Withdraw a device.
    ///
    /// Writes the tombstone **unconditionally**, even for a device this group
    /// has never seen linked: a revocation that arrives before its link must
    /// still win, and the tombstone is what the link consults. Dropping an
    /// early revocation would make the outcome depend on arrival order.
    ///
    /// # Errors
    /// Propagates the store write failure.
    pub fn apply_revocation(&self, group: &ContextGroupId, device: DeviceId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(
            &GroupRevokedDevice::new(group.to_bytes(), *device.as_bytes()),
            &(),
        )?;
        handle.delete(&GroupDeviceBinding::new(
            group.to_bytes(),
            *device.as_bytes(),
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};
    use calimero_account::{sign_device_cert, sign_root_key_handoff, KemPublicKey};
    use calimero_primitives::identity::PrivateKey;

    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    fn genesis_for(seed: u8) -> AccountGenesis {
        AccountGenesis::new(key(seed).public_key(), [seed; 16])
    }

    fn cert_for(
        genesis: &AccountGenesis,
        signer: &PrivateKey,
        device_seed: u8,
        key_epoch: u32,
        device_epoch: u32,
    ) -> DeviceCert {
        let account = genesis.account_id();
        sign_device_cert(
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

    #[test]
    fn a_valid_link_is_recorded_and_reported_live() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        let bound = repo
            .apply_link(&gid, &g, &[], &cert)
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
                .apply_link(&gid, &g, &[], &cert_for(&g, &key(1), seed, 0, 0))
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
            repo.apply_link(&gid, &g, &[], &cert).expect("store"),
            Err(BindingRejected::DeviceRevoked)
        );
        assert!(repo.live_bindings(&gid).expect("read").is_empty());
    }

    #[test]
    fn revocation_after_a_link_removes_the_binding() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let cert = cert_for(&g, &key(1), 5, 0, 0);

        let _ = repo.apply_link(&gid, &g, &[], &cert).expect("store");
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
            let _ = repo.apply_link(&gid, &g, &[], &cert).expect("store");
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
            let _ = repo.apply_link(&gid, &g, &[], &cert).expect("store");
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
            sign_device_cert(
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
                let _ = repo.apply_link(&gid, &g, &[], cert).expect("store");
            }
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
            sign_device_cert(
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
            sign_root_key_handoff(&key(1), account, 0, &key(2).public_key()).expect("sign");

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
            let _ = repo.apply_link(&gid, &g, &[], &low_cert).expect("store");
            repo.apply_rotation(&gid, &handoff)
                .expect("store")
                .expect("rotated");
            for step in order {
                match step {
                    Step::LinkLow => {
                        let _ = repo.apply_link(&gid, &g, &[handoff], &low_cert).expect("s");
                    }
                    Step::LinkHigh => {
                        let _ = repo
                            .apply_link(&gid, &g, &[handoff], &high_cert)
                            .expect("s");
                    }
                    Step::LinkDoomed => {
                        let _ = repo.apply_link(&gid, &g, &[handoff], &doomed).expect("s");
                    }
                    Step::Revoke => repo.apply_revocation(&gid, doomed.device).expect("revoke"),
                }
            }
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
            .apply_link(&gid, &g, &[], &cert_for(&g, &key(1), 5, 0, 0))
            .expect("store")
            .expect("admitted");
        let handoff =
            sign_root_key_handoff(&key(1), account, 0, &key(2).public_key()).expect("sign");
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
            .apply_link(&gid, &g, &[handoff], &cert_for(&g, &key(2), 6, 1, 0))
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
            let _ = repo.apply_link(&gid, &g, &[], cert).expect("store");
            let _ = repo.apply_link(&other, &g, &[], cert).expect("store");
        }
        repo.apply_revocation(&gid, doomed.device).expect("revoke");
        repo.apply_revocation(&other, doomed.device)
            .expect("revoke");

        repo.clear_all_for_group(&gid).expect("clear");

        assert!(repo.live_bindings(&gid).expect("read").is_empty());
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
        assert!(repo.is_revoked(&other, doomed.device).expect("read"));
        assert!(repo
            .account_key(&other, g.account_id())
            .expect("read")
            .is_some());

        // Idempotent.
        repo.clear_all_for_group(&gid).expect("clear again");
    }

    #[test]
    fn a_rotation_for_an_unknown_account_is_refused() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let g = genesis_for(1);
        let handoff =
            sign_root_key_handoff(&key(1), g.account_id(), 0, &key(2).public_key()).expect("sign");
        assert_eq!(
            repo.apply_rotation(&gid, &handoff).expect("store"),
            Err(BindingRejected::RotationNotContinuous)
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
            repo.apply_link(&gid, &g, &[], &cert).expect("store"),
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
        let _ = repo.apply_link(&gid, &g, &[], &v0).expect("store");

        // Same device id, fresh keypair, higher epoch — accepted.
        let v1 = sign_device_cert(
            &key(1),
            g.account_id(),
            v0.device,
            &key(7).public_key(),
            &KemPublicKey::from([7u8; 32]),
            0,
            1,
        )
        .expect("sign");
        let _ = repo.apply_link(&gid, &g, &[], &v1).expect("store");

        // Replaying the epoch-0 certificate does not roll it back.
        assert_eq!(
            repo.apply_link(&gid, &g, &[], &v0).expect("store"),
            Err(BindingRejected::EpochNotAdvanced {
                offered: 0,
                stored: 1
            })
        );
        let live = repo.live_bindings(&gid).expect("read");
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].sign_pk, key(7).public_key());
    }

    #[test]
    fn a_device_cannot_be_moved_between_accounts() {
        let store = test_store();
        let gid = test_group_id();
        let repo = AccountBindingRepository::new(&store);
        let alice = genesis_for(1);
        let mallory = genesis_for(2);

        let cert = cert_for(&alice, &key(1), 5, 0, 0);
        let _ = repo.apply_link(&gid, &alice, &[], &cert).expect("store");

        // Mallory certifies the same device id under his own account.
        let hijack = sign_device_cert(
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
            repo.apply_link(&gid, &mallory, &[], &hijack)
                .expect("store"),
            Err(BindingRejected::AccountReassignment)
        );
    }
}
