//! Storage interface providing functionality for interacting with stored data.
//!
//! This module provides the primary interface for interacting with the storage
//! system, as a layer on top of the underlying database store.
//!

#![forbid(unreachable_pub, unsafe_op_in_unsafe_fn)]
#![deny(
    unsafe_code,
    clippy::expect_used,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![warn(
    missing_docs,
    clippy::future_not_send,
    clippy::let_underscore_untyped,
    clippy::map_err_ignore,
    clippy::pattern_type_mismatch,
    clippy::same_name_method,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::unreachable,
    clippy::use_debug
)]
//	Lints specifically disabled for unit tests
#![cfg_attr(
    test,
    allow(
        non_snake_case,
        clippy::arithmetic_side_effects,
        clippy::assigning_clones,
        clippy::cast_lossless,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cognitive_complexity,
        clippy::default_numeric_fallback,
        clippy::exhaustive_enums,
        clippy::exhaustive_structs,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::missing_assert_message,
        clippy::missing_panics_doc,
        clippy::must_use_candidate,
        clippy::panic,
        clippy::print_stdout,
        clippy::too_many_lines,
        clippy::unwrap_in_result,
        clippy::unwrap_used,
        reason = "Not useful in unit tests"
    )
)]

pub mod action;
pub mod address;
pub mod collections;
pub mod constants;
pub mod delta;
pub mod entities;
pub mod env;
pub mod error;
pub mod index;
pub mod integration;
pub mod interface;
pub mod js;
pub mod logical_clock;
pub mod merge;
pub mod rotation_log;
pub mod snapshot;
pub mod store;

/// CRDT convergence property-test harness (issue #2552).
///
/// Native (`cargo test`) harness that drives N in-memory replicas of a
/// `Mergeable` app-state type through randomly-interleaved operations and
/// asserts they converge to the same Merkle root. Gated on the `testing`
/// feature so the CRDT merge registry is compiled in (real registration is a
/// no-op otherwise).
#[cfg(all(not(target_arch = "wasm32"), feature = "testing"))]
pub mod testing;

// Re-export for convenience
// `register_crdt_merge` is WASM-only in production. Re-exposed under
// `cfg(test)` for the storage crate's own unit tests and under the
// `testing` feature flag for dependent crates' tests. See
// `merge::registry` module docs for the rationale (core#2469).
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
pub use merge::register_crdt_merge;
// Always-native wrapper used by the `TestHost` bridge; no-op unless the
// `testing` feature (or `cfg(test)`) compiles the registry in.
#[cfg(not(target_arch = "wasm32"))]
pub use merge::register_crdt_merge_for_test;

/// Re-exported types, mostly for use in macros (for convenience).
pub mod exports {
    pub use sha2::{Digest, Sha256};
}

/// Re-export the storage macros
pub use calimero_storage_macros::{AtomicUnit, Collection};

// Re-export commonly used types
pub use entities::{Data, Element};
pub use error::StorageError;
pub use interface::Interface;

/// Shared test functionality.
///
/// The module gate is `any(test, feature = "testing")` rather than plain
/// `#[cfg(test)]` so the cross-crate helpers in [`common`] (action signing /
/// builder utilities) are visible to dependent crates' tests through the
/// `testing` feature. Every other submodule stays `#[cfg(test)]`: they are
/// storage-internal unit tests with no cross-crate consumers, and exposing
/// them under the feature would pull dev-only dependencies into ordinary
/// `--features testing` builds.
///
/// `#[doc(hidden)]`: this is test/dev scaffolding, not public API. Consumers
/// must enable `testing` only in `[dev-dependencies]` (as `calimero-node`
/// does); it is not meant for production dependency graphs.
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub mod tests {
    /// Common test utilities and data structures.
    ///
    /// This is the one submodule exposed cross-crate via the `testing`
    /// feature.
    pub mod common;

    /// AuthoredMap/AuthoredVector merge-time auth tests.
    #[cfg(test)]
    pub mod authored_primitives;
    /// CRDT collections (UnorderedMap, Vector, Counter) tests.
    #[cfg(test)]
    pub mod collections;
    /// Concurrency race reproduction (core#2571).
    #[cfg(test)]
    pub mod concurrency;
    /// Comprehensive CRDT behavior tests.
    #[cfg(test)]
    pub mod crdt;
    /// Delta creation and commit tests.
    #[cfg(test)]
    pub mod delta;
    /// LWW (Last-Write-Wins) Register CRDT tests.
    #[cfg(test)]
    pub mod lww_register;
    /// CRDT type-based merge dispatch tests (TDD for PR #1889).
    #[cfg(test)]
    pub mod merge_dispatch;
    /// Merge integration tests (using serialization instead of Clone).
    #[cfg(test)]
    pub mod merge_integration;
    /// Merkle hash propagation tests.
    #[cfg(test)]
    pub mod merkle;
    /// RGA (Replicated Growable Array) CRDT tests.
    #[cfg(test)]
    pub mod rga;
    /// Sync-merge batch resilience: a single rejected action (e.g. an unsigned
    /// `Shared` action) must not abort the whole `Root::sync` batch (core#2716).
    #[cfg(test)]
    pub mod sync_batch_resilience;
    /// Storage-internal regression: the rotation-write hook depends on the
    /// stored-writers field staying frozen at bootstrap (see #2266 step 5).
    #[cfg(test)]
    pub mod write_hook_stale_writers;
    // TODO: Re-enable once Clone is implemented for collections
    // /// Nested CRDT merge behavior tests.
    // pub mod nested_crdt_merge;
}

#[cfg(test)]
mod doc_tests_package_usage {
    use calimero_sdk as _;
}
