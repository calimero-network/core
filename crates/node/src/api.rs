//! The node's HTTP API. Clients reach it inside the sealed channel; only
//! `/attestation` and `/health` are served in the clear.
//!
//! - Client: `/auth/*`, `/bundles`, `/apps`, `/apps/{app}/{intents,views,methods/{method},openapi.json,events,upgrade,access}`,
//!   `/orgs/*`, `/invites/{token}/redeem`, `/consent/requests`, `/consent/{id}/result`.
//! - The consent page (`consent_page`) is served in the clear, outside this
//!   router, on the node's own origin.
//! - Internal (peers, cluster secret): `/internal/*`.
//! - Ops (mdma, operator token): `/ops/*`.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::convert::Infallible;
use std::ops::Bound;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes as Body;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use calimero_identity::{
    b64url, confirm_challenge, peek_session, random, sha256, verify_assertion, verify_registration,
    verify_session, Assertion, Registration, RelyingParty, SessionClaims,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::app::{AppMachine, Status};
use crate::attest::{kms_report_data, quote, report_data, AttestationDoc};
use crate::error::ApiError;
use crate::node::blob_guards::MAX_LEASE_IDS;
use crate::node::{CallRequest, Node, ViewRequest};
use crate::report::{self, Reader, MAX_REPORT_PAGE};
use crate::system::cluster_version::{scope_app_features, Feature};
use crate::system::{
    deployment_id, deployment_name_fits, deployment_segment_fits, Deployer, Deployers, Deployment,
    Deployments, PackageRef, RefreshGrant, SignUp, SignUpInvite, SystemCmd, SystemState, OPERATOR,
};
use crate::types::{
    now_secs, Access, Account, ApiKeyRecord, Caller, Invite, NodeRole, OrgRole, RefreshRecord,
    TeamJoin,
};

const SESSION_TTL: u64 = 15 * 60;
const REFRESH_TTL: u64 = 30 * 24 * 60 * 60;

/// How long a refresh token is kept by the refreshes that present it
/// (`Config::refresh_rotate_after`): younger, a refresh mints a session on the
/// system state at a read index and writes nothing to the system log; older,
/// it is exchanged for a new token in the log (`UseRefresh`). Each active
/// session then writes once per this, not once per [`SESSION_TTL`].
pub const REFRESH_ROTATE_AFTER: Duration = Duration::from_secs(60 * 60);
const CHALLENGE_TTL: u64 = 5 * 60;
/// The code of a create or upgrade by bundle hash whose releases ship different compute components.
const AMBIGUOUS_COMPUTE: &str = "AMBIGUOUS_COMPUTE";

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

mod account_proof;
mod approvals;
mod apps;
mod auth;
mod calls;
mod deployments;
mod deprecation;
mod exports;
mod files;
mod internal;
#[cfg(test)]
mod openapi;
mod ops;
mod orgs;
mod origins;
mod packages;
mod passkeys;
mod pipeline_events;
pub(crate) mod pipeline_receipt;
mod pipelines;
mod recovery;
mod restore;
mod sign_up;
mod upgrades;

pub(crate) use approvals::{
    set_outbound_approval, set_release_approval, OutboundApprovalBody, ReleaseApprovalBody,
};
pub(crate) use exports::ExportLimits;
pub(crate) use internal::peer_only;
pub(crate) use orgs::{create_invite, create_org, redeem_invite, InviteBody};
pub use pipeline_events::MAX_RUN_SUBSCRIBERS_PER_APP;
pub(crate) use upgrades::{refuse_stalled, upgrade_app, UpgradeBody};
// The request bodies the OpenAPI registry names as `super::<Body>`.
#[cfg(test)]
use {
    approvals::{OpsDeadlineBody, OpsOutboundBody, OpsReleaseBody},
    apps::{AccessBody, AppAttestQuery, AppListQuery, CreateAppBody, MoveBody, TransferBody},
    auth::{
        ActivityQuery, AddPasskeyFinish, AliasBody, ApiKeyBody, AssertionBody, LoginBegin,
        PasskeyLabelBody, RefreshBody, RegisterBegin, RegisterFinish,
    },
    calls::{EventsQuery, IntentBody, MethodBody, PresenceBody, RunsQuery, ViewBody},
    deployments::{ClusterOwnerBody, DeployerBody, DeploymentBody, OfferBody},
    files::{BeginUpload, BlobGrant, BlobUploadQuery},
    ops::{BackupQuery, NodeBody, PoolsBody, PricesBody, ReplicaBody},
    orgs::{
        AllAppsBody, CapabilitiesBody, MemberBody, MembersQuery, OrgBody, RenameOrgBody,
        ScopeAppsBody, TeamBody, TeamIdsBody, TeamMemberBody, TeamRowsBody, TeamUpdateBody,
    },
    packages::{InstallBody, PublisherBody},
    recovery::{RecoveryBegin, RecoveryFinish, RecoveryKeyBody, RecoveryStart, RecoveryStartBody},
    sign_up::{OpsSignUpInviteBody, SignUpBody, SignUpInviteBody},
    upgrades::{OpsUpgradeBody, OrgUpgradeBody, PublisherUpgradeBody},
};

/// The area modules `router` merges (`.merge(<area>::routes())`) and their
/// sources, for the tests that read the route table from source (`openapi`,
/// `metrics`).
#[cfg(test)]
pub(crate) const AREA_SOURCES: &[(&str, &str)] = &[
    ("approvals", include_str!("api/approvals.rs")),
    ("apps", include_str!("api/apps.rs")),
    ("auth", include_str!("api/auth.rs")),
    ("calls", include_str!("api/calls.rs")),
    ("deployments", include_str!("api/deployments.rs")),
    ("files", include_str!("api/files.rs")),
    ("internal", include_str!("api/internal.rs")),
    ("ops", include_str!("api/ops.rs")),
    ("orgs", include_str!("api/orgs.rs")),
    ("packages", include_str!("api/packages.rs")),
    ("recovery", include_str!("api/recovery.rs")),
    ("sign_up", include_str!("api/sign_up.rs")),
    ("upgrades", include_str!("api/upgrades.rs")),
];

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
    /// A sign-in: its challenge is the statement's, which states the
    /// session it opens ([`NewSession`]).
    Login {
        challenge: [u8; 32],
        expires: u64,
        statement: crate::proof::Statement,
        session: NewSession,
    },
    /// A sign-up whose passkey is registered and must now sign the account
    /// it creates (`account:<id> / __create_account`).
    SignUp {
        challenge: [u8; 32],
        expires: u64,
        statement: crate::proof::Statement,
        account: Box<Account>,
        session: NewSession,
        app: Option<String>,
        /// The registered origin the sign-up came from, which the session
        /// names (`SessionClaims::origin`).
        #[serde(default)]
        origin: Option<String>,
    },
    AddPasskey {
        challenge: [u8; 32],
        account: String,
        expires: u64,
    },
    /// An account recovery's start: the account the code named at begin,
    /// and the code's public key (never the code), which the start's code
    /// must match.
    RecoverStart {
        challenge: [u8; 32],
        account: String,
        recovery_key: String,
        expires: u64,
    },
    /// An account recovery's finish: a sign-in challenge stating the session
    /// it opens ([`NewSession`]), for the account the request named, if any.
    RecoverFinish {
        challenge: [u8; 32],
        expires: u64,
        statement: crate::proof::Statement,
        session: NewSession,
        account: Option<String>,
    },
    Confirm {
        challenge: [u8; 32],
        app: String,
        method: String,
        args: Vec<u8>,
        /// The challenge's nonce (base64url): logged with the consent, so
        /// `apply` can rebuild the challenge.
        nonce: String,
        account: String,
        expires: u64,
        /// What the retry needs that is not signed and never logged (a new
        /// API key, an invite token, the registered credential), for a
        /// step-up whose arguments the node made ([`consent_minted`]).
        #[serde(default)]
        carry: Option<Value>,
    },
}

