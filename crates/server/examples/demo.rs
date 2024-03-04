use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let config = calimero_server::config::ServerConfig {
        listen: vec!["/ip4/127.0.0.1/tcp/2528".parse()?],

        #[cfg(feature = "graphql")]
        graphql: Some(calimero_server::graphql::GraphQLConfig { enabled: true }),
    };

    info!("Starting server with config: {:#?}", config);

    let (tx, mut rx) = mpsc::channel(32);

    let mut server = Box::pin(calimero_server::start(config, tx));

    loop {
        tokio::select! {
            result = &mut server => {
                result?;
                break;
            },
            Some((method, payload, reply)) = rx.recv() => {
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
