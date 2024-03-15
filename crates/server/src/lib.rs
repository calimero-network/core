use std::net::{IpAddr, SocketAddr};

use axum::routing::Router;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::warn;

pub mod config;
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "websocket")]
pub mod websocket;

type ServerSender = mpsc::Sender<(
    // todo! move to calimero-node-primitives
    String,
    Vec<u8>,
    bool,
    oneshot::Sender<calimero_runtime::logic::Outcome>,
)>;

pub async fn start(
    config: config::ServerConfig,
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
            app = app.route(path, handler);
            serviced = true;
        }
    }

    #[cfg(feature = "websocket")]
    {
        if let Some((path, handler)) = websocket::service2(&config, node_events.clone())? {
            app = app.route(path, handler);
            serviced = true;
        }
    }

    if !serviced {
        warn!("No services enabled, enable at least one service to start the server");

        return Ok(());
    }

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
