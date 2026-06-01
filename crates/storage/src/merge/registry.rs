//! Merge registry for automatic CRDT merging — WASM-side only.
//!
//! The `#[app::state]` macro emits a `__calimero_register_merge` WASM
//! export the runtime calls at module-load time; that export calls
//! `register_crdt_merge::<AppState>()` inside the WASM module, which
//! writes a merge closure into this registry. WASM-side
//! `merge_root_state` (called from `Interface::save_internal` when a
//! root-entity action applies inside WASM) then consults the registry
//! to dispatch the typed merge.
//!
//! ## Scope
//!
//! Compiled **only** when `cfg(target_arch = "wasm32")` or `cfg(test)`.
//! The host binary doesn't link the registry at all:
//!
//! - Host code can't accidentally call `register_crdt_merge` — the
//!   symbol doesn't exist. (Pre-cleanup it did, but nothing ever
//!   populated it, and host-side `merge_root_state` consulting an
//!   empty registry was the root cause of core#2469.)
//! - Host root-state merges route through WASM via the macro-generated
//!   `__calimero_merge_root_state` export +
//!   [`crate::merge::merge_root_state_typed`] +
//!   `ContextClient::merge_root_state`. The registry is no longer the
//!   dispatch boundary.
//!
//! Tests still build the registry so the existing
//! `merge_registry_integration` / `merge_registry_concurrent` /
//! `merge_integration` test suites keep exercising the dispatch shape
//! the WASM side relies on.
//!
//! ## Storage backend
//!
//! Production WASM uses a process-global `RwLock<HashMap<...>>` — apps
//! register their state types once at static-init and dispatch is
//! against the same table thereafter. Tests use a `thread_local!` so
//! parallel-running tests can't stomp on each other's registrations.

#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
use std::any::TypeId;
#[cfg(test)]
use std::cell::RefCell;
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
use std::sync::{LazyLock, RwLock};

/// Function signature for merging serialized state
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
pub type MergeFn = fn(&[u8], &[u8], u64, u64) -> Result<Vec<u8>, Box<dyn std::error::Error>>;

/// Result of attempting to merge using registered merge functions.
///
/// Available on every target — `merge_root_state` pattern-matches on
/// this enum to choose the bootstrap / I5-error / LWW-fallback path.
/// On host production builds the registry doesn't exist so the
/// `Success` and `AllFunctionsFailed` arms can't be produced from
/// inside the storage crate, but the enum still has to be matchable
/// (the host stub in `merge_root_state` constructs
/// `NoFunctionsRegistered` and the match handles it identically to the
/// WASM path).
#[derive(Debug)]
#[must_use]
pub enum MergeRegistryResult {
    /// A registered merge function succeeded
    Success(Vec<u8>),
    /// No merge functions are registered (I5 enforcement needed)
    NoFunctionsRegistered,
    /// Merge functions are registered but all failed (e.g., type mismatch)
    AllFunctionsFailed,
}

/// Production registry — process-global, shared across async workers.
#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
static MERGE_REGISTRY: LazyLock<RwLock<HashMap<TypeId, MergeFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

// Test registry — per-thread. cargo test runs tests in parallel on
// different threads; with a global registry, a test calling
// `clear_merge_registry()` (e.g. to assert "no merge functions
// registered" behaviour) could wipe entries that another thread's
// test had just populated via `register_test_merge_functions()`, and
// the subsequent `apply_action` on that other thread would then fail
// dispatch mid-flight. `#[serial]` only serialises the clearers
// against each other — unrelated non-serial tests still ran in
// parallel with them. Thread-local storage makes each test's
// registry state private to its own thread, so the race can't occur.
//
// Trade-off: unit tests can no longer observe cross-thread visibility
// of registrations (a property the production RwLock does provide).
// We consider this acceptable — the cross-thread-share story is
// delegated to `std::sync::RwLock`, which we trust, and in practice
// apps register their types once at startup before any async workers
// are spawned. If cross-thread dispatch ever becomes part of the
// behaviour-under-test (not just implementation detail), that belongs
// in an integration test compiled without `#[cfg(test)]` rather than
// here.
#[cfg(test)]
thread_local! {
    static MERGE_REGISTRY: RefCell<HashMap<TypeId, MergeFn>> = RefCell::new(HashMap::new());
}

