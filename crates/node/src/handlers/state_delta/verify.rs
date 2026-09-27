//! Apply-time governance authorization for state deltas (core#2716 Phase 4).
//!
//! [`authorize_delta_at_edge_projected`] is the single source of truth for "is this
//! author authorized to write into this context, at the cut its governance parent
//! edge names?", shared by the gossip-receive, governance-pending drain,
//! snapshot-replay, and DAG-catchup paths. It resolves membership FROM THE UNIFIED
//! PROJECTION at the op's causal cut (F5 #29b); the live `acl_view_at` resolver it
//! replaced is retired. The group is derived from the context (canonical
//! context→group mapping), never a signer-supplied `group_id` — which is what makes a
//! separate `group_id`-equality anti-bypass check unnecessary.

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;

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
    /// Membership resolution says the author is NOT a member at the cut — the
    /// projection's definitive not-a-member verdict (`member_at_cut == Some(false)`).
    /// The delta is rejected.
    MembershipReject { reason: &'static str },
    /// Local governance state is behind the cited cut. Buffer until catchup;
    /// `needed` lists every missing governance head so the receiver can
    /// request them all at once.
    Buffer { needed: Vec<[u8; 32]> },
}

/// The projection's membership verdict at a governance cut — the resolver result for
/// [`authorize_delta_at_edge_projected`] (F5 #29b). Mirrors what the live
/// `acl_view_at` produced, minus the Removed/NeverMember split (both are "not a
/// member", which the gossip path treats identically) and minus any `needed` set:
/// the projection reports incompleteness as the `Incomplete` variant alone, and
/// `authorize_delta_at_edge_projected` populates `DeltaAuthOutcome::Buffer.needed`
/// from the governance position's heads, not from this resolver.
pub(crate) enum CutMembership {
    /// Author is a member at the cut, with this effective role.
    Member(calimero_primitives::context::GroupMemberRole),
    /// Author is not a member at the cut (the projection's complete-fold verdict).
    NotMember,
    /// The cited ancestry isn't fully folded — buffer until governance catches up
    /// (the projection's `None`; the old `Unknown`).
    Incomplete,
}

/// Authorize a state delta against its **governance parent edge** (core#2716 P4),
/// resolving membership via a caller-supplied projection `resolve` (F5 #29b). The
/// successor to the live `acl_view_at`-backed resolver: the structural checks (group
/// derivation from the context, bypass / non-group rejects) are unchanged; the
/// membership verdict comes from the projection at the op's governance cut.
///
/// `governance_position` is the signed envelope's edge (`None` for a non-group
/// context); only its `governance_dag_heads` are consulted. The group is derived from
/// `context_id` via the canonical context→group mapping — the position's own
/// `group_id` is intentionally ignored, which is what makes the old
/// `group_id`-equality anti-bypass structurally unnecessary.
///
/// **Forward-only / TOCTOU**: the projection observes only the ancestry of the cited
/// heads, so a pre-removal write authorizes regardless of receive order; and
/// `ContextManager` serializes governance ops, so no group reassignment interleaves
/// between the group lookup and the resolve.
///
/// `resolve` wraps the node's maintained projection
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
    let owning = match calimero_governance_store::get_group_for_context(store, context_id) {
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

/// Why a delta's `calimero/tee/1` envelope, or its lack of one, refuses it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TeeEnvelopeRefusal {
    /// A TEE wrote without one. A TEE node writes only from a run its TEE
    /// scheduler fired, and every such run is signed under `calimero/tee/1`, so
    /// a write from it that is not was not made by that path.
    WriteWithoutTrigger,
    /// A key that is not an attested TEE's signed one. The envelope is where the
    /// fired marker comes from, so a member signing its own delta this way
    /// would stand every waiting TEE down.
    TriggerFromNonTee,
}

impl core::fmt::Display for TeeEnvelopeRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::WriteWithoutTrigger => "a TEE's delta that is not signed under calimero/tee/1",
            Self::TriggerFromNonTee => {
                "a calimero/tee/1 envelope from a key that is not an attested TEE"
            }
        })
    }
}

