//! Scriptable mock for [`super::SyncNetwork`].
//!
//! `subscribed_peers` returns recorded peer lists; `open_stream`
//! returns recorded errors, hangs, or — via `Stream::test_pair()`
//! behind the `test-utils` feature — a real `Ok(Stream)`. The success
//! arm lets tests cover the discovery loop's recovery path (a peer
//! succeeds after earlier ones fail), not just its give-up paths.
//!
//! ## Per-topic vs shared queues
//!
//! `subscribed_peers` is scripted two ways:
//!
//! - **Per-topic** ([`MockSyncNetwork::push_subscribed_peers_for`]):
//!   responses are keyed by `TopicHash`, so a test can say "context
//!   topic returns empty, namespace topic returns peers" — the two
//!   draw from independent queues. This is what the discovery code's
//!   namespace-fallback path needs, since it queries two distinct
//!   topics and the outcome depends on which one yields peers.
//! - **Shared** ([`MockSyncNetwork::push_subscribed_peers`]): a
//!   single topic-agnostic queue, used by tests that don't care which
//!   topic was requested. When a topic has no per-topic queue seeded,
//!   `subscribed_peers` falls through to this shared queue, so the
//!   simpler callers keep working unchanged.
//!
//! Per-topic queues take precedence; the shared queue is the
//! fallthrough.
//!
//! ## Exhaustion semantics
//!
//! The two methods behave **asymmetrically** when their queue is
//! drained. This is intentional and matches production:
//!
//! - `subscribed_peers` — **sticky last** (per queue, shared or
//!   per-topic): if N entries are queued and the method draws from
//!   that queue >N times, every draw past N returns the Nth value.
//!   Matches production "subscriber set stable after discovery
//!   completes" behaviour — a sync loop polling `subscribed_peers`
//!   while waiting for a peer to come up sees the same answer until
//!   the set actually changes, so the mock shouldn't suddenly start
//!   returning empty when the test forgets to script another tick.
//! - `open_stream` — **error on exhaust**: every call past the
//!   scripted count returns `Err("…no queued response")`. Each
//!   `open_stream` is a distinct attempt that must succeed or fail
//!   on its own merits; silently returning a stale Ok would mask
//!   test bugs.
//!
//! Tests that need to detect "called more times than expected"
//! against either method should use [`MockSyncNetwork::assert_all_consumed`]
//! after running the code under test.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use async_trait::async_trait;
use calimero_network_primitives::stream::Stream;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use parking_lot::Mutex;
use tokio::time;

use super::SyncNetwork;

/// Per-call directive for a mocked `open_stream` response.
pub(crate) enum OpenStreamResponse {
    /// Hand back a successfully-opened stream — an in-memory
    /// `Stream::test_pair()` end. Lets tests exercise the
    /// "peer succeeds after earlier failures" recovery path.
    Ok(Stream),
    /// Synthesise an error string.
    Err(String),
    /// Sleep for `Duration` then return `Err` — useful for
    /// exercising `tokio::time::timeout` wrapping the call.
    SleepThenErr(Duration, String),
}

