//! The runtime every route is sent through: the part of the client that is
//! not a route.
//!
//! A route layer (the one generated from the node's OpenAPI document,
//! [`crate::routes`]) knows a method, a path and what the route takes and
//! answers. Everything else is [`Transport`]: the sealed channel to a node
//! whose attestation passed the [`Policy`] first, the credential, the
//! session's refresh on a `401`, the two-round approvals (`412
//! CONFIRM_REQUIRED` with a passkey, `428 OPERATOR_SIGNATURE_REQUIRED` with the
//! operator's key: [`Transport::approved`]), error mapping ([`answer`]) and
//! server-sent events ([`Transport::events`]). `meroctl proxy` forwards plain
//! HTTP through [`Transport::send`].

use std::borrow::Cow;
use std::sync::Arc;

use calimero_sealed::client::{SealedRefusal, SealedTransport};
use futures_util::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use url::Url;

use crate::attest::{HttpAttestor, Policy};
use crate::route::OperatorSigner;
use crate::{Error, Passkey, Result, Session};

/// The path a session is refreshed at; a `401` there is not refreshed again.
pub(crate) const REFRESH_PATH: &str = "auth/refresh";

/// What a request carries, kept so it can be sent again after a refresh.
#[derive(Clone, Debug, Default)]
pub enum Body {
    #[default]
    Empty,
    Json(Value),
    Bytes {
        content_type: Cow<'static, str>,
        bytes: bytes::Bytes,
    },
}

/// Which credential a request carries.
#[derive(Clone, Debug, Default)]
pub enum Credential {
    /// The transport's own (an API key or a session), and a session's refresh
    /// on a `401`.
    #[default]
    Held,
    /// This `Authorization` value, sent as given and never refreshed: a
    /// caller's own credential (what `meroctl proxy` passes through).
    Authorization(HeaderValue),
    /// None.
    Anonymous,
}

/// One request to a node route.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    /// Relative to the node (`apps/{app}/intents`; a leading `/` is dropped).
    /// It may carry its own query.
    pub path: String,
    /// Query parameters appended to the path's own.
    pub query: Vec<(String, String)>,
    /// Headers sent as given (`content-type` is the body's when it has one).
    pub headers: HeaderMap,
    pub body: Body,
    pub credential: Credential,
}

impl Request {
    #[must_use]
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            headers: HeaderMap::new(),
            body: Body::Empty,
            credential: Credential::Held,
        }
    }

    /// Add the query parameter `name=value` (encoded here).
    #[must_use]
    pub fn query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((name.into(), value.into()));
        self
    }

    /// A JSON body.
    #[must_use]
    pub fn json(mut self, body: Value) -> Self {
        self.body = Body::Json(body);
        self
    }

    /// A raw body of `content_type`.
    #[must_use]
    pub fn bytes(
        mut self,
        content_type: impl Into<Cow<'static, str>>,
        bytes: impl Into<bytes::Bytes>,
    ) -> Self {
        self.body = Body::Bytes {
            content_type: content_type.into(),
            bytes: bytes.into(),
        };
        self
    }

    #[must_use]
    pub fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    #[must_use]
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        let _prev = self.headers.insert(name, value);
        self
    }

    #[must_use]
    pub fn credential(mut self, credential: Credential) -> Self {
        self.credential = credential;
        self
    }

    fn relative_path(&self) -> &str {
        self.path.trim_start_matches('/')
    }
}

/// The error a node's answer `status` with body `v` stands for.
pub(crate) fn api_error(status: u16, v: &Value) -> Error {
    let code = v["error"].as_str().unwrap_or("ERROR").to_owned();
    let message = v["message"].as_str().unwrap_or_default().to_owned();
    let details = v.get("details").cloned().unwrap_or(Value::Null);
    if status == 421 && code == "WRONG_NODE" {
        return Error::WrongNode { message, details };
    }
    Error::Api {
        status,
        code,
        message,
        details,
    }
}

/// `resp`'s JSON body, or the error its status stands for.
///
/// # Errors
/// [`Error::Api`] (or [`Error::WrongNode`]) for a status of 400 or more.
pub async fn answer(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status().as_u16();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if status >= 400 {
        return Err(api_error(status, &v));
    }
    Ok(v)
}

