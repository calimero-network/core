//! Sealed transport: requests encrypted end to end to this node's attested key.
//!
//! The HTTP server speaks plain HTTP, and whatever TLS sits in front of it ends
//! outside the TD — at a reverse proxy, a load balancer, a relay — wherever the
//! operator points it. Everything that crosses that hop (bearer tokens, JSON-RPC
//! arguments, results) is readable there, attestation or not: a genuine quote
//! says what runs inside the TD and nothing about who reads the bytes on the way
//! in.
//!
//! Sealing closes that without trusting the transport:
//!
//! * At startup the server makes an X25519 **transport key**. It lives only in
//!   this process's memory — inside the TD on a TEE node — and is never written
//!   anywhere, so a restart replaces it.
//! * `POST /admin-api/tee/attest` with `bindTransportKey` returns its public half
//!   and commits to it in the quote's report data
//!   ([`calimero_tee_attestation::attest_transport_binding`]). A client that
//!   verifies the quote knows the key belongs to the attested TD.
//! * The client opens a **session** with a Noise NK handshake to that key
//!   ([`HANDSHAKE_PATH`]). The transport key only authenticates the node: the
//!   session keys come from both sides' ephemeral keys, which are discarded once
//!   the handshake ends. That is the forward secrecy — when a session is gone
//!   (after [`SESSION_LIFETIME`] at most), nothing can open what was sent in it,
//!   not even the transport key.
//! * The client wraps a whole HTTP request — method, path, headers (the bearer
//!   token among them) and body — seals it under the session and posts it to
//!   [`SEALED_PATH`]. [`intercept`] opens it, dispatches the inner request
//!   through the same router every other request takes (so auth, limits and
//!   metrics apply unchanged), and streams the response back in sealed frames.
//!
//! A proxy sees opaque POSTs. It cannot read a request or a response, cannot
//! forge a response frame the client accepts, cannot reorder, drop or truncate
//! frames unnoticed, and cannot replay a request: each carries an id its session
//! accepts once.
//!
//! Responses are framed rather than buffered, so a server-sent event stream is
//! sealed like any other response, frame by frame, until its session expires.
//! A WebSocket upgrade cannot cross a POST and is refused inside the envelope.
//!
//! Wire format (v2), which mero-js implements identically:
//!
//! ```text
//! handshake   Noise_NK_25519_AESGCM_SHA256, prologue "calimero/sealed-http/v2",
//!             the node's static key = the transport key
//!   request   POST /sealed/v2/handshake
//!             0x02 || transport_pk || Noise message 1 (e, es; empty payload)
//!   response  0x02 || Noise message 2 (e, ee; payload = session_id(16) || lifetime secs, u32 BE)
//!   keys      (req key, resp key) = Split()
//!
//! exchange    POST /sealed/v2
//!   header    0x02 || session_id(16) || request_id (u64 BE; from 1, each used once)
//!   request   header || AES-256-GCM(req key, nonce = request_id || 0u32, aad = header, inner)
//!   response  frames, each u32 BE length || AES-256-GCM(resp key,
//!             nonce = request_id || frame index (u32 BE, from 0), aad = header, kind || data)
//!             kind 0 = head (JSON, first frame only), 1 = body bytes, 2 = end (empty)
//!             a response that stops without an end frame was cut short
//!   inner     u32 BE head length || head (JSON) || body
//!   req head  {"method": "POST", "path": "/jsonrpc?x=y", "headers": [[name, value], ..]}
//!   resp head {"status": 200, "headers": [[name, value], ..]}
//! ```

use core::time::Duration;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Instant;

