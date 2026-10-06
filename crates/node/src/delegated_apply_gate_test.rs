//! A delegated delta's warrant is checked, and its nonce spent, when the delta
//! APPLIES — on every path that applies one — and exactly once.
//!
//! A delta reaches `ContextStorageApplier::apply` three ways: as the primary of
//! an add, as a pending child an add unblocks (a cascade), and as a persisted
//! parent `get_missing_parents` loads into the DAG. The gate used to run only at
//! arrival, on the add paths, and spent the nonce there even when the delta went
//! pending. So a delta refused at arrival but left as a persisted row applied
//! unchecked when loaded as a parent, and a pending delta's warrant was spent
//! before it applied: after a restart the re-driven delta was refused as a
//! replay of itself.
//!
//! The executor is a stub that records which delta ids it applied, so these
//! tests reach the real `DeltaStore` paths without WASM, and the governance rows
//! are real, so the real warrant gate decides.

use std::sync::{Arc, Mutex};

use actix::Actor;
use calimero_account::{AccountId, Delegation, Warrant, WarrantTerms};
use calimero_context_client::messages::{ContextMessage, ExecuteResponse};
use calimero_context_client::{ContextAtomic, ContextAtomicKey, ContextGuard};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_dag::{CausalDelta, DeltaKind};
use calimero_governance_store::test_fixtures::{
    enrol_member, real_join_account, sample_meta_with_admin, test_store,
};
use calimero_governance_store::{
    AdmissionCut, CapabilitiesRepository, MembershipRepository, MetaRepository,
};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::action::Action;
use calimero_storage::delta::StorageDelta;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::{key, types, Store};
use calimero_utils_actix::LazyRecipient;
use tokio::sync::RwLock;

use crate::delta_store::DeltaStore;
use crate::test_support::{context, delta_store_over_with_manager, KeepAlive, GENESIS};

const GROUP: [u8; 32] = [0xC0; 32];
const AUTHOR_KEY: [u8; 32] = [0x0A; 32];
const RELAY_KEY: [u8; 32] = [0x0B; 32];

/// The parent a delta can be held back on.
const PARENT: [u8; 32] = [0x50; 32];
/// The delegated delta that should apply.
const ORIGINAL: [u8; 32] = [0x01; 32];
/// A different delta presenting the same warrant (so the same nonce).
const SECOND: [u8; 32] = [0x02; 32];
/// A child naming [`SECOND`] as its parent, which makes it a missing parent.
const CHILD: [u8; 32] = [0x03; 32];

type Applied = Arc<Mutex<Vec<[u8; 32]>>>;

/// Stands in for `ContextManager`: records each applied delta's id from the
/// `CausalActions` artifact and answers success, minting guards as the real
/// executor does.
struct RecordingExecutor {
    lock: Arc<RwLock<ContextId>>,
    applied: Applied,
}

impl Actor for RecordingExecutor {
    type Context = actix::Context<Self>;
}

impl actix::Handler<ContextMessage> for RecordingExecutor {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, _ctx: &mut Self::Context) -> Self::Result {
        let ContextMessage::Execute { request, outcome } = msg else {
            return;
        };
        if let Ok(StorageDelta::CausalActions { delta_id, .. }) =
            borsh::from_slice::<StorageDelta>(&request.payload)
        {
            self.applied.lock().expect("applied").push(delta_id);
        }
        let lock = Arc::clone(&self.lock);
        let _handle = actix::spawn(async move {
            let (guard, is_atomic) = match request.atomic {
                None => (
                    ContextGuard::write(Arc::clone(&lock).write_owned().await),
                    false,
                ),
                Some(ContextAtomic::Lock) => (
                    ContextGuard::write(Arc::clone(&lock).write_owned().await),
                    true,
                ),
                Some(ContextAtomic::Held(ContextAtomicKey(held))) => (held, true),
            };
            let _ = outcome.send(Ok(ExecuteResponse {
                returns: Ok(None),
                logs: Vec::new(),
                events: Vec::new(),
                root_hash: Hash::from([0x77; 32]),
                artifact: Vec::new(),
                atomic: is_atomic.then_some(ContextAtomicKey(guard)),
                read_only_write_discarded: false,
            }));
        });
    }
}

