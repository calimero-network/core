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
    SigningKeysRepository, UpgradeLadderRepository, UpgradesRepository,
};
use std::time::Duration;

use calimero_context::group_store::register_context_in_group;
use calimero_context_client::group::UpgradeGroupRequest;
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
use serial_test::serial;
use tokio::time::sleep;

use crate::local_governance_node_e2e::boot_test_node;

/// The app-keys the fixture runs on. Blob ids are content hashes, so the
/// fixture seeds REAL blob bytes (a minimal wasm module with an embedded
/// `calimero_abi_v1` section) and uses the returned ids — the cascade
/// dispatch resolves the migration decision from these very blobs.
struct AppBlobs {
    /// Current bytecode: declares `state_version = 1`.
    v1: [u8; 32],
    /// Code-only target: also `state_version = 1` (1 → 1 ⇒ CodeOnly).
    v2: [u8; 32],
    /// Migration-declaring target: `state_version = 2` + a v1→v2 edge.
    v2_migrating: [u8; 32],
    /// Heterogeneous sibling key for the predicate-skip test.
    other: [u8; 32],
}

/// Minimal valid wasm module: magic + version, no sections.
const EMPTY_WASM: [u8; 8] = [0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

async fn seed_app_blobs(node: &crate::local_governance_node_e2e::TestNode) -> AppBlobs {
    use calimero_wasm_abi::embed::write_embedded_state_schema;
    use calimero_wasm_abi::schema::{Manifest, MigrationEdgeAbi};

    async fn add(node: &crate::local_governance_node_e2e::TestNode, bytes: Vec<u8>) -> [u8; 32] {
        let (blob_id, _) = node
            .node_client
            .add_blob(&bytes[..], None, None)
            .await
            .expect("seed blob");
        *blob_id.as_ref()
    }

    let wasm_with = |manifest: &Manifest| {
        write_embedded_state_schema(&EMPTY_WASM, manifest).expect("embed ABI")
    };

    let mut m_v1 = Manifest::new();
    m_v1.state_version = Some(1);
    let mut m_v2 = Manifest::new();
    m_v2.state_version = Some(1);
    // Distinct state_root only varies the bytes so v1 and v2 get
    // different blob ids while both stay code-only (1 == 1).
    m_v2.state_root = Some("V2Plain".to_owned());
    let mut m_mig = Manifest::new();
    m_mig.state_version = Some(2);
    m_mig.migrations = vec![MigrationEdgeAbi {
        method: "migrate_v1_to_v2".to_owned(),
        from_version: 1,
    }];
    let mut m_other = Manifest::new();
    m_other.state_version = Some(1);
    m_other.state_root = Some("Other".to_owned());

    AppBlobs {
        v1: add(node, wasm_with(&m_v1)).await,
        v2: add(node, wasm_with(&m_v2)).await,
        v2_migrating: add(node, wasm_with(&m_mig)).await,
        other: add(node, wasm_with(&m_other)).await,
    }
}

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
fn meta_for(
    admin: PublicKey,
    app_key: [u8; 32],
    target: ApplicationId,
    upgrade_policy: UpgradePolicy,
) -> GroupMetaValue {
    GroupMetaValue {
        app_key,
        target_application_id: target,
        upgrade_policy,
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
    policy: UpgradePolicy,
) {
    MetaRepository::new(store)
        .save(gid, &meta_for(admin, app_key, target, policy))
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
    blobs: &AppBlobs,
    g2_on_other: bool,
    policy: UpgradePolicy,
    target_v2_key: [u8; 32],
) -> CascadeFixture {
    let admin_pk = admin_sk.public_key();
    let ns = ContextGroupId::from([0x70; 32]);
    let g1 = ContextGroupId::from([0xA1; 32]);
    let g2 = ContextGroupId::from([0xA2; 32]);

    provision_group(store, &ns, admin_pk, blobs.v1, app_id_v1(), policy.clone());
    provision_group(store, &g1, admin_pk, blobs.v1, app_id_v1(), policy.clone());
    // G2 may be on a different app_key for the heterogeneous test.
    let (g2_app_key, g2_target) = if g2_on_other {
        (blobs.other, app_id_other())
    } else {
        (blobs.v1, app_id_v1())
    };
    provision_group(store, &g2, admin_pk, g2_app_key, g2_target, policy);

    NamespaceRepository::new(store)
        .nest(&ns, &g1)
        .expect("nest g1");
    NamespaceRepository::new(store)
        .nest(&ns, &g2)
        .expect("nest g2");

    install_application(store, app_id_v1(), blobs.v1, "0.1.0");
    install_application(store, app_id_v2(), target_v2_key, "0.2.0");
    if g2_on_other {
        install_application(store, app_id_other(), blobs.other, "0.1.0-other");
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
        if g2_on_other {
            app_id_other()
        } else {
            app_id_v1()
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
/// cascade: true }` against a synthetic namespace with two child
/// subgroups + per-group contexts (the migration decision is resolved
/// from the seeded blobs' embedded ABIs — code-only here). After the
/// RPC resolves:
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
// These `boot_test_node`-based e2e tests share process-global state (the
// `calimero_context::tee_subgroup_admit` subscriber singleton + the
// `op_events` broadcast channel), so they must not run concurrently.
#[tokio::test]
#[serial(boot_test_node)]
async fn cascade_dispatch_e2e_single_node_emitter() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let blobs = seed_app_blobs(&node).await;
    let fx = provision_namespace(
        &node.store,
        &admin_sk,
        &blobs,
        false,
        UpgradePolicy::LazyOnAccess,
        blobs.v2,
    );

    let response = node
        .context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: fx.ns,
            target_application_id: app_id_v2(),
            requester: Some(fx.admin_pk),
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
            blobs.v2,
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
    // The cascade engine's contract is "InProgress was written", not
    // "InProgress stays" — a `wait_until` poll is enough.
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

/// Test 2 — write-gate refuses a state-op `ExecuteRequest` against a
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
/// We exercise the state-op (`__calimero_sync_next`) path specifically: user
/// calls now execute during `InProgress` (reads served, only writes refused
/// post-execution), but state-ops are writes by construction and stay refused
/// *before* execution — the branch this test pins, deterministically and
/// without a real WASM module (the fixture installs a dummy blob). The
/// `context_client.execute` of `__calimero_sync_next` for `G1` must surface
/// `ExecuteError::UpgradeInProgress { group_id: g1 }`. (User-call
/// read-allowed / write-refused behavior is covered by the
/// `upgrade_rejects_committed_write` unit tests in `calimero-context`.)
#[tokio::test]
#[serial(boot_test_node)]
async fn cascade_dispatch_e2e_write_gate_blocks_state_ops() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    // Code-only path (no migration), so descendant policy is irrelevant to
    // the gate; keep Automatic.
    let blobs = seed_app_blobs(&node).await;
    let fx = provision_namespace(
        &node.store,
        &admin_sk,
        &blobs,
        false,
        UpgradePolicy::Automatic,
        blobs.v2,
    );

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
                cascade_seq: None,
            },
        )
        .expect("save_group_upgrade InProgress for G1");

    let err = node
        .context_client
        .execute(
            &fx.ctx_g1,
            &fx.admin_pk,
            "__calimero_sync_next".to_owned(),
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect_err("state-op execute must be refused while G1 is InProgress");

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
             state-op write-gate is not firing on a pre-set InProgress status"
        ),
    }
}

/// Test 3 — predicate-skip on a heterogeneous descendant.
///
/// Same shape as Test 1 but `G2` is preconfigured on the heterogeneous
/// `other` app key (and `app_id_other`). The cascade's apply arm walks
/// descendants of NS and skips any whose current `app_key != from_app_key`:
///
///   * NS and G1 (both on the v1 key) flip to the v2 key.
///   * G2 (on the `other` key) is left untouched.
///   * No `GroupUpgradeValue` row is written for G2 — the
///     `dispatch_cascade` per-descendant loop only writes status for
///     matched descendants.
#[tokio::test]
#[serial(boot_test_node)]
async fn cascade_dispatch_e2e_predicate_skip_on_heterogeneous() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let blobs = seed_app_blobs(&node).await;
    let fx = provision_namespace(
        &node.store,
        &admin_sk,
        &blobs,
        true,
        UpgradePolicy::LazyOnAccess,
        blobs.v2,
    );

    node.context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: fx.ns,
            target_application_id: app_id_v2(),
            requester: Some(fx.admin_pk),
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
            blobs.v2,
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
        meta_g2.app_key, blobs.other,
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

/// Test 4 — migrating cascade is rejected when a matched descendant is not
/// `LazyOnAccess`, validating the per-descendant policy gate through the real
/// `dispatch_cascade` path (the unit tests cover the pure helper).
///
/// Descendants are provisioned `Automatic` and the target blob's embedded
/// ABI declares a v1→v2 migration edge; the cascade must fail with the
/// policy error BEFORE any op is emitted — so no descendant's `app_key`
/// rotates and no `GroupUpgradeValue` row is written.
#[tokio::test]
#[serial(boot_test_node)]
async fn cascade_dispatch_e2e_migration_under_automatic_descendant_rejected() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let blobs = seed_app_blobs(&node).await;
    let fx = provision_namespace(
        &node.store,
        &admin_sk,
        &blobs,
        false,
        UpgradePolicy::Automatic,
        blobs.v2_migrating,
    );

    let result = node
        .context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: fx.ns,
            target_application_id: app_id_v2(),
            requester: Some(fx.admin_pk),
            cascade: true,
        })
        .await;

    let err = result.expect_err("migrating cascade under Automatic descendants must be rejected");
    assert!(
        err.to_string().contains("LazyOnAccess"),
        "error should name the required policy, got: {err}"
    );

    // No op emitted: every group keeps its original app_key and target, and no
    // GroupUpgradeValue row exists.
    for gid in [&fx.ns, &fx.g1, &fx.g2] {
        let meta = MetaRepository::new(&node.store)
            .load(gid)
            .expect("load_group_meta")
            .expect("meta exists");
        assert_eq!(
            meta.app_key,
            blobs.v1,
            "group {} must NOT rotate app_key on a rejected cascade",
            hex::encode(gid.to_bytes())
        );
        assert!(
            UpgradesRepository::new(&node.store)
                .load(gid)
                .expect("load_group_upgrade")
                .is_none(),
            "group {} must have no GroupUpgradeValue row on a rejected cascade",
            hex::encode(gid.to_bytes())
        );
    }
}

