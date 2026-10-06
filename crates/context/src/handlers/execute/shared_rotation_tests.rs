//! A run that asks to rotate a `SharedStorage` cell's writers: the node publishes the
//! rotation as a governance op before anything the run wrote is kept.
//!
//! Driven through a live `ContextManager` and a hand-written wasm module that records one
//! rotation and commits, so "what was published" and "was the write kept" are plain reads
//! of the namespace's op log and the context's root hash afterwards.

use calimero_account::AccountId;
use calimero_context_client::local_governance::GroupOp;
use calimero_context_client::messages::{ExecuteError, InternalErrorKind, SharedRotationRefusal};
use calimero_context_config::types::GovernanceParentEdge;
use calimero_governance_store::{NamespaceDagService, NamespaceOpLogService};
use calimero_governance_types::NamespaceOp;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PrivateKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::collections::cell_id;
use calimero_storage::delta::StorageDelta;
use calimero_storage::entities::{Metadata, OpMask, StorageType};
use calimero_storage::shared_writers::{CellWriters, SharedRotation, Writers};
use calimero_store::key::{self, ContextDagDelta};

use super::state_write_gate_tests::{
    fixture_running, Fixture, LocalRole, COMMITTED_ROOT, INITIAL_ROOT,
};
use crate::scope_projection::ScopeProjections;
use crate::test_support::{account_for, join_namespace};

fn writers(accounts: &[AccountId]) -> Writers {
    accounts.iter().map(|a| (*a, OpMask::FULL)).collect()
}

/// A cell the local node created alone, and the rotation that adds a second writer to it.
fn rotation_for(account: AccountId, field: u8) -> SharedRotation {
    let prior = writers(&[account]);
    SharedRotation {
        cell: cell_id(Id::new([field; 32]), &prior),
        new: writers(&[account, AccountId::from([0xEE; 32])]),
        prior,
    }
}

/// A module whose `rotate` records `rotation` and commits [`COMMITTED_ROOT`], laid out like
/// the gate tests' module with the rotation's descriptor at 96 and its bytes at 128.
fn module_rotating(rotation: &SharedRotation) -> String {
    let no_writes = borsh::to_vec(&StorageDelta::Actions(Vec::new())).expect("encodes");
    module_with_artifact(rotation, &no_writes)
}

