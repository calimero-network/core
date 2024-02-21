mod subscriptions;

use futures_util::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use subscriptions::Subscriptions;

pub async fn start(
    cancellation_token: CancellationToken,
    clients: calimero_api::ws::ClientsState,
    mut rx: ReceiverStream<calimero_primitives::controller::Command>,
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
    clients: &calimero_api::ws::ClientsState,
    command: calimero_primitives::controller::Command,
) {
    match command {
        calimero_primitives::controller::Command::WsApiRequest(client_id, request_id, request) => {
            let response = match request {
                calimero_primitives::api::ApiRequest::ListRemoteApps() => {
                    handle_list_remote_apps().await
                }
                calimero_primitives::api::ApiRequest::ListInstalledApps() => todo!(),
                calimero_primitives::api::ApiRequest::InstallBinaryApp(_) => todo!(),
                calimero_primitives::api::ApiRequest::InstallRemoteApp(_) => todo!(),
                calimero_primitives::api::ApiRequest::UninstallApp(_) => todo!(),
                calimero_primitives::api::ApiRequest::Subscribe(installed_app_id) => {
                    subscriptions.subscribe(installed_app_id, client_id);
                    calimero_primitives::api::ApiResponse::Subscribe(installed_app_id)
                }
                calimero_primitives::api::ApiRequest::Unsubscribe(installed_app_id) => {
                    subscriptions.unsubscribe(installed_app_id, client_id);
                    calimero_primitives::api::ApiResponse::Unsubscribe(installed_app_id)
                }
                calimero_primitives::api::ApiRequest::UnsubscribeFromAll() => {
                    subscriptions.unsubscribe_from_all(client_id);
                    calimero_primitives::api::ApiResponse::UnsubscribeFromAll()
                }
            };

            let response = calimero_primitives::api::WsResponse {
                id: request_id,
                result: calimero_primitives::api::ApiResponseResult::Ok(response),
            };

            if let Some(tx) = clients.read().await.get(&client_id) {
                tx.send(calimero_primitives::api::WsCommand::Reply(response))
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

async fn handle_list_remote_apps() -> calimero_primitives::api::ApiResponse {
    calimero_primitives::api::ApiResponse::ListRemoteApps(vec![
        calimero_primitives::app::App {
            id: 1000,
            description: "Chat".to_string(),
        },
        calimero_primitives::app::App {
            id: 2000,
            description: "Forum".to_string(),
        },
    ])
}