impl Pending {
    const fn expires(&self) -> u64 {
        match self {
            Self::Register { expires, .. }
            | Self::Login { expires, .. }
            | Self::SignUp { expires, .. }
            | Self::AddPasskey { expires, .. }
            | Self::RecoverStart { expires, .. }
            | Self::RecoverFinish { expires, .. }
            | Self::Confirm { expires, .. } => *expires,
        }
    }

    /// The account whose holder the ceremony is for, when the caller proved
    /// it (a session, an API key, a recovery code): such ceremonies draw on
    /// that account's budget, the rest on the anonymous one.
    fn account(&self) -> Option<&str> {
        match self {
            Self::AddPasskey { account, .. }
            | Self::RecoverStart { account, .. }
            | Self::Confirm { account, .. } => Some(account),
            Self::Register { .. }
            | Self::Login { .. }
            | Self::SignUp { .. }
            | Self::RecoverFinish { .. } => None,
        }
    }
}

/// The most ceremonies a node holds open for callers it does not know yet
/// (sign-in, sign-up, a recovery's finish). Each lives at most
/// `CHALLENGE_TTL`; past this the node answers 429 instead of growing without
/// bound (Xil's review, issue 12). Ceremonies of known accounts do not count
/// here, so a full anonymous pool never blocks an account's step-up.
pub const MAX_PENDING_CHALLENGES: usize = 10_000;
/// The most ceremonies a node holds open for known accounts, all together.
pub const MAX_ACCOUNT_CHALLENGES: usize = 10_000;
/// The most ceremonies a node holds open for one account, so no account
/// fills the pool the others share.
pub const MAX_CHALLENGES_PER_ACCOUNT: usize = 64;

fn put_pending(node: &Node, pending: Pending) -> Result<String, ApiError> {
    let id = format!("{}.{}", node.config.node_id, b64url(&random::<16>()));
    let mut c = node
        .challenges
        .lock()
        .map_err(|_| ApiError::internal("challenge store poisoned"))?;
    admit_pending(&mut c, &pending, now_secs())?;
    let _prev = c.insert(id.clone(), pending);
    Ok(id)
}

