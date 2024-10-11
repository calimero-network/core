use core::net::{AddrParseError, IpAddr, Ipv4Addr, SocketAddr};
use std::env;

use axum::extract::State;
use axum::http::status::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use calimero_context_config::client::relayer::RelayRequest;
use calimero_context_config::client::{near, Transport, TransportRequest};
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use futures_util::FutureExt;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use super::RootArgs;
use calimero_config::ConfigFile;

pub const DEFAULT_PORT: u16 = 63529; // Mero-rELAY = MELAY
pub const DEFAULT_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT);

#[derive(Debug, Parser)]
pub struct RelayCommand {
    /// Sets the address to listen on [default: 0.0.0.0:63529]
    /// Valid: `63529`, `127.0.0.1`, `127.0.0.1:63529` [env: PORT]
    #[clap(short, long, value_name = "URI")]
    #[clap(verbatim_doc_comment, value_parser = addr_from_str)]
    #[clap(default_value = "0.0.0.0", hide_default_value = true)]
    pub listen: SocketAddr,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CallType {
    Query,
    Mutate,
}

impl RelayCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path)?;

        let (tx, mut rx) = mpsc::channel::<RequestPayload>(32);

        let transport = near::NearTransport::new(&near::NearConfig {
            networks: config
                .context
                .client
                .signer
                .local
                .iter()
                .map(|(network, config)| {
                    (
                        network.into(),
                        near::NetworkConfig {
                            rpc_url: config.rpc_url.clone(),
                            account_id: config.credentials.account_id.clone(),
                            access_key: config.credentials.secret_key.clone(),
                        },
                    )
                })
                .collect(),
        });

        let handle = async move {
            while let Some((request, res_tx)) = rx.recv().await {
                let payload = request.payload;

                let request = TransportRequest::new(
                    request.network_id,
                    request.contract_id,
                    request.operation,
                );

                let _ignored =
                    res_tx.send(transport.send(request, payload).await.map_err(Into::into));
            }
        };

        let app = Router::new().route("/", post(handler)).with_state(tx);

        let listener = TcpListener::bind(self.listen).await?;

        info!("Listening on '\x1b[1;33mhttp://{}\x1b[0m'", self.listen);

        let server = axum::serve(listener, app);

        tokio::try_join!(handle.map(Ok), server)?;

        Ok(())
    }
}

type AppState = mpsc::Sender<RequestPayload>;
type RequestPayload = (RelayRequest<'static>, HandlerSender);
type HandlerSender = oneshot::Sender<EyreResult<Vec<u8>>>;

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

    match res {
        Ok(res) => Ok(res.into_response()),
        Err(err) => {
            debug!("failed to send request to handler: {:?}", err);

            Ok((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response())
        }
    }
}

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
