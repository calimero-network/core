//! `add_delta_internal` must not acquire the `dag` lock while it holds the
//! per-context execution lock.
//!
//! Inbound deltas take the locks in the order `dag` write → context. The
//! cascade-commit tail inverts that: it keeps the retained execution guard from
//! the apply, releases the `dag` write lock, and then re-acquires `dag` (read)
//! to clone the cascaded delta bodies — context → `dag`. Two inbound deltas
//! landing in that window wedge the context permanently:
//!
//! - task A holds the context guard and waits for `dag` read,
//! - task C holds `dag` write and waits for the context lock inside its apply.
//!
//! Nothing else can take the `dag` lock again, so the context's root hash is
//! frozen for the process lifetime — the node stops converging and every sync
//! session against it times out.
//!
//! The schedule below is deterministic rather than probabilistic: it runs on a
//! single-threaded runtime and each step waits for an observable the previous
//! step produced, so the two tasks interleave the same way on every run.

use std::sync::Arc;
use std::time::Duration;

use actix::Actor;
use calimero_context_client::messages::{ContextMessage, ExecuteResponse};
use calimero_context_client::{ContextAtomic, ContextAtomicKey, ContextGuard};
use calimero_dag::{CausalDelta, DeltaKind};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_storage::action::Action;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::db::InMemoryDB;
use calimero_store::{key, types, Store};
use calimero_utils_actix::LazyRecipient;
use tokio::sync::{oneshot, RwLock};

use crate::test_support::{context, delta_store_over_with_manager, GENESIS};

/// Applies immediately: genesis is always treated as an applied parent.
const DELTA_A: [u8; 32] = [0xA1; 32];
/// Child of [`DELTA_A`] — pre-added while A is still absent, so it sits pending
/// and is unblocked by A's apply. That cascade is what drives the commit tail's
/// `dag` read-lock re-acquisition; without it the inverted path is skipped.
const DELTA_B: [u8; 32] = [0xB2; 32];
/// The concurrent inbound delta. Also genesis-rooted, so it applies (and so
/// takes the context lock) the moment it gets the `dag` write lock.
const DELTA_C: [u8; 32] = [0xC3; 32];

/// Stands in for `ContextManager` on the execute path.
///
/// It owns the same kind of per-context `RwLock` the real actor hands out and
/// mints guards under the same rules as `handlers::execute`: `None` and `Lock`
/// both acquire, `Held` reuses the caller's guard, and every atomic form hands
/// the guard back so the applier can stash it in its relay slot.
///
/// Standing in for the executor keeps the test off WASM (an applied delta
/// otherwise needs a real installed application) while leaving the locking —
/// the whole subject of this test — real on both sides.
struct StubContextManager {
    /// The per-context execution lock. Cloned into every guard this mints.
    lock: Arc<RwLock<ContextId>>,
    /// Fires when the first execute (delta A's apply) is under way, i.e. once
    /// task A is inside the `dag` write lock.
    entered_tx: Option<oneshot::Sender<()>>,
    /// Holds that first execute open until the test says so, standing in for a
    /// slow WASM apply. Keeping A parked there keeps the `dag` write lock held
    /// while task C queues behind it.
    gate_rx: Option<oneshot::Receiver<()>>,
}

impl Actor for StubContextManager {
    type Context = actix::Context<Self>;
}

impl actix::Handler<ContextMessage> for StubContextManager {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, _ctx: &mut Self::Context) -> Self::Result {
        let ContextMessage::Execute { request, outcome } = msg else {
            // The delta path only ever sends `Execute`; anything else would be
            // a change in what this test covers, so leave it unanswered (the
            // caller's `oneshot` resolves to an error) rather than fake a reply.
            return;
        };

        let lock = Arc::clone(&self.lock);
        let entered = self.entered_tx.take();
        let gate = self.gate_rx.take();

        // Spawned rather than handled inline: the real `ContextManager` answers
        // an execute with `ActorResponse::r#async`, so its mailbox keeps serving
        // while one execute waits on the context lock. Awaiting inline would
        // wedge this stub's mailbox instead, manufacturing a stall that has
        // nothing to do with the lock order under test.
        let _handle = actix::spawn(async move {
            let (guard, is_atomic) = match request.atomic {
                None => (mint_guard(&lock).await, false),
                Some(ContextAtomic::Lock) => (mint_guard(&lock).await, true),
                Some(ContextAtomic::Held(ContextAtomicKey(held))) => (held, true),
            };

            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            if let Some(gate) = gate {
                let _ = gate.await;
            }

            let response = ExecuteResponse {
                returns: Ok(None),
                logs: Vec::new(),
                events: Vec::new(),
                // Any hash works: a mismatch against the delta's expected root
                // is normal under concurrent branches and only gets logged.
                root_hash: Hash::from([0x77; 32]),
                artifact: Vec::new(),
                atomic: is_atomic.then_some(ContextAtomicKey(guard)),
            };

            let _ = outcome.send(Ok(response));
        });
    }
}

