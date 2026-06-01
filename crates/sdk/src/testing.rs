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
//!
//! # Limitations
//!
//! The harness covers normal `call`/`view`/event/log/identity flows. It does
//! **not** model:
//! - **Cross-context calls** (`env::xcall`), **networked blob announce/fetch**,
//!   and **`env::ed25519_verify`** — these panic if invoked (no in-process
//!   equivalent); test them with merobox workflows.
//! - **`#[app::migrate]` / [`read_raw`](crate::read_raw)** — committed state
//!   lives in `calimero_storage`'s mock store, while `read_raw` reads the
//!   SDK-level host map, so it won't observe the live root. Migration paths are
//!   out of scope here.

use core::cell::Cell;
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

thread_local! {
    /// `true` while a `TestHost` is alive on this thread. Guards against two
    /// live harnesses sharing (and clobbering) the same thread-local mock state.
    static HARNESS_LIVE: Cell<bool> = const { Cell::new(false) };
}

/// An in-process test host wrapping a single application state instance.
///
/// Construct one with [`TestHost::new`], drive mutations with
/// [`call`](TestHost::call), read with [`view`](TestHost::view), and inspect
/// captured [`events`](TestHost::events) / [`logs`](TestHost::logs).
///
/// # State isolation
///
/// All mock state (storage, events, logs, identity) lives in thread-locals, and
/// [`TestHost::new`] resets every one of them — so each harness starts from a
/// clean slate. Rust's test runner uses a thread *pool* but runs tests
/// sequentially on each thread (one test finishes before that thread starts the
/// next), so the reset-on-construction is what keeps tests independent, not a
/// thread-per-test guarantee. Construct a fresh `TestHost` at the top of each
/// `#[test]`. Only one may be live per thread at a time — since they share that
/// thread's mock state, [`new`](TestHost::new) **panics** if another is still
/// alive rather than silently resetting it out from under you.
///
/// # Default identity
///
/// Until you call [`set_executor`](TestHost::set_executor) /
/// [`set_context`](TestHost::set_context) / [`call_as`](TestHost::call_as), the
/// harness reports fixed well-known executor / context ids. Access-control tests
/// that assert a *rejection* path must set a non-owner identity explicitly —
/// the default will otherwise satisfy owner checks and silently pass.
#[must_use = "a TestHost only does work when you call/view through it"]
pub struct TestHost<S> {
    _state: PhantomData<S>,
    // The harness is backed entirely by thread-locals initialized in `new`, so
    // a `TestHost` is only valid on its creating thread. The raw-pointer marker
    // makes it `!Send + !Sync`, turning "move it to another thread and `call`
    // there" into a compile error instead of a silent read of a different
    // thread's (default) mock host.
    _not_send: PhantomData<*const ()>,
}

impl<S> Drop for TestHost<S> {
    fn drop(&mut self) {
        HARNESS_LIVE.with(|live| live.set(false));
    }
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
    /// `#[app::init]` entrypoint guarantees. If your `#[app::init]` body emits
    /// events or logs, they're captured during construction and visible via
    /// [`events`](TestHost::events) / [`logs`](TestHost::logs) before the first
    /// `call` / `view` — this is intentional (it lets you assert on init).
    pub fn new(build: impl FnOnce() -> S) -> Self {
        HARNESS_LIVE.with(|live| {
            assert!(
                !live.get(),
                "another TestHost is still alive on this thread — only one may be \
                 live at a time (they share this thread's mock state). Drop the \
                 first (let it go out of scope) before constructing the next."
            );
            live.set(true);
        });

        host::reset();
        S::__test_reset();
        event::register::<S>();

        let mut build = Some(build);
        S::__test_install(&mut || (build.take().expect("state builder called more than once"))());

        Self {
            _state: PhantomData,
            _not_send: PhantomData,
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
    /// previous identity — even if the closure panics.
    ///
    /// Sets both the SDK-level [`env::executor_id`](crate::env::executor_id)
    /// (what app logic reads) and the storage-layer authorship identity (what
    /// CRDT element writes record), so multi-author scenarios resolve exactly as
    /// they would across nodes. Both layers switch together for the duration of
    /// the closure and are unwound together: the SDK identity is restored by an
    /// RAII guard nested *inside* the storage layer's own
    /// `with_executor_id` scope, so a panic can't leave the two layers
    /// disagreeing for a later `call` / `view` on the same thread.
    pub fn call_as<R>(&mut self, executor: [u8; 32], f: impl FnOnce(&mut S) -> R) -> R {
        // Restores the SDK-host executor on scope exit (incl. unwind), mirroring
        // the storage layer's RAII restore in `with_executor_id`.
        struct SdkExecutorGuard([u8; 32]);
        impl Drop for SdkExecutorGuard {
            fn drop(&mut self) {
                host::set_executor_id(self.0);
            }
        }

        let mut out = None;
        let mut f = Some(f);
        // `__test_with_executor` switches the storage executor to `executor`
        // for the body below (and restores it on the way out). We switch the
        // SDK executor *inside* that body so both layers are aligned before any
        // user code runs and restored together as the body unwinds.
        S::__test_with_executor(executor, &mut || {
            let _sdk = SdkExecutorGuard(host::executor_id());
            host::set_executor_id(executor);

            let mut inner = f.take();
            S::__test_with_mut(&mut |state| {
                out = Some((inner.take().expect("call_as closure invoked once"))(state));
            });
        });

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
