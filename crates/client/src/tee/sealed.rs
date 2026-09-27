//! Requests sealed end to end to a TEE node's attested transport key.
//!
//! For a node whose TLS ends outside the TD — at a proxy, a load balancer, a
//! relay — where everything crossing that hop (bearer tokens, JSON-RPC
//! arguments, results) is readable. A [`SealedTransport`] opens a session with a
//! Noise NK handshake to the X25519 key the node's quote commits to, seals each
//! whole HTTP request under it, and opens the response frame by frame, so only
//! the attested TD reads either. The session keys come from both sides'
//! ephemeral keys: once a session is gone (an hour at most), what was sent in it
//! cannot be opened, not even with the transport key.
//!
//! The node's side, and the wire format this implements byte for byte (as
//! mero-js does), are in `calimero-server`'s `sealed` module:
//!
//! ```text
//! handshake   Noise_NK_25519_AESGCM_SHA256, prologue "calimero/sealed-http/v2"
//!   request   POST /sealed/v2/handshake   0x02 || transport_pk || Noise message 1
//!   response  0x02 || Noise message 2 (payload = session_id(16) || lifetime secs, u32 BE)
//!   keys      (req key, resp key) = Split()
//! exchange    POST /sealed/v2
//!   header    0x02 || session_id(16) || request_id (u64 BE; from 1, each used once)
//!   request   header || AES-256-GCM(req key, nonce = request_id || 0u32, aad = header, inner)
//!   response  frames, each u32 BE length || AES-256-GCM(resp key,
//!             nonce = request_id || frame index (u32 BE, from 0), aad = header, kind || data)
//!             kind 0 = head (JSON), 1 = body bytes, 2 = end; no end frame = cut short
//!   inner     u32 BE head length || head (JSON) || body
//! ```
//!
//! WebSockets cannot be sealed: an upgrade cannot cross a POST.

use core::time::Duration;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use eyre::{bail, eyre, Result, WrapErr};
use futures_util::{Stream, StreamExt};
use reqwest::header::{HeaderValue, CONTENT_TYPE, RETRY_AFTER};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use url::Url;
use zeroize::Zeroizing;

use super::{Attestor, Bind};
use crate::connection::{read_body_capped, resolve_path};

/// Where a session is opened, under the node's base URL.
pub const HANDSHAKE_PATH: &str = "sealed/v2/handshake";
/// Where sealed requests are posted, under the node's base URL.
pub const SEALED_PATH: &str = "sealed/v2";
/// Content type of every sealed body.
pub const SEALED_CONTENT_TYPE: &str = "application/vnd.calimero.sealed";

const VERSION: u8 = 2;
const NOISE_PARAMS: &str = "Noise_NK_25519_AESGCM_SHA256";
const PROLOGUE: &[u8] = b"calimero/sealed-http/v2";
const SESSION_ID_LEN: usize = 16;
/// `version || session_id || request_id`, a request's AAD and its frames'.
const HEADER_LEN: usize = 1 + SESSION_ID_LEN + 8;
/// Noise message 1 of NK: the client's ephemeral key and an empty payload's tag.
const MESSAGE_1_LEN: usize = 32 + 16;
const FRAME_HEAD: u8 = 0;
const FRAME_DATA: u8 = 1;
const FRAME_END: u8 = 2;
/// The largest frame the node sends: a kind byte, at most 64 KiB of data, a
/// tag. A larger one is refused before it is buffered, so a proxy cannot make
/// the client hold an unbounded frame it has yet to authenticate.
const MAX_FRAME_LEN: usize = 1 + 64 * 1024 + 16;
/// Far more than a handshake reply (69 bytes) ever is.
const MAX_HANDSHAKE_REPLY: usize = 1024;
/// A refusal is a small JSON error.
const MAX_REFUSAL: usize = 64 * 1024;
/// Open a new session this long before the node would drop the current one.
const RENEW_BEFORE: Duration = Duration::from_secs(60);
/// Longest pause honoured when the node asks to retry a handshake later.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);
/// Responses these statuses carry have no body, and the node sends none.
const NULL_BODY_STATUSES: [u16; 4] = [101, 204, 205, 304];

/// The node refused the envelope itself, before anything inside it ran.
#[derive(Debug, thiserror::Error)]
#[error("sealed request refused: {message}")]
pub struct SealedRefusal {
    /// The HTTP status of the refusal.
    pub status: u16,
    /// The node's refusal code (`unknown_session`, `busy`, ...), when the
    /// refusal came from the node rather than from something in front of it.
    pub code: Option<String>,
    pub message: String,
    /// How long the node asked to wait before trying again, if it did.
    pub retry_after: Option<Duration>,
}

