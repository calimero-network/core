//! Calimero SDK for building CRDT-based distributed applications.
//!
//! # Event Handlers ⚠️
//!
//! **IMPORTANT**: Event handlers may execute in **parallel** (not guaranteed sequential).
//!
//! Your handlers MUST be:
//! - **Commutative**: Order-independent (use CRDTs)
//! - **Independent**: No shared mutable state
//! - **Idempotent**: Safe to retry
//! - **Pure**: Only modify CRDT state, no external side effects
//!
//! See [`event`] module documentation for detailed requirements and examples.

// Note: embed_abi macro is deprecated - use JSON files instead
// pub use calimero_wasm_abi::embed_abi;
pub use {borsh, serde, serde_json};

pub mod env;
pub mod event;
mod macros;
pub mod private_storage;
mod returns;
pub mod state;
/// In-process unit-test harness for app logic. Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub mod testing;
pub mod types;
pub use calimero_primitives::blobs::BlobId;
pub use calimero_primitives::context::ContextId;
pub use calimero_primitives::identity::PublicKey;
pub use state::read_raw;

pub mod app {
    use super::types::Error;

    pub type Result<T, E = Error> = core::result::Result<T, E>;

    pub use calimero_sdk_macros::{
        bail, destroy, emit, err, event, init, log, logic, migrate, private, state, Mergeable,
        Migrate,
    };

    use core::sync::atomic::{AtomicU32, Ordering};

    use crate::state::AppState;

    /// The schema version the currently-installed binary targets for its
    /// identity-gated writes.
    ///
    /// Set at install/migrate via [`register_schema_version`] from the active
    /// [`AppState::SCHEMA_VERSION`]; defaults to `0` (the unversioned value
    /// legacy apps carry) until then. Stored in an atomic — never observed
    /// across guest invocations on wasm (single-threaded, reset per call) but
    /// likewise sound for the native unit-test/TestHost harness.
    static SCHEMA_VERSION: AtomicU32 = AtomicU32::new(0);

    /// Returns the schema version the installed binary targets.
    ///
    /// Owner-driven migration (PR-6c) reads this — type-erased, with no access
    /// to the concrete state struct at the storage stamp site — to tag a stale
    /// identity-gated entry it rewrites. Returns `0` for binaries that never
    /// declared a version (legacy / unversioned apps).
    #[must_use]
    pub fn schema_version() -> u32 {
        SCHEMA_VERSION.load(Ordering::Relaxed)
    }

    /// Registers the active state's [`AppState::SCHEMA_VERSION`] as the value
    /// [`schema_version`] returns.
    ///
    /// Called at install and at migrate (alongside the event-emitter register)
    /// so the storage layer can stamp the right target without knowing the
    /// concrete state type.
    pub fn register_schema_version<S: AppState>() {
        SCHEMA_VERSION.store(S::SCHEMA_VERSION, Ordering::Relaxed);
    }
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod schema_version_tests {
    use borsh::{BorshDeserialize, BorshSerialize};

    use crate::event::NoEvent;
    use crate::state::{AppState, AppStateInit};

    // A binary that never declares a schema version — the legacy / unversioned
    // case. Inherits the trait's `SCHEMA_VERSION = 0` default. `Identity` is
    // blanket-impl'd for every `AppState`, so it isn't (and can't be) declared
    // here.
    #[derive(BorshSerialize, BorshDeserialize)]
    struct Unversioned;
    impl AppStateInit for Unversioned {
        type Return = Unversioned;
    }
    impl AppState for Unversioned {
        type Event<'a> = NoEvent;
    }

    // A v2 binary that declares its target.
    #[derive(BorshSerialize, BorshDeserialize)]
    struct V2;
    impl AppStateInit for V2 {
        type Return = V2;
    }
    impl AppState for V2 {
        type Event<'a> = NoEvent;
        const SCHEMA_VERSION: u32 = 2;
    }

    // One test, not two: the surfaced version lives in a process-global
    // static, so splitting these across parallel tests would let one's
    // `register` race the other's `schema_version` read. The default const and
    // the declared override are both checked here in sequence.
    #[test]
    fn schema_version_surface_reflects_active_app() {
        // An app that never declared SCHEMA_VERSION surfaces the unversioned 0.
        crate::app::register_schema_version::<Unversioned>();
        assert_eq!(
            crate::app::schema_version(),
            0,
            "an app that never declared SCHEMA_VERSION must surface the unversioned default 0"
        );

        // A binary that declares its target surfaces exactly that.
        crate::app::register_schema_version::<V2>();
        assert_eq!(
            crate::app::schema_version(),
            2,
            "register_schema_version must surface the declared AppState::SCHEMA_VERSION"
        );

        // Re-registering an unversioned app moves the surface back to 0.
        crate::app::register_schema_version::<Unversioned>();
        assert_eq!(crate::app::schema_version(), 0);
    }
}
