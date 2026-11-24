//! An API key made with follow-ups (`POST /auth/apikeys { then }`): confirm
//! calls the same passkey approves, `key:$key` written as the new key's
//! principal, run before the key is made, so a store's key and its grant take
//! one prompt.
//!
//! Builds `apps/fixture-notes-v2` from this checkout: its `set` is a confirm
//! method, its `get` a view.

use calimero_client::types::ViewsRequestConsistency;
use calimero_client::{Client, Request};
use calimero_identity::testing::SoftAuthenticator;
use reqwest::Method;
use serde_json::{json, Value};

use crate::harness;

use harness::{fixture, operator_sign, signed_up, typed, wire, Cluster};

/// A notes instance of `owner`'s.
async fn notes(owner: &Client, name: &str) -> String {
    let bundle = owner
        .packages()
        .bundle_put(fixture("apps/fixture-notes-v2"))
        .await
        .map(|b| b.hash)
        .unwrap();
    owner
        .transport()
        .json(Request::new(Method::POST, "apps").json(json!({ "name": name, "bundle": bundle })))
        .await
        .map(|v| v["appId"].as_str().unwrap().to_owned())
        .unwrap()
}

async fn note(client: &Client, app: &str, id: &str) -> Value {
    client
        .apps()
        .view(
            app,
            "get",
            Some(json!({ "id": id })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result
}

#[allow(
    clippy::result_large_err,
    reason = "the client's own error, passed through for the test to read its code"
)]
async fn create(client: &Client, body: &Value) -> calimero_client::Result<Value> {
    client
        .transport()
        .json(Request::new(Method::POST, "auth/apikeys").json(body.clone()))
        .await
}