/// Drop expired ceremonies, then refuse `pending` when its budget is full:
/// the anonymous pool, or its account's share of the accounts' pool.
fn admit_pending(
    c: &mut HashMap<String, Pending>,
    pending: &Pending,
    now: u64,
) -> Result<(), ApiError> {
    c.retain(|_, p| p.expires() > now);
    let full = match pending.account() {
        None => c.values().filter(|p| p.account().is_none()).count() >= MAX_PENDING_CHALLENGES,
        Some(account) => {
            let (mine, all) = c
                .values()
                .filter_map(Pending::account)
                .fold((0, 0), |(mine, all), a| {
                    (mine + usize::from(a == account), all + 1)
                });
            mine >= MAX_CHALLENGES_PER_ACCOUNT || all >= MAX_ACCOUNT_CHALLENGES
        }
    };
    if full {
        return Err(ApiError::new(
            429,
            "RATE_LIMITED",
            "too many passkey ceremonies in progress; try again shortly",
        ));
    }
    Ok(())
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
            .sys(|s| s.replica_node(&holder).map(|n| n.addr.clone()))
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

/// A package's frontend origin a ceremony comes from: one the node serves
/// under the apps domain (`app_host`), or one its manager registered
/// (`crate::origins`), a custom domain the node serves included.
#[derive(Clone, Debug)]
pub(crate) struct AppOrigin {
    pub(crate) package: String,
    pub(crate) origin: String,
    /// A registered origin's rp-id (a custom domain's too); `None` for a
    /// frontend under the apps domain, whose ceremonies use the cluster's.
    pub(crate) rp_id: Option<String>,
}

impl AppOrigin {
    /// The origin a session opened here names (`SessionClaims::origin`):
    /// only a registered one, which can be removed.
    fn registered(&self) -> Option<String> {
        self.rp_id.as_ref().map(|_| self.origin.clone())
    }
}

/// The package frontend `origin` is, if any: a served one first, else a
/// registered one whose rp-id cannot answer for the cluster's passkeys.
pub(crate) fn app_origin(node: &Node, origin: &str) -> Option<AppOrigin> {
    if let Some(package) = node.app_origin_package(origin) {
        return Some(AppOrigin {
            package,
            origin: origin.to_owned(),
            rp_id: None,
        });
    }
    let held = node.sys(|s| s.package_origins.get(origin).cloned())?;
    if crate::origins::covers_cluster(&held.rp_id, &node.config.rp_id) {
        return None;
    }
    Some(AppOrigin {
        package: held.package,
        origin: origin.to_owned(),
        rp_id: Some(held.rp_id),
    })
}

/// The package frontend a ceremony claims to come from (unverified: the
/// ceremony is then checked against that origin).
fn claimed_app(node: &Node, client_data_json: &str) -> Option<AppOrigin> {
    app_origin(node, &calimero_identity::claimed_origin(client_data_json)?)
}

/// The relying party for a ceremony that may come from `app`'s frontend
/// origin: a served one as well as the cluster's own origins; a registered
/// one alone, under its own rp-id.
fn rp_with(base: RelyingParty, app: Option<&AppOrigin>) -> RelyingParty {
    let mut rp = base;
    match app {
        Some(AppOrigin {
            origin,
            rp_id: Some(id),
            ..
        }) => {
            rp.id.clone_from(id);
            rp.origins = vec![origin.clone()];
        }
        Some(a) => rp.origins.push(a.origin.clone()),
        None => {}
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
    /// The verified release of the node that signed the session: it reaches only the apps whose owner approved that release.
    pub issuer_release: Option<String>,
    /// The node's consent page acting for the account a consent request
    /// names (`consent_page`): no session of its own, so every action it
    /// takes is a step-up, and the step-up may come from the node's own
    /// origins as well as the cluster's.
    pub consent_page: bool,
    /// The system index the request's read-index round named (`None`: no
    /// leader answered, or no round was made for it). Every system write
    /// committed before the request arrived is at or below it, so the other
    /// checks made for the request, here or on the app's leader
    /// (`CallRequest::sys_index`), wait to apply it instead of running a
    /// round of their own (`Node::sys_fresh_from`, which gives the argument).
    pub sys_index: Option<u64>,
}

impl Principal {
    pub(crate) fn account(&self) -> Result<String, ApiError> {
        self.caller
            .account()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::unauthenticated("sign in first"))
    }

    /// Whether the caller is an upgrade key ([`ApiKeyRecord::upgrades`]).
    pub(crate) fn upgrade_key(&self) -> bool {
        self.api_key.as_ref().is_some_and(|k| k.upgrades.is_some())
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
        // A session vouches for its account only where the node that signed
        // it may hold the data: a node on an unapproved release cannot mint
        // its way into an app (F1).
        if let Some(release) = &self.issuer_release {
            if !node.sys(|s| s.release_may_hold(app, Some(release))) {
                return Err(ApiError::unauthenticated(
                    "this session was signed by a node whose release this app's owner has not approved; sign in again",
                ));
            }
        }
        match self.bound_app() {
            None => Ok(()),
            Some(pkg)
                if node
                    .release_of_app(app)
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

/// Who is calling, refusing an upgrade key: such a key upgrades one app
/// (`POST /apps/{app}/upgrade`, [`credential`]) and is refused everywhere
/// else, reads included.
async fn principal(node: &Node, headers: &HeaderMap) -> Result<Option<Principal>, ApiError> {
    let p = credential(node, headers).await?;
    if p.as_ref().is_some_and(Principal::upgrade_key) {
        return Err(upgrade_key_only());
    }
    Ok(p)
}

/// What an upgrade key gets on any route but its app's upgrade.
fn upgrade_key_only() -> ApiError {
    ApiError::new(
        403,
        "UPGRADE_KEY_ONLY",
        "this API key upgrades one app and does nothing else",
    )
}

/// The bearer token of the `Authorization` header.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("authorization").and_then(|v| v.to_str().ok())?;
    Some(value.strip_prefix("Bearer ").unwrap_or(value).trim())
}

/// Who is calling, from the `Authorization` header, upgrade keys included.
async fn credential(node: &Node, headers: &HeaderMap) -> Result<Option<Principal>, ApiError> {
    let Some(token) = bearer(headers) else {
        return Ok(None);
    };
    // A credential is judged on the cluster's latest system state (a read
    // index): a session or key revoked through another node is refused here
    // at once, not once this replica catches up.
    let (sys_index, _caught_up) = node.sys_fresh_index().await;
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
            issuer_release: None,
            consent_page: false,
            sys_index,
        }));
    }
    let (claims, issuer_release) = verify_token(node, token).await?;
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
    // A session opened on a registered origin ends when the origin is removed.
    if let Some(origin) = &claims.origin {
        if !origin_holds(node, claims.app.as_deref(), origin) {
            return Err(origin_gone());
        }
    }
    Ok(Some(Principal {
        caller: Caller::Account {
            account: claims.account.clone(),
        },
        session: Some(claims),
        api_key: None,
        issuer_release,
        consent_page: false,
        sys_index,
    }))
}

/// Whether `origin` is still registered for `package`.
fn origin_holds(node: &Node, package: Option<&str>, origin: &str) -> bool {
    node.sys(|s| {
        s.package_origins
            .get(origin)
            .is_some_and(|o| Some(o.package.as_str()) == package)
    })
}

fn origin_gone() -> ApiError {
    ApiError::unauthenticated(
        "the origin this session was opened on is no longer registered for its app; sign in again",
    )
}

/// Check a session token: signed by the receipt key of the node it names,
/// which this node attests at that node's registered address, and whose
/// attested release may serve the account
/// (`SystemState::release_may_serve_account`). Answers the claims and the
/// issuer's release, which each app access checks too (`Principal::within`).
async fn verify_token(
    node: &Node,
    token: &str,
) -> Result<(SessionClaims, Option<String>), ApiError> {
    let claims = peek_session(token).map_err(|_| ApiError::unauthenticated("malformed token"))?;
    let issuer_id = claims.node;
    let unknown = || {
        ApiError::unauthenticated(
            "the token's issuing node is not a node of this cluster, or cannot be attested; refresh the session",
        )
    };
    // Answered from the key this node attested ahead of time: the issuer may
    // be stopped or frozen by now.
    let mut issuer = node
        .session_issuer(issuer_id, false)
        .await
        .ok_or_else(unknown)?;
    let named = calimero_identity::from_b64url(&claims.key).ok();
    if named.as_deref() != Some(issuer.key.as_slice()) {
        // Signed under another key than the one attested here: the issuer's
        // data directory may have been replaced since (a restart keeps it).
        issuer = node
            .session_issuer(issuer_id, true)
            .await
            .ok_or_else(unknown)?;
    }
    let claims = verify_session(&issuer.key, token, now_secs())
        .map_err(|e| ApiError::unauthenticated(&e.to_string()))?;
    let release = issuer
        .release
        .ok_or_else(|| ApiError::unauthenticated("the token's issuing node names no release"))?;
    if !node.sys(|s| s.release_may_serve_account(&claims.account, &release)) {
        if issuer_id == node.config.node_id {
            return Err(wrong_node());
        }
        return Err(ApiError::unauthenticated(
            "this session was signed by a node whose release the account's owners have not approved; sign in again",
        ));
    }
    Ok((claims, Some(release)))
}

