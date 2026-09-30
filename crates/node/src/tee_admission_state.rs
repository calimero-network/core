//! State a TEE admission needs on each side of it.
//!
//! An admitting member issues a challenge and keeps it single-use for a short
//! window ([`TeeChallenges`]). A joiner remembers that it is waiting to be
//! admitted, so it answers a challenge offered to it and nothing else
//! ([`PendingTeeJoins`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

use calimero_node_primitives::client::TeeAdmissionParams;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;

/// How long an issued challenge can still be answered.
pub(crate) const CHALLENGE_TTL: Duration = Duration::from_secs(60);

/// The fewest seconds between two challenges to one peer for one namespace, so
/// a joiner that re-prompts every couple of seconds is not offered a new one
/// each time.
const MIN_REISSUE: Duration = Duration::from_secs(5);

/// Outstanding challenges one peer may hold at once. A new one past this evicts
/// that peer's oldest.
const MAX_PER_PEER: usize = 4;

/// Outstanding challenges a dialer asked for, which proved possession of a key.
const MAX_REQUESTED: usize = 1024;

/// Outstanding challenges offered to a prompting peer. Any peer on the topic
/// can prompt, so these have a budget of their own and cannot crowd out the
/// requests above.
const MAX_OFFERED: usize = 256;

/// Offers in flight at once. Each holds a stream to a peer that may never answer.
const MAX_CONCURRENT_OFFERS: usize = 16;

/// How long a joiner keeps answering offers after it last asked to be admitted.
/// `fleet-join` asks again on every call, so this only has to outlast one.
pub(crate) const PENDING_JOIN_TTL: Duration = Duration::from_secs(120);

/// The fewest seconds between two quotes a joiner makes for one namespace.
/// Generating one is not free, and any peer can offer a challenge.
const MIN_ATTEST_INTERVAL: Duration = Duration::from_secs(2);

/// The fewest seconds between two quotes a joiner makes for one offering peer,
/// so a single peer cannot keep the namespace's slot to itself.
const MIN_ATTEST_INTERVAL_PER_PEER: Duration = Duration::from_secs(10);

/// Offering peers a joiner remembers. The oldest is forgotten past this.
const MAX_TRACKED_OFFERERS: usize = 64;

#[derive(Debug)]
struct Issued {
    namespace: [u8; 32],
    peer: PeerId,
    /// The key the requester proved possession of, when it asked for the
    /// challenge itself. `None` for one offered to a prompting peer.
    identity: Option<PublicKey>,
    at: Instant,
}

impl Issued {
    const fn offered(&self) -> bool {
        self.identity.is_none()
    }
}

/// Why no challenge was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IssueRefusal {
    /// The peer was issued one for this namespace a moment ago.
    TooSoon,
}

/// The challenges this node has issued and not yet seen answered.
///
/// A challenge is 32 random bytes the admitting member chose. It is bound to the
/// namespace and to the peer it was issued to (and, when the peer asked for it,
/// to the key it proved), it is consumed the first time it is presented whatever
/// came of it, and it lapses after [`CHALLENGE_TTL`].
#[derive(Clone, Debug)]
pub(crate) struct TeeChallenges {
    issued: Arc<Mutex<HashMap<[u8; 32], Issued>>>,
    offers: Arc<Semaphore>,
}