use axum::body::{to_bytes, Body, BodyDataStream, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, RETRY_AFTER, TRANSFER_ENCODING,
    UPGRADE,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use curve25519_dalek::montgomery::MontgomeryPoint;
use futures_util::{stream, StreamExt};
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use zeroize::Zeroizing;

pub use self::session::SESSION_LIFETIME;
use self::session::{SessionId, SessionKeys, Sessions, MESSAGE_1_LEN, SESSION_ID_LEN};
use crate::proxy_identity;

mod session;

/// Where a session is opened, under the node's mount (see [`SealedOptions`]).
pub const HANDSHAKE_PATH: &str = "/sealed/v2/handshake";
/// Where sealed requests are posted, under the node's mount.
pub const SEALED_PATH: &str = "/sealed/v2";
/// Content type of every sealed body.
pub const SEALED_CONTENT_TYPE: &str = "application/vnd.calimero.sealed";

const VERSION: u8 = 2;
/// `version || transport_pk || Noise message 1`.
const HANDSHAKE_LEN: usize = 1 + 32 + MESSAGE_1_LEN;
/// `version || session_id || request_id`, a request's AAD and its frames'.
const HEADER_LEN: usize = 1 + SESSION_ID_LEN + 8;
const TAG_LEN: usize = 16;

/// Largest sealed request accepted.
const MAX_SEALED_BYTES: usize = 64 * 1024 * 1024;
/// Most data sealed into one frame, a head or a piece of body, so a client can
/// refuse a larger frame before holding any of it.
const MAX_FRAME_DATA: usize = 64 * 1024;

/// Handshakes answered per second, node-wide, once the burst is spent. A
/// handshake needs no credential, so this is what bounds the work a stranger can
/// make the node do and how fast they can churn the session table. It is
/// node-wide because the node cannot tell clients apart: behind a proxy every
/// connection comes from the proxy. Per-client limits belong where client
/// addresses are known.
const HANDSHAKES_PER_SECOND: f64 = 100.0;
const HANDSHAKE_BURST: f64 = 200.0;
/// How often expired sessions are swept, which bounds how long their keys
/// outlive them.
const EXPIRY_SWEEP: Duration = Duration::from_secs(30);

/// Unsealed paths still served when sealing is required, under the admin API:
/// probes, and the attestation a client needs before it can seal anything.
const UNSEALED_ADMIN_PATHS: [&str; 3] = ["/health", "/ready", "/tee/attest"];

/// The admin routes this process serves without a credential, and so the only
/// ones a sealed request may reach under [`InnerScope::Uncredentialed`]. They
/// mirror the public router in `admin::service`; delegated execution is added
/// separately, since it is public only when `delegated_access` is on.
const UNCREDENTIALED_ADMIN_PATHS: [&str; 5] = [
    "/health",
    "/ready",
    "/is-authed",
    "/tee/info",
    "/tee/attest",
];

const FRAME_HEAD: u8 = 0;
const FRAME_DATA: u8 = 1;
const FRAME_END: u8 = 2;

const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
/// Tells nginx-style proxies not to buffer a response, which would hold back
/// every frame of a sealed event stream.
const X_ACCEL_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");

/// Headers that describe the hop, not the message, and so never cross the
/// envelope in either direction.
const HOP_HEADERS: [HeaderName; 5] = [CONNECTION, CONTENT_LENGTH, HOST, TRANSFER_ENCODING, UPGRADE];

/// Which routes a sealed request may reach once opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InnerScope {
    /// Any route. This process checks every request itself (embedded auth),
    /// so an opened request meets the same guard a direct one would.
    #[default]
    Any,
    /// Only the routes this process serves without a credential: probes, the
    /// public TEE routes, and delegated execution when `delegated_access` is
    /// on. For a node whose auth a proxy enforces (`auth_mode = "proxy"`),
    /// where this process guards nothing itself. The proxy sees only
    /// `POST /sealed/v2`, never the request inside, so a route it would have
    /// guarded must not be reachable through an envelope: anything else is
    /// refused with `403 sealed_route_unguarded`.
    Uncredentialed {
        /// Also allow `GET`/`POST {admin}/contexts/{id}/intents`, public on
        /// this node (`[server.admin] delegated_access`).
        delegated_access: bool,
    },
}

/// How this node serves the sealed transport.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SealedOptions {
    /// Refuse unsealed requests (`[server.sealed] required`).
    pub required: bool,
    /// The node's `NODE_PATH_PREFIX`, if any. The envelope is served both at
    /// the root and under it, and a request opened there is routed under it,
    /// exactly as a direct request through the same base URL would be.
    pub path_prefix: Option<String>,
    /// Which routes an opened request may reach.
    pub inner_scope: InnerScope,
}

impl SealedOptions {
    #[must_use]
    pub const fn new(required: bool, path_prefix: Option<String>) -> Self {
        Self {
            required,
            path_prefix,
            inner_scope: InnerScope::Any,
        }
    }

    /// Limit which routes an opened request may reach; see [`InnerScope`].
    #[must_use]
    pub const fn with_inner_scope(mut self, inner_scope: InnerScope) -> Self {
        self.inner_scope = inner_scope;
        self
    }
}

