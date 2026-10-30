//! The node's HTTP API. Clients reach it inside the sealed channel; only
//! `/attestation` and `/health` are served in the clear.
//!
//! - Client: `/auth/*`, `/bundles`, `/apps`, `/apps/{app}/{intents,views,events,upgrade,access}`,
//!   `/orgs/*`, `/invites/{token}/redeem`, `/consent/requests`, `/consent/{id}/result`.
//! - The consent page (`consent_page`) is served in the clear, outside this
//!   router, on the node's own origin.
//! - Internal (peers, cluster secret): `/internal/*`.
//! - Ops (mdma, operator token): `/ops/*`.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ops::Bound;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes as Body;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use calimero_identity::{
    b64url, confirm_challenge, mint_session, peek_session, random, sha256, verify_assertion,
    verify_registration, verify_session, Assertion, Registration, RelyingParty, SessionClaims,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::app::{Applied, Status};
use crate::attest::{kms_report_data, quote, report_data, AttestationDoc};
use crate::error::ApiError;
use crate::node::{CallRequest, Node, ViewRequest};
use crate::report::{self, Reader, MAX_REPORT_PAGE};
use crate::system::{SystemCmd, SystemState};
use crate::types::{
    now_secs, Access, Account, ApiKeyRecord, Caller, Invite, OrgRole, RefreshRecord, TeamJoin,
};

const SESSION_TTL: u64 = 15 * 60;
const REFRESH_TTL: u64 = 30 * 24 * 60 * 60;
const CHALLENGE_TTL: u64 = 5 * 60;

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

/// A ceremony a node started. Its id names that node (`{node}.{random}`): the
/// ceremony may finish on any node, which takes the challenge from the one that
/// holds it (`internal/challenge/take`), so a challenge is still used once.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Pending {
    Register {
        challenge: [u8; 32],
        name: String,
        expires: u64,
    },
    Login {
        challenge: [u8; 32],
        expires: u64,
    },
    AddPasskey {
        challenge: [u8; 32],
        account: String,
        expires: u64,
    },
    Confirm {
        challenge: [u8; 32],
        app: String,
        method: String,
        args: Vec<u8>,
        account: String,
        expires: u64,
    },
}

impl Pending {
    const fn expires(&self) -> u64 {
        match self {
            Self::Register { expires, .. }
            | Self::Login { expires, .. }
            | Self::AddPasskey { expires, .. }
            | Self::Confirm { expires, .. } => *expires,
        }
    }
}

/// The most passkey ceremonies a node holds open at once. Each lives at most
/// `CHALLENGE_TTL`; past this the node answers 429 instead of growing without
/// bound (Xil's review, issue 12).
pub const MAX_PENDING_CHALLENGES: usize = 10_000;

fn put_pending(node: &Node, pending: Pending) -> Result<String, ApiError> {
    let id = format!("{}.{}", node.config.node_id, b64url(&random::<16>()));
    let mut c = node
        .challenges
        .lock()
        .map_err(|_| ApiError::internal("challenge store poisoned"))?;
    let now = now_secs();
    c.retain(|_, p| p.expires() > now);
    if c.len() >= MAX_PENDING_CHALLENGES {
        return Err(ApiError::new(
            429,
            "RATE_LIMITED",
            "too many sign-ins in progress; try again shortly",
        ));
    }
    let _prev = c.insert(id.clone(), pending);
    Ok(id)
}

fn unknown_challenge() -> ApiError {
    ApiError::bad_request(
        "UNKNOWN_CHALLENGE",
        "unknown or used challenge; start the ceremony again",
    )
}

/// The ceremony `id` names, taken from this node or the node that began it.
async fn take_pending(node: &Node, id: &str) -> Result<Pending, ApiError> {
    let holder = id
        .split_once('.')
        .and_then(|(n, _)| n.parse::<u64>().ok())
        .ok_or_else(unknown_challenge)?;
    let p = if holder == node.config.node_id {
        take_local(node, id)?
    } else {
        let addr = node
            .sys(|s| s.nodes.get(&holder).map(|n| n.addr.clone()))
            .ok_or_else(unknown_challenge)?;
        let v = node
            .peer_post(&addr, "internal/challenge/take", &json!({ "id": id }))
            .await
            .map_err(|e| match e.downcast::<ApiError>() {
                Ok(api) => api,
                Err(e) => ApiError::unavailable(&format!(
                    "the node that began this ceremony did not answer: {e}"
                )),
            })?;
        serde_json::from_value(v).map_err(|e| ApiError::internal(&e.to_string()))?
    };
    if p.expires() <= now_secs() {
        return Err(ApiError::bad_request(
            "EXPIRED_CHALLENGE",
            "the challenge expired",
        ));
    }
    Ok(p)
}

fn take_local(node: &Node, id: &str) -> Result<Pending, ApiError> {
    node.challenges
        .lock()
        .ok()
        .and_then(|mut c| c.remove(id))
        .ok_or_else(unknown_challenge)
}

#[derive(Deserialize)]
struct TakeChallengeBody {
    id: String,
}

/// A peer finishes a ceremony this node began: hand it the challenge, once.
async fn internal_challenge_take(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<TakeChallengeBody>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let p = take_local(&node, &body.id)?;
    Ok(Json(
        serde_json::to_value(p).map_err(|e| ApiError::internal(&e.to_string()))?,
    ))
}

/// The package whose frontend origin a ceremony claims (unverified: the
/// ceremony is then checked against that origin), and the origin.
fn claimed_app(node: &Node, client_data_json: &str) -> Option<(String, String)> {
    let origin = calimero_identity::claimed_origin(client_data_json)?;
    let package = node.app_origin_package(&origin)?;
    Some((package, origin))
}

/// The relying party for a ceremony that may come from `app`'s frontend
/// origin as well as the cluster's own.
fn rp_with(base: RelyingParty, app: Option<&(String, String)>) -> RelyingParty {
    let mut rp = base;
    if let Some((_, origin)) = app {
        rp.origins.push(origin.clone());
    }
    rp
}

fn rp(node: &Node) -> RelyingParty {
    RelyingParty {
        id: node.config.rp_id.clone(),
        origins: node.config.origins.clone(),
        require_uv: false,
    }
}

/// A confirmation (step-up) needs the user verified (PIN or biometric), not
/// just present: a touch alone is not consent to a confirm method (Xil's
/// review, issue 6).
pub(crate) fn rp_step_up(node: &Node) -> RelyingParty {
    RelyingParty {
        require_uv: true,
        ..rp(node)
    }
}

/// Who is calling, from the `Authorization` header.
#[derive(Clone, Debug)]
pub struct Principal {
    pub caller: Caller,
    pub session: Option<SessionClaims>,
    pub api_key: Option<ApiKeyRecord>,
    /// The node's consent page acting for the account a consent request
    /// names (`consent_page`): no session of its own, so every action it
    /// takes is a step-up, and the step-up may come from the node's own
    /// origins as well as the cluster's.
    pub consent_page: bool,
}

impl Principal {
    pub(crate) fn account(&self) -> Result<String, ApiError> {
        self.caller
            .account()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::unauthenticated("sign in first"))
    }

    /// The package a session opened on that package's frontend origin is
    /// bound to: it reaches that package's apps only.
    pub(crate) fn bound_app(&self) -> Option<&str> {
        self.session.as_ref().and_then(|s| s.app.as_deref())
    }

    /// Refuse a package-bound session: credentials, keys, approvals and
    /// upgrades are the account holder's, on the cluster's own origin.
    fn unbound(&self) -> Result<(), ApiError> {
        match self.bound_app() {
            Some(_) => Err(ApiError::forbidden(
                "a session opened on an app's frontend can use that app only; do this on the cluster's own page",
            )),
            None => Ok(()),
        }
    }

    /// Whether this principal may touch `app` at all: a package-bound
    /// session only an app running a release of its package.
    pub(crate) fn within(
        &self,
        node: &Node,
        app: &crate::types::AppRecord,
    ) -> Result<(), ApiError> {
        match self.bound_app() {
            None => Ok(()),
            Some(pkg)
                if node
                    .release_of(&app.bundle)
                    .is_some_and(|r| r.at.package == pkg) =>
            {
                Ok(())
            }
            Some(_) => Err(ApiError::not_found("no such app")),
        }
    }
}

/// [`person`], and never a package-bound session.
async fn holder(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let p = person(node, headers).await?;
    p.unbound()?;
    Ok(p)
}

async fn principal(node: &Node, headers: &HeaderMap) -> Result<Option<Principal>, ApiError> {
    let Some(value) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let token = value.strip_prefix("Bearer ").unwrap_or(value).trim();
    // A credential is judged on the cluster's latest system state (a read
    // index): a session or key revoked through another node is refused here
    // at once, not once this replica catches up.
    node.sys_fresh().await;
    if let Some(_key) = token.strip_prefix("cal_") {
        let hash = calimero_identity::api_key_hash(token);
        let rec = node
            .sys_until(|s| s.api_keys.get(&hash).cloned())
            .await
            .ok_or_else(|| ApiError::unauthenticated("unknown API key"))?;
        if rec.expires_at <= now_secs() {
            return Err(ApiError::unauthenticated("API key expired"));
        }
        return Ok(Some(Principal {
            caller: Caller::ApiKey {
                key: rec.key_id.clone(),
                account: rec.account.clone(),
            },
            session: None,
            api_key: Some(rec),
            consent_page: false,
        }));
    }
    let claims = peek_session(token).map_err(|_| ApiError::unauthenticated("malformed token"))?;
    let key = node
        .sys_until(|s| s.session_key(claims.key_version))
        .await
        .ok_or_else(|| ApiError::unauthenticated("token signed with an unknown key"))?;
    let claims = verify_session(&key, token, now_secs())
        .map_err(|e| ApiError::unauthenticated(&e.to_string()))?;
    if node.sys(|s| s.revoked_sessions.contains(&claims.session)) {
        return Err(ApiError::unauthenticated("session revoked"));
    }
    // A session opened with a passkey the account no longer has is over.
    if let Some(passkey) = &claims.passkey {
        let still = node.sys(|s| s.credentials.get(passkey) == Some(&claims.account));
        if !still {
            return Err(ApiError::unauthenticated(
                "the passkey this session used was removed",
            ));
        }
    }
    Ok(Some(Principal {
        caller: Caller::Account {
            account: claims.account.clone(),
        },
        session: Some(claims),
        api_key: None,
        consent_page: false,
    }))
}

pub(crate) async fn signed_in(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
    principal(node, headers)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("sign in first"))
}

/// Who may install code or create apps: a passkey session, or an API key
/// scoped to no app and no method and not read-only. A key handed to one app
/// (or read-only) must not install bundles or start new apps (Xil's review,
/// issue 7).
async fn installer(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let p = signed_in(node, headers).await?;
    p.unbound()?;
    if let Some(k) = &p.api_key {
        if k.read_only || k.apps.is_some() || k.methods.is_some() {
            return Err(ApiError::forbidden(
                "this API key is scoped; installing code and creating apps needs an unscoped key or a passkey session",
            ));
        }
    }
    Ok(p)
}

/// Only a passkey session, not an API key, may manage credentials.
pub(crate) async fn person(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let p = signed_in(node, headers).await?;
    if p.session.is_none() {
        return Err(ApiError::forbidden(
            "this needs a passkey session, not an API key",
        ));
    }
    Ok(p)
}

async fn new_session(node: &Node, account: &str, passkey: &str, app: Option<String>) -> ApiResult {
    let (version, key) = node
        .sys(|s| s.current_session_key())
        .ok_or_else(|| ApiError::unavailable("the cluster has no session key yet"))?;
    let session = b64url(&random::<16>());
    let expires_at = now_secs() + SESSION_TTL;
    let token = mint_session(
        &key,
        &SessionClaims {
            account: account.to_owned(),
            session: session.clone(),
            expires_at,
            key_version: version,
            passkey: Some(passkey.to_owned()),
            app: app.clone(),
        },
    );
    let refresh = format!("rt_{}", b64url(&random::<32>()));
    let _ok = node
        .sys_write(&SystemCmd::CreateRefresh {
            hash: b64url(&sha256(refresh.as_bytes())),
            record: RefreshRecord {
                account: account.to_owned(),
                session,
                expires_at: now_secs() + REFRESH_TTL,
                passkey: Some(passkey.to_owned()),
                app,
            },
        })
        .await?;
    Ok(Json(json!({
        "accountId": account,
        "sessionToken": token,
        "expiresAt": expires_at,
        "refreshToken": refresh,
    })))
}

// ---------------------------------------------------------------- public

async fn health(State(node): State<Arc<Node>>) -> Json<Value> {
    let s = node.system.status();
    Json(
        json!({ "ok": true, "node": node.config.node_id, "systemLeader": s.leader, "release": node.config.release }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttestQuery {
    nonce: String,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    app_hash: Option<String>,
    /// `receipts`: attest this node's receipt key instead of its transport key.
    /// `kms`: the transport key and what this node verified of its KMS, when
    /// it has a record (otherwise the transport key alone, as with none).
    #[serde(default)]
    purpose: Option<String>,
}

async fn attestation(State(node): State<Arc<Node>>, Query(q): Query<AttestQuery>) -> ApiResult {
    let nonce: [u8; 32] = hex::decode(&q.nonce)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| ApiError::bad_request("BAD_NONCE", "nonce must be 32 bytes hex"))?;
    let app_hash: [u8; 32] = match (&q.app, &q.app_hash) {
        (Some(app), _) => {
            let rec = node.app(app).await?;
            hex::decode(&rec.bundle)
                .ok()
                .and_then(|v| v.try_into().ok())
                .unwrap_or([0; 32])
        }
        (None, Some(h)) => hex::decode(h)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| ApiError::bad_request("BAD_HASH", "appHash must be 32 bytes hex"))?,
        (None, None) => [0; 32],
    };
    let transport_key = node.transport.public_key();
    let receipts = q.purpose.as_deref() == Some("receipts");
    // Only when asked: a client that does not know the KMS binding keeps the
    // report data it has always checked.
    let kms = (q.purpose.as_deref() == Some("kms") && !node.config.kms_keys.is_empty())
        .then(|| node.config.kms_keys.clone());
    // The rules the cluster was deployed with, as this replica's state holds
    // them: bound with the receipt key, so a user proves both at once.
    let governance = node.sys(crate::system::SystemState::governance);
    let report = if receipts {
        let mut r = [0; 64];
        r[..32].copy_from_slice(&nonce);
        r[32..].copy_from_slice(&calimero_client::attest::receipt_key_binding(
            &node.receipt_public(),
            &governance,
        ));
        r
    } else if let Some(kms) = &kms {
        kms_report_data(&nonce, &app_hash, &transport_key, kms)
    } else {
        report_data(&nonce, &app_hash, &transport_key)
    };
    let q =
        quote(node.config.attestation, report).map_err(|e| ApiError::internal(&e.to_string()))?;
    let doc = AttestationDoc {
        quote: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, q),
        transport_key: hex::encode(transport_key),
        app_hash: hex::encode(app_hash),
        nonce: hex::encode(nonce),
        release: node.config.attested_release(),
        cluster: node.config.cluster.clone(),
        node: node.config.node_id,
        mock: node.config.attestation == crate::attest::Mode::Mock,
        receipt_key: receipts.then(|| hex::encode(node.receipt_public())),
        governance: receipts.then_some(governance),
        kms,
    };
    Ok(Json(serde_json::to_value(doc).unwrap_or_default()))
}

/// Every replica of an app attests its receipt key over `nonce`: what a user
/// checks to know the whole group holding the app is genuine, and which keys
/// sign its receipts.
async fn app_attestation(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<AttestQuery>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let mut replicas = Vec::new();
    for id in &app.replicas {
        let Some(addr) = node.sys(|s| s.nodes.get(id).map(|n| n.addr.clone())) else {
            continue;
        };
        let doc: Result<Value, String> = async {
            let mut url: url::Url = addr.parse().map_err(|e| format!("{e}"))?;
            url = url.join("attestation").map_err(|e| format!("{e}"))?;
            let _q = url
                .query_pairs_mut()
                .append_pair("nonce", &q.nonce)
                .append_pair("purpose", "receipts");
            node.http
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| e.to_string())?
                .json::<Value>()
                .await
                .map_err(|e| e.to_string())
        }
        .await;
        replicas.push(match doc {
            Ok(d) => json!({ "node": id, "attestation": d }),
            Err(e) => json!({ "node": id, "error": e }),
        });
    }
    Ok(Json(json!({ "appId": app_id, "replicas": replicas })))
}

// ---------------------------------------------------------------- auth

#[derive(Deserialize)]
struct RegisterBegin {
    #[serde(default)]
    name: String,
}