/// [`module_rotating`] committing `artifact`, which it holds at 512.
fn module_with_artifact(rotation: &SharedRotation, artifact: &[u8]) -> String {
    let escape = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("\\{b:02x}")).collect() };
    let bytes = borsh::to_vec(rotation).expect("a rotation encodes");
    let mut descriptor = 128u64.to_le_bytes().to_vec();
    descriptor.extend((bytes.len() as u64).to_le_bytes());
    format!(
        r#"
        (module
            (import "env" "commit" (func $commit (param i64 i64)))
            (import "env" "shared_writers_rotate" (func $rotate (param i64)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{root}")
            (data (i32.const 512) "{artifact}")
            (data (i32.const 64)
                "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
                "\00\02\00\00\00\00\00\00{artifact_len}")
            (data (i32.const 96) "{descriptor}")
            (data (i32.const 128) "{bytes}")
            (func (export "rotate") (call $rotate (i64.const 96)) (call $commit (i64.const 64) (i64.const 80))))
        "#,
        root = escape(&COMMITTED_ROOT),
        artifact = escape(artifact),
        artifact_len = escape(&(artifact.len() as u64).to_le_bytes()),
        descriptor = escape(&descriptor),
        bytes = escape(&bytes),
    )
}

/// A fixture whose module rotates the cell `rotation_for(account, 0xA1)` names.
async fn fixture(role: LocalRole) -> Fixture {
    fixture_running(role, |account| {
        module_rotating(&rotation_for(account, 0xA1))
    })
    .await
}

fn op_of(fx: &Fixture, rotation: &SharedRotation, prior: Writers, nonce: u64) -> GroupOp {
    GroupOp::SharedWritersRotated {
        context_id: fx.context_id,
        cell: rotation.cell,
        prior,
        nonce,
        new: rotation.new.clone(),
    }
}

/// Puts the ops a real node's log holds in the namespace's governance log: the local node's
/// join, so its rotations stand at the fold, then a rotation of a cell the module does not touch.
async fn seed_governance(fx: &Fixture) {
    let admin = PrivateKey::from([0x11; 32]);
    let _account = join_namespace(
        &fx.store,
        &fx.group_id,
        &admin,
        account_for(&admin.public_key()),
        &PrivateKey::from([0x22; 32]),
    );
    let other = rotation_for(fx.account, 0xB2);
    let report = calimero_governance_store::sign_apply_and_publish(
        &fx.store,
        &fx.harness.node_client,
        fx.harness.context_client.ack_router(),
        &fx.group_id,
        &PrivateKey::from([0x22; 32]),
        op_of(fx, &other, other.prior.clone(), 1),
    )
    .await
    .expect("the seed rotation publishes");
    assert!(report.is_some(), "the seed rotation reached the op log");
}

fn heads(fx: &Fixture) -> Vec<[u8; 32]> {
    ScopeProjections::namespace_current_heads(&fx.store, fx.group_id).unwrap_or_default()
}

/// Every group op in the namespace's log, oldest first. The log here is one chain.
fn published(fx: &Fixture) -> Vec<GroupOp> {
    let namespace = fx.group_id.to_bytes();
    let log = NamespaceOpLogService::new(&fx.store, namespace.into());
    let mut ops = Vec::new();
    let mut at = NamespaceDagService::new(&fx.store, namespace.into())
        .read_head_record()
        .expect("read the head")
        .parent_hashes;
    while let Some(id) = at.pop() {
        let signed = log
            .get_signed_op(id)
            .expect("read the op")
            .expect("the op is logged");
        at.extend(signed.parent_op_hashes.iter().copied());
        if let NamespaceOp::Group {
            group_id,
            key_id,
            encrypted,
            ..
        } = &signed.op
        {
            let op = calimero_governance_store::decrypt_group_op(
                &fx.store,
                namespace.into(),
                *group_id,
                key_id.as_bytes(),
                encrypted,
            )
            .expect("decrypt")
            .expect("this node holds the key");
            ops.push(op);
        }
    }
    ops.reverse();
    ops
}

/// The edge the run's delta was signed at.
fn delta_position(fx: &Fixture) -> GovernanceParentEdge {
    let meta: calimero_store::types::ContextMeta = fx
        .store
        .handle()
        .get(&key::ContextMeta::new(fx.context_id))
        .expect("read the context meta")
        .expect("the context meta exists");
    let [delta_id] = meta.dag_heads[..] else {
        panic!("one delta was produced, got {:?}", meta.dag_heads);
    };
    let row: calimero_store::types::ContextDagDelta = fx
        .store
        .handle()
        .get(&ContextDagDelta::new(fx.context_id, delta_id))
        .expect("read the delta")
        .expect("the delta is stored");
    borsh::from_slice(
        &row.governance_position_blob
            .expect("the delta has a position"),
    )
    .expect("the position decodes")
}

fn refusal(result: Result<impl Sized, ExecuteError>) -> SharedRotationRefusal {
    match result {
        Err(ExecuteError::SharedRotationRefused { reason, .. }) => reason,
        other => panic!("expected a refused rotation, got {:?}", other.map(drop)),
    }
}

/// The feature: the rotation goes out as exactly the op the fold will read, signed before the
/// delta, and the delta's governance position cites it.
#[actix::test]
async fn a_rotating_call_publishes_its_rotation_before_its_delta() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
    seed_governance(&fx).await;
    let before = heads(&fx);
    let rotation = rotation_for(fx.account, 0xA1);

    fx.call_locally("rotate").await.expect("the call runs");

    assert!(fx.write_was_kept(), "the run's write was committed");
    let ops = published(&fx);
    assert_eq!(
        ops.len(),
        2,
        "the seed and the run's rotation, nothing else"
    );
    let GroupOp::SharedWritersRotated {
        context_id,
        cell,
        prior,
        nonce,
        new,
    } = &ops[1]
    else {
        panic!("the run published a {}", ops[1].op_kind_label());
    };
    assert_eq!(*context_id, fx.context_id);
    assert_eq!(*cell, rotation.cell);
    assert_eq!(*prior, rotation.prior, "the set the cell's id commits to");
    assert_eq!(*new, rotation.new);
    assert!(*nonce > 0);

    let after = heads(&fx);
    assert_ne!(after, before, "the rotation advanced the governance heads");
    assert_eq!(
        delta_position(&fx).governance_dag_heads,
        after,
        "the delta is signed at a position that includes the rotation"
    );
}

