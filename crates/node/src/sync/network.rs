//! Narrow trait over the [`NetworkClient`] surface that the sync
//! subsystem actually uses.
//!
//! Sync only needs two methods (`mesh_peers`, `open_stream`); the real
//! `NetworkClient` has ~15. Defining a sync-specific trait here lets
//! tests substitute a mock without spinning up an Actix runtime + the
//! whole libp2p stack, and lets the post-extraction sync crate
//! (#2302) declare its network dependency in one place.
//!
//! The trait deliberately mirrors the existing `NetworkClient` method
//! signatures exactly so the blanket impl is one-line-per-method.
//!
//! **`open_stream` mockability**: `Stream` is a concrete type wrapping
//! a real `libp2p::Stream` and currently has no synthetic constructor.
//! Mocks can return `Err(_)` from `open_stream` (sufficient for
//! testing retry/timeout/error paths in the namespace-join discovery
//! loop, the snapshot/delta-request open paths, etc.) but cannot
//! return a synthetic `Ok(Stream)` until a `Stream::test_pair()`
//! constructor lands — tracked as a follow-up.
//!
//! See #2406 + #2302 (sync extraction epic).
//!
//! [`NetworkClient`]: calimero_network_primitives::client::NetworkClient

use async_trait::async_trait;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;

/// The network surface the sync subsystem uses.
///
/// The blanket impl on `NetworkClient` is the production wiring; the
/// `#[cfg(test)]` mock in [`mock`] is the unit-test wiring.
#[async_trait]
pub trait SyncNetwork: Send + Sync + 'static {
    /// Return the current gossipsub mesh peers for `topic`.
    ///
    /// Used by sync to discover sync peers and namespace-join targets.
    async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId>;

    /// Open a new substream to `peer_id` over the calimero protocol.
    ///
    /// Used by every sync initiator path. Mock impls can return
    /// `Err(_)` to exercise the retry/timeout/error branches.
    async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<Stream>;
}

#[async_trait]
impl SyncNetwork for NetworkClient {
    async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId> {
        NetworkClient::mesh_peers(self, topic).await
    }

    async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<Stream> {
        NetworkClient::open_stream(self, peer_id).await
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! Scriptable mock for [`SyncNetwork`].
    //!
    //! Scope is "failure-path tests" — `mesh_peers` returns recorded
    //! peer lists; `open_stream` returns recorded errors (or a queued
    //! callback returning `eyre::Error`). Success-path tests that
    //! actually exchange messages need a real `Stream`, which is
    //! tracked as a separate follow-up (see module doc).

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

    /// Mock impl. `mesh_peers_responses` is a per-call FIFO queue;
    /// once exhausted it returns the last response repeatedly (or
    /// empty if never seeded). Same shape for `open_stream_responses`,
    /// except it surfaces `eyre::Error` on every call once exhausted.
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
        pub fn push_open_stream_hang(
            &self,
            sleep_for: Duration,
            then_msg: impl Into<String>,
        ) -> &Self {
            self.open_stream_responses
                .lock()
                .push_back(OpenStreamResponse::SleepThenErr(sleep_for, then_msg.into()));
            self
        }
    }

    #[async_trait]
    impl SyncNetwork for MockSyncNetwork {
        async fn mesh_peers(&self, _topic: TopicHash) -> Vec<PeerId> {
            let mut queue = self.mesh_peers_responses.lock();
            // Pop the front; if it's the last entry, clone instead of
            // popping so repeated reads after the script is exhausted
            // keep returning the final value (matches "mesh is stable
            // after discovery completes" production behaviour).
            match queue.len() {
                0 => Vec::new(),
                1 => queue[0].clone(),
                _ => queue.pop_front().unwrap_or_default(),
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
    }
}
