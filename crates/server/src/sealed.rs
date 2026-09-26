//! Sealed transport: requests encrypted to this node's attested key.
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
//! * The client wraps a whole HTTP request — method, path, headers (the bearer
//!   token among them) and body — encrypts it to that key under a one-time key of
//!   its own, and posts the result to [`SEALED_PATH`]. [`intercept`] opens it,
//!   dispatches the inner request through the same router every other request
//!   takes (so auth, limits and metrics apply unchanged), and seals the response
//!   back under the same exchange.
//!
//! A proxy sees one opaque POST each way. It cannot read the request, cannot
//! forge a response the client accepts (only the transport key's holder can
//! derive the response key), and cannot replay a request: each carries a
//! timestamp and a one-time key, and a one-time key is accepted once.
//!
//! Streaming responses (SSE) and WebSocket upgrades cannot be buffered into one
//! sealed reply, so they are refused inside the envelope rather than left to
//! hang.
//!
//! Wire format (v1), which mero-js implements identically:
//!
//! ```text
//! shared    = X25519(secret, peer public)            all-zero result refused
//! prk       = HKDF-SHA256-Extract(salt = transport_pk || client_pk, ikm = shared)
//! req key   = HKDF-Expand(prk, REQUEST_DOMAIN,  32)
//! resp key  = HKDF-Expand(prk, RESPONSE_DOMAIN, 32)
//! request   = 0x01 || transport_pk || client_pk || nonce(12)
//!             || AES-256-GCM(req key, nonce, aad = the 77 bytes before it, inner request)
//! response  = 0x01 || nonce(12)
//!             || AES-256-GCM(resp key, nonce, aad = 0x01 || transport_pk || client_pk, inner response)
//! inner     = u32 BE head length || head (JSON) || body
//! req head  = {"method": "POST", "path": "/jsonrpc?x=y", "headers": [[name, value], ..], "ts": unix secs}
//! resp head = {"status": 200, "headers": [[name, value], ..]}
//! ```

use core::time::Duration;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, TRANSFER_ENCODING, UPGRADE,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use curve25519_dalek::montgomery::MontgomeryPoint;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::hkdf::{Salt, HKDF_SHA256};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;
use zeroize::Zeroizing;

/// Where sealed requests are posted.
pub const SEALED_PATH: &str = "/sealed/v1";
/// Content type of both sealed bodies.
pub const SEALED_CONTENT_TYPE: &str = "application/vnd.calimero.sealed";

const VERSION: u8 = 1;
const REQUEST_DOMAIN: &[u8] = b"calimero/sealed-http/v1/request";
const RESPONSE_DOMAIN: &[u8] = b"calimero/sealed-http/v1/response";
/// `version || transport_pk || client_pk || nonce`, the request's AAD.
const REQUEST_HEADER_LEN: usize = 1 + 32 + 32 + NONCE_LEN;
const TAG_LEN: usize = 16;

/// Largest sealed body accepted, and largest inner response sealed back.
const MAX_SEALED_BYTES: usize = 64 * 1024 * 1024;
/// How far a request's timestamp may sit from this node's clock.
const MAX_SKEW: Duration = Duration::from_secs(300);
/// One-time keys remembered for replay detection before new requests are
/// refused. Each is held only for its skew window, so reaching this takes over
/// two hundred sealed requests a second, sustained.
const MAX_REMEMBERED: usize = 1 << 17;

const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

/// Headers that describe the hop, not the message, and so never cross the
/// envelope in either direction.
const HOP_HEADERS: [HeaderName; 5] = [CONNECTION, CONTENT_LENGTH, HOST, TRANSFER_ENCODING, UPGRADE];

/// This process's transport key, and the one-time keys already used with it.
pub struct SealedTransport {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
    /// One-time client key → the unix second after which its request would be
    /// refused as stale anyway, so it can be forgotten.
    seen: Mutex<HashMap<[u8; 32], u64>>,
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
            seen: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub const fn public_key(&self) -> [u8; 32] {
        self.public
    }

