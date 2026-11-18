//! The sealed channel the Python layer's generated client runs on.
//!
//! The routes are a client generated from core's OpenAPI document
//! (`calimero_client_py/openapi`); what it cannot be generated from is
//! here: each request is sealed to the transport key the node's attestation
//! binds, and opened again, by core's `calimero-sealed`, with the credential
//! the client holds. The generated code's httpx transport
//! (`calimero_client_py.sealed.SealedTransport`) hands this a request's
//! method, path, headers and body and gets the node's answer as it came:
//! status, headers and body bytes.
//!
//! Core's `calimero_client::Client` answers JSON only (a status other than an
//! error is dropped, and a body that is not JSON with it), so each node keeps
//! its own [`SealedTransport`] for these requests, attested under the same
//! policy. Core's client stays for what is not a plain request: the event
//! streams, the attestation document and `verify_app_cluster`.

use std::ops::Deref;

use calimero_client::HttpAttestor;
use calimero_sealed::client::{SealedRefusal, SealedTransport};
use url::Url;

use super::prelude::*;
use super::recover::Node;
use super::tls::ExtraRoots;

/// One node of the cluster: core's client, and the sealed channel the
/// generated client's requests take.
pub(crate) struct SealedNode {
    client: Client,
    sealed: SealedTransport,
    base: Url,
    http: reqwest::Client,
}

impl Deref for SealedNode {
    type Target = Client;

    fn deref(&self) -> &Client {
        &self.client
    }
}

/// A request as the generated client made it: `path` is relative to the
/// node's base URL, query included.
#[derive(Clone, Debug)]
pub(crate) struct RawRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The node's answer, as it came.
#[derive(Clone, Debug)]
pub(crate) struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Headers the channel sets itself, or that mean nothing inside it: the
/// credential is the client's own (`use_api_key`, a session), and an encoded
/// answer would not be decoded.
const OWN_HEADERS: [&str; 6] = [
    "authorization",
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "accept-encoding",
];

impl SealedNode {
    /// A node at `url`, attested under `policy` before the first request
    /// (core's client and the channel each attest on first use).
    /// The channel trusts `roots` for TLS on top of the public roots.
    pub(crate) fn connect(url: &str, policy: Policy, roots: &ExtraRoots) -> eyre::Result<Self> {
        let client = Client::connect(url, policy.clone())?;
        let base: Url = if url.ends_with('/') {
            url.parse()?
        } else {
            format!("{url}/").parse()?
        };
        let http = roots.builder().timeout(Duration::from_secs(60)).build()?;
        let sealed = SealedTransport::attested(
            base.clone(),
            http.clone(),
            Arc::new(HttpAttestor {
                policy,
                seen_release: Arc::default(),
            }),
        );
        Ok(Self {
            client,
            sealed,
            base,
            http,
        })
    }

    /// Send `req` sealed with `token` as the bearer credential, and the
    /// answer as it came. Only the channel's own failures are errors (a node
    /// out of reach, an attestation that does not verify, an envelope
    /// refused as too large); every answer of the node is a response.
    pub(crate) async fn send(
        &self,
        req: &RawRequest,
        token: Option<&str>,
    ) -> Result<RawResponse, Error> {
        let url = self
            .base
            .join(req.path.trim_start_matches('/'))
            .map_err(eyre::Report::from)?;
        let method =
            reqwest::Method::from_bytes(req.method.as_bytes()).map_err(eyre::Report::from)?;
        let mut b = self.http.request(method, url);
        for (name, value) in &req.headers {
            if !OWN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                b = b.header(name, value);
            }
        }
        if let Some(token) = token {
            b = b.bearer_auth(token);
        }
        if !req.body.is_empty() {
            b = b.body(req.body.clone());
        }
        let request = b.build().map_err(eyre::Report::from)?;
        let resp = self.sealed.execute(request).await.map_err(sealed_error)?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_owned(), v.to_owned()))
            })
            .collect();
        let body = resp.bytes().await.map_err(eyre::Report::from)?.to_vec();
        Ok(RawResponse {
            status,
            headers,
            body,
        })
    }
}

