//! The generated route layer (`calimero_client::routes`, from `docs/public/openapi.json`)
//! against an in-process node: a step-up route answered with the passkey given, an operator
//! route signed with the operator key, raw bytes up and down, and a field whose `null` means
//! something other than its absence.

mod common;

use calimero_client::types::{AppCreateRequest, OpsQuotasSetRequest, TeamJoin};
use common::{component, operator_sign, signed_up, Cluster};
use serde_json::json;

/// `x-calimero-step-up`: without a passkey the node's `412` is the answer and nothing
/// changes; with one the generated method answers the challenge and the route runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_up_route_is_answered_with_the_passkey_given() {
    let c = Cluster::start(1).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let org = alice.orgs().create("acme", None, None).await.unwrap().org_id;

    let refused = alice.orgs().delete(&org, None).await.unwrap_err();
    assert_eq!(refused.code(), Some("CONFIRM_REQUIRED"), "{refused:?}");
    assert_eq!(alice.orgs().get(&org).await.unwrap().name, "acme");

    let _deleted = alice.orgs().delete(&org, Some(&mut pk)).await.unwrap();
    assert!(alice.orgs().get(&org).await.is_err(), "the org is gone");
}

/// `x-calimero-operator`: without the operator's signer the node's `428` is the answer;
/// with it the generated method signs the statement the node names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operator_route_is_signed_with_the_operator_key() {
    let c = Cluster::start(1).await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let quotas = OpsQuotasSetRequest {
        max_concurrent_runs_per_org: Some(3),
        ..OpsQuotasSetRequest::default()
    };

    let refused = ops
        .ops()
        .quotas_set(quotas.clone(), None)
        .await
        .unwrap_err();
    assert_eq!(
        refused.code(),
        Some("OPERATOR_SIGNATURE_REQUIRED"),
        "{refused:?}"
    );

    let _set = ops
        .ops()
        .quotas_set(quotas, Some(&operator_sign))
        .await
        .unwrap();
    assert_eq!(
        ops.ops()
            .quotas_get()
            .await
            .unwrap()
            .max_concurrent_runs_per_org,
        3
    );
}

/// Raw bodies follow the document's media types: a bundle up as `application/wasm`, a blob
/// up with the caller's content type and back as bytes; calls and views answer typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_bodies_go_up_and_come_back_as_bytes() {
    let c = Cluster::start(1).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(component("kv"))
        .await
        .unwrap()
        .hash;
    let app = alice
        .apps()
        .create(AppCreateRequest {
            name: Some("kv".into()),
            bundle: Some(bundle),
            ..AppCreateRequest::default()
        })
        .await
        .unwrap()
        .app_id;

    let called = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();
    let viewed = alice
        .apps()
        .view(&app, "get", Some(json!({ "key": "k" })), None, None)
        .await
        .unwrap();
    assert_eq!(viewed.result, json!("v"));
    assert!(viewed.index >= called.index);

    let blob = alice
        .apps()
        .blob_upload(&app, b"hello".to_vec(), None)
        .await
        .unwrap();
    assert_eq!(blob.size, 5);
    let back = alice
        .apps()
        .blob_download(&app, &blob.id, None)
        .await
        .unwrap();
    assert_eq!(back, b"hello");
}

/// A field that may be absent or `null` (`Option<Option<T>>`): absent leaves a team where it
/// is, `null` moves it to the top of the organization.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_null_is_sent_and_an_absent_field_is_not() {
    let c = Cluster::start(1).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let org = alice.orgs().create("acme", None, None).await.unwrap().org_id;
    let eng = alice
        .orgs()
        .team_create(&org, "eng", TeamJoin::Invite, None)
        .await
        .unwrap()
        .team_id;
    let web = alice
        .orgs()
        .team_create(&org, "web", TeamJoin::Invite, Some(&eng))
        .await
        .unwrap()
        .team_id;

    let _renamed = alice
        .orgs()
        .team_update(&org, &web, Some("frontend"), None, None, None)
        .await
        .unwrap();
    let team = alice.orgs().team_get(&org, &web).await.unwrap();
    assert_eq!(team.name, "frontend");
    assert_eq!(
        team.parent.as_deref(),
        Some(eng.as_str()),
        "absent: unchanged"
    );

    let _moved = alice
        .orgs()
        .team_update(&org, &web, None, None, Some(None), None)
        .await
        .unwrap();
    let team = alice.orgs().team_get(&org, &web).await.unwrap();
    assert_eq!(team.parent, None, "null: to the top");
}
