//! CRDT convergence property-test harness for custom `Mergeable` types.
//!
//! [`converge`] lets an app author assert — in a single `#[test]` — that their
//! state type converges under concurrent, reordered operations. It drives N
//! in-memory replicas (each backed by its own store, with its own executor
//! identity), applies the registered operations on every replica in a
//! per-replica randomized order, gossips the resulting `StorageDelta`s between
//! replicas in a randomized causal order, and asserts every replica ends on the **same
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
//! every *other* replica's deltas, in a shuffled interleaving of the authors
//! that keeps each author's deltas in the order it wrote them, which is every
//! order a DAG can deliver them in: a delta applies only after its parents.
//! Convergence =
//! identical root hash across all replicas, regardless of interleaving.
//!
//! "Its own executor id" holds at **both** layers an app can observe: the
//! storage `RuntimeEnv` and the SDK host are set together for each replica's
//! scope, so `calimero_sdk::env::device_id()` — what app code actually calls —
//! varies per replica, and `account_id()` follows
//! [`one_account`](Converge::one_account). This matters for any rule that
//! branches on the writer: with only the storage half set, such a rule reads one
//! device on every replica and quietly degenerates into a rule that does not
//! branch at all, which reads as a broken rule rather than a harness that never
//! varied its input.
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
//! - **`Shared` / `Authored` / `User` / `Frozen` storage** is covered: each
//!   replica holds a real ed25519 device key, and the harness signs every
//!   captured delta the way `calimero-context` does before a peer applies it.
//!   What it still does NOT model is the causal cut — `effective_writers` is
//!   always `None`, so writer sets resolve from settled local state rather than
//!   from the governance fold at the delta's parents. A test about rotation
//!   ORDERING still belongs in merobox.
//!
//!   Until 2026-09 this was not covered at all, and the way it failed is worth
//!   knowing: an unverifiable action is *dropped* by the sync merge, not raised,
//!   so replicas silently exchanged nothing and each kept its own local write —
//!   values individually correct, roots different. That is indistinguishable
//!   from a CRDT bug by inspection, and was reported as one (core#3965). The
//!   harness now fails on a dropped action instead; see
//!   [`allow_dropped_actions`](Converge::allow_dropped_actions).
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
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Mutex;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use calimero_sdk::testing::with_identity;
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;

use crate::action::Action;
use crate::collections::{dropped_action_count, reset_dropped_action_count, Mergeable, Root};
use crate::delta::StorageDelta;
use crate::env::{self, RuntimeEnv};
use crate::interface::ApplyContext;
use crate::register_crdt_merge_for_test;
use crate::store::Key;

/// Fixed context id shared by all replicas (they're the same context).
const CONTEXT_ID: [u8; 32] = [7u8; 32];

/// Default RNG seed, so a green run is reproducible and a failure is printable.
const DEFAULT_SEED: u64 = 0xC0FFEE;

/// Executor identity for the genesis (leader) install. Distinct from every
/// replica's, so replica 0's local writes are genuinely concurrent with genesis
/// from the storage layer's view, not a continuation of it.
///
/// A real verifying key, for the same reason [`device_key_for`] is: the genesis
/// install writes signed storage too, and a made-up id cannot verify.
fn genesis_executor() -> [u8; 32] {
    genesis_key().verifying_key().to_bytes()
}

/// Serializes harness runs across threads. A run mutates process-global state
/// (the merge registry — cleared and repopulated below — and the rekey
/// registry) plus thread-locals, so two concurrent runs would corrupt each
/// other. Holding this for the whole run makes the harness self-serializing:
/// a caller who forgets `#[serial]` gets correct (just slower) behaviour, not a
/// silent race. Poison is recovered (a panicking run leaves no invariant
/// broken — the next run resets all state anyway).
static HARNESS_LOCK: Mutex<()> = Mutex::new(());

/// An in-memory main-storage backend owned by a single replica.
type Store = Rc<RefCell<HashMap<[u8; crate::store::KEY_LEN], Vec<u8>>>>;

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
///
/// `executor` is the DEVICE — the replica id, which is what convergence tests
/// vary. Each replica also gets a distinct ACCOUNT (see [`account_for`]), never
/// equal to its device: a harness where the two matched would let an
/// account-keyed gate and a device-keyed one behave identically, and the
/// difference between those is the thing worth testing.
fn env_for(store: &Store, executor: [u8; 32], account: [u8; 32]) -> RuntimeEnv {
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
    RuntimeEnv::new(reader, writer, remover, CONTEXT_ID, executor, account)
}