/// The node no longer holds the transport key this client was given: it
/// restarted, and its new key has to be attested.
#[derive(Debug, thiserror::Error)]
#[error("the node no longer holds the attested transport key; attest again for the current one")]
pub struct StaleTransportKey;

/// Seals every request to a node's attested transport key.
#[derive(Debug)]
pub struct SealedTransport {
    api_url: Url,
    http: reqwest::Client,
    key: KeySource,
    state: Mutex<State>,
}

#[derive(Debug)]
enum KeySource {
    /// A key verified some other way; a restarted node surfaces as
    /// [`StaleTransportKey`].
    Fixed([u8; 32]),
    /// Attested on first use and again whenever the node restarts.
    Attested(Attestor),
}

#[derive(Debug, Default)]
struct State {
    transport_key: Option<[u8; 32]>,
    session: Option<Arc<Session>>,
}

impl SealedTransport {
    /// Seal to the transport key of the node at `api_url`, attested with
    /// `attestor` before the first request and again after the node restarts.
    /// Envelopes travel over `http`, which may itself be pinned
    /// ([`super::tls::AttestedTls`]).
    #[must_use]
    pub fn attested(api_url: Url, http: reqwest::Client, attestor: Attestor) -> Self {
        Self::with_source(api_url, http, KeySource::Attested(attestor))
    }

    /// Seal to a transport key verified some other way. A restarted node
    /// holds a new key, and then every request fails with
    /// [`StaleTransportKey`]; [`Self::attested`] handles that itself.
    #[must_use]
    pub fn with_key(api_url: Url, http: reqwest::Client, transport_public_key: [u8; 32]) -> Self {
        Self::with_source(api_url, http, KeySource::Fixed(transport_public_key))
    }

    fn with_source(api_url: Url, http: reqwest::Client, key: KeySource) -> Self {
        Self {
            api_url,
            http,
            key,
            state: Mutex::new(State::default()),
        }
    }

    /// Send `request` sealed, and open its response.
    ///
    /// A request for a URL outside the node's base URL is refused rather than
    /// sent in the clear, and so is one whose body is a stream: sealing needs
    /// the whole body.
    ///
    /// # Errors
    /// When the request cannot be sealed, the node refuses the envelope, or the
    /// response does not open.
    pub async fn execute(&self, request: reqwest::Request) -> Result<reqwest::Response> {
        let (head, body) = self.unwrap(&request)?;
        let session = self.current_session().await?;
        match self.send_in(&session, &head, &body).await {
            // Refused before anything in it ran, so it is safe to send again
            // under a new session: the node forgot this one (it restarted, or
            // dropped it to make room).
            Err(err) if refusal_code(&err) == Some("unknown_session") => {
                let session = self.renew(&session).await?;
                self.send_in(&session, &head, &body).await
            }
            outcome => outcome,
        }
    }

    /// The inner request's head and body: the path is relative to the node's
    /// base URL, as the node routes it under the mount the envelope arrived on.
    fn unwrap(&self, request: &reqwest::Request) -> Result<(RequestHead, Vec<u8>)> {
        let url = request.url();
        let base_path = self.api_url.path().trim_end_matches('/');
        let within = url.scheme() == self.api_url.scheme()
            && url.host_str() == self.api_url.host_str()
            && url.port_or_known_default() == self.api_url.port_or_known_default()
            && url
                .path()
                .strip_prefix(base_path)
                .is_some_and(|rest| rest.starts_with('/'));
        if !within {
            bail!(
                "refusing to send {url} outside the sealed node at {}",
                self.api_url
            );
        }
        let mut path = url.path()[base_path.len()..].to_owned();
        if let Some(query) = url.query() {
            path.push('?');
            path.push_str(query);
        }
        let headers = request
            .headers()
            .iter()
            .map(|(name, value)| {
                value
                    .to_str()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
                    .map_err(|_| eyre!("header {name} is not text, so it cannot be sealed"))
            })
            .collect::<Result<_>>()?;
        let body = match request.body() {
            None => Vec::new(),
            Some(body) => body
                .as_bytes()
                .ok_or_else(|| eyre!("a streamed body cannot be sealed"))?
                .to_vec(),
        };
        Ok((
            RequestHead {
                method: request.method().as_str().to_owned(),
                path,
                headers,
            },
            body,
        ))
    }

    /// The session to use now, opened if there is none or it is about to lapse.
    async fn current_session(&self) -> Result<Arc<Session>> {
        let mut state = self.state.lock().await;
        if let Some(session) = &state.session {
            if Instant::now() < session.renew_at {
                return Ok(Arc::clone(session));
            }
        }
        self.open_session(&mut state).await
    }

