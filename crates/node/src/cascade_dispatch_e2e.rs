//! Single-node actor-level integration test for the cascade RPC
//! (`dispatch_cascade` in `crates/context/src/handlers/upgrade_group.rs`).
//!
//! This closes the verification gap in PR #2493: the existing Rust
//! integration tests in `calimero-context` cover the apply-arm
//! (`cascade_apply_walk.rs`) and concurrent-safety properties
//! (`cascade_concurrent_safety.rs`) in isolation, but not the
//! emitter-side RPC flow as a whole — walk → permission pre-scan →
//! publish (cleartext `GroupOp::CascadeTargetApplicationSet` +
//! optional `CascadeGroupMigrationSet`) → local apply → per-descendant
//! `UpgradesRepository::new(InProgress).save()` → propagator spawn.
//!
//! Cross-peer convergence via real gossip is intentionally out of
//! scope: that ships in #2494 (gated on `merobox#255`). The
//! `StubNetworkActor` used by `boot_test_node` (sibling test module)
//! short-circuits mesh sampling and best-effort publishes with benign
//! defaults so the local apply runs and the cascade engine can be
//! observed end-to-end without standing up a libp2p transport.

use calimero_context::group_store::{
    MembershipRepository, MetaRepository, MetadataRepository, NamespaceRepository,
    SigningKeysRepository, UpgradesRepository,
};
use std::time::Duration;

use calimero_context::group_store::register_context_in_group;
use calimero_context_client::group::UpgradeGroupRequest;
use calimero_context_client::messages::MigrationParams;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    self, ApplicationMeta as ApplicationMetaKey, ContextMeta as ContextMetaKey, GroupMetaValue,
    GroupUpgradeStatus, GroupUpgradeValue,
};
use calimero_store::types::{ApplicationMeta, ContextMeta};
use calimero_store::Store;
use rand::rngs::OsRng;
use tokio::time::sleep;

use crate::local_governance_node_e2e::boot_test_node;

/// Synthetic app-key for the "current" application — used as the
/// initial `GroupMetaValue.app_key` on every cascade-target group.
const APP_KEY_V1: [u8; 32] = [0x11; 32];
/// Synthetic app-key for the "target" application — the cascade is
/// expected to rewrite matched descendants from `APP_KEY_V1` to this.
const APP_KEY_V2: [u8; 32] = [0x22; 32];
/// Heterogeneous app-key used by Test 3 — a sibling subgroup on this
/// key must be left untouched by the cascade (predicate skip).
const APP_KEY_OTHER: [u8; 32] = [0x33; 32];

fn app_id_v1() -> ApplicationId {
    ApplicationId::from([0xAA; 32])
}
fn app_id_v2() -> ApplicationId {
    ApplicationId::from([0xBB; 32])
}
fn app_id_other() -> ApplicationId {
    ApplicationId::from([0xCC; 32])
}

