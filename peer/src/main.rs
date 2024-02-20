use std::{env, str::FromStr};

use color_eyre::eyre;
use primitives::controller::ControllerCommand;
use tokio::signal;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::Level;
use tracing_subscriber::{filter::Targets, fmt, prelude::*};

use api::ws::{self, WsClients};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let tracker = TaskTracker::new();
    let token = CancellationToken::new();

    let clients = WsClients::default();

    let (controller_tx, controller_rx) = mpsc::channel::<ControllerCommand>(32);
    let controller_rx = ReceiverStream::new(controller_rx);

    tracker.spawn(controller::start(
        token.clone(),
        clients.clone(),
        controller_rx,
    ));
    tracker.spawn(ws::start(
        token.clone(),
        clients.clone(),
        controller_tx.clone(),
    ));

    signal::ctrl_c().await?;
    token.cancel();
    tracker.close();
    tracker.wait().await;

    Ok(())
}

pub fn setup() -> eyre::Result<()> {
    let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| "error".to_string());
    let filter = Targets::new()
        .with_target("peer", Level::INFO)
        .with_default(Level::from_str(&rust_log)?);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .init();

    color_eyre::install()?;

    Ok(())
}