    /// Forget expired one-time keys, then remember this one. `Err` names why the
    /// request must be refused.
    fn remember(&self, client_public: [u8; 32], ts: u64, now: u64) -> Result<(), Refusal> {
        if ts.abs_diff(now) > MAX_SKEW.as_secs() {
            return Err(Refusal::bad_request(
                "stale_request",
                "the sealed request's timestamp is outside the accepted window",
            ));
        }
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.contains_key(&client_public) {
            return Err(Refusal::bad_request(
                "replayed_request",
                "this sealed request was already received",
            ));
        }
        if seen.len() >= MAX_REMEMBERED {
            seen.retain(|_, expiry| *expiry >= now);
            if seen.len() >= MAX_REMEMBERED {
                return Err(Refusal {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    code: "busy",
                    message: "too many sealed requests in flight; retry shortly",
                });
            }
        }
        let _previous = seen.insert(client_public, ts.saturating_add(MAX_SKEW.as_secs()));
        Ok(())
    }
}

/// Route [`SEALED_PATH`] through the envelope; pass everything else by.
///
/// This has to wrap the router from outside, not sit on a route: the inner
/// request is handed back to the router to be routed afresh.
pub async fn intercept(
    State(transport): State<Arc<SealedTransport>>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() != SEALED_PATH {
        return next.run(request).await;
    }
    if request.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    match open_and_dispatch(&transport, request, next).await {
        Ok(response) => response,
        Err(refusal) => {
            debug!(code = refusal.code, "refused a sealed request");
            refusal.into_response()
        }
    }
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
    if envelope.transport_public != transport.public {
        // A restart replaced the key. The client has to attest again: taking a
        // new key from this response would be taking it from whoever sent it.
        return Err(Refusal {
            status: StatusCode::CONFLICT,
            code: "stale_transport_key",
            message: "the request is sealed to a transport key this node no longer holds; \
                      attest again for the current one",
        });
    }
    let keys = ExchangeKeys::derive(
        &transport.secret,
        &envelope.client_public,
        &transport.public,
        &envelope.client_public,
    )?;
    let plaintext = keys.open_request(&envelope)?;
    let (head, body) = split_inner::<RequestHead>(&plaintext)?;
    transport.remember(envelope.client_public, head.ts, unix_now())?;

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
    let (status, headers, body) = buffer_response(response).await;
    Ok(keys.seal_response(status, &headers, &body))
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
    header: &'a [u8],
    transport_public: [u8; 32],
    client_public: [u8; 32],
    nonce: [u8; NONCE_LEN],
    ciphertext: &'a [u8],
}

impl<'a> RequestEnvelope<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, Refusal> {
        if bytes.len() < REQUEST_HEADER_LEN + TAG_LEN {
            return Err(malformed());
        }
        let (header, ciphertext) = bytes.split_at(REQUEST_HEADER_LEN);
        let (&[version], rest) = header.split_first_chunk::<1>().ok_or_else(malformed)?;
        if version != VERSION {
            return Err(Refusal::bad_request(
                "unsupported_version",
                "unsupported sealed transport version",
            ));
        }
        let (transport_public, rest) = rest.split_first_chunk::<32>().ok_or_else(malformed)?;
        let (client_public, rest) = rest.split_first_chunk::<32>().ok_or_else(malformed)?;
        let nonce = rest.first_chunk::<NONCE_LEN>().ok_or_else(malformed)?;
        Ok(Self {
            header,
            transport_public: *transport_public,
            client_public: *client_public,
            nonce: *nonce,
            ciphertext,
        })
    }
}

/// The two one-use AEAD keys of one request/response exchange.
struct ExchangeKeys {
    request: LessSafeKey,
    response: LessSafeKey,
    response_aad: [u8; 65],
}

impl ExchangeKeys {
    /// `secret` is this side's X25519 secret and `peer` the other side's public
    /// key; the node and a client arrive at the same keys.
    fn derive(
        secret: &[u8; 32],
        peer: &[u8; 32],
        transport_public: &[u8; 32],
        client_public: &[u8; 32],
    ) -> Result<Self, Refusal> {
        let shared = Zeroizing::new(MontgomeryPoint(*peer).mul_clamped(*secret).0);
        // A low-order point yields an all-zero secret that anybody can compute.
        if shared.iter().all(|byte| *byte == 0) {
            return Err(malformed());
        }
        let mut salt = [0u8; 64];
        salt[..32].copy_from_slice(transport_public);
        salt[32..].copy_from_slice(client_public);
        let prk = Salt::new(HKDF_SHA256, &salt).extract(shared.as_ref());
        let key = |domain: &[u8]| {
            prk.expand(&[domain], &AES_256_GCM)
                .map(|okm| LessSafeKey::new(UnboundKey::from(okm)))
                .map_err(|_| malformed())
        };
        let mut response_aad = [0u8; 65];
        response_aad[0] = VERSION;
        response_aad[1..].copy_from_slice(&salt);
        Ok(Self {
            request: key(REQUEST_DOMAIN)?,
            response: key(RESPONSE_DOMAIN)?,
            response_aad,
        })
    }

