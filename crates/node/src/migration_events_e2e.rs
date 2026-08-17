//! Ordering gate for the four migration events, over their real emit sites.
//!
//! The sibling `cascade_dispatch_e2e` proves each event fires; nothing there
//! proves a subscriber sees them in an order a client can render. The four come
//! from four layers - the governance op apply, the cascade propagator, the
//! heartbeat receive reaction, and the fleet-completion latch - so the order is
//! an emergent property of the whole node, not of any one of them.
//!
//! Two arcs, because only one node sees all four:
//!
//! * The admin's node runs the propagator, so it alone sees `CascadeProgress` -
//!   and it must still reach `MigrationCompleted`, because the admin who ran the
//!   upgrade is the participant most waiting to hear it finished. The propagator
//!   stamps `completed_at` (its own contexts are swapped); the fleet latch is a
//!   separate field, so that stamp does not close it.
//! * A member's node folds the same op without a propagator, so it never sees
//!   `CascadeProgress`, which is admin-only on the wire anyway. The versions in
//!   its record come from its own application rows, not the op.
//!
//! The harness wires a `StubNetworkActor`, so the peer half of the fleet is a
//! second member identity whose signed heartbeats are fed through the real
//! `NetworkEvent` receive entry point. Everything downstream of the wire - the
//! cache, the facts comparison, the rollup, the emit, the stamp - runs for real.

use core::pin::pin;
use std::time::Duration;

use calimero_context_client::group::UpgradeGroupRequest;
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    apply_local_signed_group_op, MembershipRepository, NamespaceRepository, UpgradesRepository,
};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::events::{GroupMigrationEvent, GroupMigrationPayload, NodeEvent};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::key::GroupUpgradeStatus;
use calimero_store::Store;
use rand::rngs::OsRng;
use serial_test::serial;

use crate::cascade_dispatch_e2e::{
    app_id_v1, app_id_v2, deliver_heartbeat, install_application, next_migration_event,
    provision_group, provision_local_context_identity, register_context_for, seed_app_blobs,
    AppBlobs,
};
use crate::migration_status::{build_signed_heartbeat, MigrationFacts};
use crate::test_node_harness::{boot_test_node, TestNode};

/// How long to wait for the first event of a burst.
const FIRST_EVENT_WINDOW: Duration = Duration::from_secs(10);
/// A gap this long ends a burst. Every event in a burst is emitted by work the
/// preceding line already kicked off, so a quiet stream means it finished.
const BURST_QUIET_WINDOW: Duration = Duration::from_millis(750);

/// Drain migration events until the stream stays quiet for [`BURST_QUIET_WINDOW`].
async fn collect_migration_events(
    events: &mut (impl futures_util::Stream<Item = NodeEvent> + Unpin),
) -> Vec<GroupMigrationEvent> {
    let mut seen = Vec::new();
    let mut budget = FIRST_EVENT_WINDOW;
    while let Some(event) = next_migration_event(events, budget).await {
        seen.push(event);
        budget = BURST_QUIET_WINDOW;
    }
    seen
}

fn payload_tag(event: &GroupMigrationEvent) -> &'static str {
    match event.payload {
        GroupMigrationPayload::MigrationStarted { .. } => "MigrationStarted",
        GroupMigrationPayload::MigrationProgress { .. } => "MigrationProgress",
        GroupMigrationPayload::CascadeProgress { .. } => "CascadeProgress",
        GroupMigrationPayload::MigrationCompleted { .. } => "MigrationCompleted",
    }
}

fn tags(seen: &[GroupMigrationEvent]) -> Vec<&str> {
    seen.iter().map(payload_tag).collect()
}

fn count(tags: &[&str], tag: &str) -> usize {
    tags.iter().filter(|t| **t == tag).count()
}

fn index_of(tags: &[&str], tag: &str) -> Option<usize> {
    tags.iter().position(|t| *t == tag)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock")
        .as_millis() as u64
}

/// Facts for a member sitting at `schema_version` with nothing outstanding.
fn facts_at(schema_version: u32) -> MigrationFacts {
    MigrationFacts {
        schema_version,
        residue_auto: 0,
        synced_up_to_hlc: 0,
        authored_remaining: 0,
        migration_failed: None,
    }
}

