use core::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use tower as _;

use axum::http::Method;
use axum::{Extension, Router};
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use config::ServerConfig;
use eyre::{bail, Result as EyreResult};
use multiaddr::Protocol;
use prometheus_client::registry::Registry;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tower_http::cors::{Any, CorsLayer};
use tracing::warn;

use crate::service_mounts::mount_runtime_services;

pub mod admin;
mod auth;
pub mod config;
mod execute;
pub mod jsonrpc;
mod metrics;
mod service_mounts;
pub mod sse;
pub mod ws;

#[derive(Debug)]
#[non_exhaustive]
pub struct AdminState {
    pub store: Store,
    pub ctx_client: ContextClient,
    pub node_client: NodeClient,
}

impl AdminState {
    #[must_use]
    pub const fn new(store: Store, ctx_client: ContextClient, node_client: NodeClient) -> Self {
        Self {
            store,
            ctx_client,
            node_client,
        }
    }
}

#[expect(clippy::print_stderr, reason = "Acceptable for CLI")]
pub async fn start(
    config: ServerConfig,
    ctx_client: ContextClient,
    node_client: NodeClient,
    datastore: Store,
    mut prom_registry: Registry,
) -> EyreResult<()> {
    let mut config = config;

    // Register HTTP request metrics on the same registry before the
    // metrics service consumes ownership of it via `mount_runtime_services`
    // → `metrics::service`. The middleware below will resolve the handle
    // out of the request `Extension`s.
    let http_metrics = crate::metrics::HttpMetrics::new(&mut prom_registry);
    let mut addrs = Vec::with_capacity(config.listen.len());
    let mut listeners = Vec::with_capacity(config.listen.len());
    let mut want_listeners = config.listen.into_iter().peekable();

    while let Some(addr) = want_listeners.next() {
        let mut components = addr.iter();

        let host: IpAddr = match components.next() {
            Some(Protocol::Ip4(host)) => host.into(),
            Some(Protocol::Ip6(host)) => host.into(),
            _ => bail!("Invalid multiaddr, expected IP4 component"),
        };

        let Some(Protocol::Tcp(port)) = components.next() else {
            bail!("Invalid multiaddr, expected TCP component");
        };

        match TcpListener::bind(SocketAddr::from((host, port))).await {
            Ok(listener) => {
                let local_port = listener.local_addr()?.port();
                addrs.push(
                    addr.replace(1, |_| Some(Protocol::Tcp(local_port)))
                        .unwrap(), // safety: we know the index is valid
                );
                listeners.push(listener);
            }
            Err(err) => {
                if want_listeners.peek().is_none() {
                    bail!(err);
                }
            }
        }
    }
    config.listen = addrs;

    let mut app = Router::new();

    let mut embedded_auth = if config.use_embedded_auth() {
        Some(auth::initialise(&config).await?)
    } else {
        None
    };

    let auth_service = embedded_auth
        .as_ref()
        .map(|auth| Arc::new(auth.auth_service()));

    let shared_state = Arc::new(AdminState::new(
        datastore.clone(),
        ctx_client.clone(),
        node_client.clone(),
    ));
    let mounted = mount_runtime_services(
        app,
        &config,
        auth_service.clone(),
        ctx_client,
        node_client.clone(),
        datastore.clone(),
        shared_state,
        prom_registry,
    );
    app = mounted.router;
    let mut service_count = mounted.added_count;

    if let Some(bundled_auth) = embedded_auth.take() {
        app = app.merge(bundled_auth.into_router());
        service_count += 1;
    }

    if service_count == 0 {
        warn!("No services enabled, enable at least one service to start the server");

        return Ok(());
    }

    // HTTP request observability middleware. Wraps every mounted route
    // (jsonrpc, ws, sse, admin, auth) — applied *before* CORS so the
    // recorded latency excludes pre-flight handling but still observes
    // failed CORS rejections.
    app = app
        .layer(axum::middleware::from_fn(crate::metrics::track_request))
        .layer(Extension(http_metrics));

    app = app.layer(build_cors_layer());

    let mut set = JoinSet::new();

    for listener in listeners {
        let app = app.clone();
        drop(set.spawn(async move { axum::serve(listener, app).await }));
    }

    while let Some(result) = set.join_next().await {
        result??;
    }

    Ok(())
}

/// CORS layer applied to every mounted route.
///
/// **Critical:** `expose_headers` MUST include `x-auth-error`. Cross-origin
/// clients (Tauri webview, browser SPAs) cannot read response headers that
/// aren't on this list. The auth middleware signals refreshable expiry via
/// `X-Auth-Error: token_expired`; mero-js's automatic refresh-on-401 flow
/// reads that header to decide whether to refresh. If the header is hidden
/// by CORS, every access-token expiry surfaces as a hard logout for the user
/// instead of a transparent refresh. See `cors_tests` for the regression
/// guard.
///
/// **`allow_credentials` is intentionally not set.** It is incompatible with
/// `allow_origin(Any)` per the CORS spec, so adding it here would be a no-op
/// for browsers in the current configuration. If credentialed requests
/// (cookies, TLS client certs) ever become required, `allow_origin(Any)`
/// must first be replaced with an explicit allow-list of trusted origins.
fn build_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods([
            Method::POST,
            Method::GET,
            Method::DELETE,
            Method::PUT,
            // PATCH backs `updateGroupSettings` (PATCH /admin-api/groups/:id);
            // omitting it fails the browser preflight as a status-0 network
            // error while curl/CLI clients work fine.
            Method::PATCH,
            Method::OPTIONS,
        ])
        .expose_headers([
            axum::http::HeaderName::from_static("x-auth-error"),
            axum::http::HeaderName::from_static("x-auth-user"),
            axum::http::HeaderName::from_static("x-auth-permissions"),
        ])
        .allow_private_network(true)
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {color_eyre as _, tracing_subscriber as _};
}