    /// Replace `stale` with a new session, unless another request already has:
    /// concurrent requests that all find it forgotten open one between them.
    async fn renew(&self, stale: &Arc<Session>) -> Result<Arc<Session>> {
        let mut state = self.state.lock().await;
        if let Some(session) = &state.session {
            if !Arc::ptr_eq(session, stale) {
                return Ok(Arc::clone(session));
            }
        }
        self.open_session(&mut state).await
    }

    async fn open_session(&self, state: &mut State) -> Result<Arc<Session>> {
        state.session = None;
        let key = match state.transport_key {
            Some(key) => key,
            None => self.transport_key().await?,
        };
        state.transport_key = Some(key);
        let session = match self.handshake(&key).await {
            Err(err) if err.downcast_ref::<StaleTransportKey>().is_some() => {
                let KeySource::Attested(_) = &self.key else {
                    return Err(err);
                };
                // The node restarted: attest its new key, through the same
                // verifier, and open the session to that.
                state.transport_key = None;
                let key = self.transport_key().await?;
                state.transport_key = Some(key);
                self.handshake(&key).await?
            }
            outcome => outcome?,
        };
        let session = Arc::new(session);
        state.session = Some(Arc::clone(&session));
        Ok(session)
    }

    async fn transport_key(&self) -> Result<[u8; 32]> {
        match &self.key {
            KeySource::Fixed(key) => Ok(*key),
            KeySource::Attested(attestor) => attestor
                .attest(
                    &self.http,
                    &self.api_url,
                    Bind {
                        transport_key: true,
                        ..Bind::default()
                    },
                )
                .await?
                .transport_public_key
                .ok_or_else(|| eyre!("the attestation named no transport key")),
        }
    }

    async fn handshake(&self, transport_key: &[u8; 32]) -> Result<Session> {
        match self.handshake_once(transport_key).await {
            // The node limits how fast sessions open. A refused handshake
            // opened nothing, so trying once more after the pause it asks for
            // is safe.
            Err(err) if refusal_code(&err) == Some("busy") => {
                let pause = err
                    .downcast_ref::<SealedRefusal>()
                    .and_then(|refusal| refusal.retry_after)
                    .unwrap_or(Duration::from_secs(1));
                tokio::time::sleep(pause).await;
                self.handshake_once(transport_key).await
            }
            outcome => outcome,
        }
    }

    async fn handshake_once(&self, transport_key: &[u8; 32]) -> Result<Session> {
        let mut noise = noise_builder()?
            .remote_public_key(transport_key)
            .build_initiator()
            .map_err(|err| eyre!("failed to start the handshake: {err}"))?;
        let mut message_1 = [0u8; MESSAGE_1_LEN];
        let len = noise
            .write_message(&[], &mut message_1)
            .map_err(|err| eyre!("failed to start the handshake: {err}"))?;
        let body = [&[VERSION][..], transport_key, &message_1[..len]].concat();

        let response = self
            .http
            .post(resolve_path(&self.api_url, HANDSHAKE_PATH)?)
            .header(CONTENT_TYPE, SEALED_CONTENT_TYPE)
            .body(body)
            .send()
            .await
            .wrap_err("the node did not answer the handshake")?;
        if !is_sealed(&response) {
            return Err(refusal(response).await);
        }
        let reply = read_body_capped(response, MAX_HANDSHAKE_REPLY)
            .await
            .wrap_err("the handshake reply is malformed")?;
        finish(&mut noise, &reply)
    }

    async fn send_in(
        &self,
        session: &Arc<Session>,
        head: &RequestHead,
        body: &[u8],
    ) -> Result<reqwest::Response> {
        let (request_id, header, envelope) = session.seal(head, body)?;
        let response = self
            .http
            .post(resolve_path(&self.api_url, SEALED_PATH)?)
            .header(CONTENT_TYPE, SEALED_CONTENT_TYPE)
            .body(envelope)
            .send()
            .await
            .wrap_err("the node did not answer the sealed request")?;
        if !is_sealed(&response) {
            return Err(refusal(response).await);
        }
        open_response(response, Arc::clone(session), header, request_id).await
    }
}

fn noise_builder() -> Result<snow::Builder<'static>> {
    let params = NOISE_PARAMS
        .parse()
        .map_err(|err| eyre!("invalid Noise parameters: {err:?}"))?;
    Ok(snow::Builder::new(params).prologue(PROLOGUE))
}

