//! A peer's delta is judged against the writers of its shared cells at the governance position
//! its author signed, through the real `DeltaStore`, the real governance fold and the real
//! storage apply. Only the executor is a stand-in: it records what the store hands it and applies
//! the artifact natively, resolving a cell's host writers at the position it was given as the
//! execute path does.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use actix::Actor;
use calimero_account::AccountId;
use calimero_context::test_support::RotationWorld;
use calimero_context_client::messages::{
    ContextMessage, ExecuteError, ExecuteRequest, ExecuteResponse, InternalErrorKind,
};
use calimero_context_config::types::GovernanceParentEdge;
use calimero_dag::{CausalDelta, DeltaKind};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::delta::StorageDelta;
use calimero_storage::entities::{full_mask, ChildInfo};
use calimero_storage::env::with_runtime_env;
use calimero_storage::interface::{disable_nonce_check_for_testing, ApplyContext, Interface};
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_storage::shared_writers::{CellWriters, Writers};
use calimero_storage::store::MainStorage;
use calimero_storage::tests::common::{
    build_signed_shared_action, cell_at, env_resolving, pubkey_of, setup_root_for_main,
};
use calimero_store::{key, types};
use calimero_utils_actix::LazyRecipient;
use ed25519_dalek::SigningKey;

use crate::delta_store::{BatchDeltaInput, DeltaStore};
use crate::test_support::{context, delta_store_over_governance, GENESIS};

/// What the stand-in executor was handed for one delta.
#[derive(Clone, Debug)]
struct Seen {
    delta_id: [u8; 32],
    position: Option<GovernanceParentEdge>,
    effective_writers: std::collections::BTreeMap<Id, Writers>,
    signer_account: Option<AccountId>,
    /// What storage refused the delta's actions with, if it did.
    refusal: Option<String>,
}

type HostWriters = Arc<dyn Fn(Id, &[[u8; 32]]) -> Option<CellWriters> + Send + Sync>;

struct Executor {
    seen: Arc<Mutex<Vec<Seen>>>,
    host: HostWriters,
}

impl Actor for Executor {
    type Context = actix::Context<Self>;
}

impl Executor {
    fn run(&self, request: ExecuteRequest) -> Result<ExecuteResponse, ExecuteError> {
        let storage_delta: StorageDelta =
            borsh::from_slice(&request.payload).expect("the applier ships a storage delta");
        let StorageDelta::CausalActions {
            actions,
            delta_id,
            effective_writers,
            signer_account,
            ..
        } = storage_delta
        else {
            panic!("a peer's delta is shipped as causal actions");
        };
        self.seen.lock().unwrap().push(Seen {
            delta_id,
            position: request.governance_position.clone(),
            effective_writers: effective_writers.clone(),
            signer_account,
            refusal: None,
        });

        let heads = request
            .governance_position
            .map(|position| position.governance_dag_heads);

        let host = Arc::clone(&self.host);
        let env = env_resolving(move |cell| match &heads {
            Some(heads) => host(cell, heads),
            None => Some(CellWriters::Genesis),
        });
        let applied = with_runtime_env(env, || {
            actions.iter().try_for_each(|action| {
                Interface::<MainStorage>::apply_action(
                    action.clone(),
                    &ApplyContext {
                        effective_writers: effective_writers.get(&action.id()).cloned(),
                        signer_account,
                    },
                )
                .map(drop)
            })
        });

        if let Err(refusal) = &applied {
            if let Some(seen) = self.seen.lock().unwrap().last_mut() {
                seen.refusal = Some(format!("{refusal:?}"));
            }
        }
        applied.map_err(|_| ExecuteError::InternalError {
            kind: InternalErrorKind::Ipc,
        })?;
        Ok(ExecuteResponse {
            returns: Ok(None),
            logs: Vec::new(),
            events: Vec::new(),
            root_hash: Hash::from([0x77; 32]),
            artifact: Vec::new(),
            atomic: None,
            read_only_write_discarded: false,
        })
    }
}

impl actix::Handler<ContextMessage> for Executor {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, _ctx: &mut Self::Context) {
        if let ContextMessage::Execute { request, outcome } = msg {
            let _ = outcome.send(self.run(request));
        }
    }
}