/// Acquire the per-context write guard, in the shape the executor returns it.
async fn mint_guard(lock: &Arc<RwLock<ContextId>>) -> ContextGuard {
    ContextGuard::write(Arc::clone(lock).write_owned().await)
}

/// A payload-free delta. An empty payload keeps the applier off the writer-set
/// and anchor-resolution paths (no `Shared` actions to resolve) so the test
/// exercises the lock order and nothing else.
fn delta(id: [u8; 32], parents: Vec<[u8; 32]>) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: Vec::new(),
        hlc: HybridTimestamp::default(),
        expected_root_hash: GENESIS,
        kind: DeltaKind::Regular,
    }
}

/// Seed the context row `apply()` reads before it executes. Without it every
/// apply fails early with "Context not found" and never reaches the executor.
fn seed_context(store: &Store) {
    let mut handle = store.handle();
    handle
        .put(
            &key::ContextMeta::new(context()),
            &types::ContextMeta::new(
                key::ApplicationMeta::new([0x01; 32].into()),
                GENESIS,
                vec![],
                None,
            ),
        )
        .expect("seed context meta");
}

#[actix::test]
async fn cascade_commit_does_not_hold_the_context_lock_across_a_dag_acquisition() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    seed_context(&store);

    let (entered_tx, entered_rx) = oneshot::channel();
    let (gate_tx, gate_rx) = oneshot::channel();

    let recipient = LazyRecipient::<ContextMessage>::new();
    let init = recipient.clone();
    let _addr = StubContextManager::create(move |ctx| {
        assert!(init.init(ctx), "context manager recipient init");
        StubContextManager {
            lock: Arc::new(RwLock::new(context())),
            entered_tx: Some(entered_tx),
            gate_rx: Some(gate_rx),
        }
    });

    let (delta_store, _tmp, _rx) = delta_store_over_with_manager(store, recipient).await;
    let delta_store = Arc::new(delta_store);

    // B arrives before its parent, so it is stored pending without ever
    // reaching the applier. A's apply will unblock it, and that cascade is the
    // precondition for the commit tail's `dag` read.
    let applied_b = delta_store
        .add_delta(delta(DELTA_B, vec![DELTA_A]), None, None, None)
        .await
        .expect("adding an orphan delta succeeds");
    assert!(!applied_b, "B must be pending until A applies");

    // Task A: applies A, cascades B, then runs the commit tail holding the
    // retained context guard. Parked inside its first execute until the gate
    // opens, so it holds the `dag` write lock while C queues up.
    let task_a = actix::spawn({
        let delta_store = Arc::clone(&delta_store);
        async move {
            delta_store
                .add_delta(delta(DELTA_A, vec![GENESIS]), None, None, None)
                .await
        }
    });

    entered_rx.await.expect("A reached the executor");

    // Task C: the concurrent inbound delta. It gets as far as the `dag` write
    // lock and blocks there, behind A.
    let task_c = actix::spawn({
        let delta_store = Arc::clone(&delta_store);
        async move {
            delta_store
                .add_delta(delta(DELTA_C, vec![GENESIS]), None, None, None)
                .await
        }
    });

    // `add_delta_internal` records the delta's expected root hash before it
    // asks for the `dag` write lock, and nothing between those two points
    // awaits on anything contended. So C's id showing up here means C is
    // already parked on the `dag` lock — an observable handoff rather than a
    // sleep long enough to "probably" get there.
    let mut queued = false;
    for _ in 0..10_000 {
        if delta_store.head_root_hash_ids().await.contains(&DELTA_C) {
            queued = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(queued, "C never reached the dag write lock");

    // Let A out of its apply. From here A hands the `dag` write lock to the
    // queued C and walks into its commit tail still holding the context guard.
    gate_tx.send(()).expect("gate receiver alive");

    // With the inversion in place this is where it wedges: A blocks on `dag`
    // read (C holds write), C blocks on the context lock (A holds it), and
    // neither ever completes.
    let applied_a = tokio::time::timeout(Duration::from_secs(10), task_a)
        .await
        .expect(
            "deadlock: the cascade-commit tail took the `dag` read lock while holding the \
             per-context execution lock, and a concurrent inbound delta held `dag` write \
             while waiting for that same context lock",
        )
        .expect("task A did not panic")
        .expect("A applies");
    assert!(applied_a, "A applies over genesis");

    // A's guard is gone now, so C's apply gets the lock and finishes too.
    let applied_c = tokio::time::timeout(Duration::from_secs(10), task_c)
        .await
        .expect("C completes once A releases the context lock")
        .expect("task C did not panic")
        .expect("C applies");
    assert!(applied_c, "C applies over genesis");
}
