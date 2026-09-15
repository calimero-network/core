//! The stub answers what the contract page says it answers.
//!
//! These run against `calimero_client_stub::router`, the same value `main`
//! serves, so a passing test is a statement about the binary rather than about a
//! router assembled for the occasion.
//!
//! What is worth asserting here is narrow. The stub verifies no signatures, so
//! there is nothing cryptographic to test. What it must get right is the set of
//! places it fails a client, because those are the places a real node fails one:
//! a bad encoding, a warrant that does not commit to the request it arrived
//! with, a spent nonce, a write posted to the read path.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use calimero_account::{AccountId, Warrant};
use calimero_client_stub::{router, StubState};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EXECUTOR: [u8; 32] = [0x57; 32];
const CTX: &str = "11111111111111111111111111111111111111111111111111111111111111aa";

fn state(refuse_authorship: bool) -> Arc<StubState> {
    Arc::new(StubState {
        executor_account: AccountId::from(EXECUTOR),
        executor_account_hex: hex::encode(EXECUTOR),
        refuse_authorship,
        challenges: Mutex::new(Vec::new()),
        spent: Mutex::new(Vec::new()),
        tokens: AtomicU64::new(1),
    })
}

async fn call(st: &Arc<StubState>, req: Request<Body>) -> (StatusCode, Value) {
    let res = router(Arc::clone(st)).oneshot(req).await.expect("router");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request")
}

fn post(path: &str, body: &Value, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::from(body.to_string())).expect("request")
}

/// Mint a warrant the way a client must: hash the method and args the request
/// will actually carry, then sign over that.
fn warrant_for(method: &str, args: &Value, nonce: u64, executor: [u8; 32]) -> (String, String) {
    // Deterministic: the test asserts on shapes and refusals, and a fixed key
    // makes a failure reproduce identically.
    let sk = PrivateKey::from([0x11; 32]);
    let args_bytes = serde_json::to_vec(args).expect("args");
    let intent = Warrant::intent_hash(method, &args_bytes);
    let w = Warrant::sign(
        &sk,
        CTX.parse::<ContextId>().expect("ctx"),
        AccountId::from([0xaa; 32]),
        AccountId::from(executor),
        intent,
        nonce,
        u64::MAX,
    )
    .expect("sign");
    (
        hex::encode(borsh::to_vec(&w).expect("borsh")),
        hex::encode([0u8; 8]),
    )
}

// --- discovery ------------------------------------------------------------

#[tokio::test]
async fn discovery_reports_the_executor_a_warrant_must_name() {
    let st = state(false);
    let (status, body) = call(&st, get(&format!("/admin-api/contexts/{CTX}/intents"))).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["executorAccount"], hex::encode(EXECUTOR));
    assert_eq!(body["data"]["canAuthorOnBehalf"], true);
}

#[tokio::test]
async fn refuse_authorship_exposes_the_branch_clients_skip() {
    let st = state(true);
    let (_, body) = call(&st, get(&format!("/admin-api/contexts/{CTX}/intents"))).await;

    assert_eq!(
        body["data"]["canAuthorOnBehalf"], false,
        "a client must be able to exercise the default state of every real context"
    );
}

// --- session --------------------------------------------------------------

#[tokio::test]
async fn challenge_is_32_bytes_of_hex() {
    let st = state(false);
    let (status, body) = call(&st, get("/auth/challenge")).await;

    assert_eq!(status, StatusCode::OK);
    let c = body["challenge"].as_str().expect("challenge");
    assert_eq!(c.len(), 64, "LoginStatement.challenge is [u8; 32]");
    assert!(hex::decode(c).is_ok(), "challenge must decode as hex");
}

#[tokio::test]
async fn challenges_do_not_repeat() {
    let st = state(false);
    let (_, a) = call(&st, get("/auth/challenge")).await;
    let (_, b) = call(&st, get("/auth/challenge")).await;

    assert_ne!(
        a["challenge"], b["challenge"],
        "a repeated challenge would let a client replay one signature"
    );
}

#[tokio::test]
async fn login_refuses_a_statement_that_is_not_hex() {
    let st = state(false);
    let body = json!({ "loginStatement": "not-hex!", "accountProof": "00" });
    let (status, body) = call(&st, post("/auth/login", &body, None)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "MALFORMED_ENCODING");
}

