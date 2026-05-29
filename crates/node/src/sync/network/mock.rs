//! Scriptable mock for [`super::SyncNetwork`].
//!
//! Scope is "failure-path tests" — `mesh_peers` returns recorded
//! peer lists; `open_stream` returns recorded errors (or a queued
//! callback returning `eyre::Error`). Success-path tests that
//! actually exchange messages need a real `Stream`, which is
//! tracked as a separate follow-up (see `super` module doc).
//!
//! ## Exhaustion semantics
//!
//! The two methods behave **asymmetrically** when their queue is
//! drained. This is intentional and matches production:
//!
//! - `mesh_peers` — **sticky last**: if N entries are queued and the
//!   method is called >N times, every call past N returns the Nth
//!   value. Matches production "mesh stable after discovery
//!   completes" behaviour — a sync loop polling `mesh_peers` while
//!   waiting for a peer to come up sees the same answer until the
//!   mesh actually changes, so the mock shouldn't suddenly start
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

use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use parking_lot::Mutex;
use tokio::time;

use super::SyncNetwork;

/// Per-call directive for a mocked `open_stream` response.
pub(crate) enum OpenStreamResponse {
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
    mesh_peers_responses: Mutex<VecDeque<Vec<PeerId>>>,
    open_stream_responses: Mutex<VecDeque<OpenStreamResponse>>,
    /// `mesh_peers` call count — needed to distinguish "queued 1
    /// sticky-last entry and the code under test called mesh_peers
    /// ≥1 times" (consumed) from "queued 1 entry and the code under
    /// test never called mesh_peers at all" (silently-unused queue).
    ///
    /// No analogous counter exists for `open_stream`: it has no
    /// sticky-last semantic, so "seeded but never called" already
    /// fails `assert_all_consumed`'s `open_stream_remaining > 0`
    /// check without needing a separate guard.
    mesh_peers_calls: Mutex<u32>,
}

