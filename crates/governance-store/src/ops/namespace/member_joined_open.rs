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
use crate::{
    ApplyError, MemberJoinedOpenRejection, MembershipPath, MembershipRepository,
    NamespaceRepository,
};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    member: PublicKey,
    group_id: [u8; 32],
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
    if resolved_ns.to_bytes() != namespace_id {
        eyre::bail!(ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::WrongNamespace {
                gid: format!("{gid:?}"),
                resolved_ns: format!("{resolved_ns:?}"),
                this_ns: format!("{:?}", ContextGroupId::from(namespace_id)),
            }
        ));
    }
    match MembershipRepository::new(store).check_path(&gid, &member)? {
        MembershipPath::Inherited { .. } => Ok(()),
        MembershipPath::Direct => {
            // Direct members go through `MemberJoined` or `add_group_members`
            // — they shouldn't be using this op.
            eyre::bail!(ApplyError::MemberJoinedOpenRejected(
                MemberJoinedOpenRejection::AlreadyDirectMember(format!("{member}"))
            ));
        }
        MembershipPath::None => {
            eyre::bail!(ApplyError::MemberJoinedOpenRejected(
                MemberJoinedOpenRejection::NoMembershipPath {
                    member: format!("{member}"),
                    gid: format!("{gid:?}"),
                }
            ));
        }
    }
}
