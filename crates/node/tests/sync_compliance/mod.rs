//! Sync Protocol Compliance Test Suite
//!
//! This module implements the compliance test suite defined in issue #1785.
//! Tests verify that the sync protocol implementation meets all CIP requirements.
//!
//! ## Categories (from issue #1785)
//!
//! - `negotiation.rs` - Protocol negotiation compliance (CIP §2.3)
//! - `buffering.rs` - Delta buffering compliance (TODO)
//! - `crdt_merge.rs` - CRDT merge compliance (TODO)
//! - `convergence.rs` - Convergence compliance (TODO)  
//! - `security.rs` - Security compliance (TODO)
//!
//! ## Adding Tests
//!
//! See `../sync_sim/AGENT_GUIDE.md` for framework usage.
//! Each test should reference the specific CIP section it validates.

pub mod negotiation;
