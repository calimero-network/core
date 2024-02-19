use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use color_eyre::eyre::{self, eyre};
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde_json;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info};
use warp::ws::{Message, WebSocket};

use primitives::api::{ApiError, ApiRequest, WsClientId, WsRequest, WsResponse};
use primitives::controller::ControllerCommand;

pub type WsClients = Arc<RwLock<HashMap<WsClientId, Sender<WsResponse>>>>;

static NEXT_CLIENT_ID: AtomicU32 = AtomicU32::new(1);

pub async fn client_connected(
    ws: WebSocket,
    clients: WsClients,
    controller_tx: Sender<ControllerCommand>,
) {
    let test = WsRequest {
        id: Some(123),
        command: ApiRequest::Subscribe(1),
    };

    let test = serde_json::to_string(&test).unwrap();

    eprintln!("{}", test);
    let test = WsResponse {
        id: None,
        result: Err(ApiError::SerdeError("Failed to serde".to_string())),
    };

    let test = serde_json::to_string(&test).unwrap();
    eprintln!("{}", test);

    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    info!("new client: {}", client_id);

    let (mut ws_tx, mut ws_rx) = ws.split();

    let (tx, rx) = mpsc::channel::<WsResponse>(32);
    let mut rx = ReceiverStream::new(rx);

    tokio::task::spawn(async move {
        while let Some(response) = rx.next().await {
            let message = match serde_json::to_string(&response) {
                Ok(message) => message,
                Err(err) => {
                    error!(
                        "failed to serialize WsResponse object(client_id={}): {}",
                        client_id, err
                    );
                    continue;
                }
            };

            ws_tx
                .send(Message::text(message))
                .unwrap_or_else(|e| {
                    error!("failed to send Ws Message(client_id={}): {}", client_id, e);
                })
                .await;
        }
    });

    clients.write().await.insert(client_id, tx);

    while let Some(result) = ws_rx.next().await {
        let message = match result {
            Ok(message) => message,
            Err(e) => {
                error!("failed to read Ws Message(client_id={}): {}", client_id, e);
                break;
            }
        };
        if message.is_text() {
            if let Err(e) = process_text_message(client_id, message, &controller_tx).await {
                error!(
                    "failed to process text Ws Message(client_id={}): {}",
                    client_id, e
                );
            }
        } else {
            error!("unsupported Ws Message type")
        }
    }

    client_disconnected(client_id, &clients, &controller_tx).await;
}

async fn process_text_message(
    client_id: WsClientId,
    message: Message,
    controller_tx: &Sender<ControllerCommand>,
) -> eyre::Result<()> {
    let message = match message.to_str() {
        Ok(s) => s,
        Err(_) => {
            return Err(eyre!("can not get string from Ws Message"));
        }
    };

    let message: WsRequest = serde_json::from_str(message)?;
    let message = ControllerCommand::WsApiRequest(client_id, message.id, message.command);
    controller_tx.send(message).await?;

    Ok(())
}

async fn client_disconnected(
    client_id: WsClientId,
    users: &WsClients,
    controller_tx: &Sender<ControllerCommand>,
) {
    eprintln!("good bye client: {}", client_id);

    let api_request = ApiRequest::UnsubscribeFromAll();
    let message = ControllerCommand::WsApiRequest(client_id, None, api_request);

    controller_tx.send(message).await.unwrap_or_else(|e| {
        eprintln!(
            "failed to send controller command from api (client_id={}): {}",
            client_id, e
        );
    });

    users.write().await.remove(&client_id);
}