/// Feed `peer`'s heartbeat for `ns` through the real receive entry point and
/// drain whatever the rollup announced.
async fn heartbeat_burst(
    node: &TestNode,
    events: &mut (impl futures_util::Stream<Item = NodeEvent> + Unpin),
    ns: ContextGroupId,
    peer_sk: &PrivateKey,
    schema_version: u32,
    ts_millis: u64,
) -> Vec<GroupMigrationEvent> {
    deliver_heartbeat(
        node,
        ns,
        build_signed_heartbeat(peer_sk, ns.to_bytes(), facts_at(schema_version), ts_millis)
            .expect("sign heartbeat"),
    )
    .await;
    collect_migration_events(events).await
}

/// Seat `admin` on `ns` with a namespace identity (so the node self-reports into
/// the rollup) and `peer` as a fellow member of the migration cohort.
fn seat_cohort(store: &Store, ns: &ContextGroupId, admin_sk: &PrivateKey, peer: PublicKey) {
    NamespaceRepository::new(store)
        .store_identity(ns, &admin_sk.public_key(), admin_sk.as_bytes())
        .expect("store namespace identity");
    // Enrolled at the anchor, so the cohort expansion resolves this row back to
    // the key the peer's heartbeats are signed with. A row with no binding
    // contributes nobody, and the cohort reads as converged without the peer.
    let peer_account = calimero_context::test_support::enrol(store, ns, &peer);
    MembershipRepository::new(store)
        .add_member(ns, &peer_account, GroupMemberRole::Admin)
        .expect("seat the peer");
}

