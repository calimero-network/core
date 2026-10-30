//! The consent page: how an app asks its user to approve something only the
//! account holder may approve, without the app's own page ever driving the
//! approval.
//!
//! # Why the app cannot approve by itself
//!
//! A passkey assertion is blind: the authenticator shows the person a site
//! name, never what the challenge commits to. Whatever page asks for the
//! assertion decides what the person believes they are approving. An app's
//! frontend is the publisher's code, served from the bundle and possibly
//! closed source, so if it could run the ceremony it could show "approve
//! v1.2" and have the passkey sign an upgrade to anything. That is why a
//! session opened on an app's origin (`SessionClaims::app`) is refused every
//! approval route (`api::Principal::unbound`), and still is.
//!
//! # The flow
//!
//! 1. **Request.** The app, with any session of the account (a package-bound
//!    one included), asks `POST /consent/requests { kind, scope, subject,
//!    returnTo }` inside the sealed channel. The node resolves the subject at
//!    once, from its own state and the signed manifests (never from the
//!    request beyond the names in it), freezes what it will show as the
//!    request's *model*, and answers `{ requestId, url, expiresAt }`. The
//!    request is held in this node's memory, bound to the account, for
//!    [`CONSENT_REQUEST_TTL`]; `returnTo` must be the requesting package's
//!    own frontend origin or one its signed manifest names, or for an unbound
//!    session one of the cluster's configured origins (no open redirect).
//! 2. **Page.** The app sends the person to `url`: `GET /consent/{id}` on
//!    this node's own origin, a static page compiled into the node binary
//!    under a strict content security policy (inline script and style
//!    allowed by hash only, `connect-src 'self'`, `frame-ancestors 'none'`).
//!    No app code runs there. The page reads the model
//!    (`GET /consent/{id}/model`) and renders it: what is approved, for
//!    whom, what changes and what stays as it is until the person approves.
//! 3. **Passkey.** On *Approve*, the page posts `POST /consent/{id}/approve`.
//!    The node runs the same route code a signed-in client would
//!    (`api::set_release_approval`, `api::upgrade_app`,
//!    `api::set_outbound_approval`) with the arguments frozen in the
//!    request, so the `412 CONFIRM_REQUIRED` challenge commits to exactly
//!    what the page showed. The page asks the authenticator for a
//!    user-verified assertion over it and posts again with `confirm`; the
//!    node checks the passkey belongs to the request's account, writes the
//!    approval with the assertion logged as consent, and marks the request
//!    decided (it is single-use).
//! 4. **Back to the app.** The page navigates to
//!    `returnTo?request_id=…&result=approved|declined`. An app that lost the
//!    redirect asks `GET /consent/{id}/result` (sealed, same account).
//!
//! # What the consent binds
//!
//! - **Release**: `{ version, policyDigest }`, the digest of the release's
//!   Sigstore-verified `published-mrtds.json` ([`release_digest`]), so a
//!   re-published release with other measurements under the same tag is not
//!   the one approved. A node that cannot fetch them refuses the approval
//!   (`503`); on mock attestation, where no node checks measurements, the
//!   digest is over none. A withdrawal (`subject.withdraw`) binds
//!   `{ version }` alone and fetches nothing: revoking a release never waits
//!   on the network.
//! - **Upgrade**: `{ bundle, migrate, manifest }`: the target WASM's hash and
//!   the target release's signed-manifest hash
//!   (`VerifiedBundle::manifest_hash`; `null` only for a raw development
//!   bundle, which has no manifest). A bundle re-installed under the same
//!   version with any other manifest (other outbound prefixes, frontend
//!   origins, artifacts) is another `.mpk`: the page refuses it as stale.
//! - **Outbound**: `{ package, prefixes, replace }`, with the prefixes the
//!   page listed, never `"manifest"` (which would read whatever the latest
//!   release declares when the approval is applied).
//!
//! Before each step the page checks the facts it showed still hold (the
//! app's current bundle, the target's `.mpk`, the latest release's `.mpk`)
//! and answers `409 CONSENT_STALE` otherwise: the app asks again and the
//! person sees the new facts.
//!
//! # What the page does not protect
//!
//! A browser cannot open the sealed channel without a client library, so the
//! page and its model travel in the clear over whatever TLS the deployment
//! terminates. Whoever terminates TLS can read what is being approved and
//! could serve another page; it cannot change *what* is approved (the
//! arguments are the request's, fixed inside the sealed channel, and the
//! passkey signs a challenge over them) nor approve without the person's
//! passkey. The page's origin must be one the passkey's relying party ID
//! covers (`--rp-id` a registrable suffix of the node's host).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use calimero_identity::{b64url, random, RelyingParty};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::api::{
    owner_holds, peer_only, person, set_outbound_approval, set_release_approval, signed_in,
    upgrade_app, ConfirmBody, OutboundApprovalBody, Principal, ReleaseApprovalBody, UpgradeBody,
};
use crate::error::ApiError;
use crate::node::Node;
use crate::system::{PackageVersion, ReleaseScope};
use crate::types::{now_secs, Caller};

mod diff;

pub use diff::{capability_sheet, Side};

/// How long a request waits for its decision.
pub const CONSENT_REQUEST_TTL: u64 = 10 * 60;
/// How long a decided request's result stays readable (`/result`).
pub const CONSENT_RESULT_TTL: u64 = 10 * 60;
/// The most requests one node holds, decided or not.
pub const MAX_CONSENT_REQUESTS: usize = 1_000;
/// The most undecided requests one account may hold on one node.
pub const MAX_OPEN_REQUESTS_PER_ACCOUNT: usize = 8;
/// The longest `returnTo` accepted.
pub const MAX_RETURN_TO_BYTES: usize = 2_048;
/// The most outbound prefixes one request may name.
pub const MAX_REQUESTED_PREFIXES: usize = 64;
/// The largest body the request and page routes read.
pub const MAX_CONSENT_BODY_BYTES: usize = 64 * 1024;
/// The longest mero-tee version a release request may name.
const MAX_VERSION_BYTES: usize = 64;
/// How many releases' manifest facts a node keeps.
const CACHED_RELEASE_FACTS: usize = 64;
/// How long the page waits for a release's signed measurements.
const POLICY_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Where mero-tee publishes its releases.
const MERO_TEE_RELEASES: &str = "https://github.com/calimero-network/mero-tee/releases/tag";

const PAGE_HTML: &str = include_str!("consent_page/page.html");
const PAGE_CSS: &str = include_str!("consent_page/page.css");
const PAGE_JS: &str = include_str!("consent_page/page.js");

