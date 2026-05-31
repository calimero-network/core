//! In-process unit-test harness for Calimero application logic.
//!
//! [`TestHost`] lets you exercise an `#[app::state]` type as ordinary Rust —
//! no `wasm32` build, no node, no containers. State lives in an in-memory mock
//! store, events and logs are captured, and the executor / context identity is
//! configurable, so you can write millisecond-level `#[cfg(test)]` assertions
//! and practise TDD against your app.
//!
//! # Example
//!
//! ```ignore
//! use calimero_sdk::testing::TestHost;
//!
//! #[test]
//! fn counter_increments() {
//!     // `build` runs after the mock host is reset, so collections are created
//!     // against a clean store — exactly like the real `#[app::init]` path.
//!     let mut app = TestHost::new(|| MyApp::init());
//!
//!     app.call(|s| s.increment("a".into())).unwrap();
//!     let value = app.view(|s| s.get("a".into())).unwrap();
//!     assert_eq!(value, 1);
//!
//!     // Events emitted via `app::emit!` are captured.
//!     assert_eq!(app.events().len(), 1);
//! }
//! ```
//!
//! # What the bridge does
//!
//! `TestHost` is generic over your state type but can't name
//! `calimero_storage::collections::Root` directly (the storage crate depends on
//! the SDK, not the other way around). The [`#[app::state]`](crate::app::state)
//! macro closes the loop by generating a [`TestState`] impl that drives `Root`
//! for load / mutate / commit. You never implement [`TestState`] by hand.

use core::marker::PhantomData;

use crate::env::host;
use crate::event::{self, AppEventExt};
use crate::state::AppState;

/// An event captured by the harness during [`TestHost::call`] / `view`.
pub use crate::env::host::CapturedEvent as Event;

/// Bridge between [`TestHost`] and the storage layer.
///
/// Implemented automatically for every `#[app::state]` type. The methods drive
/// `calimero_storage::collections::Root` to install, load, mutate, and commit
/// state against the native mock store. **You should never implement or call
/// these directly** — use [`TestHost`].
#[doc(hidden)]
pub trait TestState: Sized {
    /// Clears the storage-layer mock so a fresh harness starts from empty state.
    fn __test_reset();

    /// Builds the initial state (mirroring `#[app::init]`: assign deterministic
    /// IDs, then commit) against the freshly-reset store.
    fn __test_install(build: &mut dyn FnMut() -> Self);

    /// Loads the committed state, runs `f` against `&mut Self`, then commits.
    fn __test_with_mut(f: &mut dyn FnMut(&mut Self));

    /// Loads the committed state and runs `f` against `&Self` (no commit).
    fn __test_with_ref(f: &mut dyn FnMut(&Self));

    /// Runs `f` with the storage-layer executor identity set to `id`.
    fn __test_with_executor(id: [u8; 32], f: &mut dyn FnMut());
}

/// An in-process test host wrapping a single application state instance.
///
/// Construct one with [`TestHost::new`], drive mutations with
/// [`call`](TestHost::call), read with [`view`](TestHost::view), and inspect
/// captured [`events`](TestHost::events) / [`logs`](TestHost::logs).
///
/// State is held in a thread-local mock store, so a `TestHost` owns the store
/// for its thread: creating a new one resets that state. Run independent
/// scenarios in separate `#[test]` functions (Rust runs them on separate
/// threads) or construct a fresh `TestHost` per scenario.
#[must_use = "a TestHost only does work when you call/view through it"]
pub struct TestHost<S> {
    _state: PhantomData<S>,
}

impl<S> TestHost<S>
where
    S: TestState + AppState,
    for<'a> S::Event<'a>: AppEventExt,
{
    /// Creates a fresh harness, resetting all mock host + storage state and
    /// installing the state produced by `build`.
    ///
    /// `build` runs *after* the reset so any collections it allocates are
    /// created against a clean store — the same ordering the real
    /// `#[app::init]` entrypoint guarantees.
    pub fn new(build: impl FnOnce() -> S) -> Self {
        host::reset();
        S::__test_reset();
        event::register::<S>();

        let mut build = Some(build);
        S::__test_install(&mut || (build.take().expect("state builder called more than once"))());

        Self {
            _state: PhantomData,
        }
    }

    /// Runs a mutating method against the state and commits the result.
    ///
    /// The closure receives `&mut S`, mirroring how the WASM runtime hands a
    /// loaded `Root<S>` to a `&mut self` method. Returns whatever the closure
    /// returns (e.g. the method's `app::Result`).
    pub fn call<R>(&mut self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut out = None;
        let mut f = Some(f);
        S::__test_with_mut(&mut |state| {
            out = Some((f.take().expect("call closure invoked once"))(state));
        });
        out.expect("state was loaded and the closure ran")
    }

    /// Runs a read-only method against the state without committing.
    pub fn view<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        let mut out = None;
        let mut f = Some(f);
        S::__test_with_ref(&mut |state| {
            out = Some((f.take().expect("view closure invoked once"))(state));
        });
        out.expect("state was loaded and the closure ran")
    }

    /// Runs a mutating method as a specific executor identity, then restores the
    /// previous identity.
    ///
    /// Sets both the SDK-level [`env::executor_id`](crate::env::executor_id)
    /// (what app logic reads) and the storage-layer authorship identity (what
    /// CRDT element writes record), so multi-author scenarios resolve exactly as
    /// they would across nodes.
    pub fn call_as<R>(&mut self, executor: [u8; 32], f: impl FnOnce(&mut S) -> R) -> R {
        let prev = host::executor_id();
        host::set_executor_id(executor);

        let mut out = None;
        let mut f = Some(f);
        S::__test_with_executor(executor, &mut || {
            let mut inner = f.take();
            S::__test_with_mut(&mut |state| {
                out = Some((inner.take().expect("call_as closure invoked once"))(state));
            });
        });

        host::set_executor_id(prev);
        out.expect("state was loaded and the closure ran")
    }

    /// Returns every event captured since the harness was created (or since the
    /// last [`take_events`](TestHost::take_events)).
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        host::events()
    }

    /// Removes and returns the captured events, clearing the buffer.
    #[must_use]
    pub fn take_events(&self) -> Vec<Event> {
        host::take_events()
    }

    /// Returns every log line captured since the harness was created.
    #[must_use]
    pub fn logs(&self) -> Vec<String> {
        host::logs()
    }

    /// Removes and returns the captured log lines, clearing the buffer.
    #[must_use]
    pub fn take_logs(&self) -> Vec<String> {
        host::take_logs()
    }

    /// Overrides the executor identity reported to app logic for subsequent
    /// `call` / `view` invocations.
    ///
    /// Affects only the SDK-level identity. To also drive CRDT authorship for a
    /// single mutation, prefer [`call_as`](TestHost::call_as).
    pub fn set_executor(&mut self, id: [u8; 32]) {
        host::set_executor_id(id);
    }

    /// Overrides the context identity reported to app logic.
    pub fn set_context(&mut self, id: [u8; 32]) {
        host::set_context_id(id);
    }

    /// Returns the current executor identity.
    #[must_use]
    pub fn executor_id(&self) -> [u8; 32] {
        host::executor_id()
    }

    /// Returns the current context identity.
    #[must_use]
    pub fn context_id(&self) -> [u8; 32] {
        host::context_id()
    }
}
