//! Who recently served this node a blob, per context.
//!
//! Discovery probes the context's subscribers in a fixed order, and the order
//! decides how many round trips a miss costs — and, once the sweep runs out of
//! deadline or hits its ceiling, whether the holder is asked at all. Anchors go
//! first because they hold everything. This module supplies the *next* best
//! guess for contexts that have no anchor, or whose anchor is down: a peer that
//! already served this node a blob in this context is, empirically, likely to
//! hold the next one — same members, same working set.
//!
//! **Memory-only, on purpose.** Nothing here is persisted and nothing is
//! replicated: it is a hint, not state. A cold start has an empty cache, and an
//! empty cache orders candidates exactly as they were ordered before this
//! existed — see [`super::order_candidates`]. That is the whole failure mode.
//!
//! **Bounded in both directions**, because it is fed by network activity and
//! must not grow with it: at most [`MAX_PROVIDERS_PER_CONTEXT`] peers are
//! remembered for a context, and at most [`MAX_TRACKED_CONTEXTS`] contexts are
//! tracked at all. Both evict least-recently-used.
//!
//! **Recorded on a successful fetch, not on a probe that answered yes.** A
//! probe says a peer *claims* to hold one blob; a completed fetch says it
//! served the bytes, they hashed to the id that was asked for, and the transfer
//! finished. Only the second is evidence worth ordering by, and it is also the
//! one an unreachable or lying peer cannot manufacture.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use calimero_primitives::context::ContextId;
use libp2p::PeerId;

/// How many recent providers are remembered for one context.
///
/// Sized to `PROBE_BATCH` in `super::blob`: the point of the list is to fill the
/// first probe batch with peers that have already proven they serve this
/// context, so remembering more than one batch's worth would buy ordering that
/// the first round trip never reaches.
const MAX_PROVIDERS_PER_CONTEXT: usize = 8;

/// How many contexts are tracked at once.
///
/// A node in more contexts than this keeps the hint for the ones it has fetched
/// in most recently and loses it for the rest, which is a degradation to the
/// pre-existing ordering, never a failure.
const MAX_TRACKED_CONTEXTS: usize = 32;

/// Tracked contexts, most-recently-used at the front; within each, the peers
/// that served it, most-recently-seen at the front.
///
/// A `VecDeque` of pairs rather than a map: both dimensions are capped in the
/// tens, so a linear scan costs less than a map plus a separate recency list,
/// and front-insertion with `truncate` IS the eviction.
type ContextLru = VecDeque<(ContextId, VecDeque<PeerId>)>;

/// A bounded, memory-only LRU of peers that recently served this node a blob.
///
/// Cloneable and shared: every clone of [`super::NodeClient`] must see the same
/// list, since the fetch that records a provider and the sweep that reads one
/// are different call sites on different clones.
#[derive(Clone, Debug, Default)]
pub struct RecentProviders {
    contexts: Arc<Mutex<ContextLru>>,
}

impl RecentProviders {
    /// Note that `peer_id` served this node a blob in `context_id`.
    ///
    /// Idempotent in effect: a peer already in the list is moved to the front
    /// rather than duplicated, so a peer that serves repeatedly stays first
    /// without crowding the list out.
    pub fn record(&self, context_id: &ContextId, peer_id: PeerId) {
        let mut contexts = self.contexts.lock().unwrap_or_else(PoisonError::into_inner);

        let existing = contexts
            .iter()
            .position(|(tracked, _peers)| tracked == context_id);

        // Take the entry out and push it back at the front: recording is what
        // makes a context recently-used, so it must not be the next evicted.
        let (tracked, mut peers) = match existing {
            Some(index) => contexts
                .remove(index)
                .unwrap_or_else(|| (*context_id, VecDeque::new())),
            None => (*context_id, VecDeque::new()),
        };

        if let Some(index) = peers.iter().position(|tracked| *tracked == peer_id) {
            let _moved = peers.remove(index);
        }
        peers.push_front(peer_id);
        peers.truncate(MAX_PROVIDERS_PER_CONTEXT);

        contexts.push_front((tracked, peers));
        contexts.truncate(MAX_TRACKED_CONTEXTS);
    }

