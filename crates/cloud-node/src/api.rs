//! The node's HTTP API. Clients reach it inside the sealed channel; only
//! `/attestation` and `/health` are served in the clear.
//!
//! - Client: `/auth/*`, `/bundles`, `/apps`, `/apps/{app}/{intents,views,events,upgrade,access}`,
//!   `/orgs/*`, `/invites/{token}/redeem`.
//! - Internal (peers, cluster secret): `/internal/*`.
//! - Ops (mdma, operator token): `/ops/*`.

use std::convert::Infallible;
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
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::{Applied, Status};
use crate::attest::{quote, report_data, AttestationDoc, Release};
use crate::error::ApiError;
use crate::node::{CallRequest, Node, ViewRequest};
use crate::system::SystemCmd;
use crate::types::{
    now_secs, Access, Account, ApiKeyRecord, Caller, Invite, OrgRole, RefreshRecord, TeamJoin,
};

const SESSION_TTL: u64 = 15 * 60;
const REFRESH_TTL: u64 = 30 * 24 * 60 * 60;
const CHALLENGE_TTL: u64 = 5 * 60;

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

/// A ceremony this node started and will finish.
#[derive(Clone, Debug)]
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
    let id = b64url(&random::<16>());
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

fn take_pending(node: &Node, id: &str) -> Result<Pending, ApiError> {
    let p = node
        .challenges
        .lock()
        .ok()
        .and_then(|mut c| c.remove(id))
        .ok_or_else(|| {
            ApiError::bad_request(
                "UNKNOWN_CHALLENGE",
                "unknown or used challenge; start again on this node",
            )
        })?;
    if p.expires() <= now_secs() {
        return Err(ApiError::bad_request(
            "EXPIRED_CHALLENGE",
            "the challenge expired",
        ));
    }
    Ok(p)
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
fn rp_step_up(node: &Node) -> RelyingParty {
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
}

impl Principal {
    fn account(&self) -> Result<String, ApiError> {
        self.caller
            .account()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::unauthenticated("sign in first"))
    }
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
    }))
}

async fn signed_in(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
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
async fn person(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let p = signed_in(node, headers).await?;
    if p.session.is_none() {
        return Err(ApiError::forbidden(
            "this needs a passkey session, not an API key",
        ));
    }
    Ok(p)
}

async fn new_session(node: &Node, account: &str, passkey: &str) -> ApiResult {
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
    // The rules the cluster was deployed with, as this replica's state holds
    // them: bound with the receipt key, so a user proves both at once.
    let governance = node.sys(crate::system::SystemState::governance);
    let report = if receipts {
        let mut r = [0; 64];
        r[..32].copy_from_slice(&nonce);
        r[32..].copy_from_slice(&calimero_cloud_client::attest::receipt_key_binding(
            &node.receipt_public(),
            &governance,
        ));
        r
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
        release: Release {
            version: node
                .config
                .tee_release
                .clone()
                .unwrap_or_else(|| node.config.release.clone()),
            profile: match (&node.config.tee_profile, node.config.attestation) {
                (_, crate::attest::Mode::Mock) => "mock".into(),
                (Some(p), _) => p.clone(),
                (None, _) => "tdx".into(),
            },
        },
        cluster: node.config.cluster.clone(),
        node: node.config.node_id,
        mock: node.config.attestation == crate::attest::Mode::Mock,
        receipt_key: receipts.then(|| hex::encode(node.receipt_public())),
        governance: receipts.then_some(governance),
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
    } = take_pending(&node, &body.challenge_id)?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a registration challenge",
        ));
    };
    let cred = verify_registration(&rp(&node), &challenge, &body.registration)
        .map_err(|e| ApiError::bad_request("BAD_REGISTRATION", &e.to_string()))?;
    let account = b64url(&random::<32>());
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
    new_session(&node, &account, &cred.id).await
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
async fn check_assertion(
    node: &Node,
    challenge: &[u8; 32],
    assertion: &Assertion,
    account: Option<&str>,
    step_up: bool,
) -> Result<String, ApiError> {
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
    let rp = if step_up { rp_step_up(node) } else { rp(node) };
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
    Ok(owner)
}

async fn login_finish(State(node): State<Arc<Node>>, Json(body): Json<AssertionBody>) -> ApiResult {
    let Pending::Login { challenge, .. } = take_pending(&node, &body.challenge_id)? else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a login challenge",
        ));
    };
    let account = check_assertion(&node, &challenge, &body.assertion, None, false).await?;
    new_session(&node, &account, &body.assertion.credential_id).await
}

