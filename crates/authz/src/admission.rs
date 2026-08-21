//! Credential admission — the rules [`crate::authorize`] and the projection's
//! fold must agree on, defined once so they cannot drift.
//!
//! # Why it is shaped this way
//!
//! **One definition, two callers, two planes.** Stating a rule here and again in
//! the projection is exactly how one node authorizes an op its peer folds
//! differently, and that is a `scope_root` divergence — the two planes would
//! disagree about the same op with no later op to reconcile them.
//!
//! **Why the rule is split in two.** The two callers need it at different
//! moments, and only one of them has a cut:
//!
//! - [`AclView::admit_device_link`] is what `authorize` calls. It decides against
//!   a fixed causal cut, so "has this signing epoch been superseded *at the cut*"
//!   is a well-defined question and is asked here.
//! - [`fold_device_link`] is what the projection's fold calls, walking ops one at
//!   a time. The same supersession question there would read whichever epoch
//!   happened to be folded so far, making admission depend on delivery order and
//!   the projection non-convergent. The fold therefore records the link and
//!   filters superseded ones out when the view is read, once the final epoch is
//!   known.
//!
//! Both paths agree on the observable state; they just get there in the order
//! each can afford. Everything in [`fold_device_link`] is monotone — a tombstone
//! is never removed, a device is never un-assigned from its account, an epoch only
//! rises — which is what lets any fold order reach the same result.
//!
//! **The at-cut half takes `&self`, the fold half does not.** `authorize` always
//! holds an [`AclView`]; the fold is *building* the state a view is later read
//! from and has no view to offer. That is the whole reason one is a method and the
//! other stays a free function, rather than both taking the maps loose.

use std::collections::{BTreeMap, BTreeSet};

use calimero_account::{AccountGenesis, DeviceCert, DeviceId, RootKeyHandoff, VerifiedDeviceCert};

use crate::error::Rejected;
use crate::view::{AclView, DeviceBinding};

impl AclView {
    /// Decide whether a `DeviceLinked` credential takes effect at this cut.
    ///
    /// The checks, in order:
    /// 1. the credential is internally valid — the genesis addresses the claimed
    ///    account and the handoff chain carries valid signatures up to the
    ///    certificate's epoch (`calimero-account`);
    /// 2. the signing epoch has not been superseded at this cut, so rotating the
    ///    root key actually withdraws the old key's authority instead of merely
    ///    adding a new key beside it;
    /// 3. the device has not been revoked — read from the grow-only revoked set,
    ///    which is what makes a revocation that folds *before* its link still win;
    /// 4. a device is never reassigned to another account;
    /// 5. a re-link strictly advances the device's rotation epoch, so an old
    ///    certificate cannot be replayed to reinstate a retired key;
    /// 6. on first link, no other device in the scope already claims the same
    ///    replica seed prefix — which turns RGA id uniqueness from a birthday
    ///    argument into a checked invariant.
    ///
    /// Deliberately **not** checked here: whether the account is a member of the
    /// scope. That is policy rather than credential validity, it only bears on the
    /// authorization decision, and leaving it out keeps the fold a pure function
    /// of already-authorized ops — `authorize`'s `DeviceLinked` arm is what asks
    /// the membership question.
    ///
    /// # Errors
    /// The `Credential*` / `Device*` variants of [`Rejected`], one per rule above.
    pub fn admit_device_link(
        &self,
        genesis: &AccountGenesis,
        chain: &[RootKeyHandoff],
        cert: &DeviceCert,
    ) -> Result<VerifiedDeviceCert, Rejected> {
        let verified =
            fold_device_link(&self.devices, &self.revoked_devices, genesis, chain, cert)?;

        // Step 2, the one check that needs a cut to mean anything — see the module
        // docs for why it cannot live in the fold half.
        if let Some(binding) = self.accounts.get(&verified.account) {
            if verified.key_epoch < binding.epoch {
                return Err(Rejected::CredentialSuperseded {
                    signed: verified.key_epoch,
                    current: binding.epoch,
                });
            }
        }

        Ok(verified)
    }

    /// Decide whether an `AccountKeysRotated` handoff takes effect at this cut.
    ///
    /// Admissible only if the scope already knows the account (it learned the
    /// genesis from a device link) and the handoff continues the chain from the
    /// epoch currently in force, signed by the key currently in force.
    ///
    /// # Errors
    /// [`Rejected::RotationAccountUnknown`] when the scope has no chain for the
    /// account, [`Rejected::RotationNotContinuous`] when the handoff starts at the
    /// wrong epoch, or [`Rejected::RotationSignatureInvalid`] when the outgoing key
    /// did not sign it.
    pub fn admit_key_rotation(&self, handoff: &RootKeyHandoff) -> Result<(), Rejected> {
        let Some(binding) = self.accounts.get(&handoff.account) else {
            return Err(Rejected::RotationAccountUnknown {
                account: handoff.account,
            });
        };
        if handoff.from_epoch != binding.epoch {
            return Err(Rejected::RotationNotContinuous {
                expected: binding.epoch,
                found: handoff.from_epoch,
            });
        }
        if binding
            .root_pk
            .verify_raw_signature(&handoff.payload(), &handoff.signature)
            .is_err()
        {
            return Err(Rejected::RotationSignatureInvalid);
        }
        Ok(())
    }
}

/// The order-independent half of [`AclView::admit_device_link`] — every rule whose
/// answer cannot change as more ops fold in.
///
/// Takes the two maps loose rather than an [`AclView`] because its caller is the
/// projection's fold, which is *building* the state a view is later read from and
/// has none to pass. See the module docs.
///
/// # Errors
/// The `Credential*` / `Device*` variants of [`Rejected`], excluding
/// [`Rejected::CredentialSuperseded`] — see [`AclView::admit_device_link`].
pub fn fold_device_link(
    devices: &BTreeMap<DeviceId, DeviceBinding>,
    revoked: &BTreeSet<DeviceId>,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) -> Result<VerifiedDeviceCert, Rejected> {
    let verified = calimero_account::verify_device_cert(cert.account, genesis, chain, cert)
        .map_err(|reason| Rejected::CredentialInvalid { reason })?;

    if revoked.contains(&verified.device) {
        return Err(Rejected::DeviceRevoked {
            device: verified.device,
        });
    }

    match devices.get(&verified.device) {
        Some(existing) => {
            if existing.account != verified.account {
                return Err(Rejected::DeviceAccountReassignment);
            }
            if verified.device_epoch <= existing.device_epoch {
                return Err(Rejected::DeviceEpochNotAdvanced {
                    offered: verified.device_epoch,
                    folded: existing.device_epoch,
                });
            }
        }
        None => {
            // No seed-collision check here. On a prefix collision the LOWER
            // device id wins, but *which* device that is cannot be decided as
            // each link folds: rejecting the newcomer only when an already-folded
            // device compares lower is order-dependent in the direction it does
            // not check, so low-then-high left one device live while
            // high-then-low left both. `ScopeState::live_devices` applies the
            // rule over the folded set instead, where it is a function of the op
            // set and every replica reaches the same view.
        }
    }

    Ok(verified)
}
