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
    now_secs, Access, Account, ApiKeyRecord, Caller, EventRecord, Invite, OrgRole, RefreshRecord,
    TeamJoin,
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

fn put_pending(node: &Node, pending: Pending) -> String {
    let id = b64url(&random::<16>());
    if let Ok(mut c) = node.challenges.lock() {
        let now = now_secs();
        c.retain(|_, p| p.expires() > now);
        let _prev = c.insert(id.clone(), pending);
    }
    id
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

async fn new_session(node: &Node, account: &str) -> ApiResult {
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
    let report = report_data(&nonce, &app_hash, &transport_key);
    let q =
        quote(node.config.attestation, report).map_err(|e| ApiError::internal(&e.to_string()))?;
    let doc = AttestationDoc {
        quote: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, q),
        transport_key: hex::encode(transport_key),
        app_hash: hex::encode(app_hash),
        nonce: hex::encode(nonce),
        release: Release {
            version: node.config.release.clone(),
            profile: if node.config.attestation == crate::attest::Mode::Mock {
                "mock".into()
            } else {
                "tdx".into()
            },
        },
        cluster: node.config.cluster.clone(),
        node: node.config.node_id,
        mock: node.config.attestation == crate::attest::Mode::Mock,
    };
    Ok(Json(serde_json::to_value(doc).unwrap_or_default()))
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
    );
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
                passkeys: vec![cred],
            },
        })
        .await?;
    new_session(&node, &account).await
}

async fn login_begin(State(node): State<Arc<Node>>) -> ApiResult {
    let challenge = random::<32>();
    let id = put_pending(
        &node,
        Pending::Login {
            challenge,
            expires: now_secs() + CHALLENGE_TTL,
        },
    );
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
    let count = verify_assertion(&rp(node), challenge, &cred, assertion)
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
    let account = check_assertion(&node, &challenge, &body.assertion, None).await?;
    new_session(&node, &account).await
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
    );
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
    let rec = node
        .sys_until(|s| s.refresh.get(&old_hash).cloned())
        .await
        .ok_or_else(|| ApiError::unauthenticated("unknown refresh token"))?;
    let (version, key) = node
        .sys(|s| s.current_session_key())
        .ok_or_else(|| ApiError::unavailable("no session key"))?;
    let new_refresh = format!("rt_{}", b64url(&random::<32>()));
    let record = RefreshRecord {
        account: rec.account.clone(),
        session: rec.session.clone(),
        expires_at: now_secs() + REFRESH_TTL,
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
        })
        .await?;
    Ok(Json(json!({})))
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
        "passkeys": acc.passkeys.iter().map(|c| json!({ "id": c.id, "signCount": c.sign_count })).collect::<Vec<_>>(),
        "orgs": orgs,
        "via": if p.api_key.is_some() { "apiKey" } else { "session" },
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
    let account = signed_in(&node, &headers).await?.account()?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppBody {
    #[serde(default)]
    name: String,
    bundle: String,
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    access: Option<Access>,
    #[serde(default)]
    confirm_methods: Vec<String>,
    /// Arguments for the app's `init`, as JSON.
    #[serde(default)]
    init_args: Value,
}

async fn app_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<CreateAppBody>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    if p.api_key.as_ref().is_some_and(|k| k.read_only) {
        return Err(ApiError::forbidden("read-only API key"));
    }
    let owner = p.account()?;
    let replicas = node.place();
    if replicas.is_empty() {
        return Err(ApiError::unavailable("no nodes registered yet"));
    }
    let app_id = b64url(&random::<16>());
    let access = body.access.unwrap_or(if body.org.is_some() {
        Access::Org
    } else {
        Access::Owner
    });
    let out = node
        .sys_write(&SystemCmd::CreateApp {
            app: crate::types::AppRecord {
                app_id: app_id.clone(),
                name: body.name,
                group: 0,
                bundle: body.bundle,
                owner,
                org: body.org,
                access,
                confirm_methods: body.confirm_methods,
                replicas,
                created_at: now_secs(),
                init_args: body.init_args,
            },
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
    Ok(Json(
        json!({ "appId": app_id, "group": out["group"], "replicas": out["replicas"] }),
    ))
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
        .peer_post(&addr, "internal/app-ready", &json!({ "appId": app_id }))
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

async fn app_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = principal(&node, &headers).await?;
    let account = p
        .as_ref()
        .and_then(|p| p.caller.account().map(str::to_owned));
    let apps: Vec<Value> = node.sys(|s| {
        s.apps
            .values()
            .filter(|a| a.access != Access::Public || account.as_deref() == Some(a.owner.as_str()))
            .filter(|a| s.may_reach(a, account.as_deref()))
            .map(|a| json!({ "appId": a.app_id, "name": a.name, "owner": a.owner, "org": a.org, "bundle": a.bundle }))
            .collect()
    });
    Ok(Json(json!({ "apps": apps })))
}

async fn app_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let p = principal(&node, &headers).await?;
    let app = node.app(&app_id).await?;
    let account = p.as_ref().and_then(|p| p.caller.account());
    if !node.sys(|s| s.may_reach(&app, account)) {
        return Err(ApiError::not_found("no such app"));
    }
    Ok(Json(serde_json::to_value(app).unwrap_or_default()))
}

