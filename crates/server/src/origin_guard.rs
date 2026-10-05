use core::net::IpAddr;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::uri::Authority;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::{debug, warn};

static FIRST_REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);

/// The fetch-metadata header that marks a request as a browser's. A same-origin
/// `GET` carries no `Origin`, so this is what still says a page sent it.
///
/// `Sec-Fetch-Site` alone, not `Sec-Fetch-Mode` or `-Dest`: Node's built-in
/// `fetch` (undici) sends `Sec-Fetch-Mode: cors` on every request, so counting
/// it would judge every server-side SDK client as a browser. A browser sends
/// `Sec-Fetch-Site` on every request, and a page can neither set nor drop it.
const FETCH_SITE: &str = "sec-fetch-site";

/// The headers that name the host a request was sent to.
const HOST_HEADERS: [&str; 2] = ["host", "x-forwarded-host"];

#[derive(Clone, Debug)]
pub(crate) struct OriginGuard {
    enforce: bool,
    allowed: Arc<[String]>,
    /// Lowercase hosts, without a port, of the `allowed` origins.
    allowed_hosts: Arc<[String]>,
}

impl OriginGuard {
    pub(crate) fn new(callers_authenticated: bool, allowed_origins: Option<&[String]>) -> Self {
        let allowed: Arc<[String]> = allowed_origins.unwrap_or_default().into();
        let allowed_hosts = allowed
            .iter()
            .filter_map(|origin| origin.split_once("://")?.1.parse::<Authority>().ok())
            .map(|authority| authority.host().to_ascii_lowercase())
            .collect();
        Self {
            enforce: !callers_authenticated,
            allowed,
            allowed_hosts,
        }
    }

    pub(crate) const fn enforced(&self) -> bool {
        self.enforce
    }

    pub(crate) fn is_listed(&self, origin: &HeaderValue) -> bool {
        origin.to_str().is_ok_and(|origin| {
            self.allowed
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(origin))
        })
    }

    /// Whether a request may reach a node that does not authenticate callers.
    ///
    /// A request with no `Origin` and no `Sec-Fetch-Site` is not a browser's,
    /// and is let through as before. A browser's is admitted when its origin is
    /// listed or is a loopback page, or when every host it names is one of this
    /// node's own and the origin is one of those hosts (or absent).
    ///
    /// The host is what makes this hold under DNS rebinding. A page on
    /// `attacker.example` whose name is re-pointed at this node sends
    /// `Origin: http://attacker.example` and `Host: attacker.example`: the two
    /// agree, so comparing them alone admits it, and a same-origin `GET` from it
    /// sends no `Origin` at all. A browser's `Host` is whatever name it
    /// resolved, so it counts only when the node recognises it: a loopback
    /// name, an IP address (a rebound page's origin is its own name, never an
    /// address) or the host of an `allowed_origins` entry. Every host header
    /// must pass, because a same-origin page may set `X-Forwarded-Host` itself.
    pub(crate) fn admits(&self, headers: &HeaderMap, uri_authority: Option<&Authority>) -> bool {
        let origin = headers.get(header::ORIGIN);
        if origin.is_none() && !headers.contains_key(FETCH_SITE) {
            return true;
        }
        if origin.is_some_and(|origin| self.is_listed(origin) || is_loopback_page(origin)) {
            return true;
        }

        let mut hosts = Vec::new();
        for name in HOST_HEADERS {
            for value in headers.get_all(name) {
                let Some(host) = value
                    .to_str()
                    .ok()
                    .and_then(|value| value.split(',').next())
                    .and_then(|value| value.trim().parse::<Authority>().ok())
                else {
                    return false;
                };
                hosts.push(host);
            }
        }
        if hosts.is_empty() {
            // HTTP/2 carries the host in `:authority`, which lands in the URI.
            hosts.extend(uri_authority.cloned());
        }
        if hosts.is_empty() || !hosts.iter().all(|host| self.is_own_host(host.host())) {
            return false;
        }

        let Some(origin) = origin else {
            return true;
        };
        let Some((scheme, authority)) = origin
            .to_str()
            .ok()
            .and_then(|origin| origin.split_once("://"))
            .and_then(|(scheme, rest)| {
                Some((scheme.to_ascii_lowercase(), rest.parse::<Authority>().ok()?))
            })
        else {
            return false;
        };

        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return false,
        };
        let port = authority.port_u16().unwrap_or(default_port);
        hosts.iter().any(|host| {
            host.host().eq_ignore_ascii_case(authority.host())
                && host.port_u16().unwrap_or(default_port) == port
        })
    }

    fn is_own_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        is_loopback_host(&host)
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .is_ok()
            || self.allowed_hosts.contains(&host)
    }
}