/// The two rules every receive path applies once a delta's envelope has
/// verified, alongside the read-only gate: a write from a TEE-role author
/// must carry a TEE trigger, and a TEE trigger must come from an attested TEE.
///
/// Fails closed on a store error, like the read-only gate beside it.
///
/// # Errors
/// The rule the delta breaks.
pub(crate) fn check_tee_envelope(
    store: &calimero_store::Store,
    context_id: &ContextId,
    author: &PublicKey,
    envelope: &calimero_node_primitives::sync::delta_auth::VerifiedEnvelope,
) -> Result<(), TeeEnvelopeRefusal> {
    let attested =
        calimero_governance_store::is_attested_tee_key_for_context(store, context_id, author)
            .unwrap_or(false);
    let read_only = || {
        calimero_governance_store::NamespaceRepository::new(store)
            .is_read_only_for_context(context_id, author)
            .unwrap_or(true)
    };
    tee_envelope_rule(envelope.tee_trigger().is_some(), attested, read_only)
}

/// [`check_tee_envelope`]'s decision, apart from the lookups it runs on.
/// `read_only` is asked only for an attested author's untriggered delta.
fn tee_envelope_rule(
    triggered: bool,
    attested: bool,
    read_only: impl FnOnce() -> bool,
) -> Result<(), TeeEnvelopeRefusal> {
    match (triggered, attested) {
        (true, true) => Ok(()),
        (true, false) => Err(TeeEnvelopeRefusal::TriggerFromNonTee),
        // An attested key that is also a writing member's (a TEE's operator
        // may hold both roles) writes as that member.
        (false, true) if read_only() => Err(TeeEnvelopeRefusal::WriteWithoutTrigger),
        (false, _) => Ok(()),
    }
}

/// Record a TEE delta the store has accepted, applied or pending: its trigger
/// has fired, so every TEE waiting to fall back on it stands down, and the
/// delta is kept with its trigger so this node can serve it.
///
/// Called only after [`check_tee_envelope`] passed, so the trigger is one an
/// attested TEE signed. Best-effort: a marker that is not recorded costs at
/// most a duplicate firing by a fallback TEE, and failing the delta over it
/// would be worse.
pub(crate) fn record_accepted_tee_delta(
    store: &calimero_store::Store,
    context_id: &ContextId,
    delta_id: &[u8; 32],
    envelope: &calimero_node_primitives::sync::delta_auth::VerifiedEnvelope,
) {
    let Some(trigger) = envelope.tee_trigger() else {
        return;
    };
    if let Err(err) =
        calimero_context_client::tee_trigger::record_tee_delta(store, context_id, delta_id, trigger)
    {
        tracing::warn!(%context_id, error = %err, "Failed to record a TEE firing");
    }
}

#[cfg(test)]
mod tests {
    use super::{tee_envelope_rule, TeeEnvelopeRefusal};

    #[test]
    fn an_attested_tee_may_sign_a_trigger() {
        assert_eq!(tee_envelope_rule(true, true, || true), Ok(()));
    }

    #[test]
    fn a_trigger_from_a_key_that_is_not_an_attested_tee_is_refused() {
        // A member, even a writing one, cannot sign its own delta as a TEE
        // firing: every TEE waiting on that trigger would stand down.
        assert_eq!(
            tee_envelope_rule(true, false, || false),
            Err(TeeEnvelopeRefusal::TriggerFromNonTee)
        );
        assert_eq!(
            tee_envelope_rule(true, false, || true),
            Err(TeeEnvelopeRefusal::TriggerFromNonTee)
        );
    }

    #[test]
    fn a_tee_write_without_a_trigger_is_refused() {
        assert_eq!(
            tee_envelope_rule(false, true, || true),
            Err(TeeEnvelopeRefusal::WriteWithoutTrigger)
        );
    }

    #[test]
    fn an_untriggered_write_from_anyone_else_is_left_to_the_other_gates() {
        // A writing member's delta, attested key or not, and a non-TEE's.
        assert_eq!(tee_envelope_rule(false, true, || false), Ok(()));
        assert_eq!(tee_envelope_rule(false, false, || true), Ok(()));
        assert_eq!(tee_envelope_rule(false, false, || false), Ok(()));
    }

    #[test]
    fn the_role_is_looked_up_only_for_an_attested_authors_untriggered_delta() {
        let asked = |triggered, attested| {
            let mut asked = false;
            let _ = tee_envelope_rule(triggered, attested, || {
                asked = true;
                false
            });
            asked
        };
        assert!(asked(false, true));
        assert!(!asked(false, false));
        assert!(!asked(true, true));
        assert!(!asked(true, false));
    }
}