/// What a request sealed and sent became: a refusal of its envelope as too
/// large (`413`) is the answer a caller acts on, the same `TOO_LARGE` the node
/// gives a body past its own limits, so it is an [`Error::Api`]. It comes back
/// in the clear, as every refusal of an envelope does: the node refuses it
/// before the request can be opened, so there is no request to seal it to.
pub(crate) fn sealed_error(e: eyre::Report) -> Error {
    match e.downcast_ref::<SealedRefusal>() {
        Some(refusal) if refusal.status == 413 => Error::Api {
            status: 413,
            code: "TOO_LARGE".to_owned(),
            message: refusal.message.clone(),
            details: Value::Null,
        },
        _ => Error::Transport(e),
    }
}

/// The sealed, attested channel to one node, with the credential its
/// requests carry.
pub struct Transport {
    base: Url,
    http: reqwest::Client,
    sealed: Arc<SealedTransport>,
    auth: RwLock<Option<String>>,
    session: RwLock<Option<Session>>,
    /// One refresh at a time: requests that got a `401` together refresh once
    /// (a refresh token presented twice ends the session).
    refreshing: tokio::sync::Mutex<()>,
}

impl Transport {
    /// The node at `base`, trusted only once `policy` accepts its attestation;
    /// each request may take 60 s.
    ///
    /// # Errors
    /// When `base` is not a URL or the HTTP client cannot be built.
    pub fn connect(base: &str, policy: Policy) -> eyre::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        Self::with_http(base, policy, http)
    }

    /// [`Self::connect`] over `http` (its timeouts, its proxy settings).
    ///
    /// # Errors
    /// When `base` is not a URL.
    pub fn with_http(base: &str, policy: Policy, http: reqwest::Client) -> eyre::Result<Self> {
        let base: Url = if base.ends_with('/') {
            base.parse()?
        } else {
            format!("{base}/").parse()?
        };
        let sealed = Arc::new(SealedTransport::attested(
            base.clone(),
            http.clone(),
            Arc::new(HttpAttestor {
                policy,
                seen_release: Arc::default(),
            }),
        ));
        Ok(Self {
            base,
            http,
            sealed,
            auth: RwLock::new(None),
            session: RwLock::new(None),
            refreshing: tokio::sync::Mutex::new(()),
        })
    }

    /// A transport over a sealed channel made elsewhere (a test's, sealed to
    /// a known key).
    #[cfg(test)]
    pub(crate) fn from_parts(
        base: Url,
        http: reqwest::Client,
        sealed: Arc<SealedTransport>,
    ) -> Self {
        Self {
            base,
            http,
            sealed,
            auth: RwLock::new(None),
            session: RwLock::new(None),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }

    /// The node's base URL (ends in `/`).
    #[must_use]
    pub fn base(&self) -> &Url {
        &self.base
    }

    /// The unsealed HTTP client, for the one route served in the clear
    /// (`/attestation`).
    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Attest the node and open the sealed session now, so a node whose quote
    /// the policy refuses is found before any request.
    ///
    /// # Errors
    /// When the node cannot be attested, or refuses the handshake.
    pub async fn attest(&self) -> Result<()> {
        self.sealed.connect().await.map_err(Error::Transport)
    }

    /// Use an API key for every request.
    pub async fn use_api_key(&self, key: &str) {
        *self.auth.write().await = Some(key.to_owned());
    }

    /// Use a session token for every request, refreshed on a `401`.
    pub async fn use_session(&self, session: Session) {
        *self.auth.write().await = Some(session.session_token.clone());
        *self.session.write().await = Some(session);
    }

    /// The session held now (a refresh replaces it).
    pub async fn session(&self) -> Option<Session> {
        self.session.read().await.clone()
    }

    /// Send `req` and answer the node's response, whatever its status. With
    /// [`Credential::Held`], a `401` to a session this transport can refresh
    /// (it holds the refresh token) is refreshed once, on this node, and the
    /// request sent once more: the node that issued the session may be gone,
    /// or have been removed, and a refresh works on any node.
    ///
    /// # Errors
    /// When the node cannot be reached or attested, or its answer not opened
    /// ([`Error::Transport`]); a sealed envelope it refuses as too large is
    /// [`Error::Api`] `413 TOO_LARGE`.
    pub async fn send(&self, req: Request) -> Result<reqwest::Response> {
        let held = matches!(req.credential, Credential::Held);
        let token = if held {
            self.auth.read().await.clone()
        } else {
            None
        };
        let resp = self.send_once(&req, token.as_deref()).await?;
        if !held || resp.status().as_u16() != 401 || req.relative_path() == REFRESH_PATH {
            return Ok(resp);
        }
        let Some(sent) = token else { return Ok(resp) };
        match self.refreshed_after(&sent).await {
            Some(fresh) => self.send_once(&req, Some(&fresh)).await,
            None => Ok(resp),
        }
    }

    /// [`Self::send`]'s JSON answer, a refusal as its [`Error`].
    ///
    /// # Errors
    /// As [`Self::send`], and [`answer`].
    pub async fn json(&self, req: Request) -> Result<Value> {
        answer(self.send(req).await?).await
    }

    /// [`Self::json`], answering a `412 CONFIRM_REQUIRED` with `passkey`'s
    /// assertion over the node's challenge: the same JSON body again, with
    /// `confirm: { challengeId, assertion }`.
    ///
    /// # Errors
    /// As [`Self::json`].
    pub async fn confirmed(&self, req: Request, passkey: &mut dyn Passkey) -> Result<Value> {
        self.approved(req, Some(passkey), None).await
    }

    /// [`Self::json`], answering the two-round refusals a route may give
    /// first: a `412 CONFIRM_REQUIRED` with `passkey`'s assertion over the
    /// node's challenge (`confirm: { challengeId, assertion }`), and a `428
    /// OPERATOR_SIGNATURE_REQUIRED` with `operator`'s signature over the
    /// statement it names (`operatorProof: { nonce, signature }`), each into
    /// the same JSON body, sent again. Without the one a refusal asks for, the
    /// refusal is returned. Each is answered once: a challenge is single-use.
    ///
    /// # Errors
    /// As [`Self::json`].
    pub async fn approved(
        &self,
        req: Request,
        mut passkey: Option<&mut dyn Passkey>,
        operator: Option<&OperatorSigner<'_>>,
    ) -> Result<Value> {
        let mut req = req;
        let (mut confirmed, mut signed) = (false, false);
        loop {
            let e = match self.json(req.clone()).await {
                Err(e) => e,
                done => return done,
            };
            let details = e.details().cloned().unwrap_or(Value::Null);
            let (field, value) = match (e.code(), passkey.as_deref_mut(), operator) {
                (Some("CONFIRM_REQUIRED"), Some(pk), _) if !confirmed => {
                    confirmed = true;
                    let challenge = calimero_identity::from_b64url(
                        details["challenge"].as_str().unwrap_or_default(),
                    )
                    .map_err(|e| eyre::eyre!("{e}"))?;
                    let assertion = pk.get(&challenge);
                    (
                        "confirm",
                        json!({ "challengeId": details["challengeId"], "assertion": assertion }),
                    )
                }
                (Some("OPERATOR_SIGNATURE_REQUIRED"), _, Some(sign)) if !signed => {
                    signed = true;
                    let message = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        details["message"].as_str().unwrap_or_default(),
                    )
                    .map_err(eyre::Report::from)?;
                    (
                        "operatorProof",
                        json!({ "nonce": details["nonce"], "signature": hex::encode(sign(&message)) }),
                    )
                }
                _ => return Err(e),
            };
            let Body::Json(Value::Object(b)) = &mut req.body else {
                return Err(e);
            };
            let _prev = b.insert(field.into(), value);
        }
    }

    /// A server-sent event stream: `(id, event, data)` per frame, `data`
    /// parsed as JSON when it is.
    ///
    /// # Errors
    /// As [`Self::send`]; a refused stream is [`Error::Api`] `EVENTS`.
    pub async fn events(
        &self,
        req: Request,
    ) -> Result<impl Stream<Item = (String, String, Value)>> {
        let resp = self.send(req).await?;
        if !resp.status().is_success() {
            return Err(Error::Api {
                status: resp.status().as_u16(),
                code: "EVENTS".into(),
                message: "event stream refused".into(),
                details: Value::Null,
            });
        }
        Ok(Box::pin(sse_frames(resp.bytes_stream())))
    }

    /// Swap the refresh token for a new session, on this node (any node of
    /// the cluster takes it).
    ///
    /// # Errors
    /// When no session is held, or the node refuses the refresh.
    pub async fn refresh(&self) -> Result<Session> {
        let current = self
            .session()
            .await
            .ok_or_else(|| eyre::eyre!("no session"))?;
        let token = self.auth.read().await.clone();
        let req = Request::new(Method::POST, REFRESH_PATH)
            .json(json!({ "refreshToken": current.refresh_token }));
        let resp = self.send_once(&req, token.as_deref()).await?;
        let s: Session = serde_json::from_value(answer(resp).await?).map_err(eyre::Report::from)?;
        self.use_session(s.clone()).await;
        Ok(s)
    }

    /// The session token to send again after `sent` was answered `401`: one a
    /// concurrent request already refreshed into, else a refresh's. `None` when
    /// `sent` is not a session this transport can refresh (an API key, a
    /// session without its refresh token) or the refresh fails.
    async fn refreshed_after(&self, sent: &str) -> Option<String> {
        let _one = self.refreshing.lock().await;
        let session = self.session.read().await.clone()?;
        if session.session_token != sent {
            let current = self.auth.read().await.clone()?;
            return (current != sent && current == session.session_token).then_some(current);
        }
        if session.refresh_token.is_empty() {
            return None;
        }
        self.refresh().await.ok().map(|s| s.session_token)
    }

    async fn send_once(&self, req: &Request, token: Option<&str>) -> Result<reqwest::Response> {
        let mut url = self
            .base
            .join(req.relative_path())
            .map_err(eyre::Report::from)?;
        if !req.query.is_empty() {
            let _pairs = url.query_pairs_mut().extend_pairs(&req.query);
        }
        let mut builder = self
            .http
            .request(req.method.clone(), url)
            .headers(req.headers.clone());
        match (&req.credential, token) {
            (Credential::Authorization(value), _) => {
                builder = builder.header(reqwest::header::AUTHORIZATION, value.clone());
            }
            (Credential::Held, Some(token)) => builder = builder.bearer_auth(token),
            _ => {}
        }
        builder = match &req.body {
            Body::Empty => builder,
            Body::Json(b) => builder.json(b),
            Body::Bytes {
                content_type,
                bytes,
            } => builder
                .header("content-type", &**content_type)
                .body(bytes.clone()),
        };
        let request = builder.build().map_err(eyre::Report::from)?;
        self.sealed.execute(request).await.map_err(sealed_error)
    }
}