/// A step built on a set another admin has since replaced applies without effect, so the
/// publish reads the cell back and reports it rather than letting the call look successful.
#[actix::test]
async fn a_rotation_overtaken_by_a_concurrent_one_is_reported_not_applied() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
    seed_governance(&fx).await;
    let rotation = rotation_for(fx.account, 0xA1);
    let projections = std::sync::Arc::new(std::sync::RwLock::new(ScopeProjections::new()));
    let pinned = super::shared_rotations::pin_cut(&fx.store, &projections, fx.context_id, None)
        .expect("the cut pins");

    // Another admin of the cell rotates it after the run pinned its cut.
    let mut concurrent = rotation.clone();
    concurrent.new = writers(&[fx.account, AccountId::from([0xDD; 32])]);
    let report = calimero_governance_store::sign_apply_and_publish(
        &fx.store,
        &fx.harness.node_client,
        fx.harness.context_client.ack_router(),
        &fx.group_id,
        &PrivateKey::from([0x22; 32]),
        op_of(&fx, &concurrent, rotation.prior.clone(), 1),
    )
    .await
    .expect("the concurrent rotation publishes");
    assert!(report.is_some());
    let now = heads(&fx);
    ScopeProjections::refresh_for_cut(&projections, &fx.store, fx.group_id, &now);
    assert_eq!(
        projections
            .read()
            .expect("not poisoned")
            .shared_writers_at_cut(&fx.store, &fx.context_id, rotation.cell, &now),
        Ok(CellWriters::Rotated(concurrent.new.clone())),
        "the concurrent rotation is the set in effect"
    );

    let published = super::shared_rotations::Publisher {
        store: &fx.store,
        node_client: &fx.harness.node_client,
        ack_router: fx.harness.context_client.ack_router(),
        projections: &projections,
        context_id: fx.context_id,
        group_id: pinned.group_id,
        author: fx.account,
    }
    .publish(
        super::shared_rotations::RunKind {
            delegated: false,
            tee: false,
            state_op: false,
        },
        &[rotation],
        &[],
        &pinned.writers,
    )
    .await;

    let error = published.expect_err("the rotation did not take effect");
    assert_eq!(
        refusal(Err::<(), _>(
            error.downcast::<ExecuteError>().expect("a typed refusal")
        )),
        SharedRotationRefusal::NotApplied
    );
}

/// A rotation that leaves `account` out of the cell, and a write `account` makes in it.
fn rotated_out_of_a_written_cell(account: AccountId) -> (SharedRotation, Action) {
    let mut rotation = rotation_for(account, 0xA1);
    rotation.new = writers(&[AccountId::from([0xEE; 32])]);
    let mut metadata = Metadata::new(1, 1);
    metadata.storage_type = StorageType::SharedMember {
        anchor: rotation.cell,
        signature_data: None,
    };
    let write = Action::Update {
        id: Id::new([0xCC; 32]),
        data: vec![1],
        ancestors: Vec::new(),
        metadata,
    };
    (rotation, write)
}

/// A run that writes a cell and rotates its author out of it is refused: receivers judge the
/// delta at a position that includes the rotation and would refuse the write.
#[actix::test]
async fn a_call_that_writes_a_cell_and_rotates_itself_out_of_it_keeps_nothing() {
    let fx = fixture_running(LocalRole::Role(GroupMemberRole::Member), |account| {
        let (rotation, write) = rotated_out_of_a_written_cell(account);
        let artifact = borsh::to_vec(&StorageDelta::Actions(vec![write])).expect("encodes");
        module_with_artifact(&rotation, &artifact)
    })
    .await;
    seed_governance(&fx).await;
    let before = heads(&fx);

    let result = fx.call_locally("rotate").await;

    assert_eq!(refusal(result), SharedRotationRefusal::RemovesOwnWrite);
    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT), "no write was kept");
    assert_eq!(heads(&fx), before, "nothing was published");
}

