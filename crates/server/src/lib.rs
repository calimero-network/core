use core::net::{IpAddr, SocketAddr};
use core::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;

use axum::extract::Request;
use axum::http::Method;
use axum::{Extension, Router, ServiceExt};
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use config::ServerConfig;
use eyre::{bail, Result as EyreResult};
use multiaddr::Protocol;
use prometheus_client::registry::Registry;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing::{info, warn};

use crate::browser_origins::BrowserOrigins;
use crate::service_mounts::mount_runtime_services;

pub mod admin;
mod auth;
mod browser_origins;
mod caller_account;
pub mod config;
mod ephemeral_replay;
mod execute;
pub mod jsonrpc;
mod metrics;
mod proof_auth;
mod proxy_identity;
pub mod sealed;
mod service_mounts;
pub mod sse;
mod subscription_grants;
#[cfg(test)]
mod test_support;
pub mod ws;

/// Node lifecycle state consulted by the readiness (`/ready`) probe.
///
/// A k8s readiness probe asks a single question — "should traffic be routed
/// here right now?" — so a node exposes one coarse lifecycle signal rather
/// than the per-namespace governance readiness FSM (which has no single
/// node-wide tier). `run::start` flips it to [`READY`](Self::READY) once every
/// subsystem is up, and to [`SHUTTING_DOWN`](Self::SHUTTING_DOWN) the moment a
/// termination signal arrives so the orchestrator stops routing new traffic
/// while in-flight requests drain.
#[derive(Debug)]
pub struct NodeReadiness(AtomicU8);

impl NodeReadiness {
    /// Subsystems still coming up — not yet safe to route traffic.
    pub const STARTING: u8 = 0;
    /// Fully started and serving.
    pub const READY: u8 = 1;
    /// Termination in progress — draining in-flight work, refuse new traffic.
    pub const SHUTTING_DOWN: u8 = 2;

    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicU8::new(Self::STARTING))
    }

    // A single independent flag with no cross-atomic ordering requirements, so
    // `Release`/`Acquire` are sufficient — `SeqCst`'s total order buys nothing
    // here.
    pub fn set_ready(&self) {
        self.0.store(Self::READY, Ordering::Release);
    }

    pub fn set_shutting_down(&self) {
        self.0.store(Self::SHUTTING_DOWN, Ordering::Release);
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::READY
    }

    /// Lower-case label for logs and probe responses.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self.0.load(Ordering::Acquire) {
            Self::READY => "ready",
            Self::SHUTTING_DOWN => "shutting_down",
            Self::STARTING => "starting",
            // Only the three constants above are ever stored; a fresh value
            // means a new state was added without updating this match. Treat it
            // as not-ready ("starting") in release, but trip in debug so the
            // omission is caught in tests rather than silently masked.
            other => {
                debug_assert!(false, "NodeReadiness holds unknown state {other}");
                "starting"
            }
        }
    }
}

impl Default for NodeReadiness {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct AdminState {
    pub store: Store,
    pub ctx_client: ContextClient,
    pub node_client: NodeClient,
    /// Node lifecycle signal driven by `run::start`; read by the readiness
    /// probe.
    pub readiness: Arc<NodeReadiness>,
    /// Public half of this process's sealed-transport key, which
    /// `/tee/attest` binds into the quote on request. See [`sealed`].
    pub transport_public_key: [u8; 32],
    /// DEV/TEST ONLY. When true, the TEE admin handlers produce and accept mock
    /// attestation quotes instead of requiring real TDX hardware. Insecure —
    /// never enable in production. Sourced from `merod run --mock-tee`. Only
    /// present under the default-off `mock-attestation` feature.
    #[cfg(feature = "mock-attestation")]
    pub mock_tee: bool,
    /// The mero-tee node release this node runs, from `MERO_TEE_VERSION`.
    /// Fleet-join names it to admitters, which check the quote against that
    /// release's signed measurements under a signed-release policy.
    pub tee_release_version: Option<String>,
}

impl AdminState {
    #[must_use]
    pub const fn new(
        store: Store,
        ctx_client: ContextClient,
        node_client: NodeClient,
        readiness: Arc<NodeReadiness>,
        transport_public_key: [u8; 32],
        #[cfg(feature = "mock-attestation")] mock_tee: bool,
    ) -> Self {
        Self {
            store,
            ctx_client,
            node_client,
            readiness,
            transport_public_key,
            #[cfg(feature = "mock-attestation")]
            mock_tee,
            tee_release_version: None,
        }
    }