/// The page, with its style and script inlined, and the policy that allows
/// exactly those two by hash and nothing else.
static PAGE: LazyLock<(String, String)> = LazyLock::new(|| {
    let hash = |s: &str| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(calimero_identity::sha256(s.as_bytes()))
    };
    let html = PAGE_HTML
        .replace("/*STYLE*/", PAGE_CSS)
        .replace("/*SCRIPT*/", PAGE_JS);
    let csp = format!(
        "default-src 'none'; script-src 'sha256-{}'; style-src 'sha256-{}'; connect-src 'self'; \
         img-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        hash(PAGE_JS),
        hash(PAGE_CSS)
    );
    (html, csp)
});

// ---------------------------------------------------------------- requests

/// `POST /consent/requests`: what an app asks its user to approve.
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CreateBody {
    /// Let nodes on a mero-tee release hold the scope's apps' data.
    Release {
        scope: ScopeBody,
        subject: ReleaseSubject,
        return_to: String,
    },
    /// Upgrade an app to another release of its own package.
    Upgrade {
        subject: UpgradeSubject,
        return_to: String,
    },
    /// Let a package's apps in the scope call out to URL prefixes.
    Outbound {
        scope: ScopeBody,
        subject: OutboundSubject,
        return_to: String,
    },
}

/// Whose approval: the account's own apps, or an organization's.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ScopeBody {
    Account,
    Org(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseSubject {
    version: String,
    /// `true` asks to withdraw the release's approval instead.
    #[serde(default)]
    withdraw: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradeSubject {
    app: String,
    to_version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboundSubject {
    /// The package; a package-bound session's own when absent.
    #[serde(default)]
    package: Option<String>,
    /// The prefixes to approve; every one the latest release declares when
    /// absent.
    #[serde(default)]
    prefixes: Option<Vec<String>>,
}

/// What approving a request does, with the facts it was shown frozen.
#[derive(Clone, Debug)]
enum Action {
    Release {
        scope: ReleaseScope,
        version: String,
        change: ReleaseChange,
    },
    Upgrade {
        app: String,
        /// The bundle the app ran when the page was made.
        from_bundle: String,
        package: String,
        version: String,
        service: Option<String>,
        /// The target release's `.mpk` (its manifest included).
        mpk: String,
    },
    Outbound {
        scope: ReleaseScope,
        package: String,
        prefixes: Vec<String>,
        /// The `.mpk` of the release whose declarations the page listed.
        latest_mpk: String,
    },
}

/// What a release request changes.
#[derive(Clone, Debug)]
enum ReleaseChange {
    /// Approve it, bound to the digest of the measurements the page showed.
    Approve { policy_digest: String },
    /// Withdraw its approval, bound to the version alone.
    Withdraw,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Decision {
    Pending,
    /// The page is running the approval now.
    Deciding,
    Approved {
        at: u64,
    },
    Declined {
        at: u64,
    },
}

impl Decision {
    const fn label(&self) -> &'static str {
        match self {
            Self::Pending | Self::Deciding => "pending",
            Self::Approved { .. } => "approved",
            Self::Declined { .. } => "declined",
        }
    }

    const fn at(&self) -> Option<u64> {
        match self {
            Self::Approved { at } | Self::Declined { at } => Some(*at),
            Self::Pending | Self::Deciding => None,
        }
    }
}

#[derive(Clone, Debug)]
struct ConsentRequest {
    account: String,
    /// The package whose frontend asked (a package-bound session's).
    package: Option<String>,
    action: Action,
    /// What the page shows, frozen when the request was made.
    model: Value,
    return_to: Url,
    expires_at: u64,
    decision: Decision,
}

/// A release's signed manifest, read once per `.mpk`.
struct ReleaseFacts {
    manifest: calimero_bundle::BundleManifest,
    manifest_hash: String,
    signer: String,
    /// Each service's embedded ABI (`""` for a single-service bundle).
    abi: BTreeMap<String, Option<calimero_wasm_abi::schema::Manifest>>,
}

/// The consent requests this node issued, and the release facts their pages
/// read.
#[derive(Default)]
pub struct ConsentRequests {
    open: Mutex<HashMap<String, ConsentRequest>>,
    facts: Mutex<HashMap<String, Arc<ReleaseFacts>>>,
}

impl ConsentRequests {
    fn open(&self) -> std::sync::MutexGuard<'_, HashMap<String, ConsentRequest>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keep `req` under a new id, within the node's and the account's bounds.
    fn insert(&self, node_id: u64, req: ConsentRequest, now: u64) -> Result<String, ApiError> {
        let mut open = self.open();
        open.retain(|_, r| r.expires_at > now);
        if open.len() >= MAX_CONSENT_REQUESTS {
            return Err(ApiError::new(
                429,
                "RATE_LIMITED",
                "too many consent requests on this node; try again shortly",
            ));
        }
        let mine = open
            .values()
            .filter(|r| r.account == req.account && r.decision == Decision::Pending)
            .count();
        if mine >= MAX_OPEN_REQUESTS_PER_ACCOUNT {
            return Err(ApiError::new(
                429,
                "RATE_LIMITED",
                "this account has too many undecided consent requests; decide or let them expire",
            ));
        }
        let id = format!("{node_id}.{}", b64url(&random::<16>()));
        let _prev = open.insert(id.clone(), req);
        Ok(id)
    }

    /// The request `id`, unless it expired.
    fn get(&self, id: &str, now: u64) -> Result<ConsentRequest, ApiError> {
        let mut open = self.open();
        open.retain(|_, r| r.expires_at > now);
        open.get(id).cloned().ok_or_else(unknown_request)
    }

    /// Move `id` from `from` to `to`; refused when it is no longer `from`
    /// (decided already, or another approval under way).
    fn transition(
        &self,
        id: &str,
        from: &Decision,
        to: Decision,
        now: u64,
    ) -> Result<ConsentRequest, ApiError> {
        let mut open = self.open();
        open.retain(|_, r| r.expires_at > now);
        let r = open.get_mut(id).ok_or_else(unknown_request)?;
        if &r.decision != from {
            return Err(ApiError::new(
                409,
                "ALREADY_DECIDED",
                &format!("this request is {} already", r.decision.label()),
            ));
        }
        if to.at().is_some() {
            r.expires_at = r.expires_at.max(now + CONSENT_RESULT_TTL);
        }
        r.decision = to;
        Ok(r.clone())
    }

    fn cached(&self, mpk: &str) -> Option<Arc<ReleaseFacts>> {
        self.facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(mpk)
            .cloned()
    }

    fn cache(&self, mpk: &str, f: Arc<ReleaseFacts>) {
        let mut facts = self.facts.lock().unwrap_or_else(PoisonError::into_inner);
        if facts.len() >= CACHED_RELEASE_FACTS {
            facts.clear();
        }
        let _prev = facts.insert(mpk.to_owned(), f);
    }
}

fn unknown_request() -> ApiError {
    ApiError::not_found("unknown or expired consent request; ask the app to start again")
}

/// Whether `id` is shaped like a request id this cluster issues
/// (`{node}.{22 base64url characters}`), checked before any lookup.
fn is_request_id(id: &str) -> bool {
    id.split_once('.').is_some_and(|(n, r)| {
        !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && r.len() == 22
            && r.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

pub(crate) fn stale(what: &str) -> ApiError {
    ApiError::new(
        409,
        "CONSENT_STALE",
        &format!("{what} changed since this page was made; ask the app to start again"),
    )
}

// ---------------------------------------------------------------- facts

/// A release's manifest facts, from its verified `.mpk` (cached by hash).
async fn release_facts(
    node: &Node,
    package: &str,
    v: &PackageVersion,
) -> Result<Arc<ReleaseFacts>, ApiError> {
    if let Some(f) = node.consent.cached(&v.mpk) {
        return Ok(f);
    }
    let mpk = node.release_mpk(package, v).await?;
    let (bundle, wasm) = crate::packages::verify(mpk)?;
    let facts = Arc::new(ReleaseFacts {
        manifest: bundle.manifest().clone(),
        manifest_hash: hex::encode(bundle.manifest_hash()),
        signer: bundle.signer_id().to_owned(),
        abi: wasm
            .iter()
            .map(|(name, bytes)| {
                (
                    name.clone(),
                    calimero_wasm_abi::embed::read_embedded_state_schema(bytes),
                )
            })
            .collect(),
    });
    node.consent.cache(&v.mpk, Arc::clone(&facts));
    Ok(facts)
}

/// The signed-manifest hash of the release an upgrade targets: `release`
/// (package, version) when the request named one, else the release `bundle`
/// came out of; `None` for a raw development bundle, which has no manifest.
pub(crate) async fn target_manifest_hash(
    node: &Node,
    release: Option<(&str, &str)>,
    bundle: &str,
) -> Result<Option<String>, ApiError> {
    let found = match release {
        Some((p, v)) => node
            .sys(|s| s.packages.get(p)?.versions.get(v).cloned())
            .map(|ver| (p.to_owned(), ver)),
        None => node.release_of(bundle).map(|r| (r.at.package, r.version)),
    };
    let Some((package, version)) = found else {
        return Ok(None);
    };
    Ok(Some(
        release_facts(node, &package, &version)
            .await?
            .manifest_hash
            .clone(),
    ))
}

/// A mero-tee release's signed measurements, Sigstore-verified. On mock
/// attestation, where no node checks measurements, none; an error when this
/// node cannot fetch them now.
async fn release_policy(
    node: &Node,
    version: &str,
) -> Result<Arc<calimero_tee_release::NodeRelease>, ApiError> {
    if node.config.attestation == crate::attest::Mode::Mock {
        return Ok(Arc::new(calimero_tee_release::NodeRelease {
            version: version.to_owned(),
            profiles: BTreeMap::new(),
        }));
    }
    fetch_release_policy(version).await.ok_or_else(|| {
        ApiError::unavailable(
            "this node cannot read the release's signed measurements now; try again",
        )
    })
}

/// The release's measurements from its Sigstore-verified `published-mrtds.json`
/// (cached by `calimero_tee_release`); `None` when they cannot be fetched now.
async fn fetch_release_policy(version: &str) -> Option<Arc<calimero_tee_release::NodeRelease>> {
    // The verifier's future is not `Send`: a thread and a runtime of its own,
    // as the client's attestation check does.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let version = version.to_owned();
    let _thread = std::thread::Builder::new()
        .name("consent-release-fetch".into())
        .spawn(move || {
            let out = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()
                .and_then(|rt| {
                    rt.block_on(calimero_tee_release::fetch_node_release(&version))
                        .ok()
                });
            let _sent = tx.send(out);
        })
        .ok()?;
    tokio::time::timeout(POLICY_FETCH_TIMEOUT, rx)
        .await
        .ok()?
        .ok()
        .flatten()
}

/// The digest an approval of release `r` binds: SHA-256 of its measurements
/// as JSON with sorted keys, hex.
#[must_use]
pub fn policy_digest(r: &calimero_tee_release::NodeRelease) -> String {
    hex::encode(calimero_identity::sha256(
        policy_json(r).to_string().as_bytes(),
    ))
}

fn policy_json(r: &calimero_tee_release::NodeRelease) -> Value {
    let profiles: BTreeMap<&String, Value> = r
        .profiles
        .iter()
        .map(|(name, m)| {
            (
                name,
                json!({
                    "mrtd": m.allowed_mrtd,
                    "rtmr0": m.allowed_rtmr0,
                    "rtmr1": m.allowed_rtmr1,
                    "rtmr2": m.allowed_rtmr2,
                    "rtmr3": m.allowed_rtmr3,
                }),
            )
        })
        .collect();
    // Keys in sorted order, so the bytes do not depend on whether
    // `serde_json`'s `preserve_order` is enabled in this build.
    json!({ "profiles": profiles, "version": r.version })
}

/// The digest an approval of mero-tee `version` binds: [`policy_digest`] of
/// its signed measurements as this node reads them now.
pub(crate) async fn release_digest(node: &Node, version: &str) -> Result<String, ApiError> {
    Ok(policy_digest(&*release_policy(node, version).await?))
}

// ---------------------------------------------------------------- return_to

/// The origin (`scheme://host[:port]`) of `u`.
fn origin_of(u: &Url) -> String {
    u.origin().ascii_serialization()
}

fn local_host(u: &Url) -> bool {
    matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        || u.host_str().is_some_and(|h| h.ends_with(".localhost"))
}

/// `raw`, if the app may be sent back there: a package-bound session to its
/// package's served frontend origin, or an origin its latest signed manifest
/// names (`links.frontend`, `frontendAliases`); an unbound session to one of
/// the cluster's configured origins. Anything else would make the node an
/// open redirect, a phishing aid that names a trusted page.
async fn allowed_return_to(node: &Node, package: Option<&str>, raw: &str) -> Result<Url, ApiError> {
    let refuse =
        |why: &str| ApiError::bad_request("RETURN_TO_NOT_ALLOWED", &format!("returnTo {why}"));
    if raw.len() > MAX_RETURN_TO_BYTES {
        return Err(refuse(&format!(
            "is longer than {MAX_RETURN_TO_BYTES} bytes"
        )));
    }
    let u = Url::parse(raw).map_err(|_| refuse("is not an absolute URL"))?;
    let secure = u.scheme() == "https" || (u.scheme() == "http" && local_host(&u));
    if !secure || !u.username().is_empty() || u.password().is_some() || u.fragment().is_some() {
        return Err(refuse(
            "must be https (http only for localhost), without credentials or a fragment",
        ));
    }
    let origin = origin_of(&u);
    let allowed = match package {
        None => node
            .config
            .origins
            .iter()
            .any(|o| o.trim_end_matches('/') == origin),
        Some(p) => {
            node.app_origin_package(&origin).as_deref() == Some(p)
                || manifest_origins(node, p).await?.contains(&origin)
        }
    };
    if allowed {
        Ok(u)
    } else {
        Err(refuse(
            "must be the requesting app's own frontend origin (or one its signed manifest names), \
             or the cluster's own origin",
        ))
    }
}

/// The frontend origins `package`'s newest release signs for.
async fn manifest_origins(node: &Node, package: &str) -> Result<BTreeSet<String>, ApiError> {
    let Some(latest) = node.sys(|s| s.packages.get(package)?.latest().cloned()) else {
        return Ok(BTreeSet::new());
    };
    let facts = release_facts(node, package, &latest).await?;
    let m = &facts.manifest;
    Ok(m.links
        .as_ref()
        .and_then(|l| l.frontend.as_deref())
        .into_iter()
        .chain(m.frontend_aliases.iter().map(String::as_str))
        .filter_map(|s| Url::parse(s).ok())
        .map(|u| origin_of(&u))
        .filter(|o| o != "null")
        .collect())
}

// ---------------------------------------------------------------- creating

/// `POST /consent/requests`: hold a consent request for the signed-in
/// account and answer where its page is.
pub async fn create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    if body.len() > MAX_CONSENT_BODY_BYTES {
        return Err(ApiError::new(413, "TOO_LARGE", "consent request too large"));
    }
    let p = person(&node, &headers).await?;
    let body: CreateBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?;
    let account = p.account()?;
    let package = p.bound_app().map(str::to_owned);
    node.sys_fresh().await;
    let (action, model, return_to) = match body {
        CreateBody::Release {
            scope,
            subject,
            return_to,
        } => {
            let to = allowed_return_to(&node, package.as_deref(), &return_to).await?;
            let (action, model) = release_request(&node, &account, &scope, subject).await?;
            (action, model, to)
        }
        CreateBody::Upgrade { subject, return_to } => {
            let to = allowed_return_to(&node, package.as_deref(), &return_to).await?;
            let (action, model) = upgrade_request(&node, &p, &account, subject).await?;
            (action, model, to)
        }
        CreateBody::Outbound {
            scope,
            subject,
            return_to,
        } => {
            let to = allowed_return_to(&node, package.as_deref(), &return_to).await?;
            let (action, model) =
                outbound_request(&node, &account, package.as_deref(), &scope, subject).await?;
            (action, model, to)
        }
    };
    let now = now_secs();
    let expires_at = now + CONSENT_REQUEST_TTL;
    let mut model = model;
    if let Some(o) = model.as_object_mut() {
        let _prev = o.insert("approver".into(), json!({ "account": account }));
        let _prev = o.insert(
            "requestedBy".into(),
            json!({ "package": package, "returnOrigin": origin_of(&return_to) }),
        );
        let _prev = o.insert("expiresAt".into(), json!(expires_at));
    }
    let id = node.consent.insert(
        node.config.node_id,
        ConsentRequest {
            account,
            package,
            action,
            model,
            return_to,
            expires_at,
            decision: Decision::Pending,
        },
        now,
    )?;
    let base = Url::parse(&node.config.public_url)
        .map_err(|e| ApiError::internal(&format!("this node's public URL: {e}")))?;
    let url = base
        .join(&format!("consent/{id}"))
        .map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(json!({
        "requestId": id,
        "url": url.as_str(),
        "expiresAt": expires_at,
    })))
}

/// `scope` for `account`, refused as missing when the account is not in the
/// organization it names.
fn scope_of(
    node: &Node,
    account: &str,
    scope: &ScopeBody,
) -> Result<(ReleaseScope, Value), ApiError> {
    match scope {
        ScopeBody::Account => Ok((
            ReleaseScope::Account,
            json!({ "kind": "account", "account": account }),
        )),
        ScopeBody::Org(org) => {
            let name = node
                .sys(|s| {
                    let o = s.orgs.get(org)?;
                    o.role(account)?;
                    Some(o.name.clone())
                })
                .ok_or_else(|| ApiError::not_found("no such organization"))?;
            Ok((
                ReleaseScope::Org { org: org.clone() },
                json!({ "kind": "org", "org": org, "name": name }),
            ))
        }
    }
}

/// How the scope reads in a sentence: "your organization Acme", "your apps".
fn scope_words(scope: &Value) -> String {
    match scope["name"].as_str() {
        Some(name) => format!("the organization {name}"),
        None => "your own apps".to_owned(),
    }
}

async fn release_request(
    node: &Node,
    account: &str,
    scope: &ScopeBody,
    subject: ReleaseSubject,
) -> Result<(Action, Value), ApiError> {
    owner_holds(node, |g| g.releases, "release approvals")?;
    let version = subject.version;
    if version.is_empty()
        || version.len() > MAX_VERSION_BYTES
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_'))
    {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "subject.version is not a release version",
        ));
    }
    let (scope, scope_json) = scope_of(node, account, scope)?;
    let approved: Vec<String> = node.sys(|s| {
        let set = match &scope {
            ReleaseScope::Org { org } => s.orgs.get(org).map(|o| o.approved_releases.clone()),
            ReleaseScope::Account => s.accounts.get(account).map(|a| a.approved_releases.clone()),
            ReleaseScope::Cluster => None,
        };
        set.unwrap_or_default().into_iter().collect()
    });
    let ids: Vec<u64> = node.sys(|s| s.nodes.keys().copied().collect());
    let mut on_it = Vec::new();
    for id in ids {
        if node.release_of_node(id).await.as_deref() == Some(version.as_str()) {
            on_it.push(id);
        }
    }
    let profile = (node.own_release() == version)
        .then(|| node.config.tee_profile.clone())
        .flatten();
    if subject.withdraw {
        let model = withdrawal_model(&version, scope_json, approved, &on_it, profile);
        return Ok((
            Action::Release {
                scope,
                version,
                change: ReleaseChange::Withdraw,
            },
            model,
        ));
    }
    let policy = release_policy(node, &version).await?;
    let digest = policy_digest(&policy);
    let mock = node.config.attestation == crate::attest::Mode::Mock;
    let tag = calimero_tee_release::normalize_release_version(
        &version,
        calimero_tee_release::NODE_RELEASE_TAG_PREFIX,
    )
    .map_or_else(
        |_| version.clone(),
        |v| format!("{}{v}", calimero_tee_release::NODE_RELEASE_TAG_PREFIX),
    );
    let whose = scope_words(&scope_json);
    let already = approved.contains(&version);
    let model = json!({
        "kind": "release",
        "scope": scope_json,
        "headline": format!("Let nodes running mero-tee {version} hold the data of {whose}"),
        "before": format!(
            "Until you approve, nodes running mero-tee {version} receive none of the data of {whose}: \
             the nodes that hold it refuse to send them any."
        ),
        "after": format!(
            "Once you approve, nodes running {version} may receive, hold and run the apps of {whose}, \
             next to the releases already approved. You can withdraw it later, unless it is the last one approved."
        ),
        "notes": if already {
            json!([format!("{version} is already approved for {whose}; approving again changes nothing.")])
        } else {
            json!([])
        },
        "release": {
            "version": version,
            "change": "approve",
            "alreadyApproved": already,
            "approved": approved,
            "nodes": on_it,
            "profile": profile,
            "measurements": {
                "policyDigest": digest,
                "profiles": policy_json(&policy)["profiles"].clone(),
                "mock": mock.then_some(
                    "this cluster runs mock attestation: no node checks measurements, and there are none to show"
                ),
            },
            "links": {
                "release": format!("{MERO_TEE_RELEASES}/{tag}"),
                "measurements": format!(
                    "{}/{tag}/{}",
                    calimero_tee_release::MERO_TEE_RELEASE_BASE,
                    calimero_tee_release::PUBLISHED_MRTDS_ASSET
                ),
            },
            "verify": "published-mrtds.json is signed with Sigstore by the mero-tee release workflow; \
                       verify it with cosign, then compare its registers with the ones shown here",
            "deadline": null,
        },
    });
    Ok((
        Action::Release {
            scope,
            version,
            change: ReleaseChange::Approve {
                policy_digest: digest,
            },
        },
        model,
    ))
}

/// What the page shows for withdrawing `version`'s approval: what the nodes
/// that hold the data do once it is withdrawn (`Node::may_send`,
/// `Node::place`, the replica-move check), what they leave as it is, and the
/// rule `apply` holds it to. No measurements: a withdrawal lets nothing in.
fn withdrawal_model(
    version: &str,
    scope: Value,
    approved: Vec<String>,
    on_it: &[u64],
    profile: Option<String>,
) -> Value {
    let whose = scope_words(&scope);
    let current = approved.iter().any(|v| v == version);
    let last = current && approved.len() == 1;
    let mut notes = Vec::new();
    if !current {
        notes.push(format!(
            "{version} is not approved for {whose}; withdrawing it changes nothing."
        ));
    }
    if last {
        notes.push(format!(
            "{version} is the only release approved for {whose}, and the node refuses to \
             withdraw the last approved release: approve another release first."
        ));
    }
    json!({
        "kind": "release",
        "scope": scope,
        "headline": format!("Stop nodes running mero-tee {version} from receiving the data of {whose}"),
        "before": format!(
            "Until you withdraw it, nodes running mero-tee {version} may receive, hold and run \
             the apps of {whose}."
        ),
        "after": format!(
            "Once you withdraw it, the nodes that hold the data of {whose} send nodes running \
             {version} no more of it (no Raft entries, no snapshots), and no app of {whose} is \
             placed or moved onto them. Nothing is deleted or moved: a replica already on such a \
             node keeps the data it holds until the operator moves it elsewhere, and one that \
             leads an app keeps leading it, receiving its writes, until leadership moves. You can \
             approve {version} again later."
        ),
        "notes": notes,
        "release": {
            "version": version,
            "change": "withdraw",
            "currentlyApproved": current,
            "lastApproved": last,
            "approved": approved,
            "nodes": on_it,
            "profile": profile,
        },
    })
}

async fn upgrade_request(
    node: &Node,
    p: &Principal,
    account: &str,
    subject: UpgradeSubject,
) -> Result<(Action, Value), ApiError> {
    owner_holds(node, |g| g.app_upgrades, "app upgrades")?;
    let app_id = node.canonical_app_id(&subject.app);
    let app = node.app(&app_id).await?;
    p.within(node, &app)?;
    if !node.sys(|s| s.may_upgrade(&app, account)) {
        return Err(ApiError::forbidden(
            "upgrading this app needs its owner, or MANAGE_APPLICATION in its organization",
        ));
    }
    let current = node.release_of(&app.bundle).ok_or_else(|| {
        ApiError::bad_request(
            "NOT_A_RELEASE",
            "this app runs a development bundle, not a signed release; upgrade it directly",
        )
    })?;
    let package = current.at.package.clone();
    let service = current.at.service.clone();
    let bundle = node
        .resolve_release(
            &package,
            Some(&subject.to_version),
            service.as_deref(),
            account,
            p.bound_app() == Some(package.as_str()),
        )
        .await?;
    let target = node
        .sys(|s| {
            s.packages
                .get(&package)?
                .versions
                .get(&subject.to_version)
                .cloned()
        })
        .ok_or_else(|| ApiError::not_found("no such release"))?;
    let from = release_facts(node, &package, &current.version).await?;
    let to = release_facts(node, &package, &target).await?;
    let key = service.clone().unwrap_or_default();
    let (from_abi, to_abi) = (
        from.abi.get(&key).cloned().flatten(),
        to.abi.get(&key).cloned().flatten(),
    );
    let plan = crate::services::plan_from_abis(from_abi.clone(), to_abi.clone());
    let sheet = capability_sheet(
        &Side {
            package: &package,
            version: &current.version.version,
            service: service.as_deref(),
            bundle: &app.bundle,
            mpk: &current.version.mpk,
            manifest_hash: &from.manifest_hash,
            signer: &from.signer,
            manifest: &from.manifest,
            abi: from_abi.as_ref(),
        },
        &Side {
            package: &package,
            version: &target.version,
            service: service.as_deref(),
            bundle: &bundle,
            mpk: &target.mpk,
            manifest_hash: &to.manifest_hash,
            signer: &to.signer,
            manifest: &to.manifest,
            abi: to_abi.as_ref(),
        },
        &plan,
    );
    let name = to
        .manifest
        .metadata
        .as_ref()
        .map_or_else(|| package.clone(), |m| m.name.clone());
    let (old, new) = (&current.version.version, &target.version);
    let mut notes = Vec::new();
    if let Err(why) = &plan {
        notes.push(format!("This upgrade would be refused: {why}."));
    }
    if let Some(export) = plan.as_ref().ok().cloned().flatten() {
        notes.push(format!(
            "Before {new} runs, its migration `{export}` rewrites the app's stored data once."
        ));
    }
    let origins = sheet["frontend"]["newLoginOrigins"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !origins.is_empty() {
        notes.push(format!(
            "{new} names {} new web address(es) its sign-in may return to: {}. Pages there could act in this app as you.",
            origins.len(),
            join(&origins)
        ));
    }
    let added = sheet["outbound"]["added"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !added.is_empty() {
        notes.push(format!(
            "{new} declares {} new address(es) it would send data to: {}. It cannot reach them until they are approved too.",
            added.len(),
            join(&added)
        ));
    }
    if sheet["publisher"]["changed"] == json!(true) {
        notes.push("The release is signed by a different key than the one running now.".to_owned());
    }
    let model = json!({
        "kind": "upgrade",
        "scope": {
            "kind": "app",
            "app": app_id,
            "name": app.name,
            "org": app.org,
        },
        "headline": format!("Upgrade {name} from {old} to {new}"),
        "before": format!(
            "Until you approve, the app keeps running {old}: only that code reads and changes its data."
        ),
        "after": format!(
            "Once you approve, {new}, signed by {}, runs on the app's existing data for everyone who uses it.",
            to.signer
        ),
        "notes": notes,
        "upgrade": sheet,
    });
    Ok((
        Action::Upgrade {
            app: app_id,
            from_bundle: app.bundle,
            package,
            version: target.version.clone(),
            service,
            mpk: target.mpk.clone(),
        },
        model,
    ))
}

fn join(items: &[Value]) -> String {
    items
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

async fn outbound_request(
    node: &Node,
    account: &str,
    bound: Option<&str>,
    scope: &ScopeBody,
    subject: OutboundSubject,
) -> Result<(Action, Value), ApiError> {
    owner_holds(node, |g| g.outbound, "outbound approvals")?;
    let package = match (subject.package, bound) {
        (Some(p), Some(b)) if p != b => {
            return Err(ApiError::forbidden(
                "an app's frontend may ask only about its own package",
            ))
        }
        (Some(p), _) => p,
        (None, Some(b)) => b.to_owned(),
        (None, None) => {
            return Err(ApiError::bad_request(
                "BAD_REQUEST",
                "subject.package is required",
            ))
        }
    };
    let latest = node
        .sys(|s| {
            let pkg = s.packages.get(&package)?;
            let latest = pkg.latest()?;
            (bound == Some(package.as_str()) || s.visible_versions(pkg, account).next().is_some())
                .then(|| latest.clone())
        })
        .ok_or_else(|| ApiError::not_found("no such package"))?;
    let (scope, scope_json) = scope_of(node, account, scope)?;
    let facts = release_facts(node, &package, &latest).await?;
    let declared: Vec<String> = latest.outbound.clone();
    let requested = match subject.prefixes {
        None => declared.clone(),
        Some(p) => p,
    };
    if requested.is_empty() || requested.len() > MAX_REQUESTED_PREFIXES {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            &format!("name between 1 and {MAX_REQUESTED_PREFIXES} prefixes"),
        ));
    }
    if let Some(p) = requested.iter().find(|p| !declared.contains(p)) {
        return Err(ApiError::bad_request(
            "NOT_DECLARED",
            &format!("{p} is not a prefix the package's latest release declares"),
        ));
    }
    let approved: Vec<String> = node.sys(|s| {
        let approvals = match &scope {
            ReleaseScope::Org { org } => s.orgs.get(org).map(|o| &o.outbound_approvals),
            ReleaseScope::Account => s.accounts.get(account).map(|a| &a.outbound_approvals),
            ReleaseScope::Cluster => None,
        };
        approvals
            .and_then(|a| a.get(&facts.signer))
            .map(|a| a.prefixes.clone())
            .unwrap_or_default()
    });
    let new: Vec<&String> = requested.iter().filter(|p| !approved.contains(p)).collect();
    let whose = scope_words(&scope_json);
    let model = json!({
        "kind": "outbound",
        "scope": scope_json,
        "headline": format!("Let {package} send data to {} address(es)", requested.len()),
        "before": format!(
            "Until you approve, apps of {package} in {whose} cannot send anything to these addresses."
        ),
        "after": format!(
            "Once you approve, the code of {package} (signed by {}) may send requests, and whatever data it puts in them, \
             to these addresses from the apps of {whose}. You can revoke this later.",
            facts.signer
        ),
        "notes": [],
        "outbound": {
            "package": package,
            "publisher": facts.signer,
            "release": latest.version,
            "requested": requested,
            "new": new,
            "alreadyApproved": approved,
            "declared": declared,
        },
    });
    Ok((
        Action::Outbound {
            scope,
            package,
            prefixes: requested,
            latest_mpk: latest.mpk.clone(),
        },
        model,
    ))
}

// ---------------------------------------------------------------- results

/// What `/result` answers for a request.
fn result_json(id: &str, r: &ConsentRequest) -> Value {
    json!({
        "requestId": id,
        "kind": r.model["kind"],
        "status": r.decision.label(),
        "decidedAt": r.decision.at(),
    })
}

/// `GET /consent/{id}/result`: the decision, for the account that asked.
pub async fn result(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    if !is_request_id(&id) {
        return Err(unknown_request());
    }
    let holder = id
        .split_once('.')
        .and_then(|(n, _)| n.parse::<u64>().ok())
        .ok_or_else(unknown_request)?;
    let ask = json!({ "id": id, "account": account, "package": p.bound_app() });
    if holder == node.config.node_id {
        return Ok(Json(local_result(&node, &ask)?));
    }
    let addr = node
        .sys(|s| s.nodes.get(&holder).map(|n| n.addr.clone()))
        .ok_or_else(unknown_request)?;
    let v = node
        .peer_post(&addr, "internal/consent/result", &ask)
        .await
        .map_err(|e| match e.downcast::<ApiError>() {
            Ok(api) => api,
            Err(e) => ApiError::unavailable(&format!(
                "the node that holds this request did not answer: {e}"
            )),
        })?;
    Ok(Json(v))
}

fn local_result(node: &Node, ask: &Value) -> Result<Value, ApiError> {
    let id = ask["id"].as_str().unwrap_or_default();
    let r = node.consent.get(id, now_secs())?;
    // Another account's request does not exist for this one, nor another
    // app's for a package-bound session.
    let package = ask["package"].as_str();
    if Some(r.account.as_str()) != ask["account"].as_str()
        || package.is_some_and(|p| r.package.as_deref() != Some(p))
    {
        return Err(unknown_request());
    }
    Ok(result_json(id, &r))
}

/// A peer asks for a request this node holds, on behalf of `account`.
pub async fn internal_result(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(ask): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    peer_only(&node, &headers)?;
    Ok(Json(local_result(&node, &ask)?))
}

// ---------------------------------------------------------------- the page

/// The relying party a consent page's step-up is checked against: the
/// cluster's own origins, and this cluster's nodes' own origins, which serve
/// the page. Never an app's.
pub(crate) fn relying_party(node: &Node) -> RelyingParty {
    let mut rp = crate::api::rp_step_up(node);
    for o in node.api_origins() {
        if !rp.origins.contains(&o) {
            rp.origins.push(o);
        }
    }
    rp
}

enum PageRoute {
    Page,
    Model,
    Approve,
    Decline,
}

/// Outside the sealed channel and CORS: `GET /consent/{id}`, `GET
/// /consent/{id}/model`, `POST /consent/{id}/approve` and `POST
/// /consent/{id}/decline`. Everything else goes on to the node's routes.
pub async fn layer(State(node): State<Arc<Node>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let Some(rest) = path.strip_prefix("/consent/") else {
        return next.run(req).await;
    };
    let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let (get, post) = (
        req.method() == Method::GET || req.method() == Method::HEAD,
        req.method() == Method::POST,
    );
    let route = match tail {
        "" if get => PageRoute::Page,
        "model" if get => PageRoute::Model,
        "approve" if post => PageRoute::Approve,
        "decline" if post => PageRoute::Decline,
        _ => return next.run(req).await,
    };
    if !is_request_id(id) {
        return next.run(req).await;
    }
    let id = id.to_owned();
    let answer = match route {
        PageRoute::Page => return page(&node, &id),
        PageRoute::Model => model(&node, &id),
        PageRoute::Approve | PageRoute::Decline => {
            if let Err(e) = same_origin(&node, req.headers()) {
                return guarded(e.into_response());
            }
            match to_bytes(req.into_body(), MAX_CONSENT_BODY_BYTES).await {
                Err(_) => Err(ApiError::new(413, "TOO_LARGE", "body too large")),
                Ok(body) if matches!(route, PageRoute::Approve) => approve(&node, &id, &body).await,
                Ok(_) => decline(&node, &id),
            }
        }
    };
    guarded(match answer {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    })
}

/// A POST from a page must come from the node's own origins (or none named,
/// as a non-browser client sends): no other site posts a decision.
fn same_origin(node: &Node, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let origin = origin.to_str().unwrap_or_default();
    if relying_party(node)
        .origins
        .iter()
        .any(|o| o.trim_end_matches('/') == origin)
    {
        Ok(())
    } else {
        Err(ApiError::forbidden("not from this node's consent page"))
    }
}

/// The headers every page answer carries.
fn guarded(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    for (k, v) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
    ] {
        let _prev = h.insert(k, HeaderValue::from_static(v));
    }
    if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        let _prev = h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
        );
    }
    resp
}

