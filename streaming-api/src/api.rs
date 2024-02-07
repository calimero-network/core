use std::collections::HashMap;
use std::error::Error;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde_json;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::{Message, WebSocket};

use crate::commands::{ClientId, ControllerCommand, WsCommand};

pub type Clients = Arc<RwLock<HashMap<ClientId, mpsc::UnboundedSender<Message>>>>;

static NEXT_CLIENT_ID: AtomicU32 = AtomicU32::new(1);

pub async fn client_connected(
    ws: WebSocket,
    clients: Clients,
    controller_tx: UnboundedSender<ControllerCommand>,
) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    eprintln!("new client: {}", client_id);

    let (mut ws_tx, mut ws_rx) = ws.split();

    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    tokio::task::spawn(async move {
        while let Some(message) = rx.next().await {
            ws_tx
                .send(message)
                .unwrap_or_else(|e| {
                    eprintln!("websocket send error: {}", e);
                })
                .await;
        }
    });

    clients.write().await.insert(client_id, tx);

    while let Some(result) = ws_rx.next().await {
        let message = match result {
            Ok(message) => message,
            Err(e) => {
                eprintln!("failed to read from ws(client_id={}): {}", client_id, e);
                break;
            }
        };
        if let Err(e) = process_client_message(client_id, message, &controller_tx) {
            eprintln!(
                "failed to process ws message(client_id={}): {}",
                client_id, e
            );
            // handle the error properly here
        }
    }

    client_disconnected(client_id, &clients, &controller_tx).await;
}

fn process_client_message(
    client_id: ClientId,
    message: Message,
    controller_tx: &UnboundedSender<ControllerCommand>,
) -> Result<(), Box<dyn Error>> {
    // Skip any non-Text messages...
    let message = if let Ok(s) = message.to_str() {
        s
    } else {
        return Ok(());
    };

    let message: WsCommand = serde_json::from_str(message)?;
    let message = match message {
        WsCommand::ListApps() => ControllerCommand::ListRemoteApps(client_id),
        WsCommand::StartPod(app_id) => ControllerCommand::StartPod(app_id, client_id),
        WsCommand::StopPod(app_id) => ControllerCommand::StopPod(app_id, client_id),
        WsCommand::ListPods() => ControllerCommand::ListPods(client_id),
        WsCommand::Subscribe(app_id) => ControllerCommand::Subscribe(app_id, client_id),
        WsCommand::Unsubscribe(app_id) => ControllerCommand::Unsubscribe(app_id, client_id),
    };
    controller_tx.send(message)?;

    return Ok(());
}

async fn client_disconnected(
    client_id: ClientId,
    users: &Clients,
    controller_tx: &UnboundedSender<ControllerCommand>,
) {
    eprintln!("good bye client: {}", client_id);

    let message = ControllerCommand::UnsubscribeFromAll(client_id);
    controller_tx.send(message).unwrap_or_else(|e| {
        eprintln!(
            "failed to send controller command from api (client_id={}): {}",
            client_id, e
        );
    });

    users.write().await.remove(&client_id);
}
