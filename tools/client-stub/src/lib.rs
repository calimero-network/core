//! A stub node serving the delegated-execution client contracts.
//!
//! # Why this exists
//!
//! The client for delegated execution needs a session, a read path and a warrant
//! builder. Two of those three do not exist on a real node yet
//! (calimero-network/core#3930, #3931), and the third is about to change shape
//! (#3933). Without something to build against, the client team either waits or
//! codes against a guess and reworks it.
//!
//! So this serves every exchange in `docs/.../build/delegated-execution-client.mdx`
//! at the shapes that page fixes. A client can be written end to end, and swapped
//! onto a real node as each issue closes.
//!
//! # What it deliberately does NOT do
//!
//! **It is not a security boundary and must never be deployed as one.** It does
//! not verify signatures, resolve accounts, or check membership: it accepts any
//! well-formed proof.
//!
//! What it *does* check is everything a client can get wrong on its own side —
//! the encodings, the field layouts, and the one computation a cross-language
//! client reliably gets wrong: `domain_hash`, where a missing length prefix
//! produces plausible digests that never verify. A warrant whose `intent_hash`
//! does not match the method and arguments it arrives with is refused here for
//! exactly the reason a node refuses it, so the client learns that at its desk
//! rather than against a relay.
//!
//! The rule for anything added here: fail the client in the same place a real
//! node would, and nowhere else. A stub that is stricter than the node teaches
//! people to work around it; a stub that is laxer lets bugs through to staging.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use calimero_account::Warrant;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// How long an issued challenge stays spendable. Matches the contract's "60
/// seconds or less"; the node's real value may be lower.
const CHALLENGE_TTL_SECS: u64 = 60;

/// Methods this stub answers reads for. A real node decides by the ABI's
/// `MethodIntent`; the stub carries a list so a client can exercise both the
/// success and the `409` branch without building a wasm.
const READ_ONLY_METHODS: &[&str] = &["get", "entries", "len", "get_unchecked", "get_result"];

pub struct StubState {
    pub executor_account: calimero_account::AccountId,
    pub executor_account_hex: String,
    pub refuse_authorship: bool,
    /// Issued-but-unspent challenges. A real node uses a stateless MAC; holding
    /// them is simpler here and single-use is the property that matters.
    pub challenges: Mutex<Vec<(String, u64)>>,
    /// Warrant nonces already accepted, per author device key. A real node keeps
    /// a bounded window; a growing set is fine for a stub and makes the replay
    /// refusal exact.
    pub spent: Mutex<Vec<(String, u64)>>,
    pub tokens: AtomicU64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// An error shaped like the node's, so a client's handling is exercised here.
fn fail(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}

// --- 1. discovery ---------------------------------------------------------

async fn intents_get(Path(context_id): Path<String>, State(st): State<Arc<StubState>>) -> Response {
    Json(json!({
        "data": {
            "executorAccount": st.executor_account_hex,
            "canAuthorOnBehalf": !st.refuse_authorship,
            "groupId": format!("group-of-{context_id}"),
            "grantedOnGroupId": Value::Null,
        }
    }))
    .into_response()
}

// --- 2. session -----------------------------------------------------------

async fn challenge(State(st): State<Arc<StubState>>) -> Response {
    // Unique, not unpredictable: a stub needs a client to be able to spend one
    // challenge once, and nothing here is a security property. 64 hex chars so
    // it decodes to the 32 bytes `LoginStatement.challenge` expects.
    let n = st.tokens.fetch_add(1, Ordering::Relaxed);
    let challenge = format!("{:032x}{:032x}", n, now());
    let expires_at = now() + CHALLENGE_TTL_SECS;

    let mut held = st.challenges.lock().expect("challenge lock");
    held.retain(|(_, exp)| *exp > now());
    held.push((challenge.clone(), expires_at));

    Json(json!({ "challenge": challenge, "expiresAt": expires_at })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginRequest {
    login_statement: String,
    account_proof: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    token: String,
    expires_at: u64,
}

async fn login(State(st): State<Arc<StubState>>, Json(req): Json<LoginRequest>) -> Response {
    // Shape only: both halves must be hex, because a client that sends base64 or
    // raw JSON here fails identically against a real node.
    for (name, value) in [
        ("loginStatement", &req.login_statement),
        ("accountProof", &req.account_proof),
    ] {
        if hex::decode(value).is_err() {
            return fail(
                StatusCode::BAD_REQUEST,
                "MALFORMED_ENCODING",
                &format!("{name} must be hex-encoded borsh"),
            );
        }
    }

    let n = st.tokens.fetch_add(1, Ordering::Relaxed);
    Json(LoginResponse {
        token: format!("stub-session-{n}"),
        expires_at: now() + 1800,
    })
    .into_response()
}

// --- 3. read --------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryRequest {
    method: String,
    args_json: Value,
}

async fn query(
    Path(context_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<QueryRequest>,
) -> Response {
    if !headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer stub-session-"))
    {
        return fail(
            StatusCode::UNAUTHORIZED,
            "NO_SESSION",
            "a read needs a session token from POST /auth/login",
        );
    }

    // The branch clients forget: a read of a method that is not read-only is a
    // write, and needs a warrant instead.
    if !READ_ONLY_METHODS.contains(&req.method.as_str()) {
        return fail(
            StatusCode::CONFLICT,
            "NOT_READ_ONLY",
            &format!(
                "method '{}' is not declared read-only; use POST .../intents with a warrant",
                req.method
            ),
        );
    }

    Json(json!({
        "data": {
            "returns": {
                "stub": true,
                "context": context_id,
                "method": req.method,
                "args": req.args_json,
            }
        }
    }))
    .into_response()
}

// --- 4. delegated write ---------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntentRequest {
    method: String,
    args_json: Value,
    warrant: String,
    author_proof: String,
}

async fn intents_post(
    Path(_context_id): Path<String>,
    State(st): State<Arc<StubState>>,
    Json(req): Json<IntentRequest>,
) -> Response {
    if hex::decode(&req.author_proof).is_err() {
        return fail(
            StatusCode::BAD_REQUEST,
            "MALFORMED_ENCODING",
            "authorProof must be hex-encoded borsh",
        );
    }

    let bytes = match hex::decode(&req.warrant) {
        Ok(b) => b,
        Err(_) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "MALFORMED_ENCODING",
                "warrant must be hex-encoded borsh",
            )
        }
    };