/// This node's release is not one the account's approvers approved: it
/// neither issues nor takes the account's sessions. `421`, so a client tries
/// another node rather than refreshing here.
fn wrong_node() -> ApiError {
    ApiError::new(
        421,
        "WRONG_NODE",
        "this node runs a release the account's owners have not approved; use another node of the cluster",
    )
}

/// Whether this node's release may serve `account`: issue and take its
/// sessions (`SystemState::release_may_serve_account`). Asked before a sign-in,
/// sign-up or refresh spends anything (a challenge, a refresh token), so a
/// `421 WRONG_NODE` leaves the client free to finish on another node.
fn serves(node: &Node, account: &str) -> bool {
    let release = node.own_release();
    node.sys(|s| s.release_may_serve_account(account, &release))
}

/// A session token for `claims`, signed by this node's attested receipt key.
fn session_token(node: &Node, claims: SessionClaims) -> Result<String, ApiError> {
    // A session no other node would take is not issued: the client goes to a
    // node its approvers approved instead.
    if !serves(node, &claims.account) {
        return Err(wrong_node());
    }
    Ok(node.mint_session(claims))
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

/// A session's refresh token, made before the passkey is asked: the
/// ceremony's challenge states it, so the log takes it only with that
/// passkey's assertion (`system::sign_in_args`, `system::account_args`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewSession {
    refresh: String,
    session: String,
    expires_at: u64,
}

impl NewSession {
    fn new() -> Self {
        Self {
            refresh: format!("rt_{}", b64url(&random::<32>())),
            session: b64url(&random::<16>()),
            expires_at: now_secs() + REFRESH_TTL,
        }
    }

    fn hash(&self) -> String {
        b64url(&sha256(self.refresh.as_bytes()))
    }

    fn record(
        &self,
        account: &str,
        passkey: &str,
        app: Option<String>,
        origin: Option<String>,
    ) -> RefreshRecord {
        RefreshRecord {
            account: account.to_owned(),
            session: self.session.clone(),
            expires_at: self.expires_at,
            passkey: Some(passkey.to_owned()),
            app,
            origin,
            key: crate::proof::refresh_key(&self.refresh),
            // Stated nowhere a passkey signs: each entry sets it
            // (`CreateRefresh`'s record, `CreateAccount::at`).
            opened_at: 0,
        }
    }

    /// What a sign-in's challenge states (`session / __sign_in`): the
    /// record's account, passkey and package are not known yet and not
    /// stated; the passkey that signs names them.
    fn sign_in_statement(&self) -> crate::proof::Statement {
        let args = crate::system::sign_in_args(&self.hash(), &self.record("", "", None, None));
        crate::proof::Statement::new("session", "__sign_in", &args)
    }
}

/// The session token for `new`, signed by this node.
fn session_token_for(
    node: &Node,
    account: &str,
    passkey: &str,
    app: Option<String>,
    origin: Option<String>,
    new: &NewSession,
) -> Result<(String, u64), ApiError> {
    let expires_at = now_secs() + SESSION_TTL;
    let token = session_token(
        node,
        SessionClaims {
            account: account.to_owned(),
            session: new.session.clone(),
            expires_at,
            passkey: Some(passkey.to_owned()),
            app,
            origin,
            node: 0,
            key: String::new(),
        },
    )?;
    Ok((token, expires_at))
}

fn session_answer(
    account: &str,
    (token, expires_at): &(String, u64),
    new: &NewSession,
) -> Json<Value> {
    Json(json!({
        "accountId": account,
        "sessionToken": token,
        "expiresAt": expires_at,
        "refreshToken": new.refresh,
    }))
}

// ---------------------------------------------------------------- public

