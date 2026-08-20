//! The decision itself.
//!
//! Two stages, in this order: does the key that signed this op currently speak
//! for the account it claims, and does that account hold the authority the
//! payload needs. Everything else in the crate exists to answer one of those two
//! questions.
//!
//! # Why it is shaped this way
//!
//! **Stage one is resolved from the cut, never from live store.** `Op::verify`
//! already proved the signature genuine; that is integrity, not authority. Only
//! the cut knows which links and revocations are in force, and a verdict that
//! depended on receiver state would let two nodes disagree about the same op and
//! diverge on `scope_root`.
//!
//! **Two payloads skip stage one, and only those two.** `DeviceLinked` is the op
//! that *establishes* a binding, so it cannot be asked to present one; its own
//! admission rules stand in. `MemberJoinedWithDevice` carries its credential for
//! the same reason, at the one cut where the binding provably cannot have folded
//! yet — the op establishing it is this one. Both then make the possession check
//! in [`check_op_is_the_certified_device`], which is the part that keeps the
//! exemption from becoming a hole.
//!
//! **The data arms inline their masks instead of calling
//! [`required_mask_for`].** Each arm carries its literal required mask, so there
//! is no `Option` to unwrap and no fallback arm that could silently deny — or
//! panic — if the arms ever drift. The cost is that the payload→mask mapping
//! exists twice; `required_mask_for_agrees_with_the_mask_authorize_enforces` is
//! what keeps the copies honest, since a comment cannot.
//!
//! **The `MemberJoinedWithDevice` arm fails closed for an ordinary self-service
//! join, on purpose.** A join's real warrant is the admin-signed invitation, and
//! `OpPayload` has nowhere to put one — so this gate cannot decide a join in the
//! affirmative, and a gate that cannot decide must refuse rather than guess. A
//! genuine non-admin joiner is therefore rejected. That is survivable only
//! because the live governance apply is what actually gates a join today;
//! `authorize` has no caller on the join path.
//!
//! **Do not wire `authorize` into the join path until the invitation is carried
//! in the payload** — doing so would refuse every self-service join. The
//! credential half IS decidable from the op alone, so it is decided here rather
//! than deferred: without it the arm would accept whatever bytes the payload
//! carried, which is a worse trap than refusing.

use calimero_account::{AccountId, VerifiedDeviceCert};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Op, OpPayload};
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

use crate::error::Rejected;
use crate::view::AclView;

/// The capability a **data** op requires of its author, or `None` for a
/// non-data op (whose authority is decided by ownership/admin, not a mask).
///
/// Returning `None` rather than `OpMask::NONE` is deliberate: the empty mask is
/// contained by *every* mask, so a `NONE` requirement fed to [`AclView::may`]
/// would authorize anyone — a footgun if a non-data payload ever reached a
/// `may` check. `None` makes that misuse impossible to express.
///
/// `authorize` does not call this — see the module docs — so it is a helper for
/// callers outside the decision, held to the same answer by test.
#[must_use]
pub fn required_mask_for(payload: &OpPayload) -> Option<OpMask> {
    match payload {
        OpPayload::Put { .. } => Some(OpMask::WRITE),
        OpPayload::Delete { .. } => Some(OpMask::DELETE),
        _ => None,
    }
}

/// `Ok` iff `author` holds `required` on `entity` (the data-plane check).
fn check_data(
    acl_at_cut: &AclView,
    author: &AccountId,
    entity: Id,
    required: OpMask,
) -> Result<(), Rejected> {
    if acl_at_cut.may(author, entity, required) {
        Ok(())
    } else {
        Err(Rejected::NotPermitted { entity, required })
    }
}

/// The possession half of a credential-bearing op: was it authored by the very
/// device the certificate grants, on behalf of the very account it grants to?
///
/// Both credential arms need exactly this, and needed it identically — written
/// out twice, the two could come to disagree about which of `device`, `device_key`
/// and `account` must match, and the weaker copy would be the one an attacker
/// used. Without it, anyone who observed a certificate could replay it and act on
/// the real device's behalf; requiring possession makes the op an act OF the
/// device rather than an assertion about it.
fn check_op_is_the_certified_device(
    op: &Op,
    verified: &VerifiedDeviceCert,
) -> Result<(), Rejected> {
    if op.authorship.device_key != verified.sign_pk || op.authorship.device != verified.device {
        return Err(Rejected::DeviceKeyStale {
            device: verified.device,
        });
    }
    // And that the account it acts as is the one the certificate grants to,
    // rather than any account the payload cared to name.
    if op.author() != verified.account {
        return Err(Rejected::DeviceAccountMismatch {
            device: verified.device,
            bound: verified.account,
            claimed: op.author(),
        });
    }
    Ok(())
}