/// Three members who share one cell, and a node judging their deltas.
struct Scene {
    world: Arc<RotationWorld>,
    store: DeltaStore,
    seen: Arc<Mutex<Vec<Seen>>>,
    alice: SigningKey,
    bob: SigningKey,
    carol: SigningKey,
    cell: Id,
    /// The writers the cell id commits to: Alice and Bob.
    genesis: Writers,
    root: ChildInfo,
    clock: std::cell::Cell<u64>,
    _nonce_off: Box<dyn std::any::Any>,
    _tmp: tempfile::TempDir,
    _keep: crate::test_support::KeepAlive,
}

/// The rotation that removes Bob and adds Carol.
const ROTATION: [u8; 32] = [0xD1; 32];

fn heads(ids: &[[u8; 32]]) -> Vec<[u8; 32]> {
    ids.to_vec()
}

impl Scene {
    async fn new() -> Self {
        Self::over(None).await
    }

    /// `over` a store whose rows and storage a previous node left behind.
    async fn over(reuse: Option<&Scene>) -> Self {
        let nonce_off = disable_nonce_check_for_testing();
        let (alice, bob, carol) = (
            SigningKey::from_bytes(&[0xA1; 32]),
            SigningKey::from_bytes(&[0xB1; 32]),
            SigningKey::from_bytes(&[0xC1; 32]),
        );
        let world = match reuse {
            Some(previous) => Arc::clone(&previous.world),
            None => Arc::new(RotationWorld::for_context(
                context(),
                &[pubkey_of(&alice), pubkey_of(&bob), pubkey_of(&carol)],
            )),
        };
        world
            .store
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
            .expect("seed the context row");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let host: HostWriters = {
            let (world, node_store) = (Arc::clone(&world), world.store.clone());
            Arc::new(move |cell, heads| {
                world
                    .projections
                    .read()
                    .unwrap()
                    .shared_writers_at_cut(&node_store, &context(), cell, heads)
                    .ok()
            })
        };
        let recipient = LazyRecipient::<ContextMessage>::new();
        let init = recipient.clone();
        let executor_seen = Arc::clone(&seen);
        let _addr = Executor::create(move |ctx| {
            assert!(init.init(ctx), "executor recipient init");
            Executor {
                seen: executor_seen,
                host,
            }
        });
        let (store, tmp, keep) = delta_store_over_governance(
            world.store.clone(),
            recipient,
            Arc::clone(&world.projections),
        )
        .await;
        let accounts: std::collections::BTreeMap<PublicKey, AccountId> = [&alice, &bob, &carol]
            .into_iter()
            .map(|sk| (pubkey_of(sk), world.account(&pubkey_of(sk))))
            .collect();
        store.arm_signer_resolver(Arc::new(move |key| accounts.get(key).copied()));

        let root = match reuse {
            Some(previous) => previous.root.clone(),
            None => setup_root_for_main(),
        };
        let genesis_accounts: BTreeSet<AccountId> = [&alice, &bob]
            .into_iter()
            .map(|sk| world.account(&pubkey_of(sk)))
            .collect();
        let cell = cell_at(0x40, &genesis_accounts);
        Scene {
            genesis: full_mask(genesis_accounts),
            world,
            store,
            seen,
            alice,
            bob,
            carol,
            cell,
            root,
            clock: std::cell::Cell::new(calimero_storage::env::time_now()),
            _nonce_off: Box::new(nonce_off),
            _tmp: tmp,
            _keep: keep,
        }
    }

    fn account(&self, key: &SigningKey) -> AccountId {
        self.world.account(&pubkey_of(key))
    }

    /// Bob is removed and Carol added, by a step Alice signs on the joined cut.
    fn rotate(&self) {
        let rotated = full_mask(
            [&self.alice, &self.carol]
                .into_iter()
                .map(|sk| self.account(sk))
                .collect(),
        );
        self.world.rotate(
            &pubkey_of(&self.alice),
            self.cell,
            ROTATION,
            &self.world.joined(),
            self.genesis.clone(),
            1,
            rotated,
        );
        self.world.set_current_heads(&[ROTATION]);
    }

    fn joined(&self) -> Vec<[u8; 32]> {
        self.world.joined()
    }

