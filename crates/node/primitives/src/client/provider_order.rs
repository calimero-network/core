//! Candidate ordering for blob probes.
//!
//! Probing is a bounded search, so *which* peers land in the first batch decides
//! whether one round trip suffices. Availability nodes are always on, publicly
//! dialable, and hold everything for the contexts they follow, so they go
//! first. Behind them go peers that recently served this node a blob in the
//! same context — see `super::recent_providers` — because same context means
//! same members and much the same working set, so the peer that had the last
//! blob is a better guess than an arbitrary subscriber. Everything else keeps
//! the order the network layer returned, which is what the whole list was
//! before either tier existed.
//!
//! Ordering is applied BEFORE the candidate list is chunked into probe batches,
//! and a sweep stops at `DISCOVERY_DEADLINE` — or at `MAX_PROBE_CANDIDATES`
//! candidates at the very latest (see `blob.rs`). So on a context whose
//! subscriber set outruns either bound, this ordering does not merely decide
//! the order in which candidates are asked — it decides WHICH candidates are
//! asked at all. An availability node placed last by the subscriber set might
//! never be probed; placed first, it answers on the first round trip. That
//! makes anchor-first a correctness-relevant ordering on large contexts, not
//! just a latency optimisation.
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

/// Order probe candidates: anchors, then recent providers, then everything
/// else, preserving the caller's order within each tier and dropping
/// duplicates.
///
/// A peer named in `anchors` or `recent` that is not in `candidates` is NOT
/// invented: only the subscriber set decides who exists, and both hint lists
/// can name a peer that has since gone away.
///
/// `recent` empty is the cold-start case and the only failure mode this tier
/// has: the result is then exactly what anchors-then-the-rest produced before
/// it existed.
#[must_use]
pub fn order_candidates(
    candidates: Vec<libp2p::PeerId>,
    anchors: &[libp2p::PeerId],
    recent: &[libp2p::PeerId],
) -> Vec<libp2p::PeerId> {
    let mut ordered: Vec<libp2p::PeerId> = Vec::with_capacity(candidates.len());
    for hinted in anchors.iter().chain(recent) {
        if candidates.contains(hinted) && !ordered.contains(hinted) {
            ordered.push(*hinted);
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
        let ordered = order_candidates(candidates, &[peer(3)], &[]);
        assert_eq!(ordered, vec![peer(3), peer(1), peer(2)]);
    }

    #[test]
    fn an_anchor_that_is_not_a_candidate_is_not_invented() {
        let ordered = order_candidates(vec![peer(1)], &[peer(9)], &[]);
        assert_eq!(ordered, vec![peer(1)]);
    }

    #[test]
    fn ordering_is_stable_and_deduplicated() {
        let ordered = order_candidates(vec![peer(1), peer(2), peer(1)], &[peer(2), peer(2)], &[]);
        assert_eq!(ordered, vec![peer(2), peer(1)]);
    }

    /// The reason ordering runs before batching: an anchor deep in the
    /// candidate list is not merely probed late, it may never be probed at all
    /// once the sweep runs out of deadline or hits its ceiling.
    #[test]
    fn an_anchor_beyond_the_sweep_cap_is_pulled_into_the_first_batch() {
        let mut candidates: Vec<libp2p::PeerId> = (0..255_u8).map(peer).collect();
        let anchor = peer(254);
        assert_eq!(candidates.last(), Some(&anchor));

        candidates = order_candidates(candidates, &[anchor], &[]);

        assert_eq!(candidates[0], anchor);
        assert_eq!(candidates.len(), 255, "no candidate is dropped");
    }

    /// The tail is where this matters: without a recent-provider list the order
    /// after the anchors is whatever `subscribed_peers` happened to return.
    #[test]
    fn a_recent_provider_is_probed_before_the_rest_of_the_tail() {
        let candidates = vec![peer(1), peer(2), peer(3), peer(4)];
        let ordered = order_candidates(candidates, &[peer(1)], &[peer(4)]);
        assert_eq!(ordered, vec![peer(1), peer(4), peer(2), peer(3)]);
    }

    /// The cold-start guarantee, stated as an equality rather than a claim: an
    /// empty cache orders exactly as the anchors-only ordering did.
    #[test]
    fn an_empty_recent_list_reproduces_the_previous_ordering() {
        let candidates = vec![peer(1), peer(2), peer(3), peer(4)];
        assert_eq!(
            order_candidates(candidates.clone(), &[peer(3)], &[]),
            vec![peer(3), peer(1), peer(2), peer(4)],
        );
        // And with no hints at all, the caller's own order survives untouched.
        assert_eq!(order_candidates(candidates.clone(), &[], &[]), candidates);
    }

    /// Anchors outrank recent providers, and a peer in both tiers is listed
    /// once.
    #[test]
    fn anchors_outrank_recent_providers_and_an_overlap_is_deduplicated() {
        let candidates = vec![peer(1), peer(2), peer(3)];
        let ordered = order_candidates(candidates, &[peer(3)], &[peer(3), peer(2)]);
        assert_eq!(ordered, vec![peer(3), peer(2), peer(1)]);
    }

    /// Same rule as for anchors: a remembered provider that has left the
    /// context is not conjured back into the candidate list.
    #[test]
    fn a_recent_provider_that_is_not_a_candidate_is_not_invented() {
        let ordered = order_candidates(vec![peer(1)], &[], &[peer(9)]);
        assert_eq!(ordered, vec![peer(1)]);
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