// --- read -----------------------------------------------------------------

#[tokio::test]
async fn a_read_without_a_session_is_refused() {
    let st = state(false);
    let body = json!({ "method": "get", "argsJson": { "key": "a" } });
    let (status, body) = call(
        &st,
        post(&format!("/admin-api/contexts/{CTX}/query"), &body, None),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "NO_SESSION");
}

#[tokio::test]
async fn a_read_with_a_session_answers() {
    let st = state(false);
    let body = json!({ "method": "get", "argsJson": { "key": "a" } });
    let (status, body) = call(
        &st,
        post(
            &format!("/admin-api/contexts/{CTX}/query"),
            &body,
            Some("stub-session-1"),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["returns"]["method"], "get");
}

#[tokio::test]
async fn a_write_posted_to_the_read_path_is_refused_as_such() {
    let st = state(false);
    let body = json!({ "method": "set", "argsJson": { "key": "a", "value": "b" } });
    let (status, body) = call(
        &st,
        post(
            &format!("/admin-api/contexts/{CTX}/query"),
            &body,
            Some("stub-session-1"),
        ),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "reads fail closed on anything not declared read-only"
    );
    assert_eq!(body["error"]["code"], "NOT_READ_ONLY");
}

// --- delegated write ------------------------------------------------------

#[tokio::test]
async fn a_well_formed_warrant_is_accepted() {
    let st = state(false);
    let args = json!({ "key": "a", "value": "b" });
    let (warrant, proof) = warrant_for("set", &args, 1, EXECUTOR);
    let body = json!({
        "method": "set", "argsJson": args,
        "warrant": warrant, "authorProof": proof,
    });

    let (status, body) = call(
        &st,
        post(&format!("/admin-api/contexts/{CTX}/intents"), &body, None),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body["data"]["rootHash"].is_string());
}

#[tokio::test]
async fn a_warrant_committing_to_other_args_is_refused() {
    let st = state(false);
    let (warrant, proof) = warrant_for("set", &json!({ "key": "a" }), 1, EXECUTOR);

    // Same warrant, different args on the request: this is the shape of both a
    // relay swapping arguments and a client hashing the wrong bytes.
    let body = json!({
        "method": "set", "argsJson": { "key": "DIFFERENT" },
        "warrant": warrant, "authorProof": proof,
    });
    let (status, body) = call(
        &st,
        post(&format!("/admin-api/contexts/{CTX}/intents"), &body, None),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "INTENT_MISMATCH");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("domain_hash")),
        "the error must point a cross-language client at the hash construction"
    );
}

#[tokio::test]
async fn a_warrant_naming_another_executor_is_refused() {
    let st = state(false);
    let args = json!({ "key": "a" });
    let (warrant, proof) = warrant_for("set", &args, 1, [0x99; 32]);
    let body = json!({
        "method": "set", "argsJson": args,
        "warrant": warrant, "authorProof": proof,
    });

    let (status, body) = call(
        &st,
        post(&format!("/admin-api/contexts/{CTX}/intents"), &body, None),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "WRONG_EXECUTOR");
}

#[tokio::test]
async fn a_nonce_cannot_be_spent_twice() {
    let st = state(false);
    let args = json!({ "key": "a" });
    let (warrant, proof) = warrant_for("set", &args, 7, EXECUTOR);
    let body = json!({
        "method": "set", "argsJson": args,
        "warrant": warrant, "authorProof": proof,
    });

    let (first, _) = call(
        &st,
        post(&format!("/admin-api/contexts/{CTX}/intents"), &body, None),
    )
    .await;
    assert_eq!(first, StatusCode::OK);

    let (second, body) = call(
        &st,
        post(&format!("/admin-api/contexts/{CTX}/intents"), &body, None),
    )
    .await;
    assert_eq!(
        second,
        StatusCode::CONFLICT,
        "a warrant authorises one write, once"
    );
    assert_eq!(body["error"]["code"], "NONCE_SPENT");
}

#[tokio::test]
async fn a_malformed_warrant_names_what_is_wrong() {
    let st = state(false);
    let body = json!({
        "method": "set", "argsJson": {},
        "warrant": "zzzz", "authorProof": "00",
    });
    let (status, body) = call(
        &st,
        post(&format!("/admin-api/contexts/{CTX}/intents"), &body, None),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "MALFORMED_ENCODING");
}
