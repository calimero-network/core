//! An admin upgrade is trialled on the admin's own contexts before anything is published.
//!
//! Drives a real `ContextManager` through `UpgradeGroupRequest` against releases whose migrate
//! traps, and reads back what reached the governance log.

use calimero_context::scope_projection::ScopeProjections;
use calimero_context_client::group::UpgradeGroupRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    MetaRepository, NamespaceRepository, NodeDeviceRepository, UpgradesRepository,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use calimero_wasm_abi::embed::write_embedded_state_schema;
use calimero_wasm_abi::schema::{Manifest, MigrationEdgeAbi};
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use serial_test::serial;

use crate::cascade_dispatch_e2e::{
    app_id_v1, app_id_v2, build_bundle_blob, install_application, provision_group,
    provision_local_context_identity, register_context_for, seed_app_blobs, AppBlobs,
};
use crate::test_node_harness::{boot_test_node, TestNode};

/// A release at `state_version` whose `method` migrate traps, as a bundle blob.
async fn trapping_release(
    node: &TestNode,
    version: &str,
    state_version: u32,
    method: &str,
    from_version: u32,
) -> [u8; 32] {
    let wasm = wat::parse_str(format!(
        r#"(module (memory (export "memory") 1) (func (export "{method}") unreachable))"#
    ))
    .expect("parse the module");
    let mut manifest = Manifest::new();
    manifest.state_version = Some(state_version);
    manifest.migrations = vec![MigrationEdgeAbi {
        method: method.to_owned(),
        from_version,
    }];
    let wasm = write_embedded_state_schema(&wasm, &manifest).expect("embed ABI");
    let bundle = build_bundle_blob("cascade-test-pkg", version, &wasm);
    let (blob_id, _) = node
        .node_client
        .add_blob(&bundle[..], None, None)
        .await
        .expect("seed the release");
    *blob_id.as_ref()
}

