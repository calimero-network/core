//! Outbound HTTPS requests from a run, and the secrets they may carry.
//!
//! The runtime's `http_request` host function hands the request bytes to
//! whatever [`OutboundHost`] the node supplied; this module is that host. It
//! makes a request only when every gate holds:
//!
//! 1. **The operator turned it on** (`[context.outbound] enabled`, off by
//!    default) and allows the request's host (`hosts`: listed, or `"*"` for
//!    any host an app declares).
//! 2. **The app declared a prefix covering the URL** in its signed bundle
//!    manifest (`outbound`): same host, and a path at or below the prefix's
//!    at a `/`. A run gets a client only when its app declares one.
//! 3. **The run writes on this node**: never a view, a peer's delta, a state
//!    op, a search export or a migration. `internal_execute` decides that.
//! 4. **The request fits**: `https` on port 443 to a DNS name that resolves
//!    only to public addresses, no fragment, the size limits in
//!    `calimero_primitives::outbound`, no redirect followed, one per run.
//!
//! # Secrets
//!
//! A request names a secret as `{{secret:NAME}}` in a header value or the
//! body. The node asks its [`SecretProvider`] for `NAME` **for the app the run
//! belongs to**, identified by the node (package and signer of the installed
//! application), never by the guest. A secret is substituted only into a
//! request to one of the hosts it is bound to; any copy of it in the response
//! is replaced with `[redacted]` before the guest sees it. The guest never
//! reads a secret, so it cannot write one into state, where it would
//! replicate to every member.
//!
//! Two providers: [`EnvSecrets`], the operator's own (environment variables
//! named in the node config, each scoped to app packages and hosts), and
//! [`SidecarSecrets`], a loopback endpoint the node asks. The sidecar is the
//! seam for a managed deployment: a secrets service next to the node (inside
//! the same TEE, say) answers for the secrets a deployer set for their app,
//! for every namespace that runs it.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use borsh::BorshDeserialize;
use calimero_primitives::outbound::{
    OutboundMethod, OutboundRefusal, OutboundRequest, OutboundResponse, SealedSecretPayload,
    MAX_HEADERS, MAX_HEADER_BYTES, MAX_REQUEST_BODY, MAX_RESPONSE_BODY, MAX_SEALED_ENVELOPE,
    MAX_URL, SEALED_PREFIX, SECRET_PREFIX, SECRET_SUFFIX, TIMEOUT_MS,
};
use calimero_runtime::logic::OutboundHost;
use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::config::OutboundSettings;

/// What replaces a secret echoed in a response.
const REDACTED: &[u8] = b"[redacted]";

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
}

/// What a run's namespace approved for the app's publisher, from the last
/// `GroupOp::OutboundApprovalSet` for that signer at the namespace root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Approval {
    /// The approved URL prefixes.
    pub prefixes: Vec<String>,
    /// The prefixes the approver was shown, approved or not.
    pub reviewed: Vec<String>,
}

/// The start of every refusal for a prefix the namespace has not approved.
/// Stable: an app matches on it to tell a member to ask an admin.
pub const NOT_APPROVED: &str = calimero_primitives::outbound::OutboundRefusal::NotApproved.word();

/// The start of every refusal for a prefix the namespace has never been asked
/// about, because a later version of the app declared it. Stable: an app
/// matches on it to show an admin what to approve.
pub const AWAITING_APPROVAL: &str =
    calimero_primitives::outbound::OutboundRefusal::AwaitingApproval.word();

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

/// What lets a run open the `{{sealed:…}}` secrets a request names: the
/// context the envelopes were sealed in, and the namespace TEE keys this node
/// holds while it is a TEE authority of the namespace, looked up only when a
/// request names one. Every run that gets a client carries it.
///
/// On a node that is not a TEE authority the lookup answers no keys. Then a
/// request naming a sealed secret goes to a TEE of the namespace through
/// `forward`, after this node's own checks pass; with no `forward`, or when no
/// TEE takes it, the request is refused `SealedUnavailable`.
#[derive(Clone)]
pub struct SealedKeys {
    pub context_id: [u8; 32],
    #[allow(clippy::type_complexity)]
    pub keys:
        Arc<dyn Fn() -> EyreResult<Vec<calimero_primitives::identity::PrivateKey>> + Send + Sync>,
    pub forward: Option<Forwarder>,
}