    /// A write to the cell signed by `signer`, claiming the writers the cell id commits to.
    fn write(
        &self,
        id: u8,
        parents: &[[u8; 32]],
        signer: &SigningKey,
        create: bool,
    ) -> CausalDelta<Vec<Action>> {
        let at = self.clock.get() + 1_000_000_000;
        self.clock.set(at);
        let claim: BTreeSet<AccountId> = self.genesis.keys().copied().collect();
        let action = build_signed_shared_action(
            create,
            self.cell,
            vec![id],
            claim,
            at,
            signer,
            if create {
                vec![self.root.clone()]
            } else {
                vec![]
            },
        );
        CausalDelta {
            id: [id; 32],
            parents: parents.to_vec(),
            payload: vec![action],
            hlc: HybridTimestamp::default(),
            kind: DeltaKind::Regular,
        }
    }

    /// The store is handed `delta` as its author signed it at `position`.
    async fn add(
        &self,
        delta: CausalDelta<Vec<Action>>,
        author: &SigningKey,
        position: Option<&[[u8; 32]]>,
    ) -> eyre::Result<bool> {
        self.store
            .add_delta(
                delta,
                Some(pubkey_of(author)),
                position.map(blob),
                Some([0xEE; 64]),
                None,
            )
            .await
    }

    /// Alice creates the cell at the joined cut.
    async fn bootstrap(&self) {
        let joined = self.joined();
        let created = self.write(0x01, &[GENESIS], &self.alice, true);
        assert!(self
            .add(created, &self.alice, Some(&joined))
            .await
            .expect("the creating delta applies"));
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// What storage refused delta `id` with; the delta must have reached it and been refused.
    fn refused_by_storage(&self, id: u8) -> String {
        self.seen()
            .into_iter()
            .rev()
            .find(|seen| seen.delta_id == [id; 32])
            .expect("the delta reached storage")
            .refusal
            .expect("storage refused it")
    }

    fn reached_the_executor(&self, id: u8) -> bool {
        self.seen().iter().any(|seen| seen.delta_id == [id; 32])
    }
}

fn blob(heads: &[[u8; 32]]) -> Vec<u8> {
    borsh::to_vec(&GovernanceParentEdge {
        governance_dag_heads: heads.to_vec(),
    })
    .expect("a position encodes")
}

#[actix::test]
async fn a_delta_is_applied_at_the_position_its_author_signed() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    let joined = scene.joined();

    let seen = scene.seen();
    assert_eq!(
        seen[0].position,
        Some(GovernanceParentEdge {
            governance_dag_heads: joined
        }),
        "the executor reads the cell's writers at the position the delta carries"
    );
    assert!(
        seen[0].effective_writers.is_empty(),
        "a cell that never rotated stands at the set its id commits to"
    );
}

#[actix::test]
async fn a_removed_writer_is_refused_at_a_position_after_his_removal() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();

    let after = heads(&[ROTATION]);
    let forged = scene.write(0x02, &[[0x01; 32]], &scene.bob, false);
    let result = scene.add(forged, &scene.bob, Some(&after)).await;
    assert!(
        result.is_err(),
        "Bob is not a writer at the rotation: {result:?}"
    );
    assert_eq!(
        scene.refused_by_storage(0x02),
        "InvalidSignature",
        "he is not among the writers handed to storage"
    );

    let seen = scene.seen();
    let judged = seen.last().expect("the store judged it and handed it on");
    assert_eq!(
        judged.effective_writers[&scene.cell],
        full_mask(
            [&scene.alice, &scene.carol]
                .into_iter()
                .map(|sk| scene.account(sk))
                .collect()
        ),
        "the writers handed to storage are the rotated set"
    );
}

#[actix::test]
async fn a_write_signed_before_the_removal_is_still_accepted_after_it() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();

    let before = scene.joined();
    let early = scene.write(0x02, &[[0x01; 32]], &scene.bob, false);
    assert!(
        scene
            .add(early, &scene.bob, Some(&before))
            .await
            .expect("Bob was a writer at the cut he signed at"),
        "the node's current heads are past the rotation, the delta's are not"
    );
}