impl Default for TeeChallenges {
    fn default() -> Self {
        Self {
            issued: Arc::default(),
            offers: Arc::new(Semaphore::new(MAX_CONCURRENT_OFFERS)),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock only means a panic elsewhere; the map is still a map.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TeeChallenges {
    /// Issue a challenge to `peer` for `namespace`, which `peer` asked for by
    /// proving possession of `identity`.
    pub(crate) fn issue_requested(
        &self,
        namespace: [u8; 32],
        peer: PeerId,
        identity: PublicKey,
    ) -> Result<[u8; 32], IssueRefusal> {
        self.issue_at(namespace, peer, Some(identity), Instant::now())
    }

    /// Issue a challenge to `peer`, which prompted for one, for `namespace`.
    pub(crate) fn issue_offered(
        &self,
        namespace: [u8; 32],
        peer: PeerId,
    ) -> Result<[u8; 32], IssueRefusal> {
        self.issue_at(namespace, peer, None, Instant::now())
    }

    /// Take one of the few slots for offering a challenge to a prompting peer.
    /// The slot is held until the offer is over.
    pub(crate) fn begin_offer(&self) -> Option<OwnedSemaphorePermit> {
        match Arc::clone(&self.offers).try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(TryAcquireError::NoPermits | TryAcquireError::Closed) => None,
        }
    }

    fn issue_at(
        &self,
        namespace: [u8; 32],
        peer: PeerId,
        identity: Option<PublicKey>,
        now: Instant,
    ) -> Result<[u8; 32], IssueRefusal> {
        let offered = identity.is_none();
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
            return Err(IssueRefusal::TooSoon);
        }
        if theirs.len() >= MAX_PER_PEER {
            theirs.sort_by_key(|(_, at, _)| *at);
            if let Some((oldest, _, _)) = theirs.first() {
                let _evicted = issued.remove(oldest);
            }
        }

        // A full book makes room from the peer holding the most of its kind
        // rather than turning a new peer away, so a flood of Sybil peers costs
        // the flood its own entries.
        let cap = if offered { MAX_OFFERED } else { MAX_REQUESTED };
        if issued.values().filter(|e| e.offered() == offered).count() >= cap {
            let mut per_peer: HashMap<PeerId, usize> = HashMap::new();
            for entry in issued.values().filter(|e| e.offered() == offered) {
                *per_peer.entry(entry.peer).or_default() += 1;
            }
            let heaviest = per_peer
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(peer, _)| peer);
            let oldest = issued
                .iter()
                .filter(|(_, e)| e.offered() == offered && Some(e.peer) == heaviest)
                .min_by_key(|(_, e)| e.at)
                .map(|(challenge, _)| *challenge);
            if let Some(oldest) = oldest {
                let _evicted = issued.remove(&oldest);
            }
        }

        let challenge: [u8; 32] = rand::random();
        let _previous = issued.insert(
            challenge,
            Issued {
                namespace,
                peer,
                identity,
                at: now,
            },
        );
        Ok(challenge)
    }

    /// Spend `challenge`: true only if it was issued to `peer` for `namespace`
    /// (and, if `peer` asked for it, to `identity`), has not been presented
    /// before and has not lapsed.
    ///
    /// Spent on the first presentation whatever its outcome, so a wrong guess
    /// cannot be retried against the same challenge.
    pub(crate) fn consume(
        &self,
        challenge: &[u8; 32],
        namespace: [u8; 32],
        peer: PeerId,
        identity: &PublicKey,
    ) -> bool {
        self.consume_at(challenge, namespace, peer, identity, Instant::now())
    }

    fn consume_at(
        &self,
        challenge: &[u8; 32],
        namespace: [u8; 32],
        peer: PeerId,
        identity: &PublicKey,
        now: Instant,
    ) -> bool {
        let Some(entry) = lock(&self.issued).remove(challenge) else {
            return false;
        };
        entry.namespace == namespace
            && entry.peer == peer
            && entry
                .identity
                .as_ref()
                .is_none_or(|issued_to| issued_to == identity)
            && now.saturating_duration_since(entry.at) < CHALLENGE_TTL
    }
}

