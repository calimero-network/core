//! Sync Protocol Scenario Tests
//!
//! This module contains simulation-based tests for sync protocol behavior.
//! Each test validates specific protocol scenarios using the sync_sim framework.
//!
//! ## Adding New Tests
//!
//! See `../sync_sim/AGENT_GUIDE.md` for detailed instructions.
//!
//! ## Test Categories
//!
//! - `snapshot_merge_protection.rs` - Invariant I5 protection tests
//! - `negotiation.rs` - Protocol negotiation tests
//! - `snapshot.rs` - Snapshot sync path tests
//! - `hash_compare.rs` - Hash comparison sync tests
//! - `delta.rs` - Delta exchange tests
//! - `partitions.rs` - Network partition tests
//! - `failures.rs` - Fault tolerance tests

pub mod snapshot_merge_protection;