/// See module doc for exhaustion semantics.
///
/// **Mutex choice**: `parking_lot::Mutex` (not `std::sync::Mutex`)
/// — never poisons on panic, so a failing test doesn't cascade
/// into "PoisonError" noise on subsequent tests. Held only for
/// the synchronous pop in each method; never across `.await`.
#[derive(Default)]
pub(crate) struct MockSyncNetwork {
    /// Shared, topic-agnostic queue. Used when no per-topic queue is
    /// seeded for the requested topic (see `subscribed_peers_by_topic`).
    subscribed_peers_responses: Mutex<VecDeque<Vec<PeerId>>>,
    /// Per-topic queues. When a queue exists for the requested topic
    /// it takes precedence over the shared queue, letting a test
    /// script distinct responses per topic (e.g. context-topic empty,
    /// namespace-topic populated) without relying on the global
    /// ordering of cross-topic calls.
    subscribed_peers_by_topic: Mutex<HashMap<TopicHash, VecDeque<Vec<PeerId>>>>,
    open_stream_responses: Mutex<VecDeque<OpenStreamResponse>>,
    /// `subscribed_peers` call count (across all topics) — needed to
    /// distinguish "queued 1 sticky-last entry and the code under test
    /// called subscribed_peers ≥1 times" (consumed) from "queued 1
    /// entry and the code under test never called it at all"
    /// (silently-unused shared queue).
    ///
    /// No analogous counter exists for `open_stream`: it has no
    /// sticky-last semantic, so "seeded but never called" already
    /// fails `assert_all_consumed`'s `open_stream_remaining > 0`
    /// check without needing a separate guard.
    subscribed_peers_calls: Mutex<u32>,
    /// Per-topic call counts, for the same "seeded but never called"
    /// guard applied to each per-topic queue independently — the
    /// global `subscribed_peers_calls` can't tell whether a *specific*
    /// topic's queue was ever read.
    subscribed_peers_calls_by_topic: Mutex<HashMap<TopicHash, u32>>,
}

impl MockSyncNetwork {
    /// Queue a response on the shared, topic-agnostic queue. Served to
    /// any topic that has no per-topic queue seeded. Use this for tests
    /// that don't distinguish topics.
    pub(crate) fn push_subscribed_peers(&self, peers: Vec<PeerId>) -> &Self {
        self.subscribed_peers_responses.lock().push_back(peers);
        self
    }

    /// Queue a response on the per-topic queue for `topic`. Calls to
    /// `subscribed_peers(topic)` draw from this queue (sticky-last)
    /// instead of the shared one, so a test can script context- and
    /// namespace-topic responses independently.
    pub(crate) fn push_subscribed_peers_for(&self, topic: TopicHash, peers: Vec<PeerId>) -> &Self {
        self.subscribed_peers_by_topic
            .lock()
            .entry(topic)
            .or_default()
            .push_back(peers);
        self
    }

    /// Queue a successful `open_stream` response.
    ///
    /// The returned `Stream` is one end of an in-memory
    /// `Stream::test_pair()`; the other end is dropped immediately.
    /// That's sufficient for the discovery loop, which only needs the
    /// open to *succeed* (it doesn't exchange messages here — the
    /// post-open protocol runs in the caller). Available because the
    /// node enables `calimero-network-primitives/test-utils` on its
    /// dev-dependency edge.
    pub(crate) fn push_open_stream_ok(&self) -> &Self {
        let (stream, _peer_end) = Stream::test_pair();
        self.open_stream_responses
            .lock()
            .push_back(OpenStreamResponse::Ok(stream));
        self
    }

    /// Queue a response for the next `open_stream` call.
    pub(crate) fn push_open_stream_err(&self, msg: impl Into<String>) -> &Self {
        self.open_stream_responses
            .lock()
            .push_back(OpenStreamResponse::Err(msg.into()));
        self
    }

    /// Queue a response that sleeps then errors. The caller's
    /// `tokio::time::timeout` wrapper should fire before the
    /// sleep completes if `sleep_for > timeout`.
    pub(crate) fn push_open_stream_hang(
        &self,
        sleep_for: Duration,
        then_msg: impl Into<String>,
    ) -> &Self {
        self.open_stream_responses
            .lock()
            .push_back(OpenStreamResponse::SleepThenErr(sleep_for, then_msg.into()));
        self
    }