/// The page itself: static, the same bytes for every request; it reads the
/// model with a script. A 404 for an unknown or expired request.
fn page(node: &Node, id: &str) -> Response {
    let status = if node.consent.get(id, now_secs()).is_ok() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    let (html, csp) = &*PAGE;
    let mut resp = Response::new(Body::from(html.clone()));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    let _prev = h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    if let Ok(v) = HeaderValue::from_str(csp) {
        let _prev = h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    guarded(resp)
}

/// What the page renders: the model frozen at creation, the request's
/// decision, and the relying party the passkey is asked for.
fn model(node: &Node, id: &str) -> Result<Value, ApiError> {
    let r = node.consent.get(id, now_secs())?;
    let mut m = r.model.clone();
    if let Some(o) = m.as_object_mut() {
        let _prev = o.insert("requestId".into(), json!(id));
        let _prev = o.insert("status".into(), json!(r.decision.label()));
        let _prev = o.insert("decidedAt".into(), json!(r.decision.at()));
        let _prev = o.insert("rpId".into(), json!(node.config.rp_id));
    }
    Ok(m)
}

/// Where the page sends the person back: `returnTo` with the request and
/// its outcome.
fn redirect(r: &ConsentRequest, id: &str, result: &str) -> String {
    let mut u = r.return_to.clone();
    let _pairs = u
        .query_pairs_mut()
        .append_pair("request_id", id)
        .append_pair("result", result);
    u.into()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApproveBody {
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// The facts the page showed, checked again before the passkey is asked and
/// before the approval is written.
fn still_current(node: &Node, action: &Action) -> Result<(), ApiError> {
    match action {
        Action::Release { .. } => Ok(()),
        Action::Upgrade {
            app,
            from_bundle,
            package,
            version,
            mpk,
            ..
        } => {
            let (bundle, target) = node.sys(|s| {
                (
                    s.apps.get(app).map(|a| a.bundle.clone()),
                    s.packages
                        .get(package)
                        .and_then(|p| p.versions.get(version))
                        .map(|v| v.mpk.clone()),
                )
            });
            if bundle.as_ref() != Some(from_bundle) {
                return Err(stale("the app's running release"));
            }
            if target.as_ref() != Some(mpk) {
                return Err(stale(&format!("{package}@{version}")));
            }
            Ok(())
        }
        Action::Outbound {
            package,
            latest_mpk,
            ..
        } => {
            let latest = node.sys(|s| s.packages.get(package)?.latest().map(|v| v.mpk.clone()));
            if latest.as_ref() == Some(latest_mpk) {
                Ok(())
            } else {
                Err(stale(&format!("the latest release of {package}")))
            }
        }
    }
}

/// `POST /consent/{id}/approve`: without `confirm`, the route's own `412
/// CONFIRM_REQUIRED` over the request's frozen arguments; with it, the
/// approval, written as the request's account, and where to go next.
async fn approve(node: &Node, id: &str, body: &[u8]) -> Result<Value, ApiError> {
    let body: ApproveBody = if body.is_empty() {
        ApproveBody { confirm: None }
    } else {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?
    };
    let now = now_secs();
    let req = node.consent.get(id, now)?;
    if req.decision != Decision::Pending {
        return Err(ApiError::new(
            409,
            "ALREADY_DECIDED",
            &format!("this request is {} already", req.decision.label()),
        ));
    }
    node.sys_fresh().await;
    still_current(node, &req.action)?;
    let confirmed = body.confirm.is_some();
    // One approval at a time: the passkey's answer is taken once.
    if confirmed {
        let _r = node
            .consent
            .transition(id, &Decision::Pending, Decision::Deciding, now)?;
    }
    let out = act(node, &req, body.confirm).await;
    match out {
        Ok(result) => {
            let r = node.consent.transition(
                id,
                &Decision::Deciding,
                Decision::Approved { at: now_secs() },
                now_secs(),
            )?;
            Ok(json!({
                "status": "approved",
                "redirect": redirect(&r, id, "approved"),
                "result": result,
            }))
        }
        Err(e) => {
            if confirmed {
                let _r =
                    node.consent
                        .transition(id, &Decision::Deciding, Decision::Pending, now_secs());
            }
            Err(e)
        }
    }
}

/// Run the request's route as its account, through the consent page.
async fn act(
    node: &Node,
    req: &ConsentRequest,
    confirm: Option<ConfirmBody>,
) -> Result<Value, ApiError> {
    let p = Principal {
        caller: Caller::Account {
            account: req.account.clone(),
        },
        session: None,
        api_key: None,
        // This node vouches for the account, as for a session it signed.
        issuer_release: Some(node.own_release()),
        consent_page: true,
    };
    let Json(v) = match &req.action {
        Action::Release {
            scope,
            version,
            change,
        } => {
            let (approved, shown) = match change {
                ReleaseChange::Approve { policy_digest } => (true, Some(policy_digest.as_str())),
                ReleaseChange::Withdraw => (false, None),
            };
            set_release_approval(
                node,
                &p,
                scope.clone(),
                ReleaseApprovalBody {
                    version: version.clone(),
                    approved,
                    confirm,
                },
                shown,
            )
            .await?
        }
        Action::Upgrade {
            app,
            package,
            version,
            service,
            ..
        } => {
            upgrade_app(
                node,
                &p,
                app.clone(),
                UpgradeBody {
                    package: Some(package.clone()),
                    version: Some(version.clone()),
                    service: service.clone(),
                    confirm,
                    ..UpgradeBody::default()
                },
            )
            .await?
        }
        Action::Outbound {
            scope,
            package,
            prefixes,
            ..
        } => {
            set_outbound_approval(
                node,
                &p,
                scope.clone(),
                OutboundApprovalBody {
                    package: package.clone(),
                    prefixes: calimero_primitives::outbound::OutboundSelection::Prefixes(
                        prefixes.clone(),
                    ),
                    replace: false,
                    confirm,
                },
            )
            .await?
        }
    };
    Ok(v)
}

/// `POST /consent/{id}/decline`: nothing is approved; the app learns so.
fn decline(node: &Node, id: &str) -> Result<Value, ApiError> {
    let now = now_secs();
    let r = node
        .consent
        .transition(id, &Decision::Pending, Decision::Declined { at: now }, now)?;
    Ok(json!({ "status": "declined", "redirect": redirect(&r, id, "declined") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(account: &str, expires_at: u64) -> ConsentRequest {
        ConsentRequest {
            account: account.to_owned(),
            package: None,
            action: Action::Release {
                scope: ReleaseScope::Account,
                version: "1.0.0".into(),
                change: ReleaseChange::Approve {
                    policy_digest: "digest".into(),
                },
            },
            model: json!({ "kind": "release" }),
            return_to: Url::parse("https://app.example/back?x=1").unwrap(),
            expires_at,
            decision: Decision::Pending,
        }
    }

    #[test]
    fn requests_expire_are_bounded_per_account_and_decide_once() {
        let store = ConsentRequests::default();
        let now = 1_000;
        let id = store.insert(7, request("alice", now + 10), now).unwrap();
        assert!(is_request_id(&id), "{id}");
        assert!(id.starts_with("7."));
        // Bounded per account, not across accounts.
        for _ in 1..MAX_OPEN_REQUESTS_PER_ACCOUNT {
            let _ = store.insert(7, request("alice", now + 10), now).unwrap();
        }
        let e = store
            .insert(7, request("alice", now + 10), now)
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (429, "RATE_LIMITED"));
        let _ = store.insert(7, request("bob", now + 10), now).unwrap();
        // Decided once: approve, then neither approve nor decline again.
        let r = store
            .transition(&id, &Decision::Pending, Decision::Deciding, now)
            .unwrap();
        assert_eq!(r.decision, Decision::Deciding);
        let e = store
            .transition(&id, &Decision::Pending, Decision::Declined { at: now }, now)
            .unwrap_err();
        assert_eq!(e.code, "ALREADY_DECIDED");
        let r = store
            .transition(
                &id,
                &Decision::Deciding,
                Decision::Approved { at: now },
                now,
            )
            .unwrap();
        assert_eq!(r.decision.label(), "approved");
        assert_eq!(
            redirect(&r, &id, "approved"),
            format!("https://app.example/back?x=1&request_id={id}&result=approved")
        );
        // The result outlives the request's own expiry, then goes.
        assert!(store.get(&id, now + 11).is_ok());
        let e = store.get(&id, now + CONSENT_RESULT_TTL + 1).unwrap_err();
        assert_eq!(e.status, 404);
        // An undecided request expires, and frees its account's slots.
        assert!(store
            .insert(7, request("alice", now + 20), now + 11)
            .is_ok());
    }

    #[test]
    fn the_node_holds_at_most_its_bound() {
        let store = ConsentRequests::default();
        for i in 0..MAX_CONSENT_REQUESTS {
            let _ = store.insert(1, request(&format!("a{i}"), 100), 0).unwrap();
        }
        let e = store.insert(1, request("late", 100), 0).unwrap_err();
        assert_eq!(e.status, 429);
    }

    #[test]
    fn request_ids_are_checked_before_any_lookup() {
        assert!(is_request_id("1.AAAAAAAAAAAAAAAAAAAAAA"));
        assert!(!is_request_id("requests"));
        assert!(!is_request_id("1.short"));
        assert!(!is_request_id("x.AAAAAAAAAAAAAAAAAAAAAA"));
        assert!(!is_request_id("1.AAAAAAAAAAAAAAAAAAAA/A"));
    }

    #[test]
    fn the_page_allows_exactly_its_own_script_and_style() {
        let (html, csp) = &*PAGE;
        assert!(html.contains(PAGE_JS) && html.contains(PAGE_CSS));
        assert!(!html.contains("/*SCRIPT*/") && !html.contains("/*STYLE*/"));
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(csp.contains("default-src 'none'"), "{csp}");
        assert!(!csp.contains("unsafe-inline"), "{csp}");
        assert!(csp.contains("connect-src 'self'"), "{csp}");
    }

    #[test]
    fn a_release_request_withdraws_only_when_it_says_so() {
        let parse = |subject: Value| {
            serde_json::from_value::<CreateBody>(json!({
                "kind": "release",
                "scope": "account",
                "subject": subject,
                "returnTo": "https://app.example/",
            }))
        };
        let withdraw = |subject: Value| match parse(subject).unwrap() {
            CreateBody::Release { subject, .. } => subject.withdraw,
            CreateBody::Upgrade { .. } | CreateBody::Outbound { .. } => unreachable!(),
        };
        assert!(
            !withdraw(json!({ "version": "1.0.0" })),
            "approval by default"
        );
        assert!(!withdraw(json!({ "version": "1.0.0", "withdraw": false })));
        assert!(withdraw(json!({ "version": "1.0.0", "withdraw": true })));
        assert!(parse(json!({ "version": "1.0.0", "withdraw": "yes" })).is_err());
        assert!(
            parse(json!({ "version": "1.0.0", "approved": false })).is_err(),
            "unknown fields are still refused"
        );
    }

    #[test]
    fn a_policy_digest_is_stable_and_covers_every_register() {
        let mut profiles = BTreeMap::new();
        let _prev = profiles.insert(
            "gcp-small".to_owned(),
            calimero_tee_release::ProfileMeasurements {
                allowed_mrtd: vec!["aa".into()],
                allowed_rtmr0: vec![],
                allowed_rtmr1: vec!["bb".into()],
                allowed_rtmr2: vec!["cc".into()],
                allowed_rtmr3: vec!["dd".into()],
            },
        );
        let r = calimero_tee_release::NodeRelease {
            version: "1.2.3".into(),
            profiles,
        };
        let a = policy_digest(&r);
        assert_eq!(a, policy_digest(&r.clone()));
        let mut other = r;
        if let Some(p) = other.profiles.get_mut("gcp-small") {
            p.allowed_rtmr3 = vec!["ee".into()];
        }
        assert_ne!(a, policy_digest(&other));
    }
}
