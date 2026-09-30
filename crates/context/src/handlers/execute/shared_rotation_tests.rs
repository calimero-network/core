//! A run that asks to rotate a `SharedStorage` cell's writers: the node publishes the
//! rotation as a governance op before anything the run wrote is kept.
//!
//! Driven through a live `ContextManager` and a hand-written wasm module that records one
//! rotation and commits, so "what was published" and "was the write kept" are plain reads
//! of the namespace's op log and the context's root hash afterwards.

use calimero_account::AccountId;
use calimero_context_client::local_governance::GroupOp;
use calimero_context_client::messages::{ExecuteError, SharedRotationRefusal};
use calimero_context_config::types::GovernanceParentEdge;
use calimero_governance_store::{NamespaceDagService, NamespaceOpLogService};
use calimero_governance_types::NamespaceOp;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PrivateKey;
use calimero_storage::address::Id;
use calimero_storage::collections::cell_id;
use calimero_storage::entities::OpMask;
use calimero_storage::shared_writers::{CellWriters, SharedRotation, Writers};
use calimero_store::key::{self, ContextDagDelta};

use super::state_write_gate_tests::{
    fixture_running, Fixture, LocalRole, COMMITTED_ROOT, INITIAL_ROOT,
};
use crate::scope_projection::ScopeProjections;

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
            (data (i32.const 32) "\01")
            (data (i32.const 64)
                "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
                "\20\00\00\00\00\00\00\00\01\00\00\00\00\00\00\00")
            (data (i32.const 96) "{descriptor}")
            (data (i32.const 128) "{bytes}")
            (func (export "rotate") (call $rotate (i64.const 96)) (call $commit (i64.const 64) (i64.const 80))))
        "#,
        root = escape(&COMMITTED_ROOT),
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

/// Puts one op in the namespace's governance log, so the run's cut is not the empty one, by
/// rotating a cell the module does not touch.
async fn seed_governance(fx: &Fixture) {
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
