use std::net::{IpAddr, SocketAddr};

use axum::{http, Router};
use axum_server::tls_rustls::RustlsConfig;
use calimero_store::Store;
use config::ServerConfig;
use tokio::sync::broadcast;
use tower_http::cors;
use tracing::warn;

pub mod certificates;

#[cfg(feature = "admin")]
pub mod admin;
pub mod config;
#[cfg(feature = "jsonrpc")]
pub mod jsonrpc;
#[cfg(feature = "admin")]
mod middleware;
mod verifysignature;
mod verifysnsignature;
#[cfg(feature = "websocket")]
pub mod ws;

pub async fn start(
    config: ServerConfig,
    server_sender: calimero_node_primitives::ServerSender,
    ctx_manager: calimero_context::ContextManager,
    node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
    store: Store,
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

    #[cfg(feature = "jsonrpc")]
    {
        if let Some((path, handler)) = jsonrpc::service(&config, server_sender.clone())? {
            app = app.route(path, handler);
            app = app.layer(middleware::auth::AuthSignatureLayer::new(store.clone()));

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
        if let Some((api_path, router)) =
            admin::service::setup(&config, store.clone(), ctx_manager)?
        {
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
            .allow_methods([
                http::Method::POST,
                http::Method::GET,
                http::Method::DELETE,
                http::Method::PUT,
                http::Method::OPTIONS,
            ])
            .allow_private_network(true),
    );
    // Check if the certificate exists and if they contain the current local IP address
    let (cert_pem, key_pem) = certificates::get_certificate(store.clone()).await?;

    // Configure certificate and private key used by https
    let rustls_config = match RustlsConfig::from_pem(cert_pem, key_pem).await {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to load TLS configuration: {:?}", e);
            return Err(e.into());
        }
    };

    let mut set = tokio::task::JoinSet::new();

    for listener in listeners {
        let rustls_config = rustls_config.clone();
        let app = app.clone();
        let addr = listener.local_addr().unwrap();
        set.spawn(async move {
            if let Err(e) = axum_server_dual_protocol::bind_dual_protocol(addr, rustls_config)
                .serve(app.into_make_service())
                .await
            {
                eprintln!("Server error: {:?}", e);
                return Err(e);
            }
            Ok::<(), std::io::Error>(())
        });
    }

    while let Some(result) = set.join_next().await {
        result??;
    }

    Ok(())
}
