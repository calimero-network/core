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
pub enum OpenStreamResponse {
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
pub struct MockSyncNetwork {
    mesh_peers_responses: Mutex<VecDeque<Vec<PeerId>>>,
    open_stream_responses: Mutex<VecDeque<OpenStreamResponse>>,
}

impl MockSyncNetwork {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a response for the next `mesh_peers` call.
    pub fn push_mesh_peers(&self, peers: Vec<PeerId>) -> &Self {
        self.mesh_peers_responses.lock().push_back(peers);
        self
    }

    /// Queue a response for the next `open_stream` call.
    pub fn push_open_stream_err(&self, msg: impl Into<String>) -> &Self {
        self.open_stream_responses
            .lock()
            .push_back(OpenStreamResponse::Err(msg.into()));
        self
    }

    /// Queue a response that sleeps then errors. The caller's
    /// `tokio::time::timeout` wrapper should fire before the
    /// sleep completes if `sleep_for > timeout`.
    pub fn push_open_stream_hang(&self, sleep_for: Duration, then_msg: impl Into<String>) -> &Self {
        self.open_stream_responses
            .lock()
            .push_back(OpenStreamResponse::SleepThenErr(sleep_for, then_msg.into()));
        self
    }

    /// Panic if any queued response wasn't consumed by the code
    /// under test.
    ///
    /// For `mesh_peers` the check is "at most 1 entry remains" —
    /// the sticky-last semantic means the last queued entry is
    /// expected to remain unconsumed (it's the steady-state
    /// answer). For `open_stream` the check is "queue is empty"
    /// — each scripted response is expected to be a distinct
    /// attempt.
    ///
    /// Call this at the end of a test that wants to detect the
    /// "queued more than the code under test consumed" failure mode,
    /// which would otherwise pass silently:
    ///
    /// ```ignore
    /// mock.push_open_stream_err("first")
    ///     .push_open_stream_err("second")
    ///     .push_open_stream_err("third");
    /// // ... run code under test ...
    /// mock.assert_all_consumed();  // panics if code only called open_stream twice
    /// ```
    #[track_caller]
    pub fn assert_all_consumed(&self) {
        let mesh_remaining = self.mesh_peers_responses.lock().len();
        let open_stream_remaining = self.open_stream_responses.lock().len();
        if mesh_remaining > 1 {
            panic!(
                "MockSyncNetwork: {} unconsumed `mesh_peers` responses queued (sticky-last \
                 leaves 1 by design; >1 means the code under test made fewer calls than expected)",
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
    async fn mesh_peers(&self, _topic: TopicHash) -> Vec<PeerId> {
        // Extract the value first, then drop the lock — keeps the
        // critical section synchronous-only even if a future
        // refactor adds an `.await` to this method's body.
        let peers = {
            let mut queue = self.mesh_peers_responses.lock();
            match queue.len() {
                0 => None,
                // Last entry: clone instead of popping so repeated
                // reads after the script is exhausted keep returning
                // the final value (matches "mesh is stable after
                // discovery completes" production behaviour).
                1 => Some(queue[0].clone()),
                _ => queue.pop_front(),
            }
        };
        peers.unwrap_or_default()
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
        let mock = MockSyncNetwork::new();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        mock.push_mesh_peers(vec![peer_a])
            .push_mesh_peers(vec![peer_b]);

        let topic = TopicHash::from_raw("test");
        assert_eq!(mock.mesh_peers(topic.clone()).await, vec![peer_a]);
        assert_eq!(mock.mesh_peers(topic.clone()).await, vec![peer_b]);
        // Exhausted: last value repeats.
        assert_eq!(mock.mesh_peers(topic.clone()).await, vec![peer_b]);
        assert_eq!(mock.mesh_peers(topic).await, vec![peer_b]);
    }

    #[tokio::test]
    async fn mesh_peers_empty_when_never_seeded() {
        let mock = MockSyncNetwork::new();
        assert!(mock.mesh_peers(TopicHash::from_raw("x")).await.is_empty());
    }

    /// Explicit boundary test for the `match queue.len()` arms in
    /// `mesh_peers`: seed N entries, call N+1 times, the (N+1)th
    /// should return the Nth value (the "sticky last" semantic).
    /// Catches off-by-one regressions in the pop-vs-clone branch.
    #[tokio::test]
    async fn mesh_peers_sticky_last_at_len_1_boundary() {
        let mock = MockSyncNetwork::new();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        mock.push_mesh_peers(vec![peer_a])
            .push_mesh_peers(vec![peer_b]);

        let topic = TopicHash::from_raw("test");
        // Call 1: 2 entries queued → pop_front returns first.
        assert_eq!(mock.mesh_peers(topic.clone()).await, vec![peer_a]);
        // Call 2: 1 entry left → clone (not pop) the last entry.
        assert_eq!(mock.mesh_peers(topic.clone()).await, vec![peer_b]);
        // Call 3: still 1 entry left (cloning didn't pop) → same value again.
        assert_eq!(mock.mesh_peers(topic).await, vec![peer_b]);
    }

    #[tokio::test]
    async fn open_stream_returns_queued_errors_then_default_after_exhaustion() {
        let mock = MockSyncNetwork::new();
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
        let mock = MockSyncNetwork::new();
        mock.push_open_stream_hang(Duration::from_secs(5), "hung");

        let peer = PeerId::random();
        let start = tokio::time::Instant::now();
        let err = mock.open_stream(peer).await.unwrap_err();
        assert!(start.elapsed() >= Duration::from_secs(5));
        assert_eq!(err.to_string(), "hung");
    }

    #[tokio::test(start_paused = true)]
    async fn open_stream_hang_is_interruptible_by_timeout() {
        let mock = MockSyncNetwork::new();
        mock.push_open_stream_hang(Duration::from_secs(30), "hung");

        let peer = PeerId::random();
        let outer = time::timeout(Duration::from_millis(100), mock.open_stream(peer)).await;
        // `time::timeout` should fire before the 30s sleep
        // completes — exact assertion sync's retry loop uses.
        assert!(outer.is_err(), "expected timeout, got {outer:?}");
    }

    #[tokio::test]
    async fn assert_all_consumed_passes_when_all_used() {
        let mock = MockSyncNetwork::new();
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
        let mock = MockSyncNetwork::new();
        let peer = PeerId::random();
        mock.push_mesh_peers(vec![peer]);
        let _ = mock.mesh_peers(TopicHash::from_raw("x")).await;
        mock.assert_all_consumed();
    }

    #[tokio::test]
    #[should_panic(expected = "unconsumed `open_stream` responses")]
    async fn assert_all_consumed_panics_on_unused_open_stream() {
        let mock = MockSyncNetwork::new();
        mock.push_open_stream_err("never-popped");
        mock.assert_all_consumed();
    }

    #[tokio::test]
    #[should_panic(expected = "unconsumed `mesh_peers` responses")]
    async fn assert_all_consumed_panics_on_excess_mesh_peers() {
        let mock = MockSyncNetwork::new();
        let p1 = PeerId::random();
        let p2 = PeerId::random();
        // 2 entries queued, none consumed → sticky-last allows 1, panics on >1.
        mock.push_mesh_peers(vec![p1]).push_mesh_peers(vec![p2]);
        mock.assert_all_consumed();
    }
}
