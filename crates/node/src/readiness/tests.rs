use calimero_context_client::local_governance::SignedReadinessBeacon;
use calimero_primitives::identity::PrivateKey;

use super::*;

fn base_state() -> ReadinessState {
    ReadinessState {
        tier: ReadinessTier::Bootstrapping,
        local_applied_through: 5,
        local_pending_ops: 0,
        subscribed_at: Instant::now(),
    }
}

#[test]
fn bootstrapping_to_peer_validated_when_caught_up_with_peer() {
    let state = base_state();
    let peers = PeerSummary {
        max_applied_through: Some(5),
        heard_recent_beacon: true,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::PeerValidatedReady);
}

#[test]
fn bootstrapping_to_catching_up_when_behind_peer() {
    let state = base_state();
    let peers = PeerSummary {
        max_applied_through: Some(10),
        heard_recent_beacon: true,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    // CatchingUp now carries the target — verify both the variant and the
    // value so a regression that loses the target wouldn't pass with a
    // `matches!(_, _ { .. })` wildcard.
    assert_eq!(
        result,
        ReadinessTier::CatchingUp {
            target_applied_through: 10
        }
    );
}

#[test]
fn bootstrapping_to_locally_ready_after_boot_grace_with_no_peers() {
    let state = ReadinessState {
        subscribed_at: Instant::now() - Duration::from_secs(11),
        ..base_state()
    };
    let peers = PeerSummary {
        max_applied_through: None,
        heard_recent_beacon: false,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::LocallyReady);
}

#[test]
fn empty_dag_with_no_beacon_stays_bootstrapping() {
    let state = ReadinessState {
        local_applied_through: 0,
        subscribed_at: Instant::now() - Duration::from_secs(60),
        ..base_state()
    };
    let peers = PeerSummary {
        max_applied_through: None,
        heard_recent_beacon: false,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::Bootstrapping);
}

#[test]
fn empty_dag_with_peer_beacon_transitions_to_catching_up() {
    // Empty-DAG joiner that hears a peer beacon must move to CatchingUp so
    // backfill begins, and the variant must carry the peer's
    // applied_through as the target.
    let state = ReadinessState {
        local_applied_through: 0,
        subscribed_at: Instant::now() - Duration::from_secs(60),
        ..base_state()
    };
    let peers = PeerSummary {
        max_applied_through: Some(7),
        heard_recent_beacon: true,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(
        result,
        ReadinessTier::CatchingUp {
            target_applied_through: 7
        }
    );
}

#[test]
fn empty_dag_never_promotes_to_locally_ready_after_boot_grace() {
    // Even after boot grace with no peers, an empty DAG must NOT
    // self-promote.
    let state = ReadinessState {
        local_applied_through: 0,
        subscribed_at: Instant::now() - Duration::from_secs(3600),
        ..base_state()
    };
    let peers = PeerSummary {
        max_applied_through: None,
        heard_recent_beacon: false,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::Bootstrapping);
}

#[test]
fn pending_ops_always_demotes_to_degraded() {
    let state = ReadinessState {
        local_pending_ops: 3,
        ..base_state()
    };
    let peers = PeerSummary {
        max_applied_through: Some(5),
        heard_recent_beacon: true,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    // Degraded now carries the reason — verify the count flows through
    // verbatim.
    assert_eq!(
        result,
        ReadinessTier::Degraded {
            reason: DemotionReason::PendingOps(3)
        }
    );
}

#[test]
fn applied_through_grace_prevents_thrashing() {
    // Local at 8, peer at 9, grace=2 → still ready (8 + 2 >= 9).
    let state = ReadinessState {
        local_applied_through: 8,
        ..base_state()
    };
    let peers = PeerSummary {
        max_applied_through: Some(9),
        heard_recent_beacon: true,
    };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::PeerValidatedReady);
}

fn make_beacon(pk: PublicKey, applied_through: u64, strong: bool) -> SignedReadinessBeacon {
    SignedReadinessBeacon {
        namespace_id: [42u8; 32],
        peer_pubkey: pk,
        dag_head: [9u8; 32],
        applied_through,
        ts_millis: 0,
        strong,
        signature: [0u8; 64],
    }
}

#[test]
fn pick_sync_partner_prefers_strong_over_locally_ready() {
    let cache = ReadinessCache::default();
    let weak_pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let strong_pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(weak_pk, 100, false));
    cache.insert(&make_beacon(strong_pk, 50, true));
    let pick = cache
        .pick_sync_partner([42u8; 32], Duration::from_secs(60))
        .unwrap();
    assert_eq!(
        pick.0, strong_pk,
        "strong=true beats higher applied_through if strong=false"
    );
}

#[test]
fn pick_sync_partner_among_strong_picks_highest_applied_through() {
    let cache = ReadinessCache::default();
    let pk_a = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let pk_b = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk_a, 5, true));
    cache.insert(&make_beacon(pk_b, 10, true));
    let pick = cache
        .pick_sync_partner([42u8; 32], Duration::from_secs(60))
        .unwrap();
    assert_eq!(pick.0, pk_b);
}

#[test]
fn pick_sync_partner_excludes_stale_entries() {
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 5, true));
    // Wait beyond TTL by setting a very small TTL on the query.
    std::thread::sleep(Duration::from_millis(10));
    let pick = cache.pick_sync_partner([42u8; 32], Duration::from_millis(5));
    assert!(pick.is_none());
}