#[actix::test]
async fn an_added_writer_is_accepted_only_at_or_after_the_rotation_that_added_her() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();

    let before = scene.joined();
    let too_early = scene.write(0x02, &[[0x01; 32]], &scene.carol, false);
    assert!(
        scene
            .add(too_early, &scene.carol, Some(&before))
            .await
            .is_err(),
        "Carol is no writer until the rotation"
    );
    assert_eq!(scene.refused_by_storage(0x02), "InvalidSignature");

    let after = heads(&[ROTATION]);
    let on_time = scene.write(0x03, &[[0x01; 32]], &scene.carol, false);
    assert!(scene
        .add(on_time, &scene.carol, Some(&after))
        .await
        .expect("Carol is a writer at the rotation"));
}

#[actix::test]
async fn a_position_older_than_a_parents_is_refused_before_storage_sees_it() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();

    let after = heads(&[ROTATION]);
    let by_carol = scene.write(0x02, &[[0x01; 32]], &scene.carol, false);
    scene
        .add(by_carol, &scene.carol, Some(&after))
        .await
        .expect("Carol's write after the rotation applies");

    // Bob builds on Carol's write, which had seen the rotation, yet cites the cut before it.
    let before = scene.joined();
    let backdated = scene.write(0x03, &[[0x02; 32]], &scene.bob, false);
    let result = scene.add(backdated, &scene.bob, Some(&before)).await;
    let refusal = format!(
        "{:#}",
        result.expect_err("a position older than a parent's")
    );
    assert!(refusal.contains("older"), "{refusal}");
    assert!(!scene.reached_the_executor(0x03));
}

#[actix::test]
async fn a_missing_or_empty_position_is_refused_once_the_cell_has_rotated() {
    let scene = Scene::new().await;
    scene.bootstrap().await;

    let unrotated = scene.write(0x02, &[[0x01; 32]], &scene.bob, false);
    assert!(
        scene
            .add(unrotated, &scene.bob, None)
            .await
            .expect("while no cell has rotated a position decides nothing"),
        "a cell that never rotated can be judged without one"
    );

    scene.rotate();
    let no_position = scene.write(0x03, &[[0x02; 32]], &scene.bob, false);
    let refusal = format!(
        "{:#}",
        scene
            .add(no_position, &scene.bob, None)
            .await
            .expect_err("no position")
    );
    assert!(refusal.contains("no governance position"), "{refusal}");
    let empty = scene.write(0x04, &[[0x02; 32]], &scene.bob, false);
    let refusal = format!(
        "{:#}",
        scene
            .add(empty, &scene.bob, Some(&[]))
            .await
            .expect_err("an empty position")
    );
    assert!(refusal.contains("no governance position"), "{refusal}");
    assert!(!scene.reached_the_executor(0x03) && !scene.reached_the_executor(0x04));
}

#[actix::test]
async fn a_cut_not_yet_folded_defers_the_delta_until_governance_catches_up() {
    let scene = Scene::new().await;
    scene.bootstrap().await;

    let after = heads(&[ROTATION]);
    let by_carol = scene.write(0x02, &[[0x01; 32]], &scene.carol, false);
    let retry = by_carol.clone();
    let early = scene.add(by_carol, &scene.carol, Some(&after)).await;
    let deferral = format!("{:#}", early.expect_err("the cut is not folded here yet"));
    assert!(deferral.contains("deferred"), "{deferral}");
    assert!(
        !scene.reached_the_executor(0x02),
        "never applied on a guess"
    );

    scene.rotate();
    assert!(
        scene
            .add(retry, &scene.carol, Some(&after))
            .await
            .expect("the same delta applies once the rotation has arrived"),
        "a deferral is not a verdict"
    );
}