async fn register_begin(
    State(node): State<Arc<Node>>,
    Json(body): Json<RegisterBegin>,
) -> ApiResult {
    let challenge = random::<32>();
    let user_id = random::<16>();
    let name = if body.name.is_empty() {
        "Calimero user".to_owned()
    } else {
        body.name
    };
    let id = put_pending(
        &node,
        Pending::Register {
            challenge,
            name: name.clone(),
            expires: now_secs() + CHALLENGE_TTL,
        },
    )?;
    Ok(Json(json!({
        "challengeId": id,
        "options": {
            "challenge": b64url(&challenge),
            "rp": { "id": node.config.rp_id, "name": "Calimero" },
            "user": { "id": b64url(&user_id), "name": name, "displayName": name },
            "pubKeyCredParams": [ { "type": "public-key", "alg": -7 }, { "type": "public-key", "alg": -8 } ],
            "timeout": 60000,
            "attestation": "none",
            "authenticatorSelection": { "residentKey": "required", "userVerification": "preferred" },
        }
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterFinish {
    challenge_id: String,
    registration: Registration,
}

async fn register_finish(
    State(node): State<Arc<Node>>,
    Json(body): Json<RegisterFinish>,
) -> ApiResult {
    let Pending::Register {
        challenge, name, ..
    } = take_pending(&node, &body.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a registration challenge",
        ));
    };
    // Signing up on a package's frontend opens a session bound to it.
    let app = claimed_app(&node, &body.registration.client_data_json);
    let cred = verify_registration(
        &rp_with(rp(&node), app.as_ref()),
        &challenge,
        &body.registration,
    )
    .map_err(|e| ApiError::bad_request("BAD_REGISTRATION", &e.to_string()))?;
    let account = typed_id::<32>();
    let _ok = node
        .sys_write(&SystemCmd::CreateAccount {
            account: Account {
                account_id: account.clone(),
                name,
                created_at: now_secs(),
                passkeys: vec![cred.clone()],
                // The release this account began on holds its apps until the
                // owner approves another.
                approved_releases: [node.own_release()].into_iter().collect(),
                outbound_approvals: std::collections::BTreeMap::new(),
                passkey_labels: std::collections::BTreeMap::new(),
                aliases: std::collections::BTreeMap::new(),
            },
        })
        .await?;
    new_session(&node, &account, &cred.id, app.map(|(pkg, _)| pkg)).await
}

async fn login_begin(State(node): State<Arc<Node>>) -> ApiResult {
    let challenge = random::<32>();
    let id = put_pending(
        &node,
        Pending::Login {
            challenge,
            expires: now_secs() + CHALLENGE_TTL,
        },
    )?;
    Ok(Json(json!({
        "challengeId": id,
        "options": { "challenge": b64url(&challenge), "rpId": node.config.rp_id, "userVerification": "preferred", "timeout": 60000 }
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssertionBody {
    challenge_id: String,
    assertion: Assertion,
}

/// Verify an assertion for `challenge` and move the credential's counter.
/// `bound`: the ceremony may come from that package's frontend origin only;
/// `None` with `any_app`, from any package's (a sign-in there opens a session
/// bound to it). Answers the account and the package whose origin it came from.
async fn check_assertion(
    node: &Node,
    challenge: &[u8; 32],
    assertion: &Assertion,
    account: Option<&str>,
    step_up: bool,
    bound: Option<&str>,
    any_app: bool,
) -> Result<(String, Option<String>), ApiError> {
    let base = if step_up { rp_step_up(node) } else { rp(node) };
    check_assertion_for(node, base, challenge, assertion, account, bound, any_app).await
}

/// [`check_assertion`] against the relying party `base` (the cluster's own
/// origins, or the consent page's).
async fn check_assertion_for(
    node: &Node,
    base: RelyingParty,
    challenge: &[u8; 32],
    assertion: &Assertion,
    account: Option<&str>,
    bound: Option<&str>,
    any_app: bool,
) -> Result<(String, Option<String>), ApiError> {
    let app = claimed_app(node, &assertion.client_data_json)
        .filter(|(pkg, _)| any_app || bound == Some(pkg.as_str()));
    let (owner, cred) = node
        .sys_until(|s| {
            s.account_of_credential(&assertion.credential_id)
                .map(|(a, c)| (a.account_id.clone(), c.clone()))
        })
        .await
        .ok_or_else(|| ApiError::unauthenticated("unknown passkey"))?;
    if account.is_some_and(|a| a != owner) {
        return Err(ApiError::forbidden(
            "this passkey belongs to another account",
        ));
    }
    let rp = rp_with(base, app.as_ref());
    let count = verify_assertion(&rp, challenge, &cred, assertion)
        .map_err(|e| ApiError::unauthenticated(&e.to_string()))?;
    if count != 0 {
        let _ok = node
            .sys_write(&SystemCmd::SetSignCount {
                credential_id: cred.id.clone(),
                count,
            })
            .await
            .map_err(|e| {
                if e.code == "REJECTED" {
                    ApiError::unauthenticated("signature counter did not advance")
                } else {
                    e
                }
            })?;
    }
    Ok((owner, app.map(|(pkg, _)| pkg)))
}

async fn login_finish(State(node): State<Arc<Node>>, Json(body): Json<AssertionBody>) -> ApiResult {
    let Pending::Login { challenge, .. } = take_pending(&node, &body.challenge_id).await? else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a login challenge",
        ));
    };
    let (account, app) =
        check_assertion(&node, &challenge, &body.assertion, None, false, None, true).await?;
    new_session(&node, &account, &body.assertion.credential_id, app).await
}

async fn passkey_add_begin(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let challenge = random::<32>();
    let id = put_pending(
        &node,
        Pending::AddPasskey {
            challenge,
            account: account.clone(),
            expires: now_secs() + CHALLENGE_TTL,
        },
    )?;
    let exclude: Vec<Value> = node.sys(|s| {
        s.accounts
            .get(&account)
            .map(|a| {
                a.passkeys
                    .iter()
                    .map(|c| json!({ "type": "public-key", "id": c.id }))
                    .collect()
            })
            .unwrap_or_default()
    });
    Ok(Json(json!({
        "challengeId": id,
        "options": {
            "challenge": b64url(&challenge),
            "rp": { "id": node.config.rp_id, "name": "Calimero" },
            "user": { "id": b64url(&sha256(account.as_bytes())[..16]), "name": account, "displayName": account },
            "pubKeyCredParams": [ { "type": "public-key", "alg": -7 }, { "type": "public-key", "alg": -8 } ],
            "excludeCredentials": exclude,
            "attestation": "none",
        }
    })))
}

async fn passkey_add_finish(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<RegisterFinish>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let Pending::AddPasskey {
        challenge, account, ..
    } = take_pending(&node, &body.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not an add-passkey challenge",
        ));
    };
    if p.account()? != account {
        return Err(ApiError::forbidden("challenge issued to another account"));
    }
    let cred = verify_registration(&rp(&node), &challenge, &body.registration)
        .map_err(|e| ApiError::bad_request("BAD_REGISTRATION", &e.to_string()))?;
    let id = cred.id.clone();
    let _ok = node
        .sys_write(&SystemCmd::AddPasskey {
            account,
            credential: cred,
        })
        .await?;
    Ok(Json(json!({ "credentialId": id })))
}

async fn passkey_remove(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(cred): Path<String>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::RemovePasskey {
            account,
            credential_id: cred,
            now: now_secs(),
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshBody {
    refresh_token: String,
}

async fn refresh(State(node): State<Arc<Node>>, Json(body): Json<RefreshBody>) -> ApiResult {
    let old_hash = b64url(&sha256(body.refresh_token.as_bytes()));
    let mut found = node.sys_until(|s| s.refresh.get(&old_hash).cloned()).await;
    let mut caught_up = true;
    if found.is_none() {
        // Not here yet, or exchanged already: this replica may be behind, so
        // ask the leader how far the log is before telling a reused token
        // (revoke the session) from an unknown one.
        caught_up = node.sys_fresh().await;
        found = node.sys(|s| s.refresh.get(&old_hash).cloned());
    }
    let Some(rec) = found else {
        // A token already exchanged, presented again: someone else holds a
        // copy, so the session it belongs to ends here. `RefreshReused` is
        // refused at apply for a token never exchanged, so a replica that
        // could not catch up lets the system log decide rather than answer
        // "unknown" to a thief from a stale view.
        let known_here = node.sys(|s| s.rotated_refresh.contains_key(&old_hash));
        if known_here || !caught_up {
            let revoked = node
                .sys_write(&SystemCmd::RefreshReused {
                    hash: old_hash,
                    now: now_secs(),
                })
                .await;
            if known_here || revoked.is_ok() {
                return Err(ApiError::unauthenticated(
                    "refresh token already used; the session is revoked",
                ));
            }
            // Not exchanged at the leader either: it may be a token issued
            // after this replica fell behind, so do not call it unknown.
            return Err(ApiError::unavailable(
                "this node is behind the cluster; retry the refresh",
            ));
        }
        return Err(ApiError::unauthenticated("unknown refresh token"));
    };
    let (version, key) = node
        .sys(|s| s.current_session_key())
        .ok_or_else(|| ApiError::unavailable("no session key"))?;
    let new_refresh = format!("rt_{}", b64url(&random::<32>()));
    let record = RefreshRecord {
        account: rec.account.clone(),
        session: rec.session.clone(),
        expires_at: now_secs() + REFRESH_TTL,
        passkey: rec.passkey.clone(),
        app: rec.app.clone(),
    };
    let exchanged = node
        .sys_write(&SystemCmd::UseRefresh {
            old_hash: old_hash.clone(),
            new_hash: b64url(&sha256(new_refresh.as_bytes())),
            record,
            now: now_secs(),
        })
        .await;
    if let Err(e) = exchanged {
        // This replica still saw the token as live, but the log had exchanged
        // it already: a copy, decided by the log itself (`RefreshReused` is
        // refused at apply for a token never exchanged).
        let revoked = node
            .sys_write(&SystemCmd::RefreshReused {
                hash: old_hash,
                now: now_secs(),
            })
            .await;
        if revoked.is_ok() {
            return Err(ApiError::unauthenticated(
                "refresh token already used; the session is revoked",
            ));
        }
        return Err(ApiError::unauthenticated(&e.message));
    }
    let expires_at = now_secs() + SESSION_TTL;
    let token = mint_session(
        &key,
        &SessionClaims {
            account: rec.account.clone(),
            session: rec.session,
            expires_at,
            key_version: version,
            passkey: rec.passkey,
            app: rec.app,
        },
    );
    Ok(Json(
        json!({ "accountId": rec.account, "sessionToken": token, "expiresAt": expires_at, "refreshToken": new_refresh }),
    ))
}

async fn logout(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = person(&node, &headers).await?;
    let session = p.session.map(|s| s.session).unwrap_or_default();
    let _ok = node
        .sys_write(&SystemCmd::RevokeSession {
            session,
            actor: p.caller.account().unwrap_or_default().to_owned(),
            now: now_secs(),
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseApprovalBody {
    /// The mero-tee release, as attestations name it.
    pub(crate) version: String,
    /// `false` withdraws an approval.
    #[serde(default = "yes")]
    pub(crate) approved: bool,
    #[serde(default)]
    pub(crate) confirm: Option<ConfirmBody>,
}

const fn yes() -> bool {
    true
}

/// Approve (or withdraw) a mero-tee release for the apps of `scope`, with a
/// passkey assertion over the exact change: an organization's by whoever
/// holds `APPROVE_RELEASES` there (`SystemState::approves_releases`), an
/// account's own by the account. Until it is approved, nodes on the release
/// get none of those apps' data.
///
/// An approval binds the version and the digest of the release's signed
/// measurements as this node reads them now (`consent_page::release_digest`),
/// so a release re-published under the same tag is another approval. A
/// withdrawal lets nothing in and binds the version alone. `shown` is the
/// digest the consent page displayed; another one now is `CONSENT_STALE`.
pub(crate) async fn set_release_approval(
    node: &Node,
    p: &Principal,
    scope: crate::system::ReleaseScope,
    body: ReleaseApprovalBody,
    shown: Option<&str>,
) -> ApiResult {
    let account = p.account()?;
    let subject = match &scope {
        crate::system::ReleaseScope::Org { org } => format!("org:{org}"),
        crate::system::ReleaseScope::Account => format!("account:{account}"),
        crate::system::ReleaseScope::Cluster => "cluster".to_owned(),
    };
    owner_holds(node, |g| g.releases, "release approvals")?;
    // Asked before the passkey ceremony, on fresh state, so a member whose
    // capability was revoked or who left is refused at once; `apply` asks
    // again.
    if let crate::system::ReleaseScope::Org { org } = &scope {
        node.sys_fresh().await;
        let (member, approves) = node.sys(|s| {
            (
                s.orgs.get(org).is_some_and(|o| o.role(&account).is_some()),
                s.approves_releases(org, &account),
            )
        });
        if !member {
            return Err(ApiError::not_found("no such organization"));
        }
        if !approves {
            return Err(ApiError::forbidden(
                "approving releases needs APPROVE_RELEASES in the organization",
            ));
        }
    }
    let method = if body.approved {
        "__approve_release"
    } else {
        "__withdraw_release"
    };
    let args = if body.approved {
        let digest = crate::consent_page::release_digest(node, &body.version).await?;
        if shown.is_some_and(|s| s != digest) {
            return Err(crate::consent_page::stale(&format!(
                "the measurements of mero-tee {}",
                body.version
            )));
        }
        json!({ "version": body.version, "policyDigest": digest })
    } else {
        json!({ "version": body.version })
    };
    let consent = consent(node, p, &subject, method, &args, body.confirm).await?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetReleaseApproval {
            scope,
            version: body.version,
            approved: body.approved,
            actor: account,
            consent,
        })
        .await?,
    ))
}

async fn org_release_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<ReleaseApprovalBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    set_release_approval(
        &node,
        &p,
        crate::system::ReleaseScope::Org { org },
        body,
        None,
    )
    .await
}

async fn account_release_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<ReleaseApprovalBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    set_release_approval(&node, &p, crate::system::ReleaseScope::Account, body, None).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutboundApprovalBody {
    /// The package whose publisher (its pinned signer) the approval is for.
    pub(crate) package: String,
    /// `"manifest"` (the default: every prefix the latest release declares)
    /// or the prefixes to approve.
    #[serde(default = "manifest")]
    pub(crate) prefixes: calimero_primitives::outbound::OutboundSelection,
    /// Set exactly these (how a prefix is revoked) instead of adding them.
    #[serde(default)]
    pub(crate) replace: bool,
    #[serde(default)]
    pub(crate) confirm: Option<ConfirmBody>,
}

const fn manifest() -> calimero_primitives::outbound::OutboundSelection {
    calimero_primitives::outbound::OutboundSelection::Manifest
}

/// Approve URL prefixes a package's publisher may call out to from the apps
/// of `scope`, with a passkey assertion over the exact change. The owner
/// decides, never the operator: the operator only bounds the hosts.
pub(crate) async fn set_outbound_approval(
    node: &Node,
    p: &Principal,
    scope: crate::system::ReleaseScope,
    body: OutboundApprovalBody,
) -> ApiResult {
    let account = p.account()?;
    // A package none of whose releases the caller may see is not installed
    // for them: answered before the passkey ceremony, as a missing one.
    node.sys_fresh().await;
    if !node.sys(|s| {
        s.packages
            .get(&body.package)
            .is_some_and(|pkg| s.visible_versions(pkg, &account).next().is_some())
    }) {
        return Err(ApiError::not_found("no such package"));
    }
    let subject = match &scope {
        crate::system::ReleaseScope::Org { org } => format!("org:{org}"),
        crate::system::ReleaseScope::Account => format!("account:{account}"),
        crate::system::ReleaseScope::Cluster => "cluster".to_owned(),
    };
    owner_holds(node, |g| g.outbound, "outbound approvals")?;
    let consent = consent(
        node,
        p,
        &subject,
        "__approve_outbound",
        &json!({
            "package": body.package,
            "prefixes": body.prefixes,
            "replace": body.replace,
        }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetOutboundApproval {
            scope,
            package: body.package,
            selection: body.prefixes,
            replace: body.replace,
            actor: account,
            consent,
        })
        .await?,
    ))
}

async fn org_outbound_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<OutboundApprovalBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    set_outbound_approval(&node, &p, crate::system::ReleaseScope::Org { org }, body).await
}

async fn account_outbound_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OutboundApprovalBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    set_outbound_approval(&node, &p, crate::system::ReleaseScope::Account, body).await
}

/// Sign out everywhere: revoke every session of the caller's account.
async fn logout_all(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let out = node
        .sys_write(&SystemCmd::RevokeAllSessions {
            actor: p.account()?,
            now: now_secs(),
        })
        .await?;
    Ok(Json(out))
}

async fn me(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let (acc, orgs) = node.sys(|s| {
        let acc = s.accounts.get(&account).cloned();
        let orgs: Vec<Value> = s
            .orgs
            .values()
            .filter_map(|o| {
                o.role(&account)
                    .map(|r| json!({ "orgId": o.org_id, "name": o.name, "role": r }))
            })
            .collect();
        (acc, orgs)
    });
    let acc = acc.ok_or_else(|| ApiError::not_found("account"))?;
    Ok(Json(json!({
        "accountId": acc.account_id,
        "name": acc.name,
        "passkeys": acc.passkeys.iter().map(|c| json!({
            "id": c.id,
            "signCount": c.sign_count,
            "label": acc.passkey_labels.get(&c.id),
        })).collect::<Vec<_>>(),
        "orgs": orgs,
        "via": if p.api_key.is_some() { "apiKey" } else { "session" },
        // The passkey (device) this session was opened with.
        "passkeyId": p.session.as_ref().and_then(|s| s.passkey.clone()),
        // Releases approved to hold this account's own apps.
        "approvedReleases": acc.approved_releases,
        // Outbound prefixes approved for this account's own apps, by signer.
        "outboundApprovals": acc.outbound_approvals,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyBody {
    #[serde(default)]
    label: String,
    #[serde(default)]
    apps: Option<Vec<String>>,
    #[serde(default)]
    methods: Option<Vec<String>>,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    allow_confirm: bool,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

async fn apikey_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<ApiKeyBody>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let (key, hash) = calimero_identity::new_api_key();
    let key_id = typed_id::<8>();
    let _ok = node
        .sys_write(&SystemCmd::CreateApiKey {
            hash,
            record: ApiKeyRecord {
                key_id: key_id.clone(),
                account,
                label: body.label,
                apps: body.apps,
                methods: body.methods,
                read_only: body.read_only,
                allow_confirm: body.allow_confirm,
                expires_at: now_secs()
                    + body
                        .ttl_secs
                        .unwrap_or(365 * 24 * 3600)
                        .min(365 * 24 * 3600),
            },
        })
        .await?;
    Ok(Json(json!({ "keyId": key_id, "key": key })))
}

async fn apikey_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let keys: Vec<ApiKeyRecord> = node.sys(|s| {
        s.api_keys
            .values()
            .filter(|k| k.account == account)
            .cloned()
            .collect()
    });
    Ok(Json(json!({ "keys": keys })))
}

async fn apikey_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::RevokeApiKey {
            key_id,
            actor: account,
        })
        .await?;
    Ok(Json(json!({})))
}

// ---------------------------------------------------------------- bundles and apps

async fn bundle_put(State(node): State<Arc<Node>>, headers: HeaderMap, body: Body) -> ApiResult {
    let account = installer(&node, &headers).await?.account()?;
    if !node.config.allow_unsigned_bundles {
        return Err(ApiError::forbidden(
            "this node takes signed packages only (POST /packages); raw WASM is for development",
        ));
    }
    if body.len() > 16 * 1024 * 1024 {
        return Err(ApiError::new(
            413,
            "TOO_LARGE",
            "bundles are limited to 16 MiB",
        ));
    }
    let hash = hex::encode(sha256(&body));
    // Refuse what its executor does not take before replicating it: a core
    // module must compile on wasmer, a component must pass the engine's prepare.
    match crate::types::Executor::of(&body) {
        crate::types::Executor::Wasmer => {
            let _module = node
                .executor
                .module(&hash, &body)
                .map_err(|e| ApiError::bad_request("BAD_BUNDLE", &e.to_string()))?;
        }
        crate::types::Executor::Component => {
            // A refused component is a bad request, as a core module wasmer can't
            // compile is (`400 BAD_BUNDLE`).
            let _prepared = node
                .prepare_bytes(&hash, body.to_vec())
                .await
                .map_err(|e| {
                    if e.code == crate::node::BAD_BUNDLE {
                        ApiError::bad_request(crate::node::BAD_BUNDLE, &e.message)
                    } else {
                        e
                    }
                })?;
        }
    }
    node.replicate_bundle(&hash, &body).await?;
    let _ok = node
        .sys_write(&SystemCmd::PutBundle {
            hash: hash.clone(),
            size: body.len() as u64,
            actor: account,
        })
        .await?;
    Ok(Json(json!({ "hash": hash })))
}

/// Install an uploaded, signed `.mpk`.
async fn package_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    let account = installer(&node, &headers).await?.account()?;
    uploads_allowed(&node)?;
    Ok(Json(
        node.install_upload(Arc::from(body.to_vec()), &account)
            .await?,
    ))
}

/// Uploads are for development: a production cluster installs every release
/// from its registry, which any node can fetch it from again.
fn uploads_allowed(node: &Node) -> Result<(), ApiError> {
    if node.config.allow_package_uploads {
        return Ok(());
    }
    Err(ApiError::new(
        403,
        "UPLOADS_DISABLED",
        "this node installs packages from its registry only (POST /packages/install); uploads are for development (--dev)",
    ))
}

#[derive(Deserialize)]
struct InstallBody {
    package: String,
    version: String,
}

/// Install `package@version` from the registry: verified here, then fetched
/// and verified again by every node that runs it.
async fn package_install(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<InstallBody>,
) -> ApiResult {
    let account = installer(&node, &headers).await?.account()?;
    Ok(Json(
        node.install_from_registry(&body.package, &body.version, &account)
            .await?,
    ))
}

/// `p` as `who` (an account, or `system::OPERATOR`) may see it: only the
/// releases [`SystemState::sees_release`] lets them see, `None` when that is
/// none, so a package they may not use is not listed and answers `404`.
/// `frontend`: `who` came through `p`'s own frontend, public by name, and
/// sees every release of it.
fn package_json(
    s: &SystemState,
    p: &crate::system::PackageRecord,
    who: &str,
    frontend: bool,
) -> Option<Value> {
    let mut versions: Vec<&crate::system::PackageVersion> = p
        .versions
        .values()
        .filter(|v| frontend || s.sees_release(v, who))
        .collect();
    if versions.is_empty() {
        return None;
    }
    versions.sort_by(|a, b| crate::system::compare_versions(&a.version, &b.version));
    Some(json!({
        "package": p.name,
        "signerId": p.signer_id,
        "owner": p.owner,
        "latest": versions.last().map(|v| v.version.clone()),
        "versions": versions.iter().map(|v| json!({
            "version": v.version,
            "mpk": v.mpk,
            "size": v.mpk_size,
            "source": v.source,
            "services": v.services,
            "metadata": v.metadata,
            "outbound": v.outbound,
            "installedBy": v.installed_by,
            "installedAt": v.installed_at,
        })).collect::<Vec<_>>(),
    }))
}

/// The packages the caller may see (`package_json`): on a cluster several
/// organizations share, one organization's releases are not another's to list.
async fn package_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let principal = signed_in(&node, &headers).await?;
    let account = principal.account()?;
    node.sys_fresh().await;
    let packages: Vec<Value> = node.sys(|s| {
        s.packages
            .values()
            .filter_map(|p| {
                package_json(
                    s,
                    p,
                    &account,
                    principal.bound_app() == Some(p.name.as_str()),
                )
            })
            .collect()
    });
    Ok(Json(json!({ "packages": packages })))
}

async fn package_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    let principal = signed_in(&node, &headers).await?;
    let account = principal.account()?;
    let frontend = principal.bound_app() == Some(package.as_str());
    node.sys_fresh().await;
    node.sys(|s| package_json(s, s.packages.get(&package)?, &account, frontend))
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such package"))
}

