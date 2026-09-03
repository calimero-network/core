//! Peer synchronization protocols and coordination.
//!
//! This module handles all aspects of state synchronization between nodes:
//! - Network protocols (libp2p streams, encryption)
//! - Sync strategy decisions (full vs delta)
//! - Peer state tracking
//! - Protocol implementations (full, delta, state)
//! - Ancillary protocols (key sharing, blob sharing)
//! - Metrics and observability
//!
//! ## Architecture (SOLID Principles Applied)
//!
//! ```text
//! SyncManager
//! ├── Orchestrates: periodic sync, peer selection
//! ├── Decides: Use delta or full resync
//! ├── Delegates to:
//! │   ├── hash_comparison_protocol.rs - Merkle tree traversal (DFS)
//! │   ├── level_sync.rs               - Level-wise sync (BFS for wide trees)
//! │   ├── snapshot.rs                 - Snapshot transfer protocol
//! │   └── blobs.rs                    - Blob sharing
//! ├── Tracks: tracking.rs (per-peer sync history)
//! └── Observes: metrics.rs (protocol cost, safety invariants)
//! ```
//!
//! ## Metrics
//!
//! The sync module provides unified metrics through `SyncMetricsCollector`:
//! - Protocol cost: messages, bytes, round trips, entities, merges
//! - Phase timing: handshake, data_transfer, merge, sync_total
//! - Safety metrics: snapshot_blocked (I5), buffer_drops (I6), verification_failures (I7)
//!
//! See [`metrics`] module for trait definition and [`prometheus_metrics`] for production use.

/// The signing keys of a group's trusted anchors.
///
/// `trusted_anchors` answers in ACCOUNTS, because that is what "authoritative
/// here" is recorded against. Peer selection matches on the key a peer actually
/// presents, so each anchor account is expanded to its live devices. An anchor
/// running two machines is preferred at both; matching on the account alone
/// would match neither, since no peer ever presents one.
///
/// Returns an empty set on any store failure — the same conservative default the
/// callers already applied, and one that only costs a less-preferred peer choice.
pub(crate) fn anchor_device_keys(
    store: &calimero_store::Store,
    group_id: &calimero_context_config::types::ContextGroupId,
) -> std::collections::BTreeSet<calimero_primitives::identity::PublicKey> {
    let Ok(anchors) =
        calimero_governance_store::MembershipRepository::new(store).trusted_anchors(group_id)
    else {
        return std::collections::BTreeSet::new();
    };
    device_keys_for_accounts(store, group_id, &anchors)
}

/// The signing keys of a group's availability nodes — its `ReadOnlyTee`
/// members, INCLUDING those it inherits from its ancestors.
///
/// Neither a subset nor a superset of [`anchor_device_keys`] — the two answer
/// deliberately different questions and walk the group tree differently. The
/// anchor set answers "who is authoritative in THIS group" (Owner ∪ Admins ∪
/// ReadOnlyTee of `group_id` alone, no ancestor walk), which is what sync peer
/// selection wants. This set answers "who is always on and holds the bytes for
/// this context", which includes `ReadOnlyTee` members inherited from
/// ancestors: a root-admitted `ReadOnlyTee` over an `Open` subgroup is an
/// availability node for that subgroup's contexts without ever holding a
/// direct membership row there, so it appears here but not in
/// [`anchor_device_keys`] for the subgroup. Conversely an ordinary admin of
/// `group_id` appears in the anchor set but never here, since only
/// `ReadOnlyTee` answers "holds the bytes".
///
/// Same account→device expansion as the anchor set, for the same reason.
/// Returns an empty set on any store failure — callers then fall back to
/// unordered candidates, which costs a round trip and never correctness.
pub(crate) fn availability_device_keys(
    store: &calimero_store::Store,
    group_id: &calimero_context_config::types::ContextGroupId,
) -> std::collections::BTreeSet<calimero_primitives::identity::PublicKey> {
    let accounts = availability_accounts_for_group(store, group_id);
    device_keys_for_accounts(store, group_id, &accounts)
}

/// The ACCOUNTS that are availability nodes for `group_id`: every `ReadOnlyTee`
/// member of the group itself, unioned with every `ReadOnlyTee` member of each
/// ancestor up to the namespace root.
///
/// The parent walk is the whole point, and it is why this is ONE function
/// rather than one per caller. A TEE admitted at the namespace root holds NO
/// direct membership row in an `Open` subgroup it follows by inheritance, yet
/// it is an availability node for that subgroup's contexts — which is the real
/// fleet-HA shape, not a corner case. A send side that looked only at the
/// context's own group would find nobody to announce to and nobody to probe
/// first, while the receive side happily accepted announcements, and every unit
/// test on either side would stay green. Both sides call this, so they agree by
/// construction rather than by coincidence.
///
/// Empty on any store failure, and bounded by `MAX_NAMESPACE_DEPTH` hops.
pub(crate) fn availability_accounts_for_group(
    store: &calimero_store::Store,
    group_id: &calimero_context_config::types::ContextGroupId,
) -> std::collections::BTreeSet<calimero_account::AccountId> {
    let members = calimero_governance_store::MembershipRepository::new(store);
    let namespaces = calimero_governance_store::NamespaceRepository::new(store);

    let mut accounts = std::collections::BTreeSet::new();
    let mut current = *group_id;
    // `<=` because reaching the root at depth D takes D+1 parent hops to
    // observe the root's `None` parent (mirrors `NamespaceRepository::resolve`).
    for _ in 0..=calimero_context_config::MAX_NAMESPACE_DEPTH {
        // A failed read at one level is not fatal: keep walking, and the
        // conservative outcome is a smaller set (a missed announce, never a
        // wrong one).
        if let Ok(list) = members.list(&current, 0, usize::MAX) {
            accounts.extend(list.into_iter().filter_map(|(account, role)| {
                matches!(
                    role,
                    calimero_primitives::context::GroupMemberRole::ReadOnlyTee
                )
                .then_some(account)
            }));
        }
        match namespaces.parent(&current) {
            Ok(Some(parent)) => current = parent,
            // Root reached (`None`), or a store error: either way there is no
            // further ancestor to consult.
            _ => break,
        }
    }
    accounts
}

