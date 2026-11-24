//! Outbound HTTPS requests from a run, and the secrets they may carry.
//!
//! A component's action asks the node for an HTTP request; the node hands the
//! request's bytes to the [`OutboundHost`] this crate makes for the run
//! ([`OutboundService::for_run`]). It makes a request only when every gate holds:
//!
//! 1. **The operator turned it on** (`[outbound] enabled`, off by
//!    default) and allows the request's host (`hosts`: listed, or `"*"` for
//!    any host an app declares).
//! 2. **The app declared a prefix covering the URL** in its signed bundle
//!    manifest (`outbound`): same host, and a path at or below the prefix's
//!    at a `/`. A run gets a client only when its app declares one.
//! 3. **Egress is on for the app**: neither its organization nor the instance
//!    turned it off. The node asks before every request ([`egress_gated`]), so
//!    a switch turned off reaches a run already going; while it is off every
//!    request is refused with [`EGRESS_OFF`], whatever the manifest declares
//!    and whatever was approved.
//! 4. **The run writes on this node** (a call, a follow-up, a timer, a
//!    delivery) or is a component's action: never a view, `init`, a migration
//!    or a compute run. The node and its component host decide that.
//! 5. **The request fits**: `https` on port 443 to a DNS name that resolves
//!    only to public addresses, no fragment, the size limits in
//!    `calimero_primitives::outbound`, no redirect followed. How many one run
//!    may send is the component host's bound (`MAX_HTTP_REQUESTS`).
//!
//! # Secrets
//!
//! A request names a secret as `{{secret:NAME}}` in a header value or the
//! body. The node asks its [`SecretProvider`] for `NAME` **for the app the run
//! belongs to**, identified by the node (package and signer of the installed
//! application), never by the guest. A secret is substituted only into a
//! request to one of the hosts it is bound to; any copy of it in the response,
//! as sent or in a common encoding (the `redact` module), is replaced with
//! `[redacted]` before the guest sees it. A failure reaches the guest and the
//! log only as its [`FailureKind`], never in the transport's words. The guest never
//! reads a secret, so it cannot write one into state, where it would
//! replicate to every member.
//!
//! Two providers: [`EnvSecrets`], the operator's own (environment variables
//! named in the node config, each scoped to app packages and hosts), and
//! [`SidecarSecrets`], a loopback endpoint the node asks. The sidecar is the
//! seam for a managed deployment: a secrets service next to the node (inside
//! the same TEE, say) answers for the secrets a deployer set for their app,
//! for every organization that runs it.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use borsh::BorshDeserialize;
use calimero_primitives::outbound::{
    OutboundMethod, OutboundRequest, OutboundResponse, MAX_HEADERS, MAX_HEADER_BYTES,
    MAX_REQUEST_BODY, MAX_RESPONSE_BODY, MAX_URL, SECRET_PREFIX, SECRET_SUFFIX, TIMEOUT_MS,
};
use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zeroize::Zeroizing;

mod connector;
mod redact;
mod settings;

pub use connector::*;
use redact::Redactor;
pub use redact::{
    MAX_ENCODED_SECRET_BYTES, MAX_REDACTION_PATTERNS, MAX_SECRETS_PER_REQUEST,
    MIN_ENCODED_SECRET_BYTES, PATTERNS_PER_SECRET,
};
pub use settings::*;

/// Sends a run's outbound request: borsh `OutboundRequest` in, borsh
/// `OutboundResponse` out (`calimero_primitives::outbound`).
pub trait OutboundHost: Send + Sync + fmt::Debug {
    /// Send `request`, or say why not.
    ///
    /// # Errors
    /// A message for the guest: a refused or malformed request, a failed
    /// connection, a timeout, a response over the size limit.
    fn send(&self, request: &[u8]) -> Result<Vec<u8>, String>;
}

/// The most prefixes one approval holds.
pub const MAX_OUTBOUND_APPROVALS: usize = 64;

/// Headers a request may not set: the node sets what it must itself, and
/// these would let a request reroute itself or carry a credential the operator
/// never saw.
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
    "te",
    "upgrade",
    "cookie",
    "proxy-authorization",
    "proxy-connection",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-real-ip",
];

/// The app a run belongs to, as this node installed it. Never from the guest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppIdentity {
    /// The bundle's package, e.g. `com.calimero.mero-forms`.
    pub package: String,
    /// The bundle signer's `did:key`.
    pub signer: String,
    /// The secrets the release declares (its signed manifest's `secrets`), by name, with
    /// the hosts each may go to. `Some`: only these names are filled, each only into a
    /// request to one of its hosts (and to a host the operator bound it to). `None`: a
    /// release that declares none, filled from the operator's bindings alone, as before.
    pub secrets: Option<BTreeMap<String, Vec<String>>>,
    /// Values set for declared names in merod: the instance owner's own (`PUT
    /// /apps/{app}/secrets/{name}`), else its deployment managers' (`PUT
    /// /deployments/{id}/secrets/{name}`). One wins over the operator's for its name, and
    /// goes only to that name's declared hosts. Empty for a release that declares none.
    pub own: OwnSecrets,
}

