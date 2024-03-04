use std::net::IpAddr;

use axum::routing::Router;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

pub mod config;
#[cfg(feature = "graphql")]
pub mod graphql;

type Sender = mpsc::Sender<(
    String,
    Vec<u8>,
    oneshot::Sender<calimero_runtime::logic::Outcome>,
)>;

pub async fn start(config: config::ServerConfig, sender: Sender) -> eyre::Result<()> {
    let mut addrs = Vec::with_capacity(config.listen.len());

    for addr in config.listen.iter() {
        let mut components = addr.iter();

        let host: IpAddr = match components.next() {
            Some(multiaddr::Protocol::Ip4(host)) => host.into(),
            Some(multiaddr::Protocol::Ip6(host)) => host.into(),
            _ => eyre::bail!("Invalid multiaddr, expected IP4 component"),
        };

        let Some(multiaddr::Protocol::Tcp(port)) = components.next() else {
            eyre::bail!("Invalid multiaddr, expected TCP component");
        };

        addrs.push((host, port).into());
    }

    let listener = tokio::net::TcpListener::bind(addrs.as_slice()).await?;

    let mut app = Router::new();

    let mut serviced = false;

    #[cfg(feature = "graphql")]
    {
        if let Some((path, handler)) = graphql::service(&config, sender.clone())? {
            app = app.route(path, handler);
            serviced = true;
        }
    }

    if !serviced {
        warn!("No services enabled, enable at least one service to start the server");

        return Ok(());
    }

    axum::serve(listener, app).await?;

    Ok(())
}