    let warrant: Warrant = match borsh::from_slice(&bytes) {
        Ok(w) => w,
        Err(e) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "MALFORMED_WARRANT",
                &format!("warrant is not a borsh-encoded Warrant: {e}"),
            )
        }
    };

    if st.refuse_authorship {
        return fail(
            StatusCode::FORBIDDEN,
            "NO_AUTHORSHIP_GRANT",
            "this stub is running with --refuse-authorship; a real node refuses \
             identically when canAuthorOnBehalf is false",
        );
    }

    if warrant.executor != st.executor_account {
        return fail(
            StatusCode::FORBIDDEN,
            "WRONG_EXECUTOR",
            "warrant.executor does not name this node's executor account; take it \
             from GET .../intents rather than composing it",
        );
    }

    // The check worth having a stub for. `intent_hash` is domain-separated and
    // length-prefixed, so a client that concatenates without the u64 length
    // prefixes produces a digest that looks fine and never matches.
    let args = serde_json::to_vec(&req.args_json).unwrap_or_default();
    let expected = Warrant::intent_hash(&req.method, &args);
    if warrant.intent_hash != expected {
        return fail(
            StatusCode::BAD_REQUEST,
            "INTENT_MISMATCH",
            &format!(
                "warrant.intent_hash does not commit to this method and args.\n  \
                 expected {}\n  got      {}\n\
                 Check domain_hash: SHA256(u64le(len(domain)) || domain || \
                 (u64le(len(part)) || part)*) over [method, args], domain \
                 'calimero.warrant.intent.v1'.",
                hex::encode(expected),
                hex::encode(warrant.intent_hash),
            ),
        );
    }

    let device = hex::encode(AsRef::<[u8; 32]>::as_ref(&warrant.author_device_key));
    let mut spent = st.spent.lock().expect("spent lock");
    if spent.contains(&(device.clone(), warrant.nonce)) {
        return fail(
            StatusCode::CONFLICT,
            "NONCE_SPENT",
            "this (author device, nonce) pair was already used; a warrant is single-use",
        );
    }
    spent.push((device, warrant.nonce));

    Json(json!({
        "data": {
            "rootHash": format!("{:064x}", warrant.nonce),
            "returns": { "stub": true, "method": req.method, "args": req.args_json },
        }
    }))
    .into_response()
}

/// The stub's routes, as one value so tests exercise exactly what `main` serves.
///
/// A test that wires its own router proves nothing about the binary; this is the
/// only definition of what the stub answers.
pub fn router(state: Arc<StubState>) -> Router {
    Router::new()
        .route("/auth/challenge", get(challenge))
        .route("/auth/login", post(login))
        .route(
            "/admin-api/contexts/{context_id}/intents",
            get(intents_get).post(intents_post),
        )
        .route("/admin-api/contexts/{context_id}/query", post(query))
        .with_state(state)
}