async fn health(State(node): State<Arc<Node>>) -> Json<Value> {
    let s = node.system.status();
    Json(json!({
        "ok": true,
        "node": node.config.node_id,
        "systemLeader": s.leader,
        "release": node.config.release,
        // The newest cluster version this build supports: what the system
        // leader raises the cluster version by (`system::cluster_version`).
        "clusterVersion": crate::system::cluster_version::supported(),
        // The cluster's rollback epoch (rollbacks forced so far); absent
        // on a node without a rollback counter.
        "rollbackEpoch": node.checkpoints.as_ref().and_then(|c| c.status()).map(|s| s.epoch),
    }))
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
    /// `orchestrator`: the transport key and the cluster identity this replica holds.
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
    if q.purpose.as_deref() == Some("jobs") {
        return Err(ApiError::not_found("this node is not a job node"));
    }
    #[cfg(feature = "testing")]
    if let Some(delay) = crate::testing::attestation_delay(&node.config.public_url) {
        tokio::time::sleep(delay).await;
    }
    let transport_key = node.transport.public_key();
    let receipts = q.purpose.as_deref() == Some("receipts");
    // Only when asked: a client that does not know the KMS binding keeps the
    // report data it has always checked.
    let kms = (q.purpose.as_deref() == Some("kms") && !node.config.kms_keys.is_empty())
        .then(|| node.config.kms_keys.clone());
    // The rules the cluster was deployed with, as this replica's state holds
    // them: bound with the receipt key, so a user proves both at once.
    let governance = node.sys(crate::system::SystemState::governance);
    let orchestrator = q.purpose.as_deref() == Some("orchestrator");
    let report = if orchestrator {
        calimero_client::attest::orchestrator_report_data(
            &nonce,
            &transport_key,
            &node.cluster_identity(),
        )
    } else if receipts {
        let mut r = [0; 64];
        r[..32].copy_from_slice(&nonce);
        r[32..].copy_from_slice(&calimero_client::attest::receipt_key_binding(
            &node.receipt_public(),
            &governance,
            &node.config.cluster,
            node.config.node_id,
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
        job_key: None,
        cluster_identity: orchestrator.then(|| hex::encode(node.cluster_identity())),
    };
    Ok(Json(serde_json::to_value(doc).unwrap_or_default()))
}

// ---------------------------------------------------------------- auth

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
) -> Result<(String, Option<AppOrigin>), ApiError> {
    let app = claimed_app(node, &assertion.client_data_json)
        .filter(|a| any_app || bound == Some(a.package.as_str()));
    let (owner, cred) = node
        .sys_until(|s| {
            s.account_of_credential(&assertion.credential_id)
                .map(|(a, c)| (a.account_id.clone(), c.clone()))
        })
        .await
        .ok_or_else(passkeys::unknown_here)?;
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
                assertion: serde_json::to_value(assertion).unwrap_or_default(),
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
    Ok((owner, app))
}

const fn manifest() -> calimero_primitives::outbound::OutboundSelection {
    calimero_primitives::outbound::OutboundSelection::Manifest
}

/// What the node knows of each account's use, from its live sessions: how
/// many it holds and when the latest was last exchanged
/// (`expires_at - REFRESH_TTL`), by account. One pass over the refresh
/// records, each session counted once.
fn session_use(s: &SystemState, now: u64) -> BTreeMap<&str, (usize, u64)> {
    let mut seen: BTreeMap<&str, (&str, u64)> = BTreeMap::new();
    for r in s.refresh.values() {
        if r.expires_at <= now || s.revoked_sessions.contains(&r.session) {
            continue;
        }
        let at = r.expires_at.saturating_sub(REFRESH_TTL);
        let _prev = seen
            .entry(r.session.as_str())
            .and_modify(|(_, t)| *t = (*t).max(at))
            .or_insert((r.account.as_str(), at));
    }
    let mut out: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    for (account, at) in seen.into_values() {
        let e = out.entry(account).or_insert((0, 0));
        e.0 += 1;
        e.1 = e.1.max(at);
    }
    out
}

/// When `account` was last active, as far as the node knows (Unix seconds):
/// its passkeys' latest sign-in or its sessions' latest exchange
/// ([`session_use`]); `None` when it knows neither.
fn last_active(s: &SystemState, account: &str, used: &BTreeMap<&str, (usize, u64)>) -> Option<u64> {
    let signed_in = s.accounts.get(account).and_then(|a| {
        a.passkey_times
            .values()
            .filter_map(|t| t.last_sign_in_at)
            .max()
    });
    let exchanged = used.get(account).map(|u| u.1).filter(|t| *t > 0);
    signed_in.max(exchanged)
}

/// A Unix time's UTC day, `YYYY-MM-DD`.
fn utc_day(secs: u64) -> Option<String> {
    let secs = i64::try_from(secs).ok()?;
    chrono::DateTime::from_timestamp(secs, 0).map(|t| t.format("%Y-%m-%d").to_string())
}

// ---------------------------------------------------------------- account recovery

// `docs/specs/account-recovery.md`: a code made with a passkey; signed out,
// the code and a new passkey start a recovery; within `RECOVERY_DELAY_SECS`
// (72 h, as an organization's) a passkey the account has cancels it; after,
// the new passkey finishes it and signs in.

// ---------------------------------------------------------------- bundles and apps

/// The bundle a create or upgrade names, with the compute component of the release it
/// comes from: a bundle hash (a raw upload, no compute), or a package release (installed
/// from the registry first if the fleet lacks it).
async fn bundle_named(
    node: &Node,
    bundle: Option<String>,
    package: Option<&str>,
    version: Option<&str>,
    service: Option<&str>,
    actor: &str,
    frontend: bool,
) -> Result<(String, Option<String>, Option<PackageRef>), ApiError> {
    match (bundle, package) {
        // A hash names a release's WASM only for who may see that release;
        // it names no release (`AppRecord::release` stays unset).
        (Some(b), None) if node.sys(|s| s.sees_bundle(&b, actor)) => {
            let compute = node
                .sys(|s| s.compute_of(&b, actor))
                .map_err(|why| ApiError::bad_request(AMBIGUOUS_COMPUTE, why))?;
            Ok((b, compute, None))
        }
        (Some(_), None) => Err(crate::node::rejected("unknown bundle")),
        (None, Some(p)) => {
            let (bundle, compute, release) = node
                .resolve_release_parts(p, version, service, actor, frontend)
                .await?;
            Ok((bundle, compute, Some(release)))
        }
        _ => Err(ApiError::bad_request(
            "BAD_REQUEST",
            "name either a bundle or a package",
        )),
    }
}

fn init_state(state: &crate::app::AppState) -> Option<Result<(), Value>> {
    match (&state.bundle, &state.init_error) {
        (None, _) => None,
        (Some(_), Some(e)) => Some(Err(e.clone())),
        (Some(_), None) => Some(Ok(())),
    }
}

/// The caller, if they may reach `app` with this request kind.
/// Whether `account` holds a read-only row in the team `app` belongs to
/// (checked again against the latest system state before saying yes: at or
/// past `sys_index`, the index the request's round named, else a round's).
pub async fn read_only_in(
    node: &Node,
    app: &crate::types::AppRecord,
    account: Option<&str>,
    sys_index: Option<u64>,
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
        node.sys_fresh_from(sys_index).await;
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
    let sys_index = match &p {
        Some(p) => p.sys_index,
        None => node.sys_fresh_index().await.0,
    };
    if !reaches(node, app, account) {
        return Err(if p.is_none() {
            ApiError::unauthenticated("sign in to use this app")
        } else {
            ApiError::not_found("no such app")
        });
    }
    // A read-only row refuses what writes: a blob upload here, a call that
    // would write at commit (`Node::call_local`); v1 discarded such writes.
    if write && method == "__blob" && read_only_in(node, app, account, sys_index).await {
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
        issuer_release: None,
        consent_page: false,
        sys_index,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmBody {
    challenge_id: String,
    assertion: Assertion,
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
    Ok(step_up(node, p, app_id, method, args, None, confirm, true)
        .await?
        .consent)
}

/// A passkey step-up over arguments the node makes itself (an id, an expiry,
/// a key): the first request makes them, keeps them in the challenge with
/// `carry`, and answers `412`; the retry answers the consent with the
/// arguments that were signed and the carry, whatever the retry's own
/// request would make now. The caller checks them against the request.
pub(crate) async fn consent_minted(
    node: &Node,
    p: &Principal,
    subject: &str,
    method: &str,
    args: &Value,
    carry: Option<Value>,
    confirm: Option<ConfirmBody>,
) -> Result<Consented, ApiError> {
    step_up(node, p, subject, method, args, carry, confirm, false).await
}

/// A consent, the arguments it signed, and what the challenge carried.
pub(crate) struct Consented {
    pub consent: Value,
    pub args: Value,
    pub carry: Option<Value>,
}

/// [`consent`] and [`consent_minted`]: `exact` refuses a retry whose
/// arguments differ from the signed ones.
#[allow(
    clippy::too_many_arguments,
    reason = "the step-up's subject, method, arguments, carry and answer"
)]
async fn step_up(
    node: &Node,
    p: &Principal,
    app_id: &str,
    method: &str,
    args: &Value,
    carry: Option<Value>,
    confirm: Option<ConfirmBody>,
    exact: bool,
) -> Result<Consented, ApiError> {
    // An API key never passes a passkey step-up for its holder: refused
    // unless the key was made for confirm methods (and then the retry still
    // needs a passkey assertion).
    if p.api_key.as_ref().is_some_and(|k| !k.allow_confirm) {
        return Err(ApiError::forbidden(
            "this method needs the account holder's passkey",
        ));
    }
    // A package-bound session steps up over its package's apps only: never
    // for the account, an organization, the cluster or another package.
    if p.bound_app().is_some() {
        match node.sys(|s| s.apps.get(app_id).cloned()) {
            Some(app) => p.within(node, &app)?,
            None => p.unbound()?,
        }
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
                nonce: b64url(&nonce),
                account: account.clone(),
                expires: now_secs() + CHALLENGE_TTL,
                carry,
            },
        )?;
        let creds: Vec<String> = node.sys(|s| {
            s.accounts
                .get(&account)
                .map(|a| a.passkeys.iter().map(|c| c.id.clone()).collect())
                .unwrap_or_default()
        });
        // A session opened on a registered origin steps up there, under its rp-id.
        let rp_id = p
            .session
            .as_ref()
            .and_then(|s| s.origin.as_deref())
            .and_then(|o| app_origin(node, o))
            .and_then(|a| a.rp_id)
            .unwrap_or_else(|| node.config.rp_id.clone());
        return Err(confirm_required(
            "approve this call with your passkey",
            &id,
            &challenge,
            &rp_id,
            &creds,
        ));
    };
    let Pending::Confirm {
        challenge,
        app,
        method: m,
        args: a,
        nonce,
        account: acc,
        carry,
        ..
    } = take_pending(node, &confirm.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a confirm challenge",
        ));
    };
    if app != app_id || m != method || (exact && a != args_bytes) || acc != account {
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
    // Logged with what was signed, so every replica checks it where the
    // command applies (`system::authorize`).
    let signed = String::from_utf8(a).unwrap_or_default();
    let args = serde_json::from_str(&signed).unwrap_or(Value::Null);
    let consent = crate::proof::Consent {
        statement: crate::proof::Statement {
            subject: app,
            method: m,
            args: signed,
            nonce,
        },
        assertion: confirm.assertion,
        // When it was given: the account's activity entry for it.
        at: now_secs(),
    };
    Ok(Consented {
        consent: serde_json::to_value(&consent).unwrap_or_default(),
        args,
        carry,
    })
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

/// How a refusal names the release an app would run: `{package}@{version}`,
/// or the bundle of a raw development upload.
pub(crate) fn release_label(release: Option<&PackageRef>, bundle: &str) -> String {
    match release {
        Some(r) => format!("{}@{}", r.package, r.version),
        None => format!("bundle {bundle}"),
    }
}

// ---------------------------------------------------------------- organizations

#[derive(Deserialize)]
struct MetadataBody {
    metadata: Value,
}

/// A body that carries only a passkey confirmation (after a `412`).
#[derive(Deserialize, Default)]
struct ConfirmOnlyBody {
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// A membership change in `org`, approved by `p`'s passkey (412 first): the
/// consent to log with it ([`consent`] on `org:<org>`).
async fn org_consent(
    node: &Node,
    p: &Principal,
    org: &str,
    method: &str,
    args: &Value,
    confirm: Option<ConfirmBody>,
) -> Result<Value, ApiError> {
    consent(node, p, &format!("org:{org}"), method, args, confirm).await
}

// ---------------------------------------------------------------- recovery

/// The time a recovery command carries: this node's clock (moved only by
/// tests, `Node::advance_clock`), stamped once and replicated with it.
fn recovery_now(node: &Node) -> u64 {
    node.app_clock().now_ms() / 1000
}

// ---------------------------------------------------------------- internal (peers)

/// Compare secrets without leaking where they differ.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    bool::from(subtle::ConstantTimeEq::ct_eq(a, b))
}

/// How many cluster-key requests one node judges at once (each verifies a
/// quote, which in TDX mode fetches a release); past it, `429`.
pub const MAX_CONCURRENT_KEY_RELEASES: usize = 4;

/// How often, at most, `internal_raft` warns that it accepted a Raft message
/// without an addressee: a peer on an earlier release sends one per heartbeat.
pub const NO_TARGET_WARN_EVERY: Duration = Duration::from_secs(60);

/// Whether a warning about an accepted message without an addressee is due:
/// true for the first one, then at most once per `NO_TARGET_WARN_EVERY` (one
/// slot per process, taken with a compare-and-swap, so concurrent messages
/// warn once between them). The count in between is in
/// `calimero_raft_misaddressed_total{reason="no_target"}`.
fn no_target_warning_due() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_secs().max(1);
    let last = LAST.load(Ordering::Relaxed);
    (last == 0 || now.saturating_sub(last) >= NO_TARGET_WARN_EVERY.as_secs())
        && LAST
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
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

/// Whether `account` may upload and list the files of `app`: its owner and the members of its
/// organization (not guests), even where the app is public; an organization's app's owner only
/// while a member ([`crate::system::SystemState::owner_acts`]).
fn may_handle_files(
    s: &crate::system::SystemState,
    app: &crate::types::AppRecord,
    account: Option<&str>,
) -> bool {
    account.is_some_and(|a| {
        s.owner_acts(app, a)
            || app
                .org
                .as_ref()
                .and_then(|o| s.orgs.get(o))
                .is_some_and(|o| o.role(a).is_some_and(|r| r >= OrgRole::Member))
    })
}

/// Whether `account` may manage `app`'s data (its owner, of an organization's
/// app while a member of it, or an admin of its organization): delete its
/// blobs ([`crate::system::SystemState::manages_app`]).
fn manages_app(node: &Node, app: &crate::types::AppRecord, account: Option<&str>) -> bool {
    account.is_some_and(|a| node.sys(|s| s.manages_app(app, a)))
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
    // No header and an empty bearer are the same nothing: never a credential,
    // whatever the node was configured with.
    let ok = ok && !given.is_empty();
    if ok {
        Ok(())
    } else {
        Err(ApiError::forbidden("operator credential required"))
    }
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
    let (nodes, job_nodes, apps, accounts, orgs, packages, bundle_bytes) = node.sys(|s| {
        (
            s.replica_nodes().count(),
            s.job_nodes().count(),
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
        "Replica nodes in the directory.",
        nodes as u64,
    );
    gauge(
        "calimero_cluster_job_nodes",
        "Job nodes in the directory.",
        job_nodes as u64,
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
        "# HELP calimero_system_read_rounds_total Read-index rounds this node began on the system group (one per request that needs fresh system state, shared by requests that wait together).\n# TYPE calimero_system_read_rounds_total counter\ncalimero_system_read_rounds_total {}",
        node.sys_rounds.load(std::sync::atomic::Ordering::Relaxed)
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

/// How long `/ops/placement` waits for a node it has not attested yet to name
/// its release; one that does not answer by then is listed with none.
pub(crate) const PLACEMENT_RELEASE_WAIT: Duration = Duration::from_secs(2);

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

/// The most a report waits for one app's replicas to name its last write
/// ([`Node::last_write_ms`]). A report asks for one page's apps at once, at
/// most [`MAX_REPORT_PAGE`].
pub(crate) const LAST_WRITE_WAIT: Duration = Duration::from_secs(2);

/// Set each of a report's `apps` entries' `lastWriteAt`: when the app last
/// committed a write (Unix seconds, the entry's timestamp, so every replica
/// names the same one); `null` before its first write, or when no replica
/// answered within [`LAST_WRITE_WAIT`].
async fn with_last_writes(node: &Node, apps: &mut [Value]) {
    let records: Vec<Option<crate::types::AppRecord>> = node.sys(|s| {
        apps.iter()
            .map(|a| a["app"].as_str().and_then(|id| s.apps.get(id)).cloned())
            .collect()
    });
    let writes = futures_util::future::join_all(records.iter().map(|r| async move {
        let r = r.as_ref()?;
        tokio::time::timeout(LAST_WRITE_WAIT, node.last_write_ms(r))
            .await
            .ok()
            .flatten()
    }))
    .await;
    for (app, ms) in apps.iter_mut().zip(writes) {
        app["lastWriteAt"] = json!(ms.map(|ms| ms / 1000));
    }
}

/// The report of one approval scope's apps for an account: an
/// organization's (`org`) or the account's own outside one (`None`). Lists
/// the apps the account reaches or may upgrade, within what its credential
/// is scoped to (a package-bound session: that package's apps; an API key
/// naming apps: those). Each app carries when it last committed a write
/// ([`with_last_writes`]).
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
    // Each node's runtime, which decides which newer release an app can
    // reach (`report::app_json`); kept a while (`Node::runtime_of_node`).
    let nodes: Vec<u64> = node.sys(|s| s.replica_nodes().map(|n| n.id).collect());
    let runtimes = node.runtimes(&nodes).await;
    let bound = p.bound_app();
    let key_apps = p.api_key.as_ref().and_then(|k| k.apps.as_ref());
    let mut out = node.sys(|s| {
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
            .map(|a| report::app_json(s, a, &releases, &runtimes, Reader::Account(account)))
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
    });
    if let Some(apps) = out["apps"].as_array_mut() {
        with_last_writes(node, apps).await;
    }
    out
}

/// `409`: a write that only this release, or a newer one, understands, refused while
/// a registered node runs an older release or could not be attested
/// (`Node::nodes_behind_release`). Today: a non-zero `maxFuelPerDeveloperPerDay`. One
/// code for every such gate, so the version gate on new entry shapes can share it.
pub const UPGRADE_PENDING: &str = "UPGRADE_PENDING";

/// `409`: a command (or a field of one) that a node of the cluster does not
/// support yet, refused before it is proposed, so no node skips an entry the
/// others apply (`system::cluster_version`). The message names the feature and
/// the version it needs; `details` is `{ feature, needs, active }`.
pub const FEATURE_NOT_ACTIVE: &str = "FEATURE_NOT_ACTIVE";

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

/// The operator's signature over a decision, as mdma sends it: the nonce of
/// the statement it signed (from the `428 OPERATOR_SIGNATURE_REQUIRED` that
/// named it, or its own) and the Ed25519 signature, hex.
#[derive(Clone, Debug, Default, Deserialize)]
struct OperatorProofBody {
    nonce: String,
    signature: String,
}

/// `421 NOT_LEADER`: send it to the group's leader at `leader_url`, node `leader` where it
/// is known (absent from `details` otherwise).
pub(crate) fn not_leader(message: &str, leader: Option<u64>, leader_url: &str) -> ApiError {
    let mut details = json!({ "leader": leader, "leaderUrl": leader_url });
    if leader.is_none() {
        let _gone = details.as_object_mut().and_then(|d| d.remove("leader"));
    }
    ApiError::new(421, "NOT_LEADER", message).with_details(details)
}

/// `412 CONFIRM_REQUIRED`: a passkey of `allow` signs `challenge` under the relying party
/// `rp_id`, and the same request sent again with that assertion names `id`.
pub(super) fn confirm_required(
    message: &str,
    id: &str,
    challenge: &[u8],
    rp_id: &str,
    allow: &[String],
) -> ApiError {
    ApiError::new(412, "CONFIRM_REQUIRED", message).with_details(json!({
        "challengeId": id,
        "challenge": b64url(challenge),
        "rpId": rp_id,
        "allowCredentials": allow,
    }))
}

/// `428 OPERATOR_SIGNATURE_REQUIRED`: the operator key of `cluster` signs `statement`; the
/// exact bytes to sign go in `message` (base64).
pub(super) fn operator_signature_required(
    statement: &crate::proof::Statement,
    cluster: &str,
) -> ApiError {
    let message = statement.operator_message(cluster);
    ApiError::new(
        428,
        "OPERATOR_SIGNATURE_REQUIRED",
        "sign this decision with the operator key and send it again with operatorProof",
    )
    .with_details(json!({
        "subject": statement.subject,
        "method": statement.method,
        "args": statement.args,
        "nonce": statement.nonce,
        "cluster": cluster,
        "message": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, message),
    }))
}

/// The operator's proof for `method` on `subject` with `args`, to log with
/// the decision ([`crate::proof::OperatorProof`]). Without one: `428
/// OPERATOR_SIGNATURE_REQUIRED`, naming the statement and the exact bytes to
/// sign under a fresh nonce. A proof that does not verify under the recorded
/// operator key: `403`.
fn operator_proof(
    node: &Node,
    subject: &str,
    method: &str,
    args: &Value,
    given: Option<OperatorProofBody>,
) -> Result<Value, ApiError> {
    let key = node.sys(|s| s.operator_key.clone());
    let mut statement = crate::proof::Statement::new(subject, method, args);
    let Some(given) = given else {
        let Some(key) = key else {
            return Err(ApiError::forbidden(
                "no operator key is registered: start the nodes with --operator-key",
            ));
        };
        return Err(operator_signature_required(&statement, &key.cluster));
    };
    statement.nonce = given.nonce;
    let proof = serde_json::to_value(crate::proof::OperatorProof {
        statement,
        signature: given.signature,
    })
    .unwrap_or_default();
    // Checked here too, so a bad signature is a 403 and not a log refusal.
    let _checked = crate::proof::verify_operator(&proof, key.as_ref())
        .map_err(|e| ApiError::new(403, "BAD_OPERATOR_SIGNATURE", e))?;
    Ok(proof)
}

/// What an operator decision that names nothing in its body (lifting a
/// deadline, removing a node, revoking a deployer, granting deployments) may
/// carry.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OpsProofBody {
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
}

/// [`OpsProofBody`] from a body that may be empty (a `DELETE`).
fn ops_proof_body(body: &[u8]) -> Result<OpsProofBody, ApiError> {
    if body.is_empty() {
        return Ok(OpsProofBody::default());
    }
    serde_json::from_slice(body).map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))
}