#[test]
fn pick_sync_partner_empty_cache_returns_none() {
    let cache = ReadinessCache::default();
    assert!(cache
        .pick_sync_partner([42u8; 32], Duration::from_secs(60))
        .is_none());
}

#[test]
fn insert_drops_stale_beacon_from_same_peer() {
    // Regression: gossipsub out-of-order delivery must not stale-overwrite
    // a fresher entry. The fresher beacon's `applied_through` and
    // `ts_millis` should remain after the older beacon arrives second.
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let mut fresh = make_beacon(pk, 100, true);
    fresh.ts_millis = 2000;
    let mut stale = make_beacon(pk, 50, true);
    stale.ts_millis = 1000;
    cache.insert(&fresh);
    cache.insert(&stale); // arrives second but is older — must be dropped
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(
        s.max_applied_through,
        Some(100),
        "stale beacon must not overwrite fresher entry from same peer",
    );
}

#[test]
fn insert_accepts_newer_beacon_from_same_peer() {
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let mut older = make_beacon(pk, 50, true);
    older.ts_millis = 1000;
    let mut newer = make_beacon(pk, 100, true);
    newer.ts_millis = 2000;
    cache.insert(&older);
    cache.insert(&newer); // arrives second and IS newer — must replace
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(s.max_applied_through, Some(100));
}

#[test]
fn insert_rejects_far_future_ts_millis() {
    // Cache-poisoning regression: a malicious or clock-skewed member could
    // sign a beacon with `ts_millis = year 2100`, then every legitimate
    // beacon would be rejected as "older". MAX_BEACON_CLOCK_DRIFT_MS (60s)
    // bounds the damage.
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut poison = make_beacon(pk, 999, true);
    poison.ts_millis = now_ms + 600_000; // 10 minutes ahead — well beyond 60s drift
    cache.insert(&poison);
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(
        s.max_applied_through, None,
        "far-future beacon must be rejected to prevent cache poisoning"
    );
    // A legitimate beacon afterwards should be accepted normally.
    let mut legit = make_beacon(pk, 42, true);
    legit.ts_millis = now_ms;
    cache.insert(&legit);
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(s.max_applied_through, Some(42));
}

#[test]
fn insert_accepts_ts_millis_within_clock_drift_window() {
    // 30s ahead is within the 60s drift window — should be accepted.
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut b = make_beacon(pk, 17, true);
    b.ts_millis = now_ms + 30_000;
    cache.insert(&b);
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(s.max_applied_through, Some(17));
}

#[test]
fn insert_uses_applied_through_to_break_ts_millis_ties() {
    // Same wall-clock millis (rare but possible across reboots / clock
    // skew): the higher applied_through wins.
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let mut a = make_beacon(pk, 10, true);
    a.ts_millis = 1000;
    let mut b = make_beacon(pk, 20, true);
    b.ts_millis = 1000;
    cache.insert(&a);
    cache.insert(&b);
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(s.max_applied_through, Some(20));
}

#[test]
fn peer_summary_atomic_when_fresh_peer_present() {
    // Snapshot must always have heard_recent_beacon == true ⇒
    // max_applied_through.is_some().
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 7, true));
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert!(s.heard_recent_beacon);
    assert_eq!(s.max_applied_through, Some(7));
}

#[test]
fn peer_summary_no_fresh_peers_returns_none_and_false() {
    let cache = ReadinessCache::default();
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert!(!s.heard_recent_beacon);
    assert_eq!(s.max_applied_through, None);
}

#[test]
fn peer_summary_excludes_stale_and_returns_none_after_ttl() {
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 9, false));
    std::thread::sleep(Duration::from_millis(10));
    let s = cache.peer_summary([42u8; 32], Duration::from_millis(5));
    assert!(!s.heard_recent_beacon);
    assert_eq!(s.max_applied_through, None);
}

#[tokio::test]
async fn await_first_fresh_beacon_resolves_immediately_when_cached() {
    let cache = ReadinessCache::default();
    let notify = ReadinessCacheNotify::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 5, true));
    let got = cache
        .await_first_fresh_beacon(
            &notify,
            [42u8; 32],
            Duration::from_secs(60),
            Duration::from_secs(5),
        )
        .await;
    assert!(got.is_some());
}

#[tokio::test]
async fn await_first_fresh_beacon_resolves_on_late_arrival() {
    // The race-fix test: spawns a writer that inserts AFTER the awaiter
    // has registered its `Notified` future via `enable()`. Without the
    // `enable()`-before-cache-check ordering, the wake fired by
    // `notify_waiters()` would be lost (Notify stores no permit) and
    // the awaiter would block until the timeout.
    let cache = std::sync::Arc::new(ReadinessCache::default());
    let notify = std::sync::Arc::new(ReadinessCacheNotify::default());
    let cache_w = cache.clone();
    let notify_w = notify.clone();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let _ = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cache_w.insert(&make_beacon(pk, 7, true));
        notify_w.notify([42u8; 32]);
    });
    let got = cache
        .await_first_fresh_beacon(
            &notify,
            [42u8; 32],
            Duration::from_secs(60),
            Duration::from_secs(2),
        )
        .await;
    assert!(got.is_some());
}

#[tokio::test]
async fn await_first_fresh_beacon_times_out() {
    let cache = ReadinessCache::default();
    let notify = ReadinessCacheNotify::default();
    let got = cache
        .await_first_fresh_beacon(
            &notify,
            [42u8; 32],
            Duration::from_secs(60),
            Duration::from_millis(50),
        )
        .await;
    assert!(got.is_none());
}
