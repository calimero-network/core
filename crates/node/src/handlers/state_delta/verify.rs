//! Apply-time governance authorization for state deltas (core#2716 Phase 4).
//!
//! [`authorize_delta_at_edge`] is the single source of truth for "is this
//! author authorized to write into this context, at the cut its governance
//! parent edge names?", shared by the gossip-receive, governance-pending
//! drain, snapshot-replay, and DAG-catchup paths. The group is derived from
//! the context (canonical context→group mapping), never a signer-supplied
//! `group_id` — which is what makes a separate `group_id`-equality anti-bypass
//! check unnecessary.

use calimero_primitives::context::ContextId;

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
    /// Reject the delta on a **structural / error** ground (NOT a membership
    /// verdict): a group-context delta with no edge (bypass attempt), an edge on
    /// a non-group context, or a lookup / walk error (rejected conservatively to
    /// avoid silent bypass on transient I/O or corruption). The projection does
    /// not override these.
    Reject(&'static str),
    /// Membership resolution says the author is NOT a member at the cut. On the
    /// gossip path this is the projection's definitive not-a-member verdict
    /// (`member_at_cut == Some(false)`); on the live sync paths it is
    /// `acl_view_at`'s Removed / NeverMember. Either way the delta is rejected.
    MembershipReject { reason: &'static str },
    /// Local governance state is behind the cited cut. Buffer until catchup;
    /// `needed` lists every missing governance head so the receiver can
    /// request them all at once.
    Buffer { needed: Vec<[u8; 32]> },
}

/// The projection's membership verdict at a governance cut — the resolver result for
/// [`authorize_delta_at_edge_projected`] (F5 #29b). Mirrors what the live
/// `acl_view_at` produced, minus the Removed/NeverMember split (both are "not a
/// member", which the gossip path treats identically) and minus the `needed` set
/// (the projection reports incompleteness as a bool; `Buffer.needed` is best-effort).
pub(crate) enum CutMembership {
    /// Author is a member at the cut, with this effective role.
    Member(calimero_primitives::context::GroupMemberRole),
    /// Author is not a member at the cut (the projection's complete-fold verdict).
    NotMember,
    /// The cited ancestry isn't fully folded — buffer until governance catches up
    /// (the projection's `None`; the old `Unknown`).
    Incomplete,
}

/// [`authorize_delta_at_edge`] resolving membership via a caller-supplied projection
/// `resolve` instead of the live `acl_view_at` (F5 #29b gossip flip). The structural
/// checks (group derivation from the context, bypass / non-group rejects) are
/// identical; only the membership verdict swaps to the projection at the op's
/// governance cut. `resolve` wraps the node's maintained projection
/// (`member_at_cut` + `role_at_cut_for_group`) — already validated divergence-free
/// against live on the `membership-cut` / `membership-cut-grant` / `data-write-role`
/// / `data-write-decision` planes. `Incomplete` maps to `Buffer` (exactly as live's
/// `Unknown` did); `Buffer.needed` carries the cited heads (consumed only as a log
/// count — the buffered delta re-resolves against the projection on drain).
pub(crate) fn authorize_delta_at_edge_projected(
    store: &calimero_store::Store,
    context_id: &ContextId,
    author: &calimero_primitives::identity::PublicKey,
    governance_position: Option<&calimero_context_config::types::GovernanceParentEdge>,
    resolve: impl FnOnce(calimero_context_config::types::ContextGroupId, &[[u8; 32]]) -> CutMembership,
) -> DeltaAuthOutcome {
    let owning = match calimero_context::group_store::get_group_for_context(store, context_id) {
        Ok(owning) => owning,
        Err(err) => {
            tracing::warn!(
                %context_id, %author, %err,
                "authorize_delta_at_edge: get_group_for_context failed; rejecting to avoid silent bypass"
            );
            return DeltaAuthOutcome::Reject(
                "get_group_for_context failed; rejecting to avoid silent bypass",
            );
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
        (Some(group), Some(pos)) => match resolve(group, &pos.governance_dag_heads) {
            CutMembership::Member(role) => DeltaAuthOutcome::Authorized { group, role },
            CutMembership::NotMember => DeltaAuthOutcome::MembershipReject {
                reason: "author is not a member of the group at governance cut (projection)",
            },
            CutMembership::Incomplete => DeltaAuthOutcome::Buffer {
                needed: pos.governance_dag_heads.clone(),
            },
        },
    }
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
    governance_position: Option<&calimero_context_config::types::GovernanceParentEdge>,
) -> DeltaAuthOutcome {
    use calimero_context::group_store::{acl_view_at, MembershipStatus};

    let owning = match calimero_context::group_store::get_group_for_context(store, context_id) {
        Ok(owning) => owning,
        Err(err) => {
            // Log the underlying error before collapsing to a static reject
            // reason — a transient store I/O / corruption fault here looks
            // identical to a deliberate bypass in the caller's warn line
            // otherwise, which hides real operational problems.
            tracing::warn!(
                %context_id, %author, %err,
                "authorize_delta_at_edge: get_group_for_context failed; rejecting to avoid silent bypass"
            );
            return DeltaAuthOutcome::Reject(
                "get_group_for_context failed; rejecting to avoid silent bypass",
            );
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
                Ok(MembershipStatus::Removed { .. }) => DeltaAuthOutcome::MembershipReject {
                    reason: "author was removed from group at governance cut",
                },
                Ok(MembershipStatus::NeverMember) => DeltaAuthOutcome::MembershipReject {
                    reason: "author is not a member of the group at governance cut",
                },
                Ok(MembershipStatus::Unknown { needed }) => DeltaAuthOutcome::Buffer { needed },
                Err(err) => {
                    // Surface the real cause (hash mismatch / store corruption /
                    // I/O) rather than swallowing it behind the static reason.
                    tracing::warn!(
                        %context_id, %author, group_id = ?group, %err,
                        "authorize_delta_at_edge: membership lookup failed; rejecting"
                    );
                    DeltaAuthOutcome::Reject(
                        "membership lookup failed (hash mismatch / corruption)",
                    )
                }
            }
        }
    }
}