/// Build a `GroupMetaValue` with the requested `app_key` / target app
/// and `admin` as both owner and admin identity (so cascade's
/// per-descendant `can_manage_application` pre-scan passes for every
/// matched group).
fn meta_for(admin: PublicKey, app_key: [u8; 32], target: ApplicationId) -> GroupMetaValue {
    GroupMetaValue {
        app_key,
        target_application_id: target,
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// Provision a group at `gid`: save its meta and seat `admin` as a
/// direct Admin row. The cascade apply arm's
/// `can_manage_application` pre-scan requires direct admin (or
/// inherited admin via an Open chain) on every matched descendant.
fn provision_group(
    store: &Store,
    gid: &ContextGroupId,
    admin: PublicKey,
    app_key: [u8; 32],
    target: ApplicationId,
) {
    MetaRepository::new(store)
        .save(gid, &meta_for(admin, app_key, target))
        .expect("save_group_meta");
    MembershipRepository::new(store)
        .add_member(gid, &admin, GroupMemberRole::Admin)
        .expect("add admin");
}

/// Install an `ApplicationMeta` keyed by `app_id` whose `bytecode`
/// blob-id is `app_key`. The cascade engine derives
/// `new_app_key = app_meta.bytecode.blob_id()` for the target — so
/// driving the test's target app_key through this field is what makes
/// the apply arm rewrite descendants to `APP_KEY_V2`.
fn install_application(store: &Store, app_id: ApplicationId, app_key: [u8; 32], version: &str) {
    let bytecode_blob = key::BlobMeta::new(calimero_primitives::blobs::BlobId::from(app_key));
    // `compiled` is unused on the cascade path (cascade-time blob
    // announce only references `bytecode`), so reusing `bytecode_blob`
    // here keeps the fixture minimal.
    let meta = ApplicationMeta::new(
        bytecode_blob,
        /* size = */ 1,
        "test://cascade".to_owned().into_boxed_str(),
        Box::new([]),
        bytecode_blob,
        "cascade-test-pkg".to_owned().into_boxed_str(),
        version.to_owned().into_boxed_str(),
        "cascade-test-signer".to_owned().into_boxed_str(),
    );
    let mut handle = store.handle();
    handle
        .put(&ApplicationMetaKey::new(app_id), &meta)
        .expect("put ApplicationMeta");
}

/// Register a `ContextMeta` row under `app_id` with a non-zero
/// `root_hash` and stitch it into `group_id` via the
/// `GroupContextIndex`. The cascade dispatch path's
/// `count_group_contexts` sees this row (drives `pre_spawn_totals`);
/// the execute write-gate (Test 2) needs the row to exist with a
/// non-zero root_hash (otherwise `ExecuteError::Uninitialized` would
/// preempt the `UpgradeInProgress` gate).
fn register_context_for(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: ContextId,
    app_id: ApplicationId,
) {
    let meta = ContextMeta::new(
        ApplicationMetaKey::new(app_id),
        /* root_hash = */ [0x01; 32],
        /* dag_heads = */ Vec::new(),
        /* service_name = */ None,
    );
    let mut handle = store.handle();
    handle
        .put(&ContextMetaKey::new(context_id), &meta)
        .expect("put ContextMeta");
    register_context_in_group(store, group_id, &context_id).expect("register_context_in_group");
}

/// Bundle returned by [`provision_namespace`] — packages the
/// synthetic IDs the assertions need to inspect after `cascade=true`
/// has been dispatched.
struct CascadeFixture {
    admin_pk: PublicKey,
    ns: ContextGroupId,
    g1: ContextGroupId,
    g2: ContextGroupId,
    ctx_ns: ContextId,
    ctx_g1: ContextId,
    ctx_g2: ContextId,
}

/// Build the canonical fixture used by Tests 1 + 2:
/// - Namespace `NS` with subgroups `G1` + `G2`, all on `APP_KEY_V1`.
/// - One registered context per group (NS, G1, G2).
/// - ApplicationMeta records for `app_v1` (current, `APP_KEY_V1`) and
///   `app_v2` (target, `APP_KEY_V2`).
/// - `admin` is direct admin on every group and holds a stored signing
///   key on NS (cascade dispatch's lightweight validation requires
///   `require_group_signing_key` to resolve when the caller doesn't
///   pass a raw key).
fn provision_namespace(
    store: &Store,
    admin_sk: &PrivateKey,
    g2_app_key: [u8; 32],
) -> CascadeFixture {
    let admin_pk = admin_sk.public_key();
    let ns = ContextGroupId::from([0x70; 32]);
    let g1 = ContextGroupId::from([0xA1; 32]);
    let g2 = ContextGroupId::from([0xA2; 32]);

    provision_group(store, &ns, admin_pk, APP_KEY_V1, app_id_v1());
    provision_group(store, &g1, admin_pk, APP_KEY_V1, app_id_v1());
    // G2 may be on a different app_key for the heterogeneous test.
    let g2_target = if g2_app_key == APP_KEY_V1 {
        app_id_v1()
    } else {
        app_id_other()
    };
    provision_group(store, &g2, admin_pk, g2_app_key, g2_target);

    NamespaceRepository::new(store)
        .nest(&ns, &g1)
        .expect("nest g1");
    NamespaceRepository::new(store)
        .nest(&ns, &g2)
        .expect("nest g2");

    install_application(store, app_id_v1(), APP_KEY_V1, "0.1.0");
    install_application(store, app_id_v2(), APP_KEY_V2, "0.2.0");
    if g2_app_key != APP_KEY_V1 {
        install_application(store, app_id_other(), APP_KEY_OTHER, "0.1.0-other");
    }

    let ctx_ns = ContextId::from([0xC0; 32]);
    let ctx_g1 = ContextId::from([0xC1; 32]);
    let ctx_g2 = ContextId::from([0xC2; 32]);
    register_context_for(store, &ns, ctx_ns, app_id_v1());
    register_context_for(store, &g1, ctx_g1, app_id_v1());
    register_context_for(
        store,
        &g2,
        ctx_g2,
        if g2_app_key == APP_KEY_V1 {
            app_id_v1()
        } else {
            app_id_other()
        },
    );

    // `dispatch_cascade` requires a signing key resolvable for the
    // requester on the signed group. We stash it under NS only; the
    // descendant `save_group_upgrade` writes don't need a per-group
    // signing key.
    SigningKeysRepository::new(store)
        .store_key(&ns, &admin_pk, admin_sk)
        .expect("store signing key");

    CascadeFixture {
        admin_pk,
        ns,
        g1,
        g2,
        ctx_ns,
        ctx_g1,
        ctx_g2,
    }
}

/// Poll `cond` until it returns `true` or the deadline elapses.
/// Mirrors the helper in `local_governance_node_e2e` — the cascade
/// dispatch's `save_group_upgrade` writes happen inside the actor's
/// `.map()` continuation before `upgrade_group` returns, so for the
/// happy path we don't actually need to poll; we still use this for
/// any cross-actor observation to keep the test robust to scheduling.
async fn wait_until<F: Fn() -> bool>(cond: F) -> bool {
    for _ in 0..200 {
        if cond() {
            return true;
        }
        sleep(Duration::from_millis(25)).await;
    }
    cond()
}

/// Test 1 — emitter happy path on a single node.
///
/// Drives a real `ContextManager` actor through `UpgradeGroupRequest {
/// cascade: true, migration: Some(..) }` against a synthetic namespace
/// with two child subgroups + per-group contexts. After the RPC
/// resolves:
///
///   * `GroupMeta.app_key` for NS, G1, G2 has flipped from
///     `APP_KEY_V1` to `APP_KEY_V2` (target's `bytecode.blob_id()`).
///   * `GroupMeta.target_application_id` is `app_v2`.
///   * A per-descendant `GroupUpgradeValue { status: InProgress }`
///     row exists for NS, G1, G2 with `total = context_count(group)`.
///
/// Together this confirms walk → permission pre-scan → cleartext
/// publish (apply-side) → per-descendant InProgress save → propagator
/// spawn happened end-to-end.
#[tokio::test]
async fn cascade_dispatch_e2e_single_node_emitter() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let fx = provision_namespace(&node.store, &admin_sk, APP_KEY_V1);

    let response = node
        .context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: fx.ns,
            target_application_id: app_id_v2(),
            requester: Some(fx.admin_pk),
            migration: Some(MigrationParams {
                method: "migrate_v1_to_v2".to_owned(),
            }),
            cascade: true,
        })
        .await
        .expect("cascade upgrade should succeed");

    assert_eq!(response.group_id, fx.ns, "response must echo signed group");

    // Apply-arm side effect: every matched descendant flipped to
    // (APP_KEY_V2, app_id_v2). The cleartext publish path inside
    // `dispatch_cascade` calls `sign_apply_local_group_op_borsh`,
    // which runs the `CascadeTargetApplicationSet` apply arm before
    // the publish gate.
    for gid in [&fx.ns, &fx.g1, &fx.g2] {
        let meta = MetaRepository::new(&node.store)
            .load(gid)
            .expect("load_group_meta")
            .expect("meta exists");
        assert_eq!(
            meta.app_key,
            APP_KEY_V2,
            "group {} must have rotated app_key",
            hex::encode(gid.to_bytes())
        );
        assert_eq!(
            meta.target_application_id,
            app_id_v2(),
            "group {} must point at app_v2",
            hex::encode(gid.to_bytes())
        );
    }

    // Per-descendant `GroupUpgradeValue` records — `dispatch_cascade`
    // writes these synchronously inside the actor's `.map()`
    // continuation (after publish, before propagator spawn). For each
    // descendant the `total` field reflects `count_group_contexts`
    // sampled at dispatch time. We tolerate either `InProgress` or
    // `Completed` here: the propagator runs immediately after the
    // save and could in principle race to `Completed` if every
    // context happens to skip (target == current with no migration).
    // With `migration: Some(..)` set, the skip branch in
    // `propagate_upgrade` is gated out, so on a clean fixture the
    // status stays `InProgress` for the propagator's full retry
    // window — but the cascade engine's contract is "InProgress was
    // written", not "InProgress stays". A `wait_until` poll is enough
    // for the contract.
    for gid in [&fx.ns, &fx.g1, &fx.g2] {
        let observed = wait_until(|| {
            UpgradesRepository::new(&node.store)
                .load(gid)
                .ok()
                .flatten()
                .is_some()
        })
        .await;
        assert!(
            observed,
            "per-descendant GroupUpgradeValue must exist for {}",
            hex::encode(gid.to_bytes())
        );
        let upgrade = UpgradesRepository::new(&node.store)
            .load(gid)
            .expect("load_group_upgrade")
            .expect("upgrade row");
        match upgrade.status {
            GroupUpgradeStatus::InProgress {
                total,
                completed: _,
                failed: _,
            } => {
                let expected_total = MetadataRepository::new(&node.store)
                    .count_contexts(gid)
                    .expect("count_group_contexts") as u32;
                assert_eq!(
                    total,
                    expected_total,
                    "InProgress.total for {} must match enumerated context count",
                    hex::encode(gid.to_bytes())
                );
            }
            GroupUpgradeStatus::Completed { .. } => {
                // Acceptable: the propagator already drained this
                // descendant. The cascade engine's invariant (a
                // status row was written) still holds.
            }
        }
        assert_eq!(upgrade.initiated_by, fx.admin_pk, "initiated_by mismatch");
    }
}

