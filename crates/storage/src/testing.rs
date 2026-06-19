//! CRDT convergence property-test harness for custom `Mergeable` types.
//!
//! [`converge`] lets an app author assert — in a single `#[test]` — that their
//! state type converges under concurrent, reordered operations. It drives N
//! in-memory replicas (each backed by its own store, with its own executor
//! identity), applies the registered operations on every replica in a
//! per-replica randomized order, gossips the resulting `StorageDelta`s between
//! replicas in randomized order, and asserts every replica ends on the **same
//! Merkle root hash**.
//!
//! This is the app-author-facing surface of the runtime CRDT-conformance
//! machinery: "prove your CRDT converges" as a one-liner.
//!
//! # Example
//!
//! ```ignore
//! use calimero_storage::testing::converge_app;
//!
//! #[test]
//! fn team_stats_converge() {
//!     // `converge_app` for an `#[app::state]` type whose methods `app::emit!`;
//!     // `converge::<T>()` / `converge_with` for a plain `Mergeable` type.
//!     converge_app(TeamMetricsApp::init)
//!         .replicas(3)
//!         .ops(|s| { let _ = s.record_win("liverpool".into()); })
//!         .assert_all_replicas_equal();
//! }
//! ```
//!
//! # What it models
//!
//! Each replica starts from an identical genesis snapshot (so ids and the base
//! hash match — mirroring how a real joiner bootstraps from a leader). Every
//! replica then applies the full op list locally under its own executor id, in
//! a shuffled order, and broadcasts one delta per op. Each replica then applies
//! every *other* replica's deltas, also in shuffled order. Convergence =
//! identical root hash across all replicas, regardless of interleaving.
//!
//! # What this actually proves
//!
//! It proves your **app state converges end-to-end** — the property whose
//! absence caused the production split-brains this harness exists to prevent.
//! It is worth understanding *which* code path does the reconciling, because it
//! is usually **not** your hand-written `Mergeable::merge`:
//!
//! - **Collection-backed fields** (`UnorderedMap`, `UnorderedSet`, `Counter`,
//!   …) converge via the storage layer's per-**child-entity** CRDT merge during
//!   delta apply. A custom root `merge` that delegates to `field.merge(..)` runs
//!   on *empty shells*: `merge_root_state` deserializes the root entity, where
//!   collection fields are bare handles with no loaded entries — so those calls
//!   see no data and have no effect.
//! - **Pure inline scalar fields** are reconciled by the root entity's HLC
//!   last-writer-wins; the custom `merge` is bypassed entirely.
//!
//! So this harness is best understood as "prove my app state converges under
//! concurrent, reordered ops", not "unit-test my custom merge function". The
//! former is what matters for correctness and what production sync relies on.
//!
//! # Limitations
//!
//! - **`Shared` / `Authored` / `User` / `Frozen` storage** need the node's
//!   signing identity (delta apply verifies signatures), which this bare
//!   harness does not provide — test those with merobox workflows.
//! - Convergence is asserted via root-hash equality, not by reading values
//!   (the harness is generic over `T` and can't name your accessors). A
//!   matching root hash proves the full Merkle state converged.
//! - A run mutates process-global registries, so it takes an internal lock for
//!   its whole duration — the harness **self-serializes**. `#[serial]` is
//!   therefore not required for correctness (only a minor speed-up by avoiding
//!   lock contention); concurrent `converge` calls run one at a time safely.
//! - Invariants ([`Converge::invariant`]) are checked on the **final** converged
//!   state of each replica (after all deltas), not after each individual delta.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

use borsh::{BorshDeserialize, BorshSerialize};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;

use crate::collections::{Mergeable, Root};
use crate::env::{self, RuntimeEnv};
use crate::interface::ApplyContext;
use crate::register_crdt_merge_for_test;
use crate::store::Key;

/// Fixed context id shared by all replicas (they're the same context).
const CONTEXT_ID: [u8; 32] = [7u8; 32];

/// Default RNG seed, so a green run is reproducible and a failure is printable.
const DEFAULT_SEED: u64 = 0xC0FFEE;

/// Executor identity for the genesis (leader) install. Distinct from every
/// replica's id ([1..=n]) so replica 0's local writes are genuinely concurrent
/// with genesis from the storage layer's view, not a continuation of it.
const GENESIS_EXECUTOR: [u8; 32] = [0xEEu8; 32];

