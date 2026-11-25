//! The node's HTTP API. Clients reach it inside the sealed channel; only
//! `/attestation` and `/health` are served in the clear.
//!
//! - Client: `/auth/*`, `/bundles`, `/apps`, `/apps/{app}/{intents,views,methods/{method},openapi.json,events,upgrade,access}`,
//!   `/orgs/*`, `/invites/{token}/redeem`, `/consent/requests`, `/consent/{id}/result`.
//! - The consent page (`consent_page`) is served in the clear, outside this
//!   router, on the node's own origin.
//! - Internal (peers, cluster secret): `/internal/*`.
//! - Ops (mdma, operator token): `/ops/*`.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

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
use calimero_sealed::SealedSession;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::app::{AppMachine, Status};
use crate::attest::{kms_report_data, report_data, AttestationDoc};
use crate::error::ApiError;
use crate::node::blob_guards::MAX_LEASE_IDS;
use crate::node::{CallRequest, Issuer, JobAttestError, Node, ViewRequest};
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
/// session then writes once per this, not once per `SESSION_TTL`.
pub const REFRESH_ROTATE_AFTER: Duration = Duration::from_secs(60 * 60);
const CHALLENGE_TTL: u64 = 5 * 60;
/// The code of a create or upgrade by bundle hash whose releases ship different compute components.
const AMBIGUOUS_COMPUTE: &str = "AMBIGUOUS_COMPUTE";

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

mod account_proof;
mod approvals;
pub(crate) mod apps;
mod auth;
mod backup_keys;
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
mod public_links;
mod recovery;
mod restore;
mod sign_up;
mod undo;
mod update_policies;
pub(crate) mod upgrades;

pub(crate) use approvals::{
    refuse_egress_off, set_egress, set_outbound_approval, set_release_approval, EgressBody,
    OutboundApprovalBody, ReleaseApprovalBody,
};
// `POST /apps/{app}/egress`, routed beside its `GET` in `upgrades::routes`.
use approvals::app_set_egress;
pub(crate) use exports::ExportLimits;
pub(crate) use internal::peer_only;
pub(crate) use orgs::{
    create_invite, create_org, delete_org, redeem_invite, remove_member, remove_member_teams,
    rename_org, revoke_invite, set_all_apps, set_capabilities, set_member_role, set_member_teams,
    CapabilitiesBody, InviteBody,
};
pub use pipeline_events::MAX_RUN_SUBSCRIBERS_PER_APP;
pub(crate) use public_links::VisitorLimits;
pub(crate) use update_policies::{approve_upgrade, approver_of, set_upgrade_policy};
pub(crate) use upgrades::{
    move_class, owner_approves, refuse_stalled, unclassified_gate, unit_plan, update_view,
    upgrade_app, upgrade_refusal, upgrade_unit, UpgradeBody,
};
// The request bodies the OpenAPI registry names as `super::<Body>`.
#[cfg(test)]
use {
    approvals::{OpsDeadlineBody, OpsOutboundBody, OpsReleaseBody},
    apps::{
        AccessBody, AppAttestQuery, AppInviteBody, AppListQuery, CreateAppBody, MoveBody,
        TransferBody,
    },
    auth::{
        ActivityQuery, AddPasskeyFinish, AliasBody, ApiKeyBody, AssertionBody, LoginBegin,
        PasskeyLabelBody, RefreshBody, RegisterBegin, RegisterFinish,
    },
    backup_keys::{BackupKeyBody, OpsBackupKeyBody},
    calls::{EventsQuery, IntentBody, MethodBody, PresenceBody, RunsQuery, ViewBody},
    deployments::{
        ClusterOwnerBody, DeployerBody, DeploymentBody, DeploymentSecretBody, OfferBody,
    },
    files::{BeginUpload, BlobGrant, BlobUploadQuery},
    ops::{BackupQuery, NodeBody, PoolsBody, PricesBody, ReplicaBody},
    orgs::{
        AllAppsBody, MemberBody, MembersQuery, OrgBody, OrgSettingsBody, RenameOrgBody,
        ScopeAppsBody, TeamBody, TeamDeleteBody, TeamIdsBody, TeamMemberBody, TeamRowsBody,
        TeamUpdateBody,
    },
    packages::{InstallBody, PublisherBody},
    public_links::PublicLinkBody,
    recovery::{RecoveryBegin, RecoveryFinish, RecoveryKeyBody, RecoveryStart, RecoveryStartBody},
    sign_up::{OpsSignUpInviteBody, SignUpBody, SignUpInviteBody},
    update_policies::{UpdatesQuery, UpgradeApprovalBody, UpgradePolicyBody},
    upgrades::{OpsUpgradeBody, OrgUpgradeBody, PublisherUpgradeBody},
};