/// The same run committing its write as anything but `Actions` is not read as writing nothing,
/// which would pass the rights checks and keep a write no delta ships.
#[actix::test]
async fn a_call_whose_artifact_is_not_actions_keeps_and_publishes_nothing() {
    let modules: [fn(AccountId) -> String; 2] = [
        |account| {
            let (rotation, write) = rotated_out_of_a_written_cell(account);
            let artifact = borsh::to_vec(&StorageDelta::CausalActions {
                actions: vec![write],
                delta_id: [0; 32],
                delta_hlc: Default::default(),
                effective_writers: Default::default(),
                signer_account: None,
                on_behalf_accounts: Default::default(),
            })
            .expect("encodes");
            module_with_artifact(&rotation, &artifact)
        },
        |account| module_with_artifact(&rotated_out_of_a_written_cell(account).0, &[0xFF; 3]),
    ];
    for module in modules {
        let fx = fixture_running(LocalRole::Role(GroupMemberRole::Member), module).await;
        seed_governance(&fx).await;
        let before = heads(&fx);

        let result = fx.call_locally("rotate").await;

        assert!(
            matches!(
                result,
                Err(ExecuteError::InternalError {
                    kind: InternalErrorKind::Runtime
                })
            ),
            "the call fails: {:?}",
            result.map(drop)
        );
        assert_eq!(fx.root(), Hash::from(INITIAL_ROOT), "no write was kept");
        assert_eq!(heads(&fx), before, "nothing was published");
    }
}

/// A state op whose position names no heads is read at the namespace's current heads, not at
/// a cut no fold answers for.
#[actix::test]
async fn a_position_with_no_heads_pins_the_current_heads() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
    seed_governance(&fx).await;
    let projections = std::sync::Arc::new(std::sync::RwLock::new(ScopeProjections::new()));
    let empty = GovernanceParentEdge {
        governance_dag_heads: Vec::new(),
    };

    let pinned =
        super::shared_rotations::pin_cut(&fx.store, &projections, fx.context_id, Some(&empty))
            .expect("the cut pins");

    assert_eq!(
        (pinned.writers)(rotation_for(fx.account, 0xA1).cell.as_bytes()),
        Some(CellWriters::Genesis)
    );
}

/// A module whose `rotate` records `rotation` and writes nothing else.
fn module_rotating_only(rotation: &SharedRotation) -> String {
    module_rotating(rotation).replace("(call $commit (i64.const 64) (i64.const 80))", "")
}

/// A rotation writes no byte, so a call that only rotates still publishes it.
#[actix::test]
async fn a_call_that_only_rotates_publishes_its_rotation() {
    let fx = fixture_running(LocalRole::Role(GroupMemberRole::Member), |account| {
        module_rotating_only(&rotation_for(account, 0xA1))
    })
    .await;
    seed_governance(&fx).await;

    fx.call_locally("rotate").await.expect("the call runs");

    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT), "no state was written");
    let ops = published(&fx);
    assert_eq!(ops.len(), 2, "the seed and the run's rotation");
    assert!(matches!(ops[1], GroupOp::SharedWritersRotated { .. }));
}

/// A read-only node that only rotates publishes nothing.
#[actix::test]
async fn a_read_only_node_that_only_rotates_publishes_nothing() {
    let fx = fixture_running(LocalRole::Role(GroupMemberRole::ReadOnly), |account| {
        module_rotating_only(&rotation_for(account, 0xA1))
    })
    .await;

    let response = fx.call_locally("rotate").await.expect("the call runs");

    assert!(response.read_only_write_discarded);
    assert!(heads(&fx).is_empty(), "nothing was published");
}

/// A view never publishes: a rotation is a write, so one asked for by a read is dropped.
#[actix::test]
async fn a_view_that_only_rotates_publishes_nothing() {
    let fx = fixture_running(LocalRole::Role(GroupMemberRole::Member), |account| {
        module_rotating_only(&rotation_for(account, 0xA1))
            .replace("(export \"rotate\")", "(export \"__calimero_search_scan\")")
    })
    .await;
    seed_governance(&fx).await;
    let before = heads(&fx);

    let response = fx
        .call_locally("__calimero_search_scan")
        .await
        .expect("the call runs");

    assert!(!response.read_only_write_discarded);
    assert_eq!(heads(&fx), before, "nothing was published");
    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT));
}