/// A page served from this machine: a local dev server or a Tauri webview.
/// Admitted whatever host it calls, as before. A rebound page's origin is the
/// attacker's own name, never a loopback one.
fn is_loopback_page(origin: &HeaderValue) -> bool {
    origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .and_then(|(scheme, rest)| Some((scheme, rest.parse::<Authority>().ok()?)))
        .is_some_and(|(scheme, authority)| {
            is_loopback_host(authority.host())
                && ["http", "https", "tauri"]
                    .iter()
                    .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
        })
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

pub(crate) async fn refuse_foreign_origins(
    State(guard): State<OriginGuard>,
    request: Request,
    next: Next,
) -> Response {
    if guard.enforce && !guard.admits(request.headers(), request.uri().authority()) {
        let origin = request.headers().get(header::ORIGIN);
        let host = request.headers().get(header::HOST);
        if FIRST_REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
            debug!(?origin, ?host, path = %request.uri().path(), "refused a browser request");
        } else {
            warn!(
                ?origin,
                ?host,
                path = %request.uri().path(),
                "refused a browser request: this node does not authenticate callers itself \
                 (auth mode proxy), so it serves browser pages only from its own origin, \
                 named by a loopback name, an IP address or a host in [server.cors] \
                 allowed_origins; list the origin it is served under there"
            );
        }
        return (
            StatusCode::FORBIDDEN,
            "browser request refused: this node does not authenticate callers itself; \
             list the origin in [server.cors] allowed_origins",
        )
            .into_response();
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use libp2p::identity::Keypair;
    use tower::ServiceExt;

    use super::{refuse_foreign_origins, OriginGuard};
    use crate::config::{AuthMode, ServerConfig, ServiceConfigs};

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            let _previous = map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    fn admits(guard: &OriginGuard, origin: &'static str, host: &'static str) -> bool {
        guard.admits(&headers(&[("origin", origin), ("host", host)]), None)
    }

    fn guard_for(auth_mode: AuthMode, proxy_identity: bool) -> OriginGuard {
        let mut config = ServerConfig::with_auth(
            vec![],
            Keypair::generate_ed25519(),
            ServiceConfigs {
                admin: None,
                jsonrpc: None,
                websocket: None,
                sse: None,
            },
            auth_mode,
            None,
        );
        config.proxy_identity = proxy_identity;
        OriginGuard::new(
            config.authenticates_callers(),
            config.cors.allowed_origins.as_deref(),
        )
    }

    /// The guard stands in for an authenticating layer only where there is
    /// none. A node that takes callers' identity from its proxy is reachable
    /// only through that proxy, which authenticates every caller, and a relay
    /// serves apps from any origin: there the guard steps aside, as it does
    /// under embedded auth. Plain proxy mode keeps it.
    #[test]
    fn a_node_whose_proxy_names_callers_is_not_guarded() {
        assert!(!guard_for(AuthMode::Embedded, false).enforced());
        assert!(!guard_for(AuthMode::Proxy, true).enforced());
        assert!(guard_for(AuthMode::Proxy, false).enforced());
    }

    #[test]
    fn unauthenticated_node_admits_only_its_own_origin_loopback_and_listed_origins() {
        let guard = OriginGuard::new(false, None);

        assert!(admits(&guard, "http://127.0.0.1:2528", "127.0.0.1:2528"));
        assert!(admits(&guard, "http://localhost:5173", "127.0.0.1:2528"));
        assert!(admits(&guard, "http://[::1]:3000", "[::1]:2528"));
        assert!(admits(
            &guard,
            "http://app.localhost:8080",
            "127.0.0.1:2528"
        ));
        assert!(admits(&guard, "http://tauri.localhost", "127.0.0.1:2528"));
        assert!(admits(&guard, "tauri://localhost", "127.0.0.1:2528"));
        assert!(admits(
            &guard,
            "http://192.168.1.5:2528",
            "192.168.1.5:2528"
        ));

        assert!(!admits(&guard, "https://site.example", "127.0.0.1:2528"));
        assert!(!admits(&guard, "https://site.example", "localhost:2528"));
        assert!(!admits(
            &guard,
            "http://node.example.com:8080",
            "node.example.com"
        ));
        assert!(!admits(
            &guard,
            "https://node.example.com.site.example",
            "node.example.com"
        ));
        assert!(!admits(&guard, "null", "127.0.0.1:2528"));
        assert!(!admits(&guard, "file://", "127.0.0.1:2528"));
        assert!(!admits(
            &guard,
            "ftp://node.example.com",
            "node.example.com"
        ));

        let listed = ["https://app.example".to_owned()];
        let guard = OriginGuard::new(false, Some(&listed));
        assert!(admits(&guard, "https://app.example", "127.0.0.1:2528"));
        assert!(!admits(&guard, "https://other.example", "127.0.0.1:2528"));

        // A node served under a name of its own lists that name; then its own
        // pages, which send it as both Origin and Host, are admitted.
        let listed = ["https://node.example.com".to_owned()];
        let guard = OriginGuard::new(false, Some(&listed));
        assert!(admits(
            &guard,
            "https://node.example.com",
            "node.example.com"
        ));
        assert!(admits(
            &guard,
            "https://node.example.com",
            "node.example.com:443"
        ));
    }

    /// DNS rebinding: a page whose name now resolves to this node sends its own
    /// name as Origin AND as Host. The two agree, so only the host can refuse it.
    #[test]
    fn a_rebound_page_is_refused_though_its_origin_equals_its_host() {
        let guard = OriginGuard::new(false, None);

        assert!(!admits(
            &guard,
            "http://attacker.example:2528",
            "attacker.example:2528"
        ));
        assert!(!admits(
            &guard,
            "https://node.example.com",
            "node.example.com"
        ));
    }

    /// A same-origin `GET` from a rebound page carries no Origin. Its fetch
    /// metadata still says a browser sent it, so its host is checked all the same.
    #[test]
    fn a_rebound_page_reading_without_an_origin_is_refused() {
        let guard = OriginGuard::new(false, None);

        assert!(!guard.admits(
            &headers(&[
                ("host", "attacker.example:2528"),
                ("sec-fetch-site", "same-origin"),
                ("sec-fetch-mode", "cors"),
            ]),
            None,
        ));
        assert!(guard.admits(
            &headers(&[
                ("host", "127.0.0.1:2528"),
                ("sec-fetch-site", "same-origin"),
            ]),
            None,
        ));
    }

    /// meroctl and curl send neither Origin nor fetch metadata, whatever name
    /// they were pointed at. They are not browsers, so a rebinding page is not
    /// among them and the proxy in front decides who they are.
    #[test]
    fn a_client_that_is_not_a_browser_is_not_judged_by_its_host() {
        let guard = OriginGuard::new(false, None);

        assert!(guard.admits(&headers(&[("host", "my-node.example:2528")]), None));
        assert!(guard.admits(&HeaderMap::new(), None));
    }

    /// Node's built-in `fetch` sends `Sec-Fetch-Mode: cors` and nothing else of
    /// the fetch metadata, so mero-js on a server is not a browser, whatever
    /// name it calls the node by.
    #[test]
    fn node_fetch_is_not_judged_as_a_browser() {
        let guard = OriginGuard::new(false, None);

        assert!(guard.admits(
            &headers(&[
                ("host", "relay.example.com"),
                ("sec-fetch-mode", "cors"),
                ("user-agent", "node"),
            ]),
            None,
        ));
    }

    /// A page on this machine (a dev server, a Tauri webview) may call a node
    /// it names by any host, as before: a rebinding page never has a loopback
    /// origin.
    #[test]
    fn a_loopback_page_reaches_a_node_named_by_any_host() {
        let guard = OriginGuard::new(false, None);

        for origin in [
            "http://localhost:5173",
            "tauri://localhost",
            "http://tauri.localhost",
        ] {
            assert!(admits(&guard, origin, "relay.example.com"), "{origin}");
        }
        assert!(!admits(&guard, "ftp://localhost", "relay.example.com"));
    }

    /// A same-origin page may set `X-Forwarded-Host` itself, so a host header
    /// naming this node does not vouch for another that does not.
    #[test]
    fn a_page_cannot_name_the_node_in_a_forwarded_host_it_sets() {
        let guard = OriginGuard::new(false, None);

        assert!(!guard.admits(
            &headers(&[
                ("origin", "http://attacker.example:2528"),
                ("host", "attacker.example:2528"),
                ("x-forwarded-host", "127.0.0.1:2528"),
            ]),
            None,
        ));
    }

    /// HTTP/2 names the host in `:authority`, which reaches the URI, not a header.
    #[test]
    fn the_uri_authority_names_the_host_when_no_header_does() {
        let guard = OriginGuard::new(false, None);
        let own = "127.0.0.1:2528".parse().unwrap();
        let rebound = "attacker.example:2528".parse().unwrap();

        assert!(guard.admits(&headers(&[("origin", "http://127.0.0.1:2528")]), Some(&own)));
        assert!(!guard.admits(
            &headers(&[("origin", "http://attacker.example:2528")]),
            Some(&rebound)
        ));
        assert!(!guard.admits(&headers(&[("origin", "http://192.168.1.5:2528")]), None));
    }

    #[test]
    fn a_proxy_forwarded_host_counts_as_the_nodes_own_origin_once_listed() {
        let forwarded = |origin| {
            headers(&[
                ("origin", origin),
                ("host", "127.0.0.1:2528"),
                ("x-forwarded-host", "node.example.com"),
            ])
        };

        let unlisted = OriginGuard::new(false, None);
        assert!(!unlisted.admits(&forwarded("https://node.example.com"), None));

        let listed = ["https://node.example.com".to_owned()];
        let guard = OriginGuard::new(false, Some(&listed));
        assert!(guard.admits(&forwarded("https://node.example.com"), None));
        assert!(!guard.admits(&forwarded("https://site.example"), None));
    }

    fn app(guard: OriginGuard) -> Router {
        Router::new()
            .route(
                "/admin-api/install-application",
                post(|| async { "installed" }),
            )
            .route("/jsonrpc", post(|| async { "executed" }))
            .route("/admin-api/health", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                guard,
                refuse_foreign_origins,
            ))
    }

    async fn status(
        app: Router,
        method: &str,
        path: &str,
        origin: Option<&'static str>,
    ) -> StatusCode {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "127.0.0.1:2528");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn unauthenticated_node_refuses_foreign_pages_before_the_handler_runs() {
        let guarded = app(OriginGuard::new(false, None));

        for path in ["/admin-api/install-application", "/jsonrpc"] {
            assert_eq!(
                status(guarded.clone(), "POST", path, Some("https://site.example")).await,
                StatusCode::FORBIDDEN,
                "{path}"
            );
            assert_eq!(
                status(guarded.clone(), "POST", path, Some("null")).await,
                StatusCode::FORBIDDEN,
                "{path}"
            );
            assert_eq!(
                status(guarded.clone(), "POST", path, None).await,
                StatusCode::OK,
                "{path}"
            );
            assert_eq!(
                status(guarded.clone(), "POST", path, Some("http://127.0.0.1:2528")).await,
                StatusCode::OK,
                "{path}"
            );
        }
    }

    /// The rebinding read: a same-origin `GET` with no Origin, refused before
    /// the handler runs, while the same request from a client that is not a
    /// browser still reaches it.
    #[tokio::test]
    async fn a_rebound_page_cannot_read_through_the_guard() {
        let guarded = app(OriginGuard::new(false, None));
        let get = |sec_fetch: bool| {
            let mut request = Request::builder()
                .method("GET")
                .uri("/admin-api/health")
                .header(header::HOST, "attacker.example:2528");
            if sec_fetch {
                request = request.header("sec-fetch-site", "same-origin");
            }
            request.body(Body::empty()).unwrap()
        };

        let refused = guarded.clone().oneshot(get(true)).await.unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let served = guarded.oneshot(get(false)).await.unwrap();
        assert_eq!(served.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_node_that_authenticates_callers_is_left_to_cors() {
        let open = app(OriginGuard::new(true, None));

        assert_eq!(
            status(
                open,
                "POST",
                "/admin-api/install-application",
                Some("https://site.example")
            )
            .await,
            StatusCode::OK
        );
    }
}
