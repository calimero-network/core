//! What switching a context before a direct call reports, driven through a live
//! `ContextManager` over the write-gate fixture.

use std::time::Duration;

use calimero_context_client::group::ActivateUpgradeRequest;
use calimero_context_client::messages::{ExecuteRequest, WriteSource};
use calimero_governance_store::{MetaRepository, UpgradeLadderRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{self, GroupMetaValue, LadderRung};
use calimero_store::types;

use super::state_write_gate_tests::{fixture, fixture_switching, Fixture, LocalRole};
use crate::test_support::eventually;

const ANSWER_WITHIN: Duration = Duration::from_secs(5); // a lock wait never ends; any bound shows it

async fn member() -> Fixture {
    fixture(LocalRole::Role(GroupMemberRole::Member)).await
}

async fn activate(fx: &Fixture, executor: PublicKey) -> eyre::Result<bool> {
    fx.harness
        .manager
        .send(ActivateUpgradeRequest {
            context_id: fx.context_id,
            executor,
        })
        .await
        .expect("mailbox")
}

fn marker(fx: &Fixture) -> Option<[u8; 32]> {
    crate::activation::activated_bytecode(&fx.store, &fx.context_id)
}

fn group_target(fx: &Fixture) -> [u8; 32] {
    edit_meta(fx, |meta| meta.target.bytecode_id)
}

/// Rewrite the group's meta, answering what `edit` returns.
fn edit_meta<T>(fx: &Fixture, edit: impl FnOnce(&mut GroupMetaValue) -> T) -> T {
    let repo = MetaRepository::new(&fx.store);
    let mut meta = repo
        .load(&fx.group_id)
        .expect("read the group meta")
        .expect("the group meta exists");
    let out = edit(&mut meta);
    repo.save(&fx.group_id, &meta).expect("save the group meta");
    out
}

/// A migration that fails leaves the context where it was, which is not the
/// same as it being current.
#[actix::test]
async fn a_failed_migration_is_an_error() {
    let fx = member().await;
    edit_meta(&fx, |meta| meta.migration = Some(b"migrate".to_vec()));

    let outcome = activate(&fx, fx.executor).await;

    assert!(
        outcome.is_err(),
        "the module has no `migrate`, got {outcome:?}"
    );
    assert_eq!(marker(&fx), None);
}

/// Seeding a marker-less context to its real version is bookkeeping, not a
/// switch: with no hop to take, it did not move.
#[actix::test]
async fn seeding_a_marker_less_context_is_not_a_switch() {
    let fx = member().await;
    let (row, application_id) = edit_meta(&fx, |meta| {
        let row = meta.target.bytecode_id;
        meta.target.bytecode_id = [0x99; 32];
        (row, meta.target.application_id)
    });
    UpgradeLadderRepository::new(&fx.store)
        .append(
            &fx.group_id,
            LadderRung {
                bytecode_id: row,
                application_id,
                package: String::new(),
                version: String::new(),
            },
        )
        .expect("record the row's release as a rung");

    let moved = activate(&fx, fx.executor).await.expect("the switch runs");

    assert!(!moved, "no hop was taken");
    assert_eq!(marker(&fx), Some(row), "the seed still binds the context");
}

/// A switch made by execute's own lazy path, as an xcall or event handler makes
/// it, still asks the node to replay what it held back for the context.
#[actix::test]
async fn a_switch_inside_execute_replays_held_deltas() {
    let mut fx = fixture_switching(LocalRole::Role(GroupMemberRole::Member)).await;
    let target = group_target(&fx);

    let _outcome = fx
        .harness
        .manager
        .send(ExecuteRequest {
            context: fx.context_id,
            executor: fx.executor,
            method: "set".to_owned(),
            payload: Vec::new(),
            atomic: None,
            xcall_origin: None,
            xcall_depth: 0,
            delegation: None,
            read_as: None,
            tee_trigger: None,
            event_handler: false,
            write_source: WriteSource::Local,
            governance_position: None,
        })
        .await
        .expect("mailbox");
    assert_eq!(marker(&fx), Some(target), "the call switched the context");

    let mut drains = Vec::new();
    let asked = eventually(|| {
        drains.extend(fx.harness.drains());
        !drains.is_empty()
    })
    .await;
    assert!(asked, "the switch asked for no replay");
    assert!(drains.iter().all(|drained| *drained == fx.context_id));
}

/// A context already on its target answers without queueing for its write lock,
/// so a direct view does not wait behind a call that is running.
#[actix::test]
async fn a_current_context_answers_without_its_lock() {
    let fx = member().await;
    crate::activation::record_activation(&fx.store, &fx.context_id, group_target(&fx));
    let _held = fx
        .harness
        .context_client
        .acquire_lock(&fx.context_id)
        .await
        .expect("the context's lock");

    let answer = tokio::time::timeout(ANSWER_WITHIN, activate(&fx, fx.executor))
        .await
        .expect("answered while the lock is held");

    assert!(!answer.expect("nothing to switch"));
}

/// A switch runs as its executor, so one this node holds no key for switches
/// nothing, as execute would refuse its call.
#[actix::test]
async fn an_executor_this_node_cannot_sign_for_switches_nothing() {
    let fx = fixture_switching(LocalRole::Role(GroupMemberRole::Member)).await;
    let stranger = PrivateKey::from([0x44; 32]).public_key();
    fx.store
        .handle()
        .put(
            &key::ContextIdentity::new(fx.context_id, stranger),
            &types::ContextIdentity { private_key: None },
        )
        .expect("record a keyless identity");

    let moved = activate(&fx, stranger).await.expect("nothing failed");

    assert!(!moved, "the stranger switched the context");
    assert_eq!(marker(&fx), None);
}
