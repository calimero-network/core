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
//!
//! `#[app::migrate]` / [`read_raw`](crate::read_raw) **are** supported — see
//! [`TestHost::migrate`] and [`assert_migrate_converges`]. Application state
//! commits to `calimero_storage`'s native mock while `read_raw` reads a separate
//! SDK host map, so the harness mirrors the committed root across after every
//! commit; `read_raw()` (and a migrate body) then observes the live root.

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

    /// Mirrors the committed root `Entry` into the SDK host map so
    /// [`read_raw`](crate::read_raw) observes it (the storage and SDK layers use
    /// separate native mocks). Idempotent; a no-op until something is committed.
    fn __test_mirror_root();

    /// Installs `build()`'s result as the new root the way the WASM
    /// `#[app::migrate]` export does — under storage merge mode, with
    /// deterministic ids — and **without resetting the store first**, so the
    /// pre-migration state survives for the body's [`read_raw`](crate::read_raw).
    fn __test_install_migrated(build: &mut dyn FnMut() -> Self);

    /// The merkle root hash recorded by the most recent commit, or `None` if
    /// nothing has been committed. Folds in every child-collection entry, so
    /// comparing it across two migrate runs detects divergence *inside* carried
    /// or seeded collections — not only in the top-level root struct.
    fn __test_root_hash() -> Option<[u8; 32]>;
}

thread_local! {
    /// `true` while a `TestHost` is alive on this thread. Guards against two
    /// live harnesses sharing (and clobbering) the same thread-local mock state.
    static HARNESS_LIVE: Cell<bool> = const { Cell::new(false) };
}

/// Releases the `HARNESS_LIVE` slot on drop. Used to hold the slot across the
/// fallible construction window in [`TestHost::new`] so a panicking `build()`
/// can't leave it stuck `true`; `mem::forget`-ed on the success path once the
/// returned [`TestHost`]'s own `Drop` takes over.
struct LiveGuard;

impl Drop for LiveGuard {
    fn drop(&mut self) {
        HARNESS_LIVE.with(|live| live.set(false));
    }
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

        // Claim the slot with an RAII guard so a panic during the fallible build
        // below (a panicking `init()` is routine in TDD) clears HARNESS_LIVE on
        // unwind — otherwise the slot stays stuck `true` and the *next* test on
        // this pooled thread fails with a misleading "another TestHost is still
        // alive". Disarmed on the success path; the returned handle's `Drop`
        // takes over ownership of the slot.
        let armed = LiveGuard;

        host::reset();
        S::__test_reset();
        event::register::<S>();

        let mut build = Some(build);
        S::__test_install(&mut || (build.take().expect("state builder called more than once"))());
        // Make the freshly-installed root visible to `read_raw()` (used by
        // `#[app::migrate]` bodies) — it reads a separate SDK host map.
        S::__test_mirror_root();

        core::mem::forget(armed); // success: hand the slot to the returned Self
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
        // Keep `read_raw()` consistent with the post-mutation root (so a later
        // `migrate` sees the up-to-date pre-migration state).
        S::__test_mirror_root();
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
        S::__test_mirror_root();

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

    /// Runs a `#[app::migrate]` function against the installed state and returns
    /// a [`TestHost`] over the migrated state type `V2`.
    ///
    /// Faithfully reproduces a node's migrate path: the pre-migration root is
    /// visible to [`read_raw`](crate::read_raw) inside `migrate_fn`, and the
    /// result is committed under storage merge mode with deterministic ids — so
    /// a determinism bug (or a guarded non-deterministic call such as
    /// `Counter::increment`) surfaces here exactly as it would across nodes.
    ///
    /// Uses the harness's current executor identity; use
    /// [`migrate_as`](TestHost::migrate_as) to pin one.
    pub fn migrate<V2>(self, migrate_fn: impl FnOnce() -> V2) -> TestHost<V2>
    where
        V2: TestState + AppState,
        for<'a> V2::Event<'a>: AppEventExt,
    {
        self.migrate_as(host::executor_id(), migrate_fn)
    }

    /// Like [`migrate`](TestHost::migrate) but pins the executor identity (both
    /// the SDK and storage layers) for the migrate body — use it to run the same
    /// migration "as" different nodes when checking convergence.
    pub fn migrate_as<V2>(self, executor: [u8; 32], migrate_fn: impl FnOnce() -> V2) -> TestHost<V2>
    where
        V2: TestState + AppState,
        for<'a> V2::Event<'a>: AppEventExt,
    {
        // Register V2's event emitter so an `app::emit!` in the migrate body
        // (e.g. a `Migrated` event) resolves the new state's emitter.
        event::register::<V2>();

        let prior = host::executor_id();
        host::set_executor_id(executor);
        let mut build = Some(migrate_fn);
        // Set the storage-layer authorship identity for the body too, so any
        // identity-gated write inside the migrate resolves as it would on a node.
        V2::__test_with_executor(executor, &mut || {
            V2::__test_install_migrated(&mut || (build.take().expect("migrate fn invoked once"))());
        });
        host::set_executor_id(prior);
        V2::__test_mirror_root();

        // Transfer this thread's single live-harness slot from `self` (V1) to
        // the V2 handle without resetting any mock state: forget `self` so its
        // `Drop` doesn't clear `HARNESS_LIVE` out from under the V2 handle.
        core::mem::forget(self);
        TestHost {
            _state: PhantomData,
            _not_send: PhantomData,
        }
    }
}