/// Hands a request (a borsh [`OutboundRequest`]) to a TEE of the namespace and
/// answers its borsh [`OutboundResponse`], or why it was not sent. Called on
/// the run's blocking thread.
pub type Forwarder = Arc<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

/// A TEE's answer to a forwarded request, as it travels back to the member.
#[derive(Clone, Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub enum ForwardedAnswer {
    /// Sent. `signature` is `tee`'s, over [`forwarded_answer_message`], so the
    /// member takes the response only from a TEE authority of the namespace.
    Sent {
        response: Vec<u8>,
        tee: [u8; 32],
        signature: Vec<u8>,
    },
    /// The TEE checked the request and would not send it.
    Refused { reason: String },
    /// The peer is not a TEE authority of the namespace; ask another.
    NotTee,
}

/// What a TEE signs to vouch for a forwarded request's response: the context,
/// and digests of the request it was asked and the response it answers.
#[must_use]
pub fn forwarded_answer_message(context_id: &[u8; 32], request: &[u8], response: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    const DOMAIN: &[u8] = b"calimero:outbound:forwarded-answer:v1";
    let mut message = Vec::with_capacity(DOMAIN.len() + 96);
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(context_id);
    message.extend_from_slice(&Sha256::digest(request));
    message.extend_from_slice(&Sha256::digest(response));
    message
}

/// The response in a forwarded request's `answer`, if a TEE in `tee_keys`
/// sent it; `Ok(None)` when the peer is no TEE and another should be asked.
///
/// # Errors
/// The TEE refused the request, or the answer is not one a TEE signed.
pub fn read_forwarded_answer(
    context_id: &[u8; 32],
    request: &[u8],
    answer: &[u8],
    tee_keys: &[calimero_primitives::identity::PublicKey],
) -> Result<Option<Vec<u8>>, String> {
    match ForwardedAnswer::try_from_slice(answer)
        .map_err(|e| format!("a malformed answer from a namespace TEE: {e}"))?
    {
        ForwardedAnswer::NotTee => Ok(None),
        ForwardedAnswer::Refused { reason } => {
            Err(format!("the namespace's TEE refused the request: {reason}"))
        }
        ForwardedAnswer::Sent {
            response,
            tee,
            signature,
        } => {
            let tee = calimero_primitives::identity::PublicKey::from(tee);
            let signature = <[u8; 64]>::try_from(signature.as_slice())
                .map_err(|_| "a malformed signature from a namespace TEE".to_owned())?;
            let message = forwarded_answer_message(context_id, request, &response);
            if !tee_keys.contains(&tee) || tee.verify_raw_signature(&message, &signature).is_err() {
                return Err("an answer no TEE authority of the namespace signed".to_owned());
            }
            Ok(Some(response))
        }
    }
}

/// Whether `request` names a `{{sealed:…}}` secret anywhere.
fn names_sealed(request: &OutboundRequest) -> bool {
    let prefix = SEALED_PREFIX.as_bytes();
    find(&request.body, prefix).is_some()
        || request
            .headers
            .iter()
            .any(|(_, value)| find(value.as_bytes(), prefix).is_some())
}

impl fmt::Debug for SealedKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SealedKeys")
    }
}