/// The area modules `router` merges (`.merge(<area>::routes())`) and their
/// sources, for the tests that read the route table from source (`openapi`,
/// `metrics`).
#[cfg(test)]
pub(crate) const AREA_SOURCES: &[(&str, &str)] = &[
    ("approvals", include_str!("api/approvals.rs")),
    ("backup_keys", include_str!("api/backup_keys.rs")),
    ("apps", include_str!("api/apps.rs")),
    ("auth", include_str!("api/auth.rs")),
    ("calls", include_str!("api/calls.rs")),
    ("deployments", include_str!("api/deployments.rs")),
    ("files", include_str!("api/files.rs")),
    ("internal", include_str!("api/internal.rs")),
    ("ops", include_str!("api/ops.rs")),
    ("orgs", include_str!("api/orgs.rs")),
    ("packages", include_str!("api/packages.rs")),
    ("public_links", include_str!("api/public_links.rs")),
    ("recovery", include_str!("api/recovery.rs")),
    ("sign_up", include_str!("api/sign_up.rs")),
    ("undo", include_str!("api/undo.rs")),
    ("update_policies", include_str!("api/update_policies.rs")),
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
        /// step-up whose arguments the node made (`consent_minted`).
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
/// `CHALLENGE_TTL`; past this a new one displaces the oldest of the caller
/// holding the most (`admit_pending`), so the pool never grows without bound
/// (Xil's review, issue 12) and a flood never locks the others out (review
/// finding D1). Ceremonies of known accounts do not count here, so a full
/// anonymous pool never blocks an account's step-up.
pub const MAX_PENDING_CHALLENGES: usize = 10_000;
/// The most anonymous ceremonies one sealed session holds open; its next
/// begin displaces its own oldest. A client has a few at once (a sign-in, a
/// sign-up, a recovery), so a session that begins more only loses its own.
pub const MAX_CEREMONIES_PER_SESSION: usize = 16;
/// The most ceremonies a node holds open for known accounts, all together.
pub const MAX_ACCOUNT_CHALLENGES: usize = 10_000;
/// The most ceremonies a node holds open for one account, so no account
/// fills the pool the others share.
pub const MAX_CHALLENGES_PER_ACCOUNT: usize = 64;

/// A ceremony this node holds open, with the sealed session that began it
/// (`None` on a plain request, which only a `--dev` node serves) and its place
/// in the order ceremonies were begun, oldest first.
#[derive(Debug)]
struct Held {
    pending: Pending,
    session: Option<SealedSession>,
    order: u64,
}

/// Where a session that holds anonymous ceremonies ranks for displacement:
/// how many it holds, then its oldest ceremony's order, reversed, so the
/// greatest is the biggest holder and, among equals, the one with the oldest.
type Rank = (usize, Reverse<u64>, Option<SealedSession>);

/// The ceremonies a node holds open ([`Pending`]), indexed so a begin, a take,
/// an expiry and the displacement a full budget calls for each cost
/// O(log n): a flood fills the pool, so nothing here scans it.
#[derive(Debug, Default)]
pub struct Ceremonies {
    held: HashMap<String, Held>,
    /// Every ceremony by expiry, then order: what [`Self::expire`] drops.
    expiry: BTreeMap<(u64, u64), String>,
    /// The anonymous ceremonies of each session, by order (oldest first).
    sessions: HashMap<Option<SealedSession>, BTreeMap<u64, String>>,
    /// Each session in `sessions`, ranked ([`Rank`]).
    ranking: BTreeSet<Rank>,
    anonymous: usize,
    /// How many ceremonies each known account holds, and all of them.
    accounts: HashMap<String, usize>,
    account_total: usize,
    next: u64,
}

impl Ceremonies {
    /// The ceremony `id` names, if this node holds it.
    pub fn get(&self, id: &str) -> Option<&Pending> {
        self.held.get(id).map(|h| &h.pending)
    }

    fn insert(&mut self, id: String, pending: Pending, session: Option<SealedSession>) {
        let order = self.next;
        self.next += 1;
        let _prev = self.expiry.insert((pending.expires(), order), id.clone());
        match pending.account() {
            None => {
                self.unrank(session);
                let _prev = self
                    .sessions
                    .entry(session)
                    .or_default()
                    .insert(order, id.clone());
                self.anonymous += 1;
                self.rank(session);
            }
            Some(account) => {
                *self.accounts.entry(account.to_owned()).or_default() += 1;
                self.account_total += 1;
            }
        }
        let _prev = self.held.insert(
            id,
            Held {
                pending,
                session,
                order,
            },
        );
    }

    /// Take the ceremony `id` names out of every index.
    fn remove(&mut self, id: &str) -> Option<Pending> {
        let h = self.held.remove(id)?;
        let _expiry = self.expiry.remove(&(h.pending.expires(), h.order));
        match h.pending.account() {
            None => {
                self.unrank(h.session);
                if let Some(mine) = self.sessions.get_mut(&h.session) {
                    let _id = mine.remove(&h.order);
                    if mine.is_empty() {
                        let _gone = self.sessions.remove(&h.session);
                    }
                }
                self.anonymous = self.anonymous.saturating_sub(1);
                self.rank(h.session);
            }
            Some(account) => {
                if let Some(n) = self.accounts.get_mut(account) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        let _gone = self.accounts.remove(account);
                    }
                }
                self.account_total = self.account_total.saturating_sub(1);
            }
        }
        Some(h.pending)
    }

    fn rank_of(&self, session: Option<SealedSession>) -> Option<Rank> {
        let mine = self.sessions.get(&session)?;
        let (&oldest, _) = mine.first_key_value()?;
        Some((mine.len(), Reverse(oldest), session))
    }

    fn unrank(&mut self, session: Option<SealedSession>) {
        if let Some(rank) = self.rank_of(session) {
            let _was = self.ranking.remove(&rank);
        }
    }

    fn rank(&mut self, session: Option<SealedSession>) {
        if let Some(rank) = self.rank_of(session) {
            let _new = self.ranking.insert(rank);
        }
    }

    /// The oldest anonymous ceremony `session` holds.
    fn oldest_of(&self, session: Option<SealedSession>) -> Option<String> {
        self.sessions
            .get(&session)
            .and_then(BTreeMap::first_key_value)
            .map(|(_, id)| id.clone())
    }

    /// Drop the ceremonies expired at `now`.
    fn expire(&mut self, now: u64) {
        while let Some(id) = self
            .expiry
            .first_key_value()
            .filter(|((expires, _), _)| *expires <= now)
            .map(|(_, id)| id.clone())
        {
            let _expired = self.remove(&id);
        }
    }
}

