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
use crate::authorizer::AtCutMembershipPath;
use crate::membership::MembershipPath;
use crate::{AccountBindingRepository, BindingRejected, MembershipRepository};
use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff};
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

    // The one policy gate: **is this account's root key a member of the group?**
    //
    // Membership rows are keyed by member key, and an `AccountId` is a one-way
    // hash, so the account cannot be looked up in them directly. What ties the
    // two together is the genesis: an account whose epoch-0 root key is a
    // granted member key is that member's account, because only the holder of
    // that key's private half can sign certificates under it.
    //
    // Anyone may *construct* a genesis naming someone else's member key — the
    // genesis is public data — and such an account passes this gate. It gains
    // them nothing: enrolling a device into it requires signing a certificate
    // with the root key, which they do not hold. The gate keeps strangers from
    // writing link rows for accounts unrelated to the group; the signature
    // keeps them from enrolling into accounts that are not theirs.
    //
    // Resolved at the op's causal cut, like every other apply-time authority
    // question. Reading live membership rows here would decide against whatever
    // this replica has folded so far, so a node that had already applied a
    // concurrent removal of the root-key holder would refuse a link its peers
    // recorded — and since a refusal writes nothing while the op still occupies
    // its place in the DAG, the two would disagree about who may author with no
    // later op to reconcile them.
    if !root_key_is_member(ctx, &genesis.root_sign_pk)? {
        log_refusal(&group_id, "device link", &BindingRejected::AccountNotMember);
        return Ok(());
    }
    let store = ctx.store();

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
/// **A group admin at the op's cut, and nobody else.** A revocation is terminal
/// — the `DeviceId` is spent for good — so an ungated one is a permanent denial
/// of service any member could inflict on any other. Membership in a group is
/// not authority over other members' devices.
///
/// The account owner revoking *their own* device without an admin is the case
/// this deliberately does **not** cover yet, even though it is the motivating one
/// (a lost laptop, where the owner may be the only person who knows). It cannot
/// be gated on folded state: "is the signer this account's current root key"
/// depends on which rotations this replica has folded, so two replicas would
/// disagree about one op. Doing it right means the op carries a **root-signed
/// revocation proof**, self-certifying exactly as `DeviceCert` is — a wire
/// addition that belongs with the CLI that mints it (phase F), not a gate that
/// looks correct and diverges.
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

    // `?` rather than a swallowed `false`: `is_admin` returns
    // `AuthorityUndecidable` when the op's cut is real but unfolded here, and that
    // must park the apply for retry rather than be read as "not an admin". A
    // genuine non-admin is a deterministic refusal every replica reaches
    // identically, so it records nothing and returns `Ok` — erroring would stall
    // the apply forever on an op that can never succeed.
    if !ctx.permissions().is_admin(ctx.signer())? {
        tracing::warn!(
            group_id = ?group_id,
            signer = %ctx.signer(),
            account = %account,
            device = %device,
            "account device unlink not recorded: signer is not an admin at the op's cut"
        );
        return Ok(());
    }

    AccountBindingRepository::new(ctx.store()).apply_revocation(&group_id, *device)?;

    tracing::info!(
        group_id = ?group_id,
        account = %account,
        device = %device,
        "account device unlinked"
    );
    Ok(())
}

/// Is `root_pk` a member of this group at the op's causal cut?
///
/// Direct or inherited both count: an account whose root key reaches the group
/// through an Open-subgroup chain holds every right its devices would gain, which
/// is the whole basis for the link needing no admin.
///
/// The live resolver is used only when the projection has no cut to resolve
/// against at all, and `ensure_live_fallback_is_sound` is what separates that
/// from an unfolded cut — where falling back would answer against a different
/// cut and let two replicas decide the same op differently.
fn root_key_is_member(
    ctx: &GroupApplyCtx<'_>,
    root_pk: &calimero_primitives::identity::PublicKey,
) -> EyreResult<bool> {
    let path = match ctx.projection_membership_path(root_pk) {
        Some(projected) => projected,
        None => {
            ctx.ensure_live_fallback_is_sound(root_pk)?;
            match MembershipRepository::new(ctx.store()).check_path(ctx.group_id(), root_pk)? {
                MembershipPath::None => AtCutMembershipPath::None,
                MembershipPath::Direct => AtCutMembershipPath::Direct,
                MembershipPath::Inherited { .. } => AtCutMembershipPath::Inherited,
            }
        }
    };
    Ok(path != AtCutMembershipPath::None)
}

/// `GroupOp::AccountKeysRotated` — roll an account's root key.
///
/// Raises the account's epoch, after which certificates signed by any
/// superseded key stop being honoured. The handoff's own signature is verified
/// by the repository against the key currently in force, so a rotation cannot
/// be forged by anyone who does not hold that key.
///
/// Deliberately ungated, unlike its two siblings, and for a reason rather than by
/// omission: the handoff is self-certifying against state the group already holds.
/// A rotation for an account this group has never learned is refused outright
/// (`RotationNotContinuous`, since there is no root key to continue from), and the
/// only way the group learns an account is through a link that already passed the
/// membership gate. So relaying someone else's rotation writes nothing an
/// attacker chose, and gating it on the relayer would only break legitimate
/// re-gossip.
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
