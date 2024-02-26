use futures_util::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use calimero_api::ws;
use calimero_primitives::api;
use calimero_primitives::app;
use calimero_primitives::controller;

mod subscriptions;

use subscriptions::Subscriptions;

pub async fn start(
    cancellation_token: CancellationToken,
    clients: ws::ClientsState,
    mut rx: ReceiverStream<controller::Command>,
) {
    info!("controller started");
    let mut subscriptions = Subscriptions::new();
    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                info!("graceful controller shutdown initiated");
                break
            }
            command = rx.next() => match command {
                Some(command) => {
                    handle_command(&mut subscriptions, &clients, command).await;
                },
                None => {
                    warn!("got empty command");
                },
            }
        }
    }
}

async fn handle_command(
    subscriptions: &mut Subscriptions,
    clients: &ws::ClientsState,
    command: controller::Command,
) {
    match command {
        controller::Command::WsApiRequest(client_id, request_id, request) => {
            let response = match request {
                api::ApiRequest::ListRemoteApps => handle_list_remote_apps().await,
                api::ApiRequest::ListInstalledApps => todo!(),
                api::ApiRequest::InstallBinaryApp(_) => todo!(),
                api::ApiRequest::InstallRemoteApp(_) => todo!(),
                api::ApiRequest::UninstallApp(_) => todo!(),
                api::ApiRequest::Subscribe(installed_app_id) => {
                    subscriptions.subscribe(installed_app_id, client_id);
                    api::ApiResponse::Subscribe(installed_app_id)
                }
                api::ApiRequest::Unsubscribe(installed_app_id) => {
                    subscriptions.unsubscribe(installed_app_id, client_id);
                    api::ApiResponse::Unsubscribe(installed_app_id)
                }
                api::ApiRequest::UnsubscribeFromAll => {
                    subscriptions.unsubscribe_from_all(client_id);
                    api::ApiResponse::UnsubscribeFromAll
                }
            };

            let response = api::WsResponse {
                id: request_id,
                result: api::ApiResponseResult::Ok(response),
            };

            if let Some(tx) = clients.read().await.get(&client_id) {
                tx.send(api::WsCommand::Reply(response))
                    .await
                    .unwrap_or_else(|e| {
                        error!("failed to send WsResponse (client_id={}): {}", client_id, e);
                    });
            }
        }
    };
}

async fn handle_list_remote_apps() -> api::ApiResponse {
    api::ApiResponse::ListRemoteApps(vec![
        app::App {
            id: 1000,
            description: "Chat".to_string(),
        },
        app::App {
            id: 2000,
            description: "Forum".to_string(),
        },
    ])
}