/// A group holding the context, the author as a member and the relay holding
/// `CAN_AUTHOR_ON_BEHALF`, and one warrant from the author to the relay.
fn seed() -> (Store, Delegation) {
    let store = test_store();
    let group = ContextGroupId::from(GROUP);
    MetaRepository::new(&store)
        .save(&group, &sample_meta_with_admin(AccountId::from([0xEE; 32])))
        .expect("save meta");
    calimero_governance_store::register_context_in_group(&store, &group, &context())
        .expect("register context");
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

    let author_pk = PublicKey::from(AUTHOR_KEY);
    let relay_pk = PublicKey::from(RELAY_KEY);
    let author = enrol_member(&store, &group, &author_pk);
    let relay = enrol_member(&store, &group, &relay_pk);
    let membership = MembershipRepository::new(&store);
    membership
        .add_member(&group, &author, GroupMemberRole::Member)
        .expect("add the author");
    membership
        .add_member(&group, &relay, GroupMemberRole::Member)
        .expect("add the relay");
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &group,
            &relay,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        )
        .expect("grant authorship");

    let warrant = Warrant::sign(
        &PrivateKey::from(AUTHOR_KEY),
        WarrantTerms {
            context: context(),
            author_account: author,
            executor: relay,
            executor_key: relay_pk,
            release_bytecode_id: [0u8; 32],
            release_version: String::new(),
            method: "send_message".to_owned(),
            intent_hash: Warrant::intent_hash("send_message", b"{}"),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 7,
            not_after: u64::MAX,
        },
    )
    .expect("warrant must sign");
    let delegation = Delegation {
        warrant: Box::new(warrant),
        author_proof: real_join_account(&author_pk),
        executor_proof: real_join_account(&relay_pk),
        executor_key: relay_pk,
    };
    (store, delegation)
}