/// Finish the handshake with the node's reply.
fn finish(noise: &mut snow::HandshakeState, reply: &[u8]) -> Result<Session> {
    let Some((&VERSION, message_2)) = reply.split_first() else {
        bail!("the handshake reply is malformed");
    };
    let mut payload = [0u8; 64];
    let len = noise.read_message(message_2, &mut payload).map_err(|_| {
        eyre!("the handshake reply did not open: it did not come from the attested node")
    })?;
    if len != SESSION_ID_LEN + 4 {
        bail!("the handshake reply is malformed");
    }
    let mut id = [0u8; SESSION_ID_LEN];
    id.copy_from_slice(&payload[..SESSION_ID_LEN]);
    let mut lifetime = [0u8; 4];
    lifetime.copy_from_slice(&payload[SESSION_ID_LEN..len]);
    let lifetime = Duration::from_secs(u32::from_be_bytes(lifetime).into());
    let (request, response) = noise.dangerously_get_raw_split();
    let (request, response) = (Zeroizing::new(request), Zeroizing::new(response));
    Ok(Session {
        id,
        request_key: aead_key(&request)?,
        response_key: aead_key(&response)?,
        next_request_id: AtomicU64::new(1),
        renew_at: Instant::now() + lifetime.saturating_sub(RENEW_BEFORE),
    })
}

/// One sealed session: its id, its two keys, and the next request id.
struct Session {
    id: [u8; SESSION_ID_LEN],
    request_key: LessSafeKey,
    response_key: LessSafeKey,
    next_request_id: AtomicU64,
    renew_at: Instant,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &hex::encode(self.id))
            .finish_non_exhaustive()
    }
}

impl Session {
    fn header(&self, request_id: u64) -> [u8; HEADER_LEN] {
        let mut header = [0u8; HEADER_LEN];
        header[0] = VERSION;
        header[1..=SESSION_ID_LEN].copy_from_slice(&self.id);
        header[1 + SESSION_ID_LEN..].copy_from_slice(&request_id.to_be_bytes());
        header
    }

    /// Seal a request under the next id: the id, the header, the envelope.
    fn seal(&self, head: &RequestHead, body: &[u8]) -> Result<(u64, [u8; HEADER_LEN], Vec<u8>)> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let header = self.header(request_id);
        let head = serde_json::to_vec(head)?;
        let head_len = u32::try_from(head.len()).map_err(|_| eyre!("request head too large"))?;
        let mut sealed = Vec::with_capacity(HEADER_LEN + 4 + head.len() + body.len() + 16);
        sealed.extend_from_slice(&header);
        let mut inner = Vec::with_capacity(4 + head.len() + body.len() + 16);
        inner.extend_from_slice(&head_len.to_be_bytes());
        inner.extend_from_slice(&head);
        inner.extend_from_slice(body);
        self.request_key
            .seal_in_place_append_tag(nonce(request_id, 0), Aad::from(&header), &mut inner)
            .map_err(|_| eyre!("failed to seal the request"))?;
        sealed.extend_from_slice(&inner);
        Ok((request_id, header, sealed))
    }
}

fn aead_key(key: &[u8; 32]) -> Result<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| eyre!("invalid session key"))
}

/// `request_id || index`, as the node numbers them.
fn nonce(request_id: u64, index: u32) -> Nonce {
    let mut nonce = [0u8; NONCE_LEN];
    nonce[..8].copy_from_slice(&request_id.to_be_bytes());
    nonce[8..].copy_from_slice(&index.to_be_bytes());
    Nonce::assume_unique_for_key(nonce)
}

