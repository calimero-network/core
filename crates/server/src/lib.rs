use core::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use admin::storage::jwt_secret::get_or_create_jwt_secret;
use axum::http::Method;
use axum::Router;
use calimero_context::ContextManager;
use calimero_node_primitives::ServerSender;
use calimero_primitives::events::NodeEvent;
use calimero_store::Store;
use config::ServerConfig;
use eyre::{bail, Result as EyreResult};
use libp2p::identity::Keypair;
use multiaddr::Protocol;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tower_http::cors::{Any, CorsLayer};
use tracing::warn;

use crate::admin::service::{setup, site};

#[cfg(feature = "admin")]
pub mod admin;
pub mod config;
#[cfg(feature = "jsonrpc")]
pub mod jsonrpc;
#[cfg(feature = "admin")]
mod middleware;
mod verifywalletsignatures;
#[cfg(feature = "websocket")]
pub mod ws;

#[derive(Debug)]
#[non_exhaustive]
pub struct AdminState {
    pub store: Store,
    pub keypair: Keypair,
    pub ctx_manager: ContextManager,
}

impl AdminState {
    #[must_use]
    pub const fn new(store: Store, keypair: Keypair, ctx_manager: ContextManager) -> Self {
        Self {
            store,
            keypair,
            ctx_manager,
        }
    }
}

// TODO: Consider splitting this long function into multiple parts.
#[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
#[expect(clippy::print_stderr, reason = "Acceptable for CLI")]
pub async fn start(
    config: ServerConfig,
    server_sender: ServerSender,
    ctx_manager: ContextManager,
    node_events: broadcast::Sender<NodeEvent>,
    store: Store,
) -> EyreResult<()> {
    let mut config = config;
    let mut addrs = Vec::with_capacity(config.listen.len());
    let mut listeners = Vec::with_capacity(config.listen.len());
    let mut want_listeners = config.listen.into_iter().peekable();

    if let Err(e) = get_or_create_jwt_secret(&store) {
        eprintln!("Failed to get JWT key: {e:?}");
        return Err(e);
    }

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
        store.clone(),
        config.identity.clone(),
        ctx_manager,
    ));

    #[cfg(feature = "jsonrpc")]
    {
        if let Some((path, router)) =
            jsonrpc::service(&config, server_sender.clone(), store.clone())
        {
            app = app.nest(path, router);
            serviced = true;
        }
    }

    #[cfg(feature = "websocket")]
    {
        if let Some((path, handler)) = ws::service(&config, node_events.clone()) {
            app = app.route(path, handler);

            serviced = true;
        }
    }

    #[cfg(feature = "admin")]
    {
        if let Some((api_path, router)) = setup(&config, store.clone(), shared_state) {
            if let Some((site_path, serve_dir)) = site(&config) {
                app = app.nest_service(site_path, serve_dir);
            }

            app = app.nest(api_path, router);
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
