use std::io::Error as IoError;
use std::net::{IpAddr, SocketAddr};

use axum::http::Method;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use axum_server_dual_protocol::bind_dual_protocol;
use calimero_context::ContextManager;
use calimero_node_primitives::ServerSender;
use calimero_primitives::events::NodeEvent;
use calimero_store::Store;
use config::ServerConfig;
use eyre::{bail, Result as EyreResult};
use multiaddr::Protocol;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tower_http::cors::{Any, CorsLayer};
use tracing::warn;

use crate::admin::service::setup;
use crate::certificates::get_certificate;

pub mod certificates;

#[cfg(feature = "admin")]
pub mod admin;
pub mod config;
#[cfg(feature = "jsonrpc")]
pub mod jsonrpc;
#[cfg(feature = "admin")]
mod middleware;
mod verifysignature;
#[cfg(feature = "websocket")]
pub mod ws;

// TODO: Consider splitting this long function into multiple parts.
#[allow(clippy::too_many_lines)]
#[allow(clippy::print_stderr)]
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

    #[cfg(feature = "jsonrpc")]
    {
        if let Some((path, handler)) = jsonrpc::service(&config, server_sender.clone()) {
            app = app.route(path, handler);
            app = app.layer(middleware::auth::AuthSignatureLayer::new(store.clone()));

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
        if let Some((api_path, router)) = setup(&config, store.clone(), ctx_manager) {
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
    // Check if the certificate exists and if they contain the current local IP address
    let (cert_pem, key_pem) = get_certificate(&store)?;

    // Configure certificate and private key used by https
    let rustls_config = match RustlsConfig::from_pem(cert_pem, key_pem).await {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to load TLS configuration: {e:?}");
            return Err(e.into());
        }
    };

    let mut set = JoinSet::new();

    for listener in listeners {
        let rustls_config = rustls_config.clone();
        let app = app.clone();
        let addr = listener.local_addr().unwrap();
        drop(set.spawn(async move {
            if let Err(e) = bind_dual_protocol(addr, rustls_config)
                .serve(app.into_make_service())
                .await
            {
                eprintln!("Server error: {e:?}");
                return Err(e);
            }
            Ok::<(), IoError>(())
        }));
    }

    while let Some(result) = set.join_next().await {
        result??;
    }

    Ok(())
}
