//! The egress switch: an owner's hard guarantee that an app's code cannot send the data
//! it reads out of the cluster. The TEE keeps the data from the operator and from
//! Calimero, not from the app's own code, which may send it to any host its manifest
//! declares and its organization approved. With egress off, for the organization or one
//! instance, no request leaves for the apps it covers, whatever was approved; only an
//! owner or admin turns it off, and back on only on the node's consent page.
//!
//! Builds the `apps/fixture-kv` component (see `harness/cluster.rs`).

use calimero_client::types::OrgRole;
use calimero_client::{Client, Request};
use calimero_identity::testing::SoftAuthenticator;
use reqwest::Method;
use serde_json::{json, Value};

use crate::component_cluster::{eventually, mpk_declaring};
use crate::consent_page::{origin, Page};
use crate::harness::{home_org, kv_wasm, signed_up, typed, wire, Cluster, ORIGIN};

const PREFIX: &str = "https://echo.outbound.test/v1/";
const PACKAGE: &str = "com.calimero.v2-kv";

/// A cluster whose operator allows the echo host, and Alice's instance of a release that
/// declares it, its key set, with the prefix approved by her organization.
async fn approved_app() -> (Cluster, Client, SoftAuthenticator, String, String) {
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let signer = calimero_bundle::derive_signer_id_did_key(publisher.verifying_key().as_bytes());
    let settings = calimero_outbound::OutboundSettings {
        enabled: true,
        hosts: vec!["echo.outbound.test".to_owned()],
        testing: Some(calimero_outbound::OutboundTestSettings {
            echo: true,
            turnstile: false,
            // The fixture's `enrich` names `ECHO_KEY`.
            secrets: [(
                "ECHO_KEY".to_owned(),
                calimero_outbound::TestSecretSettings {
                    value: "s3cret-value".to_owned(),
                    packages: vec![PACKAGE.to_owned()],
                    signers: vec![signer],
                    hosts: vec!["echo.outbound.test".to_owned()],
                },
            )]
            .into(),
        }),
        ..calimero_outbound::OutboundSettings::default()
    };
    let service = calimero_outbound::OutboundService::from_settings(&settings).unwrap();
    let c = Cluster::start_with(3, |cfg| cfg.outbound.clone_from(&service)).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let _ = alice
        .packages()
        .upload(
            mpk_declaring(PACKAGE, "0.1.0", &kv_wasm(), &publisher, &[PREFIX]),
            None,
        )
        .await
        .unwrap();
    let org = home_org(&alice).await;
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "package": PACKAGE, "org": org }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .orgs()
        .outbound_approval(
            &org,
            PACKAGE,
            Some(typed(json!("manifest"))),
            Some(false),
            Some(&mut pk),
        )
        .await
        .unwrap();
    (c, alice, pk, org, app)
}

/// Run the app's `enrich` action for `key` (set to `value` first): `Ok` with what the echo
/// host answered, or `Err` with the reason the action failed.
async fn enrich(cl: &Client, app: &str, key: &str) -> Result<String, String> {
    let _set = cl
        .apps()
        .call(
            app,
            "set",
            Some(json!({ "key": key, "value": format!("{key}.com") })),
            None,
            None,
        )
        .await
        .unwrap();
    let id = cl
        .apps()
        .call(app, "startEnrich", Some(json!({ "key": key })), None, None)
        .await
        .unwrap()
        .result
        .as_str()
        .unwrap()
        .to_owned();
    // Done once the enrichment is written, or once this action's failure is.
    let mine = |f: &Value| f["enrich"].as_str().is_some_and(|f| f.starts_with(&id));
    let state = crate::harness::poll::eventually(
        crate::harness::poll::EVENTUALLY,
        "the action ends",
        || async {
            let got = eventually(cl, app, "enrichment", json!({ "key": key }), |_| true).await;
            let failed = eventually(cl, app, "actionFailures", json!({}), |_| true).await;
            if got.is_string() || mine(&failed) {
                Ok((got, failed))
            } else {
                Err((got, failed))
            }
        },
    )
    .await;
    match state {
        (Value::String(got), _) => Ok(got),
        (_, failed) => Err(failed["enrich"].as_str().unwrap().to_owned()),
    }
}

/// `POST path` with `body`, its passkey step-up answered with `pk` when given.
#[allow(
    clippy::result_large_err,
    reason = "the client's own error, passed through for the test to read its code"
)]
async fn post(
    cl: &Client,
    path: &str,
    body: Value,
    pk: Option<&mut SoftAuthenticator>,
) -> Result<Value, calimero_client::Error> {
    let req = Request::new(Method::POST, path).json(body);
    match pk {
        Some(pk) => cl.transport().confirmed(req, pk).await,
        None => cl.transport().json(req).await,
    }
}

async fn get(cl: &Client, path: &str) -> Value {
    cl.transport()
        .json(Request::new(Method::GET, path))
        .await
        .unwrap()
}