/// The account replica `r` writes as. Distinct from its device id
/// ([`executor_for`]) so the two can never be confused, and distinct per replica
/// so each is its own principal.
fn account_for(r: usize) -> [u8; 32] {
    let mut id = [0u8; 32];
    id[0] = (r as u8).wrapping_add(1);
    id[1] = 0xAC; // marks this as an account, and keeps it != the device id
    id
}

/// The single account every replica writes as under
/// [`one_account`](Converge::one_account) — one person holding N devices. Shaped
/// like [`account_for`]'s ids (and so distinct from every device id) but tied to
/// no replica index.
const SHARED_ACCOUNT: [u8; 32] = {
    let mut id = [0u8; 32];
    id[0] = 0xA1;
    id[1] = 0xAC;
    id
};

/// The ed25519 device key replica `r` signs its writes with.
///
/// A real keypair rather than a made-up 32 bytes, and that is load-bearing
/// rather than tidy. `Shared`, `SharedMember`, `User` and `Authored` writes ship
/// a signature the receiver verifies against the key the action names
/// (`Interface::apply_action`'s `resolve_signer`), so a device id that is not
/// a verifying key cannot verify — and a rejected action is *dropped*, not
/// raised (`apply_child_action_lenient`). The harness therefore used to
/// exchange deltas that every replica silently refused, leaving each holding its
/// own local write: values individually correct, roots different. That is
/// core#3965.
///
/// Seeded off the replica index so a failure is reproducible, and distinct per
/// replica so per-writer CRDT state (counter slots, HLC seeds, LWW tiebreaks)
/// stays genuinely per device.
fn device_key_for(r: usize) -> SigningKey {
    let mut seed = [0u8; 32];
    seed[0] = (r as u8).wrapping_add(1);
    // Domain-separates replica keys from the genesis key below, so a replica
    // index that wraps to the genesis byte still gets its own key.
    seed[31] = 0xD5;
    SigningKey::from_bytes(&seed)
}

/// Executor identity for replica `r`: the public half of [`device_key_for`].
/// Distinct, non-zero and deterministic, so concurrent writes from different
/// replicas genuinely diverge before merge.
fn executor_for(r: usize) -> [u8; 32] {
    device_key_for(r).verifying_key().to_bytes()
}

/// The genesis installer's signing key. Its own seed, so it collides with no
/// replica's.
fn genesis_key() -> SigningKey {
    let mut seed = [0u8; 32];
    seed[0] = 0xEE;
    seed[31] = 0x6E;
    SigningKey::from_bytes(&seed)
}