    /// Panic if a queued response wasn't consumed by the code
    /// under test, or if a non-empty queue was never read from.
    ///
    /// Checks:
    ///
    /// - `open_stream`: queue must be empty.
    /// - `subscribed_peers` (shared queue, and each per-topic queue
    ///   independently): the queue must have ≤1 entry left (the
    ///   sticky-last steady state), AND if any entries were queued the
    ///   code under test must have drawn from that queue at least once.
    ///   The latter catches "test queued a single sticky-last entry but
    ///   never exercised the discovery path at all" — without the
    ///   call-count guard the assertion would pass silently in that
    ///   case.
    ///
    /// `#[track_caller]` so panics point at the test's call
    /// site, not inside this helper.
    ///
    /// ```ignore
    /// mock.push_open_stream_err("first")
    ///     .push_open_stream_err("second")
    ///     .push_open_stream_err("third");
    /// // ... run code under test ...
    /// mock.assert_all_consumed();  // panics if code only called open_stream twice
    /// ```
    #[track_caller]
    pub(crate) fn assert_all_consumed(&self) {
        let shared_remaining = self.subscribed_peers_responses.lock().len();
        let shared_calls = *self.subscribed_peers_calls.lock();
        let open_stream_remaining = self.open_stream_responses.lock().len();

        if shared_remaining > 1 {
            panic!(
                "MockSyncNetwork: {shared_remaining} unconsumed `subscribed_peers` responses queued \
                 (sticky-last leaves 1 by design; >1 means the code under test made fewer calls \
                 than expected)",
            );
        }
        // Sticky-last guard: if anything was queued on the shared
        // queue and the code under test never called subscribed_peers
        // (against any topic that fell through to it), the queue still
        // has 1 entry but `shared_calls == 0` — flag that, otherwise
        // the unconsumed-1-entry case is indistinguishable from the
        // healthy steady-state read. A per-topic call still bumps the
        // global counter, so this is a lower-bound guard (it can't
        // mis-fire), and the per-topic loop below catches the more
        // precise "this topic's queue was never read".
        if shared_remaining > 0 && shared_calls == 0 {
            panic!(
                "MockSyncNetwork: `subscribed_peers` shared queue was seeded with {shared_remaining} \
                 entries but the code under test never called it",
            );
        }
        // Same two checks, applied to each per-topic queue. The
        // per-topic call map lets us tell "this topic was never
        // queried" apart from "queried, drew its single sticky entry".
        let by_topic = self.subscribed_peers_by_topic.lock();
        let calls_by_topic = self.subscribed_peers_calls_by_topic.lock();
        for (topic, queue) in by_topic.iter() {
            let remaining = queue.len();
            if remaining > 1 {
                panic!(
                    "MockSyncNetwork: {remaining} unconsumed `subscribed_peers` responses queued \
                     for topic {topic:?} (sticky-last leaves 1 by design; >1 means the code under \
                     test made fewer calls than expected)",
                );
            }
            if remaining > 0 && calls_by_topic.get(topic).copied().unwrap_or(0) == 0 {
                panic!(
                    "MockSyncNetwork: `subscribed_peers` was seeded with {remaining} entries for \
                     topic {topic:?} but the code under test never queried it",
                );
            }
        }
        if open_stream_remaining > 0 {
            panic!(
                "MockSyncNetwork: {} unconsumed `open_stream` responses queued",
                open_stream_remaining
            );
        }
    }
}

#[async_trait]
impl SyncNetwork for MockSyncNetwork {
    async fn subscribed_peers(&self, topic: TopicHash) -> Vec<PeerId> {
        *self.subscribed_peers_calls.lock() += 1;

        // Per-topic queue takes precedence: if this topic was seeded
        // via `push_subscribed_peers_for`, draw from its own queue
        // (sticky-last). Otherwise fall through to the shared queue so
        // topic-agnostic callers keep working. The whole match runs
        // under one lock with no `.await` in scope, so cloning the
        // sticky entry inline is fine.
        let served = {
            let mut by_topic = self.subscribed_peers_by_topic.lock();
            by_topic.get_mut(&topic).map(|queue| match queue.len() {
                0 => Vec::new(),
                // Last entry: clone (don't pop) so repeated reads keep
                // returning the final value — matches "subscriber set
                // is stable after discovery completes".
                1 => queue.front().cloned().unwrap_or_default(),
                _ => queue.pop_front().unwrap_or_default(),
            })
        };
        if let Some(peers) = served {
            *self
                .subscribed_peers_calls_by_topic
                .lock()
                .entry(topic)
                .or_insert(0) += 1;
            return peers;
        }

        // Shared, topic-agnostic queue (fallthrough). Pull out a
        // borrow indicator under the lock and do the (possibly-cloning)
        // work after dropping it. Keeps the critical section minimal
        // and bounded to synchronous operations.
        enum Take {
            Empty,
            Stick,
            Pop(Vec<PeerId>),
        }
        let take = {
            let mut queue = self.subscribed_peers_responses.lock();
            match queue.len() {
                0 => Take::Empty,
                // Last entry: clone *after* dropping the lock so
                // repeated reads keep returning the final value
                // (matches "subscriber set is stable after discovery
                // completes" production behaviour).
                1 => Take::Stick,
                _ => Take::Pop(queue.pop_front().unwrap_or_default()),
            }
        };
        match take {
            Take::Empty => Vec::new(),
            Take::Stick => {
                // Re-lock briefly just to clone the last entry —
                // bounded work, no `.await` in scope.
                self.subscribed_peers_responses
                    .lock()
                    .front()
                    .cloned()
                    .unwrap_or_default()
            }
            Take::Pop(peers) => peers,
        }
    }