    /// Peers that recently served a blob in `context_id`, most recent first.
    ///
    /// Empty for an untracked context, which is the cold-start answer and the
    /// one that reproduces the pre-existing candidate order.
    #[must_use]
    pub fn for_context(&self, context_id: &ContextId) -> Vec<PeerId> {
        self.contexts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(tracked, _peers)| tracked == context_id)
            .map(|(_tracked, peers)| peers.iter().copied().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    #[test]
    fn an_untracked_context_has_no_recent_providers() {
        let recent = RecentProviders::default();
        assert!(recent.for_context(&context(1)).is_empty());
    }

    #[test]
    fn the_most_recent_provider_comes_first() {
        let recent = RecentProviders::default();
        let (first, second) = (PeerId::random(), PeerId::random());

        recent.record(&context(1), first);
        recent.record(&context(1), second);

        assert_eq!(recent.for_context(&context(1)), vec![second, first]);
    }

    #[test]
    fn recording_a_peer_twice_moves_it_rather_than_duplicating_it() {
        let recent = RecentProviders::default();
        let (first, second) = (PeerId::random(), PeerId::random());

        recent.record(&context(1), first);
        recent.record(&context(1), second);
        recent.record(&context(1), first);

        assert_eq!(recent.for_context(&context(1)), vec![first, second]);
    }

    #[test]
    fn providers_are_bounded_per_context() {
        let recent = RecentProviders::default();
        let peers: Vec<PeerId> = (0..MAX_PROVIDERS_PER_CONTEXT * 3)
            .map(|_| PeerId::random())
            .collect();

        for peer in &peers {
            recent.record(&context(1), *peer);
        }

        let tracked = recent.for_context(&context(1));
        assert_eq!(tracked.len(), MAX_PROVIDERS_PER_CONTEXT);
        // The newest `MAX_PROVIDERS_PER_CONTEXT`, newest first: the oldest are
        // what an LRU drops.
        let expected: Vec<PeerId> = peers
            .iter()
            .rev()
            .take(MAX_PROVIDERS_PER_CONTEXT)
            .copied()
            .collect();
        assert_eq!(tracked, expected);
    }

    #[test]
    fn contexts_are_bounded_and_the_oldest_is_evicted() {
        let recent = RecentProviders::default();
        let peer = PeerId::random();

        for index in 0..=u8::try_from(MAX_TRACKED_CONTEXTS).expect("fits in u8") {
            recent.record(&context(index), peer);
        }

        // Context 0 was the first recorded and never touched again.
        assert!(
            recent.for_context(&context(0)).is_empty(),
            "the least-recently-used context is dropped"
        );
        assert_eq!(
            recent.for_context(&context(
                u8::try_from(MAX_TRACKED_CONTEXTS).expect("fits in u8")
            )),
            vec![peer]
        );
    }

    #[test]
    fn recording_refreshes_a_contexts_recency() {
        let recent = RecentProviders::default();
        let peer = PeerId::random();

        recent.record(&context(0), peer);
        // Fill the rest of the budget, then touch context 0 again so it is no
        // longer the oldest, then push one more context in.
        for index in 1..u8::try_from(MAX_TRACKED_CONTEXTS).expect("fits in u8") {
            recent.record(&context(index), peer);
        }
        recent.record(&context(0), peer);
        recent.record(
            &context(u8::try_from(MAX_TRACKED_CONTEXTS).expect("fits in u8")),
            peer,
        );

        assert_eq!(
            recent.for_context(&context(0)),
            vec![peer],
            "a refreshed context survives an eviction it would otherwise lose"
        );
        assert!(recent.for_context(&context(1)).is_empty());
    }

    #[test]
    fn contexts_do_not_share_providers() {
        let recent = RecentProviders::default();
        let (one, two) = (PeerId::random(), PeerId::random());

        recent.record(&context(1), one);
        recent.record(&context(2), two);

        assert_eq!(recent.for_context(&context(1)), vec![one]);
        assert_eq!(recent.for_context(&context(2)), vec![two]);
    }
}