async fn key_ids(client: &Client) -> Vec<Value> {
    let keys = client
        .transport()
        .json(Request::new(Method::GET, "auth/apikeys"))
        .await
        .unwrap();
    keys["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["keyId"].clone())
        .collect()
}

/// `body` with `pk`'s answer to the 412 `asked`.
fn approved(body: &Value, asked: &calimero_client::Error, pk: &mut SoftAuthenticator) -> Value {
    assert_eq!(asked.code(), Some("CONFIRM_REQUIRED"), "{asked}");
    let d = asked.details().unwrap();
    let challenge = calimero_identity::from_b64url(d["challenge"].as_str().unwrap()).unwrap();
    let mut body = body.clone();
    body["confirm"] =
        json!({ "challengeId": d["challengeId"], "assertion": pk.assert(&challenge) });
    body
}

fn set(app: &str, id: &str, title: &str, text: &str) -> Value {
    json!({ "app": app, "method": "set", "args": { "id": id, "title": title, "text": text } })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_and_its_grant_take_one_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let app = notes(&alice, "notes").await;

    // A plain key's 412 lists no follow-ups.
    let asked = create(&alice, &json!({ "label": "plain" }))
        .await
        .unwrap_err();
    assert_eq!(asked.details().unwrap().get("then"), None, "{asked}");

    // Only a value that is exactly `key:$key` names the key.
    let body = json!({
        "label": "store",
        "apps": [app],
        "then": [set(&app, "n1", "key:$key", "not key:$key")],
    });
    let asked = create(&alice, &body).await.unwrap_err();
    let shown = asked.details().unwrap()["then"].clone();
    let principal = shown[0]["args"]["title"].as_str().unwrap().to_owned();
    assert_eq!(shown[0]["args"]["text"], json!("not key:$key"), "{shown}");
    let retry = approved(&body, &asked, &mut pk);
    let made = create(&alice, &retry).await.unwrap();
    let key_id = made["keyId"].as_str().unwrap();
    assert_eq!(principal, format!("key:{key_id}"));
    assert_eq!(
        made["then"],
        json!([{ "index": 0, "result": null }]),
        "{made}"
    );
    assert_eq!(
        note(&alice, &app, "n1").await,
        json!(format!("{principal}\nnot key:$key"))
    );
    assert_eq!(key_ids(&alice).await, [json!(key_id)]);

    // The approval is spent: sent again, it neither runs the calls nor makes a key.
    let again = create(&alice, &retry).await.unwrap_err();
    assert_eq!(again.code(), Some("UNKNOWN_CHALLENGE"), "{again}");
    assert_eq!(key_ids(&alice).await, [json!(key_id)]);

    // The account's history shows the call the one prompt approved.
    let activity = alice
        .transport()
        .json(Request::new(Method::GET, "auth/activity"))
        .await
        .unwrap();
    assert!(
        activity["activity"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["method"] == "set" && e["subject"] == json!(app)),
        "{activity}"
    );

    let agent = c.client(1);
    agent.use_api_key(made["key"].as_str().unwrap()).await;
    assert_eq!(
        note(&agent, &app, "n1").await,
        json!(format!("{principal}\nnot key:$key"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn follow_ups_are_refused_before_the_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let app = notes(&alice, "notes").await;
    let other = notes(&alice, "other").await;
    let refused = |code: &'static str, then: Value, extra: Value| {
        let alice = &alice;
        let app = &app;
        async move {
            let mut body = json!({ "apps": [app], "then": then });
            if let (Some(b), Some(x)) = (body.as_object_mut(), extra.as_object()) {
                b.extend(x.clone());
            }
            let e = create(alice, &body).await.unwrap_err();
            assert_eq!(e.code(), Some(code), "{body}: {e}");
        }
    };

    // An app outside the key's own apps.
    refused(
        "BAD_FOLLOW_UP",
        json!([set(&other, "n", "t", "x")]),
        json!({}),
    )
    .await;
    // A view, and a method the app does not have.
    let get = json!([{ "app": app, "method": "get", "args": { "id": "n" } }]);
    refused("BAD_FOLLOW_UP", get, json!({})).await;
    let nope = json!([{ "app": app, "method": "nope" }]);
    refused("BAD_FOLLOW_UP", nope, json!({})).await;
    // The node's own commands.
    let upgrade = json!([{ "app": app, "method": "__upgrade" }]);
    refused("RESERVED_METHOD", upgrade, json!({})).await;
    // Past the count and the size.
    let five: Vec<Value> = (0..5)
        .map(|i| set(&app, &i.to_string(), "t", "x"))
        .collect();
    refused("BAD_FOLLOW_UP", json!(five), json!({})).await;
    let big = "x".repeat(65 * 1024);
    refused(
        "ARGS_TOO_LARGE",
        json!([set(&app, "n", "t", &big)]),
        json!({}),
    )
    .await;
    // Within the size as sent, past it once the key's id is written in.
    let pad = vec!["key:$key"; 5000];
    let grows = json!([{ "app": app, "method": "set",
        "args": { "id": "n", "title": "t", "text": "x", "pad": pad } }]);
    refused("ARGS_TOO_LARGE", grows, json!({})).await;
    // An upgrade key, and a key not scoped to apps.
    let one = json!([set(&app, "n", "t", "x")]);
    refused(
        "BAD_FOLLOW_UP",
        one.clone(),
        json!({ "upgrades": { "app": app } }),
    )
    .await;
    refused("BAD_FOLLOW_UP", one.clone(), json!({ "apps": null })).await;

    // Someone who does not reach the app, though the key names it.
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;
    let e = create(&bob, &json!({ "apps": [app], "then": one }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");

    // An API key, even one made for confirm methods, makes no key.
    let plain = json!({ "label": "agent", "allowConfirm": true });
    let asked = create(&alice, &plain).await.unwrap_err();
    let made = create(&alice, &approved(&plain, &asked, &mut pk))
        .await
        .unwrap();
    let agent = c.client(2);
    agent.use_api_key(made["key"].as_str().unwrap()).await;
    let e = create(&agent, &json!({ "apps": [app], "then": one }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    assert_eq!(key_ids(&alice).await, [made["keyId"].clone()]);
    assert_eq!(note(&alice, &app, "n").await, Value::Null);

    // A client cannot take the idempotency keys follow-ups run under.
    for (path, body) in [
        (
            format!("apps/{app}/intents"),
            json!({ "method": "set", "idempotencyKey": "node:apikey:k:0:x", "args": {} }),
        ),
        (
            format!("apps/{app}/methods/set"),
            json!({ "idempotencyKey": "node:apikey:k:0:x", "args": {} }),
        ),
    ] {
        let e = alice
            .transport()
            .json(Request::new(Method::POST, path.as_str()).json(body))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("RESERVED_IDEMPOTENCY_KEY"), "{path}: {e}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_or_failing_follow_up_makes_no_key() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let app = notes(&alice, "notes").await;

    // The retry asks for other calls than the passkey approved.
    let body = json!({ "apps": [app], "then": [set(&app, "n1", "key:$key", "a")] });
    let asked = create(&alice, &body).await.unwrap_err();
    let mut retry = approved(&body, &asked, &mut pk);
    retry["then"][0]["args"]["text"] = json!("b");
    let e = create(&alice, &retry).await.unwrap_err();
    assert_eq!(e.code(), Some("WRONG_CHALLENGE"), "{e}");
    assert_eq!(note(&alice, &app, "n1").await, Value::Null);

    // The second call fails: no key, and the answer names the call.
    let missing_args = json!({ "app": app, "method": "set", "args": { "id": "n3" } });
    let body = json!({
        "apps": [app],
        "then": [set(&app, "n2", "key:$key", "a"), missing_args],
    });
    let asked = create(&alice, &body).await.unwrap_err();
    let principal = asked.details().unwrap()["then"][0]["args"]["title"].clone();
    let e = create(&alice, &approved(&body, &asked, &mut pk))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FOLLOW_UP_FAILED"), "{e}");
    let d = e.details().unwrap();
    assert_eq!(
        (&d["index"], &d["app"], &d["method"]),
        (&json!(1), &json!(app), &json!("set")),
        "{d}"
    );
    assert_eq!(d["error"]["error"], json!("APP_ERROR"), "{d}");
    assert_eq!(d["applied"], json!([0]), "{d}");
    assert!(key_ids(&alice).await.is_empty());
    // The call before it applied, naming a key that never exists.
    assert_eq!(
        note(&alice, &app, "n2").await,
        json!(format!("{}\na", principal.as_str().unwrap()))
    );
}

/// The key quota is judged before any call runs: a quota filled between the
/// prompt and the retry leaves the calls unrun.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_key_quota_runs_no_follow_up() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let app = notes(&alice, "notes").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let mut quotas = ops.ops().quotas_get().await.map(wire).unwrap();
    quotas["maxApiKeysPerAccount"] = json!(1);
    let _ok = ops
        .ops()
        .quotas_set(typed(quotas), Some(&operator_sign))
        .await
        .unwrap();

    let body = json!({ "apps": [app], "then": [set(&app, "n1", "key:$key", "a")] });
    let asked = create(&alice, &body).await.unwrap_err();
    let plain = json!({ "label": "first" });
    let first = create(&alice, &plain).await.unwrap_err();
    let _made = create(&alice, &approved(&plain, &first, &mut pk))
        .await
        .unwrap();
    let e = create(&alice, &approved(&body, &asked, &mut pk))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("QUOTA_EXCEEDED"), "{e}");
    assert_eq!(note(&alice, &app, "n1").await, Value::Null);

    // Full before the prompt: refused before it.
    let e = create(&alice, &body).await.unwrap_err();
    assert_eq!(e.code(), Some("QUOTA_EXCEEDED"), "{e}");
}
