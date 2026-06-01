//! Integration tests for the CRDT convergence harness (`calimero_storage::testing::converge`).
//!
//! Run with: `cargo test -p calimero-storage --features testing --test converge`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, Mergeable, UnorderedMap};
use calimero_storage::testing::converge;
use serial_test::serial;

/// Mirrors `apps/team-metrics-custom`: a map of team -> win counter, with a
/// custom (but correct) `Mergeable` impl. This is the issue's headline example.
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct TeamMetrics {
    teams: UnorderedMap<String, Counter>,
}

impl Mergeable for TeamMetrics {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.teams.merge(&other.teams)
    }
}

impl TeamMetrics {
    fn record_win(&mut self, team: &str) -> app::Result<()> {
        // Mirrors how real apps write methods: `app::Result` + `?` (StoreError
        // converts into `app::Error`). `entry().or_default()` returns a
        // write-back-guarded handle (core#2576); the increment persists when the
        // guard drops — no explicit re-insert.
        self.teams
            .entry(team.to_owned())?
            .or_default()?
            .increment()?;
        Ok(())
    }
}

#[test]
#[serial]
fn team_stats_converge() {
    converge::<TeamMetrics>()
        .replicas(3)
        .ops(|r| r.record_win("liverpool").unwrap())
        .assert_all_replicas_equal();
}

#[test]
#[serial]
fn team_stats_converge_many_ops_and_replicas() {
    converge::<TeamMetrics>()
        .replicas(5)
        .seed(12345)
        .ops(|r| r.record_win("liverpool").unwrap())
        .ops(|r| r.record_win("arsenal").unwrap())
        .ops(|r| r.record_win("liverpool").unwrap())
        .assert_all_replicas_equal();
}
/// A single inline `LwwRegister` whose convergence is governed by the storage
/// layer's last-writer-wins (highest HLC) reconciliation. Different inline-state
/// shape from the counter-based example above; both must converge.
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Inline {
    a: Counter,
    b: Counter,
}

impl Mergeable for Inline {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.a.merge(&other.a)?;
        self.b.merge(&other.b)
    }
}

impl Inline {
    fn bump_a(&mut self) {
        self.a.increment().unwrap();
    }
    fn bump_b(&mut self) {
        self.b.increment().unwrap();
    }
}

#[test]
#[serial]
fn two_counters_converge_under_interleaving() {
    converge::<Inline>()
        .replicas(4)
        .seed(99)
        .ops(|r| r.bump_a())
        .ops(|r| r.bump_b())
        .ops(|r| r.bump_a())
        .assert_all_replicas_equal();
}

// NOTE on negative testing: inducing a genuine split-brain from app code is
// hard *by design* — the storage layer reconciles collection-backed CRDTs at
// the child-entity level and inline scalars via HLC last-writer-wins, so a
// "broken" custom `Mergeable` is, in practice, either bypassed or run on empty
// shells (collection fields deserialize from the root entity as bare handles).
// The harness's divergence *detection* is therefore unit-tested directly in
// `calimero_storage::testing::tests::reports_divergence_when_hashes_differ`,
// rather than via a contrived non-convergent type here.
