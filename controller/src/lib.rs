use futures_util::StreamExt;

use primitives::controller::ControllerCommand;
use tokio_stream::wrappers::ReceiverStream;

use api::ws::WsClients;
use primitives::api::{ApiRequest, ApiResponse, WsResponse};
use primitives::app::App;

mod subscriptions;
use subscriptions::Subscriptions;

pub fn start(clients: WsClients, mut rx: ReceiverStream<ControllerCommand>) {
    tokio::task::spawn(async move {
        let mut subscriptions = Subscriptions::new();

        while let Some(message) = rx.next().await {
            match message {
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
                        result: Ok(response),
                    };

                    if let Some(tx) = clients.read().await.get(&client_id) {
                        tx.send(respone).await.unwrap_or_else(|e| {
                            eprintln!("failed to send event to ws client from controller: {}", e);
                        });
                    }
                }
            };
        }
    });
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