async fn package_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: None,
            actor: account,
        })
        .await?,
    ))
}

async fn package_version_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((package, version)): Path<(String, String)>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: Some(version),
            actor: account,
        })
        .await?,
    ))
}

/// The operator installs an uploaded, signed `.mpk` (where it upgrades apps,
/// it needs the releases it upgrades to). Checked as any install is.
async fn ops_package_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    operator(&node, &headers)?;
    uploads_allowed(&node)?;
    Ok(Json(
        node.install_upload(Arc::from(body.to_vec()), crate::system::OPERATOR)
            .await?,
    ))
}

/// The operator installs `package@version` from the registry.
async fn ops_package_install(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<InstallBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.install_from_registry(&body.package, &body.version, crate::system::OPERATOR)
            .await?,
    ))
}

/// The releases the operator installed: publisher metadata, no app data, and
/// nothing an account installed.
async fn ops_package_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    // An install committed through another node is listed (read index).
    node.sys_fresh().await;
    let packages: Vec<Value> = node.sys(|s| {
        s.packages
            .values()
            .filter_map(|p| package_json(s, p, crate::system::OPERATOR, false))
            .collect()
    });
    Ok(Json(json!({ "packages": packages })))
}

/// The operator uninstalls a package it installed.
async fn ops_package_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: None,
            actor: crate::system::OPERATOR.to_owned(),
        })
        .await?,
    ))
}

async fn ops_package_version_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((package, version)): Path<(String, String)>,
) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: Some(version),
            actor: crate::system::OPERATOR.to_owned(),
        })
        .await?,
    ))
}

