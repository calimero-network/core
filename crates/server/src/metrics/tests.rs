use core::net::Ipv4Addr;
use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::Duration;

use calimero_context_client::client::ContextClient;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use libp2p::identity::Keypair;
use mero_auth::config::StorageConfig;
use multiaddr::{Multiaddr, Protocol};
use prometheus_client::registry::Registry;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::{AuthMode, ServerConfig, ServiceConfigs};
use crate::jsonrpc::JsonRpcConfig;
use crate::test_support::test_node_client;
use crate::NodeReadiness;

fn free_port() -> u16 {
    StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .expect("a free loopback port")
        .port()
}

fn loopback(port: u16) -> Multiaddr {
    Multiaddr::from(Ipv4Addr::LOCALHOST).with(Protocol::Tcp(port))
}

/// The status `GET /metrics` gets on `port`, or `None` when nothing listens there.
async fn scrape(port: u16) -> Option<u16> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.ok()?;
    let request =
        format!("GET /metrics HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = String::new();
    let _read = stream.read_to_string(&mut response).await.ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// Runs the whole server with `/metrics` asked for on `metrics_port`, until it
/// accepts. Returns the public port.
async fn start_server(
    auth_mode: AuthMode,
    metrics_port: u16,
    shutdown: CancellationToken,
) -> (u16, TempDir) {
    let public_port = free_port();
    let mut config = ServerConfig::with_auth(
        vec![loopback(public_port)],
        Keypair::generate_ed25519(),
        ServiceConfigs {
            admin: None,
            jsonrpc: Some(JsonRpcConfig::new(true)),
            websocket: None,
            sse: None,
        },
        auth_mode,
        None,
    );
    if matches!(auth_mode, AuthMode::Embedded) {
        let mut auth = mero_auth::embedded::default_config();
        auth.storage = StorageConfig::Memory;
        config.embedded_auth = Some(auth);
    }
    config.metrics_listen = Some(loopback(metrics_port));

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, blob_dir) =
        test_node_client(&store, LazyRecipient::new(), broadcast::channel(1).0).await;
    let ctx_client = ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
    let server = tokio::spawn(crate::start(
        config,
        ctx_client,
        node_client,
        store,
        Registry::default(),
        Arc::new(NodeReadiness::new()),
        shutdown,
        #[cfg(feature = "mock-attestation")]
        false,
    ));

    for _ in 0..200 {
        if server.is_finished() {
            panic!("the server stopped: {:?}", server.await);
        }
        // The metrics listener is bound before the public one serves.
        if TcpStream::connect((Ipv4Addr::LOCALHOST, public_port))
            .await
            .is_ok()
        {
            return (public_port, blob_dir);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server did not start listening");
}

async fn assert_metrics_only_on_their_own_listener(auth_mode: AuthMode) {
    let shutdown = CancellationToken::new();
    let metrics_port = free_port();
    let (public_port, _blob_dir) = start_server(auth_mode, metrics_port, shutdown.clone()).await;

    assert_eq!(
        scrape(public_port).await,
        Some(404),
        "the public listener must not serve /metrics"
    );
    assert_eq!(
        scrape(metrics_port).await,
        Some(200),
        "the metrics listener serves /metrics"
    );
    shutdown.cancel();
}

#[tokio::test]
async fn proxy_mode_serves_metrics_only_on_the_metrics_listener() {
    assert_metrics_only_on_their_own_listener(AuthMode::Proxy).await;
}

#[tokio::test]
async fn embedded_mode_serves_metrics_only_on_the_metrics_listener() {
    assert_metrics_only_on_their_own_listener(AuthMode::Embedded).await;
}

#[tokio::test]
async fn a_taken_default_address_is_skipped_but_a_configured_one_must_bind() {
    let taken = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken_addr = taken.local_addr().unwrap();

    let skipped = super::bind(None, taken_addr).await;
    assert!(
        matches!(skipped, Ok(None)),
        "a node must start without metrics when the default port is taken"
    );

    let refused = super::bind(Some(&loopback(taken_addr.port())), taken_addr).await;
    assert!(
        refused.is_err(),
        "an address the operator set must not be silently dropped"
    );

    let not_tcp: Multiaddr = "/dns4/localhost/tcp/9528".parse().unwrap();
    assert!(
        super::bind(Some(&not_tcp), taken_addr).await.is_err(),
        "an address that names no IP must be refused"
    );
}

#[tokio::test]
async fn the_default_address_is_used_when_none_is_configured() {
    let default = (Ipv4Addr::LOCALHOST, free_port()).into();
    let listener = super::bind(None, default)
        .await
        .unwrap()
        .expect("a free default address binds");
    assert_eq!(listener.local_addr().unwrap(), default);
}