/// This process's transport key, and the sessions opened to it.
pub struct SealedTransport {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
    sessions: Mutex<Sessions>,
    handshakes: Mutex<HandshakeLimit>,
    /// Where the envelope is served: the root, and the path prefix if any.
    mounts: Vec<String>,
    /// The unsealed paths still served when sealing is required; `None` when
    /// it is not.
    unsealed_allowed: Option<Vec<String>>,
    /// Which routes an opened request may reach, and the prefix they sit under.
    inner_scope: InnerScope,
    prefix: String,
    metrics: SealedMetrics,
}

impl core::fmt::Debug for SealedTransport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SealedTransport")
            .field("public", &hex::encode(self.public))
            .finish_non_exhaustive()
    }
}

impl SealedTransport {
    /// A new transport key, with the transport's metrics registered against
    /// `registry`.
    #[must_use]
    pub fn generate(options: &SealedOptions, registry: &mut Registry) -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        UnwrapErr(SysRng).fill_bytes(secret.as_mut());
        Self::from_secret(secret, options, SealedMetrics::register(registry))
    }

    fn from_secret(
        secret: Zeroizing<[u8; 32]>,
        options: &SealedOptions,
        metrics: SealedMetrics,
    ) -> Self {
        let public = MontgomeryPoint::mul_base_clamped(*secret).0;
        let prefix = options.path_prefix.clone().unwrap_or_default();
        let mut mounts = vec![String::new()];
        if !prefix.is_empty() {
            mounts.push(prefix.clone());
        }
        let unsealed_allowed = options.required.then(|| {
            UNSEALED_ADMIN_PATHS
                .iter()
                .map(|path| format!("{prefix}/admin-api{path}"))
                .collect()
        });
        Self {
            secret,
            public,
            sessions: Mutex::default(),
            handshakes: Mutex::new(HandshakeLimit::full(Instant::now())),
            mounts,
            unsealed_allowed,
            inner_scope: options.inner_scope,
            prefix,
            metrics,
        }
    }

    #[must_use]
    pub const fn public_key(&self) -> [u8; 32] {
        self.public
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, Sessions> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record_session_count(&self, sessions: &Sessions) {
        let _previous = self
            .metrics
            .sessions
            .set(i64::try_from(sessions.len()).unwrap_or(i64::MAX));
    }

    /// Which envelope endpoint `path` names, and the mount it names it under.
    fn route(&self, path: &str) -> Option<(Endpoint, &str)> {
        self.mounts.iter().find_map(|mount| {
            let endpoint = match path.strip_prefix(mount.as_str())? {
                HANDSHAKE_PATH => Endpoint::Handshake,
                SEALED_PATH => Endpoint::Exchange,
                _ => return None,
            };
            Some((endpoint, mount.as_str()))
        })
    }

    /// Whether an opened request for `path` may be dispatched.
    ///
    /// Matched exactly, never by prefix: a prefix or a pattern loose enough to
    /// admit a sibling route would reopen, through the envelope, a route the
    /// proxy guards.
    fn allows_inner(&self, path: &str) -> bool {
        let InnerScope::Uncredentialed { delegated_access } = self.inner_scope else {
            return true;
        };
        let Some(admin) = path
            .strip_prefix(self.prefix.as_str())
            .and_then(|rest| rest.strip_prefix("/admin-api"))
        else {
            return false;
        };
        if UNCREDENTIALED_ADMIN_PATHS.contains(&admin) {
            return true;
        }
        delegated_access && is_intents_path(admin)
    }

    fn refuses_unsealed(&self, path: &str) -> bool {
        self.unsealed_allowed
            .as_ref()
            .is_some_and(|allowed| !allowed.iter().any(|allowed| allowed == path))
    }

    fn refuse(&self, refusal: &Refusal) {
        debug!(code = refusal.code, "refused a sealed request");
        let _previous = self
            .metrics
            .refusals
            .get_or_create(&RefusalLabels { code: refusal.code })
            .inc();
    }
}

#[derive(Clone, Copy)]
enum Endpoint {
    Handshake,
    Exchange,
}

/// A token bucket over handshakes.
struct HandshakeLimit {
    tokens: f64,
    refilled: Instant,
}

impl HandshakeLimit {
    const fn full(now: Instant) -> Self {
        Self {
            tokens: HANDSHAKE_BURST,
            refilled: now,
        }
    }