#[derive(Debug, Serialize)]
struct RequestHead {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
struct ResponseHead {
    status: u16,
    headers: Vec<(String, String)>,
}

/// Open a sealed response as it streams: wait for its head, then hand back a
/// response whose body opens each frame as it arrives. A frame that does not
/// open, arrives out of order, or a stream that stops before its end frame
/// errors the body rather than passing anything unauthenticated on.
async fn open_response(
    response: reqwest::Response,
    session: Arc<Session>,
    header: [u8; HEADER_LEN],
    request_id: u64,
) -> Result<reqwest::Response> {
    let mut frames = Frames {
        bytes: response.bytes_stream().boxed(),
        buffer: Vec::new(),
        session,
        header,
        request_id,
        index: 0,
    };
    let Some((FRAME_HEAD, head)) = frames.next().await? else {
        bail!("the sealed response is malformed");
    };
    let head: ResponseHead =
        serde_json::from_slice(&head).wrap_err("the sealed response head is malformed")?;

    let mut builder = http::Response::builder().status(head.status);
    for (name, value) in &head.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let body = if NULL_BODY_STATUSES.contains(&head.status) {
        reqwest::Body::from(Vec::new())
    } else {
        reqwest::Body::wrap_stream(frames.into_body())
    };
    let response = builder
        .body(body)
        .wrap_err("the sealed response head is malformed")?;
    Ok(reqwest::Response::from(response))
}

/// The length-prefixed frames of a sealed response, opened in order.
struct Frames {
    bytes: futures_util::stream::BoxStream<'static, reqwest::Result<Bytes>>,
    buffer: Vec<u8>,
    session: Arc<Session>,
    header: [u8; HEADER_LEN],
    request_id: u64,
    index: u32,
}

impl Frames {
    /// The next frame's kind and data, or `None` when the stream ends.
    async fn next(&mut self) -> Result<Option<(u8, Vec<u8>)>> {
        if !self.fill(4).await? {
            return Ok(None);
        }
        let mut len = [0u8; 4];
        len.copy_from_slice(&self.buffer[..4]);
        let len = usize::try_from(u32::from_be_bytes(len))?;
        if len > MAX_FRAME_LEN {
            bail!("the sealed response is malformed: a frame is larger than the node sends");
        }
        if !self.fill(4 + len).await? {
            return Ok(None);
        }
        let mut frame: Vec<u8> = self.buffer.drain(..4 + len).skip(4).collect();
        let plain = self
            .session
            .response_key
            .open_in_place(
                nonce(self.request_id, self.index),
                Aad::from(&self.header),
                &mut frame,
            )
            .map_err(|_| {
                eyre!(
                    "a sealed response frame did not open: it was not sealed by the attested node"
                )
            })?;
        self.index = self
            .index
            .checked_add(1)
            .ok_or_else(|| eyre!("the sealed response has too many frames"))?;
        let Some((&kind, data)) = plain.split_first() else {
            bail!("the sealed response is malformed");
        };
        Ok(Some((kind, data.to_vec())))
    }

    /// Buffer at least `len` bytes; false when the stream ends first.
    async fn fill(&mut self, len: usize) -> Result<bool> {
        while self.buffer.len() < len {
            match self.bytes.next().await {
                Some(chunk) => self.buffer.extend_from_slice(&chunk?),
                None => return Ok(false),
            }
        }
        Ok(true)
    }

    /// The body: data frames until the end frame.
    fn into_body(self) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
        futures_util::stream::unfold(Some(self), |frames| async move {
            let mut frames = frames?;
            let item = match frames.next().await {
                Ok(Some((FRAME_DATA, data))) => Ok(Bytes::from(data)),
                Ok(Some((FRAME_END, _))) => return None,
                Ok(Some(_)) => Err(io::Error::other("the sealed response is malformed")),
                Ok(None) => Err(io::Error::other("the sealed response was cut short")),
                Err(err) => Err(io::Error::other(err.to_string())),
            };
            // After an error nothing more is read: what follows is not trusted.
            let next = item.is_ok().then_some(frames);
            Some((item, next))
        })
    }
}

fn is_sealed(response: &reqwest::Response) -> bool {
    response.headers().get(CONTENT_TYPE) == Some(&HeaderValue::from_static(SEALED_CONTENT_TYPE))
}

/// The error a refused envelope becomes: [`StaleTransportKey`] when the node
/// has a new key, else a [`SealedRefusal`].
async fn refusal(response: reqwest::Response) -> eyre::Report {
    #[derive(Deserialize)]
    struct Body {
        error: Detail,
    }
    #[derive(Deserialize)]
    struct Detail {
        code: Option<String>,
        message: Option<String>,
    }

    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(|secs| Duration::from_secs(secs).min(MAX_RETRY_AFTER));
    // Not the node's JSON refusal (a proxy's error page, most likely): the
    // status is all that can honestly be reported.
    let detail = read_body_capped(response, MAX_REFUSAL)
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<Body>(&body).ok())
        .map(|body| body.error);
    let code = detail.as_ref().and_then(|detail| detail.code.clone());
    if code.as_deref() == Some("stale_transport_key") {
        return StaleTransportKey.into();
    }
    let message = detail
        .and_then(|detail| detail.message)
        .unwrap_or_else(|| format!("HTTP {status}"));
    SealedRefusal {
        status,
        code,
        message,
        retry_after,
    }
    .into()
}

fn refusal_code(err: &eyre::Report) -> Option<&str> {
    err.downcast_ref::<SealedRefusal>()
        .and_then(|refusal| refusal.code.as_deref())
}

#[cfg(test)]
mod tests;