/// The secret values an instance's owner or its deployment's managers set, by name. Its
/// `Debug` names them, never a value.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OwnSecrets(pub BTreeMap<String, Zeroizing<String>>);

impl fmt::Debug for OwnSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// What the approver (the app's organization, or the operator where it holds
/// outbound approvals) approved for the app's publisher, as the system group
/// keeps it for that signer (`SystemState::outbound_approval` in merod).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Approval {
    /// The approved URL prefixes.
    pub prefixes: Vec<String>,
    /// The prefixes the approver was shown, approved or not.
    pub reviewed: Vec<String>,
}

/// The start of every refusal for a prefix the approver has not approved.
/// Stable: an app matches on it to tell a member to ask an admin.
pub const NOT_APPROVED: &str = calimero_primitives::outbound::OutboundRefusal::NotApproved.word();

/// The start of every refusal for a prefix the approver has never been asked
/// about, because a later version of the app declared it. Stable: an app
/// matches on it to show an admin what to approve.
pub const AWAITING_APPROVAL: &str =
    calimero_primitives::outbound::OutboundRefusal::AwaitingApproval.word();

/// The start of every refusal while the app's organization, or the instance,
/// turned egress off. Stable: an app matches on it to tell its user why
/// nothing can leave the cluster.
pub const EGRESS_OFF: &str = calimero_primitives::outbound::OutboundRefusal::EgressOff.word();

/// Whether egress is off for one app right now: `Some(who)` names who turned it
/// off ("its organization", "the instance"). The node answers it from its
/// replicated system state.
pub type EgressCheck = Arc<dyn Fn() -> Option<&'static str> + Send + Sync>;

/// `inner`, asking `off` before each request and refusing it with
/// [`EGRESS_OFF`] while egress is off, before any other gate: nothing is
/// resolved, filled in or sent.
#[must_use]
pub fn egress_gated(inner: Arc<dyn OutboundHost>, off: EgressCheck) -> Arc<dyn OutboundHost> {
    Arc::new(EgressGated { inner, off })
}

/// [`egress_gated`]'s host.
struct EgressGated {
    inner: Arc<dyn OutboundHost>,
    off: EgressCheck,
}

impl fmt::Debug for EgressGated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EgressGated")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl OutboundHost for EgressGated {
    fn send(&self, request: &[u8]) -> Result<Vec<u8>, String> {
        if let Some(who) = (self.off)() {
            return Err(format!(
                "{EGRESS_OFF}: {who} turned egress off, so no request leaves the cluster \
                 for this app, whatever was approved"
            ));
        }
        self.inner.send(request)
    }
}

/// A secret, and the hosts it may be sent to.
pub struct Secret {
    pub value: Zeroizing<String>,
    pub hosts: Vec<String>,
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret")
            .field("value", &"…")
            .field("hosts", &self.hosts)
            .finish()
    }
}

/// Where the node gets the secrets an app's requests name.
pub trait SecretProvider: Send + Sync + fmt::Debug {
    /// `name` for `app`, or `None` when there is none for that app.
    ///
    /// # Errors
    /// The provider could not be asked.
    fn secret(&self, app: &AppIdentity, name: &str) -> EyreResult<Option<Secret>>;
}

/// Secrets from environment variables the operator names in the node config.
#[derive(Debug)]
pub struct EnvSecrets {
    secrets: BTreeMap<String, EnvSecret>,
}

struct EnvSecret {
    value: Zeroizing<String>,
    packages: BTreeSet<String>,
    signers: BTreeSet<String>,
    hosts: Vec<String>,
}

/// Never the value: the service is `Debug`, and a log line must not carry a
/// secret.
impl fmt::Debug for EnvSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvSecret")
            .field("value", &"[redacted]")
            .field("packages", &self.packages)
            .field("signers", &self.signers)
            .field("hosts", &self.hosts)
            .finish()
    }
}

impl EnvSecrets {
    /// Reads every configured variable now, so a missing one fails the node's
    /// start rather than a request.
    ///
    /// # Errors
    /// A variable is unset or empty, or a secret names no package, no signer
    /// or no host.
    pub fn from_settings(settings: &OutboundSettings) -> EyreResult<Self> {
        let mut secrets = BTreeMap::new();
        for (name, entry) in &settings.env_secrets {
            if !is_secret_name(name) {
                bail!("outbound secret name {name:?} must be A-Z, 0-9 and _");
            }
            if entry.packages.is_empty() || entry.signers.is_empty() || entry.hosts.is_empty() {
                bail!("outbound secret {name} must name the app packages, their signers and the hosts it is for");
            }
            let value = std::env::var(&entry.env)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    eyre!(
                        "outbound secret {name}: environment variable {} is unset",
                        entry.env
                    )
                })?;
            let _ = secrets.insert(
                name.clone(),
                EnvSecret {
                    value: Zeroizing::new(value),
                    packages: entry.packages.iter().cloned().collect(),
                    signers: entry.signers.iter().cloned().collect(),
                    hosts: normalize_hosts(&entry.hosts)?,
                },
            );
        }
        Ok(Self { secrets })
    }
}

