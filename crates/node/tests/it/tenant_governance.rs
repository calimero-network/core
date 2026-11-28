//! Governance by an organization's admin who joined after the organization was
//! made. Once the control plane shards by tenant, an organization's membership
//! lives in its own tenant group, while the governance commands (outbound
//! approvals, the egress switch, the update policy) still apply on the root
//! group, whose copy of the organization holds only what `CreateOrg` wrote. Each
//! such command carries the actor's standing, read from the tenant at propose, so
//! a later admin governs like the owner does
//! ([design note](https://github.com/calimero-network/core/blob/master/docs/design/system-group-sharding.md)).
//!
//! The break this guards shows only with the cutover on (`shard::ACTIVE`); with
//! it off every command and read is on the one system group.

use calimero_client::types::OrgRole;
use serde_json::json;

use crate::component_cluster::mpk_declaring;
use crate::egress::{approved_app, post, PACKAGE, PREFIX};
use crate::harness::{kv_wasm, signed_up, typed, wire};

/// An admin added after the organization was made approves outbound hosts for
/// it, sets its update policy, and turns egress off for one instance and for
/// the whole organization.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_admin_who_joined_later_governs_the_organization() {
    let (c, alice, mut pk, org, app) = approved_app().await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let bob_id = wire(bob.auth().me().await.unwrap())["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .orgs()
        .set_member(&org, &bob_id, OrgRole::Admin, Some(&mut pk))
        .await
        .unwrap();
    // A release installed for the organization: its members read it, so an
    // approval by one covers it.
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let _ = alice
        .packages()
        .upload(
            mpk_declaring(PACKAGE, "0.2.0", &kv_wasm(), &publisher, &[PREFIX]),
            Some(&org),
        )
        .await
        .unwrap();

    let approved = bob
        .orgs()
        .outbound_approval(
            &org,
            PACKAGE,
            Some(typed(json!("manifest"))),
            Some(true),
            Some(&mut bob_pk),
        )
        .await;
    assert!(approved.is_ok(), "outbound approval: {approved:?}");

    let policy = post(
        &bob,
        &format!("orgs/{org}/upgrade-policy"),
        json!({ "policy": { "secondApprover": true } }),
        Some(&mut bob_pk),
    )
    .await;
    assert!(policy.is_ok(), "update policy: {policy:?}");

    let one = post(
        &bob,
        &format!("apps/{app}/egress"),
        json!({ "egress": false }),
        Some(&mut bob_pk),
    )
    .await;
    assert!(one.is_ok(), "instance egress: {one:?}");

    let all = post(
        &bob,
        &format!("orgs/{org}/egress"),
        json!({ "egress": false }),
        Some(&mut bob_pk),
    )
    .await
    .unwrap_or_else(|e| panic!("organization egress: {e}"));
    assert_eq!(all["orgEgressOff"], true, "{all}");
}