async fn passkey_add_begin(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = person(&node, &headers).await?;
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
    let p = person(&node, &headers).await?;
    let Pending::AddPasskey {
        challenge, account, ..
    } = take_pending(&node, &body.challenge_id)?
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
    let account = person(&node, &headers).await?.account()?;
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
    let Some(rec) = node.sys_until(|s| s.refresh.get(&old_hash).cloned()).await else {
        // A token already exchanged, presented again: someone else holds a
        // copy, so the session it belongs to ends here.
        if node.sys(|s| s.rotated_refresh.contains_key(&old_hash)) {
            let _ok = node
                .sys_write(&SystemCmd::RefreshReused {
                    hash: old_hash,
                    now: now_secs(),
                })
                .await;
            return Err(ApiError::unauthenticated(
                "refresh token already used; the session is revoked",
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
    };
    let _ok = node
        .sys_write(&SystemCmd::UseRefresh {
            old_hash,
            new_hash: b64url(&sha256(new_refresh.as_bytes())),
            record,
            now: now_secs(),
        })
        .await
        .map_err(|e| ApiError::unauthenticated(&e.message))?;
    let expires_at = now_secs() + SESSION_TTL;
    let token = mint_session(
        &key,
        &SessionClaims {
            account: rec.account.clone(),
            session: rec.session,
            expires_at,
            key_version: version,
            passkey: rec.passkey,
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
struct ReleaseApprovalBody {
    /// The mero-tee release, as attestations name it.
    version: String,
    /// `false` withdraws an approval.
    #[serde(default = "yes")]
    approved: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

const fn yes() -> bool {
    true
}

/// Approve (or withdraw) a mero-tee release for the apps of `scope`, with a
/// passkey assertion over the exact change. Until the owner approves a
/// release, nodes on it get none of those apps' data.
async fn set_release_approval(
    node: &Node,
    headers: &HeaderMap,
    scope: crate::system::ReleaseScope,
    body: ReleaseApprovalBody,
) -> ApiResult {
    let p = person(node, headers).await?;
    let account = p.account()?;
    let subject = match &scope {
        crate::system::ReleaseScope::Org { org } => format!("org:{org}"),
        crate::system::ReleaseScope::Account => format!("account:{account}"),
        crate::system::ReleaseScope::Cluster => "cluster".to_owned(),
    };
    owner_holds(node, |g| g.releases, "release approvals")?;
    let method = if body.approved {
        "__approve_release"
    } else {
        "__withdraw_release"
    };
    let consent = consent(
        node,
        &p,
        &subject,
        method,
        &json!({ "version": body.version }),
        body.confirm,
    )
    .await?;
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
    set_release_approval(
        &node,
        &headers,
        crate::system::ReleaseScope::Org { org },
        body,
    )
    .await
}

async fn account_release_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<ReleaseApprovalBody>,
) -> ApiResult {
    set_release_approval(&node, &headers, crate::system::ReleaseScope::Account, body).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutboundApprovalBody {
    /// The package whose publisher (its pinned signer) the approval is for.
    package: String,
    /// `"manifest"` (the default: every prefix the latest release declares)
    /// or the prefixes to approve.
    #[serde(default = "manifest")]
    prefixes: calimero_primitives::outbound::OutboundSelection,
    /// Set exactly these (how a prefix is revoked) instead of adding them.
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

const fn manifest() -> calimero_primitives::outbound::OutboundSelection {
    calimero_primitives::outbound::OutboundSelection::Manifest
}

/// Approve URL prefixes a package's publisher may call out to from the apps
/// of `scope`, with a passkey assertion over the exact change. The owner
/// decides, never the operator: the operator only bounds the hosts.
async fn set_outbound_approval(
    node: &Node,
    headers: &HeaderMap,
    scope: crate::system::ReleaseScope,
    body: OutboundApprovalBody,
) -> ApiResult {
    let p = person(node, headers).await?;
    let account = p.account()?;
    let subject = match &scope {
        crate::system::ReleaseScope::Org { org } => format!("org:{org}"),
        crate::system::ReleaseScope::Account => format!("account:{account}"),
        crate::system::ReleaseScope::Cluster => "cluster".to_owned(),
    };
    owner_holds(node, |g| g.outbound, "outbound approvals")?;
    let consent = consent(
        node,
        &p,
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
    set_outbound_approval(
        &node,
        &headers,
        crate::system::ReleaseScope::Org { org },
        body,
    )
    .await
}

async fn account_outbound_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OutboundApprovalBody>,
) -> ApiResult {
    set_outbound_approval(&node, &headers, crate::system::ReleaseScope::Account, body).await
}

/// Sign out everywhere: revoke every session of the caller's account.
async fn logout_all(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = person(&node, &headers).await?;
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
    let account = person(&node, &headers).await?.account()?;
    let (key, hash) = calimero_identity::new_api_key();
    let key_id = b64url(&random::<8>());
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
    let account = person(&node, &headers).await?.account()?;
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
    let account = person(&node, &headers).await?.account()?;
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
    // Refuse what does not compile before replicating it.
    let _module = node
        .executor
        .module(&hash, &body)
        .map_err(|e| ApiError::bad_request("BAD_BUNDLE", &e.to_string()))?;
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
    Ok(Json(
        node.install_upload(Arc::from(body.to_vec()), &account)
            .await?,
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

fn package_json(p: &crate::system::PackageRecord) -> Value {
    let mut versions: Vec<&crate::system::PackageVersion> = p.versions.values().collect();
    versions.sort_by(|a, b| crate::system::compare_versions(&a.version, &b.version));
    json!({
        "package": p.name,
        "signerId": p.signer_id,
        "owner": p.owner,
        "latest": p.latest().map(|v| v.version.clone()),
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
    })
}

async fn package_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let _p = signed_in(&node, &headers).await?;
    let packages: Vec<Value> = node.sys(|s| s.packages.values().map(package_json).collect());
    Ok(Json(json!({ "packages": packages })))
}

async fn package_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    let _p = signed_in(&node, &headers).await?;
    node.sys(|s| s.packages.get(&package).map(package_json))
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such package"))
}

async fn package_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    let account = person(&node, &headers).await?.account()?;
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
    let account = person(&node, &headers).await?.account()?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: Some(version),
            actor: account,
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
) -> Result<String, ApiError> {
    match (bundle, package) {
        (Some(b), None) => Ok(b),
        (None, Some(p)) => node.resolve_release(p, version, service, actor).await,
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
    /// The instance's metadata (v1's context metadata).
    #[serde(default)]
    metadata: Value,
}

async fn app_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<CreateAppBody>,
) -> ApiResult {
    let p = installer(&node, &headers).await?;
    if p.api_key.as_ref().is_some_and(|k| k.read_only) {
        return Err(ApiError::forbidden("read-only API key"));
    }
    let owner = p.account()?;
    let bundle = bundle_named(
        &node,
        body.bundle,
        body.package.as_deref(),
        body.version.as_deref(),
        body.service.as_deref(),
        &owner,
    )
    .await?;
    // Only nodes on a release the owner approved may hold the app.
    let approved = node.sys(|s| {
        match &body.org {
            Some(o) => s.orgs.get(o).map(|o| o.approved_releases.clone()),
            None => s.accounts.get(&owner).map(|a| a.approved_releases.clone()),
        }
        .filter(|set| !set.is_empty())
    });
    let replicas = node.place(approved.as_ref()).await;
    if replicas.is_empty() {
        return Err(ApiError::unavailable(
            "no node runs a release this app's owner approved",
        ));
    }
    let app_id = b64url(&random::<16>());
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
                confirm_methods: body.confirm_methods,
                replicas,
                created_at: now_secs(),
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
    let apps: Vec<Value> = node.sys(|s| {
        s.apps
            .values()
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
    let account = person(&node, &headers).await?.account()?;
    let _app = node.app(&app_id).await?;
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
    let account = p.as_ref().and_then(|p| p.caller.account());
    if !node.sys(|s| s.may_reach(&app, account)) {
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

async fn gate(
    node: &Node,
    headers: &HeaderMap,
    app: &crate::types::AppRecord,
    method: &str,
    write: bool,
) -> Result<Principal, ApiError> {
    let p = principal(node, headers).await?;
    let account = p.as_ref().and_then(|p| p.caller.account());
    // Access is judged on the cluster's latest system state (a read index): a
    // removal or revocation committed through another node holds before the
    // next request, read or write, and a grant is seen at once (Xil's review:
    // access judged on lagging local state). `principal` already read fresh
    // for a request that carries a credential.
    if p.is_none() {
        node.sys_fresh().await;
    }
    let reaches = node.sys(|s| s.may_reach(app, account));
    if !reaches {
        return Err(if p.is_none() {
            ApiError::unauthenticated("sign in to use this app")
        } else {
            ApiError::not_found("no such app")
        });
    }
    // A read-only row refuses what writes: a blob upload here, a call that
    // would write at commit (`Node::call_local`), as v1 discarded its writes.
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
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmBody {
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
async fn consent(
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
    } = take_pending(node, &confirm.challenge_id)?
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
    let _owner =
        check_assertion(node, &challenge, &confirm.assertion, Some(&account), true).await?;
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

async fn intents(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<IntentBody>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, &body.method, true).await?;
    // A declared view runs as one, whichever way it is invoked (v1's rule): it
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

fn local_events(
    node: &Node,
    app_id: &str,
    from: u64,
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
    let stream = futures_util::stream::iter(backlog).chain(live).map(|e| {
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
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let slot = StreamSlot::take(&node)?;
    if let Some(stream) = local_events(&node, &app_id, q.from) {
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
            &format!("internal/events?app={app_id}&from={}", q.from),
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
#[serde(rename_all = "camelCase")]
struct UpgradeBody {
    /// A bundle hash, or a `version` of the app's package (or another `package`).
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    service: Option<String>,
    /// The owner's passkey approval of this exact upgrade (412 asks for it).
    #[serde(default)]
    confirm: Option<ConfirmBody>,
    /// The bundle's migration export, if the state shape changed.
    #[serde(default)]
    migrate: Option<String>,
}

async fn app_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(mut body): Json<UpgradeBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    // Who may upgrade is the system log's rule (`UpgradeApp`); asked first, so
    // a refused or failed upgrade records nothing. An upgrade is the app's
    // owner's decision (the organization's owner for an org app), never an
    // operator's or an admin's: new code reads the app's data.
    let app = node.app(&app_id).await?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if !node.sys(|s| s.may_upgrade(&app, &account)) {
        return Err(ApiError::forbidden(
            "only the app's owner (or its organization's owner) may upgrade it",
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
        &node,
        body.bundle.clone(),
        package.as_deref(),
        body.version.as_deref(),
        service.as_deref(),
        &account,
    )
    .await?;
    if !node.sys(|s| s.bundles.contains_key(&bundle)) {
        return Err(crate::node::rejected("unknown bundle"));
    }
    // A fresh passkey assertion over this exact upgrade, logged as consent.
    let consent = consent(
        &node,
        &p,
        &app_id,
        "__upgrade",
        &json!({ "bundle": bundle, "migrate": body.migrate }),
        body.confirm.take(),
    )
    .await?;
    run_upgrade(
        &node,
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
            "operator".to_owned()
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
    let applied = match node
        .call(&CallRequest::new(
            app_id.clone(),
            "__upgrade".into(),
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
        return Err(
            ApiError::new(422, "UPGRADE_FAILED", "the upgrade did not run").with_details(
                json!({ "index": applied.index, "status": applied.status, "error": applied.error }),
            ),
        );
    }
    // The app now runs the new bundle: the directory says so too.
    let _ok = node
        .sys_write(&SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator: who.by_operator,
            actor: who.actor,
            at: now_secs(),
            run: who.run,
            consent: who.consent,
        })
        .await?;
    Ok(Json(
        json!({ "index": applied.index, "status": applied.status, "error": applied.error }),
    ))
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

/// The bundle `version` of the package gives an app (same service), if installed.
fn target_bundle(node: &Node, app: &crate::types::AppRecord, version: &str) -> Option<String> {
    let r = node.release_of(&app.bundle)?;
    let service = r.at.service.clone().unwrap_or_default();
    node.sys(|s| {
        s.packages
            .get(&r.at.package)?
            .versions
            .get(version)?
            .services
            .get(&service)
            .cloned()
    })
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
    let run_id = b64url(&random::<12>());
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
            match target_bundle(&node, &app, &version) {
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
/// subtree) that runs `package` to `version`: the org owner's, with one passkey
/// assertion over the whole run, where owners hold app upgrades.
async fn org_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(mut body): Json<OrgUpgradeBody>,
) -> ApiResult {
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&account))) != Some(OrgRole::Owner) {
        return Err(ApiError::forbidden(
            "only the organization's owner upgrades all its apps",
        ));
    }
    // Installed from the registry first if the cluster lacks the release.
    let _bundle = node
        .resolve_release(&body.package, Some(&body.version), None, &account)
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

/// An organization-wide upgrade's progress (v1's upgrade and cascade status).
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
/// same terms (v1's retry), as a new run.
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
    if prev.by == "operator" {
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
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&account))) != Some(OrgRole::Owner) {
        return Err(ApiError::forbidden(
            "only the organization's owner upgrades all its apps",
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

/// Name an app for this account (v1's aliases).
async fn alias_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
    Json(body): Json<AliasBody>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let app = node.canonical_app_id(&body.app);
    let rec = node.app(&app).await?;
    if !node.sys(|s| s.may_reach(&rec, Some(&account))) {
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
    let account = person(&node, &headers).await?.account()?;
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
    let org_id = b64url(&random::<16>());
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
async fn org_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Option<Json<DeleteOrgBody>>,
) -> ApiResult {
    let p = person(&node, &headers).await?;
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
    let actor = signed_in(&node, &headers).await?.account()?;
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

/// Name one of the caller's passkeys (v1's device label).
async fn passkey_label(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(cred): Path<String>,
    Json(body): Json<PasskeyLabelBody>,
) -> ApiResult {
    let account = person(&node, &headers).await?.account()?;
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
            team_id: b64url(&random::<16>()),
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

fn peer_only(node: &Node, headers: &HeaderMap) -> Result<(), ApiError> {
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

/// A blob id as a client gives it: base58 (what apps print, v1's `BlobId`) or hex.
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
    if p.caller.account().is_none() {
        return Err(ApiError::unauthenticated("sign in to upload"));
    }
    let id = node.store_blob(&body).await?;
    let size = body.len() as u64;
    let a = node
        .call(&CallRequest::new(
            app_id,
            "__blob".into(),
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
    let raw = parse_blob_id(&id)?;
    if !node.blob_held(&app, &raw).await? {
        return Err(ApiError::not_found("no such blob in this app"));
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

/// The app's blobs (v1's blob list).
async fn blob_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "__blob", false).await?;
    let stats = node.storage_stats(&app).await?;
    Ok(Json(json!({
        "blobs": stats.blobs.iter().map(|(id, size)| blob_json(id, *size)).collect::<Vec<_>>(),
    })))
}

/// One blob's size (v1's blob info); 404 if the app has no such blob.
async fn blob_info(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "__blob", false).await?;
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
        .call(&CallRequest::new(
            app_id,
            "__blob_forget".into(),
            json!({ "id": hex::encode(raw) }),
            p.caller,
        ))
        .await?;
    node.collect_blob(&raw).await;
    Ok(Json(json!({ "index": a.index })))
}

/// What the app's storage holds (v1's context storage): keys, bytes, blobs.
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

/// The ABI the app's bundle embeds (v1's application ABI); `null` for none.
async fn app_abi(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let wasm = node.bundle_wasm(&app.bundle).await?;
    let abi = calimero_wasm_abi::embed::read_embedded_state_schema(&wasm);
    Ok(Json(json!({ "bundle": app.bundle, "abi": abi })))
}

/// What the caller's account uses, against the cluster's quotas (v1's usage).
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
    let a = node.call_here(&req).await?;
    Ok(Json(serde_json::to_value(a).unwrap_or_default()))
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
}

async fn internal_events(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<InternalEvents>,
) -> Result<Response, ApiError> {
    peer_only(&node, &headers)?;
    let stream = local_events(&node, &q.app, q.from)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

// ---------------------------------------------------------------- ops (mdma)

fn operator(node: &Node, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(expected) = node.config.ops_token.as_deref() else {
        return Err(ApiError::forbidden("the Ops API is disabled on this node"));
    };
    let given = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if ct_eq(given.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(ApiError::forbidden("operator credential required"))
    }
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
    // Counts only: the Ops API never returns app state, names or credentials.
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
async fn ops_set_replicas(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(b): Json<ReplicaBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    operator(&node, &headers)?;
    if b.replicas.len() < 3.min(node.sys(|s| s.nodes.len())) {
        return Err(ApiError::bad_request(
            "TOO_FEW",
            "an app keeps at least 3 replicas",
        ));
    }
    let app = node.app(&app_id).await?;
    // The operator moves an app only onto nodes on a release its owner
    // approved: a move is how data would reach a new image.
    for id in &b.replicas {
        let release = node.release_of_node(*id).await;
        if !node.sys(|s| s.release_may_hold(&app, release.as_deref())) {
            return Err(ApiError::forbidden(&format!(
                "node {id} runs release {}, which this app's owner has not approved",
                release.as_deref().unwrap_or("(not attested yet)")
            )));
        }
    }
    // Only the leader changes membership; others redirect before writing anything.
    if let Some(st) = node.hosted(&app_id).map(|g| g.status()) {
        if let (false, Some(id)) = (st.is_leader, st.leader) {
            let addr = node.sys(|s| s.nodes.get(&id).map(|n| n.addr.clone()));
            return Err(membership_err(&calimero_log::WriteError::NotLeader {
                leader: addr.map(|a| (id, a)),
            }));
        }
    }
    let union: Vec<u64> = app
        .replicas
        .iter()
        .chain(b.replicas.iter())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    // Every node of the union starts the group before membership changes.
    let _ok = node
        .sys_write(&SystemCmd::SetReplicas {
            app_id: app_id.clone(),
            replicas: union,
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let g = node
        .hosted(&app_id)
        .ok_or_else(|| ApiError::bad_request("NOT_HOSTED", "send this to a current replica"))?;
    let addrs: Vec<(u64, String)> = node.sys(|s| {
        b.replicas
            .iter()
            .filter_map(|r| s.nodes.get(r).map(|n| (*r, n.addr.clone())))
            .collect()
    });
    for (id, addr) in addrs {
        if !g.status().voters.contains(&id) {
            g.add_learner(id, addr)
                .await
                .map_err(|e| membership_err(&e))?;
        }
    }
    g.change_voters(b.replicas.iter().copied().collect())
        .await
        .map_err(|e| membership_err(&e))?;
    let _ok = node
        .sys_write(&SystemCmd::SetReplicas {
            app_id,
            replicas: b.replicas,
        })
        .await?;
    Ok(Json(json!({ "status": g.status() })))
}

async fn ops_quotas_get(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        serde_json::to_value(node.sys(|s| s.quotas.clone())).unwrap_or_default(),
    ))
}

async fn ops_quotas_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(quotas): Json<crate::system::Quotas>,
) -> ApiResult {
    operator(&node, &headers)?;
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
fn owner_holds(
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
/// must be installed already (the operator holds no account to install as).
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
    if !node.sys(|s| s.bundles.contains_key(&bundle)) {
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
        .route("/apps/{app}/intents", post(intents))
        .route("/apps/{app}/attestation", get(app_attestation))
        .route("/apps/{app}/views", post(views))
        .route("/apps/{app}/events", get(events))
        .route("/apps/{app}/upgrade", post(app_upgrade))
        .route("/apps/{app}/access", post(app_access))
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
        .route("/orgs/{org}/outbound", post(org_outbound_approval))
        .route("/orgs/{org}/teams/{team}/members", post(team_set_member))
        .route(
            "/orgs/{org}/teams/{team}/members/{account}",
            delete(team_remove_member),
        )
        .route("/orgs/{org}/invites", post(invite_create))
        .route("/invites/{token}/redeem", post(invite_redeem))
        .route("/internal/raft", post(internal_raft))
        .route("/internal/system", post(internal_system))
        .route("/internal/system/index", post(internal_system_index))
        .route("/internal/call", post(internal_call))
        .route("/internal/blob", post(internal_blob_put))
        .route("/internal/blob/get", post(internal_blob_get))
        .route("/internal/blob/held", post(internal_blob_held))
        .route("/internal/blob/delete", post(internal_blob_delete))
        .route("/internal/storage", post(internal_storage))
        .route("/internal/view", post(internal_view))
        .route("/internal/app-ready", post(internal_app_ready))
        .route("/internal/events", get(internal_events))
        .route("/ops/status", get(ops_status))
        .route("/ops/nodes", post(ops_register_node))
        .route("/ops/apps/{app}/replicas", post(ops_set_replicas))
        .route("/ops/backup", post(ops_backup))
        .route("/ops/releases", post(ops_release))
        .route("/ops/outbound", post(ops_outbound))
        .route("/ops/apps/{app}/upgrade", post(ops_upgrade))
        .route("/ops/orgs/{org}/upgrade", post(ops_org_upgrade))
        .route("/ops/quotas", get(ops_quotas_get).post(ops_quotas_set))
        // Bundles (peer blob pushes, uploads) exceed axum's 2 MiB default; the sealed envelope caps requests at 64 MiB.
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(node)
}