impl SecretProvider for EnvSecrets {
    fn secret(&self, app: &AppIdentity, name: &str) -> EyreResult<Option<Secret>> {
        Ok(self
            .secrets
            .get(name)
            .filter(|s| s.packages.contains(&app.package) && s.signers.contains(&app.signer))
            .map(|s| Secret {
                value: s.value.clone(),
                hosts: s.hosts.clone(),
            }))
    }
}

/// Secrets from a loopback service next to the node.
///
/// The node `POST`s `{"package", "signer", "name"}` to `url` with
/// `Authorization: Bearer <token>`, and expects `200 {"value", "hosts"}` or
/// `404`. The service decides which secrets an app gets; the node still binds
/// each to the hosts the answer names, and to its own host list.
pub struct SidecarSecrets {
    url: reqwest::Url,
    token: Zeroizing<String>,
}

impl fmt::Debug for SidecarSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SidecarSecrets")
            .field("url", &self.url.as_str())
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct SidecarAsk<'a> {
    package: &'a str,
    signer: &'a str,
    name: &'a str,
}

#[derive(Deserialize)]
struct SidecarAnswer {
    value: String,
    hosts: Vec<String>,
}

impl SidecarSecrets {
    /// # Errors
    /// The URL is not `http` on a loopback address, or the token variable is unset.
    pub fn new(url: &str, token_env: &str) -> EyreResult<Self> {
        let url = reqwest::Url::parse(url)?;
        let loopback = match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(d)) => d == "localhost",
            None => false,
        };
        if url.scheme() != "http" || !loopback {
            bail!("the outbound secrets sidecar must be http on a loopback address, got {url}");
        }
        let token = std::env::var(token_env)
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                eyre!("outbound secrets sidecar: environment variable {token_env} is unset")
            })?;
        Ok(Self {
            url,
            token: Zeroizing::new(token),
        })
    }
}

impl SecretProvider for SidecarSecrets {
    fn secret(&self, app: &AppIdentity, name: &str) -> EyreResult<Option<Secret>> {
        let ask = serde_json::to_vec(&SidecarAsk {
            package: &app.package,
            signer: &app.signer,
            name,
        })?;
        let (status, body) = run_blocking(async {
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_millis(TIMEOUT_MS))
                .build()?;
            let response = client
                .post(self.url.clone())
                .bearer_auth(self.token.as_str())
                .header("content-type", "application/json")
                .body(ask)
                .send()
                .await?;
            let status = response.status().as_u16();
            let body = Zeroizing::new(read_capped(response, MAX_HEADER_BYTES * 8).await?);
            Ok::<_, eyre::Report>((status, body))
        })??;
        match status {
            200 => {
                let answer: SidecarAnswer = serde_json::from_slice(&body)?;
                Ok(Some(Secret {
                    value: Zeroizing::new(answer.value),
                    hosts: normalize_hosts(&answer.hosts)?,
                }))
            }
            404 => Ok(None),
            other => bail!("the outbound secrets sidecar answered {other}"),
        }
    }
}

/// A request that passed every check, with its secrets filled in.
pub struct Prepared {
    pub method: OutboundMethod,
    pub url: reqwest::Url,
    pub host: String,
    pub headers: Vec<(String, Zeroizing<String>)>,
    pub body: Zeroizing<Vec<u8>>,
    /// The secrets substituted, once per substitution.
    secrets: Vec<Zeroizing<String>>,
    /// Their redaction from the response.
    redactor: Redactor,
}

/// Header names and sizes only: the values and the body carry secrets.
impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("method", &self.method)
            .field("url", &self.url.as_str())
            .field(
                "headers",
                &self.headers.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .field("body_len", &self.body.len())
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// Sends a prepared request. Real HTTPS in production; a fake in tests.
pub trait Transport: Send + Sync + fmt::Debug {
    /// # Errors
    /// The request failed, timed out, or answered more than the limit.
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)>;
}

/// Cloudflare Turnstile's token check. The node posts a client's token here, through
/// the app's own outbound gate, before it runs a `captcha(turnstile)` method.
pub const TURNSTILE_VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// The node's outbound policy: the operator's hosts and secrets.
#[derive(Debug)]
pub struct OutboundService {
    hosts: BTreeSet<String>,
    /// `hosts` held `"*"`: any host an app declares, still public-only.
    any_host: bool,
    secrets: Option<Arc<dyn SecretProvider>>,
    transport: Arc<dyn Transport>,
}