#[actix::test]
async fn a_cell_over_the_rotation_budget_refuses_the_delta() {
    let scene = Scene::new().await;
    scene.bootstrap().await;

    // Every step by a member in standing counts toward the cell's budget, whatever it rests on.
    let mut parents = scene.joined();
    for n in 0..=calimero_storage::shared_writers::MAX_STEPS_PER_CELL {
        let mut id = [0x5E; 32];
        id[..8].copy_from_slice(&u64::try_from(n).unwrap().to_be_bytes());
        let mut new = scene.genesis.clone();
        let _ = new.insert(
            AccountId::from([0x99; 32]),
            calimero_storage::entities::OpMask::WRITE,
        );
        scene.world.rotate(
            &pubkey_of(&scene.alice),
            scene.cell,
            id,
            &parents,
            scene.genesis.clone(),
            u64::try_from(n).unwrap() + 1,
            new,
        );
        parents = vec![id];
    }

    let over = heads(&parents);
    let write = scene.write(0x02, &[[0x01; 32]], &scene.alice, false);
    let refusal = format!(
        "{:#}",
        scene
            .add(write, &scene.alice, Some(&over))
            .await
            .expect_err("a cell past the budget has no answer")
    );
    assert!(refusal.contains("refused"), "{refusal}");
}

#[actix::test]
async fn a_delta_that_touches_no_shared_cell_needs_no_governance() {
    let scene = Scene::new().await;
    let plain = CausalDelta {
        id: [0x09; 32],
        parents: vec![GENESIS],
        payload: Vec::new(),
        hlc: HybridTimestamp::default(),
        kind: DeltaKind::Regular,
    };
    // A position the node has never folded, and a signer it cannot place: neither matters.
    let unknown = heads(&[[0xEE; 32]]);
    assert!(scene
        .add(plain, &scene.bob, Some(&unknown))
        .await
        .expect("nothing shared is touched, so nothing is judged"));
    assert!(scene.seen()[0].effective_writers.is_empty());
}

#[actix::test]
async fn a_batch_judges_a_child_against_the_position_of_a_parent_in_the_same_batch() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();

    let (after, before) = (heads(&[ROTATION]), scene.joined());
    let input = |delta, author: &SigningKey, position: &[[u8; 32]]| BatchDeltaInput {
        delta,
        events: None,
        author_id: Some(pubkey_of(author)),
        governance_position_blob: Some(blob(position)),
        delta_signature: Some([0xEE; 64]),
        delegation: None,
    };
    let result = scene
        .store
        .add_deltas_batch(
            vec![
                input(
                    scene.write(0x02, &[[0x01; 32]], &scene.carol, false),
                    &scene.carol,
                    &after,
                ),
                input(
                    scene.write(0x03, &[[0x02; 32]], &scene.bob, false),
                    &scene.bob,
                    &before,
                ),
            ],
            |_| {},
        )
        .await
        .expect("the batch runs");
    assert_eq!(result.applied, vec![[0x02; 32]]);
    assert_eq!(
        result.failed,
        vec![[0x03; 32]],
        "Bob's backdated child is refused"
    );
}

/// A batch arms the signer resolver for each delta at that delta's own position, so a delta
/// signed before a change in the bindings is resolved as it was then.
#[actix::test]
async fn a_batch_resolves_each_delta_at_its_own_position() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();

    let (after, before) = (heads(&[ROTATION]), scene.joined());
    let input = |delta, position: &[[u8; 32]]| BatchDeltaInput {
        delta,
        events: None,
        author_id: Some(pubkey_of(&scene.alice)),
        governance_position_blob: Some(blob(position)),
        delta_signature: Some([0xEE; 64]),
        delegation: None,
    };
    // A key resolves only at a cut that includes the rotation.
    let alice = scene.account(&scene.alice);
    let arm = |input: &BatchDeltaInput| {
        let resolves = scene
            .store
            .position_of(&input.delta.id)
            .is_some_and(|position| position.contains(&ROTATION));
        scene
            .store
            .arm_signer_resolver(Arc::new(move |_| resolves.then_some(alice)));
    };
    let _result = scene
        .store
        .add_deltas_batch(
            vec![
                input(
                    scene.write(0x03, &[[0x01; 32]], &scene.alice, false),
                    &after,
                ),
                input(
                    scene.write(0x02, &[[0x01; 32]], &scene.alice, false),
                    &before,
                ),
            ],
            arm,
        )
        .await
        .expect("the batch runs");

    let signer_of = |id: u8| {
        scene
            .seen()
            .into_iter()
            .find(|seen| seen.delta_id == [id; 32])
            .map(|seen| seen.signer_account)
    };
    assert_eq!(
        signer_of(0x03),
        Some(Some(alice)),
        "resolved at its own cut"
    );
    assert_eq!(
        signer_of(0x02),
        Some(None),
        "not resolved at the cut of the delta before it"
    );
}