    fn admit(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.tokens = elapsed
            .mul_add(HANDSHAKES_PER_SECOND, self.tokens)
            .min(HANDSHAKE_BURST);
        self.refilled = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RefusalLabels {
    code: &'static str,
}

#[derive(Clone, Debug, Default)]
struct SealedMetrics {
    handshakes: Counter,
    requests: Counter,
    refusals: Family<RefusalLabels, Counter>,
    sessions: Gauge,
}

impl SealedMetrics {
    fn register(registry: &mut Registry) -> Self {
        let metrics = Self::default();
        registry.register(
            "sealed_handshakes",
            "Sealed sessions opened",
            metrics.handshakes.clone(),
        );
        registry.register(
            "sealed_requests",
            "Sealed requests opened and dispatched",
            metrics.requests.clone(),
        );
        registry.register(
            "sealed_refusals",
            "Sealed-transport refusals, by code: of the envelope, of a handshake \
             over the rate limit (busy), and of an unsealed request while sealing \
             is required (sealed_required)",
            metrics.refusals.clone(),
        );
        registry.register(
            "sealed_sessions",
            "Sealed sessions whose keys the node holds",
            metrics.sessions.clone(),
        );
        metrics
    }
}

/// Sweep expired sessions every [`EXPIRY_SWEEP`] until `shutdown`, so a
/// session's keys are dropped when it expires, not whenever it is next named.
/// Holds the transport weakly: it ends when the server does.
pub async fn expire_sessions(transport: Weak<SealedTransport>, shutdown: CancellationToken) {
    let mut ticker = tokio::time::interval(EXPIRY_SWEEP);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let Some(transport) = transport.upgrade() else {
            return;
        };
        let mut sessions = transport.sessions();
        sessions.expire(Instant::now());
        transport.record_session_count(&sessions);
    }
}

/// Route [`HANDSHAKE_PATH`] and [`SEALED_PATH`] through the envelope; pass
/// everything else by, unless sealing is required.
///
/// This has to wrap the router from outside, not sit on a route: the inner
/// request is handed back to the router to be routed afresh, and does not pass
/// through here again.
pub async fn intercept(
    State(transport): State<Arc<SealedTransport>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let Some((endpoint, mount)) = transport.route(path) else {
        if transport.refuses_unsealed(path) {
            let refusal = Refusal {
                status: StatusCode::FORBIDDEN,
                code: "sealed_required",
                message: "this node serves only sealed requests; attest, open a sealed \
                          session and send the request through it",
            };
            transport.refuse(&refusal);
            return refusal.into_response();
        }
        return next.run(request).await;
    };
    if request.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let mount = mount.to_owned();
    let result = match endpoint {
        Endpoint::Handshake => handshake(&transport, request).await,
        Endpoint::Exchange => open_and_dispatch(&transport, request, &mount, next).await,
    };
    result.unwrap_or_else(|refusal| {
        transport.refuse(&refusal);
        refusal.into_response()
    })
}

async fn handshake(transport: &SealedTransport, request: Request) -> Result<Response, Refusal> {
    let admitted = transport
        .handshakes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .admit(Instant::now());
    if !admitted {
        return Err(Refusal {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "busy",
            message: "too many sealed sessions are being opened; retry shortly",
        });
    }
    let body = to_bytes(request.into_body(), HANDSHAKE_LEN)
        .await
        .map_err(|_| malformed())?;
    if body.len() != HANDSHAKE_LEN {
        return Err(malformed());
    }
    let (&[version], rest) = body.split_first_chunk::<1>().ok_or_else(malformed)?;
    check_version(version)?;
    let (transport_public, message_1) = rest.split_first_chunk::<32>().ok_or_else(malformed)?;
    if *transport_public != transport.public {
        // A restart replaced the key. The client has to attest again: taking a
        // new key from this response would be taking it from whoever sent it.
        return Err(Refusal {
            status: StatusCode::CONFLICT,
            code: "stale_transport_key",
            message: "the handshake is addressed to a transport key this node no longer holds; \
                      attest again for the current one",
        });
    }

    let mut session_id: SessionId = [0; SESSION_ID_LEN];
    UnwrapErr(SysRng).fill_bytes(&mut session_id);
    let (message_2, keys) = session::respond(&transport.secret, message_1, &session_id)?;
    {
        let mut sessions = transport.sessions();
        sessions.insert(session_id, keys, Instant::now());
        transport.record_session_count(&sessions);
    }
    let _previous = transport.metrics.handshakes.inc();

    let mut sealed = Vec::with_capacity(1 + message_2.len());
    sealed.push(VERSION);
    sealed.extend_from_slice(&message_2);
    Ok(sealed_response(Body::from(sealed)))
}

async fn open_and_dispatch(
    transport: &SealedTransport,
    request: Request,
    mount: &str,
    next: Next,
) -> Result<Response, Refusal> {
    let (outer, body) = request.into_parts();
    let sealed = to_bytes(body, MAX_SEALED_BYTES)
        .await
        .map_err(|_| Refusal {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "too_large",
            message: "the sealed request is too large",
        })?;

    let envelope = RequestEnvelope::parse(&sealed)?;
    let (keys, expires) = {
        let mut sessions = transport.sessions();
        let found = sessions.keys(&envelope.session_id, Instant::now());
        transport.record_session_count(&sessions);
        found?
    };
    let plaintext = open_request(&keys, &envelope)?;
    transport
        .sessions()
        .admit(&envelope.session_id, envelope.request_id, Instant::now())?;
    let (head, body) = split_inner(&plaintext)?;

    let frames = FrameSealer::new(&keys, envelope.header, envelope.request_id);
    if head
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case(UPGRADE.as_str()))
    {
        let refused = plain_error(
            StatusCode::NOT_IMPLEMENTED,
            "a WebSocket cannot be sealed; subscribe over server-sent events instead",
        );
        return Ok(seal_response(frames, refused, expires));
    }