/// The bundle a create or upgrade names: a bundle hash, or a package release
/// (installed from the registry first if the fleet lacks it).
async fn bundle_named(
    node: &Node,
    bundle: Option<String>,
    package: Option<&str>,
    version: Option<&str>,
    service: Option<&str>,
    actor: &str,
    frontend: bool,
) -> Result<String, ApiError> {
    match (bundle, package) {
        // A hash names a release's WASM only for who may see that release.
        (Some(b), None) if node.sys(|s| s.sees_bundle(&b, actor)) => Ok(b),
        (Some(_), None) => Err(crate::node::rejected("unknown bundle")),
        (None, Some(p)) => {
            node.resolve_release(p, version, service, actor, frontend)
                .await
        }
        _ => Err(ApiError::bad_request(
            "BAD_REQUEST",
            "name either a bundle or a package",
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppBody {
    #[serde(default)]
    name: String,
    /// A bundle hash (a raw upload), or `package` (+ `version`, `service`).
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    access: Option<Access>,
    #[serde(default)]
    confirm_methods: Vec<String>,
    /// Arguments for the app's `init`, as JSON.
    #[serde(default)]
    init_args: Value,
    /// The instance's metadata.
    #[serde(default)]
    metadata: Value,
}

async fn app_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<CreateAppBody>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    match p.bound_app() {
        // A frontend makes instances of its own package, already installed.
        Some(pkg) => {
            let installed = node.sys(|s| s.packages.contains_key(pkg));
            if body.bundle.is_some() || body.package.as_deref() != Some(pkg) || !installed {
                return Err(ApiError::forbidden(
                    "a session opened on an app's frontend creates instances of that app only",
                ));
            }
        }
        None => {
            let _p = installer(&node, &headers).await?;
        }
    }
    if p.api_key.as_ref().is_some_and(|k| k.read_only) {
        return Err(ApiError::forbidden("read-only API key"));
    }
    let owner = p.account()?;
    let now = now_secs();
    let bundle = bundle_named(
        &node,
        body.bundle,
        body.package.as_deref(),
        body.version.as_deref(),
        body.service.as_deref(),
        &owner,
        // Checked above: a bound session names its own package only.
        p.bound_app().is_some(),
    )
    .await?;
    // The executor is decided once, here, from the WASM the bundle names (a
    // package's service, or the raw upload), and recorded with the app.
    let executor = node.executor_of(&bundle).await?;
    let confirm_methods = match executor {
        crate::types::Executor::Wasmer => body.confirm_methods,
        // A component's confirm methods are its ABI's (S5), never the request's.
        crate::types::Executor::Component => {
            if serde_json::to_vec(&body.init_args).map_or(0, |a| a.len())
                > crate::node::MAX_ARGS_BYTES
            {
                return Err(crate::node::args_too_large());
            }
            Vec::new()
        }
    };
    // Past a release deadline the app's approver has not met, no new app;
    // `CreateApp` asks again at the same time.
    if let Some(stall) =
        node.sys(|s| s.stalled_for_bundle(&bundle, body.org.as_deref(), &owner, now))
    {
        return Err(crate::node::stalled(&stall));
    }
    // Only nodes on an approved release (the publisher's, the owner's for an
    // unbound package, or the operator's where the deployment gave it
    // release approvals) may hold the app.
    let approved = node.sys(|s| {
        s.approved_for_bundle(&bundle, body.org.as_deref(), &owner)
            .cloned()
    });
    let replicas = node.place(approved.as_ref(), now).await;
    if replicas.is_empty() {
        return Err(ApiError::unavailable(
            "no node runs a release this app's owner approved",
        ));
    }
    let app_id = typed_id::<16>();
    let access = body.access.unwrap_or(if body.org.is_some() {
        Access::Org
    } else {
        Access::Owner
    });
    let out = node
        .sys_write(&SystemCmd::CreateApp {
            app: Box::new(crate::types::AppRecord {
                app_id: app_id.clone(),
                name: body.name,
                group: 0,
                bundle,
                owner,
                org: body.org,
                access,
                confirm_methods,
                executor,
                replicas,
                created_at: now,
                init_args: body.init_args,
                metadata: body.metadata,
                last_upgrade: None,
            }),
        })
        .await?;
    // Wait for the app's first entry (init) to commit somewhere.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match app_ready(&node, &app_id).await {
            Some(Ok(())) => break,
            Some(Err(error)) => {
                return Err(ApiError::new(422, "INIT_FAILED", "the app's init failed")
                    .with_details(json!({ "appId": app_id, "error": error })));
            }
            None => {}
        }
        if tokio::time::Instant::now() > deadline {
            return Err(ApiError::unavailable(
                "the app was created but has not started yet",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(Json(json!({
        "appId": app_id,
        "contextId": hex::encode(sha256(app_id.as_bytes())),
        "group": out["group"],
        "replicas": out["replicas"],
    })))
}

/// Whether the app has run `init`, asking a replica if this node hosts none.
/// Whether the app's init has committed: `None` not yet, `Some(Err(why))` failed.
pub async fn app_ready(node: &Node, app_id: &str) -> Option<Result<(), Value>> {
    if let Some(g) = node.hosted(app_id) {
        return g.read_local(|m, _| init_state(&m.state));
    }
    let app = node.app(app_id).await.ok()?;
    let addr = node.sys(|s| {
        app.replicas
            .iter()
            .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
    })?;
    let v = node
        .peer_forward(&addr, "internal/app-ready", &json!({ "appId": app_id }))
        .await
        .ok()?;
    match (v.get("ready").and_then(Value::as_bool), v.get("initError")) {
        (_, Some(e)) if !e.is_null() => Some(Err(e.clone())),
        (Some(true), _) => Some(Ok(())),
        _ => None,
    }
}

fn init_state(state: &crate::app::AppState) -> Option<Result<(), Value>> {
    match (&state.bundle, &state.init_error) {
        (None, _) => None,
        (Some(_), Some(e)) => Some(Err(e.clone())),
        (Some(_), None) => Some(Ok(())),
    }
}

#[derive(Deserialize)]
struct AppListQuery {
    /// Only instances of this package (any version).
    #[serde(default)]
    package: Option<String>,
    /// Only instances running this bundle.
    #[serde(default)]
    bundle: Option<String>,
}

async fn app_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<AppListQuery>,
) -> ApiResult {
    let p = principal(&node, &headers).await?;
    let account = p
        .as_ref()
        .and_then(|p| p.caller.account().map(str::to_owned));
    let bound = p.as_ref().and_then(|p| p.bound_app().map(str::to_owned));
    let apps: Vec<Value> = node.sys(|s| {
        s.apps
            .values()
            .filter(|a| {
                bound.as_ref().is_none_or(|pkg| {
                    s.bundles
                        .get(&a.bundle)
                        .and_then(|b| b.package.as_ref())
                        .is_some_and(|r| r.package == *pkg)
                })
            })
            .filter(|a| a.access != Access::Public || account.as_deref() == Some(a.owner.as_str()))
            .filter(|a| s.may_reach(a, account.as_deref()))
            .filter(|a| q.bundle.as_ref().is_none_or(|b| *b == a.bundle))
            .filter(|a| {
                q.package.as_ref().is_none_or(|p| {
                    s.bundles
                        .get(&a.bundle)
                        .and_then(|b| b.package.as_ref())
                        .is_some_and(|r| r.package == *p)
                })
            })
            .map(|a| {
                let release = s.bundles.get(&a.bundle).and_then(|b| b.package.clone());
                json!({
                    "appId": a.app_id,
                    "name": a.name,
                    "owner": a.owner,
                    "org": a.org,
                    "bundle": a.bundle,
                    "package": release.as_ref().map(|r| &r.package),
                    "version": release.as_ref().map(|r| &r.version),
                })
            })
            .collect()
    });
    Ok(Json(json!({ "apps": apps })))
}

/// Delete an app instance: its owner, or an admin of its organization.
async fn app_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    let app = node.app(&app_id).await?;
    p.within(&node, &app)?;
    let _ok = node
        .sys_write(&SystemCmd::DeleteApp {
            app_id: app_id.clone(),
            actor: account,
        })
        .await?;
    Ok(Json(json!({ "appId": app_id, "deleted": true })))
}

async fn app_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = principal(&node, &headers).await?;
    let app = node.app(&app_id).await?;
    if let Some(p) = &p {
        p.within(&node, &app)?;
    }
    let account = p.as_ref().and_then(|p| p.caller.account());
    if !reaches(&node, &app, account) {
        return Err(ApiError::not_found("no such app"));
    }
    Ok(Json(serde_json::to_value(app).unwrap_or_default()))
}

/// The caller, if they may reach `app` with this request kind.
/// Whether `account` holds a read-only row in the team `app` belongs to
/// (checked again against the latest system state before saying yes).
pub async fn read_only_in(
    node: &Node,
    app: &crate::types::AppRecord,
    account: Option<&str>,
) -> bool {
    let (crate::types::Access::Team { team }, Some(a)) = (&app.access, account) else {
        return false;
    };
    if a == app.owner {
        return false;
    }
    let check = || {
        node.sys(|s| {
            app.org
                .as_ref()
                .and_then(|o| s.orgs.get(o))
                .is_some_and(|o| !o.writes_team(a, team))
        })
    };
    check() && {
        node.sys_fresh().await;
        check()
    }
}

/// Whether `account` may reach `app`, judged on the app's record as applied
/// now. Handlers read the record before `principal` makes its fresh system
/// read, so the record in hand can predate a grant or revocation committed
/// through another node; this reads it again.
fn reaches(node: &Node, app: &crate::types::AppRecord, account: Option<&str>) -> bool {
    node.sys(|s| {
        s.apps
            .get(&app.app_id)
            .is_some_and(|now| s.may_reach(now, account))
    })
}

async fn gate(
    node: &Node,
    headers: &HeaderMap,
    app: &crate::types::AppRecord,
    method: &str,
    write: bool,
) -> Result<Principal, ApiError> {
    let p = principal(node, headers).await?;
    if let Some(p) = &p {
        p.within(node, app)?;
    }
    let account = p.as_ref().and_then(|p| p.caller.account());
    // Access is judged on the cluster's latest system state (a read index): a
    // removal or revocation committed through another node holds before the
    // next request, read or write, and a grant is seen at once (Xil's review:
    // access judged on lagging local state). `principal` already read fresh
    // for a request that carries a credential.
    if p.is_none() {
        node.sys_fresh().await;
    }
    if !reaches(node, app, account) {
        return Err(if p.is_none() {
            ApiError::unauthenticated("sign in to use this app")
        } else {
            ApiError::not_found("no such app")
        });
    }
    // A read-only row refuses what writes: a blob upload here, a call that
    // would write at commit (`Node::call_local`); v1 discarded such writes.
    if write && method == "__blob" && read_only_in(node, app, account).await {
        return Err(ApiError::forbidden("read-only in this team"));
    }
    if let Some(k) = p.as_ref().and_then(|p| p.api_key.as_ref()) {
        if write && k.read_only {
            return Err(ApiError::forbidden("read-only API key"));
        }
        if k.apps.as_ref().is_some_and(|a| !a.contains(&app.app_id)) {
            return Err(ApiError::forbidden("API key not scoped to this app"));
        }
        if k.methods
            .as_ref()
            .is_some_and(|m| !m.iter().any(|x| x == method || x == "*"))
        {
            return Err(ApiError::forbidden("API key not scoped to this method"));
        }
    }
    Ok(p.unwrap_or(Principal {
        caller: Caller::Public,
        session: None,
        api_key: None,
        consent_page: false,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmBody {
    challenge_id: String,
    assertion: Assertion,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntentBody {
    method: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// For confirm methods: the 412 that asks for a passkey, or the consent to log.
pub(crate) async fn consent(
    node: &Node,
    p: &Principal,
    app_id: &str,
    method: &str,
    args: &Value,
    confirm: Option<ConfirmBody>,
) -> Result<Value, ApiError> {
    if p.api_key.as_ref().is_some_and(|k| !k.allow_confirm) {
        return Err(ApiError::forbidden(
            "this method needs the account holder's passkey",
        ));
    }
    let account = p.account()?;
    let args_bytes = serde_json::to_vec(args).unwrap_or_default();
    let Some(confirm) = confirm else {
        let nonce = random::<16>();
        let challenge = confirm_challenge(app_id, method, &args_bytes, &nonce);
        let id = put_pending(
            node,
            Pending::Confirm {
                challenge,
                app: app_id.to_owned(),
                method: method.to_owned(),
                args: args_bytes,
                account: account.clone(),
                expires: now_secs() + CHALLENGE_TTL,
            },
        )?;
        let creds: Vec<String> = node.sys(|s| {
            s.accounts
                .get(&account)
                .map(|a| a.passkeys.iter().map(|c| c.id.clone()).collect())
                .unwrap_or_default()
        });
        return Err(ApiError::new(
            412,
            "CONFIRM_REQUIRED",
            "approve this call with your passkey",
        )
        .with_details(json!({
            "challengeId": id,
            "challenge": b64url(&challenge),
            "rpId": node.config.rp_id,
            "allowCredentials": creds,
        })));
    };
    let Pending::Confirm {
        challenge,
        app,
        method: m,
        args: a,
        account: acc,
        ..
    } = take_pending(node, &confirm.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a confirm challenge",
        ));
    };
    if app != app_id || m != method || a != args_bytes || acc != account {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "the approval was for a different call",
        ));
    }
    // The consent page is served on the node's own origin, not the cluster's.
    let base = if p.consent_page {
        crate::consent_page::relying_party(node)
    } else {
        rp_step_up(node)
    };
    let _owner = check_assertion_for(
        node,
        base,
        &challenge,
        &confirm.assertion,
        Some(&account),
        p.bound_app(),
        false,
    )
    .await?;
    Ok(serde_json::to_value(&confirm.assertion).unwrap_or_default())
}

fn answer(node: &Node, app_id: &str, a: Applied) -> Response {
    if a.conflict {
        return ApiError::unavailable(
            "the call raced a leader change; retry with the same idempotency key",
        )
        .into_response();
    }
    if a.error.as_str() == Some(crate::node::READ_ONLY) {
        return ApiError::forbidden(crate::node::READ_ONLY).into_response();
    }
    match a.status {
        Status::Ok => {
            let receipt = node.receipt(app_id, a.index, &a.result);
            Json(json!({
                "index": a.index,
                "result": a.result,
                "events": a.events.iter().map(|(k, d)| json!({ "kind": k, "data": d })).collect::<Vec<_>>(),
                // Signed by this node's attested receipt key (requirement: show
                // the end user the answer came from a genuine TEE cluster).
                "receipt": receipt,
            }))
            .into_response()
        }
        Status::AppError => ApiError::new(422, "APP_ERROR", "the app refused the call")
            .with_details(json!({ "index": a.index, "error": a.error }))
            .into_response(),
        Status::Trap => ApiError::new(422, "TRAP", "the call failed")
            .with_details(json!({ "index": a.index, "error": a.error }))
            .into_response(),
    }
}

/// The 422/503 of a component's run that did not end `Ok` (spec section 5, D23, D36).
fn component_failure(
    index: u64,
    status: &Status,
    error: &Value,
    timeout: bool,
) -> Option<ApiError> {
    if timeout {
        return Some(crate::node::timeout_error());
    }
    match status {
        Status::Ok => None,
        Status::AppError => Some(
            ApiError::new(422, "APP_ERROR", "the app refused the call").with_details(json!({
                "index": index,
                "code": error["code"],
                "fields": error["fields"],
            })),
        ),
        Status::Trap => Some(
            ApiError::new(422, "TRAP", "the run trapped").with_details(json!({
                "index": index,
                "kind": error["kind"],
                "detail": error["detail"],
            })),
        ),
    }
}

/// A component call's answer. Its events are the ones the caller is in the
/// audience of (R9).
fn component_answer(
    node: &Node,
    app_id: &str,
    caller: &Caller,
    a: crate::component::Applied,
) -> Response {
    if a.conflict {
        return ApiError::unavailable(
            "the call raced a leader change; retry with the same idempotency key",
        )
        .into_response();
    }
    if a.error.as_str() == Some(crate::node::NOT_REACHED) {
        return ApiError::not_found(crate::node::NOT_REACHED).into_response();
    }
    if a.error.as_str() == Some(crate::node::READ_ONLY) {
        return ApiError::forbidden(crate::node::READ_ONLY).into_response();
    }
    if let Some(e) = component_failure(a.index, &a.status, &a.error, a.timeout) {
        return e.into_response();
    }
    let account = caller.account();
    let events: Vec<Value> = node.sys(|s| {
        let Some(app) = s.apps.get(app_id) else {
            return Vec::new();
        };
        a.events
            .iter()
            .filter(|e| s.receives(app, &e.audience, account, account))
            .map(|e| json!({ "kind": e.kind, "data": e.data }))
            .collect()
    });
    let receipt = node.receipt(app_id, a.index, &a.result);
    Json(json!({ "index": a.index, "result": a.result, "events": events, "receipt": receipt }))
        .into_response()
}

/// `/intents` for a component app: the ABI of the bundle the app group runs
/// (Ruling 40) decides whether it is a view and whether it needs a passkey.
async fn component_intents(
    node: &Node,
    app: &crate::types::AppRecord,
    p: Principal,
    body: IntentBody,
) -> Result<Response, ApiError> {
    let app_id = app.app_id.clone();
    let method = node
        .prepared_for(&node.running_bundle(app))
        .await?
        .abi
        .methods
        .get(&body.method)
        .cloned();
    let caller = p.caller.clone();
    // A declared view runs as one, whichever way it is invoked: it
    // reads the latest state and writes nothing.
    if method
        .as_ref()
        .is_some_and(|m| m.kind == calimero_app_host::MethodKind::View && !m.confirm)
    {
        let a = node
            .view(&ViewRequest {
                app_id: app_id.clone(),
                method: body.method,
                args: body.args,
                caller: p.caller,
                strong: true,
                min_index: 0,
            })
            .await?;
        return Ok(component_answer(
            node,
            &app_id,
            &caller,
            crate::component::Applied {
                index: a.index,
                status: a.status,
                result: a.result,
                error: a.error,
                events: Vec::new(),
                conflict: false,
                timeout: a.timeout,
            },
        ));
    }
    // Confirm flags come from the running bundle's ABI (S5), never from the app record.
    let consent_value = if method.as_ref().is_some_and(|m| m.confirm) {
        Some(consent(node, &p, &app_id, &body.method, &body.args, body.confirm).await?)
    } else {
        None
    };
    let unconfirmed = consent_value.is_none();
    let (method_name, args) = (body.method.clone(), body.args.clone());
    let applied = match node
        .call_component(&CallRequest {
            idempotency_key: body.idempotency_key,
            consent: consent_value,
            ..CallRequest::new(app_id.clone(), body.method, body.args, p.caller.clone())
        })
        .await
    {
        // The replica that runs it found a confirm method this node's ABI did not
        // know (it was stale): the step-up is the usual `412 CONFIRM_REQUIRED` with
        // a challenge (SDK reference), as on the wasmer path.
        Err(e) if unconfirmed && e.details["code"] == "ConfirmRequired" => {
            return Err(consent(node, &p, &app_id, &method_name, &args, None)
                .await
                .err()
                .unwrap_or(e));
        }
        other => other?,
    };
    Ok(component_answer(node, &app_id, &caller, applied))
}

async fn intents(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<IntentBody>,
) -> Result<Response, ApiError> {
    not_reserved(&body.method)?;
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, &body.method, true).await?;
    if app.executor == crate::types::Executor::Component {
        return component_intents(&node, &app, p, body).await;
    }
    // A declared view runs as one, whichever way it is invoked: it
    // reads the latest state, writes nothing, and may search.
    if node.facts(&app.bundle).await.views.contains(&body.method) {
        let a = node
            .view(&ViewRequest {
                app_id: app_id.clone(),
                method: body.method,
                args: body.args,
                caller: p.caller,
                strong: true,
                min_index: 0,
            })
            .await?;
        return Ok(answer(
            &node,
            &app_id,
            Applied {
                index: a.index,
                status: a.status,
                result: a.result,
                error: a.error,
                events: Vec::new(),
                conflict: false,
            },
        ));
    }
    let consent_value = if app.confirm_methods.iter().any(|m| m == &body.method) {
        Some(consent(&node, &p, &app_id, &body.method, &body.args, body.confirm).await?)
    } else {
        None
    };
    let applied = node
        .call(&CallRequest {
            idempotency_key: body.idempotency_key,
            consent: consent_value,
            ..CallRequest::new(app_id.clone(), body.method, body.args, p.caller)
        })
        .await?;
    Ok(answer(&node, &app_id, applied))
}

/// The node's commands and the runtime's exports are not an app's methods
/// (`node::reserved_method`): a client naming one would otherwise reach
/// `__upgrade` or `__blob` past the API's authority checks.
fn not_reserved(method: &str) -> Result<(), ApiError> {
    if crate::node::reserved_method(method) {
        return Err(ApiError::bad_request(
            "RESERVED_METHOD",
            &format!("{method} is not a method of this app"),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewBody {
    method: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    consistency: Option<String>,
    #[serde(default)]
    min_index: u64,
}

async fn views(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<ViewBody>,
) -> Result<Response, ApiError> {
    not_reserved(&body.method)?;
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, &body.method, false).await?;
    // A confirm method runs only as a confirmed call: as a view its writes are
    // dropped, but its answer was handed back unconfirmed (Xil's review, issue 6).
    if app.confirm_methods.iter().any(|m| m == &body.method) {
        return Err(ApiError::new(
            412,
            "CONFIRM_REQUIRED",
            "this method needs a passkey confirmation; call it through /intents",
        ));
    }
    let a = node
        .view(&ViewRequest {
            app_id: app_id.clone(),
            method: body.method,
            args: body.args,
            caller: p.caller,
            strong: body.consistency.as_deref() != Some("stale"),
            min_index: body.min_index,
        })
        .await?;
    if app.executor == crate::types::Executor::Component {
        return Ok(
            match component_failure(a.index, &a.status, &a.error, a.timeout) {
                Some(e) => e.into_response(),
                None => {
                    let receipt = node.receipt(&app_id, a.index, &a.result);
                    Json(json!({ "index": a.index, "result": a.result, "receipt": receipt }))
                        .into_response()
                }
            },
        );
    }
    Ok(match a.status {
        Status::Ok => {
            let receipt = node.receipt(&app_id, a.index, &a.result);
            Json(json!({ "index": a.index, "result": a.result, "receipt": receipt }))
                .into_response()
        }
        Status::AppError => ApiError::new(422, "APP_ERROR", "the app refused the read")
            .with_details(json!({ "index": a.index, "error": a.error }))
            .into_response(),
        Status::Trap => ApiError::new(422, "TRAP", "the read failed")
            .with_details(json!({ "index": a.index, "error": a.error }))
            .into_response(),
    })
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    from: u64,
}

/// The app's events from `from`, then live. A component's event carries its
/// audience (R9) and goes to `subscriber` (`None`: anonymous) only if the system
/// state, when it is sent, says it still reaches the app and is in the audience,
/// so a member removed from the org or the team stops receiving at once. An
/// event of an app on wasmer has no audience and goes out as before.
fn local_events(
    node: &Arc<Node>,
    app_id: &str,
    from: u64,
    subscriber: Option<String>,
) -> Option<impl Stream<Item = Result<Event, Infallible>>> {
    let g = node.hosted(app_id)?;
    let (backlog, rx) = g.read_local(|m, _| {
        let rx = m.events_tx.subscribe();
        (m.events_from(from), rx)
    });
    let last = backlog.last().map(|e| (e.index, e.seq));
    let live = tokio_stream::wrappers::BroadcastStream::new(rx)
        .filter_map(|r| async move { r.ok() })
        .filter(move |e| {
            let fresh = last.is_none_or(|l| (e.index, e.seq) > l);
            async move { fresh }
        });
    let (node, app) = (Arc::clone(node), app_id.to_owned());
    let receives = move |e: &crate::types::EventRecord| match &e.audience {
        None => true,
        Some(audience) => node.sys(|s| {
            s.apps.get(&app).is_some_and(|a| {
                s.receives(a, audience, e.caller.as_deref(), subscriber.as_deref())
            })
        }),
    };
    let stream = futures_util::stream::iter(backlog).chain(live);
    let stream = stream.filter(move |e| {
        let ok = receives(e);
        async move { ok }
    });
    let stream = stream.map(|e| {
        Ok(Event::default()
            .id(format!("{}:{}", e.index, e.seq))
            .event(e.kind.clone())
            .data(serde_json::to_string(&e.data).unwrap_or_default()))
    });
    Some(stream)
}

/// The most event streams one node serves at once (Xil's review, issue 12).
pub const MAX_EVENT_STREAMS: usize = 4096;

/// One open event stream, counted until the stream is dropped.
struct StreamSlot(Arc<std::sync::atomic::AtomicUsize>);

impl StreamSlot {
    fn take(node: &Node) -> Result<Self, ApiError> {
        use std::sync::atomic::Ordering;
        let open = node.event_streams.fetch_add(1, Ordering::SeqCst);
        let slot = Self(Arc::clone(&node.event_streams));
        if open >= MAX_EVENT_STREAMS {
            return Err(ApiError::new(
                429,
                "RATE_LIMITED",
                "this node serves as many event streams as it can; try another node or later",
            ));
        }
        Ok(slot)
    }
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let _was = self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn events(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let subscriber = p.caller.account().map(str::to_owned);
    let slot = StreamSlot::take(&node)?;
    if let Some(stream) = local_events(&node, &app_id, q.from, subscriber.clone()) {
        let stream = stream.map(move |e| {
            let _held = &slot;
            e
        });
        return Ok(Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response());
    }
    let addr = node
        .sys(|s| {
            app.replicas
                .iter()
                .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
        })
        .ok_or_else(|| ApiError::unavailable("no replica known"))?;
    let resp = node
        .peer_stream(
            &addr,
            &format!(
                "internal/events?app={app_id}&from={}&subscriber={}",
                q.from,
                subscriber.unwrap_or_default()
            ),
        )
        .await
        .map_err(|e| ApiError::unavailable(&e.to_string()))?;
    let body = axum::body::Body::from_stream(resp.bytes_stream().map(move |b| {
        let _held = &slot;
        b
    }));
    Ok((
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-store"),
        ],
        body,
    )
        .into_response())
}

#[derive(Deserialize)]
struct PresenceBody {
    seq: u64,
    /// Hex; `null` retracts.
    #[serde(default)]
    state: Option<String>,
}

/// Publish the caller's presence in an app (`presence-intents`): anyone
/// who reaches the app, one entry per device (the session's passkey, or the
/// API key). Re-send the same `seq` and state as a heartbeat.
async fn presence_publish(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<PresenceBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let account = p.account()?;
    if let Some(s) = &body.state {
        if hex::decode(s).is_err() || s.len() > 2 * crate::presence::MAX_STATE_BYTES {
            return Err(ApiError::bad_request(
                "BAD_STATE",
                "state is hex of at most 4 KiB",
            ));
        }
    }
    let device = match (&p.session, &p.api_key) {
        (Some(s), _) => s.passkey.clone().unwrap_or_else(|| s.session.clone()),
        (None, Some(k)) => format!("key:{}", k.key_id),
        (None, None) => String::new(),
    };
    node.publish_presence(crate::presence::Update {
        app: app_id,
        account,
        device,
        seq: body.seq,
        state: body.state,
    });
    Ok(Json(json!({})))
}

/// Who is present in an app now.
async fn presence_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    Ok(Json(json!({ "present": node.presence.snapshot(&app_id) })))
}

/// Presence as a stream: a `snapshot` event, then a `presence` event per change
/// (`state: null` when someone left).
async fn presence_stream(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let slot = StreamSlot::take(&node)?;
    let (now, rx) = node.presence.subscribe(&app_id);
    let first = futures_util::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event("snapshot")
                .data(serde_json::to_string(&now).unwrap_or_default()),
        )
    });
    let live = tokio_stream::wrappers::BroadcastStream::new(rx)
        .filter_map(|r| async move { r.ok() })
        .map(move |d| {
            let _held = &slot;
            Ok(Event::default()
                .event("presence")
                .data(serde_json::to_string(&d).unwrap_or_default()))
        });
    Ok(Sse::new(first.chain(live))
        .keep_alive(KeepAlive::default())
        .into_response())
}

/// A presence update another node took (and authenticated).
async fn internal_presence(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(u): Json<crate::presence::Update>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let _applied = node.presence.apply(&u, crate::types::now_ms());
    Ok(Json(json!({})))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpgradeBody {
    /// A bundle hash, or a `version` of the app's package (or another `package`).
    #[serde(default)]
    pub(crate) bundle: Option<String>,
    #[serde(default)]
    pub(crate) package: Option<String>,
    #[serde(default)]
    pub(crate) version: Option<String>,
    #[serde(default)]
    pub(crate) service: Option<String>,
    /// The owner's passkey approval of this exact upgrade (412 asks for it).
    #[serde(default)]
    pub(crate) confirm: Option<ConfirmBody>,
    /// The bundle's migration export, if the state shape changed.
    #[serde(default)]
    pub(crate) migrate: Option<String>,
}

async fn app_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<UpgradeBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    upgrade_app(&node, &p, app_id, body).await
}

/// Upgrade `app_id` as `p`: the route's, and the consent page's for an
/// upgrade request.
pub(crate) async fn upgrade_app(
    node: &Node,
    p: &Principal,
    app_id: String,
    mut body: UpgradeBody,
) -> ApiResult {
    let account = p.account()?;
    // Who may upgrade is the system log's rule (`UpgradeApp`); asked first, so
    // a refused or failed upgrade records nothing. An upgrade is the app's
    // owner's decision, or, for an org app, of whoever holds
    // `MANAGE_APPLICATION` there (`SystemState::manages_apps`), each with
    // their own passkey: new code reads the app's data.
    let app = node.app(&app_id).await?;
    owner_holds(node, |g| g.app_upgrades, "app upgrades")?;
    let (may, unbound) = node.sys(|s| {
        (
            s.may_upgrade(&app, &account),
            !s.upgrades_of(&app.bundle).is_owner() && s.publisher_of(&app.bundle).is_none(),
        )
    });
    if unbound {
        return Err(ApiError::conflict(
            "this app's package is upgraded by its publisher, and no organization has bound its key yet",
        ));
    }
    if !may {
        return Err(ApiError::forbidden(
            "upgrading this app needs whoever its package names: its owner (or MANAGE_APPLICATION in its organization), or its publisher's MANAGE_APPLICATION holders",
        ));
    }
    // A version alone means the app's own package and service.
    let current = node.release_of(&app.bundle);
    let package = body.package.clone().or_else(|| {
        body.version
            .as_ref()
            .and(current.as_ref().map(|r| r.at.package.clone()))
    });
    let service = body
        .service
        .clone()
        .or_else(|| current.as_ref().and_then(|r| r.at.service.clone()));
    let bundle = bundle_named(
        node,
        body.bundle.clone(),
        package.as_deref(),
        body.version.as_deref(),
        service.as_deref(),
        &account,
        false,
    )
    .await?;
    if !node.sys(|s| s.bundles.contains_key(&bundle)) {
        return Err(crate::node::rejected("unknown bundle"));
    }
    if !node.sys(|s| s.upgrade_keeps_authority(&app.bundle, &bundle)) {
        return Err(ApiError::forbidden(
            "an app whose owner upgrades it is not moved to a release its publisher upgrades",
        ));
    }
    refuse_stalled(node, &app, &bundle, now_secs())?;
    // What the publisher signed for the target: the WASM hash alone would let
    // another manifest (other outbound prefixes, frontend origins) under the
    // same version and WASM pass for the one approved.
    let manifest = crate::consent_page::target_manifest_hash(
        node,
        package.as_deref().zip(body.version.as_deref()),
        &bundle,
    )
    .await?;
    // A fresh passkey assertion over this exact upgrade, logged as consent.
    let consent = consent(
        node,
        p,
        &app_id,
        "__upgrade",
        &json!({ "bundle": bundle, "migrate": body.migrate, "manifest": manifest }),
        body.confirm.take(),
    )
    .await?;
    run_upgrade(
        node,
        app_id,
        bundle,
        body.migrate,
        Upgrader {
            caller: Caller::Account {
                account: account.clone(),
            },
            actor: account,
            by_operator: false,
            consent,
            run: None,
        },
    )
    .await
}

/// Who is upgrading, as the directory records it.
#[derive(Clone)]
struct Upgrader {
    /// Who the migration runs as.
    caller: Caller,
    /// The account, or empty for the operator.
    actor: String,
    by_operator: bool,
    consent: Value,
    /// The organization-wide run this belongs to.
    run: Option<String>,
}

impl Upgrader {
    fn by(&self) -> String {
        if self.by_operator {
            crate::system::OPERATOR.to_owned()
        } else {
            self.actor.clone()
        }
    }
}

/// Run an upgrade on the app's group and, once it succeeded, record the new
/// bundle in the directory (where `UpgradeApp` judges who may). A failure is
/// recorded too: the app's last upgrade says why.
async fn run_upgrade(
    node: &Node,
    app_id: String,
    bundle: String,
    migrate: Option<String>,
    who: Upgrader,
) -> ApiResult {
    // Judged once, before the migration runs; `UpgradeApp` carries this time
    // so `apply` judges the deadline at the same moment and never refuses
    // the record of an upgrade the app's group already ran.
    let begun = now_secs();
    // Who may, and where to: judged here, for every path (one app, an
    // organization's run, a publisher's run), before anything runs on the
    // app's group; `UpgradeApp` judges both again.
    let refusal = node.sys(|s| {
        let a = s.apps.get(&app_id)?;
        if !who.by_operator && !s.may_upgrade(a, &who.actor) {
            return Some(ApiError::forbidden(
                "upgrading this app needs whoever its package names: its owner (or MANAGE_APPLICATION in its organization), or its publisher's MANAGE_APPLICATION holders",
            ));
        }
        if !who.by_operator && !s.upgrade_keeps_authority(&a.bundle, &bundle) {
            return Some(ApiError::forbidden(
                "an app whose owner upgrades it is not moved to a release its publisher upgrades",
            ));
        }
        s.stalled_for_bundle(&bundle, a.org.as_deref(), &a.owner, begun)
            .map(|stall| crate::node::stalled(&stall))
    });
    if let Some(e) = refusal {
        let error = json!({ "code": e.code, "message": e.message });
        note_failed_upgrade(node, &app_id, &bundle, &who, error).await;
        return Err(e);
    }
    let applied = match node
        .call(&CallRequest::host(
            app_id.clone(),
            "__upgrade",
            json!({ "bundle": bundle, "migrate": migrate }),
            who.caller.clone(),
        ))
        .await
    {
        Ok(a) => a,
        Err(e) => {
            let error = json!({ "code": e.code, "message": e.message });
            note_failed_upgrade(node, &app_id, &bundle, &who, error).await;
            return Err(e);
        }
    };
    // A refused or failed upgrade leaves the app on its bundle; say so as an
    // error, not a 200 a client can mistake for success.
    if applied.status != Status::Ok {
        note_failed_upgrade(node, &app_id, &bundle, &who, applied.error.clone()).await;
        let why = applied.error.as_str().map_or_else(
            || "the upgrade did not run".to_owned(),
            |e| format!("the upgrade did not run: {e}"),
        );
        return Err(ApiError::new(422, "UPGRADE_FAILED", &why).with_details(
            json!({ "index": applied.index, "status": applied.status, "error": applied.error }),
        ));
    }
    // The app now runs the new bundle: the directory says so too.
    let _ok = node
        .sys_write(&SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator: who.by_operator,
            actor: who.actor,
            at: begun,
            run: who.run,
            consent: who.consent,
        })
        .await?;
    Ok(Json(
        json!({ "index": applied.index, "status": applied.status, "error": applied.error }),
    ))
}

/// Refused `409 RELEASE_DEADLINE` when `app`, upgraded to `bundle`, may start
/// no new work at `now` ([`crate::system::SystemState::stalled_for_bundle`]).
fn refuse_stalled(
    node: &Node,
    app: &crate::types::AppRecord,
    bundle: &str,
    now: u64,
) -> Result<(), ApiError> {
    match node.sys(|s| s.stalled_for_bundle(bundle, app.org.as_deref(), &app.owner, now)) {
        Some(stall) => Err(crate::node::stalled(&stall)),
        None => Ok(()),
    }
}

async fn note_failed_upgrade(
    node: &Node,
    app_id: &str,
    bundle: &str,
    who: &Upgrader,
    error: Value,
) {
    let _r = node
        .sys_write(&SystemCmd::UpgradeFailed {
            app_id: app_id.to_owned(),
            bundle: bundle.to_owned(),
            by: who.by(),
            at: now_secs(),
            error,
            run: who.run.clone(),
        })
        .await;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrgUpgradeBody {
    package: String,
    version: String,
    /// Only the apps of this team (and, with `cascade`, of the teams below it).
    #[serde(default)]
    team: Option<String>,
    #[serde(default)]
    cascade: bool,
    #[serde(default)]
    migrate: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// The apps an organization-wide upgrade of `package` covers.
fn run_targets(
    node: &Node,
    org: &str,
    package: &str,
    team: Option<&str>,
    cascade: bool,
) -> Vec<crate::types::AppRecord> {
    let apps: Vec<crate::types::AppRecord> = node.sys(|s| {
        let Some(o) = s.orgs.get(org) else {
            return Vec::new();
        };
        let teams: Option<Vec<String>> = team.map(|t| {
            if cascade {
                o.subtree(t)
            } else {
                vec![t.to_owned()]
            }
        });
        s.apps
            .values()
            .filter(|a| a.org.as_deref() == Some(org))
            .filter(|a| match (&teams, &a.access) {
                (None, _) => true,
                (Some(ts), Access::Team { team }) => ts.contains(team),
                (Some(_), _) => false,
            })
            .cloned()
            .collect()
    });
    apps.into_iter()
        .filter(|a| {
            node.release_of(&a.bundle)
                .is_some_and(|r| r.at.package == package)
        })
        .collect()
}

/// The bundle `version` of the package gives an app (same service), if
/// installed and the upgrader may see it ([`upgrade_sees`]).
fn target_bundle(
    node: &Node,
    app: &crate::types::AppRecord,
    version: &str,
    who: &Upgrader,
) -> Option<String> {
    let r = node.release_of(&app.bundle)?;
    let service = r.at.service.clone().unwrap_or_default();
    node.sys(|s| {
        let v = s.packages.get(&r.at.package)?.versions.get(version)?;
        if !upgrade_sees(s, v, app, &who.actor, who.by_operator) {
            return None;
        }
        v.services.get(&service).cloned()
    })
}

/// Whether an upgrade may move `app` to release `v`: one the upgrading
/// account may see, or, for the operator, one it installed or one the app's
/// owner may see. A release of another organization never reaches an app.
fn upgrade_sees(
    s: &SystemState,
    v: &crate::system::PackageVersion,
    app: &crate::types::AppRecord,
    actor: &str,
    by_operator: bool,
) -> bool {
    if by_operator {
        s.sees_release(v, crate::system::OPERATOR) || s.sees_release(v, &app.owner)
    } else {
        s.sees_release(v, actor)
    }
}

/// Start an organization-wide upgrade: recorded, then run app by app in the
/// background; `GET /orgs/{org}/upgrades/{run}` follows it.
async fn start_run(
    node: Arc<Node>,
    org: String,
    body: OrgUpgradeBody,
    apps: Vec<crate::types::AppRecord>,
    who: Upgrader,
) -> ApiResult {
    let run_id = typed_id::<12>();
    let run = crate::types::UpgradeRun {
        run_id: run_id.clone(),
        package: body.package.clone(),
        version: body.version.clone(),
        team: body.team.clone(),
        cascade: body.cascade,
        by: who.by(),
        started_at: now_secs(),
        apps: apps.iter().map(|a| a.app_id.clone()).collect(),
        done: Vec::new(),
        failed: std::collections::BTreeMap::new(),
        finished: false,
    };
    let _ok = node
        .sys_write(&SystemCmd::StartUpgradeRun {
            org: org.clone(),
            run: Box::new(run.clone()),
        })
        .await?;
    let who = Upgrader {
        run: Some(run_id.clone()),
        ..who
    };
    let version = body.version.clone();
    let migrate = body.migrate.clone();
    let _task = tokio::spawn(async move {
        for app in apps {
            match target_bundle(&node, &app, &version, &who) {
                Some(bundle) if bundle == app.bundle => {
                    // Already there: done, nothing to run.
                    let _r = node
                        .sys_write(&SystemCmd::UpgradeApp {
                            app_id: app.app_id.clone(),
                            bundle,
                            by_operator: who.by_operator,
                            actor: who.actor.clone(),
                            at: now_secs(),
                            run: who.run.clone(),
                            consent: who.consent.clone(),
                        })
                        .await;
                }
                Some(bundle) => {
                    let _r = run_upgrade(
                        &node,
                        app.app_id.clone(),
                        bundle,
                        migrate.clone(),
                        who.clone(),
                    )
                    .await;
                }
                None => {
                    note_failed_upgrade(
                        &node,
                        &app.app_id,
                        "",
                        &who,
                        json!({ "code": "NOT_INSTALLED", "message": "the release ships no bundle for this app's service" }),
                    )
                    .await;
                }
            }
        }
        let _r = node
            .sys_write(&SystemCmd::FinishUpgradeRun { org, run_id })
            .await;
    });
    Ok(Json(serde_json::to_value(run).unwrap_or_default()))
}

/// Upgrade every app of an organization (or a team, with `cascade` its
/// subtree) that runs `package` to `version`: for whoever holds
/// `MANAGE_APPLICATION` there (the owner, an admin, a team's lead, or a member
/// granted it), with one passkey assertion over the whole run, where owners
/// hold app upgrades. Each app is still checked on its own (`UpgradeApp`).
async fn org_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(mut body): Json<OrgUpgradeBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if !node.sys(|s| s.manages_apps(&org, &account, body.team.as_deref())) {
        return Err(ApiError::forbidden(
            "upgrading the organization's apps needs MANAGE_APPLICATION there",
        ));
    }
    // Installed from the registry first if the cluster lacks the release.
    let _bundle = node
        .resolve_release(&body.package, Some(&body.version), None, &account, false)
        .await;
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__upgrade_org",
        &json!({
            "package": body.package, "version": body.version, "team": body.team,
            "cascade": body.cascade, "migrate": body.migrate,
        }),
        body.confirm.take(),
    )
    .await?;
    let apps = run_targets(
        &node,
        &org,
        &body.package,
        body.team.as_deref(),
        body.cascade,
    );
    let who = Upgrader {
        caller: Caller::Account {
            account: account.clone(),
        },
        actor: account,
        by_operator: false,
        consent,
        run: None,
    };
    start_run(node, org, body, apps, who).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublisherUpgradeBody {
    version: String,
    #[serde(default)]
    migrate: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Upgrade every instance of `package`, whoever owns it, that its publisher
/// upgrades (its release declares `upgrades: publisher`): for whoever holds
/// `MANAGE_APPLICATION` in the publisher organization (`org`, which proved
/// it holds the package's key), with one passkey assertion over the run.
/// The run is kept on the publisher; each app is still checked on its own.
async fn publisher_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, package)): Path<(String, String)>,
    Json(mut body): Json<PublisherUpgradeBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    node.sys_fresh().await;
    let publishes = node.sys(|s| {
        s.orgs.get(&org).is_some_and(|o| o.role(&account).is_some())
            && s.packages
                .get(&package)
                .and_then(|pk| s.publisher_of_signer(&pk.signer_id))
                .is_some_and(|o| *o == org)
    });
    if !publishes {
        return Err(ApiError::not_found(
            "the organization publishes no such package",
        ));
    }
    if !node.sys(|s| s.manages_apps(&org, &account, None)) {
        return Err(ApiError::forbidden(
            "upgrading a published package's instances needs MANAGE_APPLICATION in the publisher",
        ));
    }
    // Installed from the registry first if the cluster lacks the release.
    let _bundle = node
        .resolve_release(&package, Some(&body.version), None, &account, false)
        .await;
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__upgrade_published",
        &json!({ "package": package, "version": body.version, "migrate": body.migrate }),
        body.confirm.take(),
    )
    .await?;
    let apps: Vec<crate::types::AppRecord> = node.sys(|s| {
        s.apps
            .values()
            .filter(|a| {
                s.bundles
                    .get(&a.bundle)
                    .and_then(|b| b.package.as_ref())
                    .is_some_and(|at| at.package == package)
                    && s.may_upgrade(a, &account)
            })
            .cloned()
            .collect()
    });
    let who = Upgrader {
        caller: Caller::Account {
            account: account.clone(),
        },
        actor: account,
        by_operator: false,
        consent,
        run: None,
    };
    let run = OrgUpgradeBody {
        package,
        version: body.version,
        team: None,
        cascade: false,
        migrate: body.migrate,
        confirm: None,
    };
    start_run(node, org, run, apps, who).await
}

