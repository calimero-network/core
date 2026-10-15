//! Who besides an admin may pay a rotation debt: a TEE, for a member who is gone.
//!
//! A departure leaves a `PendingRotation` row, and paying it is a
//! `GroupKeyRotated { departed }` carrying a fresh key as its sidecar. An org
//! always runs a TEE replica, which holds the key of every group it is a member
//! of, so the rotation need not wait for an admin node to come online.
//!
//! # The rule
//!
//! A `GroupKeyRotated { departed }` for group `D` signed by an account that is
//! not an admin of `D` is accepted iff, at the op's cut:
//!
//! * the signer is an effective member of `D` whose role `is_tee()`, the TEE
//!   role read at its namespace root row the way
//!   [`executor_standing`](crate::warrant_admission::executor_standing) reads it
//!   (`namespace_tee_role`). Both TEE roles: a replica pays a debt as a relay
//!   does, because a rotation is key hygiene, not a state write;
//! * `departed` is NOT an effective member of `D` (deny-list and exit aware).
//!
//! That is all a TEE gains. It cannot add, remove or re-role anyone, and it
//! cannot rotate a current member out: the only account a TEE rotation may name
//! is one already gone. The recipients of the new key are the publisher's
//! choice and no peer checks them, exactly as for an admin; a member a rotation
//! leaves out is still a live member and pulls the key.
//!
//! # One rule, three callers
//!
//! The op handler (`ops/group/group_key_rotated.rs`), the rotation sidecar
//! gate (`NamespaceGovernance::apply_key_rotation`) and the publisher
//! (`ensure_rotation_is_publishable_for`) all ask [`tee_rotation_standing`]. If
//! the handler and the sidecar disagreed, a node could clear the pending row
//! while every peer threw the key away.
//!
//! # Decided at the cut
//!
//! The reads are the op's cut through [`AdmissionCut`]: the projection folded
//! to the op's parents when it is folded here, the live rows only when there is
//! no cut to contradict them, and `ApplyError::AuthorityUndecidable` when the
//! cut is real but not folded. Never a live guess. So a TEE's rotation and an
//! admin's concurrent one converge as two admins' rotations do: both exclude
//! the departed member, and the keyring's total order picks one.

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use thiserror::Error as ThisError;

use crate::permission_checker::PermissionChecker;
use crate::warrant_admission::{namespace_tee_role, LiveReads};
use crate::{key_covering_group, GroupKeyring, StandingReads};

/// Why a rotation by a non-admin is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ThisError)]
pub enum TeeRotationRefusal {
    /// The signer is not a TEE member of the group.
    #[error("the rotation's signer is neither an admin nor a TEE member of the group")]
    SignerNotTee,
    /// The account the rotation names is still a member of the group.
    #[error(
        "a TEE may rotate a group's key only for a member who is gone, and {0} is still a member"
    )]
    DepartedIsMember(AccountId),
}

/// Whether `signer` may rotate `group_id`'s key out from under `departed` as a
/// TEE, decided at the cut `permissions` carries. See the module docs.
///
/// `Ok(Ok(()))` accepts; `Ok(Err(_))` refuses for the reason given. Admin
/// standing is the caller's question and is not asked here.
///
/// # Errors
/// `ApplyError::AuthorityUndecidable` when the cut is real but not folded here
/// (the caller parks and retries), or a store failure.
pub(crate) fn tee_rotation_standing(
    store: &Store,
    permissions: &PermissionChecker<'_>,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    departed: &AccountId,
) -> EyreResult<Result<(), TeeRotationRefusal>> {
    let at_cut = permissions.admission_cut().reads(group_id, signer)?;
    let live = LiveReads::new(store);
    let reads: &dyn StandingReads = at_cut.as_deref().unwrap_or(&live);
    let Some(signer_account) = permissions.account_for_signer(signer)? else {
        return Ok(Err(TeeRotationRefusal::SignerNotTee));
    };
    standing(store, reads, group_id, &signer_account, departed)
}

fn standing(
    store: &Store,
    reads: &dyn StandingReads,
    group_id: &ContextGroupId,
    signer: &AccountId,
    departed: &AccountId,
) -> EyreResult<Result<(), TeeRotationRefusal>> {
    let Some((role, role_group)) = reads.effective_role(group_id, signer)? else {
        return Ok(Err(TeeRotationRefusal::SignerNotTee));
    };
    let (role, _) = namespace_tee_role(store, reads, group_id, signer, role, role_group)?;
    if !role.is_tee() {
        return Ok(Err(TeeRotationRefusal::SignerNotTee));
    }
    if reads.effective_role(group_id, departed)?.is_some() {
        return Ok(Err(TeeRotationRefusal::DepartedIsMember(*departed)));
    }
    Ok(Ok(()))
}

/// Whether this node, signing as `rotator`, should pay the rotation `departed`
/// left owed in `group_id` as a TEE: the rule above on its live rows, and the
/// node holds the key the op will be sealed under. A TEE that does not hold it
/// leaves the debt to a node that does; publishing anyway would apply the op
/// locally, clearing the row, and then send nothing.
///
/// # Errors
/// A store failure.
pub fn tee_may_pay_rotation(
    store: &Store,
    group_id: &ContextGroupId,
    rotator: &PublicKey,
    departed: &AccountId,
) -> EyreResult<bool> {
    let checker = PermissionChecker::new(store, *group_id);
    if tee_rotation_standing(store, &checker, group_id, rotator, departed)?.is_err() {
        return Ok(false);
    }
    let covering = key_covering_group(store, group_id)?;
    Ok(GroupKeyring::new(store, covering)
        .load_current_key()?
        .is_some())
}
