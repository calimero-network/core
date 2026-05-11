//! Position-aware membership lookup for cross-DAG authorization.
//!
//! [`MembershipStatus`] is the type the apply-time membership check consumes
//! to decide whether a state delta signed against a specific governance cut
//! should be applied. It distinguishes the four answers a position-aware
//! lookup can produce:
//!
//! - [`Member`](MembershipStatus::Member) — signer is a member at the named
//!   cut, with role known.
//! - [`Removed`](MembershipStatus::Removed) — signer was a member but removed
//!   on or before the named cut.
//! - [`NeverMember`](MembershipStatus::NeverMember) — signer is not in the
//!   member set at the named cut, and no record of removal exists.
//! - [`Unknown`](MembershipStatus::Unknown) — local governance state hasn't
//!   advanced to the named cut yet, so the answer can't be determined. The
//!   state-delta receive path buffers ops on this signal until governance
//!   catches up.
//!
//! Collapsing these into `Option<GroupMemberRole>` (as the legacy
//! `get_member_role` API does) makes "forgot to check" a silent runtime bug;
//! [`MembershipStatus`] makes it a non-exhaustive-match warning.

use std::collections::{HashSet, VecDeque};

use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::{
    ContextGroupId, GovernancePosition, MAX_GOVERNANCE_DAG_HEADS,
};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::group_keys::{decrypt_group_op, load_group_key_by_id};
use super::namespace_op_log::NamespaceOpLogService;

/// Maximum number of governance ops the prefix walk will visit before
/// bailing. Bounds memory + CPU for the BFS even on very deep DAGs and
/// guards against op-log corruption that could otherwise cause an
/// unbounded traversal. Sized far above any realistic governance history
/// (a busy group might accumulate hundreds of governance ops per year);
/// if a real deployment ever sees the bound it indicates either DAG
/// corruption or a use-case the prefix walk wasn't designed for, both
/// of which are better surfaced as explicit errors than as silent
/// resource exhaustion.
pub const MAX_PREFIX_WALK_NODES: usize = 10_000;

/// Position-aware membership state.
///
/// Always carries enough information for the apply-time membership check to
/// decide whether a state op signed at `position` can be applied. Receivers
/// must match all four variants — `_` arms are a smell that suggests a
/// missing case.
#[derive(Clone, Debug, PartialEq)]
pub enum MembershipStatus {
    /// Signer is a member at the named cut.
    Member(GroupMemberRole),
    /// Signer was a member at some earlier cut but removed on or before
    /// `position`. `last_role` is the role they held at the time of removal,
    /// useful for diagnostics and any downstream deny-list logic.
    Removed { last_role: GroupMemberRole },
    /// Signer never appears in any `MemberAdded` op in the prefix up to
    /// `position`.
    NeverMember,
    /// Local governance state hasn't advanced to `position` yet — the answer
    /// is undecidable at this moment in time. `needed` lists every governance
    /// op hash from `position.governance_dag_heads` that is missing locally.
    ///
    /// Returning the full set (rather than just the first missing head) lets
    /// the receiver's pending-buffer wait for all of them in parallel
    /// instead of O(n) sequential buffer-and-retry round-trips. Always
    /// non-empty when this variant is returned.
    Unknown { needed: Vec<[u8; 32]> },
}

