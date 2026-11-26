//! First-party connectors, called by an app guest (`connector.call`, WIT 0.5.8): the
//! release's signed manifest declares `connectors`, the guest names an operation and a
//! JSON input, and the node builds the request, attaches the key and returns only the
//! operation's declared fields. Anything outside the declaration, with egress off or with
//! no key bound, is refused before anything is sent.
//!
//! The connectors' hosts are answered in-process (`[outbound.testing] connectors`), with
//! fields no operation declares, so the test sees them cut. Builds the `apps/fixture-kv`
//! component, whose `connect` call calls a connector and answers what came back.

use calimero_client::{Client, Request};
use calimero_identity::testing::SoftAuthenticator;
use reqwest::Method;
use serde_json::{json, Value};

use crate::component_cluster::mpk_declaring_with;
use crate::harness::{home_org, kv_wasm, signed_up, typed, wire, Cluster};

const PACKAGE: &str = "com.calimero.connector-kv";
/// The bound Stripe key, never seen by the app.
const STRIPE_KEY: &str = "sk_test_cluster_key";

/// What the release declares: two Stripe operations with one metadata key, and OpenAI's
/// chat with one model (whose key no one binds).
fn declared() -> Value {
    json!({
        "stripe": {
            "operations": ["createPaymentIntent", "retrievePaymentIntent"],
            "metadata": ["order_id"],
        },
        "openai": { "operations": ["chatCompletion"], "model": ["gpt-4o-mini"] },
    })
}

