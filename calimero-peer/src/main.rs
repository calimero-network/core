use std::{env, str::FromStr};

use clap::Parser;
use color_eyre::eyre;
use tokio::signal;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::Level;
use tracing_subscriber::{filter::Targets, fmt, prelude::*};

use calimero_peer::cli::RootCommand;
use calimero_peer::config::Config;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let command = RootCommand::parse();

    if !Config::exists(&command.args.home) {
        eyre::bail!("peer is not initialized in {:?}", command.args.home);
    }
    let config: Config = Config::load(&command.args.home)?;

    let tracker = TaskTracker::new();
    let token = CancellationToken::new();

    let clients = calimero_api::ws::ClientsState::default();

    let (controller_tx, controller_rx) = mpsc::channel(32);
    let controller_rx = ReceiverStream::new(controller_rx);

    tracker.spawn(calimero_controller::start(
        token.clone(),
        clients.clone(),
        controller_rx,
    ));
    tracker.spawn(calimero_api::ws::start(
        config.websocket_api.get_socket_addr()?,
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
