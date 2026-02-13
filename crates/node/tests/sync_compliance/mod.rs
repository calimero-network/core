//! Sync Protocol Compliance Test Suite
//!
//! This module implements the compliance test suite defined in issue #1785.
//! Tests verify that the sync protocol implementation meets all CIP requirements.
//!
//! ## Categories (from issue #1785)
//!
//! - `negotiation.rs` - CIP §2.3 Protocol negotiation compliance
//! - `crdt_merge.rs` - CIP §6.2 CRDT merge semantics (Invariant I5)
//! - `crdt_dispatch.rs` - CIP §6.2 CRDT type-based merge dispatch (TDD for PR #1889)
//! - `builtin_merge.rs` - Verify all builtin CRDT types merge correctly
//! - `convergence.rs` - CIP §2.4 Strategy equivalence (Invariant I4)
//! - Buffering compliance (TODO: I6)
//! - Security compliance (TODO)
//!
//! ## Invariants Tested
//!
//! | Invariant | Module | Description |
//! |-----------|--------|-------------|
//! | I4 | `convergence` | Strategy equivalence |
//! | I5 | `crdt_merge` | No silent data loss (protocol level) |
//! | I5 | `crdt_dispatch` | No silent data loss (storage layer merge dispatch) |
//! | I5 | `builtin_merge` | All builtin types merge without WASM |
//! | I6 | (TODO) | Delta buffering during sync |
//!
//! ## Adding Tests
//!
//! See `../sync_sim/AGENT_GUIDE.md` for framework usage.
//! Each test should reference the specific CIP section it validates.

pub mod builtin_merge;
pub mod convergence;
pub mod crdt_dispatch;
pub mod crdt_merge;
pub mod negotiation;
