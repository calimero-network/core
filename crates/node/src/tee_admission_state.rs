//! State a TEE admission needs on each side of it.
//!
//! An admitting member issues a challenge and keeps it single-use for a short
//! window ([`TeeChallenges`]). A joiner remembers that it is waiting to be
//! admitted, so it answers a challenge offered to it and nothing else
//! ([`PendingTeeJoins`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use calimero_node_primitives::client::TeeAdmissionParams;
use libp2p::PeerId;

/// How long an issued challenge can still be answered.
pub(crate) const CHALLENGE_TTL: Duration = Duration::from_secs(60);

/// The fewest seconds between two challenges to one peer for one namespace, so
/// a joiner that re-announces every couple of seconds is not offered a new one
/// each time.
const MIN_REISSUE: Duration = Duration::from_secs(5);

/// Outstanding challenges one peer may hold at once. A new one past this evicts
/// that peer's oldest.
const MAX_PER_PEER: usize = 4;

/// Outstanding challenges in all. A peer cannot grow the book past this.
const MAX_OUTSTANDING: usize = 1024;

/// How long a joiner keeps answering offers after it last asked to be admitted.
/// `fleet-join` asks again on every call, so this only has to outlast one.
pub(crate) const PENDING_JOIN_TTL: Duration = Duration::from_secs(120);

/// The fewest seconds between two quotes a joiner makes for one namespace.
/// Generating one is not free, and any peer on the topic can offer a challenge.
const MIN_ATTEST_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug)]
struct Issued {
    namespace: [u8; 32],
    peer: PeerId,
    at: Instant,
}

/// The challenges this node has issued and not yet seen answered.
///
/// A challenge is 32 random bytes the admitting member chose. It is bound to the
/// namespace and to the peer it was issued to, it is consumed the first time it
/// is presented whatever came of it, and it lapses after [`CHALLENGE_TTL`].
#[derive(Clone, Debug, Default)]
pub(crate) struct TeeChallenges {
    issued: Arc<Mutex<HashMap<[u8; 32], Issued>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock only means a panic elsewhere; the map is still a map.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TeeChallenges {
    /// Issue a challenge to `peer` for `namespace`, or `None` when it was
    /// issued one too recently or the book is full.
    pub(crate) fn issue(&self, namespace: [u8; 32], peer: PeerId) -> Option<[u8; 32]> {
        self.issue_at(namespace, peer, Instant::now())
    }

    fn issue_at(&self, namespace: [u8; 32], peer: PeerId, now: Instant) -> Option<[u8; 32]> {
        let mut issued = lock(&self.issued);
        issued.retain(|_, entry| now.saturating_duration_since(entry.at) < CHALLENGE_TTL);

        let mut theirs: Vec<([u8; 32], Instant, bool)> = issued
            .iter()
            .filter(|(_, entry)| entry.peer == peer)
            .map(|(challenge, entry)| (*challenge, entry.at, entry.namespace == namespace))
            .collect();
        if theirs
            .iter()
            .any(|(_, at, same)| *same && now.saturating_duration_since(*at) < MIN_REISSUE)
        {
            return None;
        }
        if theirs.len() >= MAX_PER_PEER {
            theirs.sort_by_key(|(_, at, _)| *at);
            if let Some((oldest, _, _)) = theirs.first() {
                let _evicted = issued.remove(oldest);
            }
        }
        if issued.len() >= MAX_OUTSTANDING {
            return None;
        }

        let challenge: [u8; 32] = rand::random();
        let _previous = issued.insert(
            challenge,
            Issued {
                namespace,
                peer,
                at: now,
            },
        );
        Some(challenge)
    }

    /// Spend `challenge`: true only if it was issued to `peer` for `namespace`,
    /// has not been presented before and has not lapsed.
    ///
    /// Spent on the first presentation whatever its outcome, so a wrong guess
    /// cannot be retried against the same challenge.
    pub(crate) fn consume(&self, challenge: &[u8; 32], namespace: [u8; 32], peer: PeerId) -> bool {
        self.consume_at(challenge, namespace, peer, Instant::now())
    }

    fn consume_at(
        &self,
        challenge: &[u8; 32],
        namespace: [u8; 32],
        peer: PeerId,
        now: Instant,
    ) -> bool {
        let Some(entry) = lock(&self.issued).remove(challenge) else {
            return false;
        };
        entry.namespace == namespace
            && entry.peer == peer
            && now.saturating_duration_since(entry.at) < CHALLENGE_TTL
    }
}

#[derive(Debug)]
struct Pending {
    params: Arc<TeeAdmissionParams>,
    expires: Instant,
    last_attested: Option<Instant>,
}

/// The namespaces this node is waiting to be admitted to, with what it needs to
/// answer a challenge for each.
#[derive(Clone, Debug, Default)]
pub(crate) struct PendingTeeJoins {
    pending: Arc<Mutex<HashMap<[u8; 32], Pending>>>,
}

impl PendingTeeJoins {
    /// Remember that this node wants to be admitted as `params` describes.
    pub(crate) fn register(&self, params: TeeAdmissionParams) {
        self.register_at(params, Instant::now());
    }

    fn register_at(&self, params: TeeAdmissionParams, now: Instant) {
        let mut pending = lock(&self.pending);
        pending.retain(|_, entry| entry.expires > now);
        let _previous = pending.insert(
            params.namespace_id,
            Pending {
                params: Arc::new(params),
                expires: now + PENDING_JOIN_TTL,
                last_attested: None,
            },
        );
    }

