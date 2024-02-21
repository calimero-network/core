mod subscriptions;

use futures_util::StreamExt;
use primitives::controller::ControllerCommand;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use api::ws::WsClients;
use primitives::api::{ApiRequest, ApiResponse, ApiResponseResult, WsResponse};
use primitives::app::App;
use subscriptions::Subscriptions;

pub async fn start(
    cancellation_token: CancellationToken,
    clients: WsClients,
    mut rx: ReceiverStream<ControllerCommand>,
) {
    tracing::info!("controller started");
    let mut subscriptions = Subscriptions::new();
    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                tracing::info!("graceful controller shutdown initiated");
                break
            }
            command = rx.next() => {
                match command {
                    Some(command) => {
                        handle_command(&mut subscriptions, &clients, command).await;
                    },
                    None => {
                        tracing::warn!("got empty command");
                    },
                }
            }
        }
    }
}

async fn handle_command(
    subscriptions: &mut Subscriptions,
    clients: &WsClients,
    command: ControllerCommand,
) {
    match command {
        ControllerCommand::WsApiRequest(client_id, request_id, request) => {
            let response = match request {
                ApiRequest::ListRemoteApps() => handle_list_remote_apps().await,
                ApiRequest::ListInstalledApps() => todo!(),
                ApiRequest::InstallBinaryApp(_) => todo!(),
                ApiRequest::InstallRemoteApp(_) => todo!(),
                ApiRequest::UninstallApp(_) => todo!(),
                ApiRequest::Subscribe(installed_app_id) => {
                    subscriptions.subscribe(installed_app_id, client_id);
                    ApiResponse::Subscribe(installed_app_id)
                }
                ApiRequest::Unsubscribe(installed_app_id) => {
                    subscriptions.unsubscribe(installed_app_id, client_id);
                    ApiResponse::Unsubscribe(installed_app_id)
                }
                ApiRequest::UnsubscribeFromAll() => {
                    subscriptions.unsubscribe_from_all(client_id);
                    ApiResponse::UnsubscribeFromAll()
                }
            };

            let respone = WsResponse {
                id: request_id,
                result: ApiResponseResult::Ok(response),
            };

            if let Some(tx) = clients.read().await.get(&client_id) {
                tx.send(respone).await.unwrap_or_else(|e| {
                    tracing::error!("failed to send WsResponse (client_id={}): {}", client_id, e);
                });
            }
        }
    };
}

async fn handle_list_remote_apps() -> ApiResponse {
    ApiResponse::ListRemoteApps(vec![
        App {
            id: 1000,
            description: "Chat".to_string(),
        },
        App {
            id: 2000,
            description: "Forum".to_string(),
        },
    ])
}