    #[must_use]
    pub fn with_tee_release_version(mut self, tee_release_version: Option<String>) -> Self {
        self.tee_release_version = tee_release_version;
        self
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "top-level server entry point wiring node-wide handles"
)]
pub async fn start(
    config: ServerConfig,
    ctx_client: ContextClient,
    node_client: NodeClient,
    datastore: Store,
    mut prom_registry: Registry,
    readiness: Arc<NodeReadiness>,
    shutdown: CancellationToken,
    #[cfg(feature = "mock-attestation")] mock_tee: bool,
) -> EyreResult<()> {
    let mut config = config;

    // Fail fast on a misconfigured CORS allowlist rather than silently serving a
    // narrower-than-intended (or empty) origin set at runtime.
    if let Err(e) = config.cors.validate() {
        bail!("invalid CORS configuration: {e}");
    }
    let browser_origins = Arc::new(BrowserOrigins::new(
        &config.listen,
        &config.cors,
        config.use_embedded_auth(),
    ));

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
        Some(auth::initialise(&config, &datastore).await?)
    } else {
        None
    };

    let auth_service = embedded_auth
        .as_ref()
        .map(|auth| Arc::new(auth.auth_service()));

    // A TEE replica re-announces itself while its authority evidence is
    // missing; idle on every other node. Spawned here because the announcement
    // it sends is the one fleet-join builds.
    drop(tokio::spawn(admin::handlers::tee::evidence_retry::run(
        datastore.clone(),
        node_client.clone(),
        config.tee_release_version.clone(),
        #[cfg(feature = "mock-attestation")]
        mock_tee,
        shutdown.clone(),
    )));