/// Minimal in-memory bundle: gz tar with an UNSIGNED manifest.json (every
/// node-side read path goes through `extract_manifest_allow_unsigned`) and
/// one `app.wasm` carrying an embedded ABI. Same shape the bundle
/// installation tests build on disk.
fn build_bundle_blob(package: &str, app_version: &str, wasm: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let manifest = serde_json::json!({
        "version": "1.0",
        "package": package,
        "appVersion": app_version,
        "minRuntimeVersion": "0.1.0",
        "wasm": { "path": "app.wasm", "size": wasm.len() },
        "migrations": [],
    });
    let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest json");

    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_path("manifest.json").expect("path");
    header.set_size(manifest_bytes.len() as u64);
    header.set_cksum();
    tar.append(&header, manifest_bytes.as_slice())
        .expect("append manifest");
    let mut header = tar::Header::new_gnu();
    header.set_path("app.wasm").expect("path");
    header.set_size(wasm.len() as u64);
    header.set_cksum();
    tar.append(&header, wasm).expect("append wasm");
    tar.into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gz")
}

/// Three bundle releases of ONE package (version-stable application id):
/// state v1, v2 (edge 1->2) and v3 (edge 2->3).
struct LadderBlobs {
    v1: [u8; 32],
    v2: [u8; 32],
    v3: [u8; 32],
}