/// A run that names a root but ships no actions cannot be published, rotation or not.
#[actix::test]
async fn a_rotating_call_with_a_root_and_no_artifact_fails_and_publishes_nothing() {
    let fx = fixture_running(LocalRole::Role(GroupMemberRole::Member), |account| {
        module_with_artifact(&rotation_for(account, 0xA1), &[])
    })
    .await;
    seed_governance(&fx).await;
    let before = heads(&fx);

    let result = fx.call_locally("rotate").await;

    assert!(result.is_err(), "the call fails: {:?}", result.map(drop));
    assert_eq!(heads(&fx), before, "nothing was published");
    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT));
}

/// A rotation the run made up a prior for is refused, and nothing it wrote is kept.
#[actix::test]
async fn a_rotation_from_a_set_the_cell_never_had_keeps_nothing() {
    let fx = fixture_running(LocalRole::Role(GroupMemberRole::Member), |account| {
        let mut forged = rotation_for(account, 0xA1);
        forged.prior = writers(&[account, AccountId::from([0xDD; 32])]);
        module_rotating(&forged)
    })
    .await;
    seed_governance(&fx).await;
    let before = heads(&fx);

    let result = fx.call_locally("rotate").await;

    assert_eq!(refusal(result), SharedRotationRefusal::PriorNotBound);
    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT), "no write was kept");
    assert_eq!(heads(&fx), before, "nothing was published");
}

/// A node whose fold cannot read the run's cut gives no answer for a cell, so it cannot rotate one.
#[actix::test]
async fn an_unreadable_cut_refuses_the_rotation_and_keeps_nothing() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
    assert!(
        heads(&fx).is_empty(),
        "precondition: the governance log is empty"
    );

    let result = fx.call_locally("rotate").await;

    assert_eq!(refusal(result), SharedRotationRefusal::WritersUnavailable);
    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT));
    assert!(heads(&fx).is_empty());
}

/// A run whose writes are dropped because the node is read-only rotates nothing.
#[actix::test]
async fn a_discarded_run_publishes_no_rotation() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::ReadOnly)).await;

    let response = fx.call_locally("rotate").await.expect("the call runs");

    assert!(response.read_only_write_discarded);
    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT));
    assert!(heads(&fx).is_empty(), "nothing was published");
}

/// A fold that starts empty is brought up to a cut by refreshing it, including a cut a
/// local publish made, and a cell nothing has rotated then stands at genesis.
#[actix::test]
async fn refreshing_folds_a_locally_published_op_and_a_fresh_cell_is_at_genesis() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
    seed_governance(&fx).await;
    let projections = std::sync::RwLock::new(ScopeProjections::new());
    let stale = |cut: &[[u8; 32]]| {
        projections
            .read()
            .expect("not poisoned")
            .namespace_to_refresh(&fx.store, fx.group_id, cut)
            .is_some()
    };

    let cut = heads(&fx);
    assert!(stale(&cut), "nothing is folded yet");
    ScopeProjections::refresh_for_cut(&projections, &fx.store, fx.group_id, &cut);
    assert!(
        !stale(&cut),
        "the seed op the publisher logged is now folded"
    );
    let fresh = rotation_for(fx.account, 0xA1).cell;
    assert_eq!(
        projections
            .read()
            .expect("not poisoned")
            .shared_writers_at_cut(&fx.store, &fx.context_id, fresh, &cut),
        Ok(CellWriters::Genesis),
        "no rotation of the cell took effect"
    );

    fx.call_locally("rotate").await.expect("the call runs");
    let cut = heads(&fx);
    assert!(
        stale(&cut),
        "the run's rotation is a head this fold has not seen"
    );
    ScopeProjections::refresh_for_cut(&projections, &fx.store, fx.group_id, &cut);
    assert!(!stale(&cut));
}

/// A run's own rotations and the actions it wrote, published as the node does after the run.
mod rights_kept {
    use std::sync::{Arc, RwLock};

