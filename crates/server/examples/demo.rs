use std::env;
use std::net::{Ipv4Addr, SocketAddr};

use libp2p::identity;
use multiaddr::Multiaddr;
use serde_json::json;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info};
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let mut listen = vec![];
    for arg in env::args().skip(1) {
        if let Ok(port) = arg.parse::<u16>() {
            listen.push(
                Multiaddr::from(Ipv4Addr::from([0, 0, 0, 0])).with(multiaddr::Protocol::Tcp(port)),
            );
            continue;
        }
        if let Ok(socket) = arg.parse::<SocketAddr>() {
            match socket {
                SocketAddr::V4(v4) => {
                    listen.push(Multiaddr::from(*v4.ip()).with(multiaddr::Protocol::Tcp(v4.port())))
                }
                SocketAddr::V6(v6) => {
                    listen.push(Multiaddr::from(*v6.ip()).with(multiaddr::Protocol::Tcp(v6.port())))
                }
            }
            continue;
        }

        listen.push(arg.parse::<Multiaddr>()?);
    }

    if listen.is_empty() {
        listen = calimero_server::config::default_addrs();
    }

    let keypair = identity::Keypair::generate_ed25519();

    let config = calimero_server::config::ServerConfig {
        listen,
        identity: keypair.clone(),

        #[cfg(feature = "admin")]
        admin: Some(calimero_server::admin::AdminConfig { enabled: true }),

        #[cfg(feature = "graphql")]
        graphql: Some(calimero_server::graphql::GraphQLConfig { enabled: true }),

        #[cfg(feature = "jsonrpc")]
        jsonrpc: Some(calimero_server::jsonrpc::JsonRpcConfig { enabled: true }),

        #[cfg(feature = "websocket")]
        websocket: Some(calimero_server::ws::WsConfig { enabled: true }),
    };

    info!("Starting server with config: {:#?}", config);

    let (server_sender, mut server_receiver) = mpsc::channel(32);
    let subscriptions_sender = broadcast::channel(32).0;

    let pk = &bs58::encode(&keypair.to_protobuf_encoding()?).into_string();
    println!("Private key {:?}", pk);

    let mut server = Box::pin(calimero_server::start(
        config,
        server_sender,
        subscriptions_sender,
    ));

    loop {
        tokio::select! {
            result = &mut server => {
                result?;
                break;
            },
            Some((_app_id, method, payload, _writes, reply)) = server_receiver.recv() => {
                handle_rpc(method, payload, reply).await?;
            }
        }
    }

    Ok(())
}

async fn handle_rpc(
    method: String,
    payload: Vec<u8>,
    reply: oneshot::Sender<calimero_runtime::logic::Outcome>,
) -> eyre::Result<()> {
    info!(%method, ?payload, "Received a request");

    let posts = json!([
        {
            "id": 0,
            "title": "Something Happened",
            "content": "This is a post about something that happened",
            "comments": [
                {
                    "text": "I agree",
                    "user": "Alice"
                }
            ]
        },
        {
            "id": 1,
            "title": "Something Else Happened",
            "content": "This is a post about something else that happened",
            "comments": [
                {
                    "text": "I disagree",
                    "user": "Bob"
                }
            ]
        }
    ]);

    let payload = match method.as_str() {
        "post" => &posts[0],
        "posts" => &posts,
        "create_post" => &posts[1],
        "create_comment" => &posts[1],
        _ => {
            error!(%method, "Unknown method");
            return Ok(());
        }
    };

    let _ = reply.send(calimero_runtime::logic::Outcome {
        returns: Ok(Some(serde_json::to_vec(payload)?)),
        logs: vec!["Log entry with some information".to_string()],
    });

    Ok(())
}

fn setup() -> eyre::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::builder().parse(format!(
            "demo=info,calimero_server=info,{}",
            std::env::var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(tracing_subscriber::fmt::layer())
        .init();

    color_eyre::install()
}