#[actix::test]
async fn a_node_that_joined_by_snapshot_judges_a_delta_at_its_own_position() {
    // The node holds the cell's anchor with the genesis set from the snapshot, knows the
    // governance history, and has no data delta before the boundary.
    let scene = Scene::new().await;
    let joined = scene.joined();
    let created = scene.write(0x01, &[GENESIS], &scene.alice, true);
    let Action::Add { .. } = &created.payload[0] else {
        panic!("the anchor is created by an add");
    };
    with_runtime_env(env_resolving(|_| Some(CellWriters::Genesis)), || {
        Interface::<MainStorage>::apply_action(
            created.payload[0].clone(),
            &calimero_storage::tests::common::apply_ctx_for(scene.account(&scene.alice)),
        )
        .expect("the anchor arrives with the snapshot");
    });
    scene.rotate();
    let boundary = [0x0B; 32];
    let _checkpoints = scene
        .store
        .add_snapshot_checkpoints(vec![boundary], [0x0C; 32])
        .await;

    let after = heads(&[ROTATION]);
    let removed = scene.write(0x02, &[boundary], &scene.bob, false);
    assert!(
        scene.add(removed, &scene.bob, Some(&after)).await.is_err(),
        "a removed writer is refused at a position after his removal"
    );
    assert_eq!(
        scene.refused_by_storage(0x02),
        "InvalidSignature",
        "by storage, against the set the fold gave for his position"
    );
    let added = scene.write(0x03, &[boundary], &scene.carol, false);
    assert!(scene
        .add(added, &scene.carol, Some(&after))
        .await
        .expect("an added writer is accepted at her own position"),);
    let before_removal = scene.write(0x04, &[boundary], &scene.bob, false);
    assert!(
        scene
            .add(before_removal, &scene.bob, Some(&joined))
            .await
            .expect("the boundary has no stored position to contradict the cut he cites"),
        "a write from before the removal is accepted, there being no parent to refute it"
    );
}

/// Compacting drops a parent's row but not what this run remembers of its position, so the stale
/// rule still holds here. It does not survive a restart: a parent whose position a node no
/// longer has is skipped, so the rule is best-effort per node and only bounds honest lag.
#[actix::test]
async fn a_compacted_parent_is_still_refuted_while_this_run_remembers_its_position() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();
    let after = heads(&[ROTATION]);
    for (id, parent) in [(0x02_u8, 0x01_u8), (0x03, 0x02), (0x04, 0x03)] {
        let delta = scene.write(id, &[[parent; 32]], &scene.carol, false);
        scene
            .add(delta, &scene.carol, Some(&after))
            .await
            .expect("Carol's chain applies");
    }
    assert!(
        scene.store.compact(1, 1).await > 0,
        "the old deltas are pruned"
    );

    // The parent's row is gone; the in-memory map is what still says where its author stood.
    let before = scene.joined();
    let backdated = scene.write(0x05, &[[0x02; 32]], &scene.bob, false);
    assert!(
        scene
            .add(backdated, &scene.bob, Some(&before))
            .await
            .is_err(),
        "a pruned parent still refutes a position older than its own"
    );
}

#[actix::test]
async fn a_restarted_node_reads_a_parents_position_from_its_stored_row() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();
    let after = heads(&[ROTATION]);
    let by_carol = scene.write(0x02, &[[0x01; 32]], &scene.carol, false);
    scene
        .add(by_carol, &scene.carol, Some(&after))
        .await
        .expect("Carol's write applies");

    let restarted = Scene::over(Some(&scene)).await;
    restarted
        .store
        .load_persisted_deltas()
        .await
        .expect("the store reloads its rows");
    let before = scene.joined();
    let backdated = scene.write(0x03, &[[0x02; 32]], &scene.bob, false);
    let refusal = format!(
        "{:#}",
        restarted
            .add(backdated, &scene.bob, Some(&before))
            .await
            .expect_err("the stored row carries the position Carol signed at")
    );
    assert!(refusal.contains("older"), "{refusal}");
}