/// The admin's arc: one announcement for the whole cascade, its own contexts'
/// swaps streamed as they land, the fleet frames after the announcement, and the
/// completion the admin ran the upgrade to hear.
///
/// The announcement is not ordered against `CascadeProgress` and this test does
/// not claim it is. `MigrationStarted` reaches subscribers through the op-event
/// bridge, a separate task, while the propagator this same continuation spawns
/// emits straight to `send_event` - so a propagation that never awaits (every
/// context already on target, or a fixture like this one) can outrun the
/// bridge. The member-visible pair below IS ordered, because a fleet frame
/// needs a peer heartbeat that arrives long after the bridge has drained.
#[tokio::test]
#[serial(boot_test_node)]
async fn the_admin_announces_once_and_streams_its_own_context_swaps() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let peer_sk = PrivateKey::random(&mut rng);
    let blobs = seed_app_blobs(&node).await;

    let ns = ContextGroupId::from([0x76; 32]);
    let sub = ContextGroupId::from([0x77; 32]);
    let ctx_ns = ContextId::from([0xD0; 32]);
    let ctx_sub = ContextId::from([0xD1; 32]);
    provision_namespace_pair(&node, &admin_sk, &blobs, ns, sub, ctx_ns, ctx_sub);
    seat_cohort(&node.store, &ns, &admin_sk, peer_sk.public_key());

    // Subscribe FIRST: `receive_events` takes its broadcast subscription at call
    // time, so anything emitted before this line is gone.
    let mut events = pin!(node.node_client.receive_events());

    let _response = node
        .context_client
        .upgrade_group(UpgradeGroupRequest {
            group_id: ns,
            target_application_id: app_id_v2(),
            cascade: true,
            force_code_only: false,
        })
        .await
        .expect("cascade upgrade should succeed");

    let mut seen = collect_migration_events(&mut events).await;
    // The peer has resolved no version for the namespace yet, then reports the
    // target: two genuine facts changes, so two fleet rollups.
    let started_at = now_millis();
    seen.extend(heartbeat_burst(&node, &mut events, ns, &peer_sk, 0, started_at).await);
    seen.extend(heartbeat_burst(&node, &mut events, ns, &peer_sk, 1, started_at + 1_000).await);

    let tags = tags(&seen);
    assert_eq!(
        count(&tags, "MigrationStarted"),
        1,
        "one announcement for the whole cascade, not one per matched descendant: {tags:?}"
    );
    assert!(
        tags.contains(&"CascadeProgress"),
        "this node's own context swaps must be reported: {tags:?}"
    );
    let announced = index_of(&tags, "MigrationStarted").expect("asserted above");
    let first_fleet_frame = index_of(&tags, "MigrationProgress")
        .unwrap_or_else(|| panic!("the peer's heartbeats must produce fleet frames: {tags:?}"));
    assert!(
        announced < first_fleet_frame,
        "the fleet must never report progress on a migration a subscriber has not \
         been told about: {tags:?}"
    );
    assert!(
        seen.iter().all(|e| e.group_id.as_bytes() == &ns.to_bytes()),
        "every event is keyed on the namespace root a client subscribed with"
    );

    // Both stamps, written by different things about different questions: the
    // propagator's the moment this node's own contexts were swapped, the fleet's
    // when the cohort converged. Latching the fleet on the local stamp is what
    // used to leave this node - the admin's - the only one never told.
    let upgrades = UpgradesRepository::new(&node.store);
    let GroupUpgradeStatus::Completed { completed_at } = upgrades
        .load(&ns)
        .expect("load record")
        .expect("record exists")
        .status
    else {
        panic!("the cascade settled, so the record must be Completed");
    };
    let fleet_completed_at = upgrades.fleet_completed_at(&ns).expect("read stamp");
    assert!(
        completed_at.is_some() && fleet_completed_at.is_some(),
        "the propagator's local stamp must not swallow the fleet's"
    );
    // The admin route answers from the same state, through this same rollup.
    // A stamp the read cannot surface is a value no operator ever sees, however
    // correctly the latch wrote it. Reports resolve to `unknown` here on
    // purpose: the durable answer is the stamp's, not the live cohort's.
    assert_eq!(
        calimero_context::handlers::get_migration_status::compute_namespace_rollup(
            &node.store,
            &ns,
            |_| None,
        )
        .expect("rollup")
        .fleet_completed_at,
        fleet_completed_at,
        "the admin read must surface the stamp the fleet latch wrote"
    );
    // The announced timestamp IS the stored one. Two actors reach the stamp for
    // the same namespace, and the store has no compare-and-swap, so a second
    // writer overwriting the stamp after its peer announced would leave an
    // operator holding a `completed_at` the admin read no longer agrees with -
    // silently, because both values are plausible.
    let announced_at = seen
        .iter()
        .find_map(|event| match event.payload {
            GroupMigrationPayload::MigrationCompleted { completed_at, .. } => Some(completed_at),
            _ => None,
        })
        .expect("the completion must have been announced to assert its timestamp");
    assert_eq!(
        Some(announced_at),
        fleet_completed_at,
        "the announced completed_at must be the timestamp the fleet latch persisted"
    );
    assert_eq!(
        count(&tags, "MigrationCompleted"),
        1,
        "the admin who ran the upgrade must be told it finished: {tags:?}"
    );
    assert_eq!(
        tags.last(),
        Some(&"MigrationCompleted"),
        "completed comes last on the admin's arc too: {tags:?}"
    );
}

