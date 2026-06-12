//! Anti-bypass `group_id` verification for state-delta governance positions.
//!
//! A signed `governance_position` carries a `group_id` the *sender* chose.
//! This module is the single source of truth for "does that claimed group
//! match the context's owning group?", shared by the apply path and the
//! DAG-catchup paths in `sync::manager` / `sync::delta_request`.

use calimero_primitives::context::ContextId;

/// Outcome of the anti-bypass `group_id` check that runs at every
/// apply path consulting a state delta's `governance_position`.
///
/// Two bypasses this check closes:
///
/// 1. **Mismatched `group_id` on a signed position.** A delta with
///    `governance_position: Some(pos)` carries a `group_id` the *sender*
///    chose at sign time. Without verification, a malicious sender could
///    craft a delta for context X (owned by group A) carrying a position
///    with `group_id = B` (a group the sender IS a member of). The
///    cross-DAG membership check would succeed against group B and the
///    write would land in context X without verifying membership in
///    group A.
///
/// 2. **Lying about being a non-group context.** `governance_position:
///    None` skips the cross-DAG check entirely (legacy non-group
///    contexts have no governance DAG). A malicious sender could omit
///    the position on a group-context delta to bypass enforcement. The
///    `GroupContextNoPosition` variant catches this.
///
/// Each call site translates the outcome to its local error handling
/// (warn-message wording, return-value shape, metric labels).
///
/// `pub(crate)` because the DAG-catchup paths in `sync::manager` and
/// `sync::delta_request` now share the same anti-bypass logic — a
/// single source of truth for "does the claimed governance position's
/// group match this context's owning group?". A copy-paste of the
/// match table across modules drifted in review (the DAG-catchup
/// head-pull was running `membership_status_at` without first checking
/// the group_id, leaving the bypass gap open); centralising fixes that
/// for good. New consumers must respect the TOCTOU and forward-only
/// invariants documented on `verify_position_group_id_matches_context`.
pub(crate) enum GroupIdCheck {
    /// Non-group context with no claimed group on the position. Legacy
    /// path: no enforcement applies. Fall through to apply.
    NonGroupOk,
    /// Group context with a position whose `group_id` matches the
    /// context's owning group. Proceed to the membership check.
    Match,
    /// Group context but the delta carries no `governance_position`.
    /// `None` is only legitimate for non-group contexts; rejected here.
    GroupContextNoPosition {
        owning: calimero_context_config::types::ContextGroupId,
    },
    /// Position claims a group, but the context is not part of any
    /// group. Rejected — a `Some` position is only legitimate for
    /// group contexts.
    NonGroupContextWithPosition {
        claimed: calimero_context_config::types::ContextGroupId,
    },
    /// Position claims a group, context is owned by a different group.
    /// Rejected — the bypass case described above.
    Mismatch {
        owning: calimero_context_config::types::ContextGroupId,
        claimed: calimero_context_config::types::ContextGroupId,
    },
    /// Store lookup failed; reject conservatively to avoid silent bypass
    /// on a transient I/O / corruption error.
    LookupError(eyre::Error),
}

// Hand-written `Debug` (rather than `#[derive(Debug)]`) because the
// `LookupError` variant wraps an `eyre::Error`, which we want to render
// via its own `Debug` impl rather than expose the full backtrace.
// Available in production code (not just tests) so call sites can
// debug-print outcomes in tracing spans.
impl std::fmt::Debug for GroupIdCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupIdCheck::NonGroupOk => write!(f, "NonGroupOk"),
            GroupIdCheck::Match => write!(f, "Match"),
            GroupIdCheck::GroupContextNoPosition { owning } => {
                write!(f, "GroupContextNoPosition {{ owning: {owning:?} }}")
            }
            GroupIdCheck::NonGroupContextWithPosition { claimed } => {
                write!(f, "NonGroupContextWithPosition {{ claimed: {claimed:?} }}")
            }
            GroupIdCheck::Mismatch { owning, claimed } => {
                write!(f, "Mismatch {{ owning: {owning:?}, claimed: {claimed:?} }}")
            }
            GroupIdCheck::LookupError(err) => write!(f, "LookupError({err:?})"),
        }
    }
}