impl OutboundService {
    /// The service `[outbound]` describes, or `None` when it is off.
    ///
    /// # Errors
    /// A host is not a DNS name, a secret variable is unset, or the sidecar is
    /// not on a loopback address.
    pub fn from_settings(settings: &OutboundSettings) -> EyreResult<Option<Arc<Self>>> {
        if !settings.enabled {
            return Ok(None);
        }
        if settings.hosts.is_empty() {
            bail!("[outbound] is enabled but lists no hosts");
        }
        #[cfg_attr(not(feature = "outbound-testing"), allow(unused_mut))]
        let mut secrets: Option<Arc<dyn SecretProvider>> =
            match (&settings.sidecar, settings.env_secrets.is_empty()) {
                (Some(_), false) => {
                    bail!("[outbound] takes env_secrets or a sidecar, not both")
                }
                (Some(sidecar), true) => Some(Arc::new(SidecarSecrets::new(
                    &sidecar.url,
                    &sidecar.token_env,
                )?)),
                (None, false) => Some(Arc::new(EnvSecrets::from_settings(settings)?)),
                (None, true) => None,
            };
        let any_host = settings.hosts.iter().any(|h| h.trim() == "*");
        let named: Vec<String> = settings
            .hosts
            .iter()
            .filter(|h| h.trim() != "*")
            .cloned()
            .collect();
        #[cfg_attr(not(feature = "outbound-testing"), allow(unused_mut))]
        let mut transport: Arc<dyn Transport> = Arc::new(HttpsTransport::public());
        #[cfg(feature = "outbound-testing")]
        if let Some(testing) = &settings.testing {
            testing::apply(settings, testing, &mut secrets, &mut transport)?;
        }
        Ok(Some(Arc::new(Self {
            hosts: normalize_hosts(&named)?.into_iter().collect(),
            any_host,
            secrets,
            transport,
        })))
    }

    /// A service with the given parts, for tests.
    #[cfg(test)]
    pub(crate) fn with_parts(
        hosts: &[&str],
        secrets: Option<Arc<dyn SecretProvider>>,
        transport: Arc<dyn Transport>,
    ) -> Arc<Self> {
        Arc::new(Self {
            hosts: hosts
                .iter()
                .filter(|h| **h != "*")
                .map(|h| (*h).to_owned())
                .collect(),
            any_host: hosts.contains(&"*"),
            secrets,
            transport,
        })
    }

    /// The client for one run of `app`, which declared the URL prefixes
    /// `declared`, whose approver approved `approval` for its publisher
    /// (`None`: nothing yet); `None` when it declared none this node can use.
    ///
    /// A run gets a client even with no approval, so a request is refused with
    /// a reason the app can show rather than trapping.
    #[must_use]
    pub fn for_run(
        self: &Arc<Self>,
        app: AppIdentity,
        declared: Vec<String>,
        approval: Option<Approval>,
        context: String,
    ) -> Option<Arc<dyn OutboundHost>> {
        self.for_run_counted(app, declared, approval, context, Arc::default())
    }

    /// [`Self::for_run`], adding one to `sent` for each request it hands the
    /// transport: a refused or malformed request left nothing and counts
    /// nothing. The node meters an app's outbound requests with it.
    #[must_use]
    pub fn for_run_counted(
        self: &Arc<Self>,
        app: AppIdentity,
        declared: Vec<String>,
        approval: Option<Approval>,
        context: String,
        sent: Arc<AtomicU64>,
    ) -> Option<Arc<dyn OutboundHost>> {
        self.run_outbound(app, declared, BTreeMap::new(), approval, context, sent)
    }

    /// The raw client for a run, refusing every host of `reserved` (a declared
    /// connector's host, by the connector's name); `None` when the app
    /// declared no prefix this node can use.
    fn run_outbound(
        self: &Arc<Self>,
        app: AppIdentity,
        declared: Vec<String>,
        reserved: BTreeMap<String, String>,
        approval: Option<Approval>,
        context: String,
        sent: Arc<AtomicU64>,
    ) -> Option<Arc<dyn OutboundHost>> {
        let declared: BTreeSet<String> = declared
            .into_iter()
            .filter(|u| u.len() <= MAX_URL && prefix(u).is_some())
            .collect();
        if declared.is_empty() {
            return None;
        }
        Some(Arc::new(RunOutbound {
            service: Arc::clone(self),
            app,
            declared,
            reserved,
            approval,
            context,
            sent,
        }))
    }

