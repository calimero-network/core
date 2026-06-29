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

/// Open a stream to a namespace mesh peer.
///
/// Polls for a namespace mesh peer until one's stream opens or
/// `discovery_wait` elapses. Each round discovers mesh peers,
/// shuffles, and tries each with a per-peer `open_timeout`. Peers in
/// `excluded_peers` are filtered out before the inner loop —
/// `initiate_namespace_join` uses this to retry against a different
/// peer after one returns `NamespaceJoinRejected` without opening
/// fresh transports to the rejecting peer.
///
/// Two empty-round cases are handled differently, because they mean
/// different things:
///
/// * **No peer discovered at all** (the subscriber set is empty): the
///   namespace mesh hasn't formed yet. On a cold cross-network join
///   that's the normal state for the first tens of seconds while peer
///   discovery runs (see [`DEFAULT_NAMESPACE_DISCOVERY_WAIT_MS`]), so
///   we keep re-polling at `mesh_retry_delay` cadence for the whole
///   `discovery_wait` budget rather than giving up after a fixed
///   number of cheap rounds.
/// * **Every discovered peer is excluded** (all have already rejected
///   this join): the set won't change within this call, so this counts
///   as a failed round against `mesh_retries` and the caller's protocol
///   loop escalates instead of the join blocking on the full budget.
///
/// A round where peers were tried and all failed likewise counts
/// against `mesh_retries` so a small set of unreachable peers fails
/// over promptly.
///
/// Returns `Ok((stream, peer_id))` on first success or `Err(_)` once
/// `discovery_wait` elapses / the retry budget exhausts. The `peer_id`
/// lets the caller record a rejection and pass the peer back via
/// `excluded_peers` on the next call.
pub(super) async fn open_namespace_join_stream(
    sync_network: &dyn SyncNetwork,
    namespace_id: [u8; 32],
    open_timeout: std::time::Duration,
    mesh_retries: u32,
    mesh_retry_delay: std::time::Duration,
    discovery_wait: std::time::Duration,
    excluded_peers: &HashSet<PeerId>,
) -> eyre::Result<(Stream, PeerId)> {
    // A zero budget would make the first deadline check fire
    // immediately and return Err with a confusing "deadline 0ms,
    // elapsed 0ms" message. Production wiring always passes the
    // non-zero `DEFAULT_NAMESPACE_DISCOVERY_WAIT_MS`; assert (not
    // `debug_assert!`) so a degenerate value is caught in release too —
    // the one-time branch is free against the discovery loop's latency.
    assert!(!discovery_wait.is_zero(), "discovery_wait must be > 0");

    let topic = TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));

    // `tokio::time::Instant` (not `std::time::Instant`) so the
    // deadline tracks virtual time under `tokio::time::pause()` —
    // tests use `start_paused = true` to fast-forward through the
    // retry loop. In production it behaves identically to
    // `std::time::Instant`.
    let connect_started = tokio::time::Instant::now();

    let mut result: Option<(Stream, PeerId)> = None;
    // Rounds where we actually had a candidate peer to try (and it
    // failed) or where every discovered peer was excluded. Cold-start
    // rounds — nothing discovered yet — deliberately do NOT count, so
    // they wait out the full `discovery_wait` instead of burning this
    // budget in a few cheap polls.
    let mut failed_attempts: u32 = 0;

    'connect: loop {
        if connect_started.elapsed() >= discovery_wait {
            debug!(
                namespace_id = %hex::encode(namespace_id),
                elapsed_ms = connect_started.elapsed().as_millis() as u64,
                "namespace-join discovery budget exhausted, giving up"
            );
            break;
        }

        let discovered = sync_network.subscribed_peers(topic.clone()).await;
        let discovered_any = !discovered.is_empty();
        let mut peers = discovered;
        // Filter excluded peers before shuffling so an excluded peer
        // doesn't get picked first and then `continue`'d — that would
        // burn a slot in the shuffle order. Filtering up-front also
        // lets us distinguish "nothing discovered yet" from "everything
        // discovered is excluded" below.
        if !excluded_peers.is_empty() {
            peers.retain(|p| !excluded_peers.contains(p));
        }

        if peers.is_empty() {
            if discovered_any {
                // Every discovered peer is excluded — the set won't
                // change within this call, so spend the bounded retry
                // budget and let the caller's protocol loop escalate
                // rather than block on the whole discovery_wait.
                failed_attempts += 1;
                if failed_attempts >= mesh_retries {
                    break;
                }
            } else {
                // Cross-network discovery hasn't surfaced a namespace
                // peer yet. Keep polling until the budget elapses.
                debug!(
                    namespace_id = %hex::encode(namespace_id),
                    elapsed_ms = connect_started.elapsed().as_millis() as u64,
                    peer_count = 0,
                    "No namespace mesh peer discovered yet; waiting for cross-network discovery..."
                );
            }
            if connect_started.elapsed().saturating_add(mesh_retry_delay) >= discovery_wait {
                break;
            }
            time::sleep(mesh_retry_delay).await;
            continue;
        }

        // In-place shuffle avoids the second `Vec` allocation that
        // `choose_multiple` would produce. Matches the pattern used
        // in `perform_interval_sync`.
        peers.shuffle(&mut rand::thread_rng());

        for peer in &peers {
            if connect_started.elapsed() >= discovery_wait {
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
                        attempt = failed_attempts + 1,
                        %err,
                        "Failed to open namespace-join stream, trying next peer..."
                    );
                }
                Err(_) => {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        %peer,
                        attempt = failed_attempts + 1,
                        "Timed out opening namespace-join stream, trying next peer..."
                    );
                }
            }
        }

        failed_attempts += 1;
        if failed_attempts >= mesh_retries {
            break;
        }
        if connect_started.elapsed().saturating_add(mesh_retry_delay) >= discovery_wait {
            break;
        }
        debug!(
            namespace_id = %hex::encode(namespace_id),
            attempt = failed_attempts,
            peer_count = peers.len(),
            "No reachable namespace mesh peer yet, retrying..."
        );
        time::sleep(mesh_retry_delay).await;
    }

    let elapsed = connect_started.elapsed();
    result.ok_or_else(|| {
        eyre::eyre!(
            "could not open a namespace-join stream to any mesh peer for namespace {} \
             (deadline {}ms, elapsed {}ms, excluded {})",
            hex::encode(namespace_id),
            discovery_wait.as_millis(),
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
    ///
    /// `discovery_wait` is sized well above `mesh_retries` worth of
    /// failed rounds so the peer-present tests bind on the retry count
    /// (their historical behaviour); the cold-start tests bind on this
    /// budget instead.
    fn defaults() -> (Duration, u32, Duration, Duration) {
        // open_timeout, mesh_retries, mesh_retry_delay, discovery_wait
        (
            Duration::from_millis(100),
            3,
            Duration::from_millis(50),
            Duration::from_millis(1_350),
        )
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
        mock.push_subscribed_peers(vec![p1, p2]);
        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        // Each round tries every peer (3 × 2 = 6 attempts) and the
        // retry budget exhausts before any extra inner-loop attempt.
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
            discovery_wait,
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
        mock.push_subscribed_peers(vec![PeerId::random(), PeerId::random()]);
        // Every peer hangs far longer than open_timeout; tokio's
        // timeout should fire each time and we move on.
        for i in 0..20 {
            mock.push_open_stream_hang(Duration::from_secs(10), format!("hang-{i}"));
        }

        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        let start = time::Instant::now();
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
            &no_excluded(),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "expected Err from hanging peers, got Ok");
        // Peers are present every round, so the loop binds on the
        // retry budget: `retries` rounds, each spending one
        // `open_timeout` per hanging peer plus an inter-round sleep.
        // Bound generously by the whole discovery budget plus one
        // extra open_timeout slot (the per-peer check may bail an
        // in-flight attempt up to one timeout late).
        let upper_bound = discovery_wait.saturating_add(open_timeout);
        assert!(
            elapsed <= upper_bound,
            "loop took {elapsed:?}, expected ≤ {upper_bound:?} (discovery_wait {discovery_wait:?} \
             + one open_timeout slot)"
        );
    }

    /// Empty mesh in every round → no peers ever tried → Err once the
    /// `discovery_wait` budget elapses (cold-start polling path).
    #[tokio::test(start_paused = true)]
    async fn empty_mesh_every_round_returns_err() {
        let mock = MockSyncNetwork::default();
        // No `push_subscribed_peers` calls → subscribed_peers returns Vec::new()
        // (the "never seeded" path; production-legitimate when the
        // mesh hasn't formed yet).

        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
            &no_excluded(),
        )
        .await;

        assert!(
            result.is_err(),
            "expected Err when mesh stays empty for the whole discovery budget"
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
        // 10 peers — enough that an unbounded round overruns the budget.
        let many_peers: Vec<PeerId> = (0..10).map(|_| PeerId::random()).collect();
        mock.push_subscribed_peers(many_peers);
        // Every peer hangs for the full open_timeout — so the
        // per-peer cost lower-bound is open_timeout.
        for i in 0..50 {
            mock.push_open_stream_hang(Duration::from_secs(60), format!("h-{i}"));
        }

        let open_timeout = Duration::from_millis(200);
        let mesh_retries: u32 = 3;
        let mesh_retry_delay = Duration::from_millis(10);
        // Budget chosen so the per-peer deadline check, not the retry
        // count, is what bounds this. With 10 peers × 200ms each, an
        // unbounded round would take 2000ms — so the per-peer check
        // must bail somewhere inside round 2 to keep total under the
        // 2430ms budget.
        let discovery_wait = Duration::from_millis(2_430);

        let start = time::Instant::now();
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            mesh_retries,
            mesh_retry_delay,
            discovery_wait,
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
        // than the budget + one per-peer slot ≈ 2430 + 200 = 2630ms.
        assert!(
            elapsed < Duration::from_secs(3),
            "loop took {elapsed:?}, expected discovery budget + per-peer guard to bound this"
        );
    }

    /// `Arc<dyn SyncNetwork>` interop: the helper takes
    /// `&dyn SyncNetwork`, but in production `SyncManager` stores
    /// the network as `Arc<dyn SyncNetwork>`. Verify that
    /// `&*arc_value` coerces cleanly.
    #[tokio::test(start_paused = true)]
    async fn accepts_arc_dyn_sync_network() {
        let mock: Arc<dyn SyncNetwork> = Arc::new(MockSyncNetwork::default());

        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        let result = open_namespace_join_stream(
            &*mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
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
        mock.push_subscribed_peers(vec![p1, p2]);
        let mut excluded = HashSet::new();
        excluded.insert(p1);
        excluded.insert(p2);

        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        // Crucially: NO `push_open_stream_*` calls. If the connect
        // loop tries to open_stream against an excluded peer, the
        // mock's "no queued response" Err surfaces — but that would
        // mean the filter failed. With the filter working, every
        // discovered peer is excluded, which counts as a failed round
        // (not a cold-start wait), so the Err returns after the retry
        // budget without consuming the open_stream queue.
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
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
        // single `push_subscribed_peers` call seeds the same list for every
        // round. The test budget below (`retries` open_stream Errs)
        // depends on that — if sticky-last ever changes to return an
        // empty list after the first read, the assertion below would
        // pass vacuously instead of guarding the filter behaviour.
        mock.push_subscribed_peers(vec![kept, blocked]);
        let mut excluded = HashSet::new();
        excluded.insert(blocked);

        // Per-round one peer remains → one open_stream attempt per
        // round → `retries` attempts total. Seed exactly that many
        // errors and assert_all_consumed below catches both
        // "filter let the blocked peer through" (would consume more
        // than seeded → error on exhaust) and "filter blocked the
        // kept peer too" (would consume fewer → unconsumed Errs).
        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        for i in 0..(retries as usize) {
            mock.push_open_stream_err(format!("kept-err-{i}"));
        }

        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
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
        // Three candidates in the (sticky) mesh; all are tried within
        // a single round.
        mock.push_subscribed_peers(vec![PeerId::random(), PeerId::random(), PeerId::random()]);
        // The mock ignores peer identity and pops responses in order:
        // the first two opens fail, the third succeeds.
        mock.push_open_stream_err("peer down")
            .push_open_stream_err("peer rejected")
            .push_open_stream_ok();

        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
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

    /// Regression for the cold-start discovery bug: the loop must keep
    /// polling past `mesh_retries` empty rounds. The mesh is empty for
    /// the first three rounds — as many as `mesh_retries` — then a peer
    /// appears on the fourth. The prior round-count-bounded loop gave
    /// up at round three and missed a peer that cross-network discovery
    /// surfaces moments later; the discovery-budget loop finds it.
    #[tokio::test(start_paused = true)]
    async fn cold_start_peer_appearing_after_retry_budget_is_found() {
        let mock = MockSyncNetwork::default();
        let peer = PeerId::random();
        // Empty for `retries` (3) rounds, then the peer shows up
        // (sticky-last keeps returning it thereafter).
        mock.push_subscribed_peers(vec![])
            .push_subscribed_peers(vec![])
            .push_subscribed_peers(vec![])
            .push_subscribed_peers(vec![peer]);
        mock.push_open_stream_ok();

        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        assert_eq!(
            retries, 3,
            "test assumes the peer appears after exactly `retries` empty rounds"
        );
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
            &no_excluded(),
        )
        .await;

        assert!(
            result.is_ok(),
            "cold-start loop should keep polling past `mesh_retries` empty rounds and \
             find the late peer"
        );
        mock.assert_all_consumed();
    }

    /// The cold-start (nothing-discovered-yet) wait spans the whole
    /// `discovery_wait` budget, not the much shorter
    /// `mesh_retries × mesh_retry_delay` floor that bounded the prior
    /// round-counted loop. Empty mesh forever → Err only after ~the
    /// full budget elapses.
    #[tokio::test(start_paused = true)]
    async fn cold_start_waits_for_full_discovery_budget() {
        let mock = MockSyncNetwork::default();
        // Never seeded → `subscribed_peers` always empty (cold start).

        let (open_timeout, retries, retry_delay, discovery_wait) = defaults();
        let start = time::Instant::now();
        let result = open_namespace_join_stream(
            &mock,
            NAMESPACE_ID,
            open_timeout,
            retries,
            retry_delay,
            discovery_wait,
            &no_excluded(),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "empty mesh forever should still error");
        // Must wait well past the old `retries × retry_delay` floor
        // (3 × 50ms = 150ms) — that floor giving up early was the bug.
        let old_round_floor = retry_delay.saturating_mul(retries);
        assert!(
            elapsed > old_round_floor,
            "cold-start gave up after {elapsed:?}, at/under the old round floor \
             {old_round_floor:?} — it should wait the discovery budget {discovery_wait:?}"
        );
        // And must not overrun the budget by more than one poll cadence.
        assert!(
            elapsed <= discovery_wait.saturating_add(retry_delay),
            "cold-start waited {elapsed:?}, expected ≤ budget {discovery_wait:?} + one poll"
        );
    }
}