/// [`org_upgrade`] by the operator, where the deployment gave it app upgrades.
async fn ops_org_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<OrgUpgradeBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if !node.sys(|s| s.orgs.contains_key(&org)) {
        return Err(ApiError::not_found("no such organization"));
    }
    let apps = run_targets(
        &node,
        &org,
        &body.package,
        body.team.as_deref(),
        body.cascade,
    );
    let who = Upgrader {
        caller: Caller::Public,
        actor: String::new(),
        by_operator: true,
        consent: Value::Null,
        run: None,
    };
    start_run(node, org, body, apps, who).await
}

/// An organization-wide upgrade's progress, cascade included.
async fn org_upgrade_status(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, run)): Path<(String, String)>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let r = node
        .sys(|s| {
            let o = s.orgs.get(&org)?;
            o.role(&account)?;
            o.upgrade_runs.get(&run).cloned()
        })
        .ok_or_else(|| ApiError::not_found("no such upgrade"))?;
    Ok(Json(serde_json::to_value(r).unwrap_or_default()))
}

/// Run an organization-wide upgrade again for the apps that failed, under the
/// same terms, as a new run.
async fn org_upgrade_retry(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, run)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> ApiResult {
    let prev = node
        .sys(|s| s.orgs.get(&org)?.upgrade_runs.get(&run).cloned())
        .ok_or_else(|| ApiError::not_found("no such upgrade"))?;
    let failed: Vec<crate::types::AppRecord> = node.sys(|s| {
        prev.failed
            .keys()
            .filter_map(|a| s.apps.get(a).cloned())
            .collect()
    });
    let mut retry = OrgUpgradeBody {
        package: prev.package.clone(),
        version: prev.version.clone(),
        team: prev.team.clone(),
        cascade: prev.cascade,
        migrate: None,
        confirm: body.and_then(|Json(b)| serde_json::from_value(b["confirm"].clone()).ok()),
    };
    if prev.by == crate::system::OPERATOR {
        operator(&node, &headers)?;
        operator_holds(&node, |g| g.app_upgrades, "app upgrades")?;
        let who = Upgrader {
            caller: Caller::Public,
            actor: String::new(),
            by_operator: true,
            consent: Value::Null,
            run: None,
        };
        return start_run(node, org, retry, failed, who).await;
    }
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if !node.sys(|s| s.manages_apps(&org, &account, retry.team.as_deref())) {
        return Err(ApiError::forbidden(
            "upgrading the organization's apps needs MANAGE_APPLICATION there",
        ));
    }
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__upgrade_org_retry",
        &json!({ "run": run }),
        retry.confirm.take(),
    )
    .await?;
    let who = Upgrader {
        caller: Caller::Account {
            account: account.clone(),
        },
        actor: account,
        by_operator: false,
        consent,
        run: None,
    };
    start_run(node, org, retry, failed, who).await
}

#[derive(Deserialize)]
struct AliasBody {
    app: String,
}

/// Name an app for this account (an alias).
async fn alias_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
    Json(body): Json<AliasBody>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let app = node.canonical_app_id(&body.app);
    let rec = node.app(&app).await?;
    p.within(&node, &rec)?;
    if !reaches(&node, &rec, Some(&account)) {
        return Err(ApiError::not_found("no such app"));
    }
    let _ok = node
        .sys_write(&SystemCmd::SetAlias {
            account,
            alias,
            app: Some(app),
        })
        .await?;
    Ok(Json(json!({})))
}

async fn alias_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetAlias {
            account,
            alias,
            app: None,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn alias_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let app = node
        .sys(|s| s.accounts.get(&account)?.aliases.get(&alias).cloned())
        .ok_or_else(|| ApiError::not_found("no such alias"))?;
    Ok(Json(json!({ "alias": alias, "appId": app })))
}

async fn alias_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let aliases = node.sys(|s| {
        s.accounts
            .get(&account)
            .map(|a| a.aliases.clone())
            .unwrap_or_default()
    });
    Ok(Json(json!({ "aliases": aliases })))
}

#[derive(Deserialize)]
struct AccessBody {
    access: Access,
}

async fn app_access(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<AccessBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    p.within(&node, &node.app(&app_id).await?)?;
    let _ok = node
        .sys_write(&SystemCmd::SetAccess {
            app_id,
            access: body.access,
            actor: account,
        })
        .await?;
    Ok(Json(json!({})))
}

// ---------------------------------------------------------------- organizations

#[derive(Deserialize)]
struct OrgBody {
    name: String,
}

async fn org_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OrgBody>,
) -> ApiResult {
    let account = person(&node, &headers).await?.account()?;
    let org_id = typed_id::<16>();
    let v = node
        .sys_write(&SystemCmd::CreateOrg {
            org_id,
            name: body.name,
            actor: account,
            release: Some(node.own_release()),
        })
        .await?;
    Ok(Json(v))
}

async fn org_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    node.sys_fresh().await;
    let o = node
        .sys(|s| s.orgs.get(&org).cloned())
        .ok_or_else(|| ApiError::not_found("no such organization"))?;
    if o.role(&account).is_none() {
        return Err(ApiError::not_found("no such organization"));
    }
    // Teams a member does not see are left out, as if they did not exist.
    let mut v = serde_json::to_value(&o).unwrap_or_default();
    if let Some(teams) = v.get_mut("teams").and_then(Value::as_object_mut) {
        teams.retain(|id, _| o.sees_team(&account, id));
    }
    // Who may approve its releases, the keys it publishes, and whether a
    // passed deadline has stopped new work on its own set (`null`: it has
    // not).
    let (approvers, publishes, stall) = node.sys(|s| {
        (
            s.release_approvers(&org),
            s.publishers
                .iter()
                .filter(|(_, b)| b.org == org)
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>(),
            s.stalled_for(Some(&org), "", now_secs()),
        )
    });
    if let Some(obj) = v.as_object_mut() {
        let _prev = obj.insert("releaseApprovers".into(), json!(approvers));
        let _prev = obj.insert("publishes".into(), json!(publishes));
        let _prev = obj.insert(
            "newWorkStopped".into(),
            stall.map_or(
                Value::Null,
                |st| json!({ "release": st.release, "after": crate::system::rfc3339(st.after) }),
            ),
        );
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct MetadataBody {
    metadata: Value,
}

#[derive(Deserialize, Default)]
struct DeleteOrgBody {
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Delete an organization, its teams, invites and apps: the owner's, with a
/// passkey assertion over this exact deletion (a 412 asks for it).
#[derive(Deserialize)]
struct TransferBody {
    to: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Hand an app to another account: the
/// owner's, with a passkey assertion over this transfer (412 first). Who then
/// upgrades the app, and so which code reads its data, is the new owner's.
async fn app_transfer(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<TransferBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let app = node.app(&app_id).await?;
    if app.owner != actor {
        return Err(ApiError::forbidden("only the app's owner transfers it"));
    }
    let consent = consent(
        &node,
        &p,
        &app_id,
        "__transfer",
        &json!({ "to": body.to }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::TransferApp {
            app_id,
            to: body.to,
            actor,
            consent,
        })
        .await?,
    ))
}

async fn org_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Option<Json<DeleteOrgBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let is_owner =
        node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&actor)) == Some(OrgRole::Owner));
    if !is_owner {
        return Err(ApiError::forbidden(
            "only the organization's owner deletes it",
        ));
    }
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__delete_org",
        &json!({ "org": org }),
        confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::DeleteOrg {
            org,
            actor,
            consent,
        })
        .await?,
    ))
}