    /// Checks `request` against every rule and fills in its secrets.
    ///
    /// # Errors
    /// The reason it is refused, worded for the app's developer.
    pub fn prepare(
        &self,
        app: &AppIdentity,
        declared: &BTreeSet<String>,
        approval: Option<&Approval>,
        request: OutboundRequest,
    ) -> EyreResult<Prepared> {
        if request.url.len() > MAX_URL {
            bail!("the URL is longer than {MAX_URL} bytes");
        }
        let url = reqwest::Url::parse(&request.url)?;
        if url.scheme() != "https" {
            bail!("only https is allowed");
        }
        if url.fragment().is_some() {
            bail!("a URL with a fragment is not allowed");
        }
        if !url.username().is_empty() || url.password().is_some() {
            bail!("a URL with credentials is not allowed");
        }
        if url.port().is_some_and(|p| p != 443) {
            bail!("only port 443 is allowed");
        }
        let host = match url.host() {
            Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
            _ => bail!("the URL must name a host by DNS name, not an address"),
        };
        if !self.any_host && !self.hosts.contains(&host) {
            bail!("{host} is not a host this node's operator allows");
        }
        let Some(under) = covering(declared, &url) else {
            bail!(
                "{} is under no URL prefix this app's manifest declares under `outbound`",
                request.url
            );
        };
        let approved = approval.is_some_and(|a| {
            a.prefixes
                .iter()
                .filter_map(|p| prefix(p))
                .any(|p| covers(&p, &url))
        });
        if !approved {
            // Reviewed means the approver saw this prefix: left out of the set,
            // it was declined or revoked. Never reviewed, under an approval the
            // approver did make, it is new in a later version of the app.
            if approval.is_some_and(|a| !a.reviewed.iter().any(|r| r == under)) {
                bail!(
                    "{AWAITING_APPROVAL}: {under} is a URL prefix this app declares that \
                     its approver (the app's organization, or the operator) has not \
                     been asked to approve yet"
                );
            }
            bail!("{NOT_APPROVED}: {under} is not approved for this app");
        }
        if request.headers.len() > MAX_HEADERS {
            bail!("at most {MAX_HEADERS} headers");
        }
        if request.body.len() > MAX_REQUEST_BODY {
            bail!("the body is longer than {MAX_REQUEST_BODY} bytes");
        }
        if request.method == OutboundMethod::Get && !request.body.is_empty() {
            bail!("a GET has no body");
        }

        let mut used = Vec::new();
        let mut headers = Vec::with_capacity(request.headers.len());
        for (name, value) in request.headers {
            let lower = name.to_ascii_lowercase();
            if name.is_empty()
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || name.len() + value.len() > MAX_HEADER_BYTES
                || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
            {
                bail!("header {name:?} is malformed or too long");
            }
            if FORBIDDEN_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") {
                bail!("header {name} may not be set");
            }
            let value = self.substitute(app, &host, value.into_bytes(), &mut used)?;
            let value = String::from_utf8(value.to_vec())
                .map_err(|_| eyre!("header {name} is not UTF-8"))?;
            headers.push((name, Zeroizing::new(value)));
        }
        let body = self.substitute(app, &host, request.body, &mut used)?;
        let redactor = Redactor::new(&used)?;
        Ok(Prepared {
            method: request.method,
            url,
            host,
            headers,
            body,
            secrets: used,
            redactor,
        })
    }

    /// Replaces every `{{secret:NAME}}` in `bytes`.
    fn substitute(
        &self,
        app: &AppIdentity,
        host: &str,
        bytes: Vec<u8>,
        used: &mut Vec<Zeroizing<String>>,
    ) -> EyreResult<Zeroizing<Vec<u8>>> {
        let prefix = SECRET_PREFIX.as_bytes();
        let suffix = SECRET_SUFFIX.as_bytes();
        let mut out = Zeroizing::new(Vec::with_capacity(bytes.len()));
        let mut rest = &bytes[..];
        while let Some(start) = find(rest, prefix) {
            out.extend_from_slice(&rest[..start]);
            let after = &rest[start + prefix.len()..];
            let end =
                find(after, suffix).ok_or_else(|| eyre!("an unterminated {SECRET_PREFIX}"))?;
            let name = core::str::from_utf8(&after[..end])
                .map_err(|_| eyre!("a malformed secret name"))?;
            if !is_secret_name(name) {
                bail!("secret name {name:?} must be A-Z, 0-9 and _");
            }
            if let Some(declared) = &app.secrets {
                let hosts = declared.get(name).ok_or_else(|| {
                    eyre!("the release declares no secret {name} (its manifest's `secrets`)")
                })?;
                if !hosts.iter().any(|h| h == host) {
                    bail!("the release declares secret {name} for other hosts than {host}");
                }
            }
            // The instance's own value for a declared name, checked against its declared
            // hosts above, wins over the operator's.
            if let Some(value) = app.own.0.get(name).filter(|_| app.secrets.is_some()) {
                out.extend_from_slice(value.as_bytes());
                used.push(value.clone());
                rest = &after[end + suffix.len()..];
                continue;
            }
            let provider = self.secrets.as_ref().ok_or_else(|| {
                eyre!(
                    "no value for secret {name}: this node has no secrets for outbound requests, \
                     and neither the app's owner nor its deployment set one"
                )
            })?;
            let secret = provider
                .secret(app, name)?
                .ok_or_else(|| eyre!("this node has no secret {name} for {}", app.package))?;
            if !secret.hosts.iter().any(|h| h == host) {
                bail!("secret {name} may not be sent to {host}");
            }
            out.extend_from_slice(secret.value.as_bytes());
            used.push(secret.value);
            rest = &after[end + suffix.len()..];
        }
        out.extend_from_slice(rest);
        Ok(out)
    }
}

