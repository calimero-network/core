//! Candidate ordering for blob probes.
//!
//! Probing is a bounded search, so *which* peers land in the first batch decides
//! whether one round trip suffices. Availability nodes are always on, publicly
//! dialable, and hold everything for the contexts they follow, so they go first.
//!
//! Ordering is applied BEFORE the candidate list is chunked into probe batches,
//! and the sweep is capped at `PROBE_BATCH * MAX_PROBE_BATCHES` = 32 candidates
//! (see `blob.rs`). So on a context larger than that, this ordering does not
//! merely decide the order in which candidates are asked — it decides WHICH
//! candidates are asked at all. An availability node placed 33rd by the
//! subscriber set would never be probed; placed first, it answers on the first
//! round trip. That makes anchor-first a correctness-relevant ordering on large
//! contexts, not just a latency optimisation.
//!
//! The role information lives in `calimero-node`'s governance/peer-identity
//! plumbing, which is `pub(crate)` there — and `calimero-node` depends on this
//! crate, so the import direction is wrong. The node implements this trait and
//! injects it.

use std::fmt;
use std::sync::{Arc, OnceLock};

use calimero_primitives::context::ContextId;

/// Supplies the availability nodes (`ReadOnlyTee` members) of a context.
pub trait MemberRoles: Send + Sync + 'static {
    /// Peers hosting `ReadOnlyTee` members of `context_id`, in a stable order.
    /// Empty when the context has no availability node, or none has been seen
    /// on a peer this node knows about.
    fn anchors_for_context(&self, context_id: &ContextId) -> Vec<libp2p::PeerId>;
}

/// Write-once holder for the [`MemberRoles`] implementation.
///
/// [`crate::client::NodeClient`] is constructed before the node state that
/// backs the role lookup exists, and it is cloned into several subsystems right
/// after. Sharing one `OnceLock` across those clones is what lets the node fill
/// the seam in later and have every existing clone see it.
///
/// Unset is a working state, not an error: ordering then degrades to the
/// caller's own candidate order, which is what every pre-Task-4 caller had.
#[derive(Clone, Default)]
pub struct MemberRolesSlot(Arc<OnceLock<Arc<dyn MemberRoles>>>);

impl fmt::Debug for MemberRolesSlot {
    // `dyn MemberRoles` is deliberately not `Debug` — implementors carry stores
    // and caches whose contents have no business in a log line — so report only
    // whether the seam has been filled.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MemberRolesSlot")
            .field(&if self.0.get().is_some() {
                "installed"
            } else {
                "unset"
            })
            .finish()
    }
}

impl MemberRolesSlot {
    /// Install the implementation. Returns `false` if one was already
    /// installed, in which case this call had no effect.
    pub fn install(&self, roles: Arc<dyn MemberRoles>) -> bool {
        self.0.set(roles).is_ok()
    }

    /// Availability peers for `context_id`, or empty when no implementation has
    /// been installed.
    #[must_use]
    pub fn anchors_for_context(&self, context_id: &ContextId) -> Vec<libp2p::PeerId> {
        self.0
            .get()
            .map(|roles| roles.anchors_for_context(context_id))
            .unwrap_or_default()
    }
}

/// Order probe candidates: anchors first, then everything else, preserving the
/// caller's order within each group and dropping duplicates.
#[must_use]
pub fn order_candidates(
    candidates: Vec<libp2p::PeerId>,
    anchors: &[libp2p::PeerId],
) -> Vec<libp2p::PeerId> {
    let mut ordered: Vec<libp2p::PeerId> = Vec::with_capacity(candidates.len());
    for anchor in anchors {
        if candidates.contains(anchor) && !ordered.contains(anchor) {
            ordered.push(*anchor);
        }
    }
    for candidate in candidates {
        if !ordered.contains(&candidate) {
            ordered.push(candidate);
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(byte: u8) -> libp2p::PeerId {
        let keypair =
            libp2p::identity::Keypair::ed25519_from_bytes([byte; 32]).expect("valid ed25519 seed");
        libp2p::PeerId::from_public_key(&keypair.public())
    }

    #[test]
    fn anchors_are_probed_first() {
        let candidates = vec![peer(1), peer(2), peer(3)];
        let ordered = order_candidates(candidates, &[peer(3)]);
        assert_eq!(ordered, vec![peer(3), peer(1), peer(2)]);
    }

    #[test]
    fn an_anchor_that_is_not_a_candidate_is_not_invented() {
        let ordered = order_candidates(vec![peer(1)], &[peer(9)]);
        assert_eq!(ordered, vec![peer(1)]);
    }

    #[test]
    fn ordering_is_stable_and_deduplicated() {
        let ordered = order_candidates(vec![peer(1), peer(2), peer(1)], &[peer(2), peer(2)]);
        assert_eq!(ordered, vec![peer(2), peer(1)]);
    }

    /// The reason ordering runs before batching: an anchor sitting past the
    /// 32-candidate sweep cap is not merely probed late, it is never probed.
    #[test]
    fn an_anchor_beyond_the_sweep_cap_is_pulled_into_the_first_batch() {
        let mut candidates: Vec<libp2p::PeerId> = (0..40_u8).map(peer).collect();
        let anchor = peer(39);
        assert_eq!(candidates.last(), Some(&anchor));

        candidates = order_candidates(candidates, &[anchor]);

        assert_eq!(candidates[0], anchor);
        assert_eq!(candidates.len(), 40, "no candidate is dropped");
    }

    #[test]
    fn an_unset_slot_orders_by_the_callers_own_order() {
        let slot = MemberRolesSlot::default();
        assert!(slot
            .anchors_for_context(&ContextId::from([0x11; 32]))
            .is_empty());
    }

    #[test]
    fn an_installed_slot_answers_and_refuses_replacement() {
        struct Fixed(Vec<libp2p::PeerId>);
        impl MemberRoles for Fixed {
            fn anchors_for_context(&self, _context_id: &ContextId) -> Vec<libp2p::PeerId> {
                self.0.clone()
            }
        }

        let slot = MemberRolesSlot::default();
        assert!(slot.install(Arc::new(Fixed(vec![peer(7)]))));
        assert!(
            !slot.install(Arc::new(Fixed(vec![peer(8)]))),
            "the seam is write-once"
        );
        assert_eq!(
            slot.anchors_for_context(&ContextId::from([0x11; 32])),
            vec![peer(7)]
        );
    }
}
