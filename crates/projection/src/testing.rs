//! Convergence + scope-isolation property harness.
//!
//! Every change to how state is projected or synced must keep two properties
//! true, and the most dangerous failure (leaking a restricted subgroup) must
//! be caught here rather than in production:
//!
//! - **Convergence (per scope):** any two replicas that are both members of a
//!   scope, having seen the same op-set in any order, compute the *same*
//!   [`ScopeState::root`]. There is no hash-neutral escape: writers and
//!   membership are folded into the root.
//! - **Isolation (partial replication):** a replica that is not a member of a
//!   scope never receives that scope's ops and therefore never holds or
//!   computes its root. Existence does not leak.
//!
//! The harness models **partial-replication delivery**: each replica folds
//! only the ops in the scopes it belongs to, in its own shuffled order. It
//! does not model the network — it pins the *model's* properties so that code
//! plugging in real delivery and the real projection has a regression net.

use std::collections::{BTreeMap, BTreeSet};

use calimero_op::{Op, ScopeId};

use crate::ScopeState;

/// One replica's outcome in a simulation: the scopes it belongs to and the
/// per-scope root it computed from the ops it was entitled to see.
#[derive(Clone, Debug)]
pub struct ReplicaView {
    /// Scopes this replica is a member of (its partial-replication set).
    pub member_of: BTreeSet<ScopeId>,
    /// `scope_root` per member scope, folded from that scope's delivered ops.
    pub roots: BTreeMap<ScopeId, [u8; 32]>,
}

/// Deterministic in-place Fisher–Yates shuffle (seeded xorshift64) — models a
/// replica observing ops in an arbitrary order, reproducibly.
fn shuffle<T>(seed: u64, items: &mut [T]) {
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for i in (1..items.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// Simulate partial-replication delivery of `ops` to replicas with the given
/// per-replica `membership`.
///
/// Each replica sees **only** the ops whose `scope` it is a member of
/// (Invariant 0 enforced at delivery — a non-member never receives a scope's
/// ops), folds them per scope in its own seed-derived order, and records the
/// resulting per-scope [`ScopeState::root`].
#[must_use]
pub fn simulate(seed: u64, membership: &[BTreeSet<ScopeId>], ops: &[Op]) -> Vec<ReplicaView> {
    membership
        .iter()
        .enumerate()
        .map(|(replica, scopes)| {
            // Partition the ops this replica is entitled to see by scope.
            let mut per_scope: BTreeMap<ScopeId, Vec<&Op>> = BTreeMap::new();
            for op in ops {
                if scopes.contains(&op.scope) {
                    per_scope.entry(op.scope).or_default().push(op);
                }
            }
            let roots = scopes
                .iter()
                .map(|scope| {
                    let mut scope_ops = per_scope.remove(scope).unwrap_or_default();
                    // Per-replica order independence: a unique seed per
                    // (seed, replica, scope) so no two folds share an order.
                    // Mix the first 8 bytes of the ScopeId (not just byte 0) so
                    // scopes that share a leading byte still get independent
                    // delivery orders.
                    let scope_seed =
                        u64::from_le_bytes(scope.as_bytes()[..8].try_into().expect("32 >= 8"));
                    shuffle(
                        seed.wrapping_add((replica as u64).wrapping_mul(0x9E37_79B9))
                            .wrapping_add(scope_seed),
                        &mut scope_ops,
                    );
                    (*scope, ScopeState::from_ops(scope_ops).root())
                })
                .collect();
            ReplicaView {
                member_of: scopes.clone(),
                roots,
            }
        })
        .collect()
}

/// Check the two invariants over a simulation result. `Err(_)` names the first
/// violation (suitable for an `assert!`/`unwrap` in a property test).
///
/// # Errors
/// - **Isolation** — a replica holds a root for a scope it is not a member of.
/// - **Convergence** — two member replicas computed different roots for a scope.
pub fn check(views: &[ReplicaView]) -> Result<(), String> {
    let mut by_scope: BTreeMap<ScopeId, [u8; 32]> = BTreeMap::new();
    for view in views {
        for (scope, root) in &view.roots {
            // Isolation: roots only ever exist for member scopes.
            if !view.member_of.contains(scope) {
                return Err(format!(
                    "isolation violated: a replica holds a root for non-member scope {scope:?}"
                ));
            }
            // Convergence: all members of a scope must agree.
            match by_scope.get(scope) {
                Some(seen) if seen != root => {
                    return Err(format!(
                        "convergence violated: scope {scope:?} roots diverge ({seen:?} vs {root:?})"
                    ));
                }
                _ => {
                    let _ = by_scope.insert(*scope, *root);
                }
            }
        }
    }
    Ok(())
}

/// Run [`simulate`] then [`check`], panicking on the first violation. The
/// one-call entry point for property tests.
///
/// # Panics
/// If convergence or isolation is violated.
pub fn assert_converges_and_isolates(seed: u64, membership: &[BTreeSet<ScopeId>], ops: &[Op]) {
    let views = simulate(seed, membership, ops);
    if let Err(violation) = check(&views) {
        panic!("scope property harness (seed={seed}): {violation}");
    }
}