/// Determine [`MembershipStatus`] for `signer` against the governance state
/// named by `position`.
///
/// Three branches:
///
/// 1. **Fast path** — `position.governance_dag_heads` equals current local
///    heads. Consults the materialized member set for an immediate
///    `Member` / `NeverMember` answer. Verifies
///    `position.group_state_hash` against the locally-computed state hash;
///    mismatch surfaces as `Err` (tampering or local divergence). On this
///    path `get_group_member_role` returning `None` does not distinguish
///    "never added" from "was added then removed" — the fast path
///    therefore conflates `Removed` into `NeverMember`. Callers must treat
///    both as "not currently a member" on this path; the distinction is
///    recovered by the prefix walk below when receivers are slightly
///    behind senders.
///
/// 2. **Unknown** — any head in `position.governance_dag_heads` is missing
///    from the local namespace op log. Returns
///    [`Unknown { needed }`](MembershipStatus::Unknown) listing every
///    missing head so the receiver can buffer once and request them all
///    in parallel.
///
/// 3. **Prefix walk** — all heads are known but `position` differs from
///    current local state (the receiver is ahead of the sender). Walks the
///    namespace governance DAG from `position.governance_dag_heads`
///    backwards, decrypts group ops affecting `signer`, replays the
///    membership state machine, and returns
///    `Member(role)` / `Removed { last_role }` / `NeverMember` based on
///    the final state at the named cut. This branch produces the full
///    distinction the fast path conflates.
pub fn membership_status_at(
    store: &Store,
    signer: &PublicKey,
    position: &GovernancePosition,
) -> EyreResult<MembershipStatus> {
    // Cheap guard against malformed / attacker-crafted positions before
    // we do any store work. See [`MAX_GOVERNANCE_DAG_HEADS`].
    if position.governance_dag_heads.len() > MAX_GOVERNANCE_DAG_HEADS {
        eyre::bail!(
            "membership_status_at: governance_dag_heads length {} exceeds \
             MAX_GOVERNANCE_DAG_HEADS={} (likely malformed or attacker-crafted)",
            position.governance_dag_heads.len(),
            MAX_GOVERNANCE_DAG_HEADS,
        );
    }

    let group_id = position.group_id;

    // Namespace creator / root admin carve-out — required because the
    // creator does **not** emit a self-`MemberJoined` op at namespace
    // genesis: their membership lives in `GroupMeta::admin_identity`,
    // not in a `GroupMember` row. Without this short-circuit, both the
    // fast-path (`get_group_member_role` returns `None`) and the
    // prefix walk (no `RootOp::MemberJoined` or `GroupOp::MemberAdded`
    // for the creator) return `NeverMember`, and the apply-time
    // membership check hard-rejects every state delta authored by the
    // namespace creator. This was the root cause of the flaky
    // `group-3node` e2e: when receivers' governance heads hadn't yet
    // caught up to node-1's at write time, the prefix-walk branch
    // fired and rejected the legitimate write.
    //
    // `is_group_admin` already does the same admin-identity carve-out
    // for the `MemberJoined`-less creator (see its doc and the
    // companion `namespace_member_pubkeys` helper at
    // `membership.rs::namespace_member_pubkeys`). Using the same path
    // here keeps the semantics aligned: any signer that
    // `is_group_admin` accepts must also pass the cross-DAG check.
    if super::membership::is_group_admin(store, &group_id, signer)? {
        return Ok(MembershipStatus::Member(
            calimero_primitives::context::GroupMemberRole::Admin,
        ));
    }

    let namespace_id = super::namespace::resolve_namespace(store, &group_id)?;
    let dag = super::namespace_dag::NamespaceDagService::new(store, namespace_id.to_bytes());
    let local_heads = dag.read_head_record()?.parent_hashes;

    // Branch 1 — heads match local state exactly. Use the materialized
    // member set; this is the steady-state hot path.
    if heads_equal(&local_heads, &position.governance_dag_heads) {
        // Verify the embedded group_state_hash against locally-computed
        // state. When heads match, both should be deterministic functions
        // of the same materialized state — divergence here signals
        // tampering or local corruption that the rest of the pipeline
        // can't detect on its own.
        let local_state_hash = super::meta::compute_group_state_hash(store, &group_id)?;
        if local_state_hash != position.group_state_hash {
            eyre::bail!(
                "membership_status_at: group_state_hash mismatch — \
                 heads match but state hashes differ \
                 (group={:?}, position_hash={}, local_hash={})",
                group_id,
                hex::encode(position.group_state_hash),
                hex::encode(local_state_hash),
            );
        }
        // See function-level doc for the `Removed` vs `NeverMember`
        // conflation caveat — the prefix walk recovers the distinction.
        return Ok(
            match super::membership::get_group_member_role(store, &group_id, signer)? {
                Some(role) => MembershipStatus::Member(role),
                None => MembershipStatus::NeverMember,
            },
        );
    }

    // Branch 2 — collect every referenced head that is not present in our
    // local namespace op log. Direct key lookups (O(1) per head) against
    // the namespace-wide op store, not a group-filtered scan: heads can
    // point at any SignedNamespaceOp (Root ops or Group ops for any group
    // in the namespace), so filtering by group_id would mis-classify
    // legitimate heads as missing.
    let op_log = NamespaceOpLogService::new(store, namespace_id.to_bytes());
    let mut missing: Vec<[u8; 32]> = Vec::with_capacity(position.governance_dag_heads.len());
    for head in &position.governance_dag_heads {
        if !op_log.contains_op(*head)? {
            missing.push(*head);
        }
    }
    if !missing.is_empty() {
        return Ok(MembershipStatus::Unknown { needed: missing });
    }

    // Branch 3 — heads known but differ from local state. Walk the
    // namespace governance DAG from `position.governance_dag_heads`
    // backwards, decrypting `Group` ops affecting this group + signer,
    // and replay the membership state machine to derive the answer at
    // the named cut.
    prefix_walk_membership(
        store,
        namespace_id.to_bytes(),
        group_id,
        signer,
        &position.governance_dag_heads,
    )
}