async fn seed_ladder_bundles(node: &crate::local_governance_node_e2e::TestNode) -> LadderBlobs {
    use calimero_wasm_abi::embed::write_embedded_state_schema;
    use calimero_wasm_abi::schema::{Manifest, MigrationEdgeAbi};

    async fn add(node: &crate::local_governance_node_e2e::TestNode, bytes: Vec<u8>) -> [u8; 32] {
        let (blob_id, _) = node
            .node_client
            .add_blob(&bytes[..], None, None)
            .await
            .expect("seed bundle blob");
        *blob_id.as_ref()
    }

    let abi_wasm = |sv: u32, edge: Option<(&str, u32)>| {
        let mut m = Manifest::new();
        m.state_version = Some(sv);
        if let Some((method, from)) = edge {
            m.migrations = vec![MigrationEdgeAbi {
                method: method.to_owned(),
                from_version: from,
            }];
        }
        write_embedded_state_schema(&EMPTY_WASM, &m).expect("embed ABI")
    };

    LadderBlobs {
        v1: add(
            node,
            build_bundle_blob("cascade-test-pkg", "0.1.0", &abi_wasm(1, None)),
        )
        .await,
        v2: add(
            node,
            build_bundle_blob(
                "cascade-test-pkg",
                "0.2.0",
                &abi_wasm(2, Some(("migrate_v1_to_v2", 1))),
            ),
        )
        .await,
        v3: add(
            node,
            build_bundle_blob(
                "cascade-test-pkg",
                "0.3.0",
                &abi_wasm(3, Some(("migrate_v2_to_v3", 2))),
            ),
        )
        .await,
    }
}