    fn open_request(&self, envelope: &RequestEnvelope<'_>) -> Result<Zeroizing<Vec<u8>>, Refusal> {
        let mut buffer = Zeroizing::new(envelope.ciphertext.to_vec());
        let len = self
            .request
            .open_in_place(
                Nonce::assume_unique_for_key(envelope.nonce),
                Aad::from(envelope.header),
                buffer.as_mut(),
            )
            .map_err(|_| {
                Refusal::bad_request(
                    "undecryptable",
                    "the sealed request did not open under this node's transport key",
                )
            })?
            .len();
        buffer.truncate(len);
        Ok(buffer)
    }

    fn seal_response(&self, status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Response {
        let mut nonce = [0u8; NONCE_LEN];
        // Random rather than fixed: the response key is unique to the client's
        // one-time key, but a node that somehow answered the same request twice
        // must still never reuse a nonce under it.
        UnwrapErr(SysRng).fill_bytes(&mut nonce);
        let sealed = self.seal_response_with(status, headers, body, nonce);
        (
            [
                (CONTENT_TYPE, SEALED_CONTENT_TYPE),
                (CACHE_CONTROL, "no-store"),
            ],
            sealed,
        )
            .into_response()
    }

    fn seal_response_with(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
        nonce: [u8; NONCE_LEN],
    ) -> Vec<u8> {
        let head = ResponseHead {
            status: status.as_u16(),
            headers: header_pairs(headers),
        };
        let mut buffer = join_inner(&head, body);
        self.response
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&self.response_aad),
                &mut buffer,
            )
            // SAFETY: sealing fails only when the plaintext exceeds AES-GCM's
            // ~64 GiB limit, and `buffer_response` caps it at MAX_SEALED_BYTES.
            .expect("sealed response within AES-GCM's length limit");
        let mut sealed = Vec::with_capacity(1 + NONCE_LEN + buffer.len());
        sealed.push(VERSION);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&buffer);
        sealed
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestHead {
    method: String,
    path: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    ts: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResponseHead {
    status: u16,
    headers: Vec<(String, String)>,
}

fn split_inner<'a, H: Deserialize<'a>>(plaintext: &'a [u8]) -> Result<(H, &'a [u8]), Refusal> {
    let (len, rest) = plaintext.split_first_chunk::<4>().ok_or_else(malformed)?;
    let len = usize::try_from(u32::from_be_bytes(*len)).map_err(|_| malformed())?;
    if len > rest.len() {
        return Err(malformed());
    }
    let (head, body) = rest.split_at(len);
    let head = serde_json::from_slice(head).map_err(|_| malformed())?;
    Ok((head, body))
}

fn join_inner<H: Serialize>(head: &H, body: &[u8]) -> Vec<u8> {
    // SAFETY: the heads are plain structs of strings and integers, which always
    // serialize.
    let head = serde_json::to_vec(head).expect("inner head serializes");
    let len = u32::try_from(head.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(4 + head.len() + body.len() + TAG_LEN);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(body);
    out
}

fn inner_request(head: RequestHead, body: &[u8]) -> Result<Request, Refusal> {
    let method = Method::from_bytes(head.method.as_bytes()).map_err(|_| malformed())?;
    let uri: Uri = head.path.parse().map_err(|_| malformed())?;
    // Origin-form only: the router must see a path, and a sealed request that
    // names the envelope again would only open another envelope.
    if uri.scheme().is_some() || !head.path.starts_with('/') || uri.path() == SEALED_PATH {
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

async fn buffer_response(response: Response) -> (StatusCode, HeaderMap, Bytes) {
    let is_stream = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if is_stream {
        return plain_error(
            StatusCode::NOT_IMPLEMENTED,
            "a streaming response cannot be sealed; open the stream directly",
        );
    }
    let (parts, body) = response.into_parts();
    match to_bytes(body, MAX_SEALED_BYTES).await {
        Ok(bytes) => (parts.status, parts.headers, bytes),
        Err(_) => plain_error(StatusCode::BAD_GATEWAY, "the response is too large to seal"),
    }
}

fn plain_error(status: StatusCode, message: &str) -> (StatusCode, HeaderMap, Bytes) {
    let mut headers = HeaderMap::new();
    let _previous = headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let body = json!({ "error": { "message": message } }).to_string();
    (status, headers, Bytes::from(body))
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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests;