/// Asserts a migration **converges**: runs `migrate_fn` from an identical,
/// deterministically-installed `install_v1()` state under two different executor
/// identities and checks the two migrated **merkle root hashes** are equal.
///
/// This is the property the whole migration model rests on — every node runs the
/// migrate independently and must land on the same root. The root hash folds in
/// every child-collection entry, so a per-node value baked *anywhere* in the
/// migrated state — `env::executor_id()` written into a field, an
/// `LwwRegister::set` inside a *carried* collection, a Counter increment — makes
/// the hashes differ and fails here, instead of silently forking a real network.
///
/// **In-process limitations** — both "nodes" share one deterministic mock store,
/// so this catches **identity/value** divergence but NOT:
/// - **iteration-order** divergence: the mock sorts child entries by id (element
///   stamps are zeroed in merge mode), so it cannot reproduce two synced nodes
///   holding the same entries in a different *local* order. An unsorted `Vector`
///   seeded from an unordered source would diverge in production yet passes here
///   — cover that with a merobox e2e.
/// - a mutator that **panics** in merge mode (`Counter::increment`/`decrement`,
///   `RGA::insert`, where that guard exists): such a migrate panics rather than
///   reaching the hash comparison.
pub fn assert_migrate_converges<V1, V2>(
    install_v1: impl Fn() -> V1,
    migrate_fn: impl Fn() -> V2 + Copy,
    node_a: [u8; 32],
    node_b: [u8; 32],
) where
    V1: TestState + AppState,
    for<'a> V1::Event<'a>: AppEventExt,
    V2: TestState + AppState,
    for<'a> V2::Event<'a>: AppEventExt,
{
    // This helper drives the mock directly and resets it; refuse to run while a
    // `TestHost` is live on this thread (we'd silently wipe its state), matching
    // `TestHost::new`'s one-live-harness guarantee.
    assert!(
        !HARNESS_LIVE.with(Cell::get),
        "assert_migrate_converges resets the mock store; drop any live TestHost on \
         this thread before calling it."
    );

    fn run<V1, V2>(
        install_v1: &impl Fn() -> V1,
        migrate_fn: impl Fn() -> V2 + Copy,
        node: [u8; 32],
    ) -> [u8; 32]
    where
        V1: TestState + AppState,
        for<'a> V1::Event<'a>: AppEventExt,
        V2: TestState + AppState,
        for<'a> V2::Event<'a>: AppEventExt,
    {
        // Drive the `TestState` bridge directly (no `TestHost` handle, so no
        // live-slot juggling across the V1->V2 type change).
        host::reset();
        V1::__test_reset();
        event::register::<V1>();

        // Install a *deterministic* v1: the builder runs inside merge mode (via
        // `__test_install_migrated`), so `LwwRegister`/`Element` stamps are
        // zeroed and v1 is byte-identical regardless of wall-clock time or
        // identity — modelling the synced, byte-identical pre-migration state
        // every node shares. (A normal install bakes per-run wall-clock stamps,
        // which would make the two runs differ before the migrate even runs.)
        V1::__test_install_migrated(&mut || install_v1());
        V1::__test_mirror_root();

        // Run the migration as `node`. Register V2's event emitter first so an
        // `app::emit!` in the migrate body resolves the new state's emitter
        // (matching `migrate_as` and the real node path).
        event::register::<V2>();
        host::set_executor_id(node);
        let mut mbuild = Some(migrate_fn);
        V2::__test_with_executor(node, &mut || {
            V2::__test_install_migrated(&mut || {
                (mbuild.take().expect("migrate fn invoked once"))()
            });
        });

        // The merkle root folds in child-collection entries, so this catches
        // divergence inside carried/seeded collections, not just the root struct.
        V2::__test_root_hash().expect("migrated root was committed")
    }

    let a = run(&install_v1, migrate_fn, node_a);
    let b = run(&install_v1, migrate_fn, node_b);
    assert_eq!(
        a, b,
        "migration is non-deterministic: nodes {node_a:?} and {node_b:?} produced \
         different v2 root hashes",
    );
}