#[derive(Debug)]
struct Pending {
    params: Arc<TeeAdmissionParams>,
    expires: Instant,
    last_attested: Option<Instant>,
    last_by_peer: HashMap<PeerId, Instant>,
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
                last_by_peer: HashMap::new(),
            },
        );
    }

    /// What to answer a challenge that `peer` offered for `namespace` with, or
    /// `None` when this node is not waiting to be admitted there, made a quote
    /// a moment ago, or made one for this peer a little while ago.
    pub(crate) fn take_for_attestation(
        &self,
        namespace: [u8; 32],
        peer: PeerId,
    ) -> Option<Arc<TeeAdmissionParams>> {
        self.take_at(namespace, peer, Instant::now())
    }

    fn take_at(
        &self,
        namespace: [u8; 32],
        peer: PeerId,
        now: Instant,
    ) -> Option<Arc<TeeAdmissionParams>> {
        let mut pending = lock(&self.pending);
        let entry = pending.get_mut(&namespace)?;
        if entry.expires <= now {
            let _expired = pending.remove(&namespace);
            return None;
        }
        // The peer's turn is checked first, so one peer offering every second
        // cannot keep the namespace's slot to itself.
        entry
            .last_by_peer
            .retain(|_, at| now.saturating_duration_since(*at) < MIN_ATTEST_INTERVAL_PER_PEER);
        if entry.last_by_peer.contains_key(&peer) {
            return None;
        }
        if entry
            .last_attested
            .is_some_and(|at| now.saturating_duration_since(at) < MIN_ATTEST_INTERVAL)
        {
            return None;
        }
        if entry.last_by_peer.len() >= MAX_TRACKED_OFFERERS {
            let oldest = entry
                .last_by_peer
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(peer, _)| *peer);
            if let Some(oldest) = oldest {
                let _forgotten = entry.last_by_peer.remove(&oldest);
            }
        }
        entry.last_attested = Some(now);
        let _previous = entry.last_by_peer.insert(peer, now);
        Some(Arc::clone(&entry.params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: [u8; 32] = [0x11; 32];

    fn key(byte: u8) -> PublicKey {
        [byte; 32].into()
    }

    fn requested(book: &TeeChallenges, peer: PeerId) -> [u8; 32] {
        book.issue_requested(NS, peer, key(1)).expect("issued")
    }

    #[test]
    fn a_challenge_is_answered_once() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();
        let challenge = requested(&book, peer);

        assert!(book.consume(&challenge, NS, peer, &key(1)));
        assert!(
            !book.consume(&challenge, NS, peer, &key(1)),
            "a challenge is single-use"
        );
    }

    #[test]
    fn a_challenge_lapses() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();
        let now = Instant::now();
        let challenge = book.issue_at(NS, peer, Some(key(1)), now).expect("issued");

        let just_in_time = now + CHALLENGE_TTL - Duration::from_secs(1);
        assert!(book.consume_at(&challenge, NS, peer, &key(1), just_in_time));

        let later = now + Duration::from_secs(10);
        let challenge = book
            .issue_at(NS, peer, Some(key(1)), later)
            .expect("issued");
        assert!(
            !book.consume_at(&challenge, NS, peer, &key(1), later + CHALLENGE_TTL),
            "a challenge older than its window is refused"
        );
    }

    #[test]
    fn a_challenge_is_bound_to_its_peer_namespace_and_key_and_a_wrong_try_spends_it() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();

        let challenge = requested(&book, peer);
        assert!(
            !book.consume(&challenge, NS, PeerId::random(), &key(1)),
            "other peer"
        );
        assert!(
            !book.consume(&challenge, NS, peer, &key(1)),
            "the wrong try spent it, so the right peer cannot use it after"
        );

        let challenge = book
            .issue_requested([0x22; 32], peer, key(1))
            .expect("issued");
        assert!(
            !book.consume(&challenge, NS, peer, &key(1)),
            "other namespace"
        );

        let asked_for = requested(&book, peer);
        assert!(
            !book.consume(&asked_for, NS, peer, &key(2)),
            "a challenge asked for by one key is not answered for another"
        );

        assert!(
            !book.consume(&[0x33; 32], NS, peer, &key(1)),
            "never issued"
        );
    }

    #[test]
    fn an_offered_challenge_is_not_tied_to_a_key() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();
        let challenge = book.issue_offered(NS, peer).expect("issued");
        assert!(book.consume(&challenge, NS, peer, &key(9)));
    }

    #[test]
    fn a_peer_is_not_offered_a_new_challenge_on_every_prompt() {
        let book = TeeChallenges::default();
        let peer = PeerId::random();
        let now = Instant::now();

        assert!(book.issue_at(NS, peer, None, now).is_ok());
        assert_eq!(
            book.issue_at(NS, peer, None, now + Duration::from_secs(1)),
            Err(IssueRefusal::TooSoon)
        );
        assert!(
            book.issue_at([0x22; 32], peer, None, now).is_ok(),
            "other namespace"
        );
        assert!(book
            .issue_at(NS, peer, None, now + MIN_REISSUE + Duration::from_secs(1))
            .is_ok());
    }

    #[test]
    fn one_peer_cannot_fill_the_book() {
        let book = TeeChallenges::default();
        let greedy = PeerId::random();
        let now = Instant::now();
        let mut first = None;
        for step in 0..(MAX_PER_PEER as u32 + 3) {
            let at = now + MIN_REISSUE * step + Duration::from_millis(1);
            let challenge = book.issue_at(NS, greedy, Some(key(1)), at).expect("issued");
            first.get_or_insert(challenge);
        }
        let at = now + MIN_REISSUE * 20;
        assert!(
            !book.consume_at(&first.expect("issued"), NS, greedy, &key(1), at),
            "the oldest made way for newer ones"
        );
        assert_eq!(
            lock(&book.issued)
                .values()
                .filter(|e| e.peer == greedy)
                .count(),
            MAX_PER_PEER
        );
        assert!(book.issue_at(NS, PeerId::random(), None, at).is_ok());
    }

    #[test]
    fn a_flood_of_prompting_peers_does_not_shut_out_a_new_one_or_a_requester() {
        let book = TeeChallenges::default();
        let now = Instant::now();
        for _ in 0..(MAX_OFFERED * 2) {
            book.issue_at(NS, PeerId::random(), None, now)
                .expect("issued");
        }
        let offered =
            |book: &TeeChallenges| lock(&book.issued).values().filter(|e| e.offered()).count();
        assert_eq!(
            offered(&book),
            MAX_OFFERED,
            "the flood is held to its budget"
        );

        let newcomer = PeerId::random();
        let challenge = book.issue_at(NS, newcomer, None, now).expect("issued");
        assert!(book.consume_at(&challenge, NS, newcomer, &key(1), now));

        let before = offered(&book);
        let requester = PeerId::random();
        let challenge = book
            .issue_at(NS, requester, Some(key(1)), now)
            .expect("issued");
        assert_eq!(
            offered(&book),
            before,
            "requests are not charged to the offers"
        );
        assert!(book.consume_at(&challenge, NS, requester, &key(1), now));
    }

    #[test]
    fn offers_in_flight_are_bounded() {
        let book = TeeChallenges::default();
        let held: Vec<_> = (0..MAX_CONCURRENT_OFFERS)
            .map(|_| book.begin_offer().expect("a slot"))
            .collect();
        assert!(book.begin_offer().is_none());
        drop(held);
        assert!(book.begin_offer().is_some());
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
        let peer = PeerId::random();
        let now = Instant::now();
        assert!(joins.take_at(NS, peer, now).is_none(), "not waiting");

        joins.register_at(params(NS), now);
        assert!(
            joins.take_at([0x22; 32], peer, now).is_none(),
            "another namespace"
        );
        assert!(joins.take_at(NS, peer, now).is_some());
        assert!(
            joins
                .take_at(NS, PeerId::random(), now + Duration::from_millis(500))
                .is_none(),
            "not twice in a moment"
        );
        assert!(joins
            .take_at(NS, PeerId::random(), now + MIN_ATTEST_INTERVAL)
            .is_some());
        assert!(
            joins
                .take_at(NS, PeerId::random(), now + PENDING_JOIN_TTL)
                .is_none(),
            "it stops answering once it has not asked for a while"
        );
    }

    #[test]
    fn one_offering_peer_cannot_keep_the_joiners_slot() {
        let joins = PendingTeeJoins::default();
        let pest = PeerId::random();
        let member = PeerId::random();
        let now = Instant::now();
        joins.register_at(params(NS), now);

        assert!(joins.take_at(NS, pest, now).is_some());
        let soon = now + MIN_ATTEST_INTERVAL;
        assert!(
            joins.take_at(NS, pest, soon).is_none(),
            "the same peer waits its own interval"
        );
        assert!(
            joins.take_at(NS, member, soon).is_some(),
            "and the namespace's slot is free for someone else"
        );
        assert!(joins
            .take_at(
                NS,
                pest,
                now + MIN_ATTEST_INTERVAL_PER_PEER + MIN_ATTEST_INTERVAL
            )
            .is_some());
    }
}