/// One run's client: the service, bound to the run's app and its declared URLs.
#[derive(Debug)]
struct RunOutbound {
    service: Arc<OutboundService>,
    app: AppIdentity,
    declared: BTreeSet<String>,
    /// Hosts reached only through a connector the app declares, by its name.
    reserved: BTreeMap<String, String>,
    approval: Option<Approval>,
    context: String,
    /// Requests handed to the transport.
    sent: Arc<AtomicU64>,
}

impl OutboundHost for RunOutbound {
    fn send(&self, request: &[u8]) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        let request = OutboundRequest::try_from_slice(request)
            .map_err(|e| format!("a malformed outbound request: {e}"))?;
        // The host alone, for the log: the rest of the URL (its path and query) is the
        // app's, and can carry a user's data or a token.
        let host = reqwest::Url::parse(&request.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
        let reserved = host
            .as_deref()
            .map(|h| h.trim_end_matches('.'))
            .and_then(|h| Some((self.reserved.get(h)?, h)));
        if let Some((connector, reserved_host)) = reserved {
            warn!(
                context = %self.context,
                package = %self.app.package,
                host = reserved_host,
                connector = %connector,
                refusal = "ConnectorHost",
                "outbound request refused"
            );
            return Err(format!(
                "{reserved_host} is reached only through the {connector} connector this app \
                 declares, never by a raw request"
            ));
        }
        let prepared =
            match self
                .service
                .prepare(&self.app, &self.declared, self.approval.as_ref(), request)
            {
                Ok(prepared) => prepared,
                Err(reason) => {
                    // The node's name for the refusal, never its words: they quote the
                    // request (its URL, a header or secret name the guest wrote) and this
                    // line ships to the operator's log. The guest gets the words.
                    let reason = reason.to_string();
                    warn!(
                        context = %self.context,
                        package = %self.app.package,
                        host = host.as_deref().unwrap_or("-"),
                        refusal = refusal_kind(&reason),
                        "outbound request refused"
                    );
                    return Err(reason);
                }
            };
        let (status, body) = self.service.transmit(
            &self.app,
            &self.context,
            &self.sent,
            &self.declared,
            &prepared,
            started,
        )?;
        borsh::to_vec(&OutboundResponse { status, body }).map_err(|e| e.to_string())
    }
}

impl OutboundService {
    /// Sends a prepared request, counts it in `sent`, logs it (with the prefix of
    /// `declared` that covers it, never its path) and redacts its secrets from
    /// the answer: every raw request and connector call ends here.
    fn transmit(
        &self,
        app: &AppIdentity,
        context: &str,
        sent: &AtomicU64,
        declared: &BTreeSet<String>,
        prepared: &Prepared,
        started: Instant,
    ) -> Result<(u16, Vec<u8>), String> {
        let _before = sent.fetch_add(1, Ordering::Relaxed);
        let (status, body) = match self.transport.send(prepared) {
            Ok(answer) => answer,
            Err(error) => {
                // Only the kind the node names, never the transport's words: they can
                // quote what was sent (a secret, in any encoding), and this line ships
                // to the operator's log while the reason goes to the guest.
                let kind = FailureKind::of(&error);
                warn!(
                    context = %context,
                    package = %app.package,
                    host = %prepared.host,
                    path = %prepared.url.path(),
                    kind = kind.as_str(),
                    "outbound request failed"
                );
                return Err(format!(
                    "the request to {} failed: {}",
                    prepared.host,
                    kind.as_str()
                ));
            }
        };
        let body = prepared.redactor.redact(body);
        info!(
            context = %context,
            package = %app.package,
            host = %prepared.host,
            // The manifest's prefix that covers the URL, not its path: a path can
            // name a user (`/users/{email}`), a prefix is what the app declared.
            prefix = covering(declared, &prepared.url).map_or("-", String::as_str),
            status,
            request_bytes = prepared.body.len(),
            response_bytes = body.len(),
            secrets = prepared.secrets.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "outbound request"
        );
        Ok((status, body))
    }
}

