use std::net::{IpAddr, SocketAddr};

use axum::{http, Router};
use config::ServerConfig;
use tokio::sync::{broadcast, mpsc, oneshot};
use tower_http::cors;
use tracing::warn;

#[cfg(feature = "admin")]
pub mod admin;
pub mod config;
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "jsonrpc")]
pub mod jsonrpc;
mod middleware;
mod verifysignature;
#[cfg(feature = "websocket")]
pub mod ws;

// TODO: add comments or even better make it explicit types
type ServerSender = mpsc::Sender<(
    // todo! move to calimero-node-primitives
    calimero_primitives::application::ApplicationId,
    String,
    Vec<u8>,
    bool,
    oneshot::Sender<calimero_runtime::logic::Outcome>,
)>;

pub async fn start(
    config: ServerConfig,
    server_sender: ServerSender,
    node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
) -> eyre::Result<()> {
    let mut config = config;
    let mut addrs = Vec::with_capacity(config.listen.len());
    let mut listeners = Vec::with_capacity(config.listen.len());
    let mut want_listeners = config.listen.into_iter().peekable();

    while let Some(addr) = want_listeners.next() {
        let mut components = addr.iter();

        let host: IpAddr = match components.next() {
            Some(multiaddr::Protocol::Ip4(host)) => host.into(),
            Some(multiaddr::Protocol::Ip6(host)) => host.into(),
            _ => eyre::bail!("Invalid multiaddr, expected IP4 component"),
        };

        let Some(multiaddr::Protocol::Tcp(port)) = components.next() else {
            eyre::bail!("Invalid multiaddr, expected TCP component");
        };

        match tokio::net::TcpListener::bind(SocketAddr::from((host, port))).await {
            Ok(listener) => {
                let local_port = listener.local_addr()?.port();
                addrs.push(
                    addr.replace(1, |_| Some(multiaddr::Protocol::Tcp(local_port)))
                        .unwrap(), // safety: we know the index is valid
                );
                listeners.push(listener);
            }
            Err(err) => {
                if want_listeners.peek().is_none() {
                    eyre::bail!(err);
                }
            }
        }
    }
    config.listen = addrs;

    let mut app = Router::new();

    let mut serviced = false;

    #[cfg(feature = "graphql")]
    {
        if let Some((path, handler)) = graphql::service(&config, server_sender.clone())? {
            let identity = config.identity.clone();
            app = app.route(path, handler);
            //.layer(middleware::auth::AuthSignatureLayer::new(identity)); //TODO will be replaced with json RPC

            serviced = true;
        }
    }

    #[cfg(feature = "jsonrpc")]
    {
        if let Some((path, handler)) = jsonrpc::service(&config, server_sender.clone())? {
            app = app.route(path, handler);

            serviced = true;
        }
    }

    #[cfg(feature = "websocket")]
    {
        if let Some((path, handler)) = ws::service(&config, node_events.clone())? {
            app = app.route(path, handler);

            serviced = true;
        }
    }

    #[cfg(feature = "admin")]
    {
        if let Some((api_path, router)) = admin::service(&config)? {
            if let Some((site_path, serve_dir)) = admin::site(&config)? {
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
        cors::CorsLayer::new()
            .allow_origin(cors::Any)
            .allow_headers(cors::Any)
            .allow_methods([http::Method::POST]),
    );

    let mut set = tokio::task::JoinSet::new();

    for listener in listeners {
        let app = app.clone();
        set.spawn(async { axum::serve(listener, app).await });
    }

    while let Some(result) = set.join_next().await {
        result??;
    }

    Ok(())
}