// Both backends assume merge functions don't call back into the registry
// while one is being dispatched. A reentrant call would deadlock under
// RwLock (write-during-read) or panic under RefCell (already borrowed) —
// either way a bug. The merge closure built in `register_crdt_merge` only
// calls borsh and the type's own `Mergeable::merge`; adding registry
// access to it would break this invariant.
//
// Production-path coverage: because unit tests only exercise the
// `#[cfg(test)]` thread-local backend, register+dispatch against the
// real `RwLock` path is covered by the integration test at
// `tests/merge_registry_integration.rs`. If you touch the
// `#[cfg(not(test))]` helpers below, that integration test is the thing
// to run.

// The `with_registry_mut` / `with_registry` helpers come in paired
// cfg-gated implementations (prod: RwLock, test: thread-local RefCell).
// The signatures must stay identical so call sites compile under both
// configurations; if you change one pair, change the other. Divergence
// would go undetected — unit tests only build the test variants, the
// integration test only builds the prod variants.

/// Runs `f` with mutable access to the registry. Aborts the process
/// if the lock is poisoned (a poisoned lock means a prior writer
/// panicked, which we treat as unrecoverable for a process-global
/// singleton).
#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
fn with_registry_mut<R>(f: impl FnOnce(&mut HashMap<TypeId, MergeFn>) -> R) -> R {
    let mut registry = MERGE_REGISTRY.write().unwrap_or_else(|_| {
        tracing::error!(
            target: "calimero_storage::merge",
            "MERGE_REGISTRY lock poisoned during write, aborting. This indicates a panic in merge code."
        );
        std::process::abort()
    });
    f(&mut registry)
}

/// Test-only variant: uses thread-local RefCell instead of global
/// RwLock. `try_borrow_mut` surfaces re-entrant access as a clear
/// panic message rather than the less-useful default `BorrowMutError`
/// debug print. Keep the signature in sync with the `cfg(not(test))`
/// variant above.
#[cfg(test)]
fn with_registry_mut<R>(f: impl FnOnce(&mut HashMap<TypeId, MergeFn>) -> R) -> R {
    MERGE_REGISTRY.with(|r| {
        let mut borrowed = r.try_borrow_mut().unwrap_or_else(|e| {
            panic!(
                "MERGE_REGISTRY RefCell already borrowed ({e}). Merge \
                 functions must not call try_merge_registered / \
                 register_crdt_merge / clear_merge_registry (see module docs)."
            )
        });
        f(&mut borrowed)
    })
}

/// Runs `f` with read-only access to the registry. Aborts the process
/// if the lock is poisoned (same reasoning as `with_registry_mut`).
#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
fn with_registry<R>(f: impl FnOnce(&HashMap<TypeId, MergeFn>) -> R) -> R {
    let registry = MERGE_REGISTRY.read().unwrap_or_else(|_| {
        tracing::error!(
            target: "calimero_storage::merge",
            "MERGE_REGISTRY lock poisoned, aborting. This indicates a panic in merge code."
        );
        std::process::abort()
    });
    f(&registry)
}

/// Test-only variant: uses thread-local RefCell instead of global
/// RwLock. Keep the signature in sync with the `cfg(not(test))`
/// variant above.
#[cfg(test)]
fn with_registry<R>(f: impl FnOnce(&HashMap<TypeId, MergeFn>) -> R) -> R {
    MERGE_REGISTRY.with(|r| {
        let borrowed = r.try_borrow().unwrap_or_else(|e| {
            panic!(
                "MERGE_REGISTRY RefCell already mutably borrowed ({e}). \
                 Merge functions must not call try_merge_registered / \
                 register_crdt_merge / clear_merge_registry (see module docs)."
            )
        });
        f(&borrowed)
    })
}