/// HTTPS through reqwest: no proxy, no redirect, a capped response, and in
/// production a resolver that answers only public addresses.
struct HttpsTransport {
    resolver: Arc<dyn reqwest::dns::Resolve>,
    /// Roots trusted instead of the built-in ones; only tests set any.
    roots: Vec<reqwest::Certificate>,
    timeout: Duration,
}

impl fmt::Debug for HttpsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsTransport")
            .field("roots", &self.roots.len())
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl HttpsTransport {
    /// What a node uses: public addresses only, the built-in roots,
    /// `TIMEOUT_MS`.
    fn public() -> Self {
        Self {
            resolver: Arc::new(PublicOnly),
            roots: Vec::new(),
            timeout: Duration::from_millis(TIMEOUT_MS),
        }
    }
}

impl Transport for HttpsTransport {
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        run_blocking(async {
            let mut client = reqwest::Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver2(Arc::clone(&self.resolver))
                .connect_timeout(self.timeout)
                .timeout(self.timeout);
            if !self.roots.is_empty() {
                client = client.tls_built_in_root_certs(false);
                for root in &self.roots {
                    client = client.add_root_certificate(root.clone());
                }
            }
            let client = client.build()?;
            let method = match request.method {
                OutboundMethod::Get => reqwest::Method::GET,
                OutboundMethod::Post => reqwest::Method::POST,
                OutboundMethod::Put => reqwest::Method::PUT,
                OutboundMethod::Patch => reqwest::Method::PATCH,
                OutboundMethod::Delete => reqwest::Method::DELETE,
            };
            let mut builder = client.request(method, request.url.clone());
            if !request.body.is_empty() {
                builder = builder.body(request.body.to_vec());
            }
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            let response = builder.send().await?;
            let status = response.status().as_u16();
            let body = read_capped(response, MAX_RESPONSE_BODY).await?;
            Ok((status, body))
        })?
    }
}

/// A resolver that refuses a name with any non-public address, so a request
/// never reaches the node's own network, whatever the name resolves to.
#[derive(Debug)]
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 443))
                .await?
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} resolves to nothing").into());
            }
            if let Some(bad) = addrs.iter().find(|a| !is_public(a.ip())) {
                return Err(format!(
                    "{host} resolves to {}, which is not a public address",
                    bad.ip()
                )
                .into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Whether `ip` is a globally routable unicast address.
#[must_use]
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b)) // shared address space
                || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
                || (a == 198 && (b == 18 || b == 19)) // benchmarking
                || a >= 240) // reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local
                || (seg[0] & 0xffc0) == 0xfe80 // link-local
                || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
                || (seg[0] == 0x0064 && seg[1] == 0xff9b) // NAT64: an IPv4 address behind it
                || seg[0] == 0x2002) // 6to4: likewise
        }
    }
}

/// Why a request to an upstream failed, as the node names it. Only this reaches
/// the guest and the node's log: the transport's own words can quote the request,
/// secrets included, and an upstream controls part of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// No answer within the timeout.
    Timeout,
    /// No connection: the name did not resolve to a public address, or the
    /// connection or TLS handshake failed.
    Connect,
    /// The answer was longer than [`MAX_RESPONSE_BODY`].
    TooLarge,
    /// The upstream answered with a redirect, which is never followed.
    Redirect,
    /// Anything else.
    Other,
}

impl FailureKind {
    /// The kind of `error`, from its typed causes only.
    #[must_use]
    pub fn of(error: &eyre::Report) -> Self {
        for cause in error.chain() {
            if cause.downcast_ref::<ResponseTooLarge>().is_some() {
                return Self::TooLarge;
            }
            if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
                if e.is_timeout() {
                    return Self::Timeout;
                }
                if e.is_redirect() {
                    return Self::Redirect;
                }
                if e.is_connect() {
                    return Self::Connect;
                }
            }
        }
        Self::Other
    }

    /// The kind's name, as logged and as the guest reads it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::TooLarge => "too large",
            Self::Redirect => "redirect",
            Self::Other => "other",
        }
    }
}

/// An answer longer than the cap.
#[derive(Debug)]
struct ResponseTooLarge(usize);

impl fmt::Display for ResponseTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the response is longer than {} bytes", self.0)
    }
}

impl std::error::Error for ResponseTooLarge {}

