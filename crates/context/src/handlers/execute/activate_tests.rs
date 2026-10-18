//! What switching a context before a direct call reports, driven through a live
//! `ContextManager` over the write-gate fixture.

use calimero_context_client::group::ActivateUpgradeRequest;
use calimero_governance_store::{MetaRepository, UpgradeLadderRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupMetaValue, LadderRung};

use super::state_write_gate_tests::{fixture, Fixture, LocalRole};

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