/// Multi-hop emit: a LazyOnAccess group two state versions behind the
/// installed row upgrades in ONE admin action. The handler discovers the
/// locally retained intermediate (still referenced by a sibling group),
/// emits one op pair per rung, and the fold captures the ladder behind
/// contexts replay. `meta.migration` ends as the LAST hop's method.
#[tokio::test]
#[serial(boot_test_node)]
async fn lazy_upgrade_emits_multi_hop_ladder() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let blobs = seed_ladder_bundles(&node).await;

    let app_id = app_id_v1();
    // The shared row holds the LATEST release (v3) — bundle ids are
    // version-stable, so the row is where a same-id upgrade targets.
    install_application(&node.store, app_id, blobs.v3, "0.3.0");

    let gid = ContextGroupId::from([0x71; 32]);
    provision_group(
        &node.store,
        &gid,
        admin_pk,
        blobs.v1,
        app_id,
        UpgradePolicy::LazyOnAccess,
    );
    register_context_for(&node.store, &gid, ContextId::from([0xC5; 32]), app_id);
    // A sibling group still running 0.2.0 keeps the intermediate blob
    // referenced — that's what makes it discoverable as a rung.
    let sibling = ContextGroupId::from([0x72; 32]);
    provision_group(
        &node.store,
        &sibling,
        admin_pk,
        blobs.v2,
        app_id,
        UpgradePolicy::LazyOnAccess,
    );
    SigningKeysRepository::new(&node.store)
        .store_key(&gid, &admin_pk, &admin_sk)
        .expect("store signing key");

    let response = node
        .context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: gid,
            target_application_id: app_id,
            requester: Some(admin_pk),
            cascade: false,
        })
        .await
        .expect("multi-hop lazy upgrade should succeed");
    assert_eq!(response.group_id, gid);

    let meta = MetaRepository::new(&node.store)
        .load(&gid)
        .expect("load meta")
        .expect("meta exists");
    assert_eq!(meta.app_key, blobs.v3, "group must land on the target blob");
    assert_eq!(
        meta.migration,
        Some(b"migrate_v2_to_v3".to_vec()),
        "group hint must be the LAST hop's method"
    );

    let rungs = UpgradeLadderRepository::new(&node.store)
        .load(&gid)
        .expect("load ladder");
    assert_eq!(
        rungs.iter().map(|r| r.app_key).collect::<Vec<_>>(),
        vec![blobs.v2, blobs.v3],
        "ladder must record the intermediate then the target, in order"
    );
    assert!(
        rungs.iter().all(|r| r.application_id == app_id),
        "version-stable bundle id on every rung"
    );
}

/// Multi-hop emit with NO retained intermediate: the upgrade must reject
/// up front (no ops emitted, group untouched) and the error must name the
/// missing state version — the support-floor message the operator acts on.
#[tokio::test]
#[serial(boot_test_node)]
async fn lazy_upgrade_multi_hop_missing_intermediate_rejects_with_floor() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let blobs = seed_ladder_bundles(&node).await;

    let app_id = app_id_v1();
    install_application(&node.store, app_id, blobs.v3, "0.3.0");

    let gid = ContextGroupId::from([0x73; 32]);
    provision_group(
        &node.store,
        &gid,
        admin_pk,
        blobs.v1,
        app_id,
        UpgradePolicy::LazyOnAccess,
    );
    register_context_for(&node.store, &gid, ContextId::from([0xC6; 32]), app_id);
    SigningKeysRepository::new(&node.store)
        .store_key(&gid, &admin_pk, &admin_sk)
        .expect("store signing key");

    let err = node
        .context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: gid,
            target_application_id: app_id,
            requester: Some(admin_pk),
            cascade: false,
        })
        .await
        .expect_err("missing intermediate must reject");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("state v2"),
        "error must name the missing state version, got: {msg}"
    );

    let meta = MetaRepository::new(&node.store)
        .load(&gid)
        .expect("load meta")
        .expect("meta exists");
    assert_eq!(
        meta.app_key, blobs.v1,
        "rejected upgrade must not move the group"
    );
    assert!(
        UpgradeLadderRepository::new(&node.store)
            .load(&gid)
            .expect("load ladder")
            .is_empty(),
        "no rung may be recorded on rejection"
    );
}