async fn read_capped(mut response: reqwest::Response, max: usize) -> EyreResult<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(ResponseTooLarge(max).into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > max {
            return Err(ResponseTooLarge(max).into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Runs `future` to completion on a runtime of its own. A run executes on a
/// blocking thread, and each request gets a fresh client, so no connection is
/// shared between runs or apps.
fn run_blocking<T: Send>(future: impl core::future::Future<Output = T> + Send) -> EyreResult<T> {
    let run = move || -> EyreResult<T> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(runtime.block_on(future))
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return run();
    };
    // Called from a runtime's thread (merod runs WASM on its workers): a
    // runtime cannot start inside another, so the request gets a thread of its
    // own, and a multi-threaded runtime moves its other tasks off this one.
    let on_thread = move || {
        std::thread::scope(|s| {
            s.spawn(run)
                .join()
                .map_err(|_| eyre!("the outbound request panicked"))?
        })
    };
    match handle.runtime_flavor() {
        tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(on_thread),
        _ => on_thread(),
    }
}

/// A declared prefix this node can match against: `https`, a DNS host, no
/// credentials, query or fragment. `None` for anything else, which then
/// covers nothing.
/// The prefix in `declared` that covers `url`, if any.
fn covering<'a>(declared: &'a BTreeSet<String>, url: &reqwest::Url) -> Option<&'a String> {
    declared
        .iter()
        .find(|d| prefix(d).is_some_and(|p| covers(&p, url)))
}

/// A refusal as the node's log names it: the word an app matches on
/// ([`OutboundRefusal::word`]) for an approval refusal, `Invalid` for any other
/// (a URL, header, size or secret the app got wrong). The full reason goes
/// to the guest only.
///
/// [`OutboundRefusal::word`]: calimero_primitives::outbound::OutboundRefusal::word
#[must_use]
pub fn refusal_kind(reason: &str) -> &'static str {
    calimero_primitives::outbound::OutboundRefusal::of(reason).map_or(
        "Invalid",
        calimero_primitives::outbound::OutboundRefusal::word,
    )
}

fn prefix(declared: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(declared).ok()?;
    let usable = url.scheme() == "https"
        && matches!(url.host(), Some(url::Host::Domain(_)))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    usable.then_some(url)
}

/// Whether `prefix` covers `url`: the same host and port, and a path equal to
/// the prefix's or below it at a `/`. Both paths are the parser's normalized
/// ones, so a `..` cannot climb out of the prefix.
fn covers(prefix: &reqwest::Url, url: &reqwest::Url) -> bool {
    if prefix.host_str() != url.host_str()
        || prefix.port_or_known_default() != url.port_or_known_default()
    {
        return false;
    }
    let (base, path) = (prefix.path(), url.path());
    path == base
        || (base.ends_with('/') && path.starts_with(base))
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn is_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn normalize_hosts(hosts: &[String]) -> EyreResult<Vec<String>> {
    hosts
        .iter()
        .map(|h| {
            let h = h.trim().to_ascii_lowercase();
            let valid = !h.is_empty()
                && h.len() <= 253
                && h.contains('.')
                && h.parse::<IpAddr>().is_err()
                && h.split('.').all(|label| {
                    !label.is_empty()
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                });
            if valid {
                Ok(h)
            } else {
                Err(eyre!("{h:?} is not a DNS host name"))
            }
        })
        .collect()
}

/// The prefixes and the reviewed list an approval holds after an approver
/// approves `selection` of an app declaring `declared`.
///
/// Adding (`replace = false`, an install or upgrade) keeps every prefix already
/// approved; replacing sets exactly `selection` (how a prefix is revoked). What
/// was reviewed only grows: the app's whole declared list was on the screen the
/// admin answered, so a prefix of it they left out reads as not approved, never
/// as awaiting approval.
///
/// # Errors
/// A selected prefix is not an `https` URL prefix, or the result is longer than
/// `MAX_OUTBOUND_APPROVALS`.
pub fn merge_approval(
    existing: Option<&Approval>,
    declared: &[String],
    selection: &calimero_primitives::outbound::OutboundSelection,
    replace: bool,
) -> EyreResult<Approval> {
    use calimero_primitives::outbound::OutboundSelection;

    let selected: &[String] = match selection {
        OutboundSelection::Manifest => declared,
        OutboundSelection::Prefixes(prefixes) => prefixes,
    };
    if let Some(bad) = selected
        .iter()
        .find(|p| !calimero_bundle::is_outbound_url(p))
    {
        bail!("{bad:?} is not an https URL prefix");
    }
    fn union<'a>(lists: impl IntoIterator<Item = &'a [String]>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for item in lists.into_iter().flatten() {
            if !out.contains(item) {
                out.push(item.clone());
            }
        }
        out
    }
    let none: &[String] = &[];
    let prefixes = if replace {
        union([selected])
    } else {
        union([existing.map_or(none, |a| &a.prefixes[..]), selected])
    };
    let reviewed = union([
        existing.map_or(none, |a| &a.reviewed[..]),
        declared,
        selected,
    ]);
    for (what, list) in [("approved", &prefixes), ("reviewed", &reviewed)] {
        if list.len() > MAX_OUTBOUND_APPROVALS {
            bail!(
                "{} {what} prefixes is more than the {MAX_OUTBOUND_APPROVALS} one approval holds",
                list.len()
            );
        }
    }
    Ok(Approval { prefixes, reviewed })
}

#[cfg(feature = "outbound-testing")]
mod testing;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod transport_tests;

#[cfg(test)]
mod log_tests;