/// A cluster whose operator allows the connectors' hosts (answered in-process) and binds
/// a Stripe key to the package, and Alice's instance of a release declaring
/// [`declared`], not approved yet.
async fn instance() -> (Cluster, Client, SoftAuthenticator, String, String) {
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let signer = calimero_bundle::derive_signer_id_did_key(publisher.verifying_key().as_bytes());
    let settings = calimero_outbound::OutboundSettings {
        enabled: true,
        hosts: vec!["api.stripe.com".to_owned(), "api.openai.com".to_owned()],
        testing: Some(calimero_outbound::OutboundTestSettings {
            echo: false,
            turnstile: false,
            connectors: true,
            secrets: [(
                "STRIPE_SECRET_KEY".to_owned(),
                calimero_outbound::TestSecretSettings {
                    value: STRIPE_KEY.to_owned(),
                    packages: vec![PACKAGE.to_owned()],
                    signers: vec![signer],
                    hosts: vec!["api.stripe.com".to_owned()],
                },
            )]
            .into(),
        }),
        ..calimero_outbound::OutboundSettings::default()
    };
    let service = calimero_outbound::OutboundService::from_settings(&settings).unwrap();
    let c = Cluster::start_with(3, |cfg| cfg.outbound.clone_from(&service)).await;
    let (alice, pk) = signed_up(&c, 0, "alice").await;
    let _ = alice
        .packages()
        .upload(
            mpk_declaring_with(
                PACKAGE,
                "0.1.0",
                &kv_wasm(),
                &publisher,
                &[],
                json!({ "connectors": declared() }),
            ),
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
    (c, alice, pk, org, app)
}

/// Approves what the release declares for Alice's organization: its connectors'
/// operation prefixes.
async fn approve(alice: &Client, pk: &mut SoftAuthenticator, org: &str) {
    let _ = alice
        .orgs()
        .outbound_approval(
            org,
            PACKAGE,
            Some(typed(json!("manifest"))),
            Some(false),
            Some(pk),
        )
        .await
        .unwrap();
}

/// The app's `connect` call: what the guest got back from `connector.call`, the answer's
/// JSON or `refused: <message>`.
async fn connect(cl: &Client, app: &str, connector: &str, operation: &str, input: Value) -> String {
    cl.apps()
        .call(
            app,
            "connect",
            Some(json!({
                "connector": connector,
                "operation": operation,
                "input": input.to_string(),
            })),
            None,
            None,
        )
        .await
        .unwrap()
        .result
        .as_str()
        .unwrap()
        .to_owned()
}

async fn get(cl: &Client, path: &str) -> Value {
    cl.transport()
        .json(Request::new(Method::GET, path))
        .await
        .unwrap()
}

/// The reproduction: a guest calls a declared Stripe operation end to end. Not approved,
/// it is refused; approved, the node sends the pinned request with the bound key, and the
/// guest gets only the declared fields; a declined charge comes back as its status and
/// code only; and the instance's outbound audit lists each call by the operation's path
/// and the key's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_guest_calls_a_declared_connector_operation_end_to_end() {
    let (_c, alice, mut pk, org, app) = instance().await;
    let charge = json!({ "amount": 2000, "currency": "usd", "metadata": { "order_id": "A-17" } });

    let waiting = connect(
        &alice,
        &app,
        "stripe",
        "createPaymentIntent",
        charge.clone(),
    )
    .await;
    assert!(
        waiting.starts_with("refused: AwaitingApproval: https://api.stripe.com/v1/payment_intents"),
        "{waiting}"
    );

    approve(&alice, &mut pk, &org).await;
    let created: Value =
        serde_json::from_str(&connect(&alice, &app, "stripe", "createPaymentIntent", charge).await)
            .unwrap();
    assert_eq!(
        created,
        json!({
            "id": "pi_test_1",
            "status": "requires_payment_method",
            "amount": 2000,
            "currency": "usd",
            "customer": null,
            "client_secret": "pi_test_1_secret_test",
            "metadata": { "order_id": "A-17" },
        }),
        "only the declared fields, never `latest_charge` or `livemode`"
    );
    let got = connect(
        &alice,
        &app,
        "stripe",
        "retrievePaymentIntent",
        json!({ "id": "pi_test_1" }),
    )
    .await;
    assert!(got.contains(r#""status":"succeeded""#), "{got}");
    assert!(!got.contains(STRIPE_KEY), "{got}");

    // The service's refusal: its status and code, never its message.
    let declined = connect(
        &alice,
        &app,
        "stripe",
        "createPaymentIntent",
        json!({ "amount": 402, "currency": "usd" }),
    )
    .await;
    assert_eq!(declined, "refused: stripe answered 402: card_declined");

    let log = get(&alice, &format!("apps/{app}/outbound-log")).await;
    let entries = log["entries"].as_array().unwrap();
    let sent: Vec<(&str, &str, &str, Value, Value)> = entries
        .iter()
        .map(|e| {
            (
                e["method"].as_str().unwrap(),
                e["host"].as_str().unwrap(),
                e["path"].as_str().unwrap(),
                e["status"].clone(),
                e["secrets"].clone(),
            )
        })
        .collect();
    assert!(
        sent.contains(&(
            "POST",
            "api.stripe.com",
            "/v1/payment_intents",
            json!(200),
            json!(["STRIPE_SECRET_KEY"])
        )),
        "{log}"
    );
    assert!(
        sent.contains(&(
            "GET",
            "api.stripe.com",
            "/v1/payment_intents/{id}",
            json!(200),
            json!(["STRIPE_SECRET_KEY"])
        )),
        "{log}"
    );
    assert!(
        sent.contains(&(
            "POST",
            "api.stripe.com",
            "/v1/payment_intents",
            json!(402),
            json!(["STRIPE_SECRET_KEY"])
        )),
        "{log}"
    );
}

/// Everything outside the declaration is refused before anything is sent: a connector or
/// an operation the release does not declare, a field the operation does not take, a call
/// with no key bound, any call while egress is off, and a call from a view. A release
/// declaring a connector or operation the node does not have is refused at install.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn connector_calls_outside_the_declaration_are_refused() {
    let (_c, alice, mut pk, org, app) = instance().await;
    approve(&alice, &mut pk, &org).await;

    assert_eq!(
        connect(&alice, &app, "slack", "postMessage", json!({})).await,
        "refused: slack is not a connector this release declares (its manifest's `connectors`)"
    );
    assert_eq!(
        connect(&alice, &app, "stripe", "createRefund", json!({})).await,
        "refused: createRefund is not an operation of the stripe connector this release declares"
    );
    assert_eq!(
        connect(
            &alice,
            &app,
            "stripe",
            "createPaymentIntent",
            json!({ "amount": 1, "currency": "usd", "description": "everything" }),
        )
        .await,
        "refused: stripe.createPaymentIntent takes no field \"description\""
    );
    let model = connect(
        &alice,
        &app,
        "openai",
        "chatCompletion",
        json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] }),
    )
    .await;
    assert_eq!(model, "refused: model is not a model allowed for openai");
    // Declared and approved, but no one bound an OpenAI key: refused, never sent bare.
    let unbound = connect(
        &alice,
        &app,
        "openai",
        "chatCompletion",
        json!({ "model": "gpt-4o-mini", "messages": [{ "role": "user", "content": "hi" }] }),
    )
    .await;
    assert!(
        unbound.starts_with("refused: secret OPENAI_API_KEY has no value for this app"),
        "{unbound}"
    );

    // A view may not call a connector.
    let e = alice
        .apps()
        .view(
            &app,
            "connectInView",
            Some(json!({ "connector": "stripe" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("HostDenied"), "{e}");

    // Egress off: the declared, approved, keyed call is refused too.
    let _off = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, &format!("apps/{app}/egress"))
                .json(json!({ "egress": false })),
            &mut pk,
        )
        .await
        .unwrap();
    let off = connect(
        &alice,
        &app,
        "stripe",
        "createPaymentIntent",
        json!({ "amount": 2000, "currency": "usd" }),
    )
    .await;
    assert!(
        off.starts_with("refused: EgressOff: the instance turned egress off"),
        "{off}"
    );

    // A release declaring what this node does not have is refused at install.
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    for (connectors, says) in [
        (
            json!({ "github": { "operations": ["createIssue"] } }),
            "github is not a connector of this node",
        ),
        (
            json!({ "stripe": { "operations": ["createRefund"] } }),
            "createRefund is not an operation of the stripe connector",
        ),
        (
            json!({ "stripe": { "operations": ["retrievePaymentIntent"], "metadata": ["k"] } }),
            "no operation of the stripe connector this release declares takes a metadata",
        ),
    ] {
        let e = alice
            .packages()
            .upload(
                mpk_declaring_with(
                    PACKAGE,
                    "0.2.0",
                    &kv_wasm(),
                    &publisher,
                    &[],
                    json!({ "connectors": connectors }),
                ),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("BAD_CONNECTORS"), "{e}");
        assert!(e.to_string().contains(says), "{e}");
    }
}