/// An RFC 3339 time in the API as Unix seconds, as the log keeps it.
fn unix_secs(t: Option<&str>, field: &str) -> Result<Option<u64>, ApiError> {
    t.map(|t| {
        chrono::DateTime::parse_from_rfc3339(t)
            .ok()
            .and_then(|t| u64::try_from(t.timestamp()).ok())
            .ok_or_else(|| {
                ApiError::bad_request(
                    "BAD_REQUEST",
                    &format!("`{field}` is an RFC 3339 time after 1970"),
                )
            })
    })
    .transpose()
}

// ---------------------------------------------------------------- deployers

// ---------------------------------------------------------------- sign-up

// ---------------------------------------------------------------- deployments

fn malformed(why: &'static str) -> ApiError {
    ApiError::bad_request("BAD_REQUEST", why)
}

/// The node's router: every route the sealed envelope may reach.
pub fn router(node: Arc<Node>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/attestation", get(attestation))
        .route("/auth/account-proof", post(account_proof::prove))
        .route(
            "/package-origins/{package}",
            get(origins::list).post(origins::add).delete(origins::remove),
        )
        .route("/consent/requests", post(crate::consent_page::create))
        .route("/consent/{request}/result", get(crate::consent_page::result))
        .route(
            "/internal/consent/result",
            post(crate::consent_page::internal_result),
        )
        .route("/ops/served-origins", get(origins::ops_served))
        .route("/ops/nodes/{id}/receipt-key", get(account_proof::ops_receipt_key))
        .route("/ops/apps/{app}/restore", post(restore::ops_restore))
        .merge(auth::routes())
        .merge(recovery::routes())
        .merge(packages::routes())
        .merge(apps::routes())
        .merge(calls::routes())
        .merge(files::routes())
        .merge(upgrades::routes())
        .merge(orgs::routes())
        .merge(approvals::routes())
        .merge(deployments::routes())
        .merge(sign_up::routes())
        .merge(internal::routes())
        .merge(ops::routes())
        // Counted, timed and sized per route template (`metrics::track`);
        // inside the sealed envelope, so it sees what each handler answered.
        .route_layer(axum::middleware::from_fn(crate::metrics::track))
        // `Deprecation` on the routes `api-deprecations.toml` deprecates (docs: compatibility).
        .route_layer(axum::middleware::from_fn(deprecation::mark))
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
    use std::collections::HashMap;

    use super::{
        admit_pending, typed_id, Pending, MAX_CHALLENGES_PER_ACCOUNT, MAX_PENDING_CHALLENGES,
    };

    fn sign_up(expires: u64) -> Pending {
        Pending::Register {
            challenge: [0; 32],
            name: String::new(),
            expires,
        }
    }

    fn step_up(account: &str, expires: u64) -> Pending {
        Pending::Confirm {
            challenge: [0; 32],
            app: "app".to_owned(),
            method: "m".to_owned(),
            args: Vec::new(),
            nonce: String::new(),
            account: account.to_owned(),
            expires,
            carry: None,
        }
    }

    fn put(c: &mut HashMap<String, Pending>, p: Pending, now: u64) -> Result<(), u16> {
        admit_pending(c, &p, now).map_err(|e| e.status)?;
        let _prev = c.insert(format!("1.{}", c.len()), p);
        Ok(())
    }

    /// Anonymous ceremonies have their own pool: filling it refuses the next
    /// sign-in, but an account's step-up and passkey add still begin.
    #[test]
    fn a_full_anonymous_pool_does_not_block_an_accounts_step_up() {
        let mut c = HashMap::new();
        for _ in 0..MAX_PENDING_CHALLENGES {
            assert_eq!(put(&mut c, sign_up(100), 0), Ok(()));
        }
        assert_eq!(put(&mut c, sign_up(100), 0), Err(429));
        assert_eq!(put(&mut c, step_up("alice", 100), 0), Ok(()));
        // Expired ones free the pool.
        assert_eq!(put(&mut c, sign_up(200), 100), Ok(()));
    }

    /// One account's step-ups are capped, and the cap is its own.
    #[test]
    fn one_account_cannot_fill_the_accounts_pool() {
        let mut c = HashMap::new();
        for _ in 0..MAX_CHALLENGES_PER_ACCOUNT {
            assert_eq!(put(&mut c, step_up("mallory", 100), 0), Ok(()));
        }
        assert_eq!(put(&mut c, step_up("mallory", 100), 0), Err(429));
        assert_eq!(put(&mut c, step_up("alice", 100), 0), Ok(()));
        assert_eq!(put(&mut c, sign_up(100), 0), Ok(()));
    }

    #[test]
    fn a_typed_id_never_starts_with_a_dash() {
        for _ in 0..10_000 {
            let id = typed_id::<16>();
            assert!(!id.starts_with('-'), "{id}");
            assert_eq!(id.len(), 22);
        }
    }

    /// The warning for a Raft message without an addressee is given once, then
    /// not again within `NO_TARGET_WARN_EVERY`, however many arrive at once.
    #[test]
    fn the_no_target_warning_is_given_at_most_once_a_minute() {
        let due: usize = std::thread::scope(|s| {
            let threads: Vec<_> = (0..16)
                .map(|_| {
                    s.spawn(|| {
                        (0..1_000)
                            .filter(|_| super::no_target_warning_due())
                            .count()
                    })
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap_or(0)).sum()
        });
        assert_eq!(due, 1);
    }
}