impl MockSyncNetwork {
    /// Queue a response for the next `mesh_peers` call.
    pub(crate) fn push_mesh_peers(&self, peers: Vec<PeerId>) -> &Self {
        self.mesh_peers_responses.lock().push_back(peers);
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
    /// - `mesh_peers`: queue must have ≤1 entry left (the
    ///   sticky-last steady state), AND if any entries were
    ///   queued the code under test must have called
    ///   `mesh_peers` at least once. The latter catches "test
    ///   queued a single sticky-last entry but never exercised
    ///   the discovery path at all" — without the call-count
    ///   guard the assertion would pass silently in that case.
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
        let mesh_remaining = self.mesh_peers_responses.lock().len();
        let mesh_calls = *self.mesh_peers_calls.lock();
        let open_stream_remaining = self.open_stream_responses.lock().len();

        if mesh_remaining > 1 {
            panic!(
                "MockSyncNetwork: {} unconsumed `mesh_peers` responses queued (sticky-last \
                 leaves 1 by design; >1 means the code under test made fewer calls than expected)",
                mesh_remaining
            );
        }
        // Sticky-last guard: if anything was queued and the code
        // under test never called mesh_peers, the queue still has
        // 1 entry but `mesh_calls == 0` — flag that, otherwise the
        // unconsumed-1-entry case is indistinguishable from the
        // healthy steady-state read.
        if mesh_remaining > 0 && mesh_calls == 0 {
            panic!(
                "MockSyncNetwork: `mesh_peers` was seeded with {} entries but the code under \
                 test never called it",
                mesh_remaining
            );
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
    // Trait method is `subscribed_peers` (the sync layer now selects from
    // the full subscriber set, not the grafted mesh); the mock's internal
    // queue keeps its historical `mesh_peers_*` field names — they're just
    // the response store and renaming them would churn unrelated tests.
    async fn subscribed_peers(&self, _topic: TopicHash) -> Vec<PeerId> {
        *self.mesh_peers_calls.lock() += 1;
        // Pull out a borrow indicator under the lock and do the
        // (possibly-cloning) work after dropping it. Keeps the
        // critical section minimal and bounded to synchronous
        // operations regardless of how the work below evolves.
        enum Take {
            Empty,
            Stick,
            Pop(Vec<PeerId>),
        }
        let take = {
            let mut queue = self.mesh_peers_responses.lock();
            match queue.len() {
                0 => Take::Empty,
                // Last entry: clone *after* dropping the lock so
                // repeated reads keep returning the final value
                // (matches "mesh is stable after discovery
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
                self.mesh_peers_responses
                    .lock()
                    .front()
                    .cloned()
                    .unwrap_or_default()
            }
            Take::Pop(peers) => peers,
        }
    }

    async fn open_stream(
        &self,
        _peer_id: PeerId,
    ) -> eyre::Result<calimero_network_primitives::stream::Stream> {
        let response = self.open_stream_responses.lock().pop_front();
        match response {
            None => Err(eyre::eyre!(
                "MockSyncNetwork: open_stream called with no queued response"
            )),
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
    async fn mesh_peers_returns_queued_value_then_repeats_last() {
        let mock = MockSyncNetwork::default();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        mock.push_mesh_peers(vec![peer_a])
            .push_mesh_peers(vec![peer_b]);

        let topic = TopicHash::from_raw("test");
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_a]);
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_b]);
        // Exhausted: last value repeats.
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_b]);
        assert_eq!(mock.subscribed_peers(topic).await, vec![peer_b]);
    }

    #[tokio::test]
    async fn mesh_peers_empty_when_never_seeded() {
        let mock = MockSyncNetwork::default();
        assert!(mock
            .subscribed_peers(TopicHash::from_raw("x"))
            .await
            .is_empty());
    }

    /// Explicit boundary test for the `match queue.len()` arms in
    /// `mesh_peers`: seed N entries, call N+1 times, the (N+1)th
    /// should return the Nth value (the "sticky last" semantic).
    /// Catches off-by-one regressions in the pop-vs-clone branch.
    #[tokio::test]
    async fn mesh_peers_sticky_last_at_len_1_boundary() {
        let mock = MockSyncNetwork::default();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        mock.push_mesh_peers(vec![peer_a])
            .push_mesh_peers(vec![peer_b]);

        let topic = TopicHash::from_raw("test");
        // Call 1: 2 entries queued → pop_front returns first.
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_a]);
        // Call 2: 1 entry left → clone (not pop) the last entry.
        assert_eq!(mock.subscribed_peers(topic.clone()).await, vec![peer_b]);
        // Call 3: still 1 entry left (cloning didn't pop) → same value again.
        assert_eq!(mock.subscribed_peers(topic).await, vec![peer_b]);
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
    async fn assert_all_consumed_passes_with_sticky_last_mesh_entry() {
        // Sticky-last semantic: leaving the last `mesh_peers` entry
        // unconsumed is by design (steady-state mesh after
        // discovery), so the assertion accepts ≤1 remaining.
        let mock = MockSyncNetwork::default();
        let peer = PeerId::random();
        mock.push_mesh_peers(vec![peer]);
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
    #[should_panic(expected = "unconsumed `mesh_peers` responses")]
    async fn assert_all_consumed_panics_on_excess_mesh_peers() {
        let mock = MockSyncNetwork::default();
        let p1 = PeerId::random();
        let p2 = PeerId::random();
        // 2 entries queued, none consumed → sticky-last allows 1, panics on >1.
        mock.push_mesh_peers(vec![p1]).push_mesh_peers(vec![p2]);
        mock.assert_all_consumed();
    }

    /// Catches the silent footgun the previous version had: a test
    /// that queues a single `mesh_peers` entry but never exercises
    /// the discovery path would have passed `assert_all_consumed`
    /// without complaint (sticky-last accepts 1 leftover). With the
    /// call-count guard, "seeded but never called" panics loudly.
    #[tokio::test]
    #[should_panic(expected = "never called it")]
    async fn assert_all_consumed_panics_on_mesh_peers_seeded_but_never_called() {
        let mock = MockSyncNetwork::default();
        mock.push_mesh_peers(vec![PeerId::random()]);
        mock.assert_all_consumed();
    }
}