/// Register a CRDT merge function for a type.
///
/// # Testing note
///
/// Under `#[cfg(test)]` the registry is **thread-local**, not global.
/// Registrations made in one test thread are not visible from a
/// `std::thread::spawn` / `tokio::spawn` worker on another thread. If
/// a test relies on cross-thread dispatch, move it to an integration
/// test under `tests/` (which links the library without `cfg(test)`
/// and therefore hits the real `RwLock`-backed registry).
///
/// Production callers (non-test builds) see the global `RwLock`
/// registry as expected.
///
/// # Example
///
/// ```ignore
/// #[derive(BorshSerialize, BorshDeserialize)]
/// struct MyState {
///     counter: Counter,
///     metadata: UnorderedMap<String, String>,
/// }
///
/// impl Mergeable for MyState {
///     fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
///         self.counter.merge(&other.counter)?;
///         self.metadata.merge(&other.metadata)?;
///         Ok(())
///     }
/// }
///
/// // Register at app startup
/// register_crdt_merge::<MyState>();
/// ```
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
pub fn register_crdt_merge<T>()
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + crate::collections::Mergeable + 'static,
{
    let type_id = TypeId::of::<T>();

    let merge_fn: MergeFn = |existing, incoming, _existing_ts, _incoming_ts| {
        // Deserialize both states
        let mut existing_state = borsh::from_slice::<T>(existing)
            .map_err(|e| format!("Failed to deserialize existing state: {}", e))?;

        let incoming_state = borsh::from_slice::<T>(incoming)
            .map_err(|e| format!("Failed to deserialize incoming state: {}", e))?;

        // Merge using Mergeable trait
        // CRITICAL: Use merge mode to prevent timestamp generation during merge.
        // Without this, different nodes generate different timestamps, causing
        // hash divergence even when logical state is identical.
        crate::env::with_merge_mode(|| {
            existing_state
                .merge(&incoming_state)
                .map_err(|e| format!("Merge failed: {}", e))
        })?;

        // Serialize result
        borsh::to_vec(&existing_state).map_err(|e| format!("Serialization failed: {}", e).into())
    };

    with_registry_mut(|registry| {
        let _ = registry.insert(type_id, merge_fn);
    });
}

/// Registers a CRDT merge function for the in-process test harness.
///
/// This is the entry point the `#[app::state]`-generated
/// `calimero_sdk::testing::TestState` bridge calls. Unlike
/// [`register_crdt_merge`], it is available in *all* native builds, so an app's
/// `#[cfg(test)]` bridge compiles whether or not the app opted into the
/// `testing` feature — that keeps `cargo test` working for example apps that
/// have no `TestHost` tests of their own.
///
/// It only performs real registration when the registry is actually compiled in
/// (`test` or the `testing` feature). Without that, it is a no-op: an app that
/// genuinely drives `TestHost` but forgot to enable
/// `calimero-storage`'s `testing` feature will see a clear
/// `NoMergeFunctionRegistered` error at runtime, pointing at the missing
/// dev-dependency, rather than a confusing compile error in macro-generated
/// code.
///
/// Host-binary safety is unchanged: the gated [`register_crdt_merge`] symbol
/// stays absent from production host builds, and this wrapper does nothing there
/// either.
#[cfg(not(target_arch = "wasm32"))]
pub fn register_crdt_merge_for_test<T>()
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + crate::collections::Mergeable + 'static,
{
    #[cfg(any(test, feature = "testing"))]
    register_crdt_merge::<T>();
}

/// Clear the merge registry (for testing only)
#[cfg(any(test, feature = "testing"))]
pub fn clear_merge_registry() {
    with_registry_mut(|registry| registry.clear());
}

