//! A pending sweep must hold the per-context execution lock from its first
//! cascaded apply through the `dag_heads` commit, like an inbound apply does.
//!
//! Each cascaded apply moves the context's `root_hash`. If the lock is let go
//! in between, a reader under it (the hash heartbeat, a sync handshake) sees
//! the new root beside the old heads: a `(heads, root)` pair no node holds,
//! which every peer at those heads counts as a root-hash divergence. The
//! startup load and snapshot-checkpoint sweeps were worse: they never
//! committed the cascaded heads at all, so the pair stayed torn until an
//! unrelated delta arrived.
//!
//! The executor is stubbed (no WASM, as in `delta_store_lock_inversion_test`)
//! and records the lock form each apply asked for: `Lock` means the sweep
//! retained the guard; `None` means the apply took and dropped it on its own.

use std::sync::{Arc, Mutex};

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
use tokio::sync::RwLock;

use crate::delta_store::DeltaStore;
use crate::test_support::{context, delta_store_over_with_manager, GENESIS};

const PARENT: [u8; 32] = [0xD1; 32];
const CHILD: [u8; 32] = [0xD2; 32];

/// Answers every execute and records the lock form it asked for.
struct RecordingContextManager {
    lock: Arc<RwLock<ContextId>>,
    forms: Arc<Mutex<Vec<&'static str>>>,
}

impl Actor for RecordingContextManager {
    type Context = actix::Context<Self>;
}

impl actix::Handler<ContextMessage> for RecordingContextManager {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, _ctx: &mut Self::Context) -> Self::Result {
        let ContextMessage::Execute { request, outcome } = msg else {
            return;
        };
        let lock = Arc::clone(&self.lock);
        let forms = Arc::clone(&self.forms);
        let _handle = actix::spawn(async move {
            let (form, guard) = match request.atomic {
                None => ("none", mint_guard(&lock).await),
                Some(ContextAtomic::Lock) => ("lock", mint_guard(&lock).await),
                Some(ContextAtomic::Held(ContextAtomicKey(held))) => ("held", held),
            };
            forms.lock().expect("forms").push(form);
            let response = ExecuteResponse {
                returns: Ok(None),
                logs: Vec::new(),
                events: Vec::new(),
                root_hash: Hash::from([0x77; 32]),
                artifact: Vec::new(),
                atomic: (form != "none").then_some(ContextAtomicKey(guard)),
                read_only_write_discarded: false,
            };
            let _ = outcome.send(Ok(response));
        });
    }
}

async fn mint_guard(lock: &Arc<RwLock<ContextId>>) -> ContextGuard {
    ContextGuard::write(Arc::clone(lock).write_owned().await)
}

fn delta(id: [u8; 32], parents: Vec<[u8; 32]>) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: Vec::new(),
        hlc: HybridTimestamp::default(),
        kind: DeltaKind::Regular,
    }
}

/// A delta store over a seeded context, with `CHILD` already pending on
/// `PARENT` (stored without reaching the applier).
async fn store_with_pending_child() -> (DeltaStore, Store, Arc<Mutex<Vec<&'static str>>>, impl Sized)
{
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    store
        .handle()
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

    let forms = Arc::new(Mutex::new(Vec::new()));
    let recipient = LazyRecipient::<ContextMessage>::new();
    let init = recipient.clone();
    let recorded = Arc::clone(&forms);
    let _addr = RecordingContextManager::create(move |ctx| {
        assert!(init.init(ctx), "context manager recipient init");
        RecordingContextManager {
            lock: Arc::new(RwLock::new(context())),
            forms: recorded,
        }
    });

    let (delta_store, tmp, rx) = delta_store_over_with_manager(store.clone(), recipient).await;
    let applied = delta_store
        .add_delta(delta(CHILD, vec![PARENT]), None, None, None, None)
        .await
        .expect("adding an orphan delta succeeds");
    assert!(!applied, "CHILD must be pending until PARENT is present");
    assert!(forms.lock().expect("forms").is_empty());
    (delta_store, store, forms, (tmp, rx))
}

fn committed_heads(store: &Store) -> Vec<[u8; 32]> {
    store
        .handle()
        .get(&key::ContextMeta::new(context()))
        .expect("read context meta")
        .expect("context meta")
        .dag_heads
}

#[actix::test]
async fn a_local_delta_cascade_holds_the_lock_through_the_heads_commit() {
    let (delta_store, store, forms, _keep) = store_with_pending_child().await;

    // The local execute already applied PARENT; registering it unblocks CHILD.
    let _events = delta_store
        .add_local_applied_delta(delta(PARENT, vec![GENESIS]))
        .await
        .expect("register local delta");

    assert_eq!(
        *forms.lock().expect("forms"),
        vec!["lock"],
        "the cascaded apply must retain the execution lock for the heads commit"
    );
    assert_eq!(committed_heads(&store), vec![CHILD]);
}

#[actix::test]
async fn a_snapshot_checkpoint_cascade_commits_the_heads_under_the_lock() {
    let (delta_store, store, forms, _keep) = store_with_pending_child().await;

    let added = delta_store
        .add_snapshot_checkpoints(vec![PARENT], [0x55; 32])
        .await;
    assert_eq!(added, 1);

    assert_eq!(
        *forms.lock().expect("forms"),
        vec!["lock"],
        "the cascaded apply must retain the execution lock for the heads commit"
    );
    assert_eq!(
        committed_heads(&store),
        vec![CHILD],
        "the cascaded heads must be committed, not left behind the moved root"
    );
}
