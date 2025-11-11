use core::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use tower as _;

use axum::http::Method;
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
use tracing::warn;

use crate::admin::service::{setup, site};

pub mod admin;
#[cfg(feature = "bundled-auth")]
mod auth;
pub mod config;
pub mod jsonrpc;
mod metrics;
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

    let mut service_count = 0usize;

    let shared_state = Arc::new(AdminState::new(
        datastore.clone(),
        ctx_client.clone(),
        node_client.clone(),
    ));

    if let Some((path, router)) = jsonrpc::service(&config, ctx_client) {
        app = app.nest(&path, router);
        service_count += 1;
    }

    if let Some((path, handler)) = ws::service(&config, node_client.clone()) {
        app = app.route(&path, handler);
        service_count += 1;
    }

    if let Some((path, router)) = sse::service(&config, node_client.clone(), datastore.clone()) {
        app = app.nest(&path, router);
        service_count += 1;
    }

    if let Some((api_path, router)) = setup(&config, shared_state) {
        if let Some((site_path, serve_dir)) = site(&config) {
            app = app.nest_service(site_path.as_str(), serve_dir);
        }

        app = app.nest(&api_path, router);
        service_count += 1;
    }

    if let Some((path, router)) = metrics::service(&config, prom_registry) {
        app = app.nest(path, router);
        service_count += 1;
    }

    #[cfg(feature = "bundled-auth")]
    {
        let bundled_auth = auth::initialise(&config).await?;
        app = app.merge(bundled_auth.into_router());
        service_count += 1;
    }

    if service_count == 0 {
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

#[cfg(test)]
mod integration_tests_package_usage {
    use {color_eyre as _, tracing_subscriber as _};
}