    let mut inner = inner_request(head, body, mount)?;
    // A sealed request that names the envelope again would only open another.
    if transport.route(inner.uri().path()).is_some() {
        return Err(malformed());
    }
    // Refused inside the envelope rather than in the clear: the session is
    // valid, so the answer is sealed like any other, and what was asked for
    // stays between the client and this node.
    if !transport.allows_inner(inner.uri().path()) {
        let refusal = Refusal {
            status: StatusCode::FORBIDDEN,
            code: "sealed_route_unguarded",
            message: "this node's auth is enforced by the proxy in front of it, which \
                      cannot see inside a sealed request, so only the routes it serves \
                      without a credential can be reached sealed",
        };
        transport.refuse(&refusal);
        return Ok(seal_response(frames, refusal.into_response(), expires));
    }
    let _previous = transport.metrics.requests.inc();
    // Whatever the server's own layers put on the outer request (connection
    // info, above all) belongs to the inner one: it is the same request.
    *inner.extensions_mut() = outer.extensions;
    // How the node was reached is a fact about the outer hop, which the inner
    // request cannot state for itself (a browser may not set `Host`). Auth reads
    // it to check a node-bound token, so it comes from outside, never from the
    // envelope.
    for name in [HOST, X_FORWARDED_HOST] {
        let _previous = inner.headers_mut().remove(&name);
        if let Some(value) = outer.headers.get(&name) {
            let _previous = inner.headers_mut().insert(name, value.clone());
        }
    }
    // Who the caller is, as the proxy says, is never something an envelope can
    // state: the proxy cannot see inside one, so a value here was written by
    // the client. Nor is it copied from outside, as `Host` is: the outer
    // request's would describe the envelope's hop, not the request it carries.
    for name in [
        &proxy_identity::ACCOUNT_HEADER,
        &proxy_identity::DEVICE_HEADER,
    ] {
        let _previous = inner.headers_mut().remove(name);
    }

    let response = next.run(inner).await;
    Ok(seal_response(frames, response, expires))
}

