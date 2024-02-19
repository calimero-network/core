use color_eyre::eyre;
use primitives::controller::ControllerCommand;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing_subscriber::{prelude::*, EnvFilter};
use warp::ws::Ws;
use warp::Filter;

use api::ws::WsClients;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let clients = WsClients::default();

    let (controller_tx, controller_rx) = mpsc::channel::<ControllerCommand>(32);
    let controller_rx = ReceiverStream::new(controller_rx);

    controller::start(clients.clone(), controller_rx);

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(warp::any().map(move || clients.clone()))
        .and(warp::any().map(move || controller_tx.clone()))
        .map(|ws: Ws, clients, controller_tx| {
            ws.on_upgrade(move |socket| api::ws::client_connected(socket, clients, controller_tx))
        });
    let routes = ws_route;

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;

    Ok(())
}

pub fn setup() -> eyre::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::builder().parse(format!(
            "chat_p0c=info,{}",
            std::env::var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(tracing_subscriber::fmt::layer())
        .init();

    color_eyre::install()?;

    Ok(())
}
