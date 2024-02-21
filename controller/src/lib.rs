mod subscriptions;

use futures_util::StreamExt;
use primitives::controller::Command;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use api::ws;
use primitives::api as api_primitives;
use primitives::app as app_primitives;
use subscriptions::Subscriptions;

pub async fn start(
    cancellation_token: CancellationToken,
    clients: ws::WsClientsState,
    mut rx: ReceiverStream<Command>,
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
    clients: &ws::WsClientsState,
    command: Command,
) {
    match command {
        Command::WsApiRequest(client_id, request_id, request) => {
            let response = match request {
                api_primitives::ApiRequest::ListRemoteApps() => handle_list_remote_apps().await,
                api_primitives::ApiRequest::ListInstalledApps() => todo!(),
                api_primitives::ApiRequest::InstallBinaryApp(_) => todo!(),
                api_primitives::ApiRequest::InstallRemoteApp(_) => todo!(),
                api_primitives::ApiRequest::UninstallApp(_) => todo!(),
                api_primitives::ApiRequest::Subscribe(installed_app_id) => {
                    subscriptions.subscribe(installed_app_id, client_id);
                    api_primitives::ApiResponse::Subscribe(installed_app_id)
                }
                api_primitives::ApiRequest::Unsubscribe(installed_app_id) => {
                    subscriptions.unsubscribe(installed_app_id, client_id);
                    api_primitives::ApiResponse::Unsubscribe(installed_app_id)
                }
                api_primitives::ApiRequest::UnsubscribeFromAll() => {
                    subscriptions.unsubscribe_from_all(client_id);
                    api_primitives::ApiResponse::UnsubscribeFromAll()
                }
            };

            let response = api_primitives::WsResponse {
                id: request_id,
                result: api_primitives::ApiResponseResult::Ok(response),
            };

            if let Some(tx) = clients.read().await.get(&client_id) {
                tx.send(api_primitives::WsCommand::Reply(response))
                    .await
                    .unwrap_or_else(|e| {
                        tracing::error!(
                            "failed to send WsResponse (client_id={}): {}",
                            client_id,
                            e
                        );
                    });
            }
        }
    };
}

async fn handle_list_remote_apps() -> api_primitives::ApiResponse {
    api_primitives::ApiResponse::ListRemoteApps(vec![
        app_primitives::App {
            id: 1000,
            description: "Chat".to_string(),
        },
        app_primitives::App {
            id: 2000,
            description: "Forum".to_string(),
        },
    ])
}
