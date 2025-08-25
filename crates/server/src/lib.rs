use core::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::Method;
use axum::middleware::{from_fn, Next};
use axum::response::Response;
use axum::Router;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use config::ServerConfig;
use eyre::{bail, Result as EyreResult};
use multiaddr::Protocol;
use prometheus_client::registry::Registry;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::warn;
use tracing::Level;

use crate::admin::service::{setup, site};

#[cfg(feature = "admin")]
pub mod admin;
pub mod config;
#[cfg(feature = "jsonrpc")]
pub mod jsonrpc;
mod metrics;
#[cfg(feature = "admin")]
mod middleware;
#[cfg(feature = "websocket")]
pub mod ws;
#[cfg(feature = "websocket")]
pub mod sse;


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

// TODO: Consider splitting this long function into multiple parts.
#[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
#[expect(clippy::print_stderr, reason = "Acceptable for CLI")]
pub async fn start(
    config: ServerConfig,
    ctx_client: ContextClient,
    node_client: NodeClient,
    datastore: Store,
    prom_registry: Registry,
) -> EyreResult<()> {
    let mut config = config;
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

    let mut serviced = false;

    let shared_state = Arc::new(AdminState::new(
        datastore.clone(),
        ctx_client.clone(),
        node_client.clone(),
    ));

    #[cfg(feature = "jsonrpc")]
    {
        if let Some((path, router)) = jsonrpc::service(&config, ctx_client) {
            app = app.nest(&path, router);
            serviced = true;
        }
    }

    #[cfg(feature = "websocket")]
    {
        if let Some((path, handler)) = ws::service(&config, node_client.clone()) {
            app = app.route(&path, handler);

            serviced = true;
        }
    }

    #[cfg(feature = "sse")]
    {
    }


    #[cfg(feature = "admin")]
    {
        if let Some((api_path, router)) = setup(&config, shared_state) {
            if let Some((site_path, serve_dir)) = site(&config) {
                app = app.nest_service(site_path.as_str(), serve_dir);
            }

            app = app.nest(&api_path, router);
            serviced = true;
        }
    }

    #[cfg(feature = "metrics")]
    {
        if let Some((path, router)) = metrics::service(&config, prom_registry) {
            app = app.nest(path, router);
            serviced = true;
        }
    }

    if !serviced {
        warn!("No services enabled, enable at least one service to start the server");

        return Ok(());
    }

    app = app.layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_headers(Any)
            .allow_methods([
                Method::POST,
                Method::GET,
                Method::DELETE,
                Method::PUT,
                Method::OPTIONS,
            ])
            .allow_private_network(true),
    );

    // Middleware to log small JSON request bodies (without impacting handlers) - COMMENTED OUT
    app = app.layer(from_fn(log_request_body));

    // Global structured request/response tracing for all server routes
    app = app.layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &axum::http::Request<_>| {
                tracing::span!(
                    Level::DEBUG,
                    "http_request",
                    method = %request.method(),
                    path = %request.uri().path()
                )
            })
            .on_request(|request: &axum::http::Request<_>, _span: &tracing::Span| {
                let content_type = request
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                tracing::debug!(
                    target: "server::http",
                    method = %request.method(),
                    path = %request.uri().path(),
                    content_type,
                    "incoming request"
                );
            })
            .on_response(
                |response: &axum::http::Response<_>,
                 latency: std::time::Duration,
                 _span: &tracing::Span| {
                    tracing::debug!(
                        target: "server::http",
                        status = %response.status(),
                        elapsed_ms = latency.as_millis() as u64,
                        "response sent"
                    );
                },
            ),
    );

    // Log response bodies (capped) after handlers run (skips static assets)
    app = app.layer(from_fn(log_response_body));

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

// Log request body if it is small (cap to avoid huge payloads), then restore the body for downstream handlers
async fn log_request_body(req: axum::http::Request<Body>, next: Next) -> Response {
    const MAX_LOG_BYTES: usize = 4096;

    // Read and buffer the body (with a size cap)
    let (parts, body) = req.into_parts();
    match to_bytes(body, MAX_LOG_BYTES).await {
        Ok(bytes) => {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let truncated = bytes.len() >= MAX_LOG_BYTES;
                tracing::debug!(target: "server::http", truncated, body = %text, "request body");
            } else {
                tracing::debug!(target: "server::http", size = bytes.len(), "request body (non-utf8)");
            }
            // Reconstruct the request with the buffered body for downstream
            let req_restored = axum::http::Request::from_parts(parts, Body::from(bytes));
            next.run(req_restored).await
        }
        Err(err) => {
            tracing::debug!(target: "server::http", error = %err, "failed to read request body");
            // Fall through and pass the original request (with empty body) to downstream
            let req_restored = axum::http::Request::from_parts(parts, Body::empty());
            next.run(req_restored).await
        }
    }
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {color_eyre as _, tracing_subscriber as _};
}

// Log response body if it is small (cap to avoid huge payloads) and then restore the body
async fn log_response_body(req: axum::http::Request<Body>, next: Next) -> Response {
    const MAX_LOG_BYTES: usize = 4096;

    // Skip logging for static asset routes to avoid Content-Length issues
    let path = req.uri().path();
    if path.starts_with("/admin-dashboard/") || path.contains("/assets/") {
        return next.run(req).await;
    }

    // Run the rest of the stack first
    let res = next.run(req).await;

    let (parts, body) = res.into_parts();
    match to_bytes(body, MAX_LOG_BYTES).await {
        Ok(bytes) => {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let truncated = bytes.len() >= MAX_LOG_BYTES;
                tracing::debug!(target: "server::http", truncated, body = %text, "response body");
            } else {
                tracing::debug!(target: "server::http", size = bytes.len(), "response body (non-utf8)");
            }
            Response::from_parts(parts, Body::from(bytes))
        }
        Err(err) => {
            tracing::debug!(target: "server::http", error = %err, "failed to read response body");
            Response::from_parts(parts, Body::empty())
        }
    }
}
