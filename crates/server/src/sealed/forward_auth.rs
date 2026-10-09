//! The proxy's guard, run for a request the proxy could not see.
//!
//! In proxy auth mode the node guards nothing itself: the reverse proxy in
//! front of it asks the auth service (`/auth/validate`, forward-auth) about each
//! request and forwards only what it admits, with the caller's identity in the
//! `X-Auth-*` headers. A sealed request crosses the proxy as `POST /sealed/v2`,
//! so the proxy never sees what it carries. Without this, an opened request may
//! therefore reach only the routes served without a credential
//! ([`super::InnerScope::Uncredentialed`]), which leaves every logged-in call
//! (login itself, queries, event streams) travelling in the clear over the hop.
//!
//! With `[server.sealed] forward_auth`, the node asks the same auth service the
//! same question the proxy would have, on loopback and inside the TD, after the
//! envelope is opened:
//!
//! * a request for the auth service itself (`/auth/…`, `/admin/…`, which the
//!   proxy routes to it unguarded) is forwarded to it, and its answer sealed;
//! * any other request the node would have refused as unguarded is sent to
//!   `/auth/validate` with its token, method and URI, exactly as the proxy's
//!   forward-auth sends them. A refusal is sealed back as it came; an admission
//!   puts the identity the auth service names on the request, replacing any the
//!   envelope stated, and routes it as a direct request through the proxy
//!   would be routed.
//!
//! The auth service is the one authority either way: the node adds no rule of
//! its own, so a route is reachable sealed exactly when it is reachable through
//! the proxy with the same token.

use core::time::Duration;

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::header::{AUTHORIZATION, HOST};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use reqwest::Url;

use super::{plain_error, HOP_HEADERS, MAX_SEALED_BYTES, X_FORWARDED_HOST};
use crate::proxy_identity;

/// What the auth service says about an admitted caller, and the proxy copies
/// onto the request it forwards. Never taken from an envelope.
pub(super) static AUTH_HEADERS: [&HeaderName; 4] = [
    &X_AUTH_USER,
    &X_AUTH_PERMISSIONS,
    &proxy_identity::ACCOUNT_HEADER,
    &proxy_identity::DEVICE_HEADER,
];

static X_AUTH_USER: HeaderName = HeaderName::from_static("x-auth-user");
static X_AUTH_PERMISSIONS: HeaderName = HeaderName::from_static("x-auth-permissions");
static X_FORWARDED_METHOD: HeaderName = HeaderName::from_static("x-forwarded-method");
static X_FORWARDED_URI: HeaderName = HeaderName::from_static("x-forwarded-uri");

/// How long the auth service has to answer. It is on loopback; a slow answer
/// is a stuck one.
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// The auth service this node's proxy asks, as `[server.sealed] forward_auth`
/// names it.
#[derive(Debug, Clone)]
pub struct ForwardAuth {
    base: Url,
    client: reqwest::Client,
}

impl ForwardAuth {
    /// The auth service at `base`, an `http` URL on a loopback address.
    ///
    /// Loopback only: every bearer token and every login of a sealed caller is
    /// sent there in the clear, which is sound only when it never leaves the
    /// machine — inside the TD on a TEE node.
    ///
    /// # Errors
    /// A URL that does not parse, is not `http`, names a host that is not a
    /// loopback address or `localhost`, or carries a path, query or credentials.
    pub fn new(base: &str) -> Result<Self, String> {
        let base = Url::parse(base).map_err(|err| format!("forward_auth: {err}"))?;
        if base.scheme() != "http" {
            return Err("forward_auth must be an http URL on loopback".to_owned());
        }
        let loopback = base.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !loopback {
            return Err(
                "forward_auth must name a loopback address: sealed callers' tokens are sent \
                 there in the clear"
                    .to_owned(),
            );
        }
        if base.path() != "/"
            || base.query().is_some()
            || base.fragment().is_some()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err("forward_auth is the auth service's origin only, with no path".to_owned());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(AUTH_TIMEOUT)
            .build()
            .map_err(|err| format!("forward_auth: {err}"))?;
        Ok(Self { base, client })
    }

    /// Whether the proxy routes `path` to the auth service rather than to the
    /// node: `/auth/` and `/admin/`, as `PathPrefix` matches them. `/admin-api`
    /// is the node's.
    pub(super) fn serves(path: &str) -> bool {
        path.starts_with("/auth/") || path.starts_with("/admin/")
    }