/// The reproduction: an app whose organization approved its host sends to it; with the
/// organization's egress off the same request is refused `EgressOff`, the approval still
/// recorded, and the organization approves nothing new. A plain member cannot turn egress
/// off, and nobody turns it back on outside the consent page.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn egress_off_refuses_an_approved_host_whatever_the_approval() {
    let (c, alice, mut pk, org, app) = approved_app().await;

    // On (the default): the approved request goes out.
    let got = enrich(&alice, &app, "acme").await.unwrap();
    assert!(got.starts_with("200 "), "{got}");

    // A plain member may not change the switch.
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let bob_id = wire(bob.auth().me().await.unwrap())["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .orgs()
        .set_member(&org, &bob_id, OrgRole::Member, Some(&mut pk))
        .await
        .unwrap();
    let e = post(
        &bob,
        &format!("orgs/{org}/egress"),
        json!({ "egress": false }),
        Some(&mut bob_pk),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    // The owner turns it off, with her passkey.
    let off = post(
        &alice,
        &format!("orgs/{org}/egress"),
        json!({ "egress": false }),
        Some(&mut pk),
    )
    .await
    .unwrap();
    assert_eq!(off["orgEgressOff"], true, "{off}");

    // The same approved request is refused now, on every replica the action may run on.
    let refused = enrich(&alice, &app, "beta").await.unwrap_err();
    assert!(
        refused.contains("EgressOff: its organization turned egress off"),
        "{refused}"
    );

    // The approval is still recorded, and stays overridden; a new one is refused.
    let o = get(&alice, &format!("orgs/{org}")).await;
    assert_eq!(o["egressOff"], true, "{o}");
    assert!(
        !o["outboundApprovals"].as_object().unwrap().is_empty(),
        "{o}"
    );
    let e = alice
        .orgs()
        .outbound_approval(
            &org,
            PACKAGE,
            Some(typed(json!("manifest"))),
            Some(true),
            Some(&mut pk),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("EGRESS_OFF"), "{e}");
    let refused = enrich(&alice, &app, "gamma").await.unwrap_err();
    assert!(refused.contains("EgressOff"), "{refused}");

    // The capability card says so.
    let card = get(&alice, &format!("apps/{app}/egress")).await;
    assert_eq!(card["egress"], "off", "{card}");
    assert_eq!(card["offBy"], "organization", "{card}");
    assert_eq!(
        card["summary"],
        "This app cannot send data outside the cluster: its egress is off."
    );
    assert_eq!(card["destinations"], json!([PREFIX]), "{card}");

    // Turning it back on is the consent page's alone, even for the owner.
    let e = post(
        &alice,
        &format!("orgs/{org}/egress"),
        json!({ "egress": true }),
        Some(&mut pk),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code(), Some("CONSENT_PAGE_REQUIRED"), "{e}");
}

/// One instance's switch: off, that instance sends nothing while the organization's
/// others still may. Turning it back on is a red change on the consent page, which takes
/// the person's passkey step-up there; then the instance sends again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_instances_egress_is_turned_off_and_back_on_only_on_the_page() {
    let (c, alice, mut pk, org, app) = approved_app().await;

    let off = post(
        &alice,
        &format!("apps/{app}/egress"),
        json!({ "egress": false }),
        Some(&mut pk),
    )
    .await
    .unwrap();
    assert_eq!(
        (off["app"].as_str(), &off["orgEgressOff"]),
        (Some(app.as_str()), &json!(false))
    );
    let refused = enrich(&alice, &app, "acme").await.unwrap_err();
    assert!(
        refused.contains("EgressOff: the instance turned egress off"),
        "{refused}"
    );
    let card = get(&alice, &format!("apps/{app}/egress")).await;
    assert_eq!(
        (&card["egress"], &card["offBy"]),
        (&json!("off"), &json!("instance"))
    );
    // The organization's own switch is still on.
    assert_eq!(
        get(&alice, &format!("orgs/{org}")).await["egressOff"],
        false
    );

    // On again: asked from the page, shown as red, and the page's approve asks for the
    // passkey (412) before anything changes.
    let made = post(
        &alice,
        "consent/requests",
        json!({
            "kind": "setEgress",
            "subject": { "org": org, "app": app, "egress": true },
            "returnTo": format!("{ORIGIN}/settings"),
        }),
        None,
    )
    .await
    .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["kind"], "setEgress", "{model}");
    assert_eq!(model["egress"]["class"], "red", "{model}");
    assert_eq!(model["attention"][0]["flag"], "egressOn", "{model}");
    assert_eq!(
        model["attention"][0]["items"],
        json!(["echo.outbound.test"]),
        "{model}"
    );
    let (code, asked) = page.post("approve", json!({})).await;
    assert_eq!(code, 412, "{asked}");
    assert_eq!(asked["error"], "CONFIRM_REQUIRED");
    assert_eq!(
        get(&alice, &format!("apps/{app}/egress")).await["egress"],
        "off",
        "nothing changed before the passkey"
    );
    let mut page_key = SoftAuthenticator::with_key(&pk, "localhost", &origin(&c.urls[0]));
    let ((code, done), args) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(
        args,
        json!({ "app": app, "egress": true, "__method": "__set_egress" })
    );
    let got = enrich(&alice, &app, "beta").await.unwrap();
    assert!(got.starts_with("200 "), "{got}");
}
