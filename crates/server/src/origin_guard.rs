use core::net::IpAddr;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::uri::Authority;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::{debug, warn};

static FIRST_REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub(crate) struct OriginGuard {
    enforce: bool,
    allowed: Arc<[String]>,
}

impl OriginGuard {
    pub(crate) fn new(auth_enforced: bool, allowed_origins: Option<&[String]>) -> Self {
        Self {
            enforce: !auth_enforced,
            allowed: allowed_origins.unwrap_or_default().into(),
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

    pub(crate) fn admits(&self, origin: &HeaderValue, headers: &HeaderMap) -> bool {
        if self.is_listed(origin) {
            return true;
        }
        let Ok(origin) = origin.to_str() else {
            return false;
        };

        let Some((scheme, rest)) = origin.split_once("://") else {
            return false;
        };
        let Ok(authority) = rest.parse::<Authority>() else {
            return false;
        };
        let scheme = scheme.to_ascii_lowercase();

        if is_loopback_host(authority.host()) {
            return matches!(scheme.as_str(), "http" | "https" | "tauri");
        }

        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return false,
        };
        let port = authority.port_u16().unwrap_or(default_port);

        [header::HOST, HeaderName::from_static("x-forwarded-host")]
            .iter()
            .filter_map(|name| headers.get(name)?.to_str().ok())
            .filter_map(|value| value.split(',').next()?.trim().parse::<Authority>().ok())
            .any(|host| {
                host.host().eq_ignore_ascii_case(authority.host())
                    && host.port_u16().unwrap_or(default_port) == port
            })
    }
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
    if guard.enforce {
        if let Some(origin) = request.headers().get(header::ORIGIN) {
            if !guard.admits(origin, request.headers()) {
                if FIRST_REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
                    debug!(?origin, path = %request.uri().path(), "refused a cross-origin request");
                } else {
                    warn!(
                        ?origin,
                        path = %request.uri().path(),
                        "refused a cross-origin request: this node does not authenticate \
                         callers itself (auth mode proxy), so browser pages from other origins \
                         are refused; list trusted origins in [server.cors] allowed_origins"
                    );
                }
                return (
                    StatusCode::FORBIDDEN,
                    "cross-origin request refused: this node does not authenticate callers \
                     itself; list the origin in [server.cors] allowed_origins",
                )
                    .into_response();
            }
        }
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use tower::ServiceExt;

    use super::{refuse_foreign_origins, OriginGuard};

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            let _previous = map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    fn admits(guard: &OriginGuard, origin: &'static str, host: &'static str) -> bool {
        guard.admits(
            &HeaderValue::from_static(origin),
            &headers(&[("host", host)]),
        )
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
            "https://node.example.com",
            "node.example.com"
        ));
        assert!(admits(
            &guard,
            "https://node.example.com",
            "node.example.com:443"
        ));
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
    }

    #[test]
    fn a_proxy_forwarded_host_counts_as_the_nodes_own_origin() {
        let guard = OriginGuard::new(false, None);

        assert!(guard.admits(
            &HeaderValue::from_static("https://node.example.com"),
            &headers(&[
                ("host", "127.0.0.1:2528"),
                ("x-forwarded-host", "node.example.com"),
            ]),
        ));
        assert!(!guard.admits(
            &HeaderValue::from_static("https://site.example"),
            &headers(&[
                ("host", "127.0.0.1:2528"),
                ("x-forwarded-host", "node.example.com"),
            ]),
        ));
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