/// Try to merge using registered merge function
///
/// Returns:
/// - `Success(merged)` if a merge function succeeded
/// - `NoFunctionsRegistered` if no merge functions are registered (I5 violation)
/// - `AllFunctionsFailed` if merge functions exist but none could merge the data
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
pub fn try_merge_registered(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> MergeRegistryResult {
    // For now, we don't have type information at runtime.
    // TODO: Store type hints with root entity for O(1) dispatch (see issue #1993)

    // Try each registered merge function until one succeeds (O(n) where n = registered types)
    with_registry(|registry| {
        if registry.is_empty() {
            return MergeRegistryResult::NoFunctionsRegistered;
        }

        for (_type_id, merge_fn) in registry.iter() {
            if let Ok(merged) = merge_fn(existing, incoming, existing_ts, incoming_ts) {
                return MergeRegistryResult::Success(merged);
            }
        }

        MergeRegistryResult::AllFunctionsFailed
    })
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;
    use crate::collections::{Counter, Mergeable};
    use crate::env;

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
    struct TestState {
        counter: Counter,
    }

    impl Mergeable for TestState {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.counter.merge(&other.counter)
        }
    }

    #[test]
    #[serial]
    fn test_register_and_merge() {
        env::reset_for_testing();
        clear_merge_registry(); // Clear any previous registrations to ensure clean test

        // Register the type
        register_crdt_merge::<TestState>();

        // Create two states with different executor IDs (use unique IDs to avoid test contamination)
        env::set_executor_id([10; 32]);
        let mut state1 = TestState {
            counter: Counter::new(),
        };
        state1.counter.increment().unwrap();
        state1.counter.increment().unwrap(); // value = 2

        env::set_executor_id([20; 32]);
        let mut state2 = TestState {
            counter: Counter::new(),
        };
        state2.counter.increment().unwrap(); // value = 1

        // Serialize
        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        // Merge via registry
        let merged_bytes = match try_merge_registered(&bytes1, &bytes2, 100, 200) {
            MergeRegistryResult::Success(bytes) => bytes,
            MergeRegistryResult::NoFunctionsRegistered => {
                panic!("Expected merge function to be registered")
            }
            MergeRegistryResult::AllFunctionsFailed => {
                panic!("Expected merge to succeed")
            }
        };

        // Deserialize result
        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();

        // Verify: counters summed (2 + 1 = 3)
        assert_eq!(merged.counter.value().unwrap(), 3);
    }

    #[test]
    #[serial]
    fn test_no_merge_function_registered_returns_error() {
        use crate::merge::merge_root_state;

        env::reset_for_testing();
        clear_merge_registry(); // Ensure registry is empty

        // Create some arbitrary data
        let data1 = vec![1, 2, 3, 4];
        let data2 = vec![5, 6, 7, 8];

        // Attempt merge with no registered functions.
        // `existing_created_at` (50) != `existing_ts` (100), so the
        // bootstrap-default branch is NOT taken — this exercises the
        // I5 error path.
        let result = merge_root_state(&data1, &data2, 50, 100, 200);

        // Should return NoMergeFunctionRegistered error (I5 enforcement)
        assert!(
            result.is_err(),
            "Expected error when no merge function is registered"
        );

        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                crate::collections::crdt_meta::MergeError::NoMergeFunctionRegistered
            ),
            "Expected NoMergeFunctionRegistered error, got: {:?}",
            err
        );
    }

    /// Bootstrap signal: when the existing entity was created and has
    /// never been explicitly updated since (`created_at == updated_at`),
    /// `merge_root_state` must accept incoming unconditionally, even
    /// when no merge function is registered. This is the regression
    /// guard for the 2026-05-14 LWW-by-HLC inversion (Ronit/Fran
    /// bootstrap, sync-regression smoke, mero-chat integration —
    /// see PR description for the full timeline).
    #[test]
    #[serial]
    fn test_bootstrap_accepts_incoming_without_registered_merger() {
        use crate::merge::merge_root_state;

        env::reset_for_testing();
        clear_merge_registry(); // No app-registered merger

        let local_default = vec![0u8; 0]; // imagine: WASM-default-serialised state
        let remote_real = vec![1, 2, 3, 4]; // sender's real bytes

        // Bootstrap shape: existing was created and never written ⇒
        // its created_at equals its updated_at. The receiver's local
        // HLC at materialisation (100) is later than the sender's
        // earlier real write (50) — exactly the LWW-by-HLC inversion
        // that the pre-fix code lost on.
        let result = merge_root_state(
            &local_default,
            &remote_real,
            /* existing_created_at */ 100,
            /* existing_ts        */ 100,
            /* incoming_ts        */ 50,
        );

        assert!(
            result.is_ok(),
            "Bootstrap (created == updated, no merger) must accept incoming, got: {:?}",
            result
        );
        assert_eq!(
            result.unwrap(),
            remote_real,
            "Bootstrap must return the incoming bytes verbatim"
        );
    }

    /// Counter-test for the bootstrap branch: if the existing entity
    /// has been written since creation (`created_at != updated_at`),
    /// the bootstrap fast-path must NOT trigger — without a registered
    /// merger this is a real divergence and must fail loudly per I5.
    #[test]
    #[serial]
    fn test_post_bootstrap_no_merger_errors_loudly() {
        use crate::merge::merge_root_state;

        env::reset_for_testing();
        clear_merge_registry();

        let existing = vec![9, 9, 9];
        let incoming = vec![7, 7, 7];

        // Existing has been updated since creation (created_at 50 < updated_at 100).
        let result = merge_root_state(
            &existing, &incoming, /* existing_created_at */ 50,
            /* existing_ts        */ 100, /* incoming_ts        */ 80,
        );

        assert!(
            result.is_err(),
            "Post-bootstrap with no merger must error (I5), got Ok: {:?}",
            result
        );
        assert!(
            matches!(
                result.unwrap_err(),
                crate::collections::crdt_meta::MergeError::NoMergeFunctionRegistered
            ),
            "Expected NoMergeFunctionRegistered for post-bootstrap-no-merger"
        );
    }
}
