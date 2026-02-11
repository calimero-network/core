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
//! - `snapshot_merge_protection.rs` - Ensures Snapshot never overwrites initialized nodes (I5)
//! - `negotiation.rs` - Protocol negotiation tests (TODO)
//! - `snapshot.rs` - Snapshot sync path tests (TODO)
//! - `hash_compare.rs` - Hash comparison sync tests (TODO)
//! - `delta.rs` - Delta exchange tests (TODO)
//! - `partitions.rs` - Network partition tests (TODO)
//! - `failures.rs` - Fault tolerance tests (TODO)

pub mod snapshot_merge_protection;