/// Test 2 — write-gate refuses user `ExecuteRequest` against a
/// context whose owning group has `GroupUpgradeStatus::InProgress`.
///
/// Scope: this test verifies ONLY the write-gate's behavior given a
/// pre-set `InProgress` status row. It is intentionally decoupled
/// from the cascade dispatch path — the cascade emission flow is
/// already covered by Test 1 (`cascade_dispatch_e2e_single_node_emitter`).
///
/// We use the canonical fixture (namespace + subgroup G1 + 1
/// registered context) but bypass `upgrade_group` entirely: the
/// `GroupUpgradeValue::InProgress` row for G1 is written directly via
/// `save_group_upgrade`. This guarantees the gate fires on a known
/// status without racing the propagator (whose internals may evolve)
/// or depending on `propagate_upgrade` failing for the right reason.
///
/// A `context_client.execute` for the context in `G1` must surface
/// `ExecuteError::UpgradeInProgress { group_id: g1 }`.
#[tokio::test]
async fn cascade_dispatch_e2e_write_gate_blocks_user_calls() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let fx = provision_namespace(&node.store, &admin_sk, APP_KEY_V1);

    // Directly pin G1's status to InProgress — no cascade dispatch,
    // no propagator involvement. The gate reads this row at
    // execute-time and must refuse.
    UpgradesRepository::new(&node.store)
        .save(
            &fx.g1,
            &GroupUpgradeValue {
                from_version: "0.1.0".to_owned(),
                to_version: "0.2.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: fx.admin_pk,
                status: GroupUpgradeStatus::InProgress {
                    total: 1,
                    completed: 0,
                    failed: 0,
                },
                cascade_hlc: None,
            },
        )
        .expect("save_group_upgrade InProgress for G1");

    let err = node
        .context_client
        .execute(
            &fx.ctx_g1,
            &fx.admin_pk,
            "set_description".to_owned(),
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect_err("execute must be refused while G1 is InProgress");

    use calimero_context_client::messages::ExecuteError;
    match err {
        ExecuteError::UpgradeInProgress { group_id } => {
            assert_eq!(
                group_id, fx.g1,
                "gate must surface the owning group of the targeted context"
            );
        }
        other => panic!(
            "expected ExecuteError::UpgradeInProgress, got {other:?} — \
             write-gate is not firing on a pre-set InProgress status"
        ),
    }
}