/// Expand governance ACCOUNTS to the live DEVICE signing keys that speak for
/// them, within the namespace owning `group_id`.
///
/// Shared by [`anchor_device_keys`] and [`availability_device_keys`]: the two
/// differ only in which accounts they select, never in how an account is
/// resolved to the keys a peer actually presents.
fn device_keys_for_accounts(
    store: &calimero_store::Store,
    group_id: &calimero_context_config::types::ContextGroupId,
    accounts: &std::collections::BTreeSet<calimero_account::AccountId>,
) -> std::collections::BTreeSet<calimero_primitives::identity::PublicKey> {
    if accounts.is_empty() {
        return std::collections::BTreeSet::new();
    }
    let Ok(namespace) =
        calimero_governance_store::NamespaceRepository::new(store).resolve(group_id)
    else {
        return std::collections::BTreeSet::new();
    };
    let Ok(bindings) =
        calimero_governance_store::AccountBindingRepository::new(store).live_bindings(&namespace)
    else {
        return std::collections::BTreeSet::new();
    };
    bindings
        .iter()
        .filter(|binding| accounts.contains(&binding.account))
        .map(|binding| binding.sign_pk)
        .collect()
}

mod blobs;
mod config;
pub(crate) mod delta_request;

/// Maximum ops exchanged in a single namespace backfill response, capping
/// memory use from large namespace governance DAGs. Enforced on BOTH ends: the
/// responder never sends more, and the receiver never applies more, so a
/// misbehaving responder cannot push an unbounded batch either way.
pub(crate) const MAX_BACKFILL_OPS: usize = 500;

pub(crate) mod driver;
mod hash_comparison;
pub mod hash_comparison_protocol;
pub(crate) mod helpers;
pub mod level_sync;
mod manager;
pub mod metrics;
pub(crate) mod network;
pub(crate) mod parent_pull;
pub(crate) mod peers;
pub mod prometheus_metrics;
pub(crate) mod protocol_selector;
pub(crate) mod reconciler;
pub mod rotation_log_reader;
pub(crate) mod session;
pub(crate) mod snapshot;
pub(crate) mod state_access;
#[cfg(test)]
pub(crate) mod state_access_mock;
pub(crate) mod stream;
mod tracking;

// Cross-node integration tests for the four motivating partition scenarios
// of #2197 / ADR 0001. Migrated from `calimero_storage::tests` per #2266
// step 5 — they exercise the production sync-layer flow: load rotation log,
// resolve `effective_writers` via `rotation_log_reader::writers_at_authenticated`,
// apply.
#[cfg(test)]
mod p3_dag_causal_tests;
#[cfg(test)]
mod p5_partition_scenarios_tests;
// Shared scaffolding for the P3/P5 tests above (the `Dag` topology mirror).
#[cfg(test)]
mod test_helpers;

pub use config::SyncConfig;
// Re-export for integration tests so they can mirror the production
// resolve flow without copying the BFS body (#2272 review).
pub use crate::delta_store::happens_before_in_topology;
pub use hash_comparison_protocol::{
    HashComparisonConfig, HashComparisonFirstRequest, HashComparisonProtocol, HashComparisonStats,
};
pub use level_sync::{LevelWiseConfig, LevelWiseFirstRequest, LevelWiseProtocol, LevelWiseStats};
pub use manager::SyncManager;
// The migration facts builder judges a context against the same gate that
// declines its state sync, so the two can never disagree about convergence.
pub(crate) use manager::pending_upgrade_target_in;
// `mod reconciler` is `pub(crate)` to `sync/`; these re-exports give
// `crate::state` a stable path to the reconcile-attempt helpers from
// its `SyncStateAccess` impl on `NodeState`. The helpers can't be
// inlined into `state.rs` without duplicating the logic that the
// dedicated tests in `sync/reconciler.rs` already cover against
// synthetic `DashMap` inputs. Keeping the helpers as implementation
// details of `sync::reconciler` and exposing them via these narrow
// re-exports preserves both surfaces.
pub use metrics::{no_op_metrics, NoOpMetrics, PhaseTimer, SharedMetrics, SyncMetricsCollector};
pub use prometheus_metrics::PrometheusSyncMetrics;
pub(crate) use reconciler::{
    reconcile_remaining_cooldown, record_reconcile_failure, record_reconcile_success,
};