    use calimero_storage::entities::{ChildInfo, EntryRules};
    use calimero_storage::tests::common::{collection_at, member_at, owned_entry_id};

    use super::*;

    const RUN: super::super::shared_rotations::RunKind = super::super::shared_rotations::RunKind {
        delegated: false,
        tee: false,
        state_op: false,
    };

    fn stamped(storage_type: StorageType) -> Metadata {
        let mut metadata = Metadata::new(1, 1);
        metadata.storage_type = storage_type;
        metadata
    }

    fn member(anchor: Id) -> StorageType {
        StorageType::SharedMember {
            anchor,
            signature_data: None,
        }
    }

    fn put_member(cell: Id, seed: u8, add: bool) -> Action {
        let (id, data, ancestors, metadata) = (
            member_at(cell, seed),
            vec![1],
            Vec::new(),
            stamped(member(cell)),
        );
        if add {
            Action::Add {
                id,
                data,
                ancestors,
                metadata,
            }
        } else {
            Action::Update {
                id,
                data,
                ancestors,
                metadata,
            }
        }
    }

    fn delete_member(cell: Id, seed: u8) -> Action {
        Action::DeleteRef {
            id: member_at(cell, seed),
            deleted_at: 1,
            metadata: stamped(member(cell)),
        }
    }

    /// An entry `owner` owns in the value of `cell`.
    fn owned_entry(cell: Id, owner: AccountId) -> Action {
        let parent = ChildInfo::new(collection_at(cell, "entries"), [0; 32], Metadata::new(1, 1));
        Action::Add {
            id: owned_entry_id(member_at(cell, 0x32), &owner),
            data: vec![1],
            ancestors: vec![parent],
            metadata: stamped(StorageType::User {
                owner,
                signature_data: None,
                rules: EntryRules::OWNED,
            }),
        }
    }

    /// `rotation` leaves the local account with `mask` and adds a second writer.
    fn leaving(fx: &Fixture, mask: OpMask) -> SharedRotation {
        let mut rotation = rotation_for(fx.account, 0xA1);
        rotation.new = Writers::from([
            (fx.account, mask),
            (AccountId::from([0xEE; 32]), OpMask::FULL),
        ]);
        rotation
    }

    /// Publishes `rotation` for a run that wrote `actions`.
    async fn publish(
        fx: &Fixture,
        rotation: &SharedRotation,
        actions: Vec<Action>,
    ) -> eyre::Result<()> {
        let projections = Arc::new(RwLock::new(ScopeProjections::new()));
        let pinned =
            super::super::shared_rotations::pin_cut(&fx.store, &projections, fx.context_id, None)
                .expect("the cut pins");
        super::super::shared_rotations::Publisher {
            store: &fx.store,
            node_client: &fx.harness.node_client,
            ack_router: fx.harness.context_client.ack_router(),
            projections: &projections,
            context_id: fx.context_id,
            group_id: pinned.group_id,
            author: fx.account,
        }
        .publish(
            RUN,
            std::slice::from_ref(rotation),
            &actions,
            &pinned.writers,
        )
        .await
    }

    fn refused(result: eyre::Result<()>) -> SharedRotationRefusal {
        refusal(Err::<(), _>(
            result
                .expect_err("the run is refused")
                .downcast::<ExecuteError>()
                .expect("a typed refusal"),
        ))
    }

    #[actix::test]
    async fn deleting_in_a_cell_the_run_rotates_itself_out_of_delete_for_is_refused() {
        let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
        seed_governance(&fx).await;
        let before = heads(&fx);
        let rotation = leaving(&fx, OpMask::WRITE);

        let result = publish(&fx, &rotation, vec![delete_member(rotation.cell, 0x31)]).await;

        assert_eq!(refused(result), SharedRotationRefusal::RemovesOwnWrite);
        assert_eq!(heads(&fx), before, "nothing was published");
    }

    #[actix::test]
    async fn a_creator_left_with_only_write_once_is_not_refused() {
        let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
        seed_governance(&fx).await;
        let rotation = leaving(&fx, OpMask::WRITE_ONCE);

        publish(&fx, &rotation, vec![put_member(rotation.cell, 0x31, true)])
            .await
            .expect("a write-once writer may create");

        assert_eq!(published(&fx).len(), 2, "the seed and the rotation");
    }

