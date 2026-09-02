//! Convergence + correctness for the real `#[app::state]` app, driven through
//! the convergence harness. `TeamStats` is `#[app::mergeable]` (see
//! `src/lib.rs`), so it exercises two different mechanisms at once — and the
//! tests are split accordingly.
//!
//! `wins` is a `Counter`: it converges structurally, as its own child entity,
//! whether or not the app declares a rule. It is the #2577 headline case.
//!
//! `badges` is a plain `u64` bitmask in the value blob. It converges ONLY
//! because the app's rule is dispatched; without that it resolves
//! last-write-wins. A counter test cannot distinguish the two, which is why both
//! are here.
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

// The app's own merge rule is NOT asserted here, and that is deliberate.
//
// Proving it needs two replicas to write DIFFERENT values into the same plain
// field, and this harness cannot arrange that. It gives each replica a distinct
// executor inside `RuntimeEnv` — which is why the counters above diverge and
// then sum — but never sets the SDK's thread-local device id, so
// `env::device_id()` returns the same default `[237; 32]` in every replica. An
// app method therefore cannot tell which replica is running it, and every
// replica computes the same plain-field value. No conflict, nothing to merge.
//
// Three versions of such a test were written and all three were vacuous; each
// passed with `TeamStats::merge` gutted:
//
//   1. a `max` rule — returns one of its inputs, and so does last-write-wins,
//      so the two agree whenever LWW's tiebreak picks the larger side;
//   2. two `.ops(..)` closures awarding explicit badges — the harness applies
//      EVERY op to EVERY replica, so both computed the union directly;
//   3. a per-device badge — defeated by the thread-local above.
//
// Where the proof actually lives:
//
//   * `calimero-storage/tests/custom_merge_e2e.rs` — two replicas, real map,
//     per-replica writes, asserts the app rule decides AND the roots converge.
//   * `workflows/team-metrics-custom.yml` — two real nodes, each awarding its
//     own badge, asserting both end up holding both.
//
// A weaker restatement here would only look like coverage.

// `assert_merge_laws` is NOT applied to `TeamStats`, and the reason is a real
// limit rather than an oversight.
//
// The helper compares borsh encodings, and `TeamStats` embeds three `Counter`
// handles. A handle is storage IDENTITY, not value: constructed outside a
// storage env each one gets a random id, so `merge(a, b)` and `merge(b, a)`
// encode differently in the first ~96 bytes while agreeing perfectly on
// `badges` — the field the rule actually decides. The helper would report a
// commutativity violation that is an artifact of the handles.
//
// That is not a gap in coverage. The counters converge structurally whatever
// `merge` does, so the only thing worth checking here is `badges`, and the laws
// belong on a type that is plain data. See `calimero-storage/tests/merge_laws.rs`.