/// The member's arc, and the one the "this workspace is updating" banner reads:
/// started, then the fleet converging, then done - exactly once, in that order.
///
/// This node never runs `upgrade_group`; it only folds the admin's op, which is
/// what leaves its record unstamped and lets the heartbeat rollup be the thing
/// that completes it.
#[tokio::test]
#[serial(boot_test_node)]
async fn a_member_sees_started_then_fleet_progress_then_completed() {
    let node = boot_test_node().await;
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    // The admin driving the upgrade from some other node.
    let peer_sk = PrivateKey::random(&mut rng);
    let blobs = seed_app_blobs(&node).await;

    let ns = ContextGroupId::from([0x78; 32]);
    let ctx = ContextId::from([0xD2; 32]);
    provision_group(&node.store, &ns, admin_pk, blobs.v1, app_id_v1());
    install_application(&node.store, app_id_v1(), blobs.v1, "0.1.0", 1);
    install_application(&node.store, app_id_v2(), blobs.v2_migrating, "0.2.0", 2);
    register_context_for(&node.store, &ns, ctx, app_id_v1());
    seat_cohort(&node.store, &ns, &admin_sk, peer_sk.public_key());

    let mut events = pin!(node.node_client.receive_events());

    let op = SignedGroupOp::sign(
        &peer_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        GroupOp::CascadeUpgrade {
            from_app_key: blobs.v1.into(),
            app_key: blobs.v2_migrating.into(),
            target_application_id: app_id_v2(),
            to_state_version: 2,
            migration: Some(b"migrate_v1_to_v2".to_vec()),
            cascade_hlc: HybridTimestamp::zero(),
        },
    )
    .expect("sign CascadeUpgrade");
    apply_local_signed_group_op(&node.store, &op).expect("member folds the admin's cascade");
    let mut seen = collect_migration_events(&mut events).await;

    // A member spawns no propagator: its contexts migrate lazily, on access.
    // The activation stands in for the migrate this fixture has no runnable wasm
    // to execute; registering the context alone would not do, since an install
    // moves the application row without migrating anything.
    register_context_for(&node.store, &ns, ctx, app_id_v2());
    calimero_context::activation::record_activation(&node.store, &ctx, blobs.v2_migrating);

    let started_at = now_millis();
    seen.extend(heartbeat_burst(&node, &mut events, ns, &peer_sk, 1, started_at).await);
    seen.extend(heartbeat_burst(&node, &mut events, ns, &peer_sk, 2, started_at + 1_000).await);

    let tags = tags(&seen);
    assert_eq!(
        tags.first(),
        Some(&"MigrationStarted"),
        "started must come first: {tags:?}"
    );
    assert_eq!(
        tags.last(),
        Some(&"MigrationCompleted"),
        "completed must come last: {tags:?}"
    );
    assert_eq!(
        count(&tags, "MigrationStarted"),
        1,
        "the announcement fires once per folded op: {tags:?}"
    );
    assert_eq!(
        count(&tags, "MigrationCompleted"),
        1,
        "completion announces exactly once: {tags:?}"
    );
    assert!(
        tags.contains(&"MigrationProgress"),
        "the laggard heartbeat must produce a fleet frame: {tags:?}"
    );
    assert_eq!(
        count(&tags, "CascadeProgress"),
        0,
        "a member runs no propagator, so it has no local swaps to stream: {tags:?}"
    );
    // The banner renders this string. Nothing on the wire carries it: the
    // receiver's record is built by the cascade apply, which resolves it from
    // this node's own application row.
    match &seen.last().expect("asserted above").payload {
        GroupMigrationPayload::MigrationCompleted { to_version, .. } => {
            assert_eq!(
                to_version, "0.2.0",
                "a member must name the version it reached"
            );
        }
        other => panic!("expected MigrationCompleted, got {other:?}"),
    }
    assert!(
        seen.iter().all(|e| e.group_id.as_bytes() == &ns.to_bytes()),
        "every event is keyed on the namespace root a client subscribed with"
    );
}

/// Namespace root plus one nested subgroup, one context each, both holding a
/// local signing identity so the propagator can actually swap them.
///
/// The target is the code-only release: this fixture has no runnable wasm, so a
/// declared migration edge would strand every context on v1 and the propagator
/// would never reach a `CascadeProgress` frame worth asserting on.
fn provision_namespace_pair(
    node: &TestNode,
    admin_sk: &PrivateKey,
    blobs: &AppBlobs,
    ns: ContextGroupId,
    sub: ContextGroupId,
    ctx_ns: ContextId,
    ctx_sub: ContextId,
) {
    let admin_pk = admin_sk.public_key();
    provision_group(&node.store, &ns, admin_pk, blobs.v1, app_id_v1());
    provision_group(&node.store, &sub, admin_pk, blobs.v1, app_id_v1());
    NamespaceRepository::new(&node.store)
        .nest(&ns, &sub)
        .expect("nest subgroup");
    install_application(&node.store, app_id_v1(), blobs.v1, "0.1.0", 1);
    install_application(&node.store, app_id_v2(), blobs.v2, "0.2.0", 1);
    for (gid, ctx) in [(ns, ctx_ns), (sub, ctx_sub)] {
        register_context_for(&node.store, &gid, ctx, app_id_v1());
        provision_local_context_identity(&node.store, ctx, admin_sk);
    }
}