/// Sign every action in a captured delta that still carries the `[0; 64]`
/// placeholder, exactly as a node's `sign_authorized_actions` does.
///
/// The storage layer stamps a placeholder for a write it has already authorized
/// locally and leaves the signing to the layer that holds the key — which in
/// production is `calimero-context` and here is the harness. The nonce is
/// stamped BEFORE the payload is computed because `payload_for_signing` commits
/// to it; signing first and stamping after signs a payload no receiver can
/// reconstruct.
///
/// A delta that does not decode, or does not re-encode, is passed through
/// untouched rather than panicking: it carries no signed action to fix, and the
/// apply path is what should report a malformed one.
fn sign_delta_actions(artifact: &[u8], key: &SigningKey) -> Vec<u8> {
    use crate::entities::StorageType;

    let Ok(mut delta) = borsh::from_slice::<StorageDelta>(artifact) else {
        return artifact.to_vec();
    };

    let actions: &mut Vec<Action> = match &mut delta {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };

    for action in actions.iter_mut() {
        let (metadata, nonce) = match action {
            Action::Add { metadata, .. } | Action::Update { metadata, .. } => {
                let nonce = *metadata.updated_at;
                (metadata, nonce)
            }
            Action::DeleteRef {
                metadata,
                deleted_at,
                ..
            } => {
                let nonce = *deleted_at;
                (metadata, nonce)
            }
        };

        let should_sign = match &mut metadata.storage_type {
            StorageType::User {
                signature_data: Some(sig_data),
                ..
            }
            | StorageType::Shared {
                signature_data: Some(sig_data),
                ..
            }
            | StorageType::SharedMember {
                signature_data: Some(sig_data),
                ..
            } => {
                let placeholder = sig_data.signature == [0; 64];
                if placeholder {
                    sig_data.nonce = nonce;
                }
                placeholder
            }
            _ => false,
        };
        if !should_sign {
            continue;
        }

        let signature = key.sign(&action.payload_for_signing()).to_bytes();
        let metadata = match action {
            Action::Add { metadata, .. }
            | Action::Update { metadata, .. }
            | Action::DeleteRef { metadata, .. } => metadata,
        };
        match &mut metadata.storage_type {
            StorageType::User {
                signature_data: Some(sig_data),
                ..
            }
            | StorageType::Shared {
                signature_data: Some(sig_data),
                ..
            }
            | StorageType::SharedMember {
                signature_data: Some(sig_data),
                ..
            } => sig_data.signature = signature,
            // `should_sign` matched one of the three arms above.
            _ => {}
        }
    }

    borsh::to_vec(&delta).unwrap_or_else(|_| artifact.to_vec())
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
    // The ONE account every replica writes as (distinct devices, one
    // principal), instead of one account each. See `one_account` and
    // `tee_authority`.
    shared_account: Option<[u8; 32]>,
    // Whether a refused incoming action is acceptable for this run. Default
    // false: a drop makes the run assert nothing. See `allow_dropped_actions`.
    allow_dropped_actions: bool,
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
        shared_account: None,
        allow_dropped_actions: false,
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
        shared_account: None,
        allow_dropped_actions: false,
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

    /// Model **one person on N devices**: every replica keeps its own device id,
    /// but they all write as the same account.
    ///
    /// The default is one account per replica — N unrelated people — which is
    /// the right model for most convergence questions. Reach for this one to test
    /// what the account plane added: a grant or an owner stamp covers every
    /// device its holder owns, while per-writer CRDT state (counter slots, HLC
    /// seeds) must stay per device. Those two pull in opposite directions, and a
    /// harness where each replica is its own account cannot exercise the tension
    /// at all.
    pub fn one_account(mut self) -> Self {
        self.shared_account = Some(SHARED_ACCOUNT);
        self
    }

    /// Every replica writes as [`AccountId::TEE_AUTHORITY`]: N TEE authorities
    /// of one namespace, each a device the node resolves to that account.
    ///
    /// The only way to exercise two TEEs writing `TeeOnly` state concurrently,
    /// which the failover of a TEE trigger can produce.
    pub fn tee_authority(mut self) -> Self {
        self.shared_account = Some(*AccountId::TEE_AUTHORITY.as_bytes());
        self
    }

    /// Permit this run to DROP incoming actions instead of applying them.
    ///
    /// Only for a run whose point is that a write gets refused — an unauthorized
    /// writer, a forged signature, a revoked device. Everywhere else a drop means
    /// the replicas never exchanged anything, so the assertion is vacuous and any
    /// divergence it reports is the refusal rather than a merge bug; the harness
    /// fails on it by default for that reason.
    pub fn allow_dropped_actions(mut self) -> Self {
        self.allow_dropped_actions = true;
        self
    }

    /// The account replica `r` writes as, honouring [`one_account`](Self::one_account).
    fn account_of(&self, r: usize) -> [u8; 32] {
        self.shared_account.unwrap_or_else(|| account_for(r))
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
        let genesis_account = self.account_of(usize::MAX);
        let genesis_device = genesis_executor();
        with_identity(genesis_device, genesis_account, || {
            env::with_runtime_env(env_for(&genesis, genesis_device, genesis_account), || {
                Root::new(|| (self.build)()).commit();
            });
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
            with_identity(executor_for(r), self.account_of(r), || {
                env::with_runtime_env(env_for(store, executor_for(r), self.account_of(r)), || {
                    for &op_idx in &order {
                        let mut app = Root::<T>::fetch().expect("converge: genesis not installed");
                        (self.ops[op_idx])(&mut app);
                        app.commit();
                        if let Some(artifact) = env::take_last_artifact() {
                            // Sign before the delta leaves its author, exactly
                            // where a node signs: storage stamps a placeholder
                            // for a write it authorized locally and leaves the
                            // key to the layer above it.
                            replica_deltas.push(sign_delta_actions(&artifact, &device_key_for(r)));
                        }
                    }
                });
            });
            deltas.push(replica_deltas);
        }

        // Gossip: every replica applies every *other* replica's deltas, in a
        // shuffled causal order, then we record its converged root hash.
        let mut hashes: Vec<Option<[u8; 32]>> = Vec::with_capacity(n);
        for (r, store) in stores.iter().enumerate() {
            // A random interleaving of the authors, each author's deltas in
            // the order it wrote them: a DAG applies a delta only after its
            // parents, and each of an author's deltas follows its previous one.
            let mut authors: Vec<usize> = (0..n)
                .filter(|&s| s != r)
                .flat_map(|s| core::iter::repeat_n(s, deltas[s].len()))
                .collect();
            authors.shuffle(&mut StdRng::seed_from_u64(
                self.seed ^ 0xDEAD_BEEF ^ (r as u64).wrapping_mul(0x85EB_CA77),
            ));
            let mut next = vec![0; n];
            let foreign: Vec<(usize, usize)> = authors
                .into_iter()
                .map(|s| {
                    let k = next[s];
                    next[s] += 1;
                    (s, k)
                })
                .collect();

            reset_dropped_action_count();
            let failed = with_identity(executor_for(r), self.account_of(r), || {
                env::with_runtime_env(env_for(store, executor_for(r), self.account_of(r)), || {
                    for (s, k) in foreign {
                        // The account the delta's SIGNER speaks for — the
                        // author's, not this replica's. Storage authorizes
                        // accounts and authenticates keys and resolves neither,
                        // so the caller owes it the bridge; `ApplyContext::empty()`
                        // supplies none, and every signed action is then refused.
                        let ctx = ApplyContext {
                            signer_account: Some(AccountId::from(self.account_of(s))),
                            ..ApplyContext::empty()
                        };
                        Root::<T>::sync(&deltas[s][k], &ctx).expect("converge: delta apply failed");
                    }
                    // Check value-level invariants while we're in this replica's env.
                    let app = Root::<T>::fetch().expect("converge: state vanished after sync");
                    self.invariants
                        .iter()
                        .filter(|(_, check)| !check(&app))
                        .map(|(desc, _)| desc.clone())
                        .collect::<Vec<_>>()
                })
            });
            hashes.push(env::root_hash());

            // A dropped action is a silent no-op that LOOKS like divergence.
            // `apply_child_action_lenient` refuses an unverifiable, unauthorized
            // or stale action and continues the batch — right for a production
            // merge, fatal for a test, because the replica then keeps only its
            // own local write. Every value-level invariant still passes (the
            // value is individually correct) and only the roots differ, which
            // reads as a CRDT bug and is not one. core#3965 was filed that way.
            // Fail here instead, where the cause is nameable.
            let dropped = dropped_action_count();
            assert!(
                dropped == 0 || self.allow_dropped_actions,
                "converge: replica {r} DROPPED {dropped} incoming action(s) instead of \
                 applying them (seed = {:#x}).\n\
                 The delta was refused as unverifiable, unauthorized or stale, so this \
                 replica kept only its own local write. Any root-hash difference that \
                 follows is that refusal, NOT a merge bug.\n\
                 Usual causes: the writing identity is not in the writer set / is not \
                 the entry's owner; or a test built its own `ApplyContext` without a \
                 `signer_account`. If a run is SUPPOSED to refuse writes, say so with \
                 `.allow_dropped_actions()`.",
                self.seed,
            );

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

// ============================================================================
// Scripted runs
// ============================================================================

/// A run whose replicas play DIFFERENT roles and see different subsets of each
/// other's writes, then every delivery order of all of them.
///
/// [`converge`] has every replica apply every op as itself, which cannot say
/// "a member claims, one TEE sees that claim and another TEE sees a different
/// one, then each decides". `Script` can: each replica is a member, the genesis
/// account on another device, or a TEE authority; [`run`](Script::run) makes
/// one local write and returns its delta; [`deliver`](Script::deliver) hands a
/// replica exactly the deltas the test chooses, in any order. Then
/// [`assert_every_order_converges`](Script::assert_every_order_converges)
/// replays EVERY order a DAG could deliver all the deltas in, each on a fresh
/// replica from genesis, and asserts one root hash and the invariant for all
/// of them. A delta's causal past is what its author had written or been
/// delivered when it wrote it; everything concurrent is tried both ways.
///
/// Deltas are signed as a node signs them, and a replay that drops any fails,
/// as in [`converge`]; [`deliver`](Script::deliver) returns the count, so a
/// test about a refused write can say so. Holds the harness lock for its
/// lifetime.
pub struct Script<T> {
    genesis: HashMap<[u8; crate::store::KEY_LEN], Vec<u8>>,
    replicas: Vec<ScriptReplica>,
    deltas: Vec<ScriptDelta>,
    _state: core::marker::PhantomData<T>,
    _run_guard: std::sync::MutexGuard<'static, ()>,
}

struct ScriptReplica {
    store: Store,
    account: [u8; 32],
    /// The deltas it wrote or was delivered.
    seen: BTreeSet<usize>,
}

struct ScriptDelta {
    author: usize,
    bytes: Vec<u8>,
    /// What its author had seen when it wrote it.
    past: BTreeSet<usize>,
}

/// The account genesis is installed as, which [`Script::founder`] writes as.
const SCRIPT_FOUNDER: [u8; 32] = {
    let mut id = [0u8; 32];
    id[0] = 0xF0;
    id[1] = 0xAC;
    id
};

/// The device every replay observes from, which wrote nothing.
const SCRIPT_OBSERVER: usize = 0xFE;

/// Replays at most this many orders, so a script too large to enumerate fails
/// loudly instead of running for hours.
const MAX_SCRIPT_ORDERS: usize = 50_000;

impl<T> Script<T>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + 'static,
{
    /// Installs `build` as genesis, written by the founder's account.
    pub fn new(build: impl FnOnce() -> T) -> Self {
        let run_guard = HARNESS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::reset_environment();
        crate::merge::clear_merge_registry();
        register_crdt_merge_for_test::<T>();

        let genesis = new_store();
        let device = genesis_executor();
        with_identity(device, SCRIPT_FOUNDER, || {
            env::with_runtime_env(env_for(&genesis, device, SCRIPT_FOUNDER), || {
                Root::new(build).commit();
            });
        });
        let genesis = genesis.borrow().clone();
        Self {
            genesis,
            replicas: Vec::new(),
            deltas: Vec::new(),
            _state: core::marker::PhantomData,
            _run_guard: run_guard,
        }
    }

    fn join(&mut self, account: [u8; 32]) -> usize {
        self.replicas.push(ScriptReplica {
            store: Rc::new(RefCell::new(self.genesis.clone())),
            account,
            seen: BTreeSet::new(),
        });
        self.replicas.len() - 1
    }

    /// A replica that writes as its own account.
    pub fn member(&mut self) -> usize {
        let at = self.replicas.len();
        self.join(account_for(at))
    }

    /// A replica that writes as the account that installed genesis, on a
    /// device of its own.
    pub fn founder(&mut self) -> usize {
        self.join(SCRIPT_FOUNDER)
    }

    /// A replica that writes as [`AccountId::TEE_AUTHORITY`], on a device of
    /// its own.
    pub fn tee(&mut self) -> usize {
        self.join(*AccountId::TEE_AUTHORITY.as_bytes())
    }

    /// The account `replica` writes as.
    #[must_use]
    pub fn account(&self, replica: usize) -> AccountId {
        AccountId::from(self.replicas[replica].account)
    }

    fn within<R>(
        &self,
        device: usize,
        store: &Store,
        account: [u8; 32],
        f: impl FnOnce() -> R,
    ) -> R {
        let executor = executor_for(device);
        with_identity(executor, account, || {
            env::with_runtime_env(env_for(store, executor, account), f)
        })
    }

    /// Runs `op` on `replica` and returns the index of the delta it wrote, if
    /// it wrote anything.
    pub fn run(&mut self, replica: usize, op: impl FnOnce(&mut T)) -> Option<usize> {
        let ScriptReplica { store, account, .. } = &self.replicas[replica];
        let bytes = self.within(replica, store, *account, || {
            let mut state = Root::<T>::fetch().expect("script: genesis not installed");
            op(&mut state);
            state.commit();
            env::take_last_artifact().map(|artifact| {
                let signed = sign_delta_actions(&artifact, &device_key_for(replica));
                persist_signatures(&signed);
                signed
            })
        })?;
        let past = self.replicas[replica].seen.clone();
        self.deltas.push(ScriptDelta {
            author: replica,
            bytes,
            past,
        });
        let at = self.deltas.len() - 1;
        let _new = self.replicas[replica].seen.insert(at);
        Some(at)
    }

    /// Applies delta `delta` on `replica`; returns how many of its actions
    /// were dropped.
    pub fn deliver(&mut self, replica: usize, delta: usize) -> u64 {
        let ScriptReplica { store, account, .. } = &self.replicas[replica];
        let dropped = self.apply(replica, store, *account, &self.deltas[delta]);
        let _new = self.replicas[replica].seen.insert(delta);
        dropped
    }

    fn apply(&self, device: usize, store: &Store, account: [u8; 32], delta: &ScriptDelta) -> u64 {
        let ctx = ApplyContext {
            signer_account: Some(AccountId::from(self.replicas[delta.author].account)),
            ..ApplyContext::empty()
        };
        self.within(device, store, account, || {
            reset_dropped_action_count();
            Root::<T>::sync(&delta.bytes, &ctx).expect("script: delta apply failed");
            dropped_action_count()
        })
    }

    /// Pushes every entity `from` holds to `to`, as a HashComparison repair
    /// does, and returns how many `to` refused.
    ///
    /// Each entity goes as the node's repair applies a pushed leaf
    /// (`apply_leaf_with_crdt_merge_as` in `calimero-node`): its stored bytes
    /// and stamp, created under the ancestor chain `from` holds it at, or
    /// merged into the one `to` already holds, as the account its signer
    /// writes for. That is a different path from a delta: no action a writer
    /// signed names the ancestors, which arrive as `from` stores them. The
    /// context root and the app's root entry are skipped, as the node defers
    /// those to the app's own merge.
    pub fn push(&self, from: usize, to: usize) -> u64 {
        let ScriptReplica { store, account, .. } = &self.replicas[from];
        let leaves = self.within(from, store, *account, stored_entities);
        let ScriptReplica { store, account, .. } = &self.replicas[to];
        self.within(to, store, *account, || {
            let mut refused = 0;
            for leaf in leaves {
                let signer_account = signer_of(&leaf.metadata.storage_type)
                    .and_then(|signer| self.account_of_device(signer));
                let Some(action) = push_action(leaf) else {
                    continue;
                };
                let ctx = ApplyContext {
                    signer_account,
                    ..ApplyContext::empty()
                };
                if crate::interface::MainInterface::apply_action(action, &ctx).is_err() {
                    refused += 1;
                }
            }
            refused
        })
    }

    /// The account the device `signer` writes for, if a replica or genesis
    /// signs with it.
    fn account_of_device(&self, signer: [u8; 32]) -> Option<AccountId> {
        if signer == genesis_executor() {
            return Some(AccountId::from(SCRIPT_FOUNDER));
        }
        (0..self.replicas.len())
            .find(|&replica| executor_for(replica) == signer)
            .map(|replica| AccountId::from(self.replicas[replica].account))
    }

    /// Reads `replica`'s state.
    pub fn view<R>(&self, replica: usize, f: impl FnOnce(&T) -> R) -> R {
        let ScriptReplica { store, account, .. } = &self.replicas[replica];
        self.within(replica, store, *account, || {
            f(&Root::<T>::fetch().expect("script: genesis not installed"))
        })
    }

    /// Replays every order of every delta that applies each after its causal
    /// past, each on a fresh replica from genesis. Asserts that nothing is
    /// dropped, that `invariant` holds on every result, and that every result
    /// has one root hash. Returns how many orders it replayed.
    ///
    /// The observer reads as the founder, so a caller-relative read (a
    /// status) is the same in every replay.
    ///
    /// # Panics
    /// On a dropped action, a failed invariant, a divergent root, or more than
    /// `MAX_SCRIPT_ORDERS` orders.
    pub fn assert_every_order_converges(&self, invariant: impl Fn(&T) -> bool) -> usize {
        let orders = self.orders();
        let mut roots = Vec::with_capacity(orders.len());
        for order in &orders {
            let store = Rc::new(RefCell::new(self.genesis.clone()));
            for &delta in order {
                let dropped =
                    self.apply(SCRIPT_OBSERVER, &store, SCRIPT_FOUNDER, &self.deltas[delta]);
                assert_eq!(dropped, 0, "script: order {order:?} dropped delta {delta}");
            }
            let (holds, root) = self.within(SCRIPT_OBSERVER, &store, SCRIPT_FOUNDER, || {
                let state = Root::<T>::fetch().expect("script: state vanished");
                (invariant(&state), env::root_hash())
            });
            assert!(holds, "script: the invariant fails after order {order:?}");
            roots.push((order.clone(), root));
        }
        let (first_order, first_root) = &roots[0];
        for (order, root) in &roots {
            assert_eq!(
                root, first_root,
                "script: order {order:?} and order {first_order:?} DIVERGED"
            );
        }
        roots.len()
    }

    /// Every order of the deltas that keeps each after its causal past.
    fn orders(&self) -> Vec<Vec<usize>> {
        let mut orders = Vec::new();
        let mut order = Vec::with_capacity(self.deltas.len());
        let mut placed = BTreeSet::new();
        self.extend(&mut placed, &mut order, &mut orders);
        assert!(!orders.is_empty(), "script: no deltas to replay");
        orders
    }

    fn extend(
        &self,
        placed: &mut BTreeSet<usize>,
        order: &mut Vec<usize>,
        orders: &mut Vec<Vec<usize>>,
    ) {
        if order.len() == self.deltas.len() {
            assert!(
                orders.len() < MAX_SCRIPT_ORDERS,
                "script: more than {MAX_SCRIPT_ORDERS} delivery orders; write a smaller script"
            );
            orders.push(order.clone());
            return;
        }
        for (at, delta) in self.deltas.iter().enumerate() {
            if placed.contains(&at) || !delta.past.is_subset(placed) {
                continue;
            }
            let _new = placed.insert(at);
            order.push(at);
            self.extend(placed, order, orders);
            let _last = order.pop();
            let _was = placed.remove(&at);
        }
    }
}

/// Writes the signatures in a signed delta back to the author's own index, as a
/// node's `persist_signed_signatures` does, so what the author later pushes in a
/// repair verifies on its peers. Runs in the author's runtime env.
fn persist_signatures(signed: &[u8]) {
    use crate::entities::StorageType;

    let Ok(delta) = borsh::from_slice::<StorageDelta>(signed) else {
        return;
    };
    let actions = match &delta {
        StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. } => actions,
    };
    for action in actions {
        let (Action::Add { id, metadata, .. }
        | Action::Update { id, metadata, .. }
        | Action::DeleteRef { id, metadata, .. }) = action;
        let signed = matches!(
            &metadata.storage_type,
            StorageType::Shared { signature_data: Some(sig), .. }
            | StorageType::User { signature_data: Some(sig), .. }
            | StorageType::SharedMember { signature_data: Some(sig), .. }
                if sig.signature != [0; 64]
        );
        if signed {
            let persisted = crate::interface::MainInterface::update_signature_in_place(
                *id,
                metadata.storage_type.clone(),
            );
            // A delete's tombstone signature is best-effort on a node too.
            if !matches!(action, Action::DeleteRef { .. }) {
                let _stored = persisted.expect("script: persisting a signature failed");
            }
        }
    }
}

/// An entity as a repair ships it: id, bytes, stamp and ancestor chain.
struct PushedEntity {
    id: crate::address::Id,
    data: Vec<u8>,
    metadata: crate::entities::Metadata,
    ancestors: Vec<crate::entities::ChildInfo>,
}

/// Every entity in the current store with bytes, parents first, but the
/// context root and the app's root entry.
fn stored_entities() -> Vec<PushedEntity> {
    use crate::address::Id;
    use crate::index::Index;
    use crate::store::MainStorage;

    let mut out = Vec::new();
    let mut pending = vec![Id::root()];
    while let Some(parent) = pending.pop() {
        for child in <Index<MainStorage>>::get_children_of(parent).unwrap_or_default() {
            let id = child.id();
            pending.push(id);
            if crate::collections::is_app_root_entry(id) {
                continue;
            }
            let Some(data) = crate::interface::MainInterface::find_by_id_raw(id) else {
                continue;
            };
            let Ok(Some(index)) = <Index<MainStorage>>::get_index(id) else {
                continue;
            };
            // What the wire carries of the stamp: no field name, and no schema
            // version, which the receiver stamps itself.
            let mut metadata = crate::entities::Metadata::default();
            metadata.created_at = index.metadata.created_at;
            metadata.updated_at = index.metadata.updated_at;
            metadata.storage_type = index.metadata.storage_type.clone();
            metadata.crdt_type = index.metadata.crdt_type.clone();
            out.push(PushedEntity {
                id,
                data,
                metadata,
                ancestors: <Index<MainStorage>>::get_ancestors_of(id).unwrap_or_default(),
            });
        }
    }
    out
}

/// The device that signed `stamp`, if it is signed.
fn signer_of(stamp: &crate::entities::StorageType) -> Option<[u8; 32]> {
    use crate::entities::StorageType;
    match stamp {
        StorageType::Shared { signature_data, .. }
        | StorageType::User { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => signature_data
            .as_ref()
            .and_then(|sig| sig.signer)
            .map(|signer| *signer.digest()),
        StorageType::Public | StorageType::Frozen => None,
    }
}

/// The action a node's repair applies for `leaf` against the current store:
/// a merge into the entity it holds, or its creation under the pushed
/// ancestors. `None` for a `Frozen` entity already held, which never changes.
fn push_action(leaf: PushedEntity) -> Option<Action> {
    use crate::entities::StorageType;
    use crate::index::Index;
    use crate::store::MainStorage;

    let PushedEntity {
        id,
        data,
        mut metadata,
        ancestors,
    } = leaf;
    let existing = <Index<MainStorage>>::get_index(id).ok().flatten();
    // A `Public` or `Frozen` stamp carries no authorization on the wire, so the
    // receiver keeps the one it stores.
    if matches!(
        metadata.storage_type,
        StorageType::Public | StorageType::Frozen
    ) {
        if let Some(existing) = &existing {
            metadata.storage_type = existing.metadata.storage_type.clone();
        }
    }
    match existing {
        Some(_) if matches!(metadata.storage_type, StorageType::Frozen) => None,
        Some(_) => Some(Action::Update {
            id,
            data,
            ancestors: Vec::new(),
            metadata,
        }),
        None => Some(Action::Add {
            id,
            data,
            ancestors,
            metadata,
        }),
    }
}

// ============================================================================
// Merge-law conformance
// ============================================================================

/// Assert that `T`'s `Mergeable::merge` obeys the laws it is required to.
///
/// [`converge`] answers "do replicas agree?"; this answers "is the rule they
/// agree by actually a merge?". Those are different questions, and the second
/// has been documented on [`Mergeable`](crate::collections::Mergeable),
/// `#[app::mergeable]` and the reference app's README while being checked
/// nowhere.
///
/// The gap is not theoretical. A merge that ran in only one direction shipped
/// in this crate: an incoming write older than the stored one was
/// short-circuited before dispatch, so whichever replica wrote second merged
/// and the other silently kept its own value. Concurrent bids of 900 and 100
/// under a "highest wins" rule settled on 900 on one node and 100 on the other.
/// It was found by a two-replica end-to-end test that happened to look, not by
/// anything asserting commutativity.
///
/// Checked over every ordered pair and triple of `samples`, comparing borsh
/// encodings rather than `PartialEq` — the stored bytes are what the Merkle
/// hash sees, so two values that compare equal but encode differently are still
/// a divergence.
///
/// # Choosing samples
///
/// Include values that DIFFER in the fields the rule reads. Identical samples
/// satisfy every law trivially and prove nothing — the same way a `Counter`
/// test cannot tell you whether a custom rule ran.
///
/// # Not for types that embed collections
///
/// A `Counter`, `UnorderedMap` or any other collection field is a HANDLE:
/// storage identity, not value. Built outside a storage env each one gets a
/// random id, so two merges of the same pair encode differently no matter what
/// the rule does, and this helper would report a commutativity violation that
/// is entirely an artifact of the handles.
///
/// That is not a coverage gap. Collection fields converge structurally — as
/// their own child entities, under the storage layer's own rules — whatever
/// `merge` does with them. The part of a custom rule that can actually be wrong
/// is the part deciding PLAIN data, which is also the only reason to reach for
/// `#[app::mergeable]` at all. Point this at that part; use
/// [`converge`] for the whole type.
///
/// # Panics
///
/// On the first violated law, naming which one and the two encodings.
///
/// ```ignore
/// assert_merge_laws(&[
///     Stats { badges: 0b001, ..Default::default() },
///     Stats { badges: 0b010, ..Default::default() },
///     Stats { badges: 0b100, ..Default::default() },
/// ]);
/// ```
pub fn assert_merge_laws<T>(samples: &[T])
where
    T: crate::collections::Mergeable + BorshSerialize + BorshDeserialize,
{
    assert!(
        samples.len() >= 2,
        "assert_merge_laws needs at least two DIFFERENT samples; one value \
         satisfies every law trivially and proves nothing"
    );

    let enc = |v: &T| borsh::to_vec(v).expect("sample must serialize");

    // Copies come from a borsh round-trip rather than `Clone`, and not only to
    // avoid the bound — an app type holding a `Counter` or a collection cannot
    // be `Clone`, which would have made this helper unusable on exactly the
    // types it is for. It is also the more faithful copy: the laws are about
    // the value as STORED, and stored is what borsh produces.
    let copy = |v: &T| T::try_from_slice(&enc(v)).expect("sample must round-trip through borsh");

    // Merge mode is what production uses, and it suppresses timestamp
    // generation. Without it a rule that touches a nested CRDT stamps a fresh
    // wall clock on each call, so even a correct merge encodes differently
    // every time and every law below fails for the wrong reason.
    let merged = |a: &T, b: &T| -> Vec<u8> {
        let mut out = copy(a);
        crate::env::with_merge_mode(|| out.merge(b)).expect(
            "merge must be TOTAL: returning Err refuses to converge, leaving the entity \
                     divergent while repair retries it forever. Reject bad input on the write \
                     path instead.",
        );
        enc(&out)
    };

    for (i, a) in samples.iter().enumerate() {
        // Idempotent: merging a value with itself must not move it.
        assert_eq!(
            merged(a, a),
            enc(a),
            "merge is not IDEMPOTENT for sample {i}: merge(a, a) != a. Re-delivery of the \
             same state is normal in sync, so a rule that drifts on it never settles."
        );

        for (j, b) in samples.iter().enumerate() {
            // Commutative: the answer cannot depend on which side arrived first.
            assert_eq!(
                merged(a, b),
                merged(b, a),
                "merge is not COMMUTATIVE for samples {i} and {j}: merge(a, b) != merge(b, a). \
                 Two replicas seeing the same pair in different orders will settle on different \
                 values and never converge."
            );

            for (k, c) in samples.iter().enumerate() {
                // Associative: grouping cannot matter either, since replicas
                // batch arrivals differently.
                let left = {
                    let mut ab = copy(a);
                    crate::env::with_merge_mode(|| ab.merge(b)).expect("merge must be total");
                    merged(&ab, c)
                };
                let right = {
                    let mut bc = copy(b);
                    crate::env::with_merge_mode(|| bc.merge(c)).expect("merge must be total");
                    merged(a, &bc)
                };
                assert_eq!(
                    left, right,
                    "merge is not ASSOCIATIVE for samples {i}, {j}, {k}: \
                     merge(merge(a, b), c) != merge(a, merge(b, c)). Replicas batch incoming \
                     state differently, so grouping must not change the result."
                );
            }
        }
    }
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
