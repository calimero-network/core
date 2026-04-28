use super::*;

fn base_state() -> ReadinessState {
    ReadinessState {
        tier: ReadinessTier::Bootstrapping,
        local_applied_through: 5,
        local_head: [0u8; 32],
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
