use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use color_eyre::eyre;
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::protocol;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use warp::Filter;

use calimero_primitives::api;

use crate::subscriptions::Subscriptions;

type ConnectionsState = Arc<RwLock<HashMap<api::WsClientId, mpsc::Sender<api::WsCommand>>>>;
type SubscriptionsState = Arc<RwLock<Subscriptions>>;

pub async fn start(addr: SocketAddr, cancellation_token: CancellationToken) {
    let server_state = ConnectionsState::default();
    let shutdown_clients = server_state.clone();

    let subscription_state = SubscriptionsState::default();

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(warp::any().map(move || server_state.clone()))
        .and(warp::any().map(move || subscription_state.clone()))
        .map(
            |websocket: warp::ws::Ws, server_state, subscription_state| {
                websocket.on_upgrade(move |socket| {
                    client_connected(socket, server_state, subscription_state)
                })
            },
        );
    let routes = ws_route;

    let (_addr, server) = warp::serve(routes).bind_with_graceful_shutdown(addr, async move {
        cancellation_token.cancelled().await;
        info!("agraceful api shutdown initiated");
        futures_util::stream::iter(shutdown_clients.write().await.drain())
            .for_each_concurrent(None, |(client_id, state)| async move {
                let command = api::WsCommand::Close(
                    protocol::frame::coding::CloseCode::Away,
                    String::from("Server shuting down"),
                );
                if let Err(e) = state.send(command).await {
                    error!(
                        %e,
                        "failed to send WsCommand::Close(client_id={})",
                        client_id,
                    );
                }
            })
            .await;
    });

    info!("api started");
    server.await;
}

async fn client_connected(
    ws: warp::ws::WebSocket,
    connections: ConnectionsState,
    subscriptions: SubscriptionsState,
) {
    let client_id = rand::random();
    info!("new client connected(client_id={})", client_id);

    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, mut rx) = mpsc::channel::<api::WsCommand>(32);

    tokio::task::spawn(async move {
        while let Some(command) = rx.recv().await {
            match command {
                api::WsCommand::Close(code, reason) => {
                    ws_tx
                        .send(warp::ws::Message::close_with(code, reason))
                        .unwrap_or_else(|e| {
                            error!(
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
                            error!(
                                %e,
                                "failed to serialize WsResponse(client_id={})",
                                client_id,
                            );
                            continue;
                        }
                    };
                    ws_tx
                        .send(warp::ws::Message::text(response))
                        .unwrap_or_else(|e| {
                            error!(
                                %e,
                                "failed to send Message(client_id={})",
                                client_id,

                            );
                        })
                        .await;
                }
            }
        }
    });

    connections.write().await.insert(client_id, tx);

    while let Some(message) = ws_rx.next().await {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                error!(%e, "failed to read Message(client_id={})", client_id);
                break;
            }
        };

        if message.is_text() {
            handle_text_message(
                client_id,
                message,
                connections.clone(),
                subscriptions.clone(),
            )
            .await
            .unwrap_or_else(|e| {
                error!(%e, "failed to process (client_id={})", client_id);
            });
        } else if message.is_close() {
            debug!("received close message");
            break;
        } else {
            error!("unsupported Ws Message type(client_id={})", client_id)
        }
    }

    client_disconnected(client_id, connections, subscriptions).await;
}

async fn handle_text_message(
    client_id: api::WsClientId,
    message: warp::ws::Message,
    connections: ConnectionsState,
    subscriptions: SubscriptionsState,
) -> eyre::Result<()> {
    let message = match message.to_str() {
        Ok(s) => s,
        Err(_) => {
            eyre::bail!("can not get string from Ws Message");
        }
    };

    let message: api::WsRequest = serde_json::from_str(message)?;

    tokio::task::spawn(async move {
        handle_api_request(client_id, message, connections, subscriptions)
            .await
            .unwrap_or_else(|e| {
                error!("failed to send WsResponse (client_id={}): {}", client_id, e);
            });
    });

    Ok(())
}

async fn handle_api_request(
    client_id: api::WsClientId,
    message: api::WsRequest,
    connections: ConnectionsState,
    subscriptions: SubscriptionsState,
) -> eyre::Result<()> {
    let response = match message.command {
        api::ApiRequest::ListRemoteApps => {
            let response = calimero_controller::list_remote_apps().await?;
            calimero_primitives::api::ApiResponse::ListRemoteApps(response)
        }
        api::ApiRequest::ListInstalledApps => {
            let response = calimero_controller::list_installed_apps().await?;
            calimero_primitives::api::ApiResponse::ListInstalledApps(response)
        }
        api::ApiRequest::InstallBinaryApp(app) => {
            let response = calimero_controller::install_binary_app(app).await?;
            calimero_primitives::api::ApiResponse::InstallBinaryApp(response)
        }
        api::ApiRequest::InstallRemoteApp(app_id) => {
            let response = calimero_controller::install_remote_app(app_id).await?;
            calimero_primitives::api::ApiResponse::InstallRemoteApp(response)
        }
        api::ApiRequest::UninstallApp(installed_app_id) => {
            let response = calimero_controller::uninstall_app(installed_app_id).await?;
            calimero_primitives::api::ApiResponse::UninstallApp(response)
        }
        api::ApiRequest::Subscribe(installed_app_id) => {
            subscriptions
                .write()
                .await
                .subscribe(installed_app_id, client_id);
            api::ApiResponse::Subscribe(installed_app_id)
        }
        api::ApiRequest::Unsubscribe(installed_app_id) => {
            subscriptions
                .write()
                .await
                .unsubscribe(installed_app_id, client_id);
            api::ApiResponse::Unsubscribe(installed_app_id)
        }
        api::ApiRequest::UnsubscribeFromAll => {
            subscriptions.write().await.unsubscribe_from_all(client_id);
            api::ApiResponse::UnsubscribeFromAll
        }
    };

    let response = api::WsResponse {
        id: message.id,
        result: api::ApiResponseResult::Ok(response),
    };

    if let Some(tx) = connections.read().await.get(&client_id) {
        tx.send(api::WsCommand::Reply(response))
            .await
            .unwrap_or_else(|e| {
                error!("failed to send WsResponse (client_id={}): {}", client_id, e);
            });
    };

    Ok(())
}

async fn client_disconnected(
    client_id: api::WsClientId,
    connections: ConnectionsState,
    subscriptions: SubscriptionsState,
) {
    info!("client disconnected(client_id={})", client_id);

    subscriptions.write().await.unsubscribe_from_all(client_id);
    connections.write().await.remove(&client_id);
}