async fn org_metadata(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<MetadataBody>,
) -> ApiResult {
    let actor = signed_in(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetOrgMetadata {
            org,
            metadata: body.metadata,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn member_metadata(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, account)): Path<(String, String)>,
    Json(body): Json<MetadataBody>,
) -> ApiResult {
    let actor = signed_in(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetMemberMetadata {
            org,
            account,
            metadata: body.metadata,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn app_metadata(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<MetadataBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = signed_in(&node, &headers).await?;
    let actor = p.account()?;
    p.within(&node, &node.app(&app_id).await?)?;
    let _ok = node
        .sys_write(&SystemCmd::SetAppMetadata {
            app_id,
            metadata: body.metadata,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct PasskeyLabelBody {
    label: String,
}

/// Name one of the caller's passkeys (a device label).
async fn passkey_label(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(cred): Path<String>,
    Json(body): Json<PasskeyLabelBody>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetPasskeyLabel {
            account,
            credential: cred,
            label: body.label,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct MemberBody {
    account: String,
    role: OrgRole,
}

async fn org_set_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<MemberBody>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetOrgRole {
            org,
            account: body.account,
            role: body.role,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn org_remove_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, account)): Path<(String, String)>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::RemoveOrgMember {
            org,
            account,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct TeamBody {
    name: String,
    #[serde(default)]
    parent: Option<String>,
    join: TeamJoin,
}

async fn team_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<TeamBody>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let v = node
        .sys_write(&SystemCmd::CreateTeam {
            org,
            team_id: typed_id::<16>(),
            name: body.name,
            parent: body.parent,
            join: body.join,
            actor,
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct TeamMemberBody {
    account: String,
    #[serde(default)]
    lead: bool,
    #[serde(default)]
    role: Option<crate::types::TeamRole>,
    #[serde(default)]
    capabilities: Option<u32>,
}

async fn team_set_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
    Json(body): Json<TeamMemberBody>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetTeamMember {
            org,
            team,
            account: body.account,
            lead: body.lead,
            role: body.role,
            capabilities: body.capabilities,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

/// A team as `account` may see it: its record, its members with their roles,
/// the teams below it they see, and its apps. `None` when they may not see it
/// (a restricted team they are not in looks like no team at all).
fn team_view(
    s: &crate::system::SystemState,
    o: &crate::types::Org,
    team: &str,
    account: &str,
) -> Option<Value> {
    if !o.sees_team(account, team) {
        return None;
    }
    let t = o.teams.get(team)?;
    let members: Vec<Value> = t
        .members
        .keys()
        .map(|a| {
            json!({
                "account": a,
                "role": o.team_role(a, team),
                "capabilities": t.capabilities.get(a).copied().unwrap_or(o.default_capabilities),
            })
        })
        .collect();
    let children: Vec<&String> = o
        .teams
        .values()
        .filter(|c| c.parent.as_deref() == Some(team) && o.sees_team(account, &c.team_id))
        .map(|c| &c.team_id)
        .collect();
    let apps: Vec<Value> = s
        .apps
        .values()
        .filter(|a| {
            a.org.as_deref() == Some(o.org_id.as_str())
                && matches!(&a.access, Access::Team { team: t } if t == team)
        })
        .map(|a| json!({ "appId": a.app_id, "name": a.name, "contextId": hex::encode(sha256(a.app_id.as_bytes())) }))
        .collect();
    Some(json!({
        "teamId": t.team_id,
        "name": t.name,
        "parent": t.parent,
        "join": t.join,
        "metadata": t.metadata,
        "members": members,
        "children": children,
        "apps": apps,
        "myRole": o.team_role(account, team),
        "member": o.in_team(account, team),
    }))
}

async fn team_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    node.sys_fresh().await;
    let v = node
        .sys(|s| {
            let o = s.orgs.get(&org)?;
            o.role(&account)?;
            team_view(s, o, &team, &account)
        })
        .ok_or_else(|| ApiError::not_found("no such team"))?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamUpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    join: Option<TeamJoin>,
    /// Present (even `null`) to move the team; `null` moves it to the top.
    #[serde(default, deserialize_with = "crate::types::present")]
    parent: Option<Option<String>>,
    #[serde(default)]
    metadata: Option<Value>,
}

async fn team_update(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
    Json(body): Json<TeamUpdateBody>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::UpdateTeam {
            org,
            team,
            name: body.name,
            join: body.join,
            parent: body.parent,
            metadata: body.metadata,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn team_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let v = node
        .sys_write(&SystemCmd::DeleteTeam { org, team, actor })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapabilitiesBody {
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    team: Option<String>,
    capabilities: u32,
}

/// A member's capabilities (`account`, organization-wide or in `team`), or,
/// without an account, the organization's default.
async fn org_capabilities(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<CapabilitiesBody>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let cmd = match body.account {
        Some(account) => SystemCmd::SetCapabilities {
            org,
            team: body.team,
            account,
            capabilities: body.capabilities,
            actor,
        },
        None => SystemCmd::SetDefaultCapabilities {
            org,
            capabilities: body.capabilities,
            actor,
        },
    };
    let _ok = node.sys_write(&cmd).await?;
    Ok(Json(json!({})))
}

async fn team_remove_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team, account)): Path<(String, String, String)>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::RemoveTeamMember {
            org,
            team,
            account,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InviteBody {
    role: OrgRole,
    #[serde(default)]
    teams: Vec<String>,
    #[serde(default)]
    ttl_secs: Option<u64>,
    #[serde(default)]
    uses: Option<u32>,
}

async fn invite_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<InviteBody>,
) -> ApiResult {
    let actor = person(&node, &headers).await?.account()?;
    let token = format!("inv_{}", b64url(&random::<24>()));
    let _ok = node
        .sys_write(&SystemCmd::CreateInvite {
            hash: b64url(&sha256(token.as_bytes())),
            invite: Invite {
                org,
                role: body.role,
                teams: body.teams,
                expires_at: now_secs() + body.ttl_secs.unwrap_or(7 * 24 * 3600),
                uses_left: body.uses.unwrap_or(1).max(1),
            },
            actor,
        })
        .await?;
    Ok(Json(json!({ "token": token })))
}

async fn invite_redeem(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> ApiResult {
    let account = person(&node, &headers).await?.account()?;
    let v = node
        .sys_write(&SystemCmd::RedeemInvite {
            hash: b64url(&sha256(token.as_bytes())),
            account,
            now: now_secs(),
        })
        .await?;
    Ok(Json(v))
}

// ---------------------------------------------------------------- internal (peers)

/// Compare secrets without leaking where they differ.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn peer_only(node: &Node, headers: &HeaderMap) -> Result<(), ApiError> {
    let ok = headers
        .get("x-cluster-auth")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| ct_eq(v.as_bytes(), node.cluster_secret().as_bytes()));
    if ok {
        Ok(())
    } else {
        Err(ApiError::forbidden("peers only"))
    }
}

/// How many cluster-key requests one node judges at once (each verifies a
/// quote, which in TDX mode fetches a release); past it, `429`.
pub const MAX_CONCURRENT_KEY_RELEASES: usize = 4;

/// `POST /internal/cluster-key`: a new node (often of another mero-tee
/// release, so without the cluster secret) asks for K_c. It is delivered, not
/// answered: to the address the cluster has for that node id (registered, or
/// in this node's `--peers`), inside a sealed channel to the key the TD there
/// attests (`cluster_key::deliver`). Refused for another cluster, an unknown
/// or removed node id; at most `MAX_CONCURRENT_KEY_RELEASES` at once.
async fn internal_cluster_key(
    State(node): State<Arc<Node>>,
    Json(body): Json<crate::cluster_key::Request>,
) -> ApiResult {
    let Ok(_permit) = node.key_releases.try_acquire() else {
        return Err(ApiError::new(
            429,
            "BUSY",
            "too many cluster-key requests; retry",
        ));
    };
    if body.cluster != node.config.cluster {
        return Err(ApiError::forbidden("another cluster"));
    }
    let id = body.node;
    let addr = node.sys(|s| {
        if s.removed_nodes.contains(&id) {
            return None;
        }
        s.nodes
            .get(&id)
            .map(|n| n.addr.clone())
            .or_else(|| node.config.peers.get(&id).cloned())
    });
    let Some(addr) = addr else {
        return Err(ApiError::forbidden(
            "not a node of this cluster: register it (/ops/nodes) first",
        ));
    };
    crate::cluster_key::deliver(
        &node.config,
        &node.http,
        &addr,
        &body.token,
        node.cluster_key(),
    )
    .await
    .map_err(|e| {
        warn!(node = id, error = %format!("{e:#}"), "could not deliver the cluster key");
        ApiError::forbidden("the cluster key could not be delivered to that node's address")
    })?;
    info!(node = id, "delivered the cluster key");
    Ok(Json(json!({ "delivered": true })))
}

#[derive(Deserialize)]
struct RaftBody {
    group: u64,
    rpc: calimero_log::RpcRequest,
}

async fn internal_raft(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<RaftBody>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let r = node.engine.handle_rpc(body.group, body.rpc).await;
    Ok(Json(serde_json::to_value(r).unwrap_or_default()))
}

async fn internal_system(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(cmd): Json<SystemCmd>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    // Only the leader answers; a follower refuses rather than forwarding again.
    let bytes = serde_json::to_vec(&cmd).map_err(|e| ApiError::internal(&e.to_string()))?;
    match node.system.write(bytes).await {
        Ok((_i, out)) => crate::system::output(&out)
            .map(Json)
            .map_err(|e| crate::node::rejected(&e)),
        Err(e) => Err(ApiError::unavailable(&e.to_string())),
    }
}

#[derive(Deserialize)]
struct BlobBody {
    hash: String,
    #[serde(default)]
    wasm: Option<calimero_log::Bytes>,
}

async fn internal_blob_put(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<BlobBody>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let wasm = body
        .wasm
        .ok_or_else(|| ApiError::bad_request("BAD_REQUEST", "wasm missing"))?;
    if hex::encode(sha256(&wasm.0)) != body.hash {
        return Err(ApiError::bad_request("BAD_REQUEST", "hash does not match"));
    }
    node.engine
        .put_blob(&body.hash, &wasm.0)
        .map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(json!({})))
}

async fn internal_blob_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<BlobBody>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let wasm = node
        .engine
        .get_blob(&body.hash)
        .map_err(|e| ApiError::internal(&e.to_string()))?
        .ok_or_else(|| ApiError::not_found("no such blob here"))?;
    Ok(Json(json!({ "wasm": calimero_log::Bytes(wasm) })))
}

/// A blob id as a client gives it: base58 (what apps print, `BlobId`) or hex.
fn parse_blob_id(id: &str) -> Result<[u8; 32], ApiError> {
    if let Ok(raw) = hex::decode(id) {
        if let Ok(b) = <[u8; 32]>::try_from(raw.as_slice()) {
            return Ok(b);
        }
    }
    id.parse::<calimero_primitives::blobs::BlobId>()
        .map(|b| *b.as_ref())
        .map_err(|_| ApiError::bad_request("BAD_REQUEST", "not a blob id"))
}

fn blob_json(id: &[u8; 32], size: u64) -> Value {
    json!({
        "blobId": calimero_primitives::blobs::BlobId::from(*id).to_string(),
        "id": hex::encode(id),
        "size": size,
    })
}

/// Upload a blob for an app: the body is its bytes. Anyone who may call the
/// app may add one; its runs then open it by id.
async fn blob_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "__blob", true).await?;
    // After the gate: one who can't reach the app learns nothing of it (404).
    no_component_blobs(&app)?;
    if p.caller.account().is_none() {
        return Err(ApiError::unauthenticated("sign in to upload"));
    }
    let id = node.store_blob(&body).await?;
    let size = body.len() as u64;
    let a = node
        .call(&CallRequest::host(
            app_id,
            "__blob",
            json!({ "id": id, "size": size }),
            p.caller,
        ))
        .await?;
    let raw = parse_blob_id(&id)?;
    let mut out = blob_json(&raw, size);
    out["index"] = json!(a.index);
    Ok(Json(out))
}

/// Download one of an app's blobs.
async fn blob_download(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "__blob", false).await?;
    // After the gate: one who can't reach the app learns nothing of it (404).
    no_component_blobs(&app)?;
    let raw = parse_blob_id(&id)?;
    // The app's leader decides: a replica that has not applied a delete yet
    // still holds the mark and the bytes.
    match node.blob_held_strong(&app, &raw).await {
        Some(true) => {}
        Some(false) => return Err(ApiError::not_found("no such blob in this app")),
        None => return Err(ApiError::unavailable("the app's leader did not answer")),
    }
    let hex_id = hex::encode(raw);
    let bytes = match node
        .engine
        .get_blob(&hex_id)
        .map_err(|e| ApiError::internal(&e.to_string()))?
    {
        Some(b) => b,
        None => node.fetch_blob(&hex_id).await?,
    };
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response())
}

/// The blob routes are the wasmer executor's: a component has no blobs until R8.
fn no_component_blobs(app: &crate::types::AppRecord) -> Result<(), ApiError> {
    if app.executor == crate::types::Executor::Component {
        return Err(ApiError::new(
            409,
            "NOT_SUPPORTED",
            "this app is a component: blobs are not available to it yet (R8)",
        ));
    }
    Ok(())
}

/// Whether `account` may manage `app`'s data (its owner, or an admin of its
/// organization): delete its blobs.
fn manages_app(node: &Node, app: &crate::types::AppRecord, account: Option<&str>) -> bool {
    let Some(a) = account else {
        return false;
    };
    app.owner == a
        || app
            .org
            .as_deref()
            .is_some_and(|o| node.sys(|s| s.is_org_admin(o, a)))
}

/// The app's blobs.
async fn blob_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "__blob", false).await?;
    // After the gate: one who can't reach the app learns nothing of it (404).
    no_component_blobs(&app)?;
    let stats = node.storage_stats(&app).await?;
    Ok(Json(json!({
        "blobs": stats.blobs.iter().map(|(id, size)| blob_json(id, *size)).collect::<Vec<_>>(),
    })))
}

/// One blob's size; 404 if the app has no such blob.
async fn blob_info(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "__blob", false).await?;
    // After the gate: one who can't reach the app learns nothing of it (404).
    no_component_blobs(&app)?;
    let raw = parse_blob_id(&id)?;
    let stats = node.storage_stats(&app).await?;
    let size = stats
        .blobs
        .iter()
        .find(|(b, _)| *b == raw)
        .map(|(_, s)| *s)
        .ok_or_else(|| ApiError::not_found("no such blob in this app"))?;
    Ok(Json(blob_json(&raw, size)))
}

/// Delete one of the app's blobs: its owner's or an org admin's. The app
/// forgets it at once; the bytes leave every node unless another app marks
/// the same content.
async fn blob_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "__blob", true).await?;
    // After the gate: one who can't reach the app learns nothing of it (404).
    no_component_blobs(&app)?;
    if !manages_app(&node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner or an org admin deletes its blobs",
        ));
    }
    let raw = parse_blob_id(&id)?;
    if !node.blob_held(&app, &raw).await? {
        return Err(ApiError::not_found("no such blob in this app"));
    }
    let a = node
        .call(&CallRequest::host(
            app_id,
            "__blob_forget",
            json!({ "id": hex::encode(raw) }),
            p.caller,
        ))
        .await?;
    node.collect_blob(&raw).await;
    Ok(Json(json!({ "index": a.index })))
}

/// What the app's storage holds: keys, bytes, blobs.
async fn app_storage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let stats = node.storage_stats(&app).await?;
    Ok(Json(json!({
        "keys": stats.keys,
        "bytes": stats.bytes,
        "blobs": stats.blobs.len(),
        "blobBytes": stats.blob_bytes,
    })))
}

/// The ABI the app's bundle embeds; `null` for none.
async fn app_abi(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    if app.executor == crate::types::Executor::Component {
        // The ABI the component embeds (its `calimero:abi` section), of the
        // bundle the app group runs.
        let bundle = node.running_bundle(&app);
        let wasm = node.bundle_wasm(&bundle).await?;
        let abi = crate::component::embedded_abi(&wasm);
        return Ok(Json(json!({ "bundle": bundle, "abi": abi })));
    }
    let wasm = node.bundle_wasm(&app.bundle).await?;
    let abi = calimero_wasm_abi::embed::read_embedded_state_schema(&wasm);
    Ok(Json(json!({ "bundle": app.bundle, "abi": abi })))
}

/// What the caller's account uses, against the cluster's quotas.
async fn usage(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    Ok(Json(node.sys(|s| {
        json!({
            "apps": s.apps.values().filter(|a| a.owner == account).count(),
            "bundleBytes": s
                .bundles
                .values()
                .filter(|b| b.uploaded_by == account)
                .map(|b| b.size)
                .sum::<u64>(),
            "apiKeys": s.api_keys.values().filter(|k| k.account == account).count(),
            "orgsOwned": s
                .orgs
                .values()
                .filter(|o| o.role(&account) == Some(OrgRole::Owner))
                .count(),
            "quotas": s.quotas,
        })
    })))
}

async fn internal_storage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let app = body["app"].as_str().unwrap_or_default();
    let g = node
        .hosted(app)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    let stats = g.read_local(|m, _| m.storage_stats());
    Ok(Json(json!({
        "keys": stats.keys,
        "bytes": stats.bytes,
        "blobBytes": stats.blob_bytes,
        "blobs": stats.blobs.iter().map(|(id, s)| (hex::encode(id), *s)).collect::<Vec<_>>(),
    })))
}

/// Delete a blob's bytes here, at the request of the node that collected it.
/// Code (a bundle, a package release) is never deleted this way.
async fn internal_blob_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let hash = body["hash"].as_str().unwrap_or_default().to_owned();
    let code = node.sys(|s| {
        s.bundles.contains_key(&hash)
            || s.packages
                .values()
                .any(|p| p.versions.values().any(|v| v.mpk == hash))
    });
    if !code {
        node.engine
            .delete_blob(&hash)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
    }
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct HeldBody {
    app: String,
    id: String,
    #[serde(default)]
    strong: bool,
}

async fn internal_blob_held(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<HeldBody>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let app = node.app(&body.app).await?;
    let raw = parse_blob_id(&body.id)?;
    let g = node
        .hosted(&app.app_id)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    // `strong`: answered only by the app's leader, after it confirmed it leads.
    let held = if body.strong {
        g.read_strong(|m, _| m.holds_blob(&raw))
            .await
            .map_err(|e| ApiError::unavailable(&e.to_string()))?
    } else {
        g.read_local(|m, _| m.holds_blob(&raw))
    };
    Ok(Json(json!({ "held": held })))
}

/// The system leader's applied index, once it confirmed it leads.
async fn internal_system_index(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    peer_only(&node, &headers)?;
    let index = node
        .system
        .read_strong(|_, applied| applied)
        .await
        .map_err(|e| ApiError::unavailable(&e.to_string()))?;
    Ok(Json(json!({ "index": index })))
}

async fn internal_call(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(req): Json<CallRequest>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    Ok(Json(node.call_here_any(&req).await?))
}

