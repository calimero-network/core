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

/// Per-round-peer budgeting cap for the outer deadline. Used to
/// size `connect_deadline = retries × (delay + cap × open_timeout)`.
///
/// This is a worst-case **cap**, not an exact per-round peer count.
/// It under-counts the per-round cost on very-large meshes — the
/// per-peer deadline check inside the inner loop bails as a safety
/// net there. It over-counts on small/empty meshes — the deadline
/// simply doesn't fire because rounds are cheap (the `retries ×
/// retry_delay` floor is well under any reasonable caller timeout).
/// Either way the loop terminates inside `retries` rounds; the
/// deadline is the upper-bound safety net, not a precise schedule.
///
/// 4 covers the expected mesh size for namespace-join discovery
/// (typically 1–3 peers in the namespace topic mesh during cold
/// start). If we ever see meshes consistently above this, the
/// constant should grow — the per-peer deadline check keeps the
/// current value sound regardless.
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
    // Production wiring always passes `DEFAULT_MESH_RETRIES_UNINITIALIZED`
    // (a non-zero compile-time const). A zero here would yield a zero
    // deadline and an empty `1..=0` loop body — the function would
    // return Err with a confusing "deadline 0ms, elapsed 0ms"
    // message. Catch the degenerate input loudly in dev/test rather
    // than silently surface the bad error.
    debug_assert!(
        mesh_retries > 0,
        "mesh_retries must be > 0; got {mesh_retries}"
    );

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
        // In-place shuffle avoids the second `Vec` allocation that
        // `choose_multiple` would produce. Matches the pattern used
        // in `perform_interval_sync`.
        peers.shuffle(&mut rand::thread_rng());

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
    /// with the deadline+elapsed signature. We seed exactly the
    /// expected error count (retries × peers = 6) and assert
    /// `assert_all_consumed` so an early-exit regression — which
    /// would leave unconsumed entries — fails this test loudly.
    #[tokio::test(start_paused = true)]
    async fn all_peers_fail_every_round_returns_err() {
        let mock = MockSyncNetwork::default();
        let p1 = PeerId::random();
        let p2 = PeerId::random();
        // Sticky-last on mesh_peers means every round sees this pair.
        mock.push_mesh_peers(vec![p1, p2]);
        let (open_timeout, retries, retry_delay) = defaults();
        // Each round tries every peer (3 × 2 = 6 attempts) and the
        // deadline guard fires before any extra inner-loop attempt.
        let expected_open_calls = (retries as usize) * 2;
        for i in 0..expected_open_calls {
            mock.push_open_stream_err(format!("err-{i}"));
        }

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
        // The retry loop ran exactly to the end of its budget:
        // every queued `open_stream` was consumed (no early bail,
        // no extra round). Sticky-last leaves the single
        // `mesh_peers` entry, which is the expected steady state.
        mock.assert_all_consumed();
    }

    /// Peer hangs past `open_timeout` → `tokio::time::timeout` fires
    /// and the loop continues with the next peer. With all peers
    /// hanging, eventually the deadline is hit and Err is returned.
    /// Under `start_paused` the test completes in virtual-time
    /// microseconds despite simulating many seconds.
    #[tokio::test(start_paused = true)]
    async fn hanging_peers_are_interrupted_by_per_peer_timeout() {
        let mock = MockSyncNetwork::default();
        mock.push_mesh_peers(vec![PeerId::random(), PeerId::random()]);
        // Every peer hangs far longer than open_timeout; tokio's
        // timeout should fire each time and we move on.
        for i in 0..20 {
            mock.push_open_stream_hang(Duration::from_secs(10), format!("hang-{i}"));
        }

        let (open_timeout, retries, retry_delay) = defaults();
        let connect_deadline = retry_delay
            .saturating_add(open_timeout.saturating_mul(DEADLINE_MAX_PEERS_PER_ROUND))
            .saturating_mul(retries);
        let start = time::Instant::now();
        let result =
            open_namespace_join_stream(&mock, NAMESPACE_ID, open_timeout, retries, retry_delay)
                .await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "expected Err from hanging peers, got Ok");
        // Tightly coupled to the deadline math so a future drift in
        // `DEADLINE_MAX_PEERS_PER_ROUND` or the formula doesn't let
        // this test silently widen. Bound: deadline + one extra
        // open_timeout slot (the per-peer check may bail mid-attempt
        // up to one timeout late).
        let upper_bound = connect_deadline.saturating_add(open_timeout);
        assert!(
            elapsed <= upper_bound,
            "loop took {elapsed:?}, expected ≤ {upper_bound:?} (deadline {connect_deadline:?} \
             + one open_timeout slot)"
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
