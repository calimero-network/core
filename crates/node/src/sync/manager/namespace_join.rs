//! Connect-loop helper for namespace-join discovery, extracted from
//! `SyncManager::initiate_namespace_join` so it can be unit-tested
//! against a [`MockSyncNetwork`](crate::sync::network::mock::MockSyncNetwork)
//! without standing up a full `SyncManager`.
//!
//! The helper owns only the parts of `initiate_namespace_join` that
//! depend on the network surface: shuffled-peer retry rounds bounded
//! by `open_stream_timeout` per peer and a worst-case outer deadline.
//! The post-open protocol exchange stays in the manager method.
//!
//! See the original call site in `manager/mod.rs` for the design
//! rationale (mesh-formation latency, stale-transport fallback, etc.);
//! this module deliberately holds none of it so the comments don't
//! drift out of sync.

use calimero_network_primitives::stream::Stream;
use libp2p::gossipsub::TopicHash;
use rand::seq::SliceRandom;
use tokio::time;
use tracing::debug;

use crate::sync::network::SyncNetwork;

/// Per-round-peer budgeting cap. See doc on the same const in
/// `manager/mod.rs::initiate_namespace_join`.
const DEADLINE_MAX_PEERS_PER_ROUND: u32 = 4;

/// Open a stream to a namespace mesh peer.
///
/// Iterates `mesh_retries` rounds. Each round: discover mesh peers,
/// shuffle, try each with a per-peer `open_timeout`. The whole loop
/// is bounded by an outer deadline computed from the retry/timeout
/// config so a pathological large-mesh case can't outlast the
/// caller's own timeout.
///
/// Returns `Ok(stream)` on first success or `Err(_)` after the
/// deadline elapses / all retries exhaust.
pub(super) async fn open_namespace_join_stream(
    sync_network: &dyn SyncNetwork,
    namespace_id: [u8; 32],
    open_timeout: std::time::Duration,
    mesh_retries: u32,
    mesh_retry_delay: std::time::Duration,
) -> eyre::Result<Stream> {
    let topic = TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));

    let connect_deadline = mesh_retry_delay
        .saturating_add(open_timeout.saturating_mul(DEADLINE_MAX_PEERS_PER_ROUND))
        .saturating_mul(mesh_retries);
    // `tokio::time::Instant` (not `std::time::Instant`) so the
    // deadline tracks virtual time under `tokio::time::pause()` —
    // tests use `start_paused = true` to fast-forward through the
    // retry loop. In production it behaves identically to
    // `std::time::Instant`.
    let connect_started = tokio::time::Instant::now();

    let mut stream = None;
    'connect: for attempt in 1..=mesh_retries {
        if connect_started.elapsed() >= connect_deadline {
            debug!(
                namespace_id = %hex::encode(namespace_id),
                attempt,
                elapsed_ms = connect_started.elapsed().as_millis() as u64,
                "namespace-join connect-loop deadline exceeded, giving up"
            );
            break;
        }
        let mut peers = sync_network.mesh_peers(topic.clone()).await;
        peers = peers
            .choose_multiple(&mut rand::thread_rng(), peers.len())
            .copied()
            .collect();

        for peer in &peers {
            if connect_started.elapsed() >= connect_deadline {
                break 'connect;
            }
            match time::timeout(open_timeout, sync_network.open_stream(*peer)).await {
                Ok(Ok(opened)) => {
                    stream = Some(opened);
                    break 'connect;
                }
                Ok(Err(err)) => {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        %peer,
                        attempt,
                        %err,
                        "Failed to open namespace-join stream, trying next peer..."
                    );
                }
                Err(_) => {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        %peer,
                        attempt,
                        "Timed out opening namespace-join stream, trying next peer..."
                    );
                }
            }
        }

        if attempt < mesh_retries
            && connect_started.elapsed().saturating_add(mesh_retry_delay) < connect_deadline
        {
            debug!(
                namespace_id = %hex::encode(namespace_id),
                attempt,
                peer_count = peers.len(),
                "No reachable namespace mesh peer yet, retrying..."
            );
            time::sleep(mesh_retry_delay).await;
        }
    }

    let elapsed = connect_started.elapsed();
    stream.ok_or_else(|| {
        eyre::eyre!(
            "could not open a namespace-join stream to any mesh peer for namespace {} \
             (deadline {}ms, elapsed {}ms)",
            hex::encode(namespace_id),
            connect_deadline.as_millis(),
            elapsed.as_millis()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use libp2p::PeerId;

    use super::*;
    use crate::sync::network::mock::MockSyncNetwork;

    const NAMESPACE_ID: [u8; 32] = [0xA1; 32];

    /// Tiny defaults so tests run fast under `start_paused = true`:
    /// the loop iterates the full retry budget when peers all fail,
    /// so individual values stay small.
    fn defaults() -> (Duration, u32, Duration) {
        // open_timeout, mesh_retries, mesh_retry_delay
        (Duration::from_millis(100), 3, Duration::from_millis(50))
    }

    /// All peers in every round return Err → function returns Err
    /// with the deadline+elapsed signature.
    #[tokio::test(start_paused = true)]
    async fn all_peers_fail_every_round_returns_err() {
        let mock = MockSyncNetwork::default();
        let p1 = PeerId::random();
        let p2 = PeerId::random();
        // Seed mesh peers; sticky-last means every round sees the
        // same pair.
        mock.push_mesh_peers(vec![p1, p2]);
        // Every open_stream attempt errors. With 3 retries × 2 peers
        // we expect up to 6 errors. Push enough.
        for i in 0..10 {
            mock.push_open_stream_err(format!("err-{i}"));
        }

        let (open_timeout, retries, retry_delay) = defaults();
        let result =
            open_namespace_join_stream(&mock, NAMESPACE_ID, open_timeout, retries, retry_delay)
                .await;

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("could not open a namespace-join stream"),
            "unexpected err: {err}"
        );
        assert!(
            err.contains("deadline"),
            "err should report deadline: {err}"
        );
        assert!(err.contains("elapsed"), "err should report elapsed: {err}");
    }

    /// Peer hangs past `open_timeout` → `tokio::time::timeout` fires
    /// and the loop continues with the next peer. With all peers
    /// hanging, eventually the deadline is hit and Err is returned.
    /// This validates the time-bounded behaviour: under
    /// `start_paused` the test completes in virtual-time microseconds
    /// even though the loop simulates many seconds.
    #[tokio::test(start_paused = true)]
    async fn hanging_peers_are_interrupted_by_per_peer_timeout() {
        let mock = MockSyncNetwork::default();
        mock.push_mesh_peers(vec![PeerId::random(), PeerId::random()]);
        // Every peer hangs for 10s — far longer than open_timeout
        // (100ms). time::timeout should fire and we move on.
        for i in 0..10 {
            mock.push_open_stream_hang(Duration::from_secs(10), format!("hang-{i}"));
        }

        let (open_timeout, retries, retry_delay) = defaults();
        let start = time::Instant::now();
        let result =
            open_namespace_join_stream(&mock, NAMESPACE_ID, open_timeout, retries, retry_delay)
                .await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "expected Err from hanging peers, got Ok");
        // Total virtual-elapsed time must be far less than what
        // would happen WITHOUT the per-peer timeout — i.e., not
        // 10s × peers × retries. Bound: deadline =
        // 3 × (50ms + 4 × 100ms) = 1350ms.
        assert!(
            elapsed < Duration::from_secs(3),
            "loop took {elapsed:?}, expected to be bounded by deadline ~1.35s"
        );
    }

    /// Empty mesh in every round → no peers ever tried → Err after
    /// `mesh_retries` rounds of the inter-round sleep.
    #[tokio::test(start_paused = true)]
    async fn empty_mesh_every_round_returns_err() {
        let mock = MockSyncNetwork::default();
        // No `push_mesh_peers` calls → mesh_peers returns Vec::new()
        // (the "never seeded" path; production-legitimate when the
        // mesh hasn't formed yet).

        let (open_timeout, retries, retry_delay) = defaults();
        let result =
            open_namespace_join_stream(&mock, NAMESPACE_ID, open_timeout, retries, retry_delay)
                .await;

        assert!(
            result.is_err(),
            "expected Err when mesh stays empty across all retries"
        );
    }

    /// Outer deadline fires mid-loop on a huge mesh: queue many
    /// hanging peers, set a tight deadline by making `open_timeout`
    /// large relative to total budget. Verifies the per-peer
    /// deadline-check inside the inner loop bails the whole connect
    /// loop without going round-robin through every peer past the
    /// budget.
    #[tokio::test(start_paused = true)]
    async fn outer_deadline_fires_inside_peer_loop_on_large_mesh() {
        let mock = MockSyncNetwork::default();
        // 10 peers — far more than DEADLINE_MAX_PEERS_PER_ROUND (4).
        let many_peers: Vec<PeerId> = (0..10).map(|_| PeerId::random()).collect();
        mock.push_mesh_peers(many_peers);
        // Every peer hangs for the full open_timeout — so the
        // per-peer cost lower-bound is open_timeout.
        for i in 0..50 {
            mock.push_open_stream_hang(Duration::from_secs(60), format!("h-{i}"));
        }

        let open_timeout = Duration::from_millis(200);
        let mesh_retries: u32 = 3;
        let mesh_retry_delay = Duration::from_millis(10);
        // deadline = 3 × (10ms + 4 × 200ms) = 2430ms. With 10 peers
        // × 200ms each, an unbounded round would take 2000ms — so
        // the per-peer-deadline check must bail somewhere inside
        // round 2 to keep total under ~2430ms.

        let start = time::Instant::now();
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            mesh_retries,
            mesh_retry_delay,
        )
        .await;
        let elapsed = start.elapsed();

        assert!(result.is_err());
        // Without the per-peer deadline check, the loop would run
        // through every peer in round 1 = 10 × 200ms = 2000ms,
        // then sleep 10ms, then maybe one more peer in round 2
        // before the top-of-loop check fires = ~2210ms. With the
        // per-peer check inside the loop, we should bail no later
        // than deadline + one per-peer slot ≈ 2430 + 200 = 2630ms.
        assert!(
            elapsed < Duration::from_secs(3),
            "loop took {elapsed:?}, expected outer deadline + per-peer guard to bound this"
        );
    }

    /// `Arc<dyn SyncNetwork>` interop: the helper takes
    /// `&dyn SyncNetwork`, but in production `SyncManager` stores
    /// the network as `Arc<dyn SyncNetwork>`. Verify that
    /// `&*arc_value` coerces cleanly.
    #[tokio::test(start_paused = true)]
    async fn accepts_arc_dyn_sync_network() {
        let mock: Arc<dyn SyncNetwork> = Arc::new(MockSyncNetwork::default());

        let (open_timeout, retries, retry_delay) = defaults();
        let result =
            open_namespace_join_stream(&*mock, NAMESPACE_ID, open_timeout, retries, retry_delay)
                .await;
        // Empty mesh → Err is expected; we're just checking the
        // type coercion compiles and runs.
        assert!(result.is_err());
    }
}