    #[actix::test]
    async fn an_owned_entry_in_a_cell_the_run_rotates_its_author_out_of_is_refused() {
        let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
        seed_governance(&fx).await;
        let before = heads(&fx);
        let mut rotation = rotation_for(fx.account, 0xA1);
        rotation.new = writers(&[AccountId::from([0xEE; 32])]);

        let result = publish(&fx, &rotation, vec![owned_entry(rotation.cell, fx.account)]).await;

        assert_eq!(refused(result), SharedRotationRefusal::RemovesOwnWrite);
        assert_eq!(heads(&fx), before, "nothing was published");
    }

    /// The run's publisher over the cut it pinned, and a helper to verify at a position.
    struct Signing<'a> {
        fx: &'a Fixture,
        projections: Arc<RwLock<ScopeProjections>>,
        pinned: super::super::shared_rotations::PinnedCut,
    }

    impl<'a> Signing<'a> {
        fn pin(fx: &'a Fixture) -> Self {
            let projections = Arc::new(RwLock::new(ScopeProjections::new()));
            let pinned = super::super::shared_rotations::pin_cut(
                &fx.store,
                &projections,
                fx.context_id,
                None,
            )
            .expect("the cut pins");
            Self {
                fx,
                projections,
                pinned,
            }
        }

        /// The run, having wrote `actions`, is signed at `position`.
        fn verify(
            &self,
            position: Option<&GovernanceParentEdge>,
            actions: Vec<Action>,
        ) -> eyre::Result<()> {
            super::super::shared_rotations::Publisher {
                store: &self.fx.store,
                node_client: &self.fx.harness.node_client,
                ack_router: self.fx.harness.context_client.ack_router(),
                projections: &self.projections,
                context_id: self.fx.context_id,
                group_id: self.pinned.group_id,
                author: self.fx.account,
            }
            .verify_signing_cut(&self.pinned, position, &[], &actions)
        }
    }

    /// Another admin of the cell replaces its writers after the run pinned its cut.
    async fn rotated_meanwhile(fx: &Fixture, rotation: &SharedRotation, new: Writers) {
        let mut concurrent = rotation.clone();
        concurrent.new = new;
        let report = calimero_governance_store::sign_apply_and_publish(
            &fx.store,
            &fx.harness.node_client,
            fx.harness.context_client.ack_router(),
            &fx.group_id,
            &PrivateKey::from([0x22; 32]),
            op_of(fx, &concurrent, rotation.prior.clone(), 1),
        )
        .await
        .expect("the concurrent rotation publishes");
        assert!(report.is_some());
    }

    fn position_now(fx: &Fixture) -> Option<GovernanceParentEdge> {
        crate::handlers::execute::compute_governance_position_for_context(&fx.store, &fx.context_id)
    }

    #[actix::test]
    async fn an_author_removed_from_a_cell_after_the_cut_was_pinned_keeps_nothing() {
        let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
        seed_governance(&fx).await;
        let signing = Signing::pin(&fx);
        let rotation = rotation_for(fx.account, 0xA1);
        rotated_meanwhile(&fx, &rotation, writers(&[AccountId::from([0xDD; 32])])).await;

        // Only a write to another cell than the one rotated is unaffected.
        let other = rotation_for(fx.account, 0xB3).cell;
        signing
            .verify(
                position_now(&fx).as_ref(),
                vec![put_member(other, 0x31, false)],
            )
            .expect("a cell nothing rotated is still the author's");

        let result = signing.verify(
            position_now(&fx).as_ref(),
            vec![put_member(rotation.cell, 0x31, false)],
        );
        assert_eq!(refused(result), SharedRotationRefusal::RemovesOwnWrite);
    }

    #[actix::test]
    async fn an_author_who_keeps_the_right_after_the_cut_was_pinned_is_not_refused() {
        let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
        seed_governance(&fx).await;
        let signing = Signing::pin(&fx);
        let rotation = rotation_for(fx.account, 0xA1);
        rotated_meanwhile(
            &fx,
            &rotation,
            writers(&[fx.account, AccountId::from([0xDD; 32])]),
        )
        .await;

        signing
            .verify(
                position_now(&fx).as_ref(),
                vec![put_member(rotation.cell, 0x31, false)],
            )
            .expect("the author is still a writer there");
    }

    #[actix::test]
    async fn a_run_signed_at_the_heads_it_read_at_reads_nothing_more() {
        let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
        seed_governance(&fx).await;
        let mut signing = Signing::pin(&fx);
        // Heads no fold can answer: a read there would fail, so passing means none was made.
        signing.pinned.heads = vec![[0x99; 32]];
        let cell = rotation_for(fx.account, 0xA1).cell;
        let at = |heads: [u8; 32]| GovernanceParentEdge {
            governance_dag_heads: vec![heads],
        };

        signing
            .verify(Some(&at([0x99; 32])), vec![put_member(cell, 0x31, false)])
            .expect("the pinned cut is not read again");
        let result = signing.verify(Some(&at([0x98; 32])), vec![put_member(cell, 0x31, false)]);
        assert_eq!(refused(result), SharedRotationRefusal::WritersUnavailable);
    }

    #[actix::test]
    async fn a_run_that_wrote_a_cell_with_no_position_to_sign_at_fails_closed() {
        let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
        seed_governance(&fx).await;
        let signing = Signing::pin(&fx);
        let cell = rotation_for(fx.account, 0xA1).cell;

        let result = signing.verify(None, vec![put_member(cell, 0x31, false)]);
        assert_eq!(refused(result), SharedRotationRefusal::WritersUnavailable);
        signing
            .verify(None, Vec::new())
            .expect("a run that wrote no cell needs no position");
    }
}