/// Serializes harness runs across threads. A run mutates process-global state
/// (the merge registry — cleared and repopulated below — and the rekey
/// registry) plus thread-locals, so two concurrent runs would corrupt each
/// other. Holding this for the whole run makes the harness self-serializing:
/// a caller who forgets `#[serial]` gets correct (just slower) behaviour, not a
/// silent race. Poison is recovered (a panicking run leaves no invariant
/// broken — the next run resets all state anyway).
static HARNESS_LOCK: Mutex<()> = Mutex::new(());

/// An in-memory main-storage backend owned by a single replica.
type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

/// A registered operation: a mutation applied to a loaded replica state.
type Op<T> = Box<dyn Fn(&mut T)>;

/// Named value-level invariants checked on each converged replica.
type InvariantList<T> = Vec<(String, Box<dyn Fn(&T) -> bool>)>;

fn new_store() -> Store {
    Rc::new(RefCell::new(HashMap::new()))
}

/// Build a [`RuntimeEnv`] routing all `MainStorage` I/O into `store`, under the
/// given executor identity. Mirrors the runtime's wiring in
/// `crates/runtime/src/logic/host_functions/system.rs`.
fn env_for(store: &Store, executor: [u8; 32]) -> RuntimeEnv {
    let r = Rc::clone(store);
    let reader = Rc::new(move |key: &Key| r.borrow().get(&key.to_bytes()).cloned());
    let w = Rc::clone(store);
    let writer = Rc::new(move |key: Key, value: &[u8]| {
        w.borrow_mut()
            .insert(key.to_bytes(), value.to_vec())
            .is_some()
    });
    let rm = Rc::clone(store);
    let remover = Rc::new(move |key: &Key| rm.borrow_mut().remove(&key.to_bytes()).is_some());
    RuntimeEnv::new(reader, writer, remover, CONTEXT_ID, executor)
}

/// Executor identity for replica `r`. Distinct, non-zero, and deterministic so
/// concurrent writes from different replicas genuinely diverge before merge.
fn executor_for(r: usize) -> [u8; 32] {
    let mut id = [0u8; 32];
    id[0] = (r as u8).wrapping_add(1);
    id
}

/// Builder for a CRDT convergence assertion. Construct with [`converge`].
#[must_use = "a convergence builder does nothing until `assert_all_replicas_equal` is called"]
pub struct Converge<T> {
    replicas: usize,
    seed: u64,
    build: Box<dyn Fn() -> T>,
    ops: Vec<Op<T>>,
    // One-time host setup run after the env reset (e.g. registering the SDK
    // event emitter for `#[app::state]` types whose methods `app::emit!`).
    // `None` for plain `Mergeable` types that don't touch the SDK host.
    host_setup: Option<Box<dyn Fn()>>,
    // Value-level invariants checked on each converged replica. Hash equality
    // alone is NOT correctness: deterministic LWW converges every replica to the
    // SAME wrong value, so a data-loss bug passes the hash check. Invariants let
    // a test assert the merged *value* is right.
    invariants: InvariantList<T>,
}

/// Start a CRDT convergence assertion for state type `T`, using [`Default`] as
/// the genesis state.
///
/// For an `#[app::state]` type whose constructor is `#[app::init]` (not
/// `Default`), use [`converge_with`] and pass that constructor.
pub fn converge<T>() -> Converge<T>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + 'static,
{
    converge_with(T::default)
}