/// A child that waited for its parent and cascaded keeps the envelope it arrived with, so
/// its position still decides the stale-position check for its own children later.
#[actix::test]
async fn a_cascaded_child_keeps_the_position_and_envelope_it_arrived_with() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();
    let after = heads(&[ROTATION]);
    let child = |id: u8, parent: u8| CausalDelta {
        id: [id; 32],
        parents: vec![[parent; 32]],
        payload: vec![],
        hlc: HybridTimestamp::default(),
        kind: DeltaKind::Regular,
    };
    let row = |id: u8| {
        use calimero_store::key::ContextDagDelta;
        scene
            .world
            .store
            .handle()
            .get(&ContextDagDelta::new(context(), [id; 32]))
            .expect("the store reads")
            .expect("the delta is persisted")
    };

    // Waiting on a parent that is not here: one with events is pre-persisted whole, the
    // other is only in memory.
    let carol = pubkey_of(&scene.carol);
    let with_events = scene
        .store
        .add_delta_with_events(
            child(0x03, 0x02),
            Some(vec![1]),
            Some(carol),
            Some(blob(&after)),
            Some([0xEE; 64]),
            None,
        )
        .await
        .expect("the child waits");
    assert!(!with_events.applied);
    let without_events = scene
        .add(child(0x04, 0x02), &scene.carol, Some(&after))
        .await
        .expect("the child waits");
    assert!(!without_events);

    let parent = scene.write(0x02, &[[0x01; 32]], &scene.carol, false);
    scene
        .add(parent, &scene.carol, Some(&after))
        .await
        .expect("the parent applies and its children cascade");

    for id in [0x03, 0x04] {
        let stored = row(id);
        assert!(stored.applied, "{id:#x} cascaded");
        assert_eq!(
            stored.governance_position_blob,
            Some(blob(&after)),
            "{id:#x} keeps the position its author signed at"
        );
    }
    let stored = row(0x03);
    assert_eq!(stored.author_id, Some(carol));
    assert_eq!(stored.delta_signature, Some([0xEE; 64]));
}

/// A delta's position is the first one its store is handed: a later envelope for the same id
/// with another position does not replace it.
#[actix::test]
async fn the_first_position_a_delta_arrives_with_is_the_one_kept() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();
    let after = heads(&[ROTATION]);
    let before = scene.joined();
    let child = CausalDelta {
        id: [0x03; 32],
        parents: vec![[0x02; 32]],
        payload: vec![],
        hlc: HybridTimestamp::default(),
        kind: DeltaKind::Regular,
    };

    // The child waits on a parent that is not here, and is handed twice.
    for position in [&after, &before] {
        let applied = scene
            .add(child.clone(), &scene.carol, Some(position))
            .await
            .expect("the child waits");
        assert!(!applied);
    }
    let parent = scene.write(0x02, &[[0x01; 32]], &scene.carol, false);
    scene
        .add(parent, &scene.carol, Some(&after))
        .await
        .expect("the parent applies and its child cascades");

    let stored = scene
        .world
        .store
        .handle()
        .get(&calimero_store::key::ContextDagDelta::new(
            context(),
            [0x03; 32],
        ))
        .expect("the store reads")
        .expect("the delta is persisted");
    assert_eq!(stored.governance_position_blob, Some(blob(&after)));
}

impl Scene {
    /// The stored row of delta `id`.
    fn row(&self, id: u8) -> types::ContextDagDelta {
        self.world
            .store
            .handle()
            .get(&key::ContextDagDelta::new(context(), [id; 32]))
            .expect("the store reads")
            .expect("the delta is persisted")
    }

    /// A delta with events that waits on a parent that is not here, signed at `position`.
    async fn wait_with_events(&self, position: &[[u8; 32]]) {
        let child = CausalDelta {
            id: [0x03; 32],
            parents: vec![[0x02; 32]],
            payload: vec![],
            hlc: HybridTimestamp::default(),
            kind: DeltaKind::Regular,
        };
        let applied = self
            .store
            .add_delta_with_events(
                child,
                Some(vec![1]),
                Some(pubkey_of(&self.carol)),
                Some(blob(position)),
                Some([0xEE; 64]),
                None,
            )
            .await
            .expect("the child waits");
        assert!(!applied.applied);
    }
}

