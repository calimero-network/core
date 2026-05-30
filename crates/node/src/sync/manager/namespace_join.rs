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

use std::collections::HashSet;

use calimero_network_primitives::stream::Stream;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
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
/// shuffle, try each with a per-peer `open_timeout`. Peers in
/// `excluded_peers` are filtered out before the inner loop —
/// `initiate_namespace_join` uses this to retry against a different
/// peer after one returns `NamespaceJoinRejected` without opening
/// fresh transports to the rejecting peer. The whole loop is bounded
/// by an outer deadline computed from the retry/timeout config so a
/// pathological large-mesh case can't outlast the caller's own
/// timeout.
///
/// Returns `Ok((stream, peer_id))` on first success or `Err(_)` after
/// the deadline elapses / all retries exhaust. The `peer_id` lets the
/// caller record a rejection and pass the peer back via
/// `excluded_peers` on the next call.
pub(super) async fn open_namespace_join_stream(
    sync_network: &dyn SyncNetwork,
    namespace_id: [u8; 32],
    open_timeout: std::time::Duration,
    mesh_retries: u32,
    mesh_retry_delay: std::time::Duration,
    excluded_peers: &HashSet<PeerId>,
) -> eyre::Result<(Stream, PeerId)> {
    // Production wiring always passes `DEFAULT_MESH_RETRIES_UNINITIALIZED`
    // (a non-zero compile-time const). A zero here would yield a zero
    // deadline and an empty `1..=0` loop body — the function would
    // return Err with a confusing "deadline 0ms, elapsed 0ms"
    // message. Use a hard `assert!` (not `debug_assert!`) so this
    // catches the degenerate input in release builds too — the
    // per-call branch cost is negligible against the discovery
    // loop's latency.
    assert!(
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

    let mut result: Option<(Stream, PeerId)> = None;
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
        let mut peers = sync_network.subscribed_peers(topic.clone()).await;
        // Filter excluded peers before shuffling so an excluded peer
        // doesn't get picked first and then `continue`'d — that would
        // burn a slot in the shuffle order. Filtering up-front also
        // makes the empty-after-exclusion case observable: if every
        // mesh peer is excluded, we skip straight to the inter-round
        // sleep (or, if this is the last attempt, the Err).
        if !excluded_peers.is_empty() {
            peers.retain(|p| !excluded_peers.contains(p));
        }
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
                    result = Some((opened, *peer));
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
    result.ok_or_else(|| {
        eyre::eyre!(
            "could not open a namespace-join stream to any mesh peer for namespace {} \
             (deadline {}ms, elapsed {}ms, excluded {})",
            hex::encode(namespace_id),
            connect_deadline.as_millis(),
            elapsed.as_millis(),
            excluded_peers.len()
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

    /// Default-empty exclusion set for tests that don't need to
    /// exercise the protocol-level-retry rejection path.
    fn no_excluded() -> HashSet<PeerId> {
        HashSet::new()
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

        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            &no_excluded(),
        )
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
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            &no_excluded(),
        )
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
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            &no_excluded(),
        )
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
            &no_excluded(),
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
        let result = open_namespace_join_stream(
            &*mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            &no_excluded(),
        )
        .await;
        // Empty mesh → Err is expected; we're just checking the
        // type coercion compiles and runs.
        assert!(result.is_err());
    }

    /// Every mesh peer present is in `excluded_peers` → connect loop
    /// has nothing to try → Err after the full retry budget. Catches
    /// the protocol-level-retry exhaustion case where every peer has
    /// already rejected `NamespaceJoinRequest` on a prior attempt and
    /// the manager re-calls the helper with all of them excluded.
    #[tokio::test(start_paused = true)]
    async fn all_peers_excluded_returns_err_without_open_attempts() {
        let mock = MockSyncNetwork::default();
        let p1 = PeerId::random();
        let p2 = PeerId::random();
        mock.push_mesh_peers(vec![p1, p2]);
        let mut excluded = HashSet::new();
        excluded.insert(p1);
        excluded.insert(p2);

        let (open_timeout, retries, retry_delay) = defaults();
        // Crucially: NO `push_open_stream_*` calls. If the connect
        // loop tries to open_stream against an excluded peer, the
        // mock's "no queued response" Err surfaces — but that would
        // mean the filter failed. With the filter working, the
        // exhausted-mesh path returns Err without consuming the
        // open_stream queue.
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            &excluded,
        )
        .await;

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("could not open a namespace-join stream"),
            "unexpected err: {err}"
        );
        assert!(
            err.contains("excluded 2"),
            "err should report the excluded-peer count: {err}"
        );
        // The filter dropped both peers before open_stream got a
        // chance to be called, so the open_stream queue is still
        // empty (nothing seeded, nothing consumed). The mesh_peers
        // entry consumed-or-sticky-last; either way no panic.
        mock.assert_all_consumed();
    }

    /// Only the excluded peer is filtered; non-excluded peers in the
    /// same mesh still get tried. The mock seeds *only* one
    /// open_stream Err — if the filter also blocked the non-excluded
    /// peer, we'd see "no queued response" instead.
    #[tokio::test(start_paused = true)]
    async fn excluded_peer_skipped_other_mesh_peer_still_attempted() {
        let mock = MockSyncNetwork::default();
        let kept = PeerId::random();
        let blocked = PeerId::random();
        // `mesh_peers` is sticky-last in the mock (see module doc): a
        // single `push_mesh_peers` call seeds the same list for every
        // round. The test budget below (`retries` open_stream Errs)
        // depends on that — if sticky-last ever changes to return an
        // empty list after the first read, the assertion below would
        // pass vacuously instead of guarding the filter behaviour.
        mock.push_mesh_peers(vec![kept, blocked]);
        let mut excluded = HashSet::new();
        excluded.insert(blocked);

        // Per-round one peer remains → one open_stream attempt per
        // round → `retries` attempts total. Seed exactly that many
        // errors and assert_all_consumed below catches both
        // "filter let the blocked peer through" (would consume more
        // than seeded → error on exhaust) and "filter blocked the
        // kept peer too" (would consume fewer → unconsumed Errs).
        let (open_timeout, retries, retry_delay) = defaults();
        for i in 0..(retries as usize) {
            mock.push_open_stream_err(format!("kept-err-{i}"));
        }

        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            &excluded,
        )
        .await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("excluded 1"),
            "err should report 1 excluded peer for diagnostic symmetry: {err}"
        );
        mock.assert_all_consumed();
    }

    /// The recovery path: earlier peers fail, then a later peer's
    /// `open_stream` succeeds → the loop returns `Ok`. This is the one
    /// outcome the mock previously couldn't script (it had no synthetic
    /// `Ok(Stream)`), so the discovery loop's "fallback actually works"
    /// behaviour went unverified. Backed now by an in-memory
    /// `Stream::test_pair()` end.
    #[tokio::test(start_paused = true)]
    async fn peer_succeeds_after_earlier_failures_returns_ok() {
        let mock = MockSyncNetwork::default();
        // Three candidates in the (sticky) mesh; all are tried in
        // round 1 since 3 < DEADLINE_MAX_PEERS_PER_ROUND.
        mock.push_mesh_peers(vec![PeerId::random(), PeerId::random(), PeerId::random()]);
        // The mock ignores peer identity and pops responses in order:
        // the first two opens fail, the third succeeds.
        mock.push_open_stream_err("peer down")
            .push_open_stream_err("peer rejected")
            .push_open_stream_ok();

        let (open_timeout, retries, retry_delay) = defaults();
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            &no_excluded(),
        )
        .await;

        assert!(
            result.is_ok(),
            "discovery loop should recover once a later peer's open succeeds"
        );
        // Exactly the three scripted opens were consumed — the loop
        // stopped at the first success: no extra round, no leftovers.
        mock.assert_all_consumed();
    }
}