/// Test 3 — predicate-skip on a heterogeneous descendant.
///
/// Same shape as Test 1 but `G2` is preconfigured on `APP_KEY_OTHER`
/// (and `app_id_other`). The cascade's apply arm walks descendants of
/// NS and skips any whose current `app_key != from_app_key`, so:
///
///   * NS and G1 (both on `APP_KEY_V1`) migrate to `APP_KEY_V2`.
///   * G2 (on `APP_KEY_OTHER`) is left untouched.
///   * No `GroupUpgradeValue` row is written for G2 — the
///     `dispatch_cascade` per-descendant loop only writes status for
///     matched descendants.
#[tokio::test]
async fn cascade_dispatch_e2e_predicate_skip_on_heterogeneous() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let fx = provision_namespace(&node.store, &admin_sk, APP_KEY_OTHER);

    node.context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: fx.ns,
            target_application_id: app_id_v2(),
            requester: Some(fx.admin_pk),
            migration: Some(MigrationParams {
                method: "migrate_v1_to_v2".to_owned(),
            }),
            cascade: true,
        })
        .await
        .expect("cascade upgrade should succeed");

    // NS + G1 migrated.
    for gid in [&fx.ns, &fx.g1] {
        let meta = MetaRepository::new(&node.store)
            .load(gid)
            .expect("load_group_meta")
            .expect("meta exists");
        assert_eq!(
            meta.app_key,
            APP_KEY_V2,
            "{} must migrate",
            hex::encode(gid.to_bytes())
        );
        assert_eq!(meta.target_application_id, app_id_v2());
    }

    // G2 untouched: predicate skip on heterogeneous app_key.
    let meta_g2 = MetaRepository::new(&node.store)
        .load(&fx.g2)
        .expect("load_group_meta g2")
        .expect("g2 meta exists");
    assert_eq!(
        meta_g2.app_key, APP_KEY_OTHER,
        "G2 must NOT be touched — predicate skip on heterogeneous app_key"
    );
    assert_eq!(meta_g2.target_application_id, app_id_other());

    // No InProgress row for G2 (the matched-descendant loop never ran
    // for it). Reads must be `Ok(None)`.
    assert!(
        UpgradesRepository::new(&node.store).load(&fx.g2)
            .expect("load_group_upgrade g2")
            .is_none(),
        "G2 must NOT have a GroupUpgradeValue row — predicate skip means no propagator and no status write"
    );

    // NS + G1 do have rows.
    for gid in [&fx.ns, &fx.g1] {
        assert!(
            UpgradesRepository::new(&node.store)
                .load(gid)
                .expect("load_group_upgrade")
                .is_some(),
            "{} must have a GroupUpgradeValue row",
            hex::encode(gid.to_bytes())
        );
    }

    // Bind these so unused warnings on `ctx_*` (only Test 2 needs
    // them by name) don't fire on this test path.
    let _ = (fx.ctx_ns, fx.ctx_g1, fx.ctx_g2);

    // Hold for one tick to let the spawned propagator's first
    // iteration land before `TestNode` drops the arbiter underneath
    // it — keeps the test's tail-end logs benign.
    sleep(Duration::from_millis(25)).await;
}