#[cfg(test)]
mod cors_tests {
    //! Regression tests for the CORS layer.
    //!
    //! These exist because of a real prod incident: without `expose_headers`
    //! listing `x-auth-error`, the Tauri webview (cross-origin to the local
    //! merod) could not read the `X-Auth-Error: token_expired` response
    //! header, so mero-js never triggered its refresh-on-401 flow and users
    //! were logged out roughly once per access-token TTL (~1h).
    //!
    //! Do not delete `expose_headers` without also breaking these tests.

    use axum::body::Body;
    use axum::http::{header, HeaderValue, Request, StatusCode};
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use super::build_cors_layer;

    /// Origin header the Tauri desktop webview presents in production.
    const TAURI_ORIGIN: &str = "http://tauri.localhost";

    async fn ok_handler() -> Response {
        Response::new(Body::from("ok"))
    }

    async fn token_expired_401_handler() -> Response {
        let mut resp = Response::new(Body::from("unauthorized"));
        *resp.status_mut() = StatusCode::UNAUTHORIZED;
        resp.headers_mut().insert(
            axum::http::HeaderName::from_static("x-auth-error"),
            HeaderValue::from_static("token_expired"),
        );
        resp
    }

    fn cors_only_router<F, Fut>(handler: F) -> Router
    where
        F: Fn() -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = Response> + Send + 'static,
    {
        Router::new()
            .route("/x", get(handler))
            .layer(build_cors_layer())
    }

    /// Browser preflight for the PATCH-backed admin routes (e.g.
    /// `PATCH /admin-api/groups/:id` = updateGroupSettings): the allow-methods
    /// list must include PATCH, otherwise web apps see a status-0 network
    /// error while curl/CLI clients (no CORS) work — which is how the gap
    /// originally shipped unnoticed.
    #[tokio::test]
    async fn cors_preflight_allows_patch() {
        let app = cors_only_router(ok_handler);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/x")
                    .header(header::ORIGIN, TAURI_ORIGIN)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PATCH")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("router service call should not fail");

        assert_eq!(resp.status(), StatusCode::OK);
        let allowed = resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .expect("preflight must return Access-Control-Allow-Methods")
            .to_str()
            .expect("allow-methods should be ASCII");
        assert!(
            allowed.contains("PATCH"),
            "PATCH missing from Access-Control-Allow-Methods ({allowed}) — \
             browser clients cannot call updateGroupSettings"
        );
    }

    /// Direct guard against the original CORS misconfiguration: a
    /// cross-origin request must come back with `Access-Control-Expose-
    /// Headers` listing `x-auth-error`, otherwise no JS-based client can see
    /// the header even when the server sets it.
    #[tokio::test]
    async fn cors_layer_exposes_x_auth_error_to_cross_origin_clients() {
        let app = cors_only_router(ok_handler);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/x")
                    .header(header::ORIGIN, TAURI_ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("router service call should not fail");

        assert_eq!(resp.status(), StatusCode::OK);

        let exposed = resp
            .headers()
            .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
            .unwrap_or_else(|| {
                panic!(
                    "missing Access-Control-Expose-Headers — JS cross-origin clients \
                     will not see X-Auth-Error: token_expired, breaking automatic \
                     token refresh in the Tauri desktop app"
                )
            })
            .to_str()
            .expect("header value must be ASCII")
            .to_ascii_lowercase();

        assert!(
            exposed.contains("x-auth-error"),
            "Access-Control-Expose-Headers must include `x-auth-error`; got: {exposed}"
        );
    }

    /// Full pipeline check: when the upstream handler returns a 401 with
    /// `X-Auth-Error: token_expired` (mimicking what the auth middleware
    /// emits when a JWT has expired), the CORS layer must not strip the
    /// header from the response AND must expose it to JS via
    /// `Access-Control-Expose-Headers`. This is the assertion mero-js's
    /// `web-client.ts` automatic-refresh logic depends on.
    #[tokio::test]
    async fn cors_preserves_and_exposes_token_expired_signal_on_401() {
        let app = cors_only_router(token_expired_401_handler);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/x")
                    .header(header::ORIGIN, TAURI_ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("router service call should not fail");

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // The header itself must survive the CORS layer.
        assert_eq!(
            resp.headers()
                .get("x-auth-error")
                .map(|v| v.to_str().unwrap()),
            Some("token_expired"),
            "CORS layer must not strip X-Auth-Error from upstream responses"
        );

        // And it must be in the expose list so cross-origin JS can read it.
        let exposed = resp
            .headers()
            .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
            .expect("Access-Control-Expose-Headers must be set on 401 too")
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        assert!(
            exposed.contains("x-auth-error"),
            "X-Auth-Error must be exposed to JS even on error responses; got: {exposed}"
        );
    }
}
