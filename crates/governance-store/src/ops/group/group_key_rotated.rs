//! `GroupOp::GroupKeyRotated` apply handler — the carrier for a post-leave rotation.
//!
//! This op mutates no governance state. It exists so the forward-secrecy key
//! rotation has an op to ride on: the rotation itself travels as a plaintext sidecar
//! on the enclosing namespace op (`NamespaceOp::Group { key_rotation, .. }`) and is
//! applied by the namespace layer, which independently verifies that the signer is an
//! admin of the group at the op's cut and that the envelope was wrapped by that same
//! identity.
//!
//! All this apply does is discharge the pending-rotation row the `MemberLeft` apply
//! recorded, so the worklist drains.

use super::context::GroupApplyCtx;
use crate::PendingRotationRepository;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, departed: &PublicKey) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Only an admin may rotate. This mirrors the gate the namespace layer applies to
    // the rotation sidecar itself — if the two disagreed, a non-admin could clear the
    // pending row here while every peer rejected the key that was supposed to
    // discharge it, and the group would believe it had rotated when it had not.
    //
    // `ctx.permissions()` resolves at the op's causal cut, like every other apply
    // gate, so each replica reaches this verdict independently of how much of the DAG
    // it has folded.
    ctx.permissions().require_admin(signer)?;

    // Idempotent: clearing an absent row is a no-op. That is what makes a concurrent
    // double-rotation harmless — if two admins both rotated after the same leave, the
    // second op to apply simply finds nothing left to discharge. Their two keys still
    // converge, because the keyring picks the highest epoch (ties broken by the larger
    // key id) and BOTH keys exclude the leaver.
    PendingRotationRepository::new(store).clear(group_id, departed)?;

    tracing::info!(
        target: "calimero::governance::rotation",
        group_id = %hex::encode(group_id.to_bytes()),
        departed = %departed,
        "group key rotated after member departure; pending rotation discharged"
    );

    Ok(())
}
