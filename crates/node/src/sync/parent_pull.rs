//! Pure-function helpers for the cross-peer missing-parent fetch loop.
//!
//! Used by both the data-delta parent-pull (in `sync/manager/mod.rs`
//! `request_dag_heads_and_sync`) and the governance-op parent-pull
//! (in `handlers/network_event/namespace.rs`) to decide which peer
//! to try next and when to stop.
//!
//! Extracting this decision as a plain state machine keeps the real
//! network and store calls out of the test surface while letting us
//! verify the scheduling logic in isolation.
//!
//! See issue #2198 for the original failure mode.

use std::collections::HashSet;

use libp2p::PeerId;
use tokio::time::{Duration, Instant};

/// Outcome of asking the budget for the next peer to try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NextPeer {
    /// Try this peer.
    Peer(PeerId),
    /// No untried peer is available in the current mesh snapshot.
    /// Caller should re-fetch the mesh once and retry; if still
    /// empty, caller should give up.
    RefetchMesh,
    /// `max_additional_peers` has been reached.
    MaxPeersReached,
    /// `budget` wall-clock has elapsed.
    BudgetExhausted,
    /// Mesh has been re-fetched and still yields no untried peers.
    NoMorePeers,
}

/// Bookkeeping for the cross-peer parent-pull loop.
#[derive(Debug)]
pub(crate) struct ParentPullBudget {
    tried: HashSet<PeerId>,
    attempts: usize,
    max_additional: usize,
    started: Instant,
    budget: Duration,
    refetched: bool,
}

impl ParentPullBudget {
    /// Seed with the peer that served DAG heads / initial backfill.
    /// That peer is considered "already tried" and is never re-attempted.
    pub(crate) fn new(initial_peer: PeerId, max_additional: usize, budget: Duration) -> Self {
        let mut tried = HashSet::new();
        let _ = tried.insert(initial_peer);
        Self {
            tried,
            attempts: 0,
            max_additional,
            started: Instant::now(),
            budget,
            refetched: false,
        }
    }

    /// How many additional peers have been attempted (not counting the initial peer).
    pub(crate) fn attempts(&self) -> usize {
        self.attempts
    }

    /// Total peer attempts including the initial peer.
    pub(crate) fn total_attempts(&self) -> usize {
        self.attempts + 1
    }

    /// Decide what to do next given the current mesh snapshot.
    ///
    /// The caller supplies `mesh_peers` already fetched from the network
    /// stack. The budget does not perform I/O.
    pub(crate) fn next(&mut self, mesh_peers: &[PeerId]) -> NextPeer {
        if self.attempts >= self.max_additional {
            return NextPeer::MaxPeersReached;
        }
        if self.started.elapsed() >= self.budget {
            return NextPeer::BudgetExhausted;
        }

        let untried = mesh_peers.iter().find(|p| !self.tried.contains(p)).copied();
        match untried {
            Some(peer) => NextPeer::Peer(peer),
            None if !self.refetched => NextPeer::RefetchMesh,
            None => NextPeer::NoMorePeers,
        }
    }

    /// Mark a peer as tried. Call after `next()` returned `Peer(p)` and
    /// before issuing the network request to that peer.
    pub(crate) fn record_attempt(&mut self, peer: PeerId) {
        let _ = self.tried.insert(peer);
        self.attempts += 1;
    }

    /// Record that the caller re-fetched the mesh after a `RefetchMesh` hint.
    /// The budget will then fall through to `NoMorePeers` on the next empty pass.
    pub(crate) fn record_refetch(&mut self) {
        self.refetched = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(byte: u8) -> PeerId {
        // Deterministic PeerIds for tests.
        use libp2p::identity::Keypair;
        let kp = Keypair::ed25519_from_bytes([byte; 32]).expect("ed25519 keypair");
        kp.public().to_peer_id()
    }

    #[test]
    fn initial_peer_is_never_suggested() {
        let initial = peer(1);
        let mut budget = ParentPullBudget::new(initial, 3, Duration::from_secs(10));

        let mesh = vec![initial];
        assert_eq!(budget.next(&mesh), NextPeer::RefetchMesh);
    }

    #[test]
    fn picks_untried_peer_from_mesh() {
        let initial = peer(1);
        let p2 = peer(2);
        let mut budget = ParentPullBudget::new(initial, 3, Duration::from_secs(10));

        assert_eq!(budget.next(&[initial, p2]), NextPeer::Peer(p2));
    }

    #[test]
    fn does_not_retry_same_peer() {
        let initial = peer(1);
        let p2 = peer(2);
        let mut budget = ParentPullBudget::new(initial, 3, Duration::from_secs(10));

        let first = budget.next(&[initial, p2]);
        assert_eq!(first, NextPeer::Peer(p2));
        budget.record_attempt(p2);

        // With the same mesh, p2 is now tried; should hint refetch.
        assert_eq!(budget.next(&[initial, p2]), NextPeer::RefetchMesh);
    }

    #[test]
    fn stops_at_max_additional_peers() {
        let initial = peer(1);
        let peers = [peer(2), peer(3), peer(4), peer(5)];
        let mut budget = ParentPullBudget::new(initial, 3, Duration::from_secs(10));

        // Full mesh includes the initial + 4 others.
        let mesh: Vec<PeerId> = std::iter::once(initial)
            .chain(peers.iter().copied())
            .collect();

        for expected in &peers[..3] {
            match budget.next(&mesh) {
                NextPeer::Peer(p) => {
                    assert_eq!(&p, expected, "scheduler returned wrong peer");
                    budget.record_attempt(p);
                }
                other => panic!("expected Peer, got {:?}", other),
            }
        }

        // 4th untried peer exists in the mesh, but budget caps at 3.
        assert_eq!(budget.next(&mesh), NextPeer::MaxPeersReached);
        assert_eq!(budget.attempts(), 3);
        assert_eq!(budget.total_attempts(), 4);
    }

    #[test]
    fn budget_exhausted_before_attempts_reached() {
        let initial = peer(1);
        let p2 = peer(2);
        // Zero budget so elapsed is immediately >= budget.
        let mut budget = ParentPullBudget::new(initial, 3, Duration::from_millis(0));
        // Give it a moment so `started.elapsed()` is non-zero.
        std::thread::sleep(Duration::from_millis(1));

        assert_eq!(budget.next(&[initial, p2]), NextPeer::BudgetExhausted);
    }

    #[test]
    fn refetch_then_no_more_peers() {
        let initial = peer(1);
        let mut budget = ParentPullBudget::new(initial, 3, Duration::from_secs(10));

        // Empty mesh (other than initial): first ask suggests refetch.
        assert_eq!(budget.next(&[initial]), NextPeer::RefetchMesh);
        budget.record_refetch();

        // After refetch, still empty: NoMorePeers.
        assert_eq!(budget.next(&[initial]), NextPeer::NoMorePeers);
    }

    #[test]
    fn refetch_followed_by_new_peer_is_accepted() {
        let initial = peer(1);
        let p2 = peer(2);
        let mut budget = ParentPullBudget::new(initial, 3, Duration::from_secs(10));

        assert_eq!(budget.next(&[initial]), NextPeer::RefetchMesh);
        budget.record_refetch();

        // A new peer joined the mesh after the hint; scheduler picks it up.
        assert_eq!(budget.next(&[initial, p2]), NextPeer::Peer(p2));
    }
}