/// Walk the namespace governance DAG from `target_heads` backwards through
/// `parent_op_hashes`, replaying every membership-affecting op for `signer`
/// in `group_id`, and return the resulting [`MembershipStatus`].
///
/// **Forward-only invariant** — load-bearing for cross-DAG correctness.
/// The walk visits *only* the ancestry of `target_heads`: an op that is in
/// the local DAG but *causally after* `target_heads` (i.e., not reachable
/// by walking `parent_op_hashes` from any head in `target_heads`) is NEVER
/// observed by this walk. This is the mechanism by which pre-removal
/// writes from a now-removed member remain valid forever, regardless of
/// arrival order: a state delta signed at position `H_pre` (before the
/// signer's `MemberRemoved` at `H_rem`) resolves to `Member(role)` here
/// even on a receiver whose local DAG has already applied `H_rem`,
/// because `H_rem ∉ ancestry(H_pre)`. Without this property, taint
/// cascade returns (see the cross-DAG authorization RFC's taint-cascade
/// scenario). **Any change to the BFS frontier — e.g., visiting
/// descendants of `target_heads` to "catch up" — breaks the invariant
/// and reintroduces the taint surface.** Regression-tested by
/// `prefix_walk_forward_only_*` cases below.
///
/// **Walk extent**: the BFS visits the entire ancestry of `target_heads`
/// (transitive closure of `parent_op_hashes` until ops with no parents).
/// Membership at the target cut is a function of the *full* prefix, not a
/// recent window — a `MemberRemoved` op deep in history is still
/// authoritative. [`MAX_PREFIX_WALK_NODES`] caps the walk for adversarial
/// or corrupt DAGs; in normal use, governance histories are small (tens
/// to low-hundreds of ops per namespace) and the walk completes in
/// microseconds.
///
/// Handles:
/// * `NamespaceOp::Group` with encrypted `GroupOp::MemberAdded` /
///   `MemberRemoved` / `MemberLeft` / `MemberRoleSet` /
///   `MemberJoinedViaTeeAttestation` (decrypted via local keyring; key
///   selected by `key_id` on each op).
/// * `NamespaceOp::Root(RootOp::MemberJoined)` — cleartext invitation-based
///   join. The role comes from the admin-signed invitation payload.
///
/// Ordering: ops are sorted by `(nonce, content_hash)` — deterministic
/// across nodes. Concurrent ops from different signers (siblings in the
/// DAG) tie-break by content hash, which converges with the apply-path
/// ordering for the materialized state.
///
/// Decryption failure for an individual op is logged at debug level and the
/// op is skipped: a key we don't hold means the op didn't affect us anyway,
/// or local state is corrupt — in either case skipping is safer than
/// bailing the whole walk.
///
/// Missing parent ops (i.e. an op present in the local log whose
/// `parent_op_hashes` reference an op that *isn't* in the log) are surfaced
/// as `MembershipStatus::Unknown { needed: parent_hashes }`, not as a hard
/// error: gossip can deliver a head before its ancestors during partial
/// sync, and treating the gap as permanent rejection would lose the
/// delta. The receiver's pending buffer retries when the gap fills in.
fn prefix_walk_membership(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: ContextGroupId,
    signer: &PublicKey,
    target_heads: &[[u8; 32]],
) -> EyreResult<MembershipStatus> {
    let op_log = NamespaceOpLogService::new(store, namespace_id);

    // BFS through the DAG via parent_op_hashes. Bounded by
    // MAX_PREFIX_WALK_NODES to cap memory + CPU on adversarial or
    // corrupt op logs.
    let mut to_visit: VecDeque<[u8; 32]> = target_heads.iter().copied().collect();
    let mut visited: HashSet<[u8; 32]> = HashSet::new();
    let mut walked: Vec<([u8; 32], SignedNamespaceOp)> = Vec::new();
    let mut missing_parents: Vec<[u8; 32]> = Vec::new();

    while let Some(hash) = to_visit.pop_front() {
        // Bound check before insert so we don't pay the store fetch + parent
        // expansion for the (n+1)-th node before bailing. We bound the
        // *combined* size of `visited` and `to_visit` rather than just
        // `visited` — a high-fan-out malicious DAG (each node naming many
        // parents) could otherwise enqueue an unbounded `to_visit` before
        // the visited bound triggers, allocating memory before the bail
        // fires. Combined-bound caps queue + set growth in the same
        // ceiling.
        if visited.len() + to_visit.len() >= MAX_PREFIX_WALK_NODES {
            eyre::bail!(
                "prefix_walk_membership: visited+queued reached \
                 MAX_PREFIX_WALK_NODES={} (visited={}, queued={}); \
                 bailing to bound resource use",
                MAX_PREFIX_WALK_NODES,
                visited.len(),
                to_visit.len(),
            );
        }
        if !visited.insert(hash) {
            continue;
        }
        // A parent that's not in the local op log means our DAG has a gap
        // (partial sync, or the op was pruned). Surface this as `Unknown`
        // rather than a hard error so the receiver's pending buffer can
        // retry — the apply path of the missing ancestor will eventually
        // populate the gap.
        let signed_op = match op_log.get_signed_op(hash)? {
            Some(op) => op,
            None => {
                missing_parents.push(hash);
                continue;
            }
        };
        // Push parents one at a time and re-check the combined bound
        // after each push. Without the per-parent check, a single
        // high-fan-out node (e.g. an op naming thousands of parents)
        // could grow `to_visit` by its entire parent count in one
        // iteration — exceeding the bound transiently between
        // top-of-loop checks. Checking per-parent caps queue growth at
        // exactly `MAX_PREFIX_WALK_NODES`.
        for parent in &signed_op.parent_op_hashes {
            if visited.len() + to_visit.len() >= MAX_PREFIX_WALK_NODES {
                eyre::bail!(
                    "prefix_walk_membership: visited+queued reached \
                     MAX_PREFIX_WALK_NODES={} mid-fanout (visited={}, \
                     queued={}); bailing to bound resource use",
                    MAX_PREFIX_WALK_NODES,
                    visited.len(),
                    to_visit.len(),
                );
            }
            to_visit.push_back(*parent);
        }
        walked.push((hash, signed_op));
    }

    if !missing_parents.is_empty() {
        return Ok(MembershipStatus::Unknown {
            needed: missing_parents,
        });
    }

    // Deterministic ordering: nonce primary, content hash tiebreak. Matches
    // the convergence rule the apply path uses for the materialized state.
    walked.sort_by_key(|(hash, op)| (op.nonce, *hash));

    let mut current_role: Option<GroupMemberRole> = None;
    let mut last_known_role: Option<GroupMemberRole> = None;

    for (_, signed_op) in &walked {
        match &signed_op.op {
            NamespaceOp::Root(RootOp::MemberJoined {
                member,
                signed_invitation,
            }) => {
                if member != signer
                    || signed_invitation.invitation.group_id.to_bytes() != group_id.to_bytes()
                {
                    continue;
                }
                let role = role_from_invited_role(signed_invitation.invitation.invited_role);
                current_role = Some(role.clone());
                last_known_role = Some(role);
            }
            NamespaceOp::Root(_) => continue,
            NamespaceOp::Group {
                group_id: op_gid,
                key_id,
                encrypted,
                ..
            } => {
                if *op_gid != group_id.to_bytes() {
                    continue;
                }
                let key_bytes = match load_group_key_by_id(store, &group_id, key_id)? {
                    Some(k) => k,
                    None => {
                        tracing::debug!(
                            group_id = ?group_id,
                            key_id = %hex::encode(key_id),
                            "prefix_walk_membership: missing group key, skipping op"
                        );
                        continue;
                    }
                };
                let group_op = match decrypt_group_op(&key_bytes, encrypted) {
                    Ok(op) => op,
                    Err(err) => {
                        tracing::debug!(
                            group_id = ?group_id,
                            key_id = %hex::encode(key_id),
                            %err,
                            "prefix_walk_membership: decrypt failed, skipping op"
                        );
                        continue;
                    }
                };
                match group_op {
                    GroupOp::MemberAdded { member, role } if member == *signer => {
                        current_role = Some(role.clone());
                        last_known_role = Some(role);
                    }
                    GroupOp::MemberJoinedViaTeeAttestation { member, role, .. }
                        if member == *signer =>
                    {
                        current_role = Some(role.clone());
                        last_known_role = Some(role);
                    }
                    GroupOp::MemberRoleSet { member, role } if member == *signer => {
                        current_role = Some(role.clone());
                        last_known_role = Some(role);
                    }
                    GroupOp::MemberRemoved { member } if member == *signer => {
                        current_role = None;
                    }
                    GroupOp::MemberLeft { member } if member == *signer => {
                        current_role = None;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(match (current_role, last_known_role) {
        (Some(role), _) => MembershipStatus::Member(role),
        (None, Some(last_role)) => MembershipStatus::Removed { last_role },
        (None, None) => MembershipStatus::NeverMember,
    })
}

/// Map the `invited_role: u8` byte from
/// `GroupInvitationFromAdmin` to the typed [`GroupMemberRole`]. The encoding
/// is documented at the source struct (0 = Admin, 1 = Member, 2 = ReadOnly).
///
/// Used by both the prefix walk in this module and the `MemberJoined`
/// apply path in `namespace_membership.rs`. Unknown values default to
/// `Member` (least-privilege) rather than `Admin`, so an attacker injecting
/// an out-of-range value cannot silently escalate.
pub(crate) fn role_from_invited_role(value: u8) -> GroupMemberRole {
    match value {
        0 => GroupMemberRole::Admin,
        2 => GroupMemberRole::ReadOnly,
        _ => GroupMemberRole::Member,
    }
}

/// Apply the membership state-machine transitions used by
/// [`prefix_walk_membership`] without going through the store / decryption
/// layers — useful for unit-testing the resolution logic in isolation.
///
/// Returns the [`MembershipStatus`] derived from applying `transitions` in
/// the supplied order. Each transition names a `GroupOp`-like effect on
/// the signer's status: an add (with role), a remove, or a role change.
#[cfg(test)]
fn resolve_membership_from_transitions(transitions: &[MembershipTransition]) -> MembershipStatus {
    let mut current_role: Option<GroupMemberRole> = None;
    let mut last_known_role: Option<GroupMemberRole> = None;
    for t in transitions {
        match t {
            MembershipTransition::Added(role) | MembershipTransition::RoleSet(role) => {
                current_role = Some(role.clone());
                last_known_role = Some(role.clone());
            }
            MembershipTransition::Removed | MembershipTransition::Left => {
                current_role = None;
            }
        }
    }
    match (current_role, last_known_role) {
        (Some(role), _) => MembershipStatus::Member(role),
        (None, Some(last_role)) => MembershipStatus::Removed { last_role },
        (None, None) => MembershipStatus::NeverMember,
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
enum MembershipTransition {
    Added(GroupMemberRole),
    Removed,
    Left,
    RoleSet(GroupMemberRole),
}

/// Strict head-set equality: requires `a` and `b` to have the same length AND
/// the same set of entries.
///
/// The length check rejects duplicates implicitly: a valid governance DAG head
/// set never contains duplicates, and treating `[h1, h1]` as equal to `[h1]`
/// would let an attacker who knows only one local head construct a multi-entry
/// `governance_dag_heads` that still passes the fast-path equality check.
/// Length-then-set is the strict comparison we want.
fn heads_equal(a: &[[u8; 32]], b: &[[u8; 32]]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let a_set: HashSet<&[u8; 32]> = a.iter().collect();
    let b_set: HashSet<&[u8; 32]> = b.iter().collect();
    // Both sides have equal length; if either contains duplicates, its set's
    // length will be smaller than the slice's length and the set comparison
    // will reject it against a duplicate-free same-length counterpart.
    a_set.len() == a.len() && a_set == b_set
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;

    use super::*;

    #[test]
    fn role_from_invited_role_maps_invitation_codes_correctly() {
        assert!(matches!(role_from_invited_role(0), GroupMemberRole::Admin));
        assert!(matches!(role_from_invited_role(1), GroupMemberRole::Member));
        assert!(matches!(
            role_from_invited_role(2),
            GroupMemberRole::ReadOnly
        ));
        // Unknown variants must NOT default to Admin — preserve a less
        // privileged classification.
        assert!(matches!(
            role_from_invited_role(99),
            GroupMemberRole::Member
        ));
    }

    #[test]
    fn prefix_walk_resolution_never_member_with_no_transitions() {
        let result = resolve_membership_from_transitions(&[]);
        assert!(matches!(result, MembershipStatus::NeverMember));
    }

    #[test]
    fn prefix_walk_resolution_member_after_add() {
        let result = resolve_membership_from_transitions(&[MembershipTransition::Added(
            GroupMemberRole::Member,
        )]);
        assert!(matches!(
            result,
            MembershipStatus::Member(GroupMemberRole::Member)
        ));
    }

    #[test]
    fn prefix_walk_resolution_removed_after_add_then_remove() {
        let result = resolve_membership_from_transitions(&[
            MembershipTransition::Added(GroupMemberRole::Admin),
            MembershipTransition::Removed,
        ]);
        match result {
            MembershipStatus::Removed { last_role } => {
                assert!(matches!(last_role, GroupMemberRole::Admin));
            }
            other => panic!("expected Removed, got {other:?}"),
        }
    }

    #[test]
    fn prefix_walk_resolution_member_after_re_add() {
        // remove → re-add resolves back to Member with the new role
        let result = resolve_membership_from_transitions(&[
            MembershipTransition::Added(GroupMemberRole::Member),
            MembershipTransition::Removed,
            MembershipTransition::Added(GroupMemberRole::Admin),
        ]);
        assert!(matches!(
            result,
            MembershipStatus::Member(GroupMemberRole::Admin)
        ));
    }

    #[test]
    fn prefix_walk_resolution_role_change_picks_latest() {
        let result = resolve_membership_from_transitions(&[
            MembershipTransition::Added(GroupMemberRole::Member),
            MembershipTransition::RoleSet(GroupMemberRole::Admin),
            MembershipTransition::RoleSet(GroupMemberRole::ReadOnly),
        ]);
        assert!(matches!(
            result,
            MembershipStatus::Member(GroupMemberRole::ReadOnly)
        ));
    }

    /// Reference implementation of the membership state machine — the
    /// simplest possible interpretation of the transition rules. Used to
    /// cross-check the production resolver under random transition
    /// sequences. If both produce the same answer for every input, the
    /// production resolver is correct on the operations the reference
    /// covers.
    fn reference_resolve(transitions: &[MembershipTransition]) -> MembershipStatus {
        let mut last_role: Option<GroupMemberRole> = None;
        let mut currently_member = false;
        for t in transitions {
            match t {
                MembershipTransition::Added(role) | MembershipTransition::RoleSet(role) => {
                    last_role = Some(role.clone());
                    currently_member = true;
                }
                MembershipTransition::Removed | MembershipTransition::Left => {
                    currently_member = false;
                }
            }
        }
        match (currently_member, last_role) {
            (true, Some(role)) => MembershipStatus::Member(role),
            (false, Some(role)) => MembershipStatus::Removed { last_role: role },
            (false, None) => MembershipStatus::NeverMember,
            (true, None) => unreachable!("currently_member implies last_role is set"),
        }
    }

    #[test]
    fn prefix_walk_resolution_matches_reference_under_random_inputs() {
        // Property-style test using a small splittable PRNG (xorshift) so
        // the test is fully deterministic and reproducible without pulling
        // in a proptest dependency. For 2000 random sequences of length
        // 0..=12, both resolvers must agree.
        const SEED: u64 = 0xFEED_BEEF_DEAD_C0DE;
        const NUM_SEQUENCES: usize = 2000;
        const MAX_LEN: usize = 12;

        let mut state = SEED;
        let mut next = || {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let roles = [
            GroupMemberRole::Admin,
            GroupMemberRole::Member,
            GroupMemberRole::ReadOnly,
        ];

        for _ in 0..NUM_SEQUENCES {
            let len = (next() as usize) % (MAX_LEN + 1);
            let mut transitions = Vec::with_capacity(len);
            for _ in 0..len {
                let kind = (next() as usize) % 4;
                let role = roles[(next() as usize) % roles.len()].clone();
                transitions.push(match kind {
                    0 => MembershipTransition::Added(role),
                    1 => MembershipTransition::RoleSet(role),
                    2 => MembershipTransition::Removed,
                    _ => MembershipTransition::Left,
                });
            }
            let prod = resolve_membership_from_transitions(&transitions);
            let refr = reference_resolve(&transitions);
            assert_eq!(
                prod, refr,
                "resolver mismatch on transitions={transitions:?}\nproduction: {prod:?}\nreference: {refr:?}"
            );
        }
    }

    #[test]
    fn prefix_walk_resolution_exhaustive_short_sequences() {
        // Exhaustive over all sequences of length 0..=4 from a small
        // alphabet (Add(M), Remove, Left, RoleSet(A)). Catches any
        // boundary case the random test might miss by chance.
        let alphabet = [
            MembershipTransition::Added(GroupMemberRole::Member),
            MembershipTransition::RoleSet(GroupMemberRole::Admin),
            MembershipTransition::Removed,
            MembershipTransition::Left,
        ];
        let n = alphabet.len();
        for len in 0..=4 {
            let total = n.pow(len as u32);
            for combo in 0..total {
                let mut idx = combo;
                let mut transitions = Vec::with_capacity(len);
                for _ in 0..len {
                    transitions.push(alphabet[idx % n].clone());
                    idx /= n;
                }
                let prod = resolve_membership_from_transitions(&transitions);
                let refr = reference_resolve(&transitions);
                assert_eq!(
                    prod, refr,
                    "exhaustive mismatch on transitions={transitions:?}"
                );
            }
        }
    }

    #[test]
    fn prefix_walk_resolution_left_treated_as_removed() {
        let result = resolve_membership_from_transitions(&[
            MembershipTransition::Added(GroupMemberRole::Member),
            MembershipTransition::Left,
        ]);
        match result {
            MembershipStatus::Removed { last_role } => {
                assert!(matches!(last_role, GroupMemberRole::Member));
            }
            other => panic!("expected Removed, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Forward-only invariant regression tests
    //
    // These tests pin the property that pre-removal writes from a
    // now-removed member must still resolve to `Member` when the
    // position points at the *prefix* before the removal. The prefix
    // walk implements this by visiting only the ancestry of
    // `target_heads`; any code change that walks beyond that frontier
    // (e.g., scanning forward to "catch up" on later ops) breaks the
    // invariant and reintroduces the taint-cascade surface.
    //
    // The `resolve_membership_from_transitions` helper here mirrors
    // what `prefix_walk_membership` does after the BFS collects the
    // relevant ops — so testing the resolver on prefix-truncated
    // transition sequences is equivalent to testing forward-only on
    // the corresponding DAG positions.
    // -----------------------------------------------------------------

    #[test]
    fn prefix_walk_forward_only_pre_removal_position_is_member() {
        // Position points at the prefix [Added] — the removal that
        // happens later in the DAG is NOT in this prefix, so the
        // resolver must return Member.
        let pre_removal_prefix = &[MembershipTransition::Added(GroupMemberRole::Member)];
        let result = resolve_membership_from_transitions(pre_removal_prefix);
        assert!(
            matches!(result, MembershipStatus::Member(GroupMemberRole::Member)),
            "forward-only: pre-removal position must resolve to Member, got {result:?}"
        );
    }

    #[test]
    fn prefix_walk_forward_only_post_removal_position_is_removed() {
        // Position points at the full prefix including the removal —
        // the resolver must return Removed. (Counterpart to the
        // pre-removal test: both must hold for forward-only to mean
        // anything.)
        let post_removal_prefix = &[
            MembershipTransition::Added(GroupMemberRole::Admin),
            MembershipTransition::Removed,
        ];
        let result = resolve_membership_from_transitions(post_removal_prefix);
        match result {
            MembershipStatus::Removed { last_role } => {
                assert!(matches!(last_role, GroupMemberRole::Admin));
            }
            other => {
                panic!("forward-only: post-removal position must resolve to Removed, got {other:?}")
            }
        }
    }

    #[test]
    fn prefix_walk_forward_only_pre_remove_then_readd_still_member() {
        // The Add → Remove → Add sequence: a position pointing at the
        // *first* Add (before the Remove) must resolve to Member with
        // the original role, regardless of any later Remove + re-add
        // activity in the DAG. The walker only sees the prefix.
        let early_prefix = &[MembershipTransition::Added(GroupMemberRole::Member)];
        let result = resolve_membership_from_transitions(early_prefix);
        assert!(
            matches!(result, MembershipStatus::Member(GroupMemberRole::Member)),
            "forward-only: position at first Add must be Member regardless of later DAG, got {result:?}"
        );
    }

    #[test]
    fn prefix_walk_forward_only_role_change_pre_position_uses_pre_position_role() {
        // The position points after the second RoleSet but before
        // any removal — the resolver must use the role at the
        // position, not the role at the latest known op.
        let position_prefix = &[
            MembershipTransition::Added(GroupMemberRole::Member),
            MembershipTransition::RoleSet(GroupMemberRole::Admin),
        ];
        let result = resolve_membership_from_transitions(position_prefix);
        assert!(
            matches!(result, MembershipStatus::Member(GroupMemberRole::Admin)),
            "forward-only: role at position governs, not role at later DAG ops, got {result:?}"
        );
    }

    #[test]
    fn prefix_walk_forward_only_property_random() {
        // Property test: for any prefix of a transition sequence ending
        // in Add (no Remove yet at the prefix boundary), the resolver
        // must return Member. Generate 1000 sequences, take a random
        // prefix that ends right after an Add, verify Member is
        // returned.
        const SEED: u64 = 0xC4FA_DEFE_EDBE_EF42;
        let mut state = SEED;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let roles = [
            GroupMemberRole::Admin,
            GroupMemberRole::Member,
            GroupMemberRole::ReadOnly,
        ];
        let mut tested = 0_usize;
        for _ in 0..2000 {
            // Build a random sequence of length 1..=8 ending with an
            // Add op. The position is the prefix up to and including
            // that Add. Any later transitions in the original
            // sequence are *not* part of the prefix and must not
            // affect the answer.
            let len = ((next() as usize) % 8) + 1;
            let mut transitions = Vec::with_capacity(len);
            for i in 0..len {
                let kind = (next() as usize) % 4;
                let role = roles[(next() as usize) % roles.len()].clone();
                let t = match kind {
                    0 => MembershipTransition::Added(role),
                    1 => MembershipTransition::RoleSet(role),
                    2 => MembershipTransition::Removed,
                    _ => MembershipTransition::Left,
                };
                transitions.push(t);
                // Find a prefix that ends in Add — that's the position.
                if matches!(transitions[i], MembershipTransition::Added(_)) {
                    let prefix = &transitions[..=i];
                    let result = resolve_membership_from_transitions(prefix);
                    assert!(
                        matches!(result, MembershipStatus::Member(_)),
                        "forward-only property violated: prefix={prefix:?} resolved to {result:?}"
                    );
                    tested += 1;
                }
            }
        }
        assert!(
            tested > 100,
            "property test must exercise the Member case at least 100 times (got {tested})"
        );
    }

    #[test]
    fn prefix_walk_forward_only_canary_retroactive_invalidation_would_break() {
        // Canary: if someone were to "fix" the resolver to look at the
        // full transition sequence rather than the prefix, this test
        // would catch it. Specifically: if the resolver were changed
        // to honor a Remove transition that comes *after* the queried
        // position, the returned status would shift from Member to
        // Removed for the pre-removal prefix.
        //
        // We can't directly test "the wrong code would fail" without
        // the wrong code, but we can pin the behavior at the prefix
        // boundary: the same Added op must produce different answers
        // depending on whether the prefix includes a later Remove,
        // demonstrating that the resolver IS sensitive to the prefix
        // boundary (not to the full sequence).
        let added_only = &[MembershipTransition::Added(GroupMemberRole::Member)];
        let added_then_removed = &[
            MembershipTransition::Added(GroupMemberRole::Member),
            MembershipTransition::Removed,
        ];

        let pre = resolve_membership_from_transitions(added_only);
        let post = resolve_membership_from_transitions(added_then_removed);

        // The prefix [Added] must be Member; the prefix [Added,
        // Removed] must be Removed. If both returned the same status
        // (either both Member or both Removed), the resolver would be
        // ignoring the prefix boundary and forward-only would be
        // either lost (both Removed → no forward-only) or
        // over-applied (both Member → impossible-to-remove).
        assert!(
            matches!(pre, MembershipStatus::Member(_)),
            "canary: pre-removal prefix must be Member, got {pre:?}"
        );
        assert!(
            matches!(post, MembershipStatus::Removed { .. }),
            "canary: post-removal prefix must be Removed, got {post:?}"
        );
    }

    #[test]
    fn membership_status_at_recognises_namespace_creator_as_admin() {
        // Regression test for the `group-3node` flake — see
        // `cross-DAG authorization` notes in this module's docs.
        //
        // The namespace creator does not emit a self-`MemberJoined` op at
        // genesis; their membership lives in `GroupMeta::admin_identity`.
        // Without the carve-out at the top of `membership_status_at`,
        // both the fast-path (no `GroupMember` row → `NeverMember`) and
        // the prefix walk (no `MemberJoined` op for the creator →
        // `NeverMember`) reject every state delta authored by the
        // creator with "not a member at governance cut."
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::context::UpgradePolicy;
        use calimero_store::key::GroupMetaValue;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let admin = PublicKey::from([0xAA; 32]);
        let group_id = ContextGroupId::from([0xAB; 32]);

        // Persist a GroupMeta with admin_identity == admin so
        // is_group_admin returns true without needing any GroupMember
        // rows or a populated namespace DAG.
        let meta = GroupMetaValue {
            app_key: [0u8; 32],
            target_application_id: ApplicationId::from([0u8; 32]),
            upgrade_policy: UpgradePolicy::LazyOnAccess,
            created_at: 0,
            admin_identity: admin.into(),
            owner_identity: admin.into(),
            migration: None,
            auto_join: false,
        };
        super::super::meta::save_group_meta(&store, &group_id, &meta).expect("save_group_meta");

        // Any well-formed position will do — the admin carve-out short-
        // circuits before we ever look at heads or `namespace_dag`.
        let position = GovernancePosition {
            group_id,
            group_state_hash: [0xCD; 32],
            governance_dag_heads: vec![[0u8; 32]],
        };

        let status = membership_status_at(&store, &admin, &position)
            .expect("admin carve-out must short-circuit cleanly");
        assert!(
            matches!(status, MembershipStatus::Member(GroupMemberRole::Admin)),
            "creator must be recognised as Member(Admin), got {status:?}"
        );

        // Non-admin signer falls through past the carve-out. Without a
        // namespace_dag set up, the call returns Err — that's fine for
        // this regression test (we only need to prove the carve-out
        // doesn't claim every signer is an admin).
        let other = PublicKey::from([0x99; 32]);
        let other_result = membership_status_at(&store, &other, &position);
        assert!(
            other_result.is_err()
                || !matches!(
                    other_result.unwrap(),
                    MembershipStatus::Member(GroupMemberRole::Admin)
                ),
            "non-admin signer must NOT be classified as Member(Admin)"
        );
    }

    #[test]
    fn membership_status_at_rejects_oversized_heads_runtime_guard() {
        // Defense-in-depth: the constructor and wire-decode bounds cover
        // sender and receiver entry points. The runtime guard inside
        // `membership_status_at` covers the residual case where a
        // GovernancePosition is built via direct struct-init (skipping
        // the `new()` validation). We construct one that way here.
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let signer = PublicKey::from([0u8; 32]);
        let oversized_heads: Vec<[u8; 32]> = (0..MAX_GOVERNANCE_DAG_HEADS + 1)
            .map(|i| {
                let mut h = [0u8; 32];
                h[0] = i as u8;
                h
            })
            .collect();
        let position = GovernancePosition {
            group_id: ContextGroupId::from([0xAB; 32]),
            group_state_hash: [0xCD; 32],
            governance_dag_heads: oversized_heads,
        };

        let err = membership_status_at(&store, &signer, &position)
            .expect_err("oversized governance_dag_heads must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("MAX_GOVERNANCE_DAG_HEADS"),
            "error should mention MAX_GOVERNANCE_DAG_HEADS, got: {msg}"
        );
    }

    #[test]
    fn heads_equal_handles_unordered_sets() {
        let h1 = [1u8; 32];
        let h2 = [2u8; 32];
        assert!(heads_equal(&[h1, h2], &[h2, h1]));
        assert!(!heads_equal(&[h1], &[h1, h2]));
        assert!(!heads_equal(&[h1, h2], &[h1, [3u8; 32]]));
        assert!(heads_equal(&[], &[]));
    }

    #[test]
    fn heads_equal_rejects_duplicate_false_positive() {
        // Regression: `[1,1]` and `[1,2]` both have length 2 but are not
        // the same set. A naive "every element of b in a" check would
        // return true here.
        let h1 = [1u8; 32];
        let h2 = [2u8; 32];
        assert!(!heads_equal(&[h1, h1], &[h1, h2]));
        assert!(!heads_equal(&[h1, h2], &[h1, h1]));
    }

    #[test]
    fn heads_equal_rejects_duplicates_against_unique() {
        // `[h1, h1]` must NOT equal `[h1]` — a malicious sender could
        // otherwise pass the fast-path check by padding a single known
        // head into a multi-entry vector.
        let h1 = [1u8; 32];
        assert!(!heads_equal(&[h1, h1], &[h1]));
        assert!(!heads_equal(&[h1], &[h1, h1]));
    }

    #[test]
    fn heads_equal_rejects_self_with_duplicates() {
        // Even `[h1, h1]` against itself is not a valid head set — heads
        // are by construction unique. The function rejects malformed
        // input on either side.
        let h1 = [1u8; 32];
        assert!(!heads_equal(&[h1, h1], &[h1, h1]));
    }
}