/// The caller, if they may reach `app` with this request kind.
async fn gate(
    node: &Node,
    headers: &HeaderMap,
    app: &crate::types::AppRecord,
    method: &str,
    write: bool,
) -> Result<Principal, ApiError> {
    let p = principal(node, headers).await?;
    let account = p.as_ref().and_then(|p| p.caller.account());
    if !node.sys(|s| s.may_reach(app, account)) {
        return Err(if p.is_none() {
            ApiError::unauthenticated("sign in to use this app")
        } else {
            ApiError::not_found("no such app")
        });
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
        );
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
    let _owner = check_assertion(node, &challenge, &confirm.assertion, Some(&account)).await?;
    Ok(serde_json::to_value(&confirm.assertion).unwrap_or_default())
}

fn answer(a: Applied) -> Response {
    if a.conflict {
        return ApiError::unavailable(
            "the call raced a leader change; retry with the same idempotency key",
        )
        .into_response();
    }
    match a.status {
        Status::Ok => Json(json!({ "index": a.index, "result": a.result, "events": a.events.iter().map(|(k, d)| json!({ "kind": k, "data": d })).collect::<Vec<_>>() })).into_response(),
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
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, &body.method, true).await?;
    let consent_value = if app.confirm_methods.iter().any(|m| m == &body.method) {
        Some(consent(&node, &p, &app_id, &body.method, &body.args, body.confirm).await?)
    } else {
        None
    };
    let applied = node
        .call(&CallRequest {
            app_id,
            method: body.method,
            args: body.args,
            caller: p.caller,
            idempotency_key: body.idempotency_key,
            consent: consent_value,
        })
        .await?;
    Ok(answer(applied))
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
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, &body.method, false).await?;
    let a = node
        .view(&ViewRequest {
            app_id,
            method: body.method,
            args: body.args,
            caller: p.caller,
            strong: body.consistency.as_deref() != Some("stale"),
            min_index: body.min_index,
        })
        .await?;
    Ok(match a.status {
        Status::Ok => Json(json!({ "index": a.index, "result": a.result })).into_response(),
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
        let backlog: Vec<EventRecord> = m
            .state
            .events
            .iter()
            .filter(|e| e.index >= from)
            .cloned()
            .collect();
        (backlog, rx)
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

async fn events(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<Response, ApiError> {
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    if let Some(stream) = local_events(&node, &app_id, q.from) {
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
    let body = axum::body::Body::from_stream(resp.bytes_stream());
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
    bundle: String,
    /// The bundle's migration export, if the state shape changed.
    #[serde(default)]
    migrate: Option<String>,
}

async fn app_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<UpgradeBody>,
) -> ApiResult {
    let account = person(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::UpgradeApp {
            app_id: app_id.clone(),
            bundle: body.bundle.clone(),
            actor: account.clone(),
        })
        .await?;
    let applied = node
        .call(&CallRequest {
            app_id,
            method: "__upgrade".into(),
            args: json!({ "bundle": body.bundle, "migrate": body.migrate }),
            caller: Caller::Account { account },
            idempotency_key: None,
            consent: None,
        })
        .await?;
    Ok(Json(
        json!({ "index": applied.index, "status": applied.status, "error": applied.error }),
    ))
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
    let o = node
        .sys(|s| s.orgs.get(&org).cloned())
        .ok_or_else(|| ApiError::not_found("no such organization"))?;
    if o.role(&account).is_none() {
        return Err(ApiError::not_found("no such organization"));
    }
    Ok(Json(serde_json::to_value(o).unwrap_or_default()))
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
            actor,
        })
        .await?;
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
    operator(&node, &headers)?;
    if b.replicas.len() < 3.min(node.sys(|s| s.nodes.len())) {
        return Err(ApiError::bad_request(
            "TOO_FEW",
            "an app keeps at least 3 replicas",
        ));
    }
    let app = node.app(&app_id).await?;
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
        .route("/auth/passkeys/{cred}", delete(passkey_remove))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/apikeys", post(apikey_create).get(apikey_list))
        .route("/auth/apikeys/{key}", delete(apikey_revoke))
        .route("/bundles", post(bundle_put))
        .route("/apps", post(app_create).get(app_list))
        .route("/apps/{app}", get(app_get))
        .route("/apps/{app}/intents", post(intents))
        .route("/apps/{app}/views", post(views))
        .route("/apps/{app}/events", get(events))
        .route("/apps/{app}/upgrade", post(app_upgrade))
        .route("/apps/{app}/access", post(app_access))
        .route("/orgs", post(org_create))
        .route("/orgs/{org}", get(org_get))
        .route("/orgs/{org}/members", post(org_set_member))
        .route("/orgs/{org}/members/{account}", delete(org_remove_member))
        .route("/orgs/{org}/teams", post(team_create))
        .route("/orgs/{org}/teams/{team}/members", post(team_set_member))
        .route(
            "/orgs/{org}/teams/{team}/members/{account}",
            delete(team_remove_member),
        )
        .route("/orgs/{org}/invites", post(invite_create))
        .route("/invites/{token}/redeem", post(invite_redeem))
        .route("/internal/raft", post(internal_raft))
        .route("/internal/system", post(internal_system))
        .route("/internal/call", post(internal_call))
        .route("/internal/blob", post(internal_blob_put))
        .route("/internal/blob/get", post(internal_blob_get))
        .route("/internal/view", post(internal_view))
        .route("/internal/app-ready", post(internal_app_ready))
        .route("/internal/events", get(internal_events))
        .route("/ops/status", get(ops_status))
        .route("/ops/nodes", post(ops_register_node))
        .route("/ops/apps/{app}/replicas", post(ops_set_replicas))
        .route("/ops/backup", post(ops_backup))
        .route("/ops/quotas", get(ops_quotas_get).post(ops_quotas_set))
        // Bundles (peer blob pushes, uploads) exceed axum's 2 MiB default; the sealed envelope caps requests at 64 MiB.
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(node)
}