impl Node for SealedNode {
    async fn session(&self) -> Option<Session> {
        self.client.session().await
    }
    async fn use_session(&self, session: Session) {
        self.client.use_session(session).await;
    }
    async fn use_api_key(&self, key: &str) {
        self.client.use_api_key(key).await;
    }
    /// Exchange the refresh token on this node's channel, as core's
    /// `Client::refresh` does on its own: the session goes to core's client
    /// too, for the event streams.
    async fn refresh(&self) -> Result<Session, Error> {
        let current = self
            .client
            .session()
            .await
            .ok_or_else(|| Error::Transport(eyre::eyre!("no session")))?;
        let body = serde_json::json!({ "refreshToken": current.refresh_token });
        let req = RawRequest {
            method: "POST".into(),
            path: "auth/refresh".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::to_vec(&body).map_err(eyre::Report::from)?,
        };
        let resp = self.send(&req, Some(&current.session_token)).await?;
        let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
        if resp.status >= 400 {
            return Err(api_error(resp.status, &v));
        }
        let s: Session = serde_json::from_value(v).map_err(eyre::Report::from)?;
        self.client.use_session(s.clone()).await;
        Ok(s)
    }
}

/// The error a node's answer `status` with body `v` stands for, as core's
/// client reads it (`{error, message, details}`).
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

/// A refusal of the envelope as too large (`413`) is the `TOO_LARGE` a
/// caller acts on, as core's client reports it; any other failure of the
/// channel is a transport error.
fn sealed_error(e: eyre::Report) -> Error {
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

/// What the client's recovery acts on in an answer, as an error: a `401
/// UNAUTHENTICATED` (refresh once, repeat), a `421 WRONG_NODE` (the next
/// node) and, for a request safe to send twice, a `503` (the next node).
/// `None` for an answer that goes back to the caller as it came.
pub(crate) fn recoverable(resp: &RawResponse, repeatable: bool) -> Option<Error> {
    if !matches!(resp.status, 401 | 421 | 503) {
        return None;
    }
    let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
    let e = api_error(resp.status, &v);
    let acts = match &e {
        Error::Api {
            status: 401, code, ..
        } => code == "UNAUTHENTICATED",
        Error::WrongNode { .. } => true,
        Error::Api { status: 503, .. } => repeatable,
        _ => false,
    };
    acts.then_some(e)
}

/// An error the recovery gave up on, back as the answer it stands for
/// (`{status, error, message, details}`, the node's error shape), so the
/// generated client reads it as it reads any other. A transport error has
/// no answer: it is raised.
pub(crate) fn as_response(e: Error) -> Result<RawResponse, Error> {
    let (status, code, message, details) = match e {
        Error::Api {
            status,
            code,
            message,
            details,
        } => (status, code, message, details),
        Error::WrongNode { message, details } => (421, "WRONG_NODE".to_owned(), message, details),
        transport @ Error::Transport(_) => return Err(transport),
    };
    let body = serde_json::json!({
        "status": status,
        "error": code,
        "message": message,
        "details": details,
    });
    Ok(RawResponse {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: serde_json::to_vec(&body).map_err(eyre::Report::from)?,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn answer(status: u16, body: Value) -> RawResponse {
        RawResponse {
            status,
            headers: Vec::new(),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    #[test]
    fn only_what_the_recovery_answers_is_an_error() {
        let unauth = answer(401, json!({ "error": "UNAUTHENTICATED", "message": "m" }));
        assert!(matches!(
            recoverable(&unauth, false),
            Some(Error::Api { status: 401, .. })
        ));
        let wrong = answer(421, json!({ "error": "WRONG_NODE", "message": "m" }));
        assert!(matches!(
            recoverable(&wrong, false),
            Some(Error::WrongNode { .. })
        ));
        // A redirect to the leader is the caller's to follow.
        let leader = answer(421, json!({ "error": "NOT_LEADER", "message": "m" }));
        assert!(recoverable(&leader, true).is_none());
        let busy = answer(503, json!({ "error": "UNAVAILABLE", "message": "m" }));
        assert!(recoverable(&busy, false).is_none());
        assert!(recoverable(&busy, true).is_some());
        for status in [200, 400, 403, 412, 428] {
            assert!(recoverable(&answer(status, json!({})), true).is_none());
        }
    }

    #[test]
    fn a_given_up_error_is_the_answer_it_stands_for() {
        let r = as_response(Error::Api {
            status: 413,
            code: "TOO_LARGE".into(),
            message: "too large".into(),
            details: Value::Null,
        })
        .unwrap();
        assert_eq!(r.status, 413);
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(
            v,
            json!({ "status": 413, "error": "TOO_LARGE", "message": "too large", "details": null })
        );
        let r = as_response(Error::WrongNode {
            message: "m".into(),
            details: json!({ "nodes": [] }),
        })
        .unwrap();
        assert_eq!(r.status, 421);
        assert!(as_response(Error::Transport(eyre::eyre!("down"))).is_err());
    }
}