/// Start a CRDT convergence assertion for state type `T`, using `build` as the
/// genesis constructor — mirror your `#[app::init]` here.
///
/// ```ignore
/// converge_with(TeamMetricsApp::init)
///     .replicas(3)
///     .ops(|r| r.record_win("liverpool".into()))
///     .assert_all_replicas_equal();
/// ```
pub fn converge_with<T>(build: impl Fn() -> T + 'static) -> Converge<T>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + 'static,
{
    Converge {
        replicas: 3,
        seed: DEFAULT_SEED,
        build: Box::new(build),
        ops: Vec::new(),
        host_setup: None,
        invariants: Vec::new(),
    }
}

/// Start a convergence assertion for a full `#[app::state]` application type
/// whose methods emit events via `app::emit!`.
///
/// Identical to [`converge_with`] but also registers the SDK event emitter
/// (otherwise `app::emit!` panics with "uninitialized event emitter"). Use this
/// when your ops call methods that emit; use [`converge`] / [`converge_with`]
/// for plain `Mergeable` types that only touch storage.
///
/// ```ignore
/// converge_app(TeamMetricsApp::init)
///     .replicas(3)
///     .ops(|s| { let _ = s.record_win("liverpool".into()); })
///     .assert_all_replicas_equal();
/// ```
pub fn converge_app<T>(build: impl Fn() -> T + 'static) -> Converge<T>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + calimero_sdk::state::AppState + 'static,
    for<'a> <T as calimero_sdk::state::AppState>::Event<'a>: calimero_sdk::event::AppEventExt,
{
    Converge {
        replicas: 3,
        seed: DEFAULT_SEED,
        build: Box::new(build),
        ops: Vec::new(),
        host_setup: Some(Box::new(|| calimero_sdk::event::register::<T>())),
        invariants: Vec::new(),
    }
}

impl<T> Converge<T>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + 'static,
{
    /// Number of replicas to simulate (default 3). Panics if `< 1`.
    pub fn replicas(mut self, n: usize) -> Self {
        assert!(n >= 1, "converge: need at least one replica");
        self.replicas = n;
        self
    }

    /// Seed the RNG that shuffles op + delta application order, so a failure is
    /// reproducible (the seed is printed on mismatch). Default: a fixed value.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Override the genesis builder (default is `T::default`). Mirror your
    /// `#[app::init]` here if it does more than default-construct collections.
    pub fn init(mut self, build: impl Fn() -> T + 'static) -> Self {
        self.build = Box::new(build);
        self
    }

    /// Register one operation. **Every** replica applies **every** registered op
    /// locally (under its own executor id, in a per-replica shuffled order) and
    /// gossips the resulting delta to the others — this is the commutativity
    /// model, not a partition of ops across replicas. Chain `.ops(..)` to build
    /// up the interleaving.
    ///
    /// Consequence for invariants: an op that appears `k` times in the list,
    /// applied by `n` replicas, runs `n × k` times in total. So 3 replicas + one
    /// `record_win` ⇒ 3 wins; add a second identical op ⇒ 6.
    pub fn ops(mut self, op: impl Fn(&mut T) + 'static) -> Self {
        self.ops.push(Box::new(op));
        self
    }

    /// Assert a value-level invariant on every converged replica.
    ///
    /// Hash equality proves convergence but **not** correctness: deterministic
    /// LWW converges all replicas to the same *wrong* value, so a data-loss bug
    /// silently passes the hash check. Use this to assert the merged value is
    /// what a correct CRDT merge would produce — e.g. that a counter summed all
    /// replicas' increments rather than dropping all but one.
    ///
    /// ```ignore
    /// .invariant("liverpool wins == replicas", |s| s.get_wins("liverpool".into()).unwrap() == 3)
    /// ```
    pub fn invariant(mut self, desc: &str, check: impl Fn(&T) -> bool + 'static) -> Self {
        self.invariants.push((desc.to_owned(), Box::new(check)));
        self
    }

    /// Run the simulation and assert every replica converges to the same root
    /// hash. Panics (failing the test) on divergence, printing the seed and the
    /// per-replica hashes so the interleaving can be reproduced.
    pub fn assert_all_replicas_equal(self) {
        let n = self.replicas;

        // Hold the harness lock for the whole run: it mutates process-global
        // registries + thread-locals, so concurrent runs would corrupt each
        // other. This makes the harness self-serializing (see `HARNESS_LOCK`),
        // so `#[serial]` is no longer load-bearing — just slower if omitted.
        let _run_guard = HARNESS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        env::reset_environment();
        // App-state types whose ops `app::emit!` need the SDK event emitter
        // registered first, or emission panics. No-op for plain Mergeable types.
        if let Some(setup) = &self.host_setup {
            setup();
        }
        // The merge registry is process-global under the `testing` feature and
        // its root-merge dispatch is type-blind (it tries every registered fn).
        // Clear it so a prior test's merge can't be picked for our `T`'s bytes,
        // then register only `T`. Safe under concurrency: `HARNESS_LOCK` above
        // serializes the clear+register+run against any other harness run.
        crate::merge::clear_merge_registry();
        register_crdt_merge_for_test::<T>();

        // Genesis: install the base state once, then snapshot it byte-for-byte
        // into every replica so all replicas share identical ids + base hash.
        let genesis: Store = new_store();
        env::with_runtime_env(env_for(&genesis, GENESIS_EXECUTOR), || {
            Root::new(|| (self.build)()).commit();
        });
        let base = genesis.borrow().clone();

        let stores: Vec<Store> = (0..n)
            .map(|_| Rc::new(RefCell::new(base.clone())))
            .collect();

        // Local apply: EVERY replica applies the FULL op list locally (under its
        // own executor), each in its own shuffled order, capturing one delta per
        // op. This is the commutativity model — N replicas each running the same
        // ops in different orders — not a partition of ops across replicas. So a
        // single `record_win` op with 3 replicas yields 3 total wins after
        // gossip (each replica contributes one); the expected value is
        // `replicas × (times that op appears in the list)`.
        let mut deltas: Vec<Vec<Vec<u8>>> = Vec::with_capacity(n);
        for (r, store) in stores.iter().enumerate() {
            let mut order: Vec<usize> = (0..self.ops.len()).collect();
            // Per-replica seed: mix the base seed with the replica index times an
            // odd Fibonacci-hashing constant (2^64/φ) for good bit diffusion, so
            // replicas shuffle differently yet reproducibly from `seed`.
            order.shuffle(&mut StdRng::seed_from_u64(
                self.seed ^ (r as u64).wrapping_mul(0x9E37_79B9),
            ));

            let mut replica_deltas = Vec::with_capacity(order.len());
            env::with_runtime_env(env_for(store, executor_for(r)), || {
                for &op_idx in &order {
                    let mut app = Root::<T>::fetch().expect("converge: genesis not installed");
                    (self.ops[op_idx])(&mut app);
                    app.commit();
                    if let Some(artifact) = env::take_last_artifact() {
                        replica_deltas.push(artifact);
                    }
                }
            });
            deltas.push(replica_deltas);
        }

        // Gossip: every replica applies every *other* replica's deltas, in a
        // shuffled order, then we record its converged root hash.
        let mut hashes: Vec<Option<[u8; 32]>> = Vec::with_capacity(n);
        for (r, store) in stores.iter().enumerate() {
            let mut foreign: Vec<(usize, usize)> = (0..n)
                .filter(|&s| s != r)
                .flat_map(|s| (0..deltas[s].len()).map(move |k| (s, k)))
                .collect();
            foreign.shuffle(&mut StdRng::seed_from_u64(
                self.seed ^ 0xDEAD_BEEF ^ (r as u64).wrapping_mul(0x85EB_CA77),
            ));

            let failed = env::with_runtime_env(env_for(store, executor_for(r)), || {
                for (s, k) in foreign {
                    Root::<T>::sync(&deltas[s][k], &ApplyContext::empty())
                        .expect("converge: delta apply failed");
                }
                // Check value-level invariants while we're in this replica's env.
                let app = Root::<T>::fetch().expect("converge: state vanished after sync");
                self.invariants
                    .iter()
                    .filter(|(_, check)| !check(&app))
                    .map(|(desc, _)| desc.clone())
                    .collect::<Vec<_>>()
            });
            hashes.push(env::root_hash());
            assert!(
                failed.is_empty(),
                "converge: replica {r} violated invariant(s) (seed = {:#x}): {}\n\
                 (note: all replicas may have *converged* to this wrong value — \
                 hash equality does not imply a correct merge)",
                self.seed,
                failed.join("; "),
            );
        }

        // All replicas must agree.
        if let Err(report) = check_converged(self.seed, &hashes) {
            panic!("{report}");
        }
    }
}

/// Compares the per-replica root hashes and returns a divergence report if they
/// don't all match. Extracted from [`Converge::assert_all_replicas_equal`] so
/// the detection logic itself is unit-testable without having to induce a real
/// split-brain (the storage layer reconciles almost everything, so a genuine
/// divergence is hard to construct from app code).
fn check_converged(seed: u64, hashes: &[Option<[u8; 32]>]) -> Result<(), String> {
    let reference = hashes.first().copied().flatten();
    if hashes.iter().all(|h| *h == reference) {
        return Ok(());
    }
    let detail = hashes
        .iter()
        .enumerate()
        .map(|(r, h)| {
            format!(
                "  replica {r}: {}",
                h.map(hex::encode).unwrap_or_else(|| "<none>".into())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "converge: replicas DIVERGED (seed = {seed:#x}).\n{detail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::check_converged;

    #[test]
    fn reports_divergence_when_hashes_differ() {
        let hashes = vec![Some([1u8; 32]), Some([2u8; 32]), Some([1u8; 32])];
        let report = check_converged(0xABC, &hashes).expect_err("must flag divergence");
        assert!(report.contains("DIVERGED"));
        assert!(report.contains("0xabc"), "report names the seed for repro");
    }

    #[test]
    fn accepts_identical_hashes() {
        let hashes = vec![Some([7u8; 32]); 4];
        assert!(check_converged(0, &hashes).is_ok());
    }

    #[test]
    fn divergence_includes_missing_hash() {
        let hashes = vec![Some([1u8; 32]), None];
        assert!(check_converged(1, &hashes).is_err());
    }
}