    /// Hand `request` to the auth service and return its answer.
    pub(super) async fn forward(&self, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let Ok(url) = self.base.join(
            parts
                .uri
                .path_and_query()
                .map_or(parts.uri.path(), |path| path.as_str()),
        ) else {
            return plain_error(
                StatusCode::BAD_REQUEST,
                "unusable path for the auth service",
            );
        };
        let Ok(body) = to_bytes(body, MAX_SEALED_BYTES).await else {
            return plain_error(StatusCode::BAD_REQUEST, "unreadable request body");
        };
        let mut headers = copy_headers(&parts.headers);
        // The proxy tells the auth service which host the caller reached; the
        // request's own `Host` is replaced by the loopback hop's.
        if let Some(host) = forwarded_host(&parts.headers) {
            let _previous = headers.insert(X_FORWARDED_HOST, host.clone());
        }
        let answer = self
            .client
            .request(parts.method, url)
            .headers(headers)
            .body(body)
            .send()
            .await;
        relay(answer).await
    }

    /// Ask the auth service whether `request` may pass, as the proxy's
    /// forward-auth would. On admission, `request` carries the identity it
    /// names and no other; on refusal, the auth service's answer is returned
    /// for the caller.
    pub(super) async fn authorize(&self, request: &mut Request) -> Result<(), Box<Response>> {
        let mut probe = HeaderMap::new();
        for value in request.headers().get_all(AUTHORIZATION) {
            let _previous = probe.append(AUTHORIZATION, value.clone());
        }
        let _previous = probe.insert(
            X_FORWARDED_METHOD.clone(),
            HeaderValue::from_str(request.method().as_str())
                .map_err(|_| Box::new(plain_error(StatusCode::BAD_REQUEST, "unusable method")))?,
        );
        let uri = request
            .uri()
            .path_and_query()
            .map_or(request.uri().path(), |path| path.as_str());
        let _previous = probe.insert(
            X_FORWARDED_URI.clone(),
            HeaderValue::from_str(uri)
                .map_err(|_| Box::new(plain_error(StatusCode::BAD_REQUEST, "unusable path")))?,
        );
        if let Some(host) = forwarded_host(request.headers()) {
            let _previous = probe.insert(X_FORWARDED_HOST, host.clone());
        }

        let Ok(url) = self.base.join("/auth/validate") else {
            return Err(Box::new(unreachable_auth()));
        };
        let answer = self.client.get(url).headers(probe).send().await;
        let answer = match answer {
            Ok(answer) if answer.status().is_success() => answer,
            other => return Err(Box::new(relay(other).await)),
        };

        let headers = request.headers_mut();
        for name in AUTH_HEADERS {
            let _previous = headers.remove(name);
        }
        for name in AUTH_HEADERS {
            for value in answer.headers().get_all(name.as_str()) {
                let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) else {
                    continue;
                };
                let _previous = headers.append(name.clone(), value);
            }
        }
        Ok(())
    }
}

/// The host the caller reached: the hop's `X-Forwarded-Host`, else its `Host`.
/// Both come from outside the envelope (see `open_and_dispatch`).
fn forwarded_host(headers: &HeaderMap) -> Option<&HeaderValue> {
    headers.get(X_FORWARDED_HOST).or_else(|| headers.get(HOST))
}

/// `headers` without hop-by-hop headers, the identity the auth service itself
/// answers with, or any `X-Forwarded-*`: the proxy overwrites those on a direct
/// request, so one an envelope states is the client's claim about a hop. The
/// one the auth service reads, the host, is set by the caller from outside.
fn copy_headers(headers: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut copied = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        if HOP_HEADERS.contains(name)
            || AUTH_HEADERS.contains(&name)
            || name.as_str().starts_with("x-forwarded-")
        {
            continue;
        }
        let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) else {
            continue;
        };
        let _previous = copied.append(name, value);
    }
    copied
}

/// The auth service's answer as the caller's response, or `502` when it gave
/// none.
async fn relay(answer: reqwest::Result<reqwest::Response>) -> Response {
    let Ok(answer) = answer else {
        return unreachable_auth();
    };
    let status = StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = HeaderMap::new();
    for (name, value) in answer.headers() {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) else {
            continue;
        };
        if HOP_HEADERS.contains(&name) {
            continue;
        }
        let _previous = headers.append(name, value);
    }
    let Ok(body) = answer.bytes().await else {
        return unreachable_auth();
    };
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

fn unreachable_auth() -> Response {
    plain_error(
        StatusCode::BAD_GATEWAY,
        "the auth service did not answer; retry shortly",
    )
}
