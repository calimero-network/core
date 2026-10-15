//! `RootOp::MemberJoinedOpen` apply check.
//!
//! The op is sealed under the namespace key, its signer must hold a credential
//! certifying `member` (see [`join_op_proves_ownership`]) and be bound to it
//! at the op's cut, and `member` must have an Inherited membership path to `group_id`. It writes no row and links
//! no device; the side effects (deny-list clear, identity restore) happen in
//! the outer `apply_signed_op` match, and the key comes from the pull path.

use super::context::NamespaceApplyCtx;
use crate::authorizer::AtCutMembershipPath;
use crate::{
    ApplyError, MemberJoinedOpenRejection, MembershipPath, MembershipRepository,
    NamespaceRepository, PermissionChecker, ReentryRepository,
};
use calimero_account::AccountId;
use calimero_context_client::local_governance::{JoinAccountCredential, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

/// Whether a self-service join op was really made by the account it admits.
///
/// The op names an ACCOUNT, so proving ownership takes two halves and the
/// credential carries both:
///
/// 1. `op.signer == cert.sign_pk` — the op was signed by the device this
///    certificate certifies. This is what the old `op.signer == member`
///    comparison did while `member` was a key; it cannot be asked of an account
///    directly, because an account is a hash and no signature names it.
/// 2. [`join_credential_binds`](calimero_op_adapter::join_credential_binds) —
///    the certificate names `member` and verifies against its own genesis and
///    chain.
///
/// Either half alone admits a forgery. Without (1), anyone can lift a
/// credential out of somebody else's cleartext join and replay it to claim
/// their account. Without (2), a member holding any valid credential could
/// name someone else's account in the op and have the row written for them.
///
/// A key the device has since been re-keyed past at the op's cut proves nothing, nor does a
/// device withdrawn there (ordered by the cut, confirmed by the binding rows): both are refused.
///
/// Deliberately NOT used by the TEE admission path, which breaks half (1) on
/// purpose: an attested replica cannot admit itself, so its op is signed by the
/// verifying member rather than by the replica's own device.
pub(crate) fn join_op_proves_ownership(
    permissions: &PermissionChecker<'_>,
    signer: &PublicKey,
    member: &AccountId,
    account: &JoinAccountCredential,
) -> EyreResult<bool> {
    if !credential_names_signer(signer, member, account) {
        return Ok(false);
    }
    let cert = &account.statement;
    if permissions.device_epoch_superseded(&cert.account, &cert.device, cert.device_epoch)? {
        eyre::bail!(
            "join signed by {signer}, a key device {} was re-keyed past at the op's cut \
             (certificate epoch {})",
            hex::encode(cert.device.as_bytes()),
            cert.device_epoch
        );
    }
    if permissions.device_withdrawn(&cert.account, &cert.device)? {
        eyre::bail!(
            "join signed by {signer}, a key of device {} the namespace has withdrawn at \
             the op's cut",
            hex::encode(cert.device.as_bytes())
        );
    }
    Ok(true)
}

/// The op-local halves of [`join_op_proves_ownership`], for an op whose signer's
/// binding the projection never folds (the founding relay's attestation).
pub(crate) fn credential_names_signer(
    signer: &PublicKey,
    member: &AccountId,
    account: &JoinAccountCredential,
) -> bool {
    *signer == account.statement.sign_pk
        && calimero_op_adapter::join_credential_binds(member, account)
}

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    member: AccountId,
    group_id: [u8; 32],
    account: &JoinAccountCredential,
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace_id = ctx.namespace_id();

    let ns_gid = ContextGroupId::from(namespace_id.to_bytes());
    let permissions = ctx.permissions_for(ns_gid);
    // The account is already here, so its signing device must be too: a device no
    // link brought in has no scope this namespace has checked.
    if !join_op_proves_ownership(&permissions, &op.signer, &member, account)?
        || permissions.account_for_signer_at_cut(&op.signer)? != Some(member)
    {
        eyre::bail!(ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::SignerMismatch {
                signer: format!("{}", op.signer),
                member: member.to_string(),
            }
        ));
    }
    let gid = ContextGroupId::from(group_id);
    // Cross-namespace forgery guard: without this check, an attacker
    // on namespace A could publish a MemberJoinedOpen naming a
    // `group_id` from namespace B; `check_group_membership_path`
    // walks parents up to whichever namespace root owns `gid`, so
    // the path check below could succeed against B's data when this
    // op is being applied in namespace A. Pin `gid` to this
    // namespace — matches the implicit assumption in the sibling
    // `MemberJoined` apply path.
    let resolved_ns = NamespaceRepository::new(store).resolve(&gid)?;
    if resolved_ns.to_bytes() != namespace_id.to_bytes() {
        eyre::bail!(ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::WrongNamespace {
                gid: gid.to_string(),
                resolved_ns: resolved_ns.to_string(),
                this_ns: format!("{:?}", ContextGroupId::from(namespace_id.to_bytes())),
            }
        ));
    }
    // Re-entry gate. An identity that exited this group does not flow back in by
    // inheritance — and it is exactly inheritance that would otherwise walk a
    // kicked member straight back into the Open subgroup they were kicked from,
    // since membership there is automatic for any parent member holding
    // `CAN_JOIN_OPEN_SUBGROUPS`. Any prior exit blocks it, a voluntary leaver
    // included: inheritance is passive and carries no fresh authorization, so
    // there is nothing here to weigh a re-admission against.
    //
    // Runs after the signer/namespace guards (never read state on the say-so of
    // an unauthenticated op) and before the path resolution below.
    //
    // A removal from a group the inheritance passes through ends it too; read
    // live because the projection below folds no deny-list or re-entry rows.
    let membership = MembershipRepository::new(store);
    let exited = if ReentryRepository::new(store)
        .block_of(&gid, &member)?
        .is_some()
    {
        Some(gid)
    } else if membership.has_direct_member(&gid, &member)? {
        None
    } else {
        membership.exited_ancestor(&gid, &member)?
    };
    if let Some(exited) = exited {
        eyre::bail!(ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::ReentryBlocked {
                member: format!("{member}"),
                gid: exited.to_string(),
            }
        ));
    }
    // F5 #29b flip: decide the membership PATH from the projection at the op's causal
    // cut (validated divergence-free on the `membership-path` plane). Live `check_path`
    // is the `None`-fallback, computed LAZILY — only when the projection abstains — so
    // a `check_path` store error can't abort an apply the projection would have
    // decided. The live read retires when `check_path` is deleted.
    let path = match ctx.projection_membership_path(&gid, &member) {
        Some(projected) => projected,
        None => {
            // Falling straight through to live collapsed the two reasons the
            // projection can abstain. When the cut is real but unfolded here, the
            // live rows are a DIFFERENT cut, so this replica could reject a join
            // its peers admitted — permanently, since the reject never advances
            // the head. Park for retry instead.
            ctx.ensure_live_fallback_is_sound(&gid, &member)?;
            membership_path_kind(&MembershipRepository::new(store).check_path(&gid, &member)?)
        }
    };
    match path {
        AtCutMembershipPath::Inherited => {
            // Emit on real join; queued so replay dedups it.
            ctx.queue_event(crate::op_events::OpEvent::MemberJoined {
                group_id,
                member,
                role: None,
            });
            // No device binding: the account is already here, and the join
            // carries no root-signed scope a new device could be checked against.
            Ok(())
        }
        AtCutMembershipPath::Direct => {
            // Direct members go through `MemberJoined` or `add_group_members`
            // — they shouldn't be using this op.
            eyre::bail!(ApplyError::MemberJoinedOpenRejected(
                MemberJoinedOpenRejection::AlreadyDirectMember(format!("{member}"))
            ));
        }
        AtCutMembershipPath::None => {
            eyre::bail!(ApplyError::MemberJoinedOpenRejected(
                MemberJoinedOpenRejection::NoMembershipPath {
                    member: format!("{member}"),
                    gid: gid.to_string(),
                }
            ));
        }
    }
}

/// The live `MembershipPath` collapsed to the at-cut path KIND (the `None`-fallback).
fn membership_path_kind(path: &MembershipPath) -> AtCutMembershipPath {
    match path {
        MembershipPath::Inherited { .. } => AtCutMembershipPath::Inherited,
        MembershipPath::Direct => AtCutMembershipPath::Direct,
        MembershipPath::None => AtCutMembershipPath::None,
    }
}
