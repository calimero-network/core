//! Which hosts and browser pages may reach this node, checked before routing and
//! CORS so a WebSocket upgrade or a simple `POST` meets the same rule as a preflight.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use multiaddr::{Multiaddr, Protocol};
use tracing::{debug, warn};

use crate::config::CorsConfig;

const LOOPBACK_NAME: &str = "localhost"; // a host name only this machine answers to
static REFUSAL_REPORTED: AtomicBool = AtomicBool::new(false); // warn once; a hostile page can send many

/// Serves a request whose `Host` is this node's own, and whose page, if a browser
/// sent it, is this node's, on loopback, or in `allowed_origins`.
#[derive(Debug)]
pub(crate) struct BrowserOrigins {
    /// Lowercase host names, without a port: listen addresses and `allowed_hosts`.
    own_hosts: Vec<String>,
    /// Listening on an unspecified address makes every address the node's own.
    any_address: bool,
    listed: Vec<String>,
}

impl BrowserOrigins {
    pub(crate) fn new(listen: &[Multiaddr], cors: &CorsConfig) -> Self {
        let listed = cors.allowed_origins.clone().unwrap_or_default();
        let mut own_hosts = Vec::new();
        let mut any_address = false;
        for protocol in listen.iter().flat_map(Multiaddr::iter) {
            match protocol {
                Protocol::Ip4(ip) if ip.is_unspecified() => any_address = true,
                Protocol::Ip6(ip) if ip.is_unspecified() => any_address = true,
                Protocol::Ip4(ip) => own_hosts.push(ip.to_string()),
                Protocol::Ip6(ip) => own_hosts.push(format!("[{ip}]")),
                _ => {}
            }
        }
        own_hosts.extend(
            cors.allowed_hosts
                .iter()
                .map(|host| host.to_ascii_lowercase()),
        );
        Self {
            own_hosts,
            any_address,
            listed,
        }
    }

    /// A browser's `Host` names whatever the page's DNS resolved, so a foreign
    /// one is refused with or without an `Origin`: same-origin reads send none.
    pub(crate) fn admits(&self, headers: &HeaderMap, uri: &Uri) -> bool {
        let host = match headers.get(header::HOST) {
            Some(host) => Some(host.to_str().unwrap_or_default()),
            None => uri.authority().map(|authority| authority.as_str()),
        };
        if host.is_some_and(|host| !self.is_own_host(host)) {
            return false;
        }
        let Some(origin) = headers.get(header::ORIGIN) else {
            return true;
        };
        let Ok(origin) = origin.to_str() else {
            return false;
        };
        if self
            .listed
            .iter()
            .any(|listed| listed.eq_ignore_ascii_case(origin))
        {
            return true;
        }
        let Some((_, authority)) = origin.split_once("://") else {
            return false;
        };
        is_loopback(&host_name(authority).to_ascii_lowercase())
            || [header::HOST.as_str(), "x-forwarded-host"]
                .into_iter()
                .filter_map(|name| headers.get(name)?.to_str().ok())
                .any(|host| self.is_own_host(host) && authority.eq_ignore_ascii_case(host))
    }

    fn is_own_host(&self, host: &str) -> bool {
        let name = host_name(host).to_ascii_lowercase();
        is_loopback(&name)
            || (self.any_address && ip(&name).is_some())
            || self.own_hosts.contains(&name)
    }
}

/// Refuses, before anything else sees it, a request [`BrowserOrigins`] does not admit.
pub(crate) async fn guard(
    State(origins): State<Arc<BrowserOrigins>>,
    request: Request,
    next: Next,
) -> Response {
    if origins.admits(request.headers(), request.uri()) {
        return next.run(request).await;
    }
    let value = |name| request.headers().get(name).and_then(|v| v.to_str().ok());
    let (host, origin) = (value(header::HOST), value(header::ORIGIN));
    if !REFUSAL_REPORTED.swap(true, Ordering::Relaxed) {
        warn!(
            host,
            origin, "request refused: foreign Host or Origin, see [server.cors]"
        );
    }
    debug!(host, origin, "request refused: foreign Host or Origin");
    StatusCode::FORBIDDEN.into_response()
}

