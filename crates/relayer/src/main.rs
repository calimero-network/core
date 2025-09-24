//! Standalone Calimero relayer service
//!
//! This service provides a standalone relayer that forwards
//! requests to the appropriate blockchain protocols.

use std::collections::BTreeMap;
use std::env::var;
use std::net::{AddrParseError, SocketAddr};

use axum::extract::State;
use axum::http::status::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use calimero_context_config::client::config::{
    ClientConfig, ClientConfigParams, ClientLocalConfig, ClientLocalSigner, ClientRelayerSigner,
    ClientSelectedSigner, ClientSigner, Credentials, LocalConfig,
};
use calimero_context_config::client::relayer::{RelayRequest, ServerError};
use calimero_context_config::client::transport::{Transport, TransportArguments, TransportRequest};
use calimero_context_config::client::Client;
use clap::Parser;
use color_eyre::install;
use eyre::Result as EyreResult;
use futures_util::FutureExt;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info};
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

mod config;
mod constants;
mod credentials;
mod near;

use config::RelayerConfig;
use constants::{DEFAULT_ADDR, DEFAULT_RELAYER_URL};
use credentials::{convert_to_client_credentials, CredentialBuilder, RelayerCredentials};
use near::near_wallet_verification_handler;

/// Relayer service that handles incoming requests
#[derive(Debug)]
struct RelayerService {
    config: RelayerConfig,
}

impl RelayerService {
    /// Create a new relayer service with the given configuration
    fn new(config: RelayerConfig) -> Self {
        Self { config }
    }

    /// Create blockchain client from relayer configuration
    fn create_client(&self) -> EyreResult<Client<impl Transport>> {
        let client_config = self.build_client_config()?;
        Ok(Client::from_local_config(&client_config)?)
    }

    /// Convert relayer config to ClientConfig format
    fn build_client_config(&self) -> EyreResult<ClientConfig> {
        let mut params = BTreeMap::new();
        let mut protocols = BTreeMap::new();

        // Build configuration for each enabled protocol
        for (protocol_name, protocol_config) in self.config.enabled_protocols() {
            // Add protocol parameters
            drop(params.insert(
                protocol_name.clone(),
                ClientConfigParams {
                    signer: ClientSelectedSigner::Local,
                    network: protocol_config.network.clone(),
                    contract_id: protocol_config.contract_id.clone(),
                },
            ));

            // Add protocol signer configuration
            let mut signers = BTreeMap::new();

            // Create credentials based on protocol and what's available
            let credentials = match protocol_config.credentials.as_ref() {
                Some(creds) => self.convert_credentials(creds)?,
                None => {
                    // Generate dummy credentials for relayer-only mode
                    // The relayer typically doesn't need real credentials for many operations
                    self.generate_dummy_credentials(protocol_name)?
                }
            };

            drop(signers.insert(
                protocol_config.network.clone(),
                ClientLocalSigner {
                    rpc_url: protocol_config.rpc_url.clone(),
                    credentials,
                },
            ));

            drop(protocols.insert(protocol_name.clone(), ClientLocalConfig { signers }));
        }

        let client_config = ClientConfig {
            params,
            signer: ClientSigner {
                relayer: ClientRelayerSigner {
                    url: DEFAULT_RELAYER_URL
                        .parse()
                        .map_err(|e| eyre::eyre!("Failed to parse relayer URL: {e}"))?, // Self-reference for relayer mode
                },
                local: LocalConfig { protocols },
            },
        };

        Ok(client_config)
    }

    /// Convert relayer credentials to client credentials
    fn convert_credentials(&self, creds: &config::ProtocolCredentials) -> EyreResult<Credentials> {
        convert_to_client_credentials(creds)
    }

    /// Generate minimal dummy credentials for protocols without explicit credentials
    fn generate_dummy_credentials(&self, protocol: &str) -> EyreResult<Credentials> {
        RelayerCredentials::dummy_credentials(protocol)
    }

    /// Start the relayer service
    async fn start(self) -> EyreResult<()> {
        let (tx, mut rx) = mpsc::channel::<RequestPayload>(32);

        // Create blockchain client from relayer config
        let transports = self.create_client()?;

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
                    .map(|res| res.map_err(|e| eyre::eyre!("Transport error: {e:?}")))
                    .map_err(|err| ServerError::UnsupportedProtocol {
                        found: err.args.protocol,
                        expected: err.expected,
                    });

                let _ignored = res_tx.send(res);
            }
        };

        let app_state = AppState {
            request_sender: tx,
            config: self.config.clone(),
        };

        let app = Router::new()
            .route("/", post(handler))
            .route("/health", get(health_check))
            .route("/near/verify-wallet", post(near_wallet_verification_handler))
            .with_state(app_state);

        let listener = TcpListener::bind(self.config.listen).await?;

        info!(
            "Listening on '\x1b[1;33mhttp://{}\x1b[0m'",
            self.config.listen
        );

        let server = axum::serve(listener, app);

        tokio::try_join!(handle.map(Ok), server)?;

        Ok(())
    }
}

type RequestPayload = (RelayRequest<'static>, HandlerSender);
type HandlerSender = oneshot::Sender<Result<EyreResult<Vec<u8>>, ServerError>>;

/// Combined application state for the relayer
#[derive(Clone)]
struct AppState {
    request_sender: mpsc::Sender<RequestPayload>,
    config: RelayerConfig,
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "calimero-relayer",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

async fn handler(
    State(state): State<AppState>,
    Json(request): Json<RelayRequest<'static>>,
) -> Result<impl IntoResponse, StatusCode> {
    let (res_tx, res_rx) = oneshot::channel();

    state.request_sender
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

/// Standalone Calimero relayer
#[derive(Debug, Parser)]
#[command(
    name = "calimero-relayer",
    about = "Standalone Calimero relayer for external client interactions",
    version = env!("CARGO_PKG_VERSION")
)]
struct Cli {
    /// Sets the address to listen on [default: 0.0.0.0:63529]
    /// Valid: `63529`, `127.0.0.1`, `127.0.0.1:63529` [env: PORT]
    #[clap(short, long, value_name = "URI")]
    #[clap(verbatim_doc_comment, value_parser = addr_from_str)]
    #[clap(default_value_t = DEFAULT_ADDR)]
    pub listen: SocketAddr,

    /// Configuration file path (optional, uses environment variables if not provided)
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    let cli = Cli::parse();

    // Load configuration from file or environment
    let mut config = if let Some(config_path) = cli.config {
        // Load from file
        let config_str = std::fs::read_to_string(&config_path)?;
        if config_path.extension().and_then(|s| s.to_str()) == Some("json") {
            serde_json::from_str(&config_str)?
        } else {
            toml::from_str(&config_str)?
        }
    } else {
        // Load from environment variables
        RelayerConfig::from_env()
    };

    // Override listen address from CLI if provided
    config.listen = cli.listen;

    let service = RelayerService::new(config);
    service.start().await
}

fn setup() -> EyreResult<()> {
    registry()
        .with(EnvFilter::builder().parse(format!(
            "calimero_relayer=info,calimero_=info,{}",
            var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(layer())
        .init();

    install()?;

    Ok(())
}

/// Parse a socket address from a string, supporting various formats
fn addr_from_str(s: &str) -> Result<SocketAddr, AddrParseError> {
    let mut addr = DEFAULT_ADDR;

    let env_port = 'port: {
        if let Ok(env_port) = var("PORT") {
            if let Ok(env_port) = env_port.parse() {
                break 'port Some(env_port);
            }
            eprintln!(
                "warning: invalid 'PORT' environment variable: '{}', ignoring..",
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