/// Authorize `op` against `acl_at_cut` — the [`AclView`] resolved at
/// `op.parents`. The **only** causal-auth decision in the unified model.
///
/// # Errors
/// Returns the plane-specific [`Rejected`] reason when the author lacks the
/// authority the op's payload requires.
pub fn authorize(op: &Op, acl_at_cut: &AclView) -> Result<(), Rejected> {
    // Stage one — see the module docs for why these two payloads are exempt.
    if !matches!(
        op.payload,
        OpPayload::DeviceLinked { .. } | OpPayload::MemberJoinedWithDevice { .. }
    ) {
        check_device_speaks_for_author(op, acl_at_cut)?;
    }

    // Stage two: does that account hold the authority this payload needs?
    match &op.payload {
        OpPayload::Put { entity, .. } => {
            check_data(acl_at_cut, &op.author(), *entity, OpMask::WRITE)
        }
        OpPayload::Delete { entity } => {
            check_data(acl_at_cut, &op.author(), *entity, OpMask::DELETE)
        }
        OpPayload::SetWriters { object, .. } => {
            if acl_at_cut.is_owner(&op.author(), *object) {
                Ok(())
            } else {
                Err(Rejected::NotOwner)
            }
        }
        OpPayload::MemberAdded { group, .. } | OpPayload::MemberRemoved { group, .. } => {
            if acl_at_cut.is_group_admin(&op.author(), *group) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        // FAILS CLOSED for an ordinary self-service join — see the module docs
        // before giving this arm a caller.
        OpPayload::MemberJoinedWithDevice {
            group,
            genesis,
            chain,
            cert,
            ..
        } => {
            let verified = acl_at_cut.admit_device_link(genesis, chain, cert)?;
            // A join could not make this check while its authorship was derived
            // from the signing key: the device it named was fiction, so comparing
            // against it refused every honest join. The op now names the device its
            // certificate grants, which makes the question decidable here.
            check_op_is_the_certified_device(op, &verified)?;
            // `is_scope_member(verified.account)` still cannot be required — the
            // whole point of a join is that the account is not a member yet.
            if acl_at_cut.is_group_admin(&op.author(), *group) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        OpPayload::SubgroupVisibilitySet { scope, .. } => {
            // Visibility is a property of the subgroup; its admin sets it.
            if acl_at_cut.is_group_admin(&op.author(), ContextGroupId::from(*scope.as_bytes())) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        OpPayload::AdminChanged { .. }
        | OpPayload::PolicyUpdated { .. }
        | OpPayload::SubgroupCreated { .. }
        | OpPayload::SubgroupReparented { .. }
        | OpPayload::SubgroupDeleted { .. } => {
            if acl_at_cut.is_root_admin(&op.author()) {
                Ok(())
            } else {
                Err(Rejected::NotRootAdmin)
            }
        }
        // Capability changes are an admin action on the target group.
        OpPayload::DefaultCapabilitiesSet { group, .. }
        | OpPayload::MemberCapabilitySet { group, .. } => {
            if acl_at_cut.is_group_admin(&op.author(), *group) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        // A graph-only node mutates nothing, so there is nothing to authorize.
        OpPayload::Noop => Ok(()),

        // ---- account plane ----
        OpPayload::DeviceLinked {
            genesis,
            chain,
            cert,
        } => {
            let verified = acl_at_cut.admit_device_link(genesis, chain, cert)?;
            check_op_is_the_certified_device(op, &verified)?;
            // The one policy gate: a device may only link itself into a scope
            // its account already belongs to. This is what makes linking cheap
            // and safe at once — the account already holds every right the
            // device gains, so the link is no privilege escalation and needs no
            // admin action. It is also the only thing between a stranger and an
            // unbounded supply of link ops in this scope.
            if !acl_at_cut.is_scope_member(&verified.account) {
                return Err(Rejected::AccountNotMember);
            }
            Ok(())
        }
        OpPayload::DeviceRevoked { account, device } => {
            // Either the account withdraws its own device (the lost-laptop
            // case, which needs no admin), or a scope admin ejects it (the
            // compromised-member case, which the account may be unable or
            // unwilling to handle itself).
            //
            // Self-service requires a folded binding that *proves* the device
            // speaks for the author. Trusting the payload's own `account` field
            // when no binding exists made the claim unfalsifiable: any linked
            // member could name its own account beside an arbitrary unbound
            // device id and be authorized. Because a tombstone is terminal —
            // and because an early revocation deliberately beats the link it
            // withdraws — that spent the id for good, so an attacker could
            // permanently lock out a device it had no relationship to simply by
            // observing its link op and revoking at an earlier cut.
            match acl_at_cut.devices.get(device) {
                Some(binding) if binding.account == op.author() && binding.account == *account => {
                    Ok(())
                }
                // An admin may eject any device, but may not MISNAME it. Authority
                // and accuracy are separate questions: the payload's `account` is
                // folded and read by account-scoped consumers, so an admin revoking
                // a device bound to X while claiming Y writes a falsehood into
                // state that outlives the op. Admin status is not a licence to do
                // that, and nothing about ejecting a device requires it.
                Some(binding) if acl_at_cut.is_root_admin(&op.author()) => {
                    if binding.account == *account {
                        Ok(())
                    } else {
                        Err(Rejected::DeviceAccountMismatch {
                            device: *device,
                            bound: binding.account,
                            claimed: *account,
                        })
                    }
                }
                // "No binding at this cut" is not a refusal, because an admin
                // must still be able to eject a device whose link this cut has
                // not folded. It only means the *self-service* claim cannot be
                // checked, so it does not authorize — and with no binding there is
                // no bound account to check the claim against, so the payload's
                // `account` is necessarily taken on trust here.
                None if acl_at_cut.is_root_admin(&op.author()) => Ok(()),
                _ => Err(Rejected::NotRootAdmin),
            }
        }
        OpPayload::AccountKeysRotated { handoff } => {
            // Only the account may roll its own key. The handoff's signature is
            // checked by `admit_key_rotation`; this is the separate question of
            // whether the *op* was authored under that account's authority.
            if op.author() != handoff.account {
                return Err(Rejected::RotationNotByAccount {
                    account: handoff.account,
                    author: op.author(),
                });
            }
            acl_at_cut.admit_key_rotation(handoff)
        }
    }
}

/// The device-binding precondition: is `op`'s signing key currently authorized
/// to act as `op.author()` at this cut?
///
/// Satisfied only by an explicit binding — a folded `DeviceLinked` naming this
/// device, this account, and this key. There is deliberately no implicit
/// fallback for an unlinked key: every author is an account, and every account
/// speaks through devices it has actually enrolled. A key nobody linked speaks
/// for nobody.
fn check_device_speaks_for_author(op: &Op, acl_at_cut: &AclView) -> Result<(), Rejected> {
    let device = op.device();

    // Checked ahead of the binding, not only in its absence. The fold does
    // maintain "revoked implies unbound" — a revocation removes the binding, and
    // `admit_device_link` refuses a link for a revoked device — so this is not a
    // reachable bypass today. It is here because `authorize` is the single
    // security boundary and takes an `AclView` with public fields from any
    // producer: resting a revocation check on an invariant maintained somewhere
    // else means a future fold that ever leaves a stale binding behind fails
    // open, silently.
    if acl_at_cut.revoked_devices.contains(&device) {
        return Err(Rejected::DeviceRevoked { device });
    }

    match acl_at_cut.devices.get(&device) {
        Some(binding) => {
            if binding.account != op.author() {
                return Err(Rejected::DeviceAccountMismatch {
                    device,
                    bound: binding.account,
                    claimed: op.author(),
                });
            }
            // Pinning the key, not merely the account, is what makes device key
            // rotation meaningful: after a re-link the retired key can no
            // longer author, even though the device is still bound.
            if binding.sign_pk != op.authorship.device_key {
                return Err(Rejected::DeviceKeyStale { device });
            }
            Ok(())
        }
        // The tombstone was already consulted above, so reaching here means the
        // device was never linked rather than withdrawn — worth keeping distinct
        // so whoever reads a rejection knows which happened.
        None => Err(Rejected::DeviceNotLinked { device }),
    }
}