/// A request that passed every check, with its secrets filled in.
pub struct Prepared {
    pub method: OutboundMethod,
    pub url: reqwest::Url,
    pub host: String,
    pub headers: Vec<(String, Zeroizing<String>)>,
    pub body: Zeroizing<Vec<u8>>,
    /// The secrets substituted, to redact from the response.
    secrets: Vec<Zeroizing<String>>,
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
    /// The service `[context.outbound]` describes, or `None` when it is off.
    ///
    /// # Errors
    /// A host is not a DNS name, a secret variable is unset, or the sidecar is
    /// not on a loopback address.
    pub fn from_settings(settings: &OutboundSettings) -> EyreResult<Option<Arc<Self>>> {
        if !settings.enabled {
            return Ok(None);
        }
        if settings.hosts.is_empty() {
            bail!("[context.outbound] is enabled but lists no hosts");
        }
        #[cfg_attr(not(feature = "outbound-testing"), allow(unused_mut))]
        let mut secrets: Option<Arc<dyn SecretProvider>> =
            match (&settings.sidecar, settings.env_secrets.is_empty()) {
                (Some(_), false) => {
                    bail!("[context.outbound] takes env_secrets or a sidecar, not both")
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
    /// `declared`, in a namespace that approved `approval` for its publisher
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
        sealed: Option<SealedKeys>,
        context: String,
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
            approval,
            sealed,
            context,
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
        self.prepare_sealed(app, declared, approval, None, request)
    }

    /// [`prepare`](Self::prepare), opening the `{{sealed:…}}` secrets the
    /// request names with `sealed`.
    ///
    /// # Errors
    /// The reason it is refused, worded for the app's developer.
    pub fn prepare_sealed(
        &self,
        app: &AppIdentity,
        declared: &BTreeSet<String>,
        approval: Option<&Approval>,
        sealed: Option<&SealedKeys>,
        request: OutboundRequest,
    ) -> EyreResult<Prepared> {
        self.prepare_inner(app, declared, approval, Some(sealed), request)
    }

    /// Every check [`prepare_sealed`](Self::prepare_sealed) makes, with the
    /// secrets left as written: what a node that is not a TEE asks before it
    /// forwards a request naming a sealed secret to one.
    ///
    /// # Errors
    /// The reason it is refused, worded for the app's developer.
    pub fn check(
        &self,
        app: &AppIdentity,
        declared: &BTreeSet<String>,
        approval: Option<&Approval>,
        request: OutboundRequest,
    ) -> EyreResult<()> {
        self.prepare_inner(app, declared, approval, None, request)
            .map(drop)
    }

    /// `fill`: the sealed keys to fill the secrets in with, or `None` to leave
    /// them as written.
    fn prepare_inner(
        &self,
        app: &AppIdentity,
        declared: &BTreeSet<String>,
        approval: Option<&Approval>,
        fill: Option<Option<&SealedKeys>>,
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
        let Some(under) = declared
            .iter()
            .find(|d| prefix(d).is_some_and(|p| covers(&p, &url)))
        else {
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
            // namespace did make, it is new in a later version of the app.
            if approval.is_some_and(|a| !a.reviewed.iter().any(|r| r == under)) {
                bail!(
                    "{AWAITING_APPROVAL}: {under} is a URL prefix this app declares that \
                     the namespace has not been asked to approve yet"
                );
            }
            bail!("{NOT_APPROVED}: the namespace has not approved {under} for this app");
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
            let value = match fill {
                Some(sealed) => {
                    let value =
                        self.substitute(app, &host, sealed, value.into_bytes(), &mut used)?;
                    String::from_utf8(value.to_vec())
                        .map_err(|_| eyre!("header {name} is not UTF-8"))?
                }
                None => value,
            };
            headers.push((name, Zeroizing::new(value)));
        }
        let body = match fill {
            Some(sealed) => self.substitute(app, &host, sealed, request.body, &mut used)?,
            None => Zeroizing::new(request.body),
        };
        Ok(Prepared {
            method: request.method,
            url,
            host,
            headers,
            body,
            secrets: used,
        })
    }

    /// Replaces every `{{secret:NAME}}` and `{{sealed:…}}` in `bytes`.
    fn substitute(
        &self,
        app: &AppIdentity,
        host: &str,
        sealed: Option<&SealedKeys>,
        bytes: Vec<u8>,
        used: &mut Vec<Zeroizing<String>>,
    ) -> EyreResult<Zeroizing<Vec<u8>>> {
        let suffix = SECRET_SUFFIX.as_bytes();
        let mut out = Zeroizing::new(Vec::with_capacity(bytes.len()));
        let mut rest = &bytes[..];
        loop {
            let named = find(rest, SECRET_PREFIX.as_bytes());
            let sealed_at = find(rest, SEALED_PREFIX.as_bytes());
            let (start, is_sealed) = match (named, sealed_at) {
                (None, None) => break,
                (Some(n), Some(s)) if s < n => (s, true),
                (Some(n), _) => (n, false),
                (None, Some(s)) => (s, true),
            };
            out.extend_from_slice(&rest[..start]);
            if is_sealed {
                let after = &rest[start + SEALED_PREFIX.len()..];
                let end =
                    find(after, suffix).ok_or_else(|| eyre!("an unterminated {SEALED_PREFIX}"))?;
                let value = open_sealed_secret(sealed, host, &after[..end])?;
                out.extend_from_slice(value.as_bytes());
                used.push(value);
                rest = &after[end + suffix.len()..];
                continue;
            }
            let after = &rest[start + SECRET_PREFIX.len()..];
            let end =
                find(after, suffix).ok_or_else(|| eyre!("an unterminated {SECRET_PREFIX}"))?;
            let name = core::str::from_utf8(&after[..end])
                .map_err(|_| eyre!("a malformed secret name"))?;
            if !is_secret_name(name) {
                bail!("secret name {name:?} must be A-Z, 0-9 and _");
            }
            let provider = self
                .secrets
                .as_ref()
                .ok_or_else(|| eyre!("this node has no secrets for outbound requests"))?;
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

/// The value of a `{{sealed:<hex>}}` for a request to `host`, opened with the
/// namespace TEE keys this node holds.
fn open_sealed_secret(
    sealed: Option<&SealedKeys>,
    host: &str,
    hex_envelope: &[u8],
) -> EyreResult<Zeroizing<String>> {
    let unavailable = || {
        eyre!(
            "{}: this node is not a TEE of the namespace, so it cannot open a sealed secret; \
             the request has to run on one of the namespace's TEE nodes",
            OutboundRefusal::SealedUnavailable.word()
        )
    };
    if hex_envelope.len() > 2 * MAX_SEALED_ENVELOPE {
        bail!("a sealed secret is longer than {MAX_SEALED_ENVELOPE} bytes");
    }
    let envelope = hex::decode(hex_envelope).map_err(|_| eyre!("a malformed sealed secret"))?;
    let envelope = calimero_crypto::SealedEnvelope::from_bytes(&envelope)
        .ok_or_else(|| eyre!("a malformed sealed secret"))?;
    let sealed = sealed.ok_or_else(unavailable)?;
    let keys = (sealed.keys)()?;
    if keys.is_empty() {
        return Err(unavailable());
    }
    let plaintext = keys
        .iter()
        .find_map(|key| {
            let purpose = calimero_crypto::Purpose::App {
                context_id: sealed.context_id,
                recipient: key.public_key(),
            };
            calimero_crypto::open_sealed(key, &envelope, purpose).ok()
        })
        .map(Zeroizing::new)
        .ok_or_else(|| {
            eyre!(
                "the sealed secret does not open on this node: it was sealed in another \
                 context, or to a namespace TEE key this node was never given"
            )
        })?;
    let payload = SealedSecretPayload::try_from_slice(&plaintext)
        .map_err(|_| eyre!("a malformed sealed secret"))?;
    let SealedSecretPayload { value, hosts } = payload;
    let value = Zeroizing::new(value);
    if !hosts.iter().any(|h| h == host) {
        bail!("the sealed secret may not be sent to {host}");
    }
    Ok(value)
}

/// One run's client: the service, bound to the run's app and its declared URLs.
#[derive(Debug)]
struct RunOutbound {
    service: Arc<OutboundService>,
    app: AppIdentity,
    declared: BTreeSet<String>,
    approval: Option<Approval>,
    sealed: Option<SealedKeys>,
    context: String,
}

impl RunOutbound {
    /// Where a request naming a sealed secret goes when this node cannot open
    /// it: `None` on a TEE authority of the namespace, which opens it itself.
    fn forwarder(&self, request: &OutboundRequest) -> Result<Option<&Forwarder>, String> {
        let Some(sealed) = &self.sealed else {
            return Ok(None);
        };
        let Some(forward) = &sealed.forward else {
            return Ok(None);
        };
        if !names_sealed(request) {
            return Ok(None);
        }
        let keys = (sealed.keys)().map_err(|e| e.to_string())?;
        Ok(keys.is_empty().then_some(forward))
    }

    /// Checks `request` here, then hands it to a TEE of the namespace.
    fn forward(
        &self,
        forward: &Forwarder,
        bytes: &[u8],
        request: OutboundRequest,
        started: Instant,
    ) -> Result<Vec<u8>, String> {
        let host = request.url.clone();
        if let Err(reason) =
            self.service
                .check(&self.app, &self.declared, self.approval.as_ref(), request)
        {
            warn!(context = %self.context, package = %self.app.package, %reason, "outbound request refused");
            return Err(reason.to_string());
        }
        match forward(bytes) {
            Ok(response) => {
                info!(
                    context = %self.context,
                    package = %self.app.package,
                    url = %host,
                    response_bytes = response.len(),
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "outbound request sent by a namespace TEE"
                );
                Ok(response)
            }
            Err(reason) => {
                warn!(context = %self.context, package = %self.app.package, %reason, "forwarded outbound request failed");
                Err(reason)
            }
        }
    }
}

impl OutboundHost for RunOutbound {
    fn send(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        let request = OutboundRequest::try_from_slice(bytes)
            .map_err(|e| format!("a malformed outbound request: {e}"))?;
        if let Some(forward) = self.forwarder(&request)? {
            return self.forward(forward, bytes, request, started);
        }
        let prepared = match self.service.prepare_sealed(
            &self.app,
            &self.declared,
            self.approval.as_ref(),
            self.sealed.as_ref(),
            request,
        ) {
            Ok(prepared) => prepared,
            Err(reason) => {
                warn!(context = %self.context, package = %self.app.package, %reason, "outbound request refused");
                return Err(reason.to_string());
            }
        };
        let result = self.service.transport.send(&prepared);
        let (status, mut body) = match result {
            Ok(answer) => answer,
            Err(reason) => {
                warn!(
                    context = %self.context,
                    package = %self.app.package,
                    host = %prepared.host,
                    path = %prepared.url.path(),
                    %reason,
                    "outbound request failed"
                );
                return Err(format!("the request to {} failed: {reason}", prepared.host));
            }
        };
        for secret in &prepared.secrets {
            body = redact(&body, secret.as_bytes());
        }
        info!(
            context = %self.context,
            package = %self.app.package,
            host = %prepared.host,
            path = %prepared.url.path(),
            status,
            request_bytes = prepared.body.len(),
            response_bytes = body.len(),
            secrets = prepared.secrets.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "outbound request"
        );
        borsh::to_vec(&OutboundResponse { status, body }).map_err(|e| e.to_string())
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

async fn read_capped(mut response: reqwest::Response, max: usize) -> EyreResult<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        bail!("the response is longer than {max} bytes");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > max {
            bail!("the response is longer than {max} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Runs `future` to completion on a runtime of its own. A run executes on a
/// blocking thread, and each request gets a fresh client, so no connection is
/// shared between runs or apps.
fn run_blocking<T>(future: impl core::future::Future<Output = T>) -> EyreResult<T> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(runtime.block_on(future))
}

/// A declared prefix this node can match against: `https`, a DNS host, no
/// credentials, query or fragment. `None` for anything else, which then
/// covers nothing.
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

fn redact(body: &[u8], secret: &[u8]) -> Vec<u8> {
    if secret.is_empty() {
        return body.to_vec();
    }
    let mut out = Vec::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = find(rest, secret) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(REDACTED);
        rest = &rest[at + secret.len()..];
    }
    out.extend_from_slice(rest);
    out
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

/// What `internal_execute` reads to decide whether a run gets a client: the
/// app's identity and declared URLs, from the installed application.
#[must_use]
pub fn app_outbound(
    app: &calimero_primitives::application::Application,
) -> Option<(AppIdentity, Vec<String>)> {
    let signer = app.signer_id.as_ref()?.to_string();
    if app.package.is_empty() {
        return None;
    }
    let meta: serde_json::Value = serde_json::from_slice(&app.metadata).ok()?;
    let declared: Vec<String> = meta
        .get("outbound")?
        .as_array()?
        .iter()
        .filter_map(|u| u.as_str().map(str::to_owned))
        .collect();
    (!declared.is_empty()).then(|| {
        (
            AppIdentity {
                package: app.package.clone(),
                signer,
            },
            declared,
        )
    })
}

/// The prefixes and the reviewed list a namespace approval holds after an admin
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
    use calimero_governance_types::bounds::MAX_OUTBOUND_APPROVALS;
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

/// What the namespace of `context` approved for apps signed by `signer`:
/// `None` when it approved nothing for them, or the context is in no namespace.
///
/// # Errors
/// The store could not be read.
pub fn namespace_approval(
    store: &calimero_store::Store,
    context: &calimero_primitives::context::ContextId,
    signer: &str,
) -> EyreResult<Option<Approval>> {
    let Some(group) = calimero_governance_store::get_group_for_context(store, context)? else {
        return Ok(None);
    };
    let root = calimero_governance_store::NamespaceRepository::new(store).resolve(&group)?;
    Ok(
        calimero_governance_store::OutboundApprovalRepository::new(store)
            .get(&root, signer)?
            .map(|a| Approval {
                prefixes: a.prefixes,
                reviewed: a.reviewed,
            }),
    )
}

#[cfg(feature = "outbound-testing")]
#[path = "outbound_testing.rs"]
mod testing;

#[cfg(test)]
#[path = "outbound_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "outbound_transport_tests.rs"]
mod transport_tests;