/// Hold `pending` and answer its id. `session` is the sealed session an
/// anonymous ceremony was begun in, which its limit is keyed on; a known
/// account's ceremony draws on that account's budget instead, and passes `None`.
fn put_pending(
    node: &Node,
    pending: Pending,
    session: Option<SealedSession>,
) -> Result<String, ApiError> {
    let id = format!("{}.{}", node.config.node_id, b64url(&random::<16>()));
    let mut c = node
        .challenges
        .lock()
        .map_err(|_| ApiError::internal("challenge store poisoned"))?;
    admit_pending(&mut c, &pending, session, now_secs())?;
    c.insert(id.clone(), pending, session);
    Ok(id)
}

/// Drop expired ceremonies, then make room for `pending` in its budget.
///
/// An anonymous ceremony is never refused: past its session's
/// `MAX_CEREMONIES_PER_SESSION` it displaces that session's oldest, and in a
/// full pool the oldest of the session holding the most. A flood from one
/// session only replaces its own; one spread over many sessions, each opened
/// by a handshake the sealed layer rate-limits, displaces the oldest first, so
/// another client's ceremony lives at least as long as the flood takes to
/// begin `MAX_PENDING_CHALLENGES` after it. The peer's IP is no key: the node
/// serves plain HTTP behind whatever TLS front the operator runs, where every
/// client has the front's address, and `X-Forwarded-For` is written outside
/// the TD by whoever relays the request.
///
/// A known account's ceremony is refused `429` when its share of the
/// accounts' pool, or that pool, is full.
fn admit_pending(
    c: &mut Ceremonies,
    pending: &Pending,
    session: Option<SealedSession>,
    now: u64,
) -> Result<(), ApiError> {
    c.expire(now);
    let Some(account) = pending.account() else {
        let mine = c.sessions.get(&session).map_or(0, BTreeMap::len);
        // A plain request's (`None`, a `--dev` node only) count together and
        // have no share of their own.
        let victim = if session.is_some() && mine >= MAX_CEREMONIES_PER_SESSION {
            c.oldest_of(session)
        } else if c.anonymous >= MAX_PENDING_CHALLENGES {
            c.ranking
                .last()
                .and_then(|&(_, _, biggest)| c.oldest_of(biggest))
        } else {
            None
        };
        if let Some(id) = victim {
            let _displaced = c.remove(&id);
        }
        return Ok(());
    };
    let mine = c.accounts.get(account).copied().unwrap_or(0);
    if mine >= MAX_CHALLENGES_PER_ACCOUNT || c.account_total >= MAX_ACCOUNT_CHALLENGES {
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
/// ceremony is then checked against that origin). An origin this node does not
/// know is looked up again on fresh state: a package installed or an origin
/// registered through another node a moment ago is not in this node's copy of
/// the directory until it applies that entry.
async fn claimed_app(node: &Node, client_data_json: &str) -> Option<AppOrigin> {
    let origin = calimero_identity::claimed_origin(client_data_json)?;
    if let Some(app) = app_origin(node, &origin) {
        return Some(app);
    }
    // The cluster's own origins are no app's: nothing to look up again.
    if node
        .config
        .origins
        .iter()
        .any(|o| o.trim_end_matches('/') == origin)
        || node.api_origins().contains(&origin)
    {
        return None;
    }
    // As `begin_rp_id` does for the begin.
    let _fresh = node.sys_fresh().await;
    app_origin(node, &origin)
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

    /// Whether `gate`, which admitted this principal to `app_id` with method `*`, still
    /// would on `s`, the facts that can change since judged again: the session not ended
    /// and its passkey, origin and issuing release still holding; the API key not revoked
    /// or expired; the account still reaching the app. What cannot change (a signature, the
    /// key's own scope) is not. A visitor opens no stream (`*` refuses it). A session token's
    /// own expiry does not end a stream: it is a refresh, not a revocation. For a stream,
    /// through [`still_admitted`].
    fn admitted_on(&self, s: &SystemState, app_id: &str, now: u64) -> bool {
        let Some(app) = s.apps.get(app_id) else {
            return false;
        };
        let credential = match (&self.session, &self.api_key) {
            (Some(c), _) => {
                !s.revoked_sessions.contains(&c.session)
                    && c.passkey
                        .as_ref()
                        .is_none_or(|k| s.credentials.get(k) == Some(&c.account))
                    && c.origin.as_ref().is_none_or(|o| {
                        s.package_origins
                            .get(o)
                            .is_some_and(|r| Some(r.package.as_str()) == c.app.as_deref())
                    })
                    && self
                        .issuer_release
                        .as_ref()
                        .is_none_or(|r| s.release_may_hold(app, Some(r)))
            }
            (None, Some(k)) => {
                k.expires_at > now
                    && s.api_keys
                        .values()
                        .any(|r| r.key_id == k.key_id && r.account == k.account)
            }
            (None, None) => !matches!(self.caller, Caller::Visitor { .. }),
        };
        credential && s.may_reach(app, self.caller.account())
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

    /// Whether this principal may change `team` of `org` as `reach` says: a
    /// package-bound session only a team of its own package's, the org's
    /// app team for that package ([`crate::types::Org::package_of_team`])
    /// or one below it, so an app's frontend runs its own channels (#5014).
    /// Who may do the change itself is judged where it applies, as for any
    /// session; this only keeps the session to its package's teams.
    pub(crate) fn within_team(
        &self,
        node: &Node,
        org: &str,
        team: &str,
        reach: OwnTeam,
    ) -> Result<(), ApiError> {
        let Some(pkg) = self.bound_app() else {
            return Ok(());
        };
        let refused = || {
            ApiError::forbidden(
                "a session opened on an app's frontend changes only that app's own teams; do this on the cluster's own page",
            )
        };
        let (own, app_team, apps) = node.sys(|s| {
            let Some((package, app_team)) = s
                .orgs
                .get(org)
                .and_then(|o| o.package_of_team(team))
                .map(|(p, top)| (p == pkg, top))
            else {
                return (false, false, Vec::new());
            };
            let apps = match reach {
                OwnTeam::BelowHoldingOwnApps => s
                    .team_apps(org, team)
                    .iter()
                    .filter_map(|a| s.apps.get(a).cloned())
                    .collect(),
                OwnTeam::AppTeamOrBelow | OwnTeam::Below => Vec::new(),
            };
            (package, app_team, apps)
        });
        if !own || (app_team && reach != OwnTeam::AppTeamOrBelow) {
            return Err(refused());
        }
        // A delete takes the apps below the team: only its own package's.
        for app in &apps {
            self.within(node, app).map_err(|_| refused())?;
        }
        Ok(())
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
                    "this session was signed by a node whose release is not approved for this app; sign in again",
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

/// Which of its own package's teams a package-bound session may change
/// ([`Principal::within_team`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnTeam {
    /// The package's app team or a team below it: its rows (members), a
    /// parent for a new channel, an app's team.
    AppTeamOrBelow,
    /// A team below the app team, never the app team itself, whose name,
    /// mark, place and openness are the organization's admins'.
    Below,
    /// A team below the app team whose subtree holds only the package's own
    /// apps: deleting it takes them.
    BelowHoldingOwnApps,
}

/// The organization step-ups a package-bound session may make, each over
/// the team its arguments name (`team`), as [`Principal::within_team`]
/// judges it. The routes judge it first; [`step_up`] judges it again, so no
/// other organization consent is ever asked of such a session.
const BOUND_TEAM_STEP_UPS: [(&str, OwnTeam); 3] = [
    ("__set_team_member", OwnTeam::AppTeamOrBelow),
    ("__remove_team_member", OwnTeam::AppTeamOrBelow),
    ("__delete_team", OwnTeam::BelowHoldingOwnApps),
];

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
    // A visitor's credential opens its instance's public methods (`gate`) and nothing else.
    if p.as_ref()
        .is_some_and(|p| matches!(p.caller, Caller::Visitor { .. }))
    {
        return Err(visitor_only());
    }
    Ok(p)
}

/// What a visitor's credential gets on any route but its instance's calls and views.
fn visitor_only() -> ApiError {
    ApiError::new(
        403,
        public_links::VISITOR_ONLY,
        "a visitor through a public link calls only the instance's public methods",
    )
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
///
/// A credential is judged on the cluster's latest system state (a read index,
/// [`Node::sys_fresh_index`]): a session or key revoked through another node is
/// refused here at once, not once this replica catches up. What needs no state
/// is checked before that round, so a credential that cannot be valid costs the
/// system leader nothing (D4): an API key's shape ([`api_key_shaped`]) and the
/// keys a fresh read found missing ([`UnknownApiKeys`]), a visitor's signature,
/// a session token's signature under its issuer's attested key and its expiry.
async fn credential(node: &Node, headers: &HeaderMap) -> Result<Option<Principal>, ApiError> {
    let Some(token) = bearer(headers) else {
        return Ok(None);
    };
    if let Some(rest) = token.strip_prefix(public_links::VISITOR_PREFIX) {
        return public_links::visitor(node, rest).await.map(Some);
    }
    if token.starts_with("cal_") {
        return api_key(node, token).await.map(Some);
    }
    let (claims, issuer) = signed_session(node, token).await?;
    let (sys_index, _caught_up) = node.sys_fresh_index().await;
    let issuer_release = session_issuer_serves(node, &claims, issuer)?;
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

/// An API key's length: `cal_` and 32 random bytes in unpadded base64url
/// (`calimero_identity::new_api_key`).
const API_KEY_CHARS: usize = 4 + 43;

/// Whether `token` has the shape of an API key this cluster makes; no other is
/// looked up.
fn api_key_shaped(token: &str) -> bool {
    token.len() == API_KEY_CHARS
        && token.strip_prefix("cal_").is_some_and(|k| {
            k.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// Most API key hashes [`UnknownApiKeys`] holds at once.
pub(crate) const MAX_UNKNOWN_API_KEYS: usize = 4096;

/// How long [`UnknownApiKeys`] remembers a missing key.
pub(crate) const UNKNOWN_API_KEY_TTL: Duration = Duration::from_secs(60);

/// The hashes of API keys a caught-up read found no key for, each kept
/// `UNKNOWN_API_KEY_TTL`, at most `MAX_UNKNOWN_API_KEYS` (past that a
/// missing key is not remembered, and costs a round each time). A key it holds
/// is refused with no system read. It never refuses a key that exists: a hash is
/// added only after a read caught up to an index past every write committed
/// before the request, a key is shown to its maker only once its creation is
/// committed, and a new key is 256 random bits, so none is presented before it
/// exists; a deleted key's hash does not come back. Per node, in memory.
#[derive(Default)]
pub struct UnknownApiKeys(Mutex<HashMap<String, Instant>>);

impl UnknownApiKeys {
    fn holds(&self, hash: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(hash)
            .is_some_and(|at| at.elapsed() < UNKNOWN_API_KEY_TTL)
    }

    fn add(&self, hash: String) {
        let mut keys = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if keys.len() >= MAX_UNKNOWN_API_KEYS {
            keys.retain(|_, at| at.elapsed() < UNKNOWN_API_KEY_TTL);
            if keys.len() >= MAX_UNKNOWN_API_KEYS {
                return;
            }
        }
        let _prev = keys.insert(hash, Instant::now());
    }
}

/// The caller an API key names. A token of another shape, or one a fresh read
/// found missing lately, is refused before the read-index round; a missing key
/// is waited for ([`Node::sys_until`]) only when the round did not catch this
/// replica up, since a caught-up state already holds every key made before the
/// request.
async fn api_key(node: &Node, token: &str) -> Result<Principal, ApiError> {
    let unknown = || ApiError::unauthenticated("unknown API key");
    if !api_key_shaped(token) {
        return Err(unknown());
    }
    let hash = calimero_identity::api_key_hash(token);
    if node.unknown_api_keys.holds(&hash) {
        return Err(unknown());
    }
    let (sys_index, caught_up) = node.sys_fresh_index().await;
    let find = |s: &SystemState| s.api_keys.get(&hash).cloned();
    let found = if caught_up {
        node.sys(find)
    } else {
        node.sys_until(find).await
    };
    let Some(rec) = found else {
        if caught_up {
            node.unknown_api_keys.add(hash);
        }
        return Err(unknown());
    };
    if rec.expires_at <= now_secs() {
        return Err(ApiError::unauthenticated("API key expired"));
    }
    Ok(Principal {
        caller: Caller::ApiKey {
            key: rec.key_id.clone(),
            account: rec.account.clone(),
        },
        session: None,
        api_key: Some(rec),
        issuer_release: None,
        consent_page: false,
        sys_index,
    })
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

/// The issuer a session token names, as this node attested it, under whose
/// receipt key the token is signed and unexpired: what needs no fresh system
/// state, so a token its issuer did not sign is refused before the read-index
/// round. [`session_issuer_serves`] judges the issuer on the fresh state after.
async fn signed_session(node: &Node, token: &str) -> Result<(SessionClaims, Issuer), ApiError> {
    let claims = peek_session(token).map_err(|_| ApiError::unauthenticated("malformed token"))?;
    let issuer_id = claims.node;
    // Answered from the key this node attested ahead of time: the issuer may
    // be stopped or frozen by now.
    let mut issuer = node
        .session_issuer(issuer_id, false)
        .await
        .ok_or_else(unknown_issuer)?;
    let named = calimero_identity::from_b64url(&claims.key).ok();
    if named.as_deref() != Some(issuer.key.as_slice()) {
        // Signed under another key than the one attested here: the issuer's
        // data directory may have been replaced since (a restart keeps it).
        issuer = node
            .session_issuer(issuer_id, true)
            .await
            .ok_or_else(unknown_issuer)?;
    }
    let claims = verify_session(&issuer.key, token, now_secs())
        .map_err(|e| ApiError::unauthenticated(&e.to_string()))?;
    Ok((claims, issuer))
}

fn unknown_issuer() -> ApiError {
    ApiError::unauthenticated(
        "the token's issuing node is not a node of this cluster, or cannot be attested; refresh the session",
    )
}

/// Judged on fresh system state, after [`signed_session`]: the token's issuer
/// is still a node of this cluster, and its attested release may serve the
/// account (`SystemState::release_may_serve_account`). Answers the issuer's
/// release, which each app access checks too (`Principal::within`).
fn session_issuer_serves(
    node: &Node,
    claims: &SessionClaims,
    issuer: Issuer,
) -> Result<Option<String>, ApiError> {
    let issuer_id = claims.node;
    // A node removed before the request arrived: its sessions stop verifying.
    if issuer_id != node.config.node_id && !node.is_peer(issuer_id) {
        return Err(unknown_issuer());
    }
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
    Ok(Some(release))
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
        "clusterVersion": node.advertises(),
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
            // In the clear and without a credential, so judged as an anonymous
            // `GET /apps/{app}` is (#4723): on the cluster's latest system state,
            // and an app nobody anonymous may reach answers as one that does not
            // exist, so the bound hash never says that a private app exists or
            // which release it runs. Read at once after the round: an unknown id
            // does not wait for the directory to name it.
            let _fresh = node.sys_fresh().await;
            let rec = node
                .sys(|s| s.apps.get(app).filter(|a| s.may_reach(a, None)).cloned())
                .ok_or_else(|| ApiError::not_found("no such app"))?;
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
    // Off the async workers, one at a time, and only now: after the `app`
    // lookup, which may wait, so a request that waits there holds no slot.
    let q = node.quoter.quote(report).await?;
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

/// A consent as the JSON a system command logs: an error, never `null`, which would be
/// logged and only refused where the command applies.
pub(crate) fn consent_value(consent: &crate::proof::Consent) -> Result<Value, ApiError> {
    serde_json::to_value(consent).map_err(|e| ApiError::internal(&format!("the consent: {e}")))
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
        .await
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
                assertion: serde_json::to_value(assertion)
                    .map_err(|e| ApiError::internal(&format!("the assertion: {e}")))?,
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
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()?
        .format(time::macros::format_description!("[year]-[month]-[day]"))
        .ok()
}

// ---------------------------------------------------------------- account recovery

// [Account recovery](https://calimero-network.github.io/core/protocol/identities/#account-recovery): a code made with a passkey; signed out,
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

/// Whether `account` holds a read-only row in the team `app` belongs to
/// (checked again against the latest system state before saying yes: at or
/// past `sys_index`, the index the request's round named, else a round's).
/// The record is read again from that state (`fresh_record`): the one in hand
/// can predate a change of its access or owner.
pub async fn read_only_in(
    node: &Node,
    app: &crate::types::AppRecord,
    account: Option<&str>,
    sys_index: Option<u64>,
) -> bool {
    let Some(a) = account else {
        return false;
    };
    let check = || {
        node.sys(|s| {
            let Some(app) = fresh_record(s, app) else {
                return false;
            };
            // Only through teams: a name, or the whole organization, writes.
            let teams = app.access.teams();
            if teams.is_empty() || a == app.owner || app.access.org_wide() || app.access.names(a) {
                return false;
            }
            s.orgs
                .get(&app.org)
                .is_some_and(|o| teams.iter().all(|t| !o.writes_team(a, t)))
        })
    };
    check() && {
        node.sys_fresh_from(sys_index).await;
        check()
    }
}

/// `app`'s record as `s` holds it. A handler reads the record (`Node::app`) before
/// `gate` makes the request's read-index round, so on a follower behind, the one in
/// hand can predate an owner, organization or access change committed through another
/// node; every check made after `gate` reads it again here. `None`: deleted since.
/// Regression: `a_former_owner_does_not_manage_the_app_on_a_follower_behind_the_transfer`.
fn fresh_record<'s>(
    s: &'s SystemState,
    app: &crate::types::AppRecord,
) -> Option<&'s crate::types::AppRecord> {
    s.apps.get(&app.app_id)
}

/// Whether `account` may reach `app`, judged on the app's record as applied
/// now. Handlers read the record before `principal` makes its fresh system
/// read, so the record in hand can predate a grant or revocation committed
/// through another node; this reads it again.
fn reaches(node: &Node, app: &crate::types::AppRecord, account: Option<&str>) -> bool {
    node.sys(|s| fresh_record(s, app).is_some_and(|now| s.may_reach(now, account)))
}

async fn gate(
    node: &Node,
    headers: &HeaderMap,
    app: &crate::types::AppRecord,
    method: &str,
    write: bool,
) -> Result<Principal, ApiError> {
    let p = credential(node, headers).await?;
    if p.as_ref().is_some_and(Principal::upgrade_key) {
        return Err(upgrade_key_only());
    }
    // A visitor through a public link: its instance's methods only, whatever its access; the
    // ABI's `public` flag is judged where the call runs.
    if let Some(v) = p
        .as_ref()
        .filter(|p| matches!(p.caller, Caller::Visitor { .. }))
    {
        public_links::visitor_gate(node, v, app, method).await?;
        return Ok(v.clone());
    }
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
    // A package-bound session steps up over its package's apps and teams
    // only: never for the account, the rest of an organization, the
    // cluster or another package.
    if p.bound_app().is_some() {
        match node.sys(|s| s.apps.get(app_id).cloned()) {
            Some(app) => p.within(node, &app)?,
            None => {
                let own_team = app_id
                    .strip_prefix("org:")
                    .zip(BOUND_TEAM_STEP_UPS.iter().find(|(m, _)| *m == method))
                    .zip(args.get("team").and_then(Value::as_str));
                match own_team {
                    Some(((org, (_, reach)), team)) => p.within_team(node, org, team, *reach)?,
                    None => p.unbound()?,
                }
            }
        }
    }
    let account = p.account()?;
    // `Value`'s `Display` is its compact JSON and cannot fail.
    let args_bytes = args.to_string().into_bytes();
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
            None,
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
    let signed = String::from_utf8(a)
        .map_err(|_| ApiError::internal("the signed arguments are not UTF-8"))?;
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
        consent: consent_value(&consent)?,
        args,
        carry,
    })
}

/// For a stream `gate` let open: whether `p` would still be admitted to `app_id`
/// (`Principal::admitted_on`), judged on fresh system state: a read-index round begun
/// now, applied here ([`Node::sys_caught_up`]), so it holds every system write committed
/// before what the stream is about to send, a removal or revocation committed through
/// another node included. False ends the stream, as does a replica that cannot catch up in
/// `Config::read_wait`; the subscriber opens it again through `gate`. Every stream judges
/// each batch it sends with it (`local_events` with the same round, the presence stream,
/// a pipeline run's events, an event stream proxied from another replica), never only
/// when it opened. Regressions: `a_removed_member_stops_receiving_presence`,
/// `a_revoked_key_or_session_stops_its_event_and_presence_streams`,
/// `a_removed_member_stops_receiving_a_runs_events`.
pub(crate) async fn still_admitted(node: &Node, p: &Principal, app_id: &str) -> bool {
    node.sys_caught_up().await && node.sys(|s| p.admitted_on(s, app_id, now_secs()))
}

/// How many events that reached this replica together one freshness round
/// ([`Node::sys_caught_up`]) in `local_events` covers.
pub const MAX_EVENT_BATCH: usize = 256;

/// The app's events from `from`, then live. A component's event carries its
/// audience (R9) and goes to `subscriber` (`None`: anonymous) only if the system
/// state says it still reaches the app and is in the audience, so a member removed
/// from the org or the team stops receiving at once. That state is fresh for each
/// event: a read-index round begun after the event reached this replica, applied
/// here ([`Node::sys_caught_up`]), so it holds every system write committed before
/// the event's entry was, a removal committed on other nodes included, though this
/// replica's system log lags its app log (the two are different logs). One round
/// covers the backlog, and one each batch of live events that arrived together (at
/// most [`MAX_EVENT_BATCH`]). When this replica cannot catch up in
/// `Config::read_wait`, the stream ends: the subscriber resumes from the last id it
/// got, judged afresh. With `principal` (a stream opened here, not one a peer asks for:
/// that peer judges its own), the same state judges the credential that opened it
/// ([`still_admitted`]), so a revoked API key or an ended session ends it. An event of an
/// app on wasmer has no audience and goes out as before. Regressions:
/// `an_event_after_a_removal_never_reaches_the_removed_member_on_a_lagging_replica`,
/// `a_revoked_key_or_session_stops_its_event_and_presence_streams`.
fn local_events(
    node: &Arc<Node>,
    app_id: &str,
    from: u64,
    subscriber: Option<String>,
    principal: Option<Principal>,
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
    let batches = futures_util::stream::once(std::future::ready(backlog))
        .chain(live.ready_chunks(MAX_EVENT_BATCH));
    let (node, app) = (Arc::clone(node), app_id.to_owned());
    let judged = batches.then(move |batch| {
        let (node, app, subscriber) = (Arc::clone(&node), app.clone(), subscriber.clone());
        let principal = principal.clone();
        async move {
            if principal.is_none() && batch.iter().all(|e| e.audience.is_none()) {
                return Some(batch);
            }
            // Every event of the batch is here already: a round begun now names an
            // index at or past every system write committed before any of them.
            if !node.sys_caught_up().await {
                return None;
            }
            node.sys(|s| {
                if principal
                    .as_ref()
                    .is_some_and(|p| !p.admitted_on(s, &app, now_secs()))
                {
                    return None;
                }
                Some(
                    batch
                        .into_iter()
                        .filter(|e| match &e.audience {
                            None => true,
                            Some(audience) => s.apps.get(&app).is_some_and(|a| {
                                s.receives(a, audience, e.caller.as_deref(), subscriber.as_deref())
                            }),
                        })
                        .collect::<Vec<_>>(),
                )
            })
        }
    });
    let stream = judged
        .take_while(|batch| std::future::ready(batch.is_some()))
        .flat_map(|batch| futures_util::stream::iter(batch.unwrap_or_default()));
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
/// while a member ([`crate::system::SystemState::owner_acts`]). Judged on the record as `s`
/// holds it ([`fresh_record`]), not the one in hand.
fn may_handle_files(
    s: &crate::system::SystemState,
    app: &crate::types::AppRecord,
    account: Option<&str>,
) -> bool {
    let Some(app) = fresh_record(s, app) else {
        return false;
    };
    account.is_some_and(|a| {
        s.owner_acts(app, a)
            || s.orgs
                .get(&app.org)
                .is_some_and(|o| o.role(a).is_some_and(|r| r >= OrgRole::Member))
    })
}

/// Whether `account` may manage `app`'s data (its owner, of an organization's
/// app while a member of it, or an admin of its organization): delete its
/// blobs ([`crate::system::SystemState::manages_app`]). Judged on the record as applied
/// now ([`fresh_record`]), not the one in hand.
fn manages_app(node: &Node, app: &crate::types::AppRecord, account: Option<&str>) -> bool {
    account.is_some_and(|a| {
        node.sys(|s| fresh_record(s, app).is_some_and(|app| s.manages_app(app, a)))
    })
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
        "# HELP calimero_replica_strong_views_total Strong views this replica answered itself after a read index from the app's leader, instead of sending them to the leader.\n# TYPE calimero_replica_strong_views_total counter\ncalimero_replica_strong_views_total {}",
        node.replica_strong_views.load(std::sync::atomic::Ordering::Relaxed)
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
        let owner_approves = s.owner_approves(false);
        let keep = |a: &crate::types::AppRecord| {
            // An organization's apps, or (no `org`) the account's own,
            // wherever they are.
            let in_scope = match org {
                Some(o) => a.org == o,
                None => a.owner == account,
            };
            in_scope
                && (s.may_reach(a, Some(account)) || s.may_upgrade(a, account, owner_approves))
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
        // The organization's set, or for the account's own apps its personal
        // organization's before organizations are ordinary, after it its only
        // organization's (each app's own is in its entry).
        let approved = match org {
            Some(o) => s.approved_for(o),
            None => s
                .personal_org_of(account)
                .or_else(|| match s.orgs_of(account).as_slice() {
                    [one] => Some(*one),
                    _ => None,
                })
                .and_then(|o| s.approved_for(o)),
        };
        let scope = match org {
            Some(o) if s.orgs.get(o).is_some_and(|o| s.acts_personal(o)) => {
                json!({ "kind": "org", "org": o, "personal": true })
            }
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
        time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|t| u64::try_from(t.unix_timestamp()).ok())
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

/// `400 BAD_SECRET`: a secret's value is empty or longer than
/// [`crate::component::MAX_SECRET_VALUE_BYTES`].
fn bad_secret_value() -> ApiError {
    ApiError::bad_request("BAD_SECRET", crate::component::SECRET_VALUE_SIZE)
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
        .route("/ops/recover", post(restore::ops_recover))
        .route("/recover/holder", post(restore::recover_holder))
        .route("/ops/recovery-files", get(restore::ops_recovery_files))
        .merge(auth::routes())
        .merge(public_links::routes())
        .merge(recovery::routes())
        .merge(backup_keys::routes())
        .merge(packages::routes())
        .merge(apps::routes())
        .merge(calls::routes())
        .merge(files::routes())
        .merge(upgrades::routes())
        .merge(update_policies::routes())
        .merge(undo::routes())
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
    use calimero_sealed::SealedSession;

    use super::{
        admit_pending, api_key_shaped, typed_id, Ceremonies, Pending, UnknownApiKeys,
        MAX_CEREMONIES_PER_SESSION, MAX_CHALLENGES_PER_ACCOUNT, MAX_PENDING_CHALLENGES,
        MAX_UNKNOWN_API_KEYS,
    };

    /// Every key the node makes passes the shape check that refuses a token
    /// before its system read; nothing else of the `cal_` prefix does.
    #[test]
    fn every_made_api_key_is_shaped_and_junk_is_not() {
        for _ in 0..1000 {
            let (key, _hash) = calimero_identity::new_api_key();
            assert!(api_key_shaped(&key), "{key}");
        }
        let made = calimero_identity::new_api_key().0;
        for junk in [
            "cal_",
            "cal_short",
            &made[..made.len() - 1],
            &format!("{made}A"),
            &format!("cal_{}", "!".repeat(43)),
            &format!("cax_{}", &made[4..]),
        ] {
            assert!(!api_key_shaped(junk), "{junk}");
        }
    }

    /// The missing-key memory holds what it was given and stops growing at
    /// its bound rather than evicting a fresh entry.
    #[test]
    fn unknown_api_keys_are_bounded() {
        let keys = UnknownApiKeys::default();
        assert!(!keys.holds("h0"));
        for i in 0..MAX_UNKNOWN_API_KEYS + 10 {
            keys.add(format!("h{i}"));
        }
        assert!(keys.holds("h0"));
        assert!(!keys.holds(&format!("h{MAX_UNKNOWN_API_KEYS}")));
    }

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

    /// The sealed session numbered `n`.
    fn session(n: u64) -> Option<SealedSession> {
        let mut id = [0; 16];
        id[..8].copy_from_slice(&n.to_be_bytes());
        Some(SealedSession(id))
    }

    /// Hold `p`, begun in `from`, under the id it answers.
    fn put(
        c: &mut Ceremonies,
        p: Pending,
        from: Option<SealedSession>,
        now: u64,
    ) -> Result<String, u16> {
        admit_pending(c, &p, from, now).map_err(|e| e.status)?;
        let id = format!("1.{}", c.next);
        c.insert(id.clone(), p, from);
        Ok(id)
    }

    fn anonymous(c: &Ceremonies) -> usize {
        let held = c
            .held
            .values()
            .filter(|h| h.pending.account().is_none())
            .count();
        assert_eq!(held, c.anonymous, "the anonymous count is the store's");
        held
    }

    fn contains(c: &Ceremonies, id: &str) -> bool {
        c.get(id).is_some()
    }

    /// Anonymous ceremonies have their own pool: filling it never blocks an
    /// account's step-up or passkey add, and expired ones free it.
    #[test]
    fn a_full_anonymous_pool_does_not_block_an_accounts_step_up() {
        let mut c = Ceremonies::default();
        for n in 0..MAX_PENDING_CHALLENGES as u64 {
            assert!(put(&mut c, sign_up(100), session(n), 0).is_ok());
        }
        assert!(put(&mut c, step_up("alice", 100), None, 0).is_ok());
        // Expired ones free the pool.
        assert!(put(&mut c, sign_up(200), session(0), 100).is_ok());
        assert_eq!(anonymous(&c), 1);
    }

    /// One session's begins past its share displace its own oldest, never
    /// another session's ceremony.
    #[test]
    fn one_session_only_displaces_its_own_ceremonies() {
        let mut c = Ceremonies::default();
        let alice = put(&mut c, sign_up(100), session(1), 0).unwrap();
        let flood: Vec<String> = (0..MAX_PENDING_CHALLENGES)
            .map(|_| put(&mut c, sign_up(100), session(2), 0).unwrap())
            .collect();
        assert!(contains(&c, &alice), "alice's sign-in is still open");
        assert_eq!(anonymous(&c), 1 + MAX_CEREMONIES_PER_SESSION);
        // The flood keeps its newest.
        let kept = &flood[flood.len() - MAX_CEREMONIES_PER_SESSION..];
        assert!(kept.iter().all(|id| contains(&c, id)));
        assert!(!contains(&c, &flood[0]));
    }

    /// A flood spread over many sessions fills the pool, and then each new
    /// begin displaces the oldest of the session holding the most, oldest
    /// first among equals: a ceremony begun after the flood filled the pool
    /// outlives the flood's next `MAX_PENDING_CHALLENGES - 1` begins.
    #[test]
    fn a_full_pool_displaces_the_biggest_holder_then_the_oldest() {
        let mut c = Ceremonies::default();
        // Two ceremonies per flooding session fill the pool.
        let per = 2;
        let sessions = (MAX_PENDING_CHALLENGES / per) as u64;
        for n in 0..sessions {
            for _ in 0..per {
                assert!(put(&mut c, sign_up(100), session(100 + n), 0).is_ok());
            }
        }
        assert_eq!(anonymous(&c), MAX_PENDING_CHALLENGES);
        // Alice holds one: a flooding session holding two loses its oldest.
        let alice = put(&mut c, sign_up(100), session(1), 0).unwrap();
        assert_eq!(anonymous(&c), MAX_PENDING_CHALLENGES);
        assert!(contains(&c, &alice));
        // Then a fresh session per begin, as the handshake limit allows:
        // the oldest ceremonies go first, never alice's while older ones remain.
        for n in 0..MAX_PENDING_CHALLENGES as u64 - 1 {
            assert!(put(&mut c, sign_up(100), session(1_000_000 + n), 0).is_ok());
            assert!(contains(&c, &alice), "after {n} more begins");
        }
        assert_eq!(anonymous(&c), MAX_PENDING_CHALLENGES);
        // Now the oldest, hers goes next.
        assert!(put(&mut c, sign_up(100), session(2_000_000), 0).is_ok());
        assert!(!contains(&c, &alice));
    }

    /// One account's step-ups are capped, and the cap is its own.
    #[test]
    fn one_account_cannot_fill_the_accounts_pool() {
        let mut c = Ceremonies::default();
        for _ in 0..MAX_CHALLENGES_PER_ACCOUNT {
            assert!(put(&mut c, step_up("mallory", 100), None, 0).is_ok());
        }
        assert_eq!(put(&mut c, step_up("mallory", 100), None, 0), Err(429));
        assert!(put(&mut c, step_up("alice", 100), None, 0).is_ok());
        assert!(put(&mut c, sign_up(100), session(1), 0).is_ok());
    }

    /// An id people type never starts with `-`, which `meroctl` would read as a flag.
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
