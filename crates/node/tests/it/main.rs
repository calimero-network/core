//! The node's integration tests in one test binary. Each module here was a
//! `tests/*.rs` binary of its own, and each of those linked wasmtime, RocksDB and
//! tantivy (about 1.5 GB apiece) and compiled `harness/` and `common/` again.
//!
//! A test's name is `<module>::<test>`: `cargo test -p calimero-node --test it cluster::`
//! runs one module, `cargo nextest run -p calimero-node -E 'binary(it) & test(/^cluster::/)'`
//! the same under nextest.
//!
//! Two stay binaries of their own. `auto_updates.rs` sets hooks that are the whole
//! process's (`testing::shift_update_clock`, `fail_restore_points` and the count of
//! automatic-update passes), and `scenarios.rs` has a main of its own (`harness = false`).
//!
//! Tests here share one process under `cargo test` (nextest runs each in its own), so
//! a hook a test sets is keyed to its own nodes (a data directory or a public URL),
//! never the process's.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

mod common;
#[path = "../harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/scenarios.rs and benches/cluster_throughput.rs, which use parts no test here does"
)]
mod harness;

mod access_gate;
mod account_proofs;
mod activity;
mod app_origins;
mod approval_bypasses;
mod bench;
mod cluster;
mod component_cluster;
mod component_compute;
mod component_files;
mod component_packages;
mod compute_runs;
mod consent_page;
mod deployers;
mod deployments;
mod egress;
mod event_streams;
mod fixtures;
mod generated_routes;
mod guests;
mod instance_orgs;
mod job_nodes;
mod job_runs;
mod key_follow_ups;
mod lineage;
mod move_app;
mod org_app_teams;
mod org_member_security;
mod org_recovery;
mod org_rename;
mod package_origins;
mod peer_tls;
mod pipeline_runs;
mod public_links;
mod recovery;
mod recovery_disk;
mod refresh;
mod refused_writes;
mod release_lookups;
mod restore;
mod rollback;
mod rooms;
mod sessions;
mod shared_storage;
mod sign_up;
mod slow_backup;
mod slow_compile;
mod slow_disk;
mod state_caps;
mod upgrade_keys;
mod upgrade_replay;