/// The first position a delta with events arrives with is the one its row keeps, although it
/// is pre-persisted on every arrival.
#[actix::test]
async fn the_first_position_of_a_delta_with_events_is_the_one_its_row_keeps() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();
    let after = heads(&[ROTATION]);

    scene.wait_with_events(&after).await;
    scene.wait_with_events(&scene.joined()).await;
    assert_eq!(scene.row(0x03).governance_position_blob, Some(blob(&after)));

    let parent = scene.write(0x02, &[[0x01; 32]], &scene.carol, false);
    scene
        .add(parent, &scene.carol, Some(&after))
        .await
        .expect("the parent applies and its child cascades");
    assert_eq!(scene.row(0x03).governance_position_blob, Some(blob(&after)));
}

/// A node that restarts still holds the position its row was written with: a second envelope
/// for the delta does not replace it.
#[actix::test]
async fn a_restarted_node_keeps_the_position_its_row_holds() {
    let scene = Scene::new().await;
    scene.bootstrap().await;
    scene.rotate();
    let after = heads(&[ROTATION]);
    scene.wait_with_events(&after).await;

    let restarted = Scene::over(Some(&scene)).await;
    restarted.wait_with_events(&restarted.joined()).await;
    assert_eq!(
        restarted.row(0x03).governance_position_blob,
        Some(blob(&after))
    );
}

/// A repair leaf has no cut, so it is judged by every writer the cell has had by this node's
/// heads: a writer a rotation removed keeps what they wrote, and a stranger never gets in.
#[actix::test]
async fn a_repair_admits_every_writer_the_cell_has_had_and_no_one_else() {
    use calimero_context_client::client::CurrentCellWritersSlot;
    use calimero_storage::tests::common::apply_ctx_for;

    let scene = Scene::new().await;
    let created = scene.write(0x01, &[GENESIS], &scene.alice, true);
    with_runtime_env(env_resolving(|_| Some(CellWriters::Genesis)), || {
        Interface::<MainStorage>::apply_action(
            created.payload[0].clone(),
            &apply_ctx_for(scene.account(&scene.alice)),
        )
        .expect("the anchor is here");
    });
    scene.rotate();

    let slot = CurrentCellWritersSlot::default();
    assert!(
        slot.install(Arc::new(crate::cell_writers::ProjectionWriters::new(
            Arc::clone(&scene.world.projections),
            scene.world.store.clone(),
        )))
    );
    let repair_env = || {
        let resolve = slot.ever_resolver(context());
        env_resolving(move |cell| resolve(cell))
    };
    let current_env = || {
        let (world, store) = (Arc::clone(&scene.world), scene.world.store.clone());
        env_resolving(move |cell| {
            world
                .projections
                .read()
                .unwrap()
                .shared_writers_at_cut(&store, &context(), cell, &[ROTATION])
                .ok()
        })
    };
    let write_as = |env: calimero_storage::env::RuntimeEnv, id: u8, key: &SigningKey, acct| {
        let action = scene.write(id, &[[0x01; 32]], key, false).payload[0].clone();
        with_runtime_env(env, || {
            Interface::<MainStorage>::apply_action(action, &apply_ctx_for(acct))
        })
    };

    let stranger = SigningKey::from_bytes(&[0xD1; 32]);
    assert!(
        matches!(
            write_as(current_env(), 0x02, &scene.bob, scene.account(&scene.bob)),
            Err(calimero_storage::interface::StorageError::InvalidSignature)
        ),
        "control: the set in effect now no longer names Bob"
    );
    assert!(
        write_as(repair_env(), 0x03, &scene.bob, scene.account(&scene.bob)).is_ok(),
        "a repair still admits the writer the rotation removed"
    );
    assert!(
        write_as(
            repair_env(),
            0x04,
            &scene.carol,
            scene.account(&scene.carol)
        )
        .is_ok(),
        "and the writer it added"
    );
    assert!(
        matches!(
            write_as(repair_env(), 0x05, &stranger, AccountId::from([0x77; 32])),
            Err(calimero_storage::interface::StorageError::InvalidSignature)
        ),
        "an account no set ever named is refused"
    );
}