    async fn open_stream(&self, _peer_id: PeerId) -> eyre::Result<Stream> {
        let response = self.open_stream_responses.lock().pop_front();
        match response {
            None => Err(eyre::eyre!(
                "MockSyncNetwork: open_stream called with no queued response"
            )),
            Some(OpenStreamResponse::Ok(stream)) => Ok(stream),
            Some(OpenStreamResponse::Err(msg)) => Err(eyre::eyre!(msg)),
            Some(OpenStreamResponse::SleepThenErr(sleep_for, msg)) => {
                time::sleep(sleep_for).await;
                Err(eyre::eyre!(msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_queue_returns_queued_value_then_repeats_last() {
        let mock = MockSyncNetwork::default();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        mock.push_subscribed_peers(vec![peer_a])
            .push_subscribed_peers(vec![peer_b]);

        let topic = TopicHash::from_raw("test");
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_a]);
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_b]);
        // Exhausted: last value repeats.
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_b]);
        assert_eq!(mock.subscribed_peers(topic).await, vec![peer_b]);
    }

    #[tokio::test]
    async fn subscribed_peers_empty_when_never_seeded() {
        let mock = MockSyncNetwork::default();
        assert!(mock
            .subscribed_peers(TopicHash::from_raw("x"))
            .await
            .is_empty());
    }

    /// Explicit boundary test for the `match queue.len()` arms in the
    /// shared-queue path: seed N entries, call N+1 times, the (N+1)th
    /// should return the Nth value (the "sticky last" semantic).
    /// Catches off-by-one regressions in the pop-vs-clone branch.
    #[tokio::test]
    async fn shared_queue_sticky_last_at_len_1_boundary() {
        let mock = MockSyncNetwork::default();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        mock.push_subscribed_peers(vec![peer_a])
            .push_subscribed_peers(vec![peer_b]);

        let topic = TopicHash::from_raw("test");
        // Call 1: 2 entries queued → pop_front returns first.
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_a]);
        // Call 2: 1 entry left → clone (not pop) the last entry.
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_b]);
        // Call 3: still 1 entry left (cloning didn't pop) → same value again.
        assert_eq!(mock.subscribed_peers(topic).await, vec![peer_b]);
    }

    /// Per-topic queues are independent: a response seeded for one
    /// topic is never served to a different topic. This is the core
    /// capability that lets discovery tests script "context topic
    /// empty, namespace topic populated".
    #[tokio::test]
    async fn per_topic_queues_are_independent() {
        let mock = MockSyncNetwork::default();
        let ctx_peer = PeerId::random();
        let ns_peer = PeerId::random();
        let ctx_topic = TopicHash::from_raw("ctx");
        let ns_topic = TopicHash::from_raw("ns");

        mock.push_subscribed_peers_for(ctx_topic.clone(), vec![ctx_peer])
            .push_subscribed_peers_for(ns_topic.clone(), vec![ns_peer]);

        // Each topic draws only from its own queue, regardless of the
        // order the two topics are queried in.
        assert_eq!(mock.subscribed_peers(ns_topic.clone()).await, vec![ns_peer]);
        assert_eq!(
            mock.subscribed_peers(ctx_topic.clone()).await,
            vec![ctx_peer]
        );
        // Sticky-last applies per topic.
        assert_eq!(mock.subscribed_peers(ctx_topic).await, vec![ctx_peer]);
        assert_eq!(mock.subscribed_peers(ns_topic).await, vec![ns_peer]);
    }

    /// A topic with a per-topic queue draws from it; a topic without
    /// one falls through to the shared queue. The two coexist in a
    /// single mock.
    #[tokio::test]
    async fn per_topic_takes_precedence_then_shared_fallthrough() {
        let mock = MockSyncNetwork::default();
        let seeded = PeerId::random();
        let shared = PeerId::random();
        let seeded_topic = TopicHash::from_raw("seeded");

        mock.push_subscribed_peers_for(seeded_topic.clone(), vec![seeded])
            .push_subscribed_peers(vec![shared]);

        // Seeded topic → its own queue.
        assert_eq!(mock.subscribed_peers(seeded_topic).await, vec![seeded]);
        // Any other topic → shared queue.
        assert_eq!(
            mock.subscribed_peers(TopicHash::from_raw("other")).await,
            vec![shared]
        );
    }

    /// Per-topic sequencing: seed `[empty, populated]` for a topic and
    /// confirm the first read is empty, the second (sticky-last)
    /// populated — the shape the discovery retry→intersection path
    /// relies on for a single topic queried across phases.
    #[tokio::test]
    async fn per_topic_sticky_last_sequence() {
        let mock = MockSyncNetwork::default();
        let peer = PeerId::random();
        let topic = TopicHash::from_raw("ctx");

        mock.push_subscribed_peers_for(topic.clone(), vec![])
            .push_subscribed_peers_for(topic.clone(), vec![peer]);

        assert!(mock.subscribed_peers(topic.clone()).await.is_empty());
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer]);
        // Sticky-last: repeats the final populated entry.
        assert_eq!(mock.subscribed_peers(topic).await, vec![peer]);
    }

    #[tokio::test]
    async fn open_stream_returns_queued_errors_then_default_after_exhaustion() {
        let mock = MockSyncNetwork::default();
        mock.push_open_stream_err("first")
            .push_open_stream_err("second");

        let peer = PeerId::random();
        let e1 = mock.open_stream(peer).await.unwrap_err().to_string();
        assert_eq!(e1, "first");
        let e2 = mock.open_stream(peer).await.unwrap_err().to_string();
        assert_eq!(e2, "second");
        // Exhausted: synthetic message about the empty queue.
        let e3 = mock.open_stream(peer).await.unwrap_err().to_string();
        assert!(e3.contains("no queued response"), "got: {e3}");
    }

    #[tokio::test(start_paused = true)]
    async fn open_stream_hang_sleeps_then_errors() {
        let mock = MockSyncNetwork::default();
        mock.push_open_stream_hang(Duration::from_secs(5), "hung");

        let peer = PeerId::random();
        let start = tokio::time::Instant::now();
        let err = mock.open_stream(peer).await.unwrap_err();
        assert!(start.elapsed() >= Duration::from_secs(5));
        assert_eq!(err.to_string(), "hung");
    }

    #[tokio::test(start_paused = true)]
    async fn open_stream_hang_is_interruptible_by_timeout() {
        let mock = MockSyncNetwork::default();
        mock.push_open_stream_hang(Duration::from_secs(30), "hung");

        let peer = PeerId::random();
        let outer = time::timeout(Duration::from_millis(100), mock.open_stream(peer)).await;
        // `time::timeout` should fire before the 30s sleep
        // completes — exact assertion sync's retry loop uses.
        assert!(outer.is_err(), "expected timeout, got {outer:?}");
    }

    #[tokio::test]
    async fn assert_all_consumed_passes_when_all_used() {
        let mock = MockSyncNetwork::default();
        mock.push_open_stream_err("first")
            .push_open_stream_err("second");
        let peer = PeerId::random();
        let _ = mock.open_stream(peer).await;
        let _ = mock.open_stream(peer).await;
        mock.assert_all_consumed();
    }

    #[tokio::test]
    async fn assert_all_consumed_passes_with_sticky_last_shared_entry() {
        // Sticky-last semantic: leaving the last shared-queue entry
        // unconsumed is by design (steady-state subscriber set after
        // discovery), so the assertion accepts ≤1 remaining.
        let mock = MockSyncNetwork::default();
        let peer = PeerId::random();
        mock.push_subscribed_peers(vec![peer]);
        let _ = mock.subscribed_peers(TopicHash::from_raw("x")).await;
        mock.assert_all_consumed();
    }

    #[tokio::test]
    #[should_panic(expected = "unconsumed `open_stream` responses")]
    async fn assert_all_consumed_panics_on_unused_open_stream() {
        let mock = MockSyncNetwork::default();
        mock.push_open_stream_err("never-popped");
        mock.assert_all_consumed();
    }

    #[tokio::test]
    #[should_panic(expected = "unconsumed `subscribed_peers` responses")]
    async fn assert_all_consumed_panics_on_excess_shared_responses() {
        let mock = MockSyncNetwork::default();
        let p1 = PeerId::random();
        let p2 = PeerId::random();
        // 2 entries queued, none consumed → sticky-last allows 1, panics on >1.
        mock.push_subscribed_peers(vec![p1])
            .push_subscribed_peers(vec![p2]);
        mock.assert_all_consumed();
    }

    /// Catches the silent footgun the previous version had: a test
    /// that queues a single shared-queue entry but never exercises
    /// the discovery path would have passed `assert_all_consumed`
    /// without complaint (sticky-last accepts 1 leftover). With the
    /// call-count guard, "seeded but never called" panics loudly.
    #[tokio::test]
    #[should_panic(expected = "never called it")]
    async fn assert_all_consumed_panics_on_shared_seeded_but_never_called() {
        let mock = MockSyncNetwork::default();
        mock.push_subscribed_peers(vec![PeerId::random()]);
        mock.assert_all_consumed();
    }

    /// Per-topic leak detection mirrors the shared-queue checks: a
    /// topic queue with >1 unconsumed entry (code made fewer calls
    /// than scripted) panics.
    #[tokio::test]
    #[should_panic(expected = "fewer calls than expected")]
    async fn assert_all_consumed_panics_on_excess_per_topic_responses() {
        let mock = MockSyncNetwork::default();
        let topic = TopicHash::from_raw("ctx");
        mock.push_subscribed_peers_for(topic.clone(), vec![PeerId::random()])
            .push_subscribed_peers_for(topic, vec![PeerId::random()]);
        mock.assert_all_consumed();
    }

    /// A per-topic queue seeded but never queried panics — the global
    /// call counter can't catch this (another topic may have bumped
    /// it), so the per-topic call map is what flags it.
    #[tokio::test]
    #[should_panic(expected = "never queried it")]
    async fn assert_all_consumed_panics_on_per_topic_seeded_but_never_queried() {
        let mock = MockSyncNetwork::default();
        // Query a *different* topic (via the shared fallthrough) so the
        // global call counter is non-zero, proving the per-topic guard
        // is what fires.
        mock.push_subscribed_peers_for(TopicHash::from_raw("never"), vec![PeerId::random()]);
        let _ = mock.subscribed_peers(TopicHash::from_raw("other")).await;
        mock.assert_all_consumed();
    }

    /// Sticky-last leftover on a per-topic queue that *was* queried is
    /// accepted, same as the shared queue.
    #[tokio::test]
    async fn assert_all_consumed_passes_with_sticky_last_per_topic_entry() {
        let mock = MockSyncNetwork::default();
        let topic = TopicHash::from_raw("ctx");
        mock.push_subscribed_peers_for(topic.clone(), vec![PeerId::random()]);
        let _ = mock.subscribed_peers(topic).await;
        mock.assert_all_consumed();
    }
}