/// A `DeltaStore` over `store` with a recording executor in front of it.
async fn delta_store(store: Store) -> (DeltaStore, Applied, tempfile::TempDir, KeepAlive) {
    let applied: Applied = Arc::default();
    let recipient = LazyRecipient::<ContextMessage>::new();
    let init = recipient.clone();
    let recorded = Arc::clone(&applied);
    let _addr = RecordingExecutor::create(move |ctx| {
        assert!(init.init(ctx), "context manager recipient init");
        RecordingExecutor {
            lock: Arc::new(RwLock::new(context())),
            applied: recorded,
        }
    });
    let (delta_store, tmp, keep) = delta_store_over_with_manager(store, recipient).await;
    (delta_store, applied, tmp, keep)
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

fn author() -> Option<PublicKey> {
    Some(PublicKey::from(AUTHOR_KEY))
}

fn applied(log: &Applied) -> Vec<[u8; 32]> {
    log.lock().expect("applied").clone()
}

/// Whether the warrant's nonce is still unspent: the gate admits it live.
fn nonce_unspent(store: &Store, delegation: &Delegation) -> bool {
    calimero_governance_store::warrant_gate::check_delegated_delta(
        store,
        &context(),
        delegation,
        AdmissionCut::live(),
    )
    .is_ok()
}

/// The gap: a second delta under a spent warrant, refused when it arrived but
/// left behind as a persisted row (an event-carrying delta is persisted before
/// it is checked), applied without any check once a child named it as a
/// missing parent.
#[actix::test]
async fn a_second_delta_under_a_spent_warrant_is_refused_when_loaded_as_a_persisted_parent() {
    let (store, delegation) = seed();
    let (ds, log, _tmp, _keep) = delta_store(store.clone()).await;

    assert!(ds
        .add_delta(
            delta(ORIGINAL, vec![GENESIS]),
            author(),
            None,
            None,
            Some(delegation.clone())
        )
        .await
        .expect("the original applies"));
    assert!(
        !nonce_unspent(&store, &delegation),
        "applying spent the nonce"
    );

    let _refused = ds
        .add_delta_with_events(
            delta(SECOND, vec![GENESIS]),
            Some(vec![1]),
            author(),
            None,
            None,
            Some(delegation.clone()),
        )
        .await;
    assert!(
        store
            .handle()
            .has(&key::ContextDagDelta::new(context(), SECOND))
            .expect("read"),
        "precondition: the refused delta left its persisted row"
    );

    let _pending = ds
        .add_delta(delta(CHILD, vec![SECOND]), None, None, None, None)
        .await
        .expect("the child goes pending on its missing parent");
    let _ = ds.get_missing_parents().await;

    assert!(
        !ds.dag_has_delta_applied(&SECOND).await,
        "a second delta reusing a spent nonce must not apply as a persisted parent"
    );
    assert_eq!(applied(&log), vec![ORIGINAL], "only the original applied");
}

/// The same pair through a cascade: both held back on one parent, so neither
/// is applied when it arrives. When the parent lands, the first to apply spends
/// the nonce and the other is refused.
#[actix::test]
async fn a_second_delta_under_the_same_warrant_is_refused_when_cascaded() {
    let (store, delegation) = seed();
    let (ds, log, _tmp, _keep) = delta_store(store.clone()).await;

    assert!(!ds
        .add_delta(
            delta(ORIGINAL, vec![PARENT]),
            author(),
            None,
            None,
            Some(delegation.clone())
        )
        .await
        .expect("the original goes pending"));
    // Before the fix this one was refused here, at arrival; now it is held
    // back like the original and judged when it would apply. Either way it
    // must never apply.
    let _second = ds
        .add_delta(
            delta(SECOND, vec![PARENT]),
            author(),
            None,
            None,
            Some(delegation.clone()),
        )
        .await;

    // The refused child surfaces as an error from the add that cascaded it, as
    // any cascaded refusal does; the parent and the original applied.
    let _cascaded = ds
        .add_delta(delta(PARENT, vec![GENESIS]), None, None, None, None)
        .await;

    assert!(
        ds.dag_has_delta_applied(&PARENT).await,
        "the parent applies"
    );
    assert!(
        ds.dag_has_delta_applied(&ORIGINAL).await,
        "the original cascades"
    );
    assert!(
        !ds.dag_has_delta_applied(&SECOND).await,
        "a second delta reusing the nonce must not apply"
    );
    assert_eq!(applied(&log), vec![PARENT, ORIGINAL]);
    assert!(!nonce_unspent(&store, &delegation));
}

/// A delta held back on a missing parent has not applied, so its warrant is not
/// spent; after a restart, re-driven from its persisted row, it applies once
/// its parent arrives. Before the fix the nonce was spent on arrival, and the
/// re-driven delta was refused as a replay of itself.
#[actix::test]
async fn a_pending_delegated_delta_spends_its_nonce_only_when_it_applies_even_across_a_restart() {
    let (store, delegation) = seed();
    {
        let (ds, log, _tmp, _keep) = delta_store(store.clone()).await;
        let pending = ds
            .add_delta_with_events(
                delta(ORIGINAL, vec![PARENT]),
                Some(vec![1]),
                author(),
                None,
                None,
                Some(delegation.clone()),
            )
            .await
            .expect("held back on its parent");
        assert!(!pending.applied);
        assert!(applied(&log).is_empty());
        assert!(
            nonce_unspent(&store, &delegation),
            "a pending delta has not applied, so its nonce is not spent"
        );
    }

    // Restart: a fresh store over the same rows.
    let (ds, log, _tmp, _keep) = delta_store(store.clone()).await;
    let _loaded = ds.load_persisted_deltas().await.expect("load");
    assert!(ds
        .add_delta(delta(PARENT, vec![GENESIS]), None, None, None, None)
        .await
        .expect("the parent applies"));

    assert!(
        ds.dag_has_delta_applied(&ORIGINAL).await,
        "the re-driven delta applies once its parent lands"
    );
    assert_eq!(applied(&log), vec![PARENT, ORIGINAL]);
    assert!(
        !nonce_unspent(&store, &delegation),
        "and spends its nonce then"
    );
}

/// Re-delivery of a delta that already applied is a duplicate, not a replay:
/// it is neither refused nor applied again.
#[actix::test]
async fn a_redelivered_delegated_delta_is_a_duplicate_not_a_replay() {
    let (store, delegation) = seed();
    let (ds, log, _tmp, _keep) = delta_store(store).await;
    let add = || {
        ds.add_delta(
            delta(ORIGINAL, vec![GENESIS]),
            author(),
            None,
            None,
            Some(delegation.clone()),
        )
    };
    assert!(add().await.expect("applies"));
    assert!(!add().await.expect("a re-delivery is not refused"));
    assert_eq!(applied(&log), vec![ORIGINAL], "applied once");
}