    /// What to answer a challenge for `namespace` with, or `None` when this node
    /// is not waiting to be admitted there or made a quote a moment ago.
    pub(crate) fn take_for_attestation(
        &self,
        namespace: [u8; 32],
    ) -> Option<Arc<TeeAdmissionParams>> {
        self.take_at(namespace, Instant::now())
    }

    fn take_at(&self, namespace: [u8; 32], now: Instant) -> Option<Arc<TeeAdmissionParams>> {
        let mut pending = lock(&self.pending);
        let entry = pending.get_mut(&namespace)?;
        if entry.expires <= now {
            let _expired = pending.remove(&namespace);
            return None;
        }
        if entry
            .last_attested
            .is_some_and(|at| now.saturating_duration_since(at) < MIN_ATTEST_INTERVAL)
        {
            return None;
        }
        entry.last_attested = Some(now);
        Some(Arc::clone(&entry.params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: [u8; 32] = [0x11; 32];

    #[test]
    fn a_challenge_is_answered_once() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();
        let challenge = book.issue(NS, peer).expect("issued");

        assert!(book.consume(&challenge, NS, peer));
        assert!(
            !book.consume(&challenge, NS, peer),
            "a challenge is single-use"
        );
    }

    #[test]
    fn a_challenge_lapses() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();
        let now = Instant::now();
        let challenge = book.issue_at(NS, peer, now).expect("issued");

        let just_in_time = now + CHALLENGE_TTL - Duration::from_secs(1);
        assert!(book.consume_at(&challenge, NS, peer, just_in_time));

        let challenge = book
            .issue_at(NS, peer, now + Duration::from_secs(10))
            .expect("issued");
        let late = now + Duration::from_secs(10) + CHALLENGE_TTL;
        assert!(
            !book.consume_at(&challenge, NS, peer, late),
            "a challenge older than its window is refused"
        );
    }

    #[test]
    fn a_challenge_is_bound_to_its_peer_and_namespace_and_a_wrong_try_spends_it() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();

        let challenge = book.issue(NS, peer).expect("issued");
        assert!(
            !book.consume(&challenge, NS, PeerId::random()),
            "other peer"
        );
        assert!(
            !book.consume(&challenge, NS, peer),
            "the wrong try spent it, so the right peer cannot use it after"
        );

        let challenge = book.issue([0x22; 32], peer).expect("issued");
        assert!(!book.consume(&challenge, NS, peer), "other namespace");

        assert!(!book.consume(&[0x33; 32], NS, peer), "never issued");
    }

    #[test]
    fn a_peer_is_not_offered_a_new_challenge_on_every_announcement() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();
        let now = Instant::now();

        assert!(book.issue_at(NS, peer, now).is_some());
        assert!(
            book.issue_at(NS, peer, now + Duration::from_secs(1))
                .is_none(),
            "too soon"
        );
        assert!(
            book.issue_at([0x22; 32], peer, now).is_some(),
            "other namespace"
        );
        assert!(book
            .issue_at(NS, peer, now + MIN_REISSUE + Duration::from_secs(1))
            .is_some());
    }

    #[test]
    fn one_peer_cannot_fill_the_book() {
        let book = TeeChallenges::default();
        let greedy = PeerId::random();
        let now = Instant::now();
        let mut first = None;
        for step in 0..(MAX_PER_PEER as u64 + 3) {
            let at =
                now + MIN_REISSUE * u32::try_from(step).expect("small") + Duration::from_millis(1);
            let challenge = book.issue_at(NS, greedy, at).expect("issued");
            first.get_or_insert(challenge);
        }
        let at = now + MIN_REISSUE * 20;
        assert!(
            !book.consume_at(&first.expect("issued"), NS, greedy, at),
            "the oldest made way for newer ones"
        );
        assert_eq!(
            lock(&book.issued)
                .iter()
                .filter(|(_, e)| e.peer == greedy)
                .count(),
            MAX_PER_PEER
        );
        assert!(
            book.issue_at(NS, PeerId::random(), at).is_some(),
            "and others are not shut out"
        );
    }

    fn params(namespace: [u8; 32]) -> TeeAdmissionParams {
        TeeAdmissionParams {
            namespace_id: namespace,
            admitter_addrs: vec![],
            public_key: [0x42; 32].into(),
            account: calimero_context::test_support::credential(&[0x42; 32].into()),
            release_version: None,
            mock_tee: false,
        }
    }

    #[test]
    fn a_joiner_answers_only_for_the_namespace_it_is_waiting_on_and_not_forever() {
        let joins = PendingTeeJoins::default();
        let now = Instant::now();
        assert!(joins.take_at(NS, now).is_none(), "not waiting");

        joins.register_at(params(NS), now);
        assert!(
            joins.take_at([0x22; 32], now).is_none(),
            "another namespace"
        );
        assert!(joins.take_at(NS, now).is_some());
        assert!(
            joins
                .take_at(NS, now + Duration::from_millis(500))
                .is_none(),
            "not twice in a moment"
        );
        assert!(joins.take_at(NS, now + MIN_ATTEST_INTERVAL).is_some());
        assert!(
            joins.take_at(NS, now + PENDING_JOIN_TTL).is_none(),
            "it stops answering once it has not asked for a while"
        );
    }
}