    // With embedded auth this process checks every request itself, so an
    // opened envelope meets the same guard a direct request would. In proxy
    // mode the proxy is the guard, and it cannot see inside an envelope: only
    // what this process serves without a credential may be reached sealed.
    let inner_scope = if config.use_embedded_auth() {
        sealed::InnerScope::Any
    } else {
        sealed::InnerScope::Uncredentialed {
            delegated_access: config
                .admin
                .as_ref()
                .is_some_and(|admin| admin.delegated_access),
        }
    };
    let transport = Arc::new(sealed::SealedTransport::generate(
        &sealed::SealedOptions::new(
            config.sealed.required,
            std::env::var("NODE_PATH_PREFIX")
                .ok()
                .filter(|prefix| !prefix.is_empty()),
        )
        .with_inner_scope(inner_scope),
        &mut prom_registry,
    ));
    if config.sealed.required {
        info!("Sealed transport required: unsealed requests are refused");
    }
    drop(tokio::spawn(sealed::expire_sessions(
        Arc::downgrade(&transport),
        shutdown.clone(),
    )));
    let shared_state = Arc::new(
        AdminState::new(
            datastore.clone(),
            ctx_client.clone(),
            node_client.clone(),
            readiness,
            transport.public_key(),
            #[cfg(feature = "mock-attestation")]
            mock_tee,
        )
        .with_tee_release_version(config.tee_release_version.clone()),
    );
    let mounted = mount_runtime_services(
        app,
        &config,
        service_mounts::RuntimeServiceDeps {
            auth_service: auth_service.clone(),
            ctx_client,
            node_client: node_client.clone(),
            datastore: datastore.clone(),
            shared_state,
            prom_registry,
        },
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

    // Outermost first: the browser guard, CORS, then the sealed envelope, whose own
    // response is the only one a browser sees for a sealed call, so CORS wraps it.
    let app = ServiceBuilder::new()
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&browser_origins),
            browser_origins::guard,
        ))
        .layer(build_cors_layer(&config.cors, browser_origins))
        .layer(axum::middleware::from_fn_with_state(
            transport,
            sealed::intercept,
        ))
        .service(app);

    let mut set = JoinSet::new();

    for listener in listeners {
        let app = app.clone();
        let shutdown = shutdown.clone();
        // `with_graceful_shutdown` stops accepting new connections when the
        // token fires and then waits for in-flight requests to finish before
        // the serve future resolves — so a termination signal drains requests
        // instead of the server task being dropped mid-response.
        drop(set.spawn(async move {
            axum::serve(
                listener,
                ServiceExt::<Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(
                    app,
                ),
            )
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await
        }));
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
/// **`allow_credentials` is intentionally not set**: callers send bearer
/// tokens, never cookies.
///
/// Answers only the origins [`BrowserOrigins`] admits; the guard in front of it
/// has already refused the rest.
fn build_cors_layer(cors: &crate::config::CorsConfig, origins: Arc<BrowserOrigins>) -> CorsLayer {
    CorsLayer::new()
        .allow_headers(Any)
        .allow_methods([
            Method::POST,
            Method::GET,
            Method::DELETE,
            Method::PUT,
            Method::OPTIONS,
        ])
        .expose_headers([
            axum::http::HeaderName::from_static("x-auth-error"),
            axum::http::HeaderName::from_static("x-auth-user"),
            axum::http::HeaderName::from_static("x-auth-permissions"),
        ])
        .allow_private_network(cors.allow_private_network)
        .allow_origin(AllowOrigin::predicate(move |_, request| {
            origins.admits(&request.headers, &request.uri)
        }))
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

    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, HeaderValue, Request, StatusCode};
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use super::{build_cors_layer, BrowserOrigins};
    use crate::config::CorsConfig;

    /// Origin header the Tauri desktop webview presents in production.
    const TAURI_ORIGIN: &str = "http://tauri.localhost";

    fn cors_layer(cors: &CorsConfig) -> tower_http::cors::CorsLayer {
        build_cors_layer(cors, Arc::new(BrowserOrigins::new(&[], cors, false)))
    }

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
            .layer(cors_layer(&CorsConfig::new()))
    }

    /// The allow-methods list tracks the methods actually routed, and no
    /// endpoint serves PATCH any more. Adding a PATCH route without adding
    /// PATCH back here fails the browser preflight as a status-0 network
    /// error while curl/CLI clients keep working, which is how the gap
    /// originally shipped unnoticed; this reddens instead.
    #[tokio::test]
    async fn cors_preflight_omits_patch_while_no_route_serves_it() {
        assert!(
            !include_str!("../endpoints.json").contains("\"PATCH "),
            "a PATCH route is back: re-add Method::PATCH to build_cors_layer"
        );

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

        let allowed = resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .expect("preflight must return Access-Control-Allow-Methods")
            .to_str()
            .expect("allow-methods should be ASCII");
        assert!(
            !allowed.contains("PATCH"),
            "PATCH is still preflight-allowed ({allowed}) but no route serves it"
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

    #[tokio::test]
    async fn cors_allowlist_admits_only_listed_origins() {
        let cors = CorsConfig {
            allowed_origins: Some(vec!["https://app.example".to_owned()]),
            ..CorsConfig::new()
        };
        let app = Router::new()
            .route("/x", get(ok_handler))
            .layer(cors_layer(&cors));

        // Allowlisted origin → echoed back.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/x")
                    .header(header::ORIGIN, "https://app.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            Some("https://app.example"),
            "allowlisted origin must be admitted"
        );

        // Non-allowlisted origin → no allow-origin header.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/x")
                    .header(header::ORIGIN, "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "a non-allowlisted origin must not receive Access-Control-Allow-Origin"
        );
    }

    #[tokio::test]
    async fn default_cors_refuses_foreign_origin_on_admin_api() {
        let app = Router::new()
            .route("/admin-api/contexts", get(ok_handler))
            .layer(cors_layer(&CorsConfig::new()));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/admin-api/contexts")
                    .header(header::ORIGIN, "https://evil.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header("access-control-request-private-network", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "foreign origin admitted by default CORS: allow-origin={:?} allow-private-network={:?}",
            resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            resp.headers().get("access-control-allow-private-network"),
        );
    }

    #[tokio::test]
    async fn embedded_auth_cors_answers_a_hosted_app_and_proxy_does_not() {
        let preflight = |embedded: bool| async move {
            let cors = CorsConfig::new();
            Router::new()
                .route("/admin-api/contexts", get(ok_handler))
                .layer(build_cors_layer(
                    &cors,
                    Arc::new(BrowserOrigins::new(&[], &cors, embedded)),
                ))
                .oneshot(
                    Request::builder()
                        .method("OPTIONS")
                        .uri("/admin-api/contexts")
                        .header(header::ORIGIN, "https://app.example")
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .cloned()
        };
        assert_eq!(
            preflight(true).await,
            Some(HeaderValue::from_static("https://app.example")),
            "embedded auth"
        );
        assert_eq!(preflight(false).await, None, "proxy auth");
    }

    /// On unless `[server.cors]` turns it off, as before the Host/Origin guard: only an
    /// origin the guard admits gets the answer, so it reaches no page it did not.
    #[tokio::test]
    async fn private_network_access_is_on_unless_turned_off() {
        let preflight = |cors: CorsConfig| async move {
            Router::new()
                .route("/x", get(ok_handler))
                .layer(cors_layer(&cors))
                .oneshot(
                    Request::builder()
                        .method("OPTIONS")
                        .uri("/x")
                        .header(header::ORIGIN, "https://app.example")
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                        .header("access-control-request-private-network", "true")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .headers()
                .contains_key("access-control-allow-private-network")
        };
        let listed = CorsConfig {
            allowed_origins: Some(vec!["https://app.example".to_owned()]),
            ..CorsConfig::new()
        };
        assert!(
            preflight(listed.clone()).await,
            "a listed public page lost private-network access it had before"
        );
        assert!(
            !preflight(CorsConfig {
                allow_private_network: false,
                ..listed
            })
            .await,
            "allow_private_network = false still answered"
        );
    }
}