/// Anti-bypass check for the apply-path consumers of a state delta's
/// `governance_position`. The `claimed_group_id` argument is the
/// `group_id` from `Some(pos)` (the sender's signed claim), or `None`
/// when the delta has no position. Returns the outcome each call site
/// interprets to log + recover in its local idiom. See [`GroupIdCheck`]
/// for the bypasses this closes.
///
/// **TOCTOU note.** Each call site runs this check immediately before
/// `membership_status_at`, which internally walks the governance DAG
/// scoped to `pos.group_id`. Between the two calls no lock is held;
/// in principle a concurrent governance op could reassign the context
/// to a different group, leaving the bypass check satisfied against
/// the old group while the membership walk runs against the new one.
/// In practice the `ContextManager` actor applies governance ops
/// sequentially, so no concurrent reassignment can interleave between
/// the check and the membership walk. The actor isolation is the
/// invariant that mitigates the TOCTOU window; if that ever changes,
/// the check needs to be promoted to a snapshot read across both
/// lookups.
pub(crate) fn verify_position_group_id_matches_context(
    store: &calimero_store::Store,
    context_id: &ContextId,
    claimed_group_id: Option<calimero_context_config::types::ContextGroupId>,
) -> GroupIdCheck {
    let owning = match calimero_context::group_store::get_group_for_context(store, context_id) {
        Ok(owning) => owning,
        Err(err) => return GroupIdCheck::LookupError(err),
    };

    match (owning, claimed_group_id) {
        (None, None) => GroupIdCheck::NonGroupOk,
        (Some(owning), None) => GroupIdCheck::GroupContextNoPosition { owning },
        (None, Some(claimed)) => GroupIdCheck::NonGroupContextWithPosition { claimed },
        (Some(owning), Some(claimed)) if owning == claimed => GroupIdCheck::Match,
        (Some(owning), Some(claimed)) => GroupIdCheck::Mismatch { owning, claimed },
    }
}

/// Outcome of the apply-time governance authorization check (core#2716 P4),
/// interpreted by each call site in its local idiom (warn wording, return
/// shape, buffering construction).
pub(crate) enum DeltaAuthOutcome {
    /// Author is a member at the cited governance cut. Carries the context's
    /// owning group + role for peer-identity observation. Proceed to apply.
    Authorized {
        group: calimero_context_config::types::ContextGroupId,
        role: calimero_primitives::context::GroupMemberRole,
    },
    /// No governance gate applies — a non-group context carrying no edge
    /// (legacy path). Proceed to apply.
    Ungated,
    /// Reject the delta. `reason` is a static label for the call site's warn
    /// log. Covers: author Removed / NeverMember at the cut; a group-context
    /// delta with no edge (bypass attempt); an edge on a non-group context;
    /// and lookup / walk errors (rejected conservatively to avoid silent
    /// bypass on transient I/O or corruption).
    Reject(&'static str),
    /// Local governance state is behind the cited cut. Buffer until catchup;
    /// `needed` lists every missing governance head so the receiver can
    /// request them all at once.
    Buffer { needed: Vec<[u8; 32]> },
}

/// Authorize a state delta against its **governance parent edge** (core#2716
/// P4) — the successor to the `GroupIdCheck` + `membership_status_at` pair.
///
/// `governance_position` is the signed envelope's edge (`None` for a
/// non-group context); only its `governance_dag_heads` are consulted. The
/// group is derived from `context_id` via the canonical context→group
/// mapping — the position's own `group_id` is intentionally ignored, which is
/// what makes the old `group_id`-equality anti-bypass structurally
/// unnecessary: the only group ever authorized against is the context's own,
/// so a signer cannot cite a different group it belongs to elsewhere.
///
/// Authorization itself is delegated to
/// [`acl_view_at`](calimero_context::group_store::acl_view_at), which resolves
/// membership at the cut named by the heads.
///
/// **Forward-only** — `acl_view_at` observes only the ancestry of the cited
/// heads, so a pre-removal write resolves to [`DeltaAuthOutcome::Authorized`]
/// regardless of the order the receiver observed the later removal.
///
/// **TOCTOU** — runs immediately before apply with no lock held;
/// `ContextManager` serializes governance ops, so no concurrent group
/// reassignment can interleave between the group lookup and the walk.
pub(crate) fn authorize_delta_at_edge(
    store: &calimero_store::Store,
    context_id: &ContextId,
    author: &calimero_primitives::identity::PublicKey,
    governance_position: Option<&calimero_context_config::types::GovernancePosition>,
) -> DeltaAuthOutcome {
    use calimero_context::group_store::{acl_view_at, MembershipStatus};

    let owning = match calimero_context::group_store::get_group_for_context(store, context_id) {
        Ok(owning) => owning,
        Err(_) => {
            return DeltaAuthOutcome::Reject(
                "get_group_for_context failed; rejecting to avoid silent bypass",
            )
        }
    };

    match (owning, governance_position) {
        (None, None) => DeltaAuthOutcome::Ungated,
        (Some(_), None) => DeltaAuthOutcome::Reject(
            "group context but no governance edge (likely a bypass attempt)",
        ),
        (None, Some(_)) => {
            DeltaAuthOutcome::Reject("governance edge present but context is not part of any group")
        }
        (Some(group), Some(pos)) => {
            match acl_view_at(store, group, author, &pos.governance_dag_heads) {
                Ok(MembershipStatus::Member(role)) => DeltaAuthOutcome::Authorized { group, role },
                Ok(MembershipStatus::Removed { .. }) => {
                    DeltaAuthOutcome::Reject("author was removed from group at governance cut")
                }
                Ok(MembershipStatus::NeverMember) => DeltaAuthOutcome::Reject(
                    "author is not a member of the group at governance cut",
                ),
                Ok(MembershipStatus::Unknown { needed }) => DeltaAuthOutcome::Buffer { needed },
                Err(_) => DeltaAuthOutcome::Reject(
                    "membership lookup failed (hash mismatch / corruption)",
                ),
            }
        }
    }
}