/// `*.localhost` resolves to loopback in every browser, so no DNS answer can
/// rebind it.
fn is_loopback(name: &str) -> bool {
    name == LOOPBACK_NAME
        || name.ends_with(".localhost")
        || ip(name).is_some_and(|ip| ip.is_loopback())
}

fn ip(name: &str) -> Option<IpAddr> {
    name.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

/// The name in a `host[:port]` authority, keeping an IPv6 literal's brackets.
pub(crate) fn host_name(authority: &str) -> &str {
    match authority.find(']') {
        Some(end) if authority.starts_with('[') => &authority[..=end],
        _ => authority
            .split_once(':')
            .map_or(authority, |(name, _)| name),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode, Uri};
    use axum::routing::post;
    use axum::Router;
    use multiaddr::Multiaddr;
    use tower::ServiceExt;

    use super::{guard, BrowserOrigins};
    use crate::config::CorsConfig;

    fn cors(origins: &[&str], hosts: &[&str]) -> CorsConfig {
        CorsConfig {
            allowed_origins: Some(origins.iter().map(|&o| o.to_owned()).collect()),
            allowed_hosts: hosts.iter().map(|&h| h.to_owned()).collect(),
            ..CorsConfig::new()
        }
    }

    fn headers(host: &str, origin: Option<&str>, forwarded: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let _ = headers.insert(header::HOST, host.parse().unwrap());
        if let Some(origin) = origin {
            let _ = headers.insert(header::ORIGIN, origin.parse().unwrap());
        }
        if let Some(forwarded) = forwarded {
            let _ = headers.insert("x-forwarded-host", forwarded.parse().unwrap());
        }
        headers
    }

    #[test]
    fn admits_only_this_nodes_own_names_loopback_pages_and_listed_origins() {
        let listen: Vec<Multiaddr> = vec![
            "/ip4/127.0.0.1/tcp/2528".parse().unwrap(),
            "/ip4/192.0.2.7/tcp/2528".parse().unwrap(),
        ];
        let origins =
            BrowserOrigins::new(&listen, &cors(&["https://app.example"], &["node.example"]));
        for (host, origin, forwarded, admitted, case) in [
            (
                "127.0.0.1:2528",
                None,
                None,
                true,
                "no Origin: not a browser",
            ),
            (
                "127.0.0.1:2528",
                Some("http://127.0.0.1:2528"),
                None,
                true,
                "a page served by this node",
            ),
            (
                "localhost:2528",
                Some("http://localhost:2528"),
                None,
                true,
                "a loopback name",
            ),
            (
                "[::1]:2528",
                Some("http://[::1]:2528"),
                None,
                true,
                "the IPv6 loopback",
            ),
            (
                "192.0.2.7:2528",
                Some("http://192.0.2.7:2528"),
                None,
                true,
                "a configured listen address",
            ),
            (
                "127.0.0.1:2528",
                Some("https://app.example"),
                None,
                true,
                "a listed origin",
            ),
            (
                "localhost:2528",
                Some("http://localhost:3000"),
                None,
                true,
                "a page on another loopback port",
            ),
            (
                "127.0.0.1:2528",
                Some("http://tauri.localhost"),
                None,
                true,
                "the desktop webview",
            ),
            ("node.example", None, None, true, "an allowed host"),
            (
                "127.0.0.1:8080",
                Some("http://192.0.2.7:2528"),
                Some("192.0.2.7:2528"),
                true,
                "a proxy forwarding a configured listen address",
            ),
            (
                "evil.example:2528",
                None,
                None,
                false,
                "a foreign name, even with no Origin",
            ),
            (
                "evil.example:2528",
                Some("http://evil.example:2528"),
                None,
                false,
                "a foreign name whose Origin equals its Host",
            ),
            (
                "127.0.0.1:2528",
                Some("https://evil.example"),
                None,
                false,
                "a foreign origin",
            ),
            (
                "127.0.0.1:2528",
                Some("null"),
                None,
                false,
                "an opaque origin",
            ),
            (
                "127.0.0.1:2528",
                Some("https://proxy.example"),
                None,
                false,
                "a proxy that rewrites Host without X-Forwarded-Host",
            ),
            (
                "192.168.1.5:2528",
                Some("http://192.168.1.5:2528"),
                None,
                false,
                "an address the node does not listen on",
            ),
            ("LOCALHOST:2528", None, None, true, "a Host in another case"),
            (
                "127.0.0.1:2528",
                Some("HTTPS://APP.EXAMPLE"),
                None,
                true,
                "a listed origin in another case",
            ),
            (
                "node.example",
                Some("https://node.example"),
                None,
                true,
                "a page served under an allowed host",
            ),
            (
                "app.example",
                None,
                None,
                false,
                "a listed origin's host is not an allowed host",
            ),
        ] {
            assert_eq!(
                origins.admits(&headers(host, origin, forwarded), &Uri::from_static("/")),
                admitted,
                "{case}"
            );
        }

        let mut no_host = HeaderMap::new();
        assert!(
            origins.admits(&no_host, &Uri::from_static("/")),
            "no Host and no authority: not a browser"
        );
        assert!(
            !origins.admits(&no_host, &Uri::from_static("http://evil.example/")),
            "the URI authority stands in for a missing Host"
        );
        let _ = no_host.insert(
            header::ORIGIN,
            HeaderValue::from_bytes(b"http://\xff").unwrap(),
        );
        assert!(
            !origins.admits(&no_host, &Uri::from_static("/")),
            "an Origin that is not text"
        );

        let everywhere = BrowserOrigins::new(
            &["/ip4/0.0.0.0/tcp/2528".parse().unwrap()],
            &CorsConfig::new(),
        );
        assert!(
            everywhere.admits(
                &headers("192.168.1.5:2528", Some("http://192.168.1.5:2528"), None),
                &Uri::from_static("/")
            ),
            "listening on every address makes each address the node's own"
        );
        assert!(
            !everywhere.admits(
                &headers("evil.example:2528", None, None),
                &Uri::from_static("/")
            ),
            "but no name"
        );
    }

    /// Through the middleware, since a simple `POST` reaches a handler with no
    /// preflight for CORS to refuse.
    #[tokio::test]
    async fn guard_refuses_a_foreign_request_before_the_handler_runs() {
        let origins = Arc::new(BrowserOrigins::new(
            &["/ip4/127.0.0.1/tcp/2528".parse().unwrap()],
            &CorsConfig::new(),
        ));
        let app = Router::new()
            .route("/admin-api/contexts", post(|| async { "created" }))
            .layer(axum::middleware::from_fn_with_state(origins, guard));
        let send = |host: &'static str, origin: Option<&'static str>| {
            let app = app.clone();
            async move {
                let mut request = Request::post("/admin-api/contexts").header(header::HOST, host);
                if let Some(origin) = origin {
                    request = request.header(header::ORIGIN, origin);
                }
                app.oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status()
            }
        };

        assert_eq!(
            send("127.0.0.1:2528", None).await,
            StatusCode::OK,
            "control: a CLI"
        );
        assert_eq!(
            send("127.0.0.1:2528", Some("http://localhost:5173")).await,
            StatusCode::OK,
            "control: a local page"
        );
        assert_eq!(
            send("127.0.0.1:2528", Some("https://evil.example")).await,
            StatusCode::FORBIDDEN,
            "a simple POST from a foreign page"
        );
        assert_eq!(
            send("evil.example:2528", None).await,
            StatusCode::FORBIDDEN,
            "a rebound name"
        );
    }
}