async fn internal_view(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(req): Json<ViewRequest>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let a = node.view(&req).await?;
    Ok(Json(serde_json::to_value(a).unwrap_or_default()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyBody {
    app_id: String,
}

async fn internal_app_ready(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<ReadyBody>,
) -> ApiResult {
    peer_only(&node, &headers)?;
    let state = node
        .hosted(&b.app_id)
        .and_then(|g| g.read_local(|m, _| init_state(&m.state)));
    Ok(Json(match state {
        Some(Ok(())) => json!({ "ready": true }),
        Some(Err(e)) => json!({ "ready": true, "initError": e }),
        None => json!({ "ready": false }),
    }))
}

#[derive(Deserialize)]
struct InternalEvents {
    app: String,
    #[serde(default)]
    from: u64,
    /// The account a component's events are filtered for; empty: anonymous.
    #[serde(default)]
    subscriber: String,
}

async fn internal_events(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<InternalEvents>,
) -> Result<Response, ApiError> {
    peer_only(&node, &headers)?;
    let subscriber = Some(q.subscriber).filter(|s| !s.is_empty());
    let stream = local_events(&node, &q.app, q.from, subscriber)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

// ---------------------------------------------------------------- ops (mdma)

fn operator(node: &Node, headers: &HeaderMap) -> Result<(), ApiError> {
    let given = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    // The token itself, or (`--ops-token-sha256`) only its hash, so the
    // machine's metadata need not hold a usable credential.
    let ok = match (&node.config.ops_token, &node.config.ops_token_sha256) {
        (Some(expected), _) => ct_eq(given.as_bytes(), expected.as_bytes()),
        (None, Some(hash)) => ct_eq(
            hex::encode(sha256(given.as_bytes())).as_bytes(),
            hash.to_ascii_lowercase().as_bytes(),
        ),
        (None, None) => return Err(ApiError::forbidden("the Ops API is disabled on this node")),
    };
    if ok {
        Ok(())
    } else {
        Err(ApiError::forbidden("operator credential required"))
    }
}

/// Prometheus metrics, behind the operator token: counts,
/// this node's groups and its streams. Never names, state or credentials.
async fn ops_metrics(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    operator(&node, &headers)?;
    let disk = crate::metrics::disk_sample(&node.config.data_dir).await;
    Ok(metrics_response(&node, disk))
}

/// `GET /metrics` on `--metrics-listen`: the same text, in plain HTTP.
pub async fn plain_metrics(State(node): State<Arc<Node>>) -> Response {
    let disk = crate::metrics::disk_sample(&node.config.data_dir).await;
    metrics_response(&node, disk)
}

/// Prometheus text: directory counts, this node's groups and streams, system
/// membership, the build's release; never names or state.
fn metrics_response(node: &Node, disk: (u64, u64)) -> Response {
    use std::fmt::Write as _;
    let (nodes, apps, accounts, orgs, packages, bundle_bytes) = node.sys(|s| {
        (
            s.nodes.len(),
            s.apps.len(),
            s.accounts.len(),
            s.orgs.len(),
            s.packages.len(),
            s.bundles.values().map(|b| b.size).sum::<u64>(),
        )
    });
    let system = node.system.status();
    let hosted: Vec<(String, calimero_log::Status)> = node
        .sys(|s| s.apps.keys().cloned().collect::<Vec<_>>())
        .into_iter()
        .filter_map(|a| node.hosted(&a).map(|g| (a, g.status())))
        .collect();
    let mut out = String::new();
    let mut gauge = |name: &str, help: &str, value: u64| {
        let _r = writeln!(
            out,
            "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}"
        );
    };
    gauge(
        "calimero_cluster_nodes",
        "Nodes in the directory.",
        nodes as u64,
    );
    gauge(
        "calimero_apps",
        "App instances in the directory.",
        apps as u64,
    );
    gauge("calimero_accounts", "Accounts.", accounts as u64);
    gauge("calimero_orgs", "Organizations.", orgs as u64);
    gauge("calimero_packages", "Installed packages.", packages as u64);
    gauge(
        "calimero_bundle_bytes",
        "Bytes of uploaded bundles.",
        bundle_bytes,
    );
    gauge(
        "calimero_hosted_groups",
        "App groups this node holds a replica of.",
        hosted.len() as u64,
    );
    gauge(
        "calimero_leading_groups",
        "App groups this node leads.",
        hosted.iter().filter(|(_, s)| s.is_leader).count() as u64,
    );
    gauge(
        "calimero_event_streams",
        "Event and presence streams open on this node.",
        node.event_streams
            .load(std::sync::atomic::Ordering::Relaxed) as u64,
    );
    gauge(
        "calimero_system_applied",
        "The system group's applied index here.",
        system.applied,
    );
    gauge(
        "calimero_system_leader",
        "1 when this node leads the system group.",
        u64::from(system.is_leader),
    );
    gauge(
        "calimero_system_voters",
        "System group voters.",
        system.voters.len() as u64,
    );
    gauge(
        "calimero_system_learners",
        "System group learners.",
        system.learners.len() as u64,
    );
    let _r = writeln!(
        out,
        "# HELP calimero_build_conflicts_total Entries this node appended that applied as conflicts and ran again.\n# TYPE calimero_build_conflicts_total counter\ncalimero_build_conflicts_total {}",
        node.conflicts.load(std::sync::atomic::Ordering::Relaxed)
    );
    let _r = writeln!(
        out,
        "# HELP calimero_group_applied An app group's applied index here.\n# TYPE calimero_group_applied gauge"
    );
    for (_app, s) in &hosted {
        let _r = writeln!(
            out,
            "calimero_group_applied{{group=\"{}\"}} {}",
            s.group, s.applied
        );
    }
    // Raft per group (the system group as `system`), disk, then requests.
    let mut samples = vec![crate::metrics::raft_sample(
        "system".to_owned(),
        &node.system,
    )];
    for app in node.sys(|s| s.apps.keys().cloned().collect::<Vec<_>>()) {
        if let Some(g) = node.hosted(&app) {
            samples.push(crate::metrics::raft_sample(
                g.status().group.to_string(),
                &g,
            ));
        }
    }
    crate::metrics::render_raft(&mut out, &samples);
    // Per-app usage: only the apps this node hosts keep their series.
    let here: std::collections::HashSet<&str> = hosted.iter().map(|(a, _)| a.as_str()).collect();
    node.usage.retain(|app| here.contains(app));
    node.usage.render(&mut out);
    crate::metrics::render_node(&mut out, node.config.node_id, disk);
    crate::metrics::render_process(&mut out);
    crate::metrics::render(&mut out);
    let _r = writeln!(
        out,
        "# HELP calimero_build_info This node's release.\n# TYPE calimero_build_info gauge\ncalimero_build_info{{release=\"{}\"}} 1",
        node.own_release().replace('"', "")
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
        .into_response()
}

async fn ops_status(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    let groups: Vec<Value> = node
        .sys(|s| {
            s.apps
                .values()
                .map(|a| (a.app_id.clone(), a.group))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .filter_map(|(app, _g)| {
            node.hosted(&app)
                .map(|g| json!({ "app": app, "status": g.status() }))
        })
        .collect();
    let (nodes, apps, accounts) = node.sys(|s| {
        (
            s.nodes.values().cloned().collect::<Vec<_>>(),
            s.apps.len(),
            s.accounts.len(),
        )
    });
    // Counts and ids only: the Ops API never returns app state, names or
    // credentials (`/ops/placement` is the same: ids, zones, releases, counts).
    Ok(Json(json!({
        "node": node.config.node_id,
        "system": node.system.status(),
        "governance": node.sys(crate::system::SystemState::governance),
        "groups": groups,
        "nodes": nodes,
        "apps": apps,
        "accounts": accounts,
    })))
}

/// How long `/ops/placement` waits for a node it has not attested yet to name
/// its release; one that does not answer by then is listed with none.
const PLACEMENT_RELEASE_WAIT: Duration = Duration::from_secs(2);

/// Where every app's replicas are, for a rebalancer (mdma): each app's id,
/// group and replica node ids, and each node's id, zone, attested release and
/// app count. Ids and counts only: no names, state or credentials. Read from
/// the system state after a read index (`fresh` says whether this replica
/// caught up with the system leader).
async fn ops_placement(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    let fresh = node.sys_fresh().await;
    let (apps, nodes) = node.sys(|s| {
        let apps: Vec<Value> = s
            .apps
            .values()
            .map(|a| json!({ "app": a.app_id, "group": a.group, "replicas": a.replicas }))
            .collect();
        let nodes: Vec<(u64, String, usize)> = s
            .nodes
            .values()
            .map(|n| {
                let hosted = s
                    .apps
                    .values()
                    .filter(|a| a.replicas.contains(&n.id))
                    .count();
                (n.id, n.zone.clone(), hosted)
            })
            .collect();
        (apps, nodes)
    });
    let releases =
        futures_util::future::join_all(nodes.iter().map(|(id, _, _)| {
            tokio::time::timeout(PLACEMENT_RELEASE_WAIT, node.release_of_node(*id))
        }))
        .await;
    let nodes: Vec<Value> = nodes
        .into_iter()
        .zip(releases)
        .map(|((id, zone, hosted), release)| {
            json!({ "id": id, "zone": zone, "release": release.ok().flatten(), "apps": hosted })
        })
        .collect();
    Ok(Json(
        json!({ "fresh": fresh, "apps": apps, "nodes": nodes }),
    ))
}

/// A page of a report (`ops/apps`, `ops/orgs`, `orgs/{org}/apps`,
/// `auth/apps`): the entries after the id `after`, at most `limit` of them
/// (default and most: [`MAX_REPORT_PAGE`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

impl PageQuery {
    fn limit(&self) -> Result<usize, ApiError> {
        match self.limit {
            None => Ok(MAX_REPORT_PAGE),
            Some(n) if (1..=MAX_REPORT_PAGE).contains(&n) => Ok(n),
            Some(_) => Err(ApiError::bad_request(
                "BAD_LIMIT",
                &format!("limit is 1 to {MAX_REPORT_PAGE}"),
            )),
        }
    }
}

/// Every registered node's attested release, attesting a node first if this
/// one has not yet (each waits at most [`PLACEMENT_RELEASE_WAIT`]).
async fn attested_releases(node: &Node) -> BTreeMap<u64, Option<String>> {
    let ids: Vec<u64> = node.sys(|s| s.nodes.keys().copied().collect());
    let releases = futures_util::future::join_all(
        ids.iter()
            .map(|id| tokio::time::timeout(PLACEMENT_RELEASE_WAIT, node.release_of_node(*id))),
    )
    .await;
    ids.into_iter()
        .zip(releases)
        .map(|(id, r)| (id, r.ok().flatten()))
        .collect()
}

/// What every app runs, for the operator (mdma): per app its id, group,
/// scope (organization id or `account`, never the account), bundle digest,
/// package release only where the operator may see it
/// (`SystemState::sees_release`: the releases it installed), who holds app
/// upgrades, its last upgrade by actor kind, the releases approved to hold
/// it, and each replica's release and whether it is approved. Paged.
async fn ops_apps(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    operator(&node, &headers)?;
    let limit = q.limit()?;
    let fresh = node.sys_fresh().await;
    let releases = attested_releases(&node).await;
    let (apps, next, governance) = node.sys(|s| {
        let (page, next) = report::page(s, q.after.as_deref(), limit, |_| true);
        let apps: Vec<Value> = page
            .into_iter()
            .map(|a| report::app_json(s, a, &releases, Reader::Operator))
            .collect();
        (apps, next, s.governance())
    });
    Ok(Json(json!({
        "fresh": fresh,
        "governance": governance,
        "apps": apps,
        "next": next,
    })))
}

/// Each organization's id and the releases approved to hold its apps, for
/// the operator. Paged like [`ops_apps`].
async fn ops_orgs(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    operator(&node, &headers)?;
    let limit = q.limit()?;
    let fresh = node.sys_fresh().await;
    let (orgs, next, governance) = node.sys(|s| {
        let start = q.after.clone().map_or(Bound::Unbounded, Bound::Excluded);
        let mut ids: Vec<&String> = s
            .orgs
            .range((start, Bound::Unbounded))
            .map(|(id, _)| id)
            .take(limit.saturating_add(1))
            .collect();
        let next = (ids.len() > limit).then(|| {
            ids.truncate(limit);
            ids.last().map(|id| (*id).clone())
        });
        let orgs: Vec<Value> = ids
            .into_iter()
            .map(|id| {
                json!({
                    "org": id,
                    "approvedReleases": report::approved_releases(s.approved_for(Some(id), "")),
                })
            })
            .collect();
        (orgs, next.flatten(), s.governance())
    });
    Ok(Json(json!({
        "fresh": fresh,
        "governance": governance,
        "orgs": orgs,
        "next": next,
    })))
}

/// The report of one approval scope's apps for an account: an
/// organization's (`org`) or the account's own outside one (`None`). Lists
/// the apps the account reaches or may upgrade, within what its credential
/// is scoped to (a package-bound session: that package's apps; an API key
/// naming apps: those).
async fn scope_report(
    node: &Node,
    p: &Principal,
    account: &str,
    org: Option<&str>,
    q: &PageQuery,
    limit: usize,
) -> Value {
    // A peer is attested once and its release kept (`Node::release_of_node`),
    // so this contacts a node only the first time, at most
    // `PLACEMENT_RELEASE_WAIT` each.
    let releases = attested_releases(node).await;
    let bound = p.bound_app();
    let key_apps = p.api_key.as_ref().and_then(|k| k.apps.as_ref());
    node.sys(|s| {
        let keep = |a: &crate::types::AppRecord| {
            let in_scope = match org {
                Some(o) => a.org.as_deref() == Some(o),
                None => a.org.is_none() && a.owner == account,
            };
            in_scope
                && (s.may_reach(a, Some(account)) || s.may_upgrade(a, account))
                && bound.is_none_or(|pkg| {
                    s.bundles
                        .get(&a.bundle)
                        .and_then(|b| b.package.as_ref())
                        .is_some_and(|r| r.package == pkg)
                })
                && key_apps.is_none_or(|apps| apps.contains(&a.app_id))
        };
        let (page, next) = report::page(s, q.after.as_deref(), limit, keep);
        let apps: Vec<Value> = page
            .into_iter()
            .map(|a| report::app_json(s, a, &releases, Reader::Account(account)))
            .collect();
        let approved = s.approved_for(org, account);
        let scope = match org {
            Some(o) => json!({ "kind": "org", "org": o }),
            None => json!({ "kind": "account" }),
        };
        json!({
            "scope": scope,
            "governance": s.governance(),
            "approvedReleases": report::approved_releases(approved),
            "clusterReleases": report::cluster_releases(&releases, approved),
            "apps": apps,
            "next": next,
        })
    })
}

/// An organization's apps, what they run and who upgraded them, the
/// releases it approved and those the cluster's nodes run: for any of its
/// members; `404` to anyone else, as `GET /orgs/{org}`.
async fn org_apps(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let limit = q.limit()?;
    let member = node.sys(|s| s.orgs.get(&org).is_some_and(|o| o.role(&account).is_some()));
    if !member {
        return Err(ApiError::not_found("no such organization"));
    }
    Ok(Json(
        scope_report(&node, &p, &account, Some(&org), &q, limit).await,
    ))
}

/// The signed-in account's own apps (outside any organization), whose
/// release approvals and upgrades are its own: as [`org_apps`].
async fn account_apps(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let limit = q.limit()?;
    Ok(Json(
        scope_report(&node, &p, &account, None, &q, limit).await,
    ))
}

#[derive(Deserialize)]
struct NodeBody {
    id: u64,
    addr: String,
    #[serde(default)]
    zone: String,
}

async fn ops_register_node(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<NodeBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    let _ok = node
        .sys_write(&SystemCmd::RegisterNode {
            node: crate::types::NodeRecord {
                id: b.id,
                addr: b.addr,
                zone: b.zone,
            },
        })
        .await?;
    Ok(Json(json!({})))
}

/// Remove a node: out of the directory and the system group. Refused while an
/// app keeps a replica on it (move them with `/ops/apps/{app}/replicas`). Runs
/// on the system leader; any other node answers `421 NOT_LEADER`. A registered
/// learner takes the removed voter's place while fewer than 3 voters are left.
async fn ops_remove_node(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> ApiResult {
    operator(&node, &headers)?;
    let st = node.system.status();
    if !st.is_leader {
        let addr = st
            .leader
            .and_then(|l| node.sys(|s| s.nodes.get(&l).map(|n| (l, n.addr.clone()))));
        return Err(membership_err(&calimero_log::WriteError::NotLeader {
            leader: addr,
        }));
    }
    let mut voters: std::collections::BTreeSet<u64> =
        st.voters.iter().copied().filter(|v| *v != id).collect();
    if voters.is_empty() {
        return Err(ApiError::bad_request(
            "LAST_NODE",
            "the cluster's last voter cannot be removed",
        ));
    }
    let _ok = node.sys_write(&SystemCmd::RemoveNode { id }).await?;
    let learners: Vec<u64> = node.sys(|s| {
        st.learners
            .iter()
            .copied()
            .filter(|l| *l != id && s.nodes.contains_key(l))
            .collect()
    });
    for l in learners {
        if voters.len() >= 3 {
            break;
        }
        let _new = voters.insert(l);
    }
    node.system
        .change_voters(voters)
        .await
        .map_err(|e| membership_err(&e))?;
    Ok(Json(json!({ "status": node.system.status() })))
}

#[derive(Deserialize)]
struct ReplicaBody {
    replicas: Vec<u64>,
}

/// A membership change failed; a follower names the leader to retry at.
fn membership_err(e: &calimero_log::WriteError) -> ApiError {
    match e {
        calimero_log::WriteError::NotLeader {
            leader: Some((id, addr)),
        } => ApiError::new(421, "NOT_LEADER", "send this to the app group's leader")
            .with_details(json!({ "leader": id, "leaderUrl": addr })),
        _ => ApiError::unavailable(&e.to_string()),
    }
}

/// Move an app to a new replica set: add learners, wait, then switch voters.
/// Runs on the app group's leader; any other replica answers `421 NOT_LEADER`.
/// Safe for a rebalancer to repeat or race: a move to the set the app already
/// has answers `unchanged: true` and writes nothing, and a second move of the
/// same app while one runs here is refused `409 MOVE_IN_PROGRESS`.
async fn ops_set_replicas(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(b): Json<ReplicaBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    operator(&node, &headers)?;
    let target: std::collections::BTreeSet<u64> = b.replicas.iter().copied().collect();
    if target.len() != b.replicas.len() {
        return Err(ApiError::bad_request(
            "DUPLICATE_REPLICA",
            "a node is named twice",
        ));
    }
    if target.len() < 3.min(node.sys(|s| s.nodes.len())) {
        return Err(ApiError::bad_request(
            "TOO_FEW",
            "an app keeps at least 3 replicas",
        ));
    }
    if let Some(id) = node.sys(|s| target.iter().copied().find(|id| !s.nodes.contains_key(id))) {
        return Err(ApiError::bad_request(
            "UNKNOWN_NODE",
            &format!("node {id} is not registered"),
        ));
    }
    let app = node.app(&app_id).await?;
    // The operator moves an app only onto nodes on a release its owner
    // approved: a move is how data would reach a new image.
    for id in &target {
        let release = node.release_of_node(*id).await;
        if !node.sys(|s| s.release_may_hold(&app, release.as_deref())) {
            return Err(ApiError::forbidden(&format!(
                "node {id} runs release {}, which this app's owner has not approved",
                release.as_deref().unwrap_or("(not attested yet)")
            )));
        }
    }
    // Only the leader changes membership; others redirect before writing
    // anything. A node holding no replica names a current replica, which
    // redirects again if it does not lead.
    let to = match node.hosted(&app_id).map(|g| g.status()) {
        Some(st) if st.is_leader => None,
        Some(st) => Some(st.leader),
        None => Some(app.replicas.first().copied()),
    };
    if let Some(id) = to {
        let addr = id.and_then(|id| node.sys(|s| s.nodes.get(&id).map(|n| (id, n.addr.clone()))));
        return Err(membership_err(&calimero_log::WriteError::NotLeader {
            leader: addr,
        }));
    }
    // One move per app at a time: two would each record their own union and
    // final set, and the directory could end naming neither group's voters.
    let Some(moving) = node.begin_move(&app_id) else {
        return Err(ApiError::new(
            409,
            "MOVE_IN_PROGRESS",
            "this app's replicas are being moved; retry once that move answers",
        ));
    };
    // The move runs to the end in a task of its own, which holds the guard:
    // a caller that disconnects does not leave it half done and unguarded.
    tokio::spawn(move_replicas(node, app_id, target, moving))
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?
}

/// The body of [`ops_set_replicas`], on the app group's leader, under `_moving`.
async fn move_replicas(
    node: Arc<Node>,
    app_id: String,
    target: std::collections::BTreeSet<u64>,
    _moving: crate::node::MoveGuard,
) -> ApiResult {
    // The replica set as of now, not as of before the guard.
    let _caught_up = node.sys_fresh().await;
    let app = node.app(&app_id).await?;
    let g = node
        .hosted(&app_id)
        .ok_or_else(|| ApiError::bad_request("NOT_HOSTED", "send this to a current replica"))?;
    let current: std::collections::BTreeSet<u64> = app.replicas.iter().copied().collect();
    let st = g.status();
    let voters: std::collections::BTreeSet<u64> = st.voters.iter().copied().collect();
    if current == target && voters == target {
        return Ok(Json(json!({ "status": st, "unchanged": true })));
    }
    let union: Vec<u64> = current.union(&target).copied().collect();
    // Every node of the union starts the group before membership changes.
    let _ok = node
        .sys_write(&SystemCmd::SetReplicas {
            app_id: app_id.clone(),
            replicas: union,
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let addrs: Vec<(u64, String)> = node.sys(|s| {
        target
            .iter()
            .filter_map(|r| s.nodes.get(r).map(|n| (*r, n.addr.clone())))
            .collect()
    });
    // The new replicas get the bundle and blobs before they can vote.
    let joining: Vec<(u64, String)> = addrs
        .iter()
        .filter(|(id, _)| !current.contains(id))
        .cloned()
        .collect();
    let _handed = node.hand_over_bytes(&app, &g, &joining).await?;
    for (id, addr) in addrs {
        if !g.status().voters.contains(&id) {
            g.add_learner(id, addr)
                .await
                .map_err(|e| membership_err(&e))?;
        }
    }
    g.change_voters(target.clone())
        .await
        .map_err(|e| membership_err(&e))?;
    let _ok = node
        .sys_write(&SystemCmd::SetReplicas {
            app_id,
            replicas: target.into_iter().collect(),
        })
        .await?;
    Ok(Json(json!({ "status": g.status(), "unchanged": false })))
}

async fn ops_quotas_get(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        serde_json::to_value(node.sys(|s| s.quotas.clone())).unwrap_or_default(),
    ))
}

/// Change the limits the body names; the others stay as they are. A key that
/// names no limit is refused (a typo would otherwise change nothing silently).
async fn ops_quotas_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(changes): Json<serde_json::Map<String, Value>>,
) -> ApiResult {
    operator(&node, &headers)?;
    node.sys_fresh().await;
    let mut merged = serde_json::to_value(node.sys(|s| s.quotas.clone()))
        .map_err(|e| ApiError::internal(&e.to_string()))?;
    let limits = merged
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("quotas are not an object"))?;
    for (k, v) in changes {
        if !limits.contains_key(&k) {
            return Err(ApiError::bad_request(
                "UNKNOWN_QUOTA",
                &format!("no quota named {k}"),
            ));
        }
        let _prev = limits.insert(k, v);
    }
    let quotas: crate::system::Quotas = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?;
    let _ok = node.sys_write(&SystemCmd::SetQuotas { quotas }).await?;
    Ok(Json(json!({})))
}

/// Refused unless the deployment gave the operator this decision
/// (`--release-approvals`, `--app-upgrades`, `--outbound-approvals`).
fn operator_holds(
    node: &Node,
    which: fn(&crate::system::Governance) -> crate::system::Authority,
    what: &str,
) -> Result<(), ApiError> {
    if which(&node.sys(crate::system::SystemState::governance))
        == crate::system::Authority::Operator
    {
        Ok(())
    } else {
        Err(ApiError::forbidden(&format!(
            "this cluster was deployed with {what} decided by each app's owner"
        )))
    }
}

/// The owner's side of [`operator_holds`]: refused where the operator decides.
pub(crate) fn owner_holds(
    node: &Node,
    which: fn(&crate::system::Governance) -> crate::system::Authority,
    what: &str,
) -> Result<(), ApiError> {
    if which(&node.sys(crate::system::SystemState::governance)) == crate::system::Authority::Owner {
        Ok(())
    } else {
        Err(ApiError::forbidden(&format!(
            "this cluster was deployed with {what} decided by its operator"
        )))
    }
}

#[derive(Deserialize)]
struct OpsReleaseBody {
    version: String,
    #[serde(default = "yes")]
    approved: bool,
}

/// Approve (or withdraw) a mero-tee release for every app, on a cluster whose
/// operator manages upgrades.
async fn ops_release(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OpsReleaseBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.releases, "release approvals")?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetOperatorRelease {
            version: body.version,
            approved: body.approved,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpsDeadlineBody {
    /// RFC 3339: from then on, a scope that has not approved the release
    /// starts no new work.
    after: String,
}

/// Set the end-of-new-work deadline of a mero-tee release, in any
/// governance: it decides availability, which is the operator's already. It
/// moves no data and blocks no approval; a scope that approves the release
/// (or a newer one) is past it at once.
async fn ops_release_deadline(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(version): Path<String>,
    Json(body): Json<OpsDeadlineBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    let after = chrono::DateTime::parse_from_rfc3339(&body.after)
        .ok()
        .and_then(|t| u64::try_from(t.timestamp()).ok())
        .ok_or_else(|| {
            ApiError::bad_request("BAD_REQUEST", "`after` is an RFC 3339 time after 1970")
        })?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetReleaseDeadline {
            version,
            after: Some(after),
        })
        .await?,
    ))
}

/// Lift a release's deadline.
async fn ops_release_deadline_lift(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(version): Path<String>,
) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetReleaseDeadline {
            version,
            after: None,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublisherBody {
    signer_id: String,
    public_key: String,
    signature: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Bind a package signing key to the organization: the key's Ed25519
/// signature over `system::publisher_binding_message` (this cluster's name,
/// the organization, the key, the key's binding generation from
/// `GET /publishers/{signer}`), by a member holding `APPROVE_RELEASES`, with a
/// passkey assertion. The organization then approves the node images every
/// instance of the key's packages runs on, and upgrades those whose package
/// says `upgrades: publisher`. Binding again moves the key, with a new
/// signature from it.
async fn org_publisher(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<PublisherBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh().await;
    let (member, holds, generation) = node.sys(|s| {
        (
            s.orgs.get(&org).is_some_and(|o| o.role(&account).is_some()),
            s.holds_approve_releases(&org, &account),
            s.binding_generation(&body.signer_id),
        )
    });
    if !member {
        return Err(ApiError::not_found("no such organization"));
    }
    if !holds {
        return Err(ApiError::forbidden(
            "binding a publisher key needs APPROVE_RELEASES in the organization",
        ));
    }
    let cluster = node.config.cluster.clone();
    let proof = crate::system::PublisherProof {
        public_key: body.public_key,
        signature: body.signature,
    };
    let message =
        crate::system::publisher_binding_message(&cluster, &org, &body.signer_id, generation);
    if !proof.proves(&body.signer_id, &message) {
        return Err(ApiError::forbidden(
            "the proof is not that key's signature over this binding",
        ));
    }
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__bind_publisher",
        &json!({ "signerId": body.signer_id, "generation": generation }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::BindPublisher {
            signer_id: body.signer_id,
            org,
            cluster,
            proof,
            actor: account,
            consent,
        })
        .await?,
    ))
}

/// What a key signs to bind it here next: this cluster's name and the key's
/// binding generation, and whether it is bound now. For anyone signed in.
async fn publisher_binding(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(signer): Path<String>,
) -> ApiResult {
    let _p = signed_in(&node, &headers).await?;
    node.sys_fresh().await;
    Ok(Json(node.sys(|s| {
        json!({
            "signerId": signer,
            "cluster": node.config.cluster,
            "generation": s.binding_generation(&signer),
            "bound": s.publisher_of_signer(&signer).is_some(),
        })
    })))
}

/// The cluster's release rules, for anyone, signed in or not (inside the
/// sealed channel, so from an attested node): the governance, and each
/// release's end-of-new-work deadline. Users see the rules before they rely
/// on them.
async fn releases_public(State(node): State<Arc<Node>>) -> ApiResult {
    node.sys_fresh().await;
    Ok(Json(node.sys(|s| {
        json!({
            "governance": s.governance(),
            "deadlines": s.deadlines_json(),
        })
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpsOutboundBody {
    package: String,
    #[serde(default = "manifest")]
    prefixes: calimero_primitives::outbound::OutboundSelection,
    #[serde(default)]
    replace: bool,
}

/// Approve URL prefixes a package's publisher may call out to, for every app,
/// where the deployment gave the operator outbound approvals.
async fn ops_outbound(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OpsOutboundBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.outbound, "outbound approvals")?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetOutboundApproval {
            scope: crate::system::ReleaseScope::Cluster,
            package: body.package,
            selection: body.prefixes,
            replace: body.replace,
            actor: String::new(),
            consent: Value::Null,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpsUpgradeBody {
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    migrate: Option<String>,
}

/// Upgrade any app, on a cluster whose operator manages upgrades. The release
/// must be installed already (`/ops/packages`).
async fn ops_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<OpsUpgradeBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let current = node.release_of(&app.bundle);
    let bundle = match (body.bundle, body.version) {
        (Some(b), _) => b,
        (None, Some(version)) => {
            let package = body
                .package
                .or_else(|| current.as_ref().map(|r| r.at.package.clone()))
                .ok_or_else(|| {
                    ApiError::bad_request("BAD_REQUEST", "name the package to upgrade to")
                })?;
            let service = body
                .service
                .or_else(|| current.as_ref().and_then(|r| r.at.service.clone()))
                .unwrap_or_default();
            node.sys(|s| {
                s.packages
                    .get(&package)
                    .and_then(|p| p.versions.get(&version))
                    .filter(|v| upgrade_sees(s, v, &app, crate::system::OPERATOR, true))
                    .and_then(|v| v.services.get(&service).cloned())
            })
            .ok_or_else(|| {
                ApiError::new(
                    409,
                    "NOT_INSTALLED",
                    &format!("{package} {version} is not installed"),
                )
            })?
        }
        (None, None) => {
            return Err(ApiError::bad_request(
                "BAD_REQUEST",
                "name a version or a bundle",
            ))
        }
    };
    // A raw hash is the operator's own, or one the app's owner may name.
    if !node.sys(|s| {
        s.sees_bundle(&bundle, crate::system::OPERATOR) || s.sees_bundle(&bundle, &app.owner)
    }) {
        return Err(crate::node::rejected("unknown bundle"));
    }
    run_upgrade(
        &node,
        app_id,
        bundle,
        body.migrate,
        Upgrader {
            caller: Caller::Public,
            actor: String::new(),
            by_operator: true,
            consent: Value::Null,
            run: None,
        },
    )
    .await
}

async fn ops_backup(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    let n = node
        .backup()
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(json!({ "written": n })))
}

/// The node's router: every route the sealed envelope may reach.
pub fn router(node: Arc<Node>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/attestation", get(attestation))
        .route("/auth/passkey/register/begin", post(register_begin))
        .route("/auth/passkey/register/finish", post(register_finish))
        .route("/auth/passkey/login/begin", post(login_begin))
        .route("/auth/passkey/login/finish", post(login_finish))
        .route("/auth/passkeys/add/begin", post(passkey_add_begin))
        .route("/auth/passkeys/add/finish", post(passkey_add_finish))
        .route("/auth/passkeys/{cred}", post(passkey_label).delete(passkey_remove))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/logout-all", post(logout_all))
        .route("/auth/releases", post(account_release_approval))
        .route("/auth/outbound", post(account_outbound_approval))
        .route("/auth/me", get(me))
        .route("/auth/apps", get(account_apps))
        .route("/auth/usage", get(usage))
        .route("/auth/aliases", get(alias_list))
        .route("/auth/aliases/{alias}", get(alias_get).put(alias_set).delete(alias_delete))
        .route("/auth/apikeys", post(apikey_create).get(apikey_list))
        .route("/auth/apikeys/{key}", delete(apikey_revoke))
        .route("/bundles", post(bundle_put))
        .route("/packages", post(package_upload).get(package_list))
        .route("/packages/install", post(package_install))
        .route("/packages/{package}", get(package_get).delete(package_uninstall))
        .route("/packages/{package}/{version}", delete(package_version_uninstall))
        .route("/apps", post(app_create).get(app_list))
        .route("/apps/{app}", get(app_get).delete(app_delete))
        .route("/apps/{app}/metadata", post(app_metadata))
        .route("/apps/{app}/storage", get(app_storage))
        .route("/apps/{app}/abi", get(app_abi))
        .route("/apps/{app}/presence", get(presence_get).post(presence_publish))
        .route("/apps/{app}/presence/stream", get(presence_stream))
        .route("/apps/{app}/intents", post(intents))
        .route("/apps/{app}/attestation", get(app_attestation))
        .route("/apps/{app}/views", post(views))
        .route("/apps/{app}/events", get(events))
        .route("/apps/{app}/upgrade", post(app_upgrade))
        .route("/apps/{app}/access", post(app_access))
        .route("/apps/{app}/transfer", post(app_transfer))
        .route("/apps/{app}/blobs", post(blob_upload).get(blob_list))
        .route("/apps/{app}/blobs/{id}", get(blob_download).delete(blob_delete))
        .route("/apps/{app}/blobs/{id}/info", get(blob_info))
        .route("/orgs", post(org_create))
        .route("/orgs/{org}", get(org_get).delete(org_delete))
        .route("/orgs/{org}/metadata", post(org_metadata))
        .route("/orgs/{org}/upgrade", post(org_upgrade))
        .route("/orgs/{org}/upgrades/{run}", get(org_upgrade_status))
        .route("/orgs/{org}/upgrades/{run}/retry", post(org_upgrade_retry))
        .route("/orgs/{org}/members/{account}/metadata", post(member_metadata))
        .route("/orgs/{org}/members", post(org_set_member))
        .route("/orgs/{org}/members/{account}", delete(org_remove_member))
        .route("/orgs/{org}/teams", post(team_create))
        .route("/orgs/{org}/teams/{team}", get(team_get).post(team_update).delete(team_delete))
        .route("/orgs/{org}/capabilities", post(org_capabilities))
        .route("/orgs/{org}/releases", post(org_release_approval))
        .route("/orgs/{org}/publishers", post(org_publisher))
        .route("/publishers/{signer}", get(publisher_binding))
        .route(
            "/orgs/{org}/packages/{package}/upgrade",
            post(publisher_upgrade),
        )
        .route("/orgs/{org}/apps", get(org_apps))
        .route("/orgs/{org}/outbound", post(org_outbound_approval))
        .route("/orgs/{org}/teams/{team}/members", post(team_set_member))
        .route(
            "/orgs/{org}/teams/{team}/members/{account}",
            delete(team_remove_member),
        )
        .route("/orgs/{org}/invites", post(invite_create))
        .route("/invites/{token}/redeem", post(invite_redeem))
        .route("/consent/requests", post(crate::consent_page::create))
        .route("/consent/{request}/result", get(crate::consent_page::result))
        .route("/internal/cluster-key", post(internal_cluster_key))
        .route("/internal/raft", post(internal_raft))
        .route("/internal/system", post(internal_system))
        .route("/internal/system/index", post(internal_system_index))
        .route("/internal/call", post(internal_call))
        .route("/internal/blob", post(internal_blob_put))
        .route("/internal/challenge/take", post(internal_challenge_take))
        .route(
            "/internal/consent/result",
            post(crate::consent_page::internal_result),
        )
        .route("/internal/blob/get", post(internal_blob_get))
        .route("/internal/blob/held", post(internal_blob_held))
        .route("/internal/blob/delete", post(internal_blob_delete))
        .route("/internal/storage", post(internal_storage))
        .route("/internal/presence", post(internal_presence))
        .route("/internal/view", post(internal_view))
        .route("/internal/app-ready", post(internal_app_ready))
        .route("/internal/events", get(internal_events))
        .route("/ops/status", get(ops_status))
        .route("/ops/metrics", get(ops_metrics))
        .route("/ops/placement", get(ops_placement))
        .route("/ops/apps", get(ops_apps))
        .route("/ops/orgs", get(ops_orgs))
        .route("/ops/nodes", post(ops_register_node))
        .route("/ops/nodes/{id}", delete(ops_remove_node))
        .route("/ops/apps/{app}/replicas", post(ops_set_replicas))
        .route("/ops/backup", post(ops_backup))
        .route("/ops/releases", post(ops_release))
        .route(
            "/ops/releases/{version}/deadline",
            post(ops_release_deadline).delete(ops_release_deadline_lift),
        )
        .route("/releases", get(releases_public))
        .route("/ops/outbound", post(ops_outbound))
        .route("/ops/apps/{app}/upgrade", post(ops_upgrade))
        .route("/ops/orgs/{org}/upgrade", post(ops_org_upgrade))
        .route("/ops/packages", post(ops_package_upload).get(ops_package_list))
        .route("/ops/packages/install", post(ops_package_install))
        .route("/ops/packages/{package}", delete(ops_package_uninstall))
        .route(
            "/ops/packages/{package}/{version}",
            delete(ops_package_version_uninstall),
        )
        .route("/ops/quotas", get(ops_quotas_get).post(ops_quotas_set))
        // Counted, timed and sized per route template (`metrics::track`);
        // inside the sealed envelope, so it sees what each handler answered.
        .route_layer(axum::middleware::from_fn(crate::metrics::track))
        // Bundles (peer blob pushes, uploads) exceed axum's 2 MiB default; the sealed envelope caps requests at 64 MiB.
        .layer(axum::extract::DefaultBodyLimit::max(
            calimero_sealed::server::MAX_SEALED_BYTES,
        ))
        .with_state(node)
}

/// A new id people type as an argument (accounts, apps, organizations, teams,
/// upgrade runs, API keys): `N` random bytes in base64url that never start
/// with `-`, which a command line (meroctl) would read as a flag.
fn typed_id<const N: usize>() -> String {
    loop {
        let id = b64url(&random::<N>());
        if !id.starts_with('-') {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::typed_id;

    #[test]
    fn a_typed_id_never_starts_with_a_dash() {
        for _ in 0..10_000 {
            let id = typed_id::<16>();
            assert!(!id.starts_with('-'), "{id}");
            assert_eq!(id.len(), 22);
        }
    }
}
