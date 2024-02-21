use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use color_eyre::eyre;
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde_json;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use warp::ws::Ws;
use warp::ws::{Message, WebSocket};
use warp::Filter;

use primitives::api::{ApiRequest, WsClientId, WsRequest, WsResponse};
use primitives::controller::ControllerCommand;

pub type WsClients = Arc<RwLock<HashMap<WsClientId, Sender<WsResponse>>>>;

static NEXT_CLIENT_ID: AtomicU32 = AtomicU32::new(1);

pub async fn start(
    addr: SocketAddr,
    cancellation_token: CancellationToken,
    clients: WsClients,
    controller_tx: Sender<ControllerCommand>,
) {
    let shutdown_clients = clients.clone();

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(warp::any().map(move || clients.clone()))
        .and(warp::any().map(move || controller_tx.clone()))
        .map(|websocket: Ws, clients, controller_tx| {
            websocket.on_upgrade(move |socket| client_connected(socket, clients, controller_tx))
        });
    let routes = ws_route;

    let (_addr, server) = warp::serve(routes).bind_with_graceful_shutdown(addr, async move {
        cancellation_token.cancelled().await;
        tracing::info!("agraceful api shutdown initiated");
        shutdown_clients.write().await.clear();
    });

    tracing::info!("api started");
    server.await;
}

async fn client_connected(
    ws: WebSocket,
    clients: WsClients,
    controller_tx: Sender<ControllerCommand>,
) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    tracing::info!("new client connected(client_id={})", client_id);

    let (mut ws_tx, mut ws_rx) = ws.split();

    let (tx, rx) = mpsc::channel::<WsResponse>(32);
    let mut rx = ReceiverStream::new(rx);

    tokio::task::spawn(async move {
        while let Some(response) = rx.next().await {
            let response = match serde_json::to_string(&response) {
                Ok(message) => message,
                Err(e) => {
                    tracing::error!(
                        "failed to serialize WsResponse object(client_id={}): {}",
                        client_id,
                        e
                    );
                    continue;
                }
            };

            ws_tx
                .send(Message::text(response))
                .unwrap_or_else(|e| {
                    tracing::error!("failed to send Message(client_id={}): {}", client_id, e);
                })
                .await;
        }

        // docs: https://developer.mozilla.org/en-US/docs/Web/API/CloseEvent/code
        // it is not rexported from tungstenite in warp
        let going_away_code: u16 = 1001;
        ws_tx
            .send(Message::close_with(going_away_code, "Server shutting down"))
            .unwrap_or_else(|e| {
                tracing::error!(
                    "failed to send Message::close_with(client_id={}): {}",
                    client_id,
                    e
                );
            })
            .await;
        let _ = ws_tx.close().await;
    });

    clients.write().await.insert(client_id, tx);

    while let Some(message) = ws_rx.next().await {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                tracing::error!("failed to read Message(client_id={}): {}", client_id, e);
                break;
            }
        };
        if message.is_text() {
            if let Err(e) = process_text_message(client_id, message, &controller_tx).await {
                tracing::error!(
                    "failed to process text Ws Message (client_id={}): {}",
                    client_id,
                    e
                );
            }
        } else {
            tracing::error!("unsupported Ws Message type(client_id={})", client_id)
        }
    }

    client_disconnected(client_id, clients, &controller_tx).await;
}

async fn process_text_message(
    client_id: WsClientId,
    message: Message,
    controller_tx: &Sender<ControllerCommand>,
) -> eyre::Result<()> {
    let message = match message.to_str() {
        Ok(s) => s,
        Err(_) => {
            eyre::bail!("can not get string from Ws Message");
        }
    };

    let message: WsRequest = serde_json::from_str(message)?;
    let message = ControllerCommand::WsApiRequest(client_id, message.id, message.command);
    controller_tx.send(message).await?;

    Ok(())
}

async fn client_disconnected(
    client_id: WsClientId,
    clients: WsClients,
    controller_tx: &Sender<ControllerCommand>,
) {
    tracing::info!("client disconnected(client_id={})", client_id);

    let api_request = ApiRequest::UnsubscribeFromAll();
    let message = ControllerCommand::WsApiRequest(client_id, None, api_request);

    controller_tx.send(message).await.unwrap_or_else(|e| {
        tracing::error!(
            "failed to send controller command(client_id={}): {}",
            client_id,
            e
        );
    });

    clients.write().await.remove(&client_id);
}
