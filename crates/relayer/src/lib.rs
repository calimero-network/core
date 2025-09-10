//! Calimero relayer library for external client interactions
//!
//! This crate provides functionality to run a relay server that forwards
//! requests to the appropriate blockchain protocols.

use core::net::{AddrParseError, IpAddr, Ipv4Addr, SocketAddr};
use std::env;

use axum::extract::State;
use axum::http::status::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use calimero_config::ConfigFile;
use calimero_context_config::client::relayer::{RelayRequest, ServerError};
use calimero_context_config::client::transport::{Transport, TransportArguments, TransportRequest};
use calimero_context_config::client::Client;
use camino::Utf8PathBuf;
use eyre::{bail, Result as EyreResult};
use futures_util::FutureExt;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

pub const DEFAULT_PORT: u16 = 63529; // Mero-rELAY = MELAY
pub const DEFAULT_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT);

/// Configuration for the relayer service
#[derive(Debug, Clone)]
pub struct RelayerConfig {
    /// The socket address to listen on
    pub listen: SocketAddr,
    /// Path to the node configuration directory
    pub node_path: Utf8PathBuf,
}

impl RelayerConfig {
    /// Create a new relayer configuration
    pub fn new(listen: SocketAddr, node_path: Utf8PathBuf) -> Self {
        Self { listen, node_path }
    }
}

/// Relayer service that handles incoming requests
#[derive(Debug)]
pub struct RelayerService {
    config: RelayerConfig,
}

impl RelayerService {
    /// Create a new relayer service with the given configuration
    pub fn new(config: RelayerConfig) -> Self {
        Self { config }
    }

    /// Start the relayer service
    pub async fn start(self) -> EyreResult<()> {
        if !ConfigFile::exists(&self.config.node_path) {
            bail!("Node is not initialized in {:?}", self.config.node_path);
        }

        let config = ConfigFile::load(&self.config.node_path).await?;

        let (tx, mut rx) = mpsc::channel::<RequestPayload>(32);

        let transports = Client::from_local_config(&config.context.client)?;

        let handle = async move {
            while let Some((request, res_tx)) = rx.recv().await {
                let args = TransportArguments {
                    protocol: request.protocol,
                    request: TransportRequest {
                        network_id: request.network_id,
                        contract_id: request.contract_id,
                        operation: request.operation,
                    },
                    payload: request.payload,
                };

                let res = transports
                    .try_send(args)
                    .await
                    .map(|res| res.map_err(Into::into))
                    .map_err(|err| ServerError::UnsupportedProtocol {
                        found: err.args.protocol,
                        expected: err.expected,
                    });

                let _ignored = res_tx.send(res);
            }
        };

        let app = Router::new().route("/", post(handler)).with_state(tx);

        let listener = TcpListener::bind(self.config.listen).await?;

        info!("Listening on '\x1b[1;33mhttp://{}\x1b[0m'", self.config.listen);

        let server = axum::serve(listener, app);

        tokio::try_join!(handle.map(Ok), server)?;

        Ok(())
    }
}

type AppState = mpsc::Sender<RequestPayload>;
type RequestPayload = (RelayRequest<'static>, HandlerSender);
type HandlerSender = oneshot::Sender<Result<EyreResult<Vec<u8>>, ServerError>>;

async fn handler(
    State(req_tx): State<AppState>,
    Json(request): Json<RelayRequest<'static>>,
) -> Result<impl IntoResponse, StatusCode> {
    let (res_tx, res_rx) = oneshot::channel();

    req_tx
        .send((request, res_tx))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let res = res_rx
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let res = match res {
        Ok(res) => res,
        Err(err) => {
            debug!("failed to send request to handler: {:?}", err);

            return Ok((StatusCode::BAD_REQUEST, Json(err)).into_response());
        }
    };

    match res {
        Ok(res) => Ok(res.into_response()),
        Err(err) => {
            debug!("failed to send request to handler: {:?}", err);

            Ok((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response())
        }
    }
}

/// Parse a socket address from a string, supporting various formats
pub fn addr_from_str(s: &str) -> Result<SocketAddr, AddrParseError> {
    let mut addr = DEFAULT_ADDR;

    let env_port = 'port: {
        if let Ok(env_port) = env::var("PORT") {
            if let Ok(env_port) = env_port.parse() {
                break 'port Some(env_port);
            }
            warn!(
                "invalid '\x1b[1mPORT\x1b[0m' environment variable: '\x1b[33m{}\x1b[0m', ignoring..",
                env_port
            );
        }
        None
    };

    if let Ok(port) = s.parse() {
        addr.set_port(port);
        return Ok(addr);
    }

    if let Ok(host) = s.parse() {
        addr.set_ip(host);
        if let Some(port) = env_port {
            addr.set_port(port);
        }
        return Ok(addr);
    }

    s.parse()
}
