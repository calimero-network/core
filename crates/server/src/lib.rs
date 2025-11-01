#[cfg(feature = "http-server")]
use core::net::{IpAddr, SocketAddr};
use std::sync::Arc;

#[cfg(feature = "http-server")]
use tower as _;

#[cfg(feature = "http-server")]
use axum::http::Method;
#[cfg(feature = "http-server")]
use axum::Router;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use config::ServerConfig;
#[cfg(feature = "http-server")]
use eyre::bail;
use eyre::Result as EyreResult;
#[cfg(feature = "http-server")]
use multiaddr::Protocol;
use prometheus_client::registry::Registry;
#[cfg(feature = "http-server")]
use tokio::net::TcpListener;
#[cfg(feature = "http-server")]
use tokio::task::JoinSet;
#[cfg(feature = "http-server")]
use tower_http::cors::{Any, CorsLayer};
#[cfg(feature = "http-server")]
use tracing::warn;

#[cfg(feature = "http-server")]
use crate::admin::service::{setup, site};

#[cfg(feature = "http-server")]
pub mod admin;
pub mod config;
#[cfg(feature = "http-server")]
pub mod jsonrpc;
#[cfg(feature = "http-server")]
mod metrics;
#[cfg(feature = "http-server")]
pub mod sse;
#[cfg(feature = "http-server")]
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

// Server mode: Full HTTP server with Axum
#[cfg(feature = "http-server")]
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

    if let Some((path, router)) = jsonrpc::service(&config, ctx_client) {
        app = app.nest(&path, router);
        serviced = true;
    }

    if let Some((path, handler)) = ws::service(&config, node_client.clone()) {
        app = app.route(&path, handler);

        serviced = true;
    }

    if let Some((path, router)) = sse::service(&config, node_client.clone(), datastore.clone()) {
        app = app.nest(&path, router);
        serviced = true;
    }

    if let Some((api_path, router)) = setup(&config, shared_state) {
        if let Some((site_path, serve_dir)) = site(&config) {
            app = app.nest_service(site_path.as_str(), serve_dir);
        }

        app = app.nest(&api_path, router);
        serviced = true;
    }

    if let Some((path, router)) = metrics::service(&config, prom_registry) {
        app = app.nest(path, router);
        serviced = true;
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

// Desktop mode: No HTTP server
#[cfg(not(feature = "http-server"))]
pub async fn start(
    _config: ServerConfig,
    _ctx_client: ContextClient,
    _node_client: NodeClient,
    _datastore: Store,
    _prom_registry: Registry,
) -> EyreResult<()> {
    // In desktop mode, calimero-server doesn't run an HTTP server
    // The Tauri app uses ContextClient/NodeClient directly via IPC
    std::future::pending().await
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {color_eyre as _, tracing_subscriber as _};
}