/// Server-sent event frames from a response body: `(id, event, data)` each.
fn sse_frames<S>(bytes: S) -> impl Stream<Item = (String, String, Value)>
where
    S: Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
{
    futures_util::stream::unfold((bytes, String::new()), |(mut bytes, mut buf)| async move {
        loop {
            if let Some(pos) = buf.find("\n\n") {
                let frame: String = buf.drain(..pos + 2).collect();
                let (mut id, mut kind, mut data) = (String::new(), String::new(), String::new());
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("id:") {
                        v.trim().clone_into(&mut id);
                    } else if let Some(v) = line.strip_prefix("event:") {
                        v.trim().clone_into(&mut kind);
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if kind.is_empty() && data.is_empty() {
                    continue;
                }
                let value = serde_json::from_str(&data).unwrap_or(Value::String(data));
                return Some(((id, kind, value), (bytes, buf)));
            }
            match bytes.next().await {
                Some(Ok(chunk)) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                _ => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use serde_json::json;

    use super::{api_error, sse_frames, Body, Credential, Request};
    use crate::Error;
    use reqwest::Method;

    #[test]
    fn a_request_builds_what_a_route_layer_names() {
        let req = Request::new(Method::DELETE, "/orgs/o-1")
            .query("cascade", "true")
            .json(json!({ "reason": "x" }))
            .credential(Credential::Anonymous);
        assert_eq!(req.relative_path(), "orgs/o-1");
        assert_eq!(req.query, vec![("cascade".into(), "true".into())]);
        assert!(matches!(req.body, Body::Json(ref v) if v["reason"] == "x"));
        assert!(matches!(req.credential, Credential::Anonymous));
        let raw = Request::new(Method::POST, "packages").bytes("application/octet-stream", vec![1]);
        assert!(
            matches!(raw.body, Body::Bytes { ref content_type, ref bytes } if content_type == "application/octet-stream" && bytes.as_ref() == [1])
        );
    }

    #[test]
    fn a_refusal_maps_to_its_error() {
        let e = api_error(
            412,
            &json!({ "error": "CONFIRM_REQUIRED", "message": "m", "details": { "challengeId": "c" } }),
        );
        assert_eq!(e.code(), Some("CONFIRM_REQUIRED"));
        assert_eq!(e.details().unwrap()["challengeId"], "c");
        assert!(matches!(
            api_error(421, &json!({ "error": "WRONG_NODE" })),
            Error::WrongNode { .. }
        ));
    }

    #[tokio::test]
    async fn server_sent_events_decode_across_chunks() {
        let chunks: Vec<reqwest::Result<bytes::Bytes>> = vec![
            Ok("id: 1\nevent: committed\nda".into()),
            Ok("ta: {\"index\":1}\n\n: keep-alive\n\nevent: note\ndata: plain\n\n".into()),
        ];
        let frames: Vec<_> = sse_frames(futures_util::stream::iter(chunks))
            .collect()
            .await;
        assert_eq!(
            frames,
            vec![
                ("1".into(), "committed".into(), json!({ "index": 1 })),
                (String::new(), "note".into(), json!("plain")),
            ]
        );
    }
}
