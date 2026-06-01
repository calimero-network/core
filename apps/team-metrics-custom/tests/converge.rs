//! Convergence + correctness for the real `#[app::state]` app, driven through
//! the convergence harness. The custom-`Mergeable` `TeamStats` (hand-written
//! `RekeyTarget`, see `src/lib.rs`) is the #2577 headline case.
//!
//! `#[serial]`: `converge_app` clears and repopulates the process-global merge
//! registry per run, so two of these must not run concurrently. Own integration
//! binary so it's also isolated from the `TestHost` unit tests.

use calimero_storage::testing::converge_app;
use serial_test::serial;
use team_metrics_custom::TeamMetricsApp;

#[test]
#[serial]
fn team_stats_converge() {
    // Convergence (equal root hash) holds regardless of the #2577 fix — even the
    // pre-fix LWW path converges, just to a lossy value. The correctness of that
    // value is asserted separately below.
    converge_app(TeamMetricsApp::init)
        .replicas(3)
        .ops(|s| {
            let _ = s.record_win("liverpool".into());
        })
        .ops(|s| {
            let _ = s.record_win("arsenal".into());
        })
        .assert_all_replicas_equal();
}

#[test]
#[serial]
fn team_stats_converge_to_correct_value() {
    // Correctness: with #2577 merged, 3 replicas each recording one win must SUM
    // to 3 (not collapse to 1 via blob LWW). Register the generated re-key
    // thunks — the WASM-load / TestHost-bridge path — so the custom struct
    // value's nested counters get deterministic ids and converge as entities.
    TeamMetricsApp::__calimero_register_rekey();

    converge_app(TeamMetricsApp::init)
        .replicas(3)
        .ops(|s| {
            let _ = s.record_win("liverpool".into());
        })
        .invariant("liverpool wins == 3 (one per replica)", |s| {
            s.get_wins("liverpool".into()).unwrap_or(0) == 3
        })
        .assert_all_replicas_equal();
}
