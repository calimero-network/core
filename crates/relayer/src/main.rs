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
use calimero_context_config::client::transport::{
    Transport, TransportArguments, TransportRequest,
};
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
mod metrics;
mod middleware;

use config::RelayerConfig;
use constants::{protocols, DEFAULT_ADDR, DEFAULT_RELAYER_URL};
use metrics::RelayerMetrics;
use prometheus_client::registry::Registry;

/// Normalize protocol name to prevent cardinality explosion
/// Only known protocols are tracked individually, others are aggregated as "other"
fn normalize_protocol_name(protocol: &str) -> &str {
    match protocol {
        protocols::near::NAME => protocol,
        _ => "other",
    }
}

/// Relayer service that handles incoming requests
#[derive(Clone)]
struct RelayerService {
    config: RelayerConfig,
    metrics: Option<std::sync::Arc<RelayerMetrics>>,
    registry: Option<std::sync::Arc<Registry>>,
}

impl RelayerService {
    /// Create a new relayer service with the given configuration
    fn new(
        config: RelayerConfig,
        metrics: Option<std::sync::Arc<RelayerMetrics>>,
        registry: Option<std::sync::Arc<Registry>>,
    ) -> Self {
        Self {
            config,
            metrics,
            registry,
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
            let credentials = if let Some(creds) = protocol_config.credentials.as_ref() {
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

        // Clone metrics for the async task
        let metrics = self.metrics.clone();

        let handle = async move {
            while let Some((request, res_tx)) = rx.recv().await {
                // Record queue depth (items waiting to be processed)
                // rx.recv() has already removed the current item, so rx.len() is correct
                if let Some(ref metrics) = metrics {
                    metrics.set_queue_depth(rx.len() as i64);
                }

                // Clone and normalize protocol name to prevent cardinality explosion
                let protocol_str = request.protocol.clone();
                let protocol_name = normalize_protocol_name(&protocol_str);
                let start = std::time::Instant::now();

                if let Some(ref metrics) = metrics {
                    metrics.inc_protocol_requests(protocol_name);
                }

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

                // Track duration and status
                // Note: res is Result<Result<Vec<u8>, eyre::Error>, ServerError>
                // - Ok(Ok(...)) = success
                // - Ok(Err(...)) = transport error (network, contract, etc.)
                // - Err(...) = unsupported protocol
                if let Some(ref metrics) = metrics {
                    let duration = start.elapsed();
                    let (status, error_type) = match &res {
                        Ok(Ok(_)) => ("success", None),
                        Ok(Err(_)) => ("error", Some("transport_error")),
                        Err(ServerError::UnsupportedProtocol { .. }) => {
                            ("error", Some("unsupported_protocol"))
                        }
                    };

                    metrics.record_protocol_duration(protocol_name, status, duration.as_secs_f64());

                    if let Some(error_type) = error_type {
                        metrics.inc_protocol_errors(protocol_name, error_type);
                    }
                }

                let _ignored = res_tx.send(res);
            }
        };

        let listener = TcpListener::bind(self.config.listen).await?;

        info!("Listening on 'http://{}'", self.config.listen);

        let metrics_for_middleware = self.metrics.clone();

        let mut app = Router::new()
            .route("/", post(handler))
            .route("/health", get(health_check));

        // Conditionally add metrics endpoint
        if self.registry.is_some() {
            app = app.route("/metrics", get(metrics_endpoint));
        }

        let app = app
            .layer(axum::Extension(self.registry.clone()))
            .with_state((tx, self.metrics.clone()))
            .layer(axum::middleware::from_fn(move |req, next| {
                let metrics = metrics_for_middleware.clone();
                async move { middleware::track_metrics(metrics, req, next).await }
            }));

        let server = axum::serve(listener, app);

        tokio::try_join!(handle.map(Ok), server)?;

        Ok(())
    }
}

type AppState = (
    mpsc::Sender<RequestPayload>,
    Option<std::sync::Arc<RelayerMetrics>>,
);
type RequestPayload = (RelayRequest<'static>, HandlerSender);
type HandlerSender = oneshot::Sender<Result<EyreResult<Vec<u8>>, ServerError>>;

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "alive",
        "service": "calimero-relayer",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

/// Metrics endpoint
async fn metrics_endpoint(
    axum::Extension(registry): axum::Extension<Option<std::sync::Arc<Registry>>>,
) -> impl IntoResponse {
    use prometheus_client::encoding::text::encode;

    if let Some(registry) = registry {
        let mut buffer = String::new();
        encode(&mut buffer, &*registry).unwrap();
        buffer
    } else {
        "# Metrics not available\n".to_string()
    }
}

async fn handler(
    State((req_tx, _metrics)): State<AppState>,
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

    // Initialize metrics registry
    let mut registry = Registry::default();
    let metrics = std::sync::Arc::new(RelayerMetrics::new(&mut registry));
    let registry = std::sync::Arc::new(registry);

    let service = RelayerService::new(config, Some(metrics), Some(registry));
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