/// A release with no migrate export: its edge is vacuous, so a trial treats it as code-only.
async fn plain_release(
    node: &TestNode,
    version: &str,
    state_version: u32,
    method: &str,
    from_version: u32,
) -> [u8; 32] {
    let mut manifest = Manifest::new();
    manifest.state_version = Some(state_version);
    manifest.migrations = vec![MigrationEdgeAbi {
        method: method.to_owned(),
        from_version,
    }];
    let wasm =
        wat::parse_str(r#"(module (memory (export "memory") 1))"#).expect("parse the module");
    let wasm = write_embedded_state_schema(&wasm, &manifest).expect("embed ABI");
    let (blob_id, _) = node
        .node_client
        .add_blob(
            &build_bundle_blob("cascade-test-pkg", version, &wasm)[..],
            None,
            None,
        )
        .await
        .expect("seed the release");
    *blob_id.as_ref()
}

struct Seat {
    node: TestNode,
    admin_sk: PrivateKey,
    blobs: AppBlobs,
    trapping: [u8; 32],
}

/// A node holding releases v1 and a v2 whose migrate traps, both installed.
async fn seat(node: TestNode) -> Seat {
    NodeDeviceRepository::new(&node.store)
        .provision_account_root()
        .expect("provision the account root an initialised node has");
    let mut rng = UnwrapErr(SysRng);
    let admin_sk = PrivateKey::random(&mut rng);
    let blobs = seed_app_blobs(&node).await;
    let trapping = trapping_release(&node, "0.2.0", 2, "migrate_v1_to_v2", 1).await;
    install_application(&node.store, app_id_v1(), blobs.v1, "0.1.0", 1);
    install_application(&node.store, app_id_v2(), trapping, "0.2.0", 2);
    Seat {
        node,
        admin_sk,
        blobs,
        trapping,
    }
}

impl Seat {
    /// A group on v1 with one context this node holds.
    fn group(&self, gid: ContextGroupId, ctx: ContextId, held: bool) {
        let store = &self.node.store;
        let admin_pk = self.admin_sk.public_key();
        provision_group(store, &gid, admin_pk, self.blobs.v1, app_id_v1());
        register_context_for(store, &gid, ctx, app_id_v1());
        if held {
            provision_local_context_identity(store, ctx, &self.admin_sk);
        }
        NamespaceRepository::new(store)
            .replace_identity(&gid, &admin_pk, self.admin_sk.as_bytes())
            .expect("seed node identity");
    }

    fn heads(&self, gid: ContextGroupId) -> Option<Vec<[u8; 32]>> {
        ScopeProjections::namespace_current_heads(&self.node.store, gid)
    }

    async fn upgrade(
        &self,
        gid: ContextGroupId,
        cascade: bool,
        skip_trial: bool,
    ) -> eyre::Result<calimero_context_client::group::UpgradeGroupResponse> {
        self.node
            .context_client
            .upgrade_group(UpgradeGroupRequest {
                group_id: gid,
                target_application_id: app_id_v2(),
                cascade,
                force_code_only: false,
                skip_trial,
            })
            .await
    }

    /// The upgrade was refused naming `ctx`, and the group is as it was.
    fn assert_refused_and_unpublished(
        &self,
        gid: ContextGroupId,
        ctx: ContextId,
        heads: Option<Vec<[u8; 32]>>,
        result: eyre::Result<calimero_context_client::group::UpgradeGroupResponse>,
    ) {
        let err = result.expect_err("the trial must refuse the upgrade");
        let message = format!("{err:#}");
        assert!(
            message.contains(&ctx.to_string()) && message.contains("Migration execution failed"),
            "the refusal names the failing context and its trap: {message}"
        );
        assert_eq!(self.heads(gid), heads, "nothing was published");
        let meta = MetaRepository::new(&self.node.store)
            .load(&gid)
            .expect("load meta")
            .expect("meta exists");
        assert_eq!(meta.target.bytecode_id, self.blobs.v1);
        assert!(
            UpgradesRepository::new(&self.node.store)
                .load(&gid)
                .expect("load upgrade")
                .is_none(),
            "the InProgress record is cleaned up"
        );
    }
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_plain_upgrade_with_a_trapping_migrate_is_refused_and_publishes_nothing() {
    let seat = seat(boot_test_node().await).await;
    let (gid, ctx) = (
        ContextGroupId::from([0x71; 32]),
        ContextId::from([0xC5; 32]),
    );
    seat.group(gid, ctx, true);
    let heads = seat.heads(gid);

    let result = seat.upgrade(gid, false, false).await;
    seat.assert_refused_and_unpublished(gid, ctx, heads, result);
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_cascade_upgrade_with_a_trapping_migrate_is_refused_and_publishes_nothing() {
    let seat = seat(boot_test_node().await).await;
    let (ns, sub) = (
        ContextGroupId::from([0x70; 32]),
        ContextGroupId::from([0xA1; 32]),
    );
    let (ctx_ns, ctx_sub) = (ContextId::from([0xC0; 32]), ContextId::from([0xC1; 32]));
    seat.group(ns, ctx_ns, false);
    seat.group(sub, ctx_sub, true);
    NamespaceRepository::new(&seat.node.store)
        .nest(&ns, &sub)
        .expect("nest the subgroup");
    let heads = seat.heads(ns);

    let result = seat.upgrade(ns, true, false).await;
    seat.assert_refused_and_unpublished(ns, ctx_sub, heads, result);
}

#[tokio::test]
#[serial(boot_test_node)]
async fn skip_trial_publishes_despite_a_trapping_migrate() {
    let seat = seat(boot_test_node().await).await;
    let (gid, ctx) = (
        ContextGroupId::from([0x71; 32]),
        ContextId::from([0xC5; 32]),
    );
    seat.group(gid, ctx, true);

    let response = seat
        .upgrade(gid, false, true)
        .await
        .expect("an upgrade that skips the trial publishes");
    assert_eq!(
        (response.contexts_trialled, response.contexts_not_held),
        (0, 0)
    );
    let meta = MetaRepository::new(&seat.node.store)
        .load(&gid)
        .expect("load meta")
        .expect("meta exists");
    assert_eq!(meta.target.bytecode_id, seat.trapping);
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_passing_trial_reports_held_and_unheld_contexts() {
    let seat = seat(boot_test_node().await).await;
    let (gid, held, unheld) = (
        ContextGroupId::from([0x71; 32]),
        ContextId::from([0xC5; 32]),
        ContextId::from([0xC6; 32]),
    );
    seat.group(gid, held, true);
    register_context_for(&seat.node.store, &gid, unheld, app_id_v1());
    // A release whose migrate the module does not export: nothing to run, nothing to fail.
    let plain = plain_release(&seat.node, "0.2.0", 2, "migrate_v1_to_v2", 1).await;
    install_application(&seat.node.store, app_id_v2(), plain, "0.2.0", 2);

    let response = seat.upgrade(gid, false, false).await.expect("publishes");
    assert_eq!(
        (response.contexts_trialled, response.contexts_not_held),
        (1, 1)
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_ladder_whose_second_rung_fails_is_refused() {
    let seat = seat(boot_test_node().await).await;
    let v2 = plain_release(&seat.node, "0.2.0", 2, "migrate_v1_to_v2", 1).await;
    let v3 = trapping_release(&seat.node, "0.3.0", 3, "migrate_v2_to_v3", 2).await;
    let (gid, sibling) = (
        ContextGroupId::from([0x71; 32]),
        ContextGroupId::from([0x72; 32]),
    );
    let ctx = ContextId::from([0xC5; 32]);
    // The shared row names the latest release; a sibling still on v2 keeps that rung known.
    install_application(&seat.node.store, app_id_v1(), v3, "0.3.0", 3);
    seat.group(gid, ctx, true);
    provision_group(
        &seat.node.store,
        &sibling,
        seat.admin_sk.public_key(),
        v2,
        app_id_v1(),
    );
    let heads = seat.heads(gid);

    let err = seat
        .node
        .context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: gid,
            target_application_id: app_id_v1(),
            cascade: false,
            force_code_only: false,
            skip_trial: false,
        })
        .await
        .expect_err("rung two fails");
    let message = format!("{err:#}");
    assert!(
        message.contains(&ctx.to_string()) && message.contains("Migration execution failed"),
        "{message}"
    );
    assert_eq!(seat.heads(gid), heads, "nothing was published");
}
