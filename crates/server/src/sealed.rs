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

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use axum::body::{to_bytes, Body, BodyDataStream, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, TRANSFER_ENCODING, UPGRADE,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use curve25519_dalek::montgomery::MontgomeryPoint;
use futures_util::{stream, StreamExt};
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;
use zeroize::Zeroizing;

pub use self::session::SESSION_LIFETIME;
use self::session::{SessionId, SessionKeys, Sessions, MESSAGE_1_LEN, SESSION_ID_LEN};

mod session;

/// Where a session is opened.
pub const HANDSHAKE_PATH: &str = "/sealed/v2/handshake";
/// Where sealed requests are posted.
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
/// Largest piece of a response body sealed into one frame, so a client never
/// holds more than this of an unauthenticated frame.
const MAX_FRAME_DATA: usize = 64 * 1024;

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

/// This process's transport key, and the sessions opened to it.
pub struct SealedTransport {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
    sessions: Mutex<Sessions>,
}

impl core::fmt::Debug for SealedTransport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SealedTransport")
            .field("public", &hex::encode(self.public))
            .finish_non_exhaustive()
    }
}

impl SealedTransport {
    #[must_use]
    pub fn generate() -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        UnwrapErr(SysRng).fill_bytes(secret.as_mut());
        Self::from_secret(secret)
    }

    fn from_secret(secret: Zeroizing<[u8; 32]>) -> Self {
        let public = MontgomeryPoint::mul_base_clamped(*secret).0;
        Self {
            secret,
            public,
            sessions: Mutex::default(),
        }
    }

    #[must_use]
    pub const fn public_key(&self) -> [u8; 32] {
        self.public
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, Sessions> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Route [`HANDSHAKE_PATH`] and [`SEALED_PATH`] through the envelope; pass
/// everything else by.
///
/// This has to wrap the router from outside, not sit on a route: the inner
/// request is handed back to the router to be routed afresh.
pub async fn intercept(
    State(transport): State<Arc<SealedTransport>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path != HANDSHAKE_PATH && path != SEALED_PATH {
        return next.run(request).await;
    }
    if request.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let result = if path == HANDSHAKE_PATH {
        handshake(&transport, request).await
    } else {
        open_and_dispatch(&transport, request, next).await
    };
    result.unwrap_or_else(|refusal| {
        debug!(code = refusal.code, "refused a sealed request");
        refusal.into_response()
    })
}

async fn handshake(transport: &SealedTransport, request: Request) -> Result<Response, Refusal> {
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
    transport
        .sessions()
        .insert(session_id, keys, Instant::now());

    let mut sealed = Vec::with_capacity(1 + message_2.len());
    sealed.push(VERSION);
    sealed.extend_from_slice(&message_2);
    Ok(sealed_response(Body::from(sealed)))
}

async fn open_and_dispatch(
    transport: &SealedTransport,
    request: Request,
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
    let (keys, expires) = transport
        .sessions()
        .keys(&envelope.session_id, Instant::now())?;
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

    let mut inner = inner_request(head, body)?;
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

    let response = next.run(inner).await;
    Ok(seal_response(frames, response, expires))
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
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
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
    let head = serde_json::to_vec(&head).expect("response head serializes");
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

fn inner_request(head: RequestHead, body: &[u8]) -> Result<Request, Refusal> {
    let method = Method::from_bytes(head.method.as_bytes()).map_err(|_| malformed())?;
    let uri: Uri = head.path.parse().map_err(|_| malformed())?;
    // Origin-form only: the router must see a path, and a sealed request that
    // names the envelope again would only open another envelope.
    if uri.scheme().is_some()
        || !head.path.starts_with('/')
        || uri.path() == SEALED_PATH
        || uri.path() == HANDSHAKE_PATH
    {
        return Err(malformed());
    }
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
