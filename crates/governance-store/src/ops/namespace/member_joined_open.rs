//! `RootOp::MemberJoinedOpen` apply check. Extracted from
//! `NamespaceGovernance::execute_member_joined_open` in #2481.
//!
//! Apply check for `RootOp::MemberJoinedOpen`. The op is cleartext,
//! the outer `SignedNamespaceOp.signer` MUST equal `member` (proves
//! key ownership), and `member` MUST have an Inherited membership
//! path to `group_id` — i.e. the subgroup is `Open` and they hold
//! `CAN_JOIN_OPEN_SUBGROUPS` at the namespace root (the same check
//! `join_context.rs` runs locally before letting the joiner
//! proceed). We don't mutate state here — the side-effects
//! (deny-list clear, identity restore) happen in the outer
//! `apply_signed_op` match. The joiner obtains the group key via the
//! direct pull-based key-delivery path, not from this op.

use super::context::NamespaceApplyCtx;
use crate::authorizer::AtCutMembershipPath;
use crate::{
    ApplyError, MemberJoinedOpenRejection, MembershipPath, MembershipRepository,
    NamespaceRepository, ReentryRepository,
};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    member: PublicKey,
    group_id: [u8; 32],
    account: &calimero_context_client::local_governance::JoinAccountCredential,
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace_id = ctx.namespace_id();

    if op.signer != member {
        eyre::bail!(ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::SignerMismatch {
                signer: format!("{}", op.signer),
                member: format!("{member}"),
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
                gid: format!("{gid:?}"),
                resolved_ns: format!("{resolved_ns:?}"),
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
    if ReentryRepository::new(store)
        .block_of(&gid, &member)?
        .is_some()
    {
        eyre::bail!(ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::ReentryBlocked {
                member: format!("{member}"),
                gid: format!("{gid:?}"),
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
            // The join is accepted, so record the joiner's device binding in the
            // same apply — see `member_joined::apply` for why no endorsement is
            // required here and why a refused credential is reported rather than
            // propagated.
            record_join_credential(ctx, member, account);
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
                    gid: format!("{gid:?}"),
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

/// Record a joiner's certified account alongside its membership.
///
/// Shared by the open-join path and [`super::member_joined::apply`]. Reports a
/// refusal instead of propagating it: a credential this group cannot admit must not
/// orphan the membership op behind it. A member with no binding is the pre-#3346
/// state and survivable; a member the DAG cannot apply at all is not.
pub(super) fn record_join_credential(
    ctx: &mut NamespaceApplyCtx<'_>,
    member: PublicKey,
    account: &calimero_context_client::local_governance::JoinAccountCredential,
) {
    let namespace =
        calimero_context_config::types::ContextGroupId::from(ctx.namespace_id().to_bytes());
    match crate::AccountBindingRepository::new(ctx.store()).apply_link(
        &namespace,
        &account.genesis,
        &account.chain,
        &account.cert,
    ) {
        Ok(Ok(_)) => {}
        Ok(Err(rejected)) => tracing::warn!(
            ?namespace,
            %member,
            ?rejected,
            "member joined but its account credential was refused; the member is \
             recorded without a binding, so its writes will attribute to a stand-in"
        ),
        Err(err) => tracing::warn!(
            ?namespace,
            %member,
            %err,
            "member joined but recording its account credential failed"
        ),
    }
}
