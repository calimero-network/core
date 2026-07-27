//! Apply handlers for the account plane: device link, device revocation, and
//! account root-key rotation.
//!
//! The three share a file because they share one invariant, and separating them
//! would let it drift: **every one of them must be idempotent and
//! order-independent**. The apply pipeline re-runs a mutation before the op-log
//! dedup gate fires, and governance ops arrive in whatever order the DAG hands
//! them over, so a handler that only works on first application or only in
//! causal order will silently diverge two replicas.
//!
//! [`crate::AccountBindingRepository`] carries the actual rules; these handlers
//! are the authorization gate plus the call.

use super::context::GroupApplyCtx;
use crate::{AccountBindingRepository, BindingRejected, MembershipRepository};
use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff};
use calimero_op_adapter::legacy_account_id;
use eyre::Result as EyreResult;

/// `GroupOp::AccountDeviceLinked` — record a device as speaking for an account.
///
/// Authorization is one question: **is the account already a member of this
/// group?** If it is, the link grants nothing the account did not already hold,
/// so no admin action is required and the device authors this for itself. If it
/// is not, a stranger could otherwise write unlimited link rows into a group
/// they have no relationship with.
///
/// Note what is *not* checked: whether the signer is the device being enrolled.
/// The certificate is root-signed and the credential is self-certifying, so a
/// replayed link records a binding the account had already authorized — it
/// gains an attacker nothing, and refusing it would break legitimate re-gossip
/// of the same op.
pub(crate) fn apply_device_linked(
    ctx: &mut GroupApplyCtx<'_>,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) -> EyreResult<()> {
    let group_id = *ctx.group_id();
    let store = ctx.store();

    // The one policy gate. Membership rows are still keyed by member key while
    // the re-key onto `AccountId` lands, so the comparison maps the member list
    // FORWARD into account space rather than trying to invert the account —
    // an account is a one-way hash, and caching the reverse would not survive a
    // rebuild from the op log.
    let is_member = MembershipRepository::new(store)
        .list(&group_id, 0, usize::MAX)?
        .into_iter()
        .any(|(member, _)| legacy_account_id(&member) == cert.account);
    if !is_member {
        log_refusal(&group_id, "device link", &BindingRejected::AccountNotMember);
        return Ok(());
    }

    let outcome =
        AccountBindingRepository::new(store).apply_link(&group_id, genesis, chain, cert)?;

    match outcome {
        Ok(binding) => {
            tracing::info!(
                group_id = ?group_id,
                account = %binding.account,
                device = %binding.device,
                device_epoch = binding.device_epoch,
                "account device linked"
            );
        }
        Err(reason) => {
            // Deterministically inadmissible: every replica reaches the same
            // verdict from the same rows, so declining to record it keeps the
            // group convergent. The op still occupies its place in the DAG.
            log_refusal(&group_id, "device link", &reason);
        }
    }
    Ok(())
}

/// `GroupOp::AccountDeviceUnlinked` — withdraw a device.
///
/// Either the account withdraws its own device (the lost-laptop case, which
/// must not need an admin — the owner may be the only one who knows) or a group
/// admin ejects it (the compromised-member case, which the account may be
/// unable or unwilling to handle).
///
/// Applied unconditionally once authorized, including for a device this group
/// has never seen linked: the tombstone is what a later link consults, so
/// dropping an early revocation would make the outcome depend on arrival order.
pub(crate) fn apply_device_unlinked(
    ctx: &mut GroupApplyCtx<'_>,
    account: &AccountId,
    device: &DeviceId,
) -> EyreResult<()> {
    let group_id = *ctx.group_id();
    let store = ctx.store();

    AccountBindingRepository::new(store).apply_revocation(&group_id, *device)?;

    tracing::info!(
        group_id = ?group_id,
        account = %account,
        device = %device,
        "account device unlinked"
    );
    Ok(())
}

/// `GroupOp::AccountKeysRotated` — roll an account's root key.
///
/// Raises the account's epoch, after which certificates signed by any
/// superseded key stop being honoured. The handoff's own signature is verified
/// by the repository against the key currently in force, so a rotation cannot
/// be forged by anyone who does not hold that key.
pub(crate) fn apply_keys_rotated(
    ctx: &mut GroupApplyCtx<'_>,
    handoff: &RootKeyHandoff,
) -> EyreResult<()> {
    let group_id = *ctx.group_id();
    let store = ctx.store();

    match AccountBindingRepository::new(store).apply_rotation(&group_id, handoff)? {
        Ok(()) => {
            tracing::info!(
                group_id = ?group_id,
                account = %handoff.account,
                from_epoch = handoff.from_epoch,
                "account root key rotated"
            );
        }
        Err(reason) => log_refusal(&group_id, "key rotation", &reason),
    }
    Ok(())
}

/// Log an inadmissible credential at `warn`.
///
/// Not an error: the op is validly signed and belongs in the DAG, it simply
/// records nothing. Returning `Err` would stall the apply and burn no nonce,
/// leaving the node retrying an op that can never succeed.
fn log_refusal(
    group_id: &calimero_context_config::types::ContextGroupId,
    what: &str,
    reason: &BindingRejected,
) {
    tracing::warn!(
        group_id = ?group_id,
        %reason,
        "account {what} not recorded"
    );
}
