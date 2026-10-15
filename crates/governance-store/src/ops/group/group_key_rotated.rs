//! `GroupOp::GroupKeyRotated` apply handler — the carrier for a post-leave rotation.
//!
//! This op mutates no governance state. It exists so the forward-secrecy key
//! rotation has an op to ride on: the rotation itself travels as a plaintext sidecar
//! on the enclosing namespace op (`NamespaceOp::Group { key_rotation, .. }`) and is
//! applied by the namespace layer, which independently verifies that the signer may
//! rotate at the op's cut (an admin, or a TEE naming a member who is gone; see
//! `crate::tee_rotation`) and that the envelope was wrapped by that same identity.
//!
//! All this apply does is discharge the pending-rotation row the departure
//! recorded, so the worklist drains.

use super::context::GroupApplyCtx;
use crate::tee_rotation::{tee_rotation_standing, TeeRotationRefusal};
use crate::{PendingDeviceRotationRepository, PendingRotationRepository};
use calimero_account::{AccountId, DeviceId};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, departed: &AccountId) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // An admin may rotate, and so may a TEE member naming an account that is no
    // longer a member (`crate::tee_rotation`). This mirrors the gate the namespace
    // layer applies to the rotation sidecar itself, through the same predicate: if the
    // two disagreed, a signer could clear the pending row here while every peer
    // rejected the key that was supposed to discharge it, and the group would believe
    // it had rotated when it had not.
    //
    // `ctx.permissions()` resolves at the op's causal cut, like every other apply
    // gate, so each replica reaches this verdict independently of how much of the DAG
    // it has folded; an unfolded cut parks the op.
    let permissions = ctx.permissions();
    if !permissions.is_admin(signer)? {
        match tee_rotation_standing(store, permissions, group_id, signer, departed)? {
            Ok(()) => {}
            Err(TeeRotationRefusal::SignerNotTee) => permissions.require_admin(signer)?,
            Err(refusal) => bail!(refusal),
        }
    }

    // Idempotent: clearing an absent row is a no-op. That is what makes a concurrent
    // double-rotation harmless — if two admins both rotated after the same leave, the
    // second op to apply simply finds nothing left to discharge. Their two keys still
    // converge, because the keyring picks the highest epoch (ties broken by the larger
    // key id) and BOTH keys exclude the leaver.
    PendingRotationRepository::new(store).clear(group_id, departed)?;

    tracing::info!(
        target: "calimero::governance::rotation",
        group_id = %hex::encode(group_id.to_bytes()),
        ?departed,
        "group key rotated after member departure; pending rotation discharged"
    );

    Ok(())
}

/// `GroupOp::GroupKeyRotatedForDevice` apply — discharge the row a device
/// revocation recorded.
///
/// Same shape as [`apply`], and the same admin gate, differing only in which
/// worklist it clears. The distinction matters at the sidecar rather than here:
/// this rotation excludes nobody by name, because the revoked device is already
/// absent from the recipient list (built from live bindings) while the account it
/// belonged to is still a member and must keep its other devices.
pub(crate) fn apply_for_device(ctx: &mut GroupApplyCtx<'_>, device: &DeviceId) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Only an admin may rotate, mirroring the gate the namespace layer applies to
    // the sidecar itself. If the two disagreed, a non-admin could clear the row
    // here while every peer rejected the key meant to discharge it, and the group
    // would believe it had rotated when it had not.
    ctx.permissions().require_admin(signer)?;

    // Idempotent, so two admins discharging the same revocation is harmless: the
    // second finds nothing left to clear, and both keys converge on the keyring's
    // total order with the revoked device excluded from each.
    PendingDeviceRotationRepository::new(store).clear(group_id, device)?;

    tracing::debug!(
        target: "calimero::governance::rotation",
        group_id = %hex::encode(group_id.to_bytes()),
        %device,
        "group key rotated after a device revocation; pending rotation discharged"
    );

    Ok(())
}
