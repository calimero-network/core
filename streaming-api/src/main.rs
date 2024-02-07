use tokio::sync::mpsc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::Ws;
use warp::Filter;

use simple_dispatcher::api;
use simple_dispatcher::api::Clients;
use simple_dispatcher::commands::{ControllerCommand, RuntimeCommand};
use simple_dispatcher::controller;
use simple_dispatcher::runtime;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let clients = Clients::default();

    let (runtime_tx, runtime_rx): (
        UnboundedSender<RuntimeCommand>,
        UnboundedReceiver<RuntimeCommand>,
    ) = mpsc::unbounded_channel();
    let runtime_rx = UnboundedReceiverStream::new(runtime_rx);

    let (controller_tx, controller_rx): (
        UnboundedSender<ControllerCommand>,
        UnboundedReceiver<ControllerCommand>,
    ) = mpsc::unbounded_channel();
    let controller_rx = UnboundedReceiverStream::new(controller_rx);

    runtime::start(runtime_rx, controller_tx.clone());
    controller::start(clients.clone(), controller_rx, runtime_tx.clone());

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(warp::any().map(move || clients.clone()))
        .and(warp::any().map(move || controller_tx.clone()))
        .map(|ws: Ws, clients, controller_tx| {
            ws.on_upgrade(move |socket| api::client_connected(socket, clients, controller_tx))
        });
    let routes = ws_route;

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}
