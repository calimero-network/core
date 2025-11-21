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
mod mock;

use config::RelayerConfig;
use constants::{protocols, DEFAULT_ADDR, DEFAULT_RELAYER_URL};
use mock::MockRelayer;

/// Relayer service that handles incoming requests
#[derive(Debug, Clone)]
struct RelayerService {
    config: RelayerConfig,
    mock_relayer: Option<MockRelayer>,
}

impl RelayerService {
    /// Create a new relayer service with the given configuration
    fn new(config: RelayerConfig) -> Self {
        // Initialize mock relayer if the mock-relayer protocol is enabled
        let mock_relayer = if config
            .protocols
            .get(protocols::mock_relayer::NAME)
            .map(|p| p.enabled)
            .unwrap_or(false)
        {
            info!("Mock relayer protocol enabled");
            Some(MockRelayer::new())
        } else {
            None
        };

        Self {
            config,
            mock_relayer,
        }
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

        info!("Initializing protocol clients...");

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

            // Create credentials only if explicitly provided
            // Skip credentials requirement for mock-relayer
            let credentials = if protocol_name == protocols::mock_relayer::NAME {
                // Mock relayer doesn't need credentials, skip signer config
                continue;
            } else if let Some(creds) = protocol_config.credentials.as_ref() {
                self.convert_credentials(creds)?
            } else {
                // Skip this protocol if no credentials are provided
                // The relayer typically doesn't need real credentials for many operations
                info!(
                    "Skipping protocol '{}' - no credentials provided",
                    protocol_name
                );
                continue;
            };

            drop(signers.insert(
                protocol_config.network.clone(),
                ClientLocalSigner {
                    rpc_url: protocol_config.rpc_url.clone(),
                    credentials,
                },
            ));

            drop(protocols.insert(protocol_name.clone(), ClientLocalConfig { signers }));

            info!(
                "Initialized '{}' client for network '{}' (RPC: {})",
                protocol_name, protocol_config.network, protocol_config.rpc_url
            );
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
    fn convert_credentials(
        &self,
        creds: &credentials::ProtocolCredentials,
    ) -> EyreResult<Credentials> {
        Ok(creds.clone().into())
    }

    /// Start the relayer service
    async fn start(self) -> EyreResult<()> {
        let (tx, mut rx) = mpsc::channel::<RequestPayload>(32);

        // Create blockchain client from relayer config
        let transports = self.create_client()?;

        // Clone mock relayer for the async task
        let mock_relayer = self.mock_relayer.clone();

        let handle = async move {
            while let Some((request, res_tx)) = rx.recv().await {
                // Check if this is a mock-relayer request
                if request.protocol == protocols::mock_relayer::NAME {
                    if let Some(ref mock) = mock_relayer {
                        debug!(
                            "Handling mock-relayer request for operation: {:?}",
                            request.operation
                        );
                        let res = mock.handle_request(request).await.map(Ok).map_err(|e| {
                            debug!("Mock relayer error: {:?}", e);
                            ServerError::UnsupportedProtocol {
                                found: protocols::mock_relayer::NAME.into(),
                                expected: vec![protocols::mock_relayer::NAME.into()].into(),
                            }
                        });
                        let _ignored = res_tx.send(res);
                        continue;
                    } else {
                        // Mock relayer not enabled
                        let res = Err(ServerError::UnsupportedProtocol {
                            found: protocols::mock_relayer::NAME.into(),
                            expected: vec![].into(),
                        });
                        let _ignored = res_tx.send(res);
                        continue;
                    }
                }

                // Handle regular blockchain protocols
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

        let app = Router::new()
            .route("/", post(handler))
            .route("/health", get(health_check))
            .with_state(tx);

        let listener = TcpListener::bind(self.config.listen).await?;

        info!("Listening on 'http://{}'", self.config.listen);

        let server = axum::serve(listener, app);

        tokio::try_join!(handle.map(Ok), server)?;

        Ok(())
    }
}

type AppState = mpsc::Sender<RequestPayload>;
type RequestPayload = (RelayRequest<'static>, HandlerSender);
type HandlerSender = oneshot::Sender<Result<EyreResult<Vec<u8>>, ServerError>>;

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "calimero-relayer",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

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

/// Standalone Calimero relayer
#[derive(Debug, Parser)]
#[command(
    name = "mero-relayer",
    about = "Standalone Calimero relayer for external client interactions",
    version = env!("CARGO_PKG_VERSION")
)]
struct Cli {
    /// Sets the address to listen on
    /// Valid: `63529`, `127.0.0.1`, `127.0.0.1:63529` [env: PORT]
    #[clap(short, long, value_name = "URI")]
    #[clap(verbatim_doc_comment, value_parser = addr_from_str)]
    pub listen: Option<SocketAddr>,

    /// Configuration file path (optional, uses environment variables if not provided)
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,

    /// Enable mock relayer protocol
    #[arg(long)]
    pub enable_mock_relayer: bool,

    /// Network for mock relayer (default: "local")
    #[arg(long, value_name = "NETWORK")]
    pub mock_relayer_network: Option<String>,

    /// RPC URL for mock relayer (default: "http://localhost:9812")
    #[arg(long, value_name = "URL")]
    pub mock_relayer_rpc_url: Option<String>,

    /// Contract ID for mock relayer (default: "mock-context-config")
    #[arg(long, value_name = "CONTRACT_ID")]
    pub mock_relayer_contract_id: Option<String>,
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
    if let Some(listen) = cli.listen {
        config.listen = listen;
    }

    // Apply mock relayer CLI flags
    if cli.enable_mock_relayer {
        let mock_config = config
            .protocols
            .entry(protocols::mock_relayer::NAME.to_owned())
            .or_insert_with(|| config::ProtocolConfig {
                enabled: true,
                network: protocols::mock_relayer::DEFAULT_NETWORK.to_owned(),
                rpc_url: protocols::mock_relayer::DEFAULT_RPC_URL.parse().unwrap(),
                contract_id: protocols::mock_relayer::DEFAULT_CONTRACT_ID.to_owned(),
                credentials: None,
            });

        mock_config.enabled = true;

        if let Some(network) = cli.mock_relayer_network {
            mock_config.network = network;
        }

        if let Some(rpc_url) = cli.mock_relayer_rpc_url {
            mock_config.rpc_url = rpc_url
                .parse()
                .map_err(|e| eyre::eyre!("Invalid mock relayer RPC URL: {e}"))?;
        }

        if let Some(contract_id) = cli.mock_relayer_contract_id {
            mock_config.contract_id = contract_id;
        }
    }

    let service = RelayerService::new(config);
    service.start().await
}

fn setup() -> EyreResult<()> {
    registry()
        .with(EnvFilter::builder().parse(format!(
            "mero_relayer=info,calimero_=info,{}",
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

    // Check for PORT environment variable first
    let env_port = var("PORT").ok().and_then(|p| match p.parse() {
        Ok(port) => Some(port),
        Err(_) => {
            eprintln!(
                "warning: invalid 'PORT' environment variable: '{}', ignoring..",
                p
            );
            None
        }
    });

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
