use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use color_eyre::eyre;
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde_json;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use warp::Filter;

use primitives::api;
use primitives::controller;

pub type WsClientsState = Arc<RwLock<HashMap<api::WsClientId, mpsc::Sender<api::WsCommand>>>>;

static NEXT_CLIENT_ID: AtomicU32 = AtomicU32::new(1);

pub async fn start(
    addr: SocketAddr,
    cancellation_token: CancellationToken,
    clients: WsClientsState,
    controller_tx: mpsc::Sender<controller::Command>,
) {
    let shutdown_clients = clients.clone();

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(warp::any().map(move || clients.clone()))
        .and(warp::any().map(move || controller_tx.clone()))
        .map(|websocket: warp::ws::Ws, clients, controller_tx| {
            websocket.on_upgrade(move |socket| client_connected(socket, clients, controller_tx))
        });
    let routes = ws_route;

    let (_addr, server) = warp::serve(routes).bind_with_graceful_shutdown(addr, async move {
        cancellation_token.cancelled().await;
        tracing::info!("agraceful api shutdown initiated");
        futures_util::stream::iter(shutdown_clients.write().await.drain())
            .for_each_concurrent(None, |(client_id, client)| async move {
                if let Err(e) = client.send(api::WsCommand::Close()).await {
                    tracing::error!(
                        %e,
                        "failed to send api::WsCommand::Close message(client_id={})",
                        client_id,
                    );
                }
            })
            .await;
    });

    tracing::info!("api started");
    server.await;
}

async fn client_connected(
    ws: warp::ws::WebSocket,
    clients: WsClientsState,
    controller_tx: mpsc::Sender<controller::Command>,
) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    tracing::info!("new client connected(client_id={})", client_id);

    let (mut ws_tx, mut ws_rx) = ws.split();

    let (tx, rx) = mpsc::channel::<api::WsCommand>(32);
    let mut rx = ReceiverStream::new(rx);

    tokio::task::spawn(async move {
        while let Some(command) = rx.next().await {
            match command {
                api::WsCommand::Close() => {
                    ws_tx
                        .send(warp::ws::Message::close_with(
                            1001 as u16,
                            "Server shutting down",
                        ))
                        .unwrap_or_else(|e| {
                            tracing::error!(
                                %e,
                                "failed to send Message::close_with(client_id={})",
                                client_id,
                            );
                        })
                        .await;
                    let _ = ws_tx.close().await;
                    break;
                }
                api::WsCommand::Reply(response) => {
                    let response = match serde_json::to_string(&response) {
                        Ok(message) => message,
                        Err(e) => {
                            tracing::error!(
                                %e,
                                "failed to serialize WsResponse object(client_id={})",
                                client_id,
                            );
                            continue;
                        }
                    };
                    ws_tx
                        .send(warp::ws::Message::text(response))
                        .unwrap_or_else(|e| {
                            tracing::error!(
                                "failed to send Message(client_id={}): {}",
                                client_id,
                                e
                            );
                        })
                        .await;
                }
            }
        }
    });

    clients.write().await.insert(client_id, tx);

    while let Some(message) = ws_rx.next().await {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                tracing::error!(%e, "failed to read Message(client_id={})", client_id);
                break;
            }
        };
        if message.is_text() {
            if let Err(e) = process_text_message(client_id, message, &controller_tx).await {
                tracing::error!(
                    %e,
                    "failed to process text Ws Message (client_id={})",
                    client_id,
                );
            }
        } else {
            tracing::error!("unsupported Ws Message type(client_id={})", client_id)
        }
    }

    client_disconnected(client_id, clients, &controller_tx).await;
}

async fn process_text_message(
    client_id: api::WsClientId,
    message: warp::ws::Message,
    controller_tx: &mpsc::Sender<controller::Command>,
) -> eyre::Result<()> {
    let message = match message.to_str() {
        Ok(s) => s,
        Err(_) => {
            eyre::bail!("can not get string from Ws Message");
        }
    };

    let message: api::WsRequest = serde_json::from_str(message)?;
    let message = controller::Command::WsApiRequest(client_id, message.id, message.command);
    controller_tx.send(message).await?;

    Ok(())
}

async fn client_disconnected(
    client_id: api::WsClientId,
    clients: WsClientsState,
    controller_tx: &mpsc::Sender<controller::Command>,
) {
    tracing::info!("client disconnected(client_id={})", client_id);

    let api_request = api::ApiRequest::UnsubscribeFromAll();
    let message = controller::Command::WsApiRequest(client_id, None, api_request);

    controller_tx.send(message).await.unwrap_or_else(|e| {
        tracing::error!(
            "failed to send controller command(client_id={}): {}",
            client_id,
            e
        );
    });

    clients.write().await.remove(&client_id);
}
