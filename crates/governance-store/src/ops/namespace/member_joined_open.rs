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
    ApplyError, BindingRejected, MemberJoinedOpenRejection, MembershipPath, MembershipRepository,
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
            // same apply — see `member_joined::apply` for why no endorsement
            // travels on the wire here and why a refused credential is reported
            // rather than propagated.
            record_join_credential(ctx, member, account)?;
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
/// credential refusal instead of propagating it: a credential this group cannot
/// admit must not orphan the membership op behind it. A member with no binding is
/// the state that held before joins carried one at all — survivable; a member the
/// DAG cannot apply at all is not.
///
/// A **store** failure is the opposite case and does propagate. It is not a verdict
/// on the credential, and swallowing it would leave this replica holding the
/// membership with no binding while its peers hold both — a permanent, silent
/// disagreement about which principal that member writes as. Returning the error
/// leaves the head unadvanced so the op is retried, which is what a transient
/// failure deserves and what a persistent one should be loud about.
///
/// # Errors
/// Propagates a store read/write failure from the binding or endorser write.
pub(super) fn record_join_credential(
    ctx: &mut NamespaceApplyCtx<'_>,
    member: PublicKey,
    account: &calimero_context_client::local_governance::JoinAccountCredential,
) -> EyreResult<()> {
    let namespace =
        calimero_context_config::types::ContextGroupId::from(ctx.namespace_id().to_bytes());

    // The credential has to be the JOINER'S OWN, and this is the only check that
    // establishes it. `apply_link` verifies the certificate against the genesis —
    // that it is a real grant by some account root — but says nothing about whose.
    // These ops are cleartext and gossip-visible, so any node can lift a
    // credential out of somebody else's join and present it as the `account` of
    // its own, signed honestly with its own key: `op.signer == member` proves the
    // envelope, never the payload. Binding it anyway would graft a stranger's
    // account into this namespace under the replayer's membership, and — because
    // the endorser row below is what makes a member account-addressed — would aim
    // the replayer's scope keys at the victim's devices.
    //
    // The certificate records the namespace identity as `sign_pk` precisely
    // because that is the key that signs ops, so for an honest joiner this is an
    // equality that already holds.
    if account.cert.sign_pk != member {
        tracing::warn!(
            ?namespace,
            %member,
            cert_sign_pk = %account.cert.sign_pk,
            "member joined presenting a credential certified for a different key; \
             the credential is ignored and the member is recorded without a binding"
        );
        return Ok(());
    }

    let bindings = crate::AccountBindingRepository::new(ctx.store());
    let outcome =
        bindings.apply_link(&namespace, &account.genesis, &account.chain, &account.cert)?;

    // The endorsement, materialized. `AccountDeviceLinked` carries an explicit
    // `AccountMemberEndorsement` because an account root is a member nowhere, so
    // its gate has to ask whether some member vouched. A join already answers
    // that: it is signed by the joining member, it carries the admin-signed
    // invitation authorising them, and the check above proves the certificate is
    // theirs. So no endorsement travels on the wire — but the ROW still has to be
    // written, because that is what every reader consults. Without it
    // `member_account_for_device_key` resolves the joiner to `None` and
    // `accounts_by_endorsing_member` never lists the account, leaving a binding
    // that is recorded and inert: no per-device authorization, no scope keys.
    if !outcome
        .as_ref()
        .err()
        .is_some_and(BindingRejected::is_permanent)
    {
        bindings.record_endorser(&namespace, account.cert.account, &member)?;
    }

    if let Err(rejected) = outcome {
        tracing::warn!(
            ?namespace,
            %member,
            ?rejected,
            "member joined but its account credential was refused; the member is \
             recorded without a binding, so its writes will attribute to a stand-in"
        );
    }
    Ok(())
}