/// A run that publishes nothing (a migration and its checks) still reads a rotated cell's
/// writers from the fold, not the set the cell id commits to.
mod migration_reads_the_fold {
    use std::sync::{Arc, RwLock};

    use super::*;

    /// A module whose `count_my_pending` returns the digit `shared_writers` answers for `cell`:
    /// 1 when no rotation took effect, 2 when the cell was rotated.
    fn module_reading(cell: Id) -> String {
        let escape =
            |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("\\{b:02x}")).collect() };
        format!(
            r#"
            (module
                (import "env" "shared_writers" (func $writers (param i64 i64) (result i32)))
                (import "env" "value_return" (func $ret (param i64)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{cell}")
                (data (i32.const 64) "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00")
                (data (i32.const 96) "\00\00\00\00\00\00\00\00\80\00\00\00\00\00\00\00\01\00\00\00\00\00\00\00")
                (func (export "count_my_pending")
                    (i32.store8 (i32.const 128)
                        (i32.add (i32.const 48) (call $writers (i64.const 64) (i64.const 1))))
                    (call $ret (i64.const 96))))
            "#,
            cell = escape(cell.as_bytes()),
        )
    }

    #[actix::test]
    async fn a_migration_reads_a_rotated_cells_writers_from_the_fold() {
        let fx = fixture_running(LocalRole::Role(GroupMemberRole::Member), |account| {
            module_reading(rotation_for(account, 0xA1).cell)
        })
        .await;
        seed_governance(&fx).await;
        let rotation = rotation_for(fx.account, 0xA1);
        let report = calimero_governance_store::sign_apply_and_publish(
            &fx.store,
            &fx.harness.node_client,
            fx.harness.context_client.ack_router(),
            &fx.group_id,
            &PrivateKey::from([0x22; 32]),
            op_of(&fx, &rotation, rotation.prior.clone(), 2),
        )
        .await
        .expect("the rotation publishes");
        assert!(report.is_some());

        let wasm = wat::parse_str(module_reading(rotation.cell)).expect("parse the module");
        let module = calimero_runtime::Engine::default()
            .compile(&wasm)
            .expect("compile the module");
        let projections = Arc::new(RwLock::new(ScopeProjections::new()));
        let answer = crate::handlers::update_application::run_count_my_pending(
            &fx.store,
            &projections,
            fx.harness.node_client.clone(),
            fx.context_id,
            module,
            fx.executor,
        )
        .await;

        assert_eq!(answer, Some(2), "the cell was rotated");
    }
}