/// `/contexts/{id}/intents`, with `id` exactly 64 lowercase hex characters,
/// as a context id renders.
fn is_intents_path(admin_path: &str) -> bool {
    admin_path
        .strip_prefix("/contexts/")
        .and_then(|rest| rest.strip_suffix("/intents"))
        .is_some_and(|id| {
            id.len() == 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn check_version(version: u8) -> Result<(), Refusal> {
    if version == VERSION {
        Ok(())
    } else {
        Err(Refusal::bad_request(
            "unsupported_version",
            "unsupported sealed transport version",
        ))
    }
}

/// A refusal of the envelope itself. It goes back in the clear: the request
/// could not be opened, so there is nothing to seal it under.
#[derive(Debug)]
struct Refusal {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl Refusal {
    const fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message,
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response();
        if self.status == StatusCode::SERVICE_UNAVAILABLE {
            let _previous = response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

const fn malformed() -> Refusal {
    Refusal::bad_request("malformed", "the sealed request is malformed")
}

struct RequestEnvelope<'a> {
    header: [u8; HEADER_LEN],
    session_id: SessionId,
    request_id: u64,
    ciphertext: &'a [u8],
}

impl<'a> RequestEnvelope<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, Refusal> {
        if bytes.len() < HEADER_LEN + TAG_LEN {
            return Err(malformed());
        }
        let (header, ciphertext) = bytes
            .split_first_chunk::<HEADER_LEN>()
            .ok_or_else(malformed)?;
        let (&[version], rest) = header.split_first_chunk::<1>().ok_or_else(malformed)?;
        check_version(version)?;
        let (session_id, request_id) = rest
            .split_first_chunk::<SESSION_ID_LEN>()
            .ok_or_else(malformed)?;
        let request_id = u64::from_be_bytes(request_id.try_into().map_err(|_| malformed())?);
        Ok(Self {
            header: *header,
            session_id: *session_id,
            request_id,
            ciphertext,
        })
    }
}

fn aead_key(key: &[u8; 32]) -> LessSafeKey {
    // SAFETY: AES-256-GCM takes exactly a 32-byte key.
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("32-byte AES-256-GCM key"))
}

/// `request_id || index`: unique per key, since a session admits each request
/// id once and numbers each response's frames from zero.
fn nonce(request_id: u64, index: u32) -> Nonce {
    let mut nonce = [0u8; NONCE_LEN];
    nonce[..8].copy_from_slice(&request_id.to_be_bytes());
    nonce[8..].copy_from_slice(&index.to_be_bytes());
    Nonce::assume_unique_for_key(nonce)
}

fn open_request(
    keys: &SessionKeys,
    envelope: &RequestEnvelope<'_>,
) -> Result<Zeroizing<Vec<u8>>, Refusal> {
    let mut buffer = Zeroizing::new(envelope.ciphertext.to_vec());
    let len = aead_key(&keys.request)
        .open_in_place(
            nonce(envelope.request_id, 0),
            Aad::from(&envelope.header),
            buffer.as_mut(),
        )
        .map_err(|_| {
            Refusal::bad_request(
                "undecryptable",
                "the sealed request did not open under its session",
            )
        })?
        .len();
    buffer.truncate(len);
    Ok(buffer)
}

/// Seals one response's frames, numbering them.
struct FrameSealer {
    key: LessSafeKey,
    aad: [u8; HEADER_LEN],
    request_id: u64,
    index: u32,
}

impl FrameSealer {
    fn new(keys: &SessionKeys, aad: [u8; HEADER_LEN], request_id: u64) -> Self {
        Self {
            key: aead_key(&keys.response),
            aad,
            request_id,
            index: 0,
        }
    }

    /// `None` once the frame index is spent — some four billion frames, far
    /// past any session's lifetime.
    fn seal(&mut self, kind: u8, data: &[u8]) -> Option<Bytes> {
        let index = self.index;
        self.index = index.checked_add(1)?;
        let mut frame = Vec::with_capacity(4 + 1 + data.len() + TAG_LEN);
        let len = u32::try_from(1 + data.len() + TAG_LEN).ok()?;
        frame.extend_from_slice(&len.to_be_bytes());
        frame.push(kind);
        frame.extend_from_slice(data);
        let tag = self
            .key
            .seal_in_place_separate_tag(
                nonce(self.request_id, index),
                Aad::from(&self.aad),
                &mut frame[4..],
            )
            .ok()?;
        frame.extend_from_slice(tag.as_ref());
        Some(Bytes::from(frame))
    }
}

/// Stream `response` back as sealed frames: its head, its body as it is
/// produced, then an end frame. At `expires` the session's keys are due to be
/// gone, so a response still streaming then is cut off without an end frame,
/// and its client reconnects under a new session.
fn seal_response(mut frames: FrameSealer, response: Response, expires: Instant) -> Response {
    let (parts, body) = response.into_parts();
    let head = ResponseHead {
        status: parts.status.as_u16(),
        headers: header_pairs(&parts.headers),
    };
    // SAFETY: a plain struct of an integer and strings always serializes.
    let mut head = serde_json::to_vec(&head).expect("response head serializes");
    let mut body = body;
    // Every frame fits in 64 KiB, so a client can refuse anything larger
    // without buffering it. Headers that do not fit are not sent at all.
    if head.len() > MAX_FRAME_DATA {
        let (parts, error) = plain_error(
            StatusCode::BAD_GATEWAY,
            "the response headers are too large to seal",
        )
        .into_parts();
        head = serde_json::to_vec(&ResponseHead {
            status: parts.status.as_u16(),
            headers: header_pairs(&parts.headers),
        })
        // SAFETY: as above.
        .expect("response head serializes");
        body = error;
    }
    let first = frames.seal(FRAME_HEAD, &head);

    let state = Streaming {
        frames,
        body: body.into_data_stream(),
        pending: Bytes::new(),
        deadline: tokio::time::Instant::from_std(expires),
        done: false,
    };
    let rest = stream::unfold(state, Streaming::next_frame);
    let frames = stream::iter(first)
        .chain(rest)
        .map(Ok::<_, core::convert::Infallible>);
    sealed_response(Body::from_stream(frames))
}

struct Streaming {
    frames: FrameSealer,
    body: BodyDataStream,
    /// Body bytes read but not yet sealed, when a chunk is larger than a frame.
    pending: Bytes,
    deadline: tokio::time::Instant,
    done: bool,
}

impl Streaming {
    async fn next_frame(mut self) -> Option<(Bytes, Self)> {
        if self.done {
            return None;
        }
        if self.pending.is_empty() {
            match tokio::time::timeout_at(self.deadline, self.body.next()).await {
                Ok(Some(Ok(chunk))) => self.pending = chunk,
                Ok(None) => {
                    self.done = true;
                    let end = self.frames.seal(FRAME_END, &[])?;
                    return Some((end, self));
                }
                // The inner response failed, or the session expired: stop
                // without an end frame, so the client knows it was cut short.
                Ok(Some(Err(_))) | Err(_) => return None,
            }
        }
        let piece = self
            .pending
            .split_to(self.pending.len().min(MAX_FRAME_DATA));
        let frame = self.frames.seal(FRAME_DATA, &piece)?;
        Some((frame, self))
    }
}

fn sealed_response(body: Body) -> Response {
    (
        [
            (CONTENT_TYPE, HeaderValue::from_static(SEALED_CONTENT_TYPE)),
            (CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (X_ACCEL_BUFFERING, HeaderValue::from_static("no")),
        ],
        body,
    )
        .into_response()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestHead {
    method: String,
    path: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResponseHead {
    status: u16,
    headers: Vec<(String, String)>,
}

fn split_inner(plaintext: &[u8]) -> Result<(RequestHead, &[u8]), Refusal> {
    let (len, rest) = plaintext.split_first_chunk::<4>().ok_or_else(malformed)?;
    let len = usize::try_from(u32::from_be_bytes(*len)).map_err(|_| malformed())?;
    if len > rest.len() {
        return Err(malformed());
    }
    let (head, body) = rest.split_at(len);
    let head = serde_json::from_slice(head).map_err(|_| malformed())?;
    Ok((head, body))
}

/// The inner request, routed under `mount` — the prefix the envelope arrived
/// under — so it reaches the router exactly as a direct request through the
/// client's base URL would.
fn inner_request(head: RequestHead, body: &[u8], mount: &str) -> Result<Request, Refusal> {
    let method = Method::from_bytes(head.method.as_bytes()).map_err(|_| malformed())?;
    // Origin-form only: the router must see a path.
    if !head.path.starts_with('/') {
        return Err(malformed());
    }
    let uri: Uri = format!("{mount}{}", head.path)
        .parse()
        .map_err(|_| malformed())?;
    let mut request = Request::new(Body::from(Bytes::copy_from_slice(body)));
    *request.method_mut() = method;
    *request.uri_mut() = uri;
    let headers = request.headers_mut();
    for (name, value) in head.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| malformed())?;
        if HOP_HEADERS.contains(&name) {
            continue;
        }
        let value = HeaderValue::from_str(&value).map_err(|_| malformed())?;
        let _previous = headers.append(name, value);
    }
    let _previous = headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    Ok(request)
}

fn plain_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": { "message": message } }))).into_response()
}

fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| !HOP_HEADERS.contains(name))
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests;
