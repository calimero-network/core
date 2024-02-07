use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::Message;

use crate::app_store;
use crate::commands::{ControllerCommand, PodId, RuntimeCommand};

type ClientId = u32;
type Clients = Arc<RwLock<HashMap<ClientId, mpsc::UnboundedSender<Message>>>>;

struct Subscriptions {
    /// Key is pod_id, value is set of clients to which client is subscribed.
    pod_to_clients: HashMap<PodId, HashSet<ClientId>>,
    /// Key is client_id, value is set of pod_ids to which client is subscribed.
    client_to_pods: HashMap<ClientId, HashSet<PodId>>,
}

impl Subscriptions {
    fn new() -> Self {
        Self {
            pod_to_clients: HashMap::new(),
            client_to_pods: HashMap::new(),
        }
    }

    fn subscribe(&mut self, pod_id: PodId, client_id: ClientId) {
        self.pod_to_clients
            .entry(pod_id)
            .or_insert_with(HashSet::new)
            .insert(client_id);
        self.client_to_pods
            .entry(client_id)
            .or_insert_with(HashSet::new)
            .insert(pod_id);
    }

    fn unsubscribe(&mut self, pod_id: PodId, client_id: ClientId) {
        self.pod_to_clients
            .get_mut(&pod_id)
            .map(|set| set.remove(&client_id));
        self.client_to_pods
            .get_mut(&client_id)
            .map(|set| set.remove(&pod_id));
    }

    fn unsubscribe_from_all(&mut self, client_id: ClientId) {
        // remove client_id from all pods
        if let Some(pod_ids) = self.client_to_pods.get(&client_id) {
            pod_ids.iter().for_each(|pod_id| {
                self.pod_to_clients
                    .get_mut(pod_id)
                    .map(|set| set.remove(&client_id));
            });
        }
        self.client_to_pods.remove(&client_id);
    }

    fn get_subscribed_clients(&self, pod_id: PodId) -> Option<&HashSet<ClientId>> {
        self.pod_to_clients.get(&pod_id)
    }
}

pub fn start(
    clients: Clients,
    mut rx: UnboundedReceiverStream<ControllerCommand>,
    runtime_tx: UnboundedSender<RuntimeCommand>,
) {
    tokio::task::spawn(async move {
        let mut subscriptions = Subscriptions::new();

        while let Some(message) = rx.next().await {
            match message {
                ControllerCommand::ListRemoteApps(client_id) => {
                    let apps = app_store::list_apps().await;
                    match apps {
                        Ok(apps) => {
                            send_ws_message_json(&clients, client_id, &apps).await;
                        }
                        Err(e) => {
                            let message = format!("failed to list apps in app store: {}", e);
                            send_ws_message_text(&clients, client_id, message).await
                        }
                    }
                }
                ControllerCommand::StartPod(app_id, client_id) => {
                    let command = RuntimeCommand::StartPod(app_id, client_id);
                    runtime_tx.send(command).unwrap_or_else(|e| {
                        eprintln!("failed to send runtime cmd from controller: {}", e);
                    });
                }
                ControllerCommand::StopPod(_, _) => todo!(),
                ControllerCommand::ListPods(client_id) => {
                    let command = RuntimeCommand::ListPods(client_id);
                    runtime_tx.send(command).unwrap_or_else(|e| {
                        eprintln!("failed to send runtime cmd from controller: {}", e);
                    });
                }
                ControllerCommand::Subscribe(pod_id, client_id) => {
                    subscriptions.subscribe(pod_id, client_id);
                    let message = format!("Subscribed to pod events (pod_id={})", pod_id);
                    send_ws_message_text(&clients, client_id, message).await;
                }
                ControllerCommand::Unsubscribe(pod_id, client_id) => {
                    subscriptions.unsubscribe(pod_id, client_id);
                    let message = format!("Unsubscribed from pod events (pod_id={})", pod_id);
                    send_ws_message_text(&clients, client_id, message).await;
                }
                ControllerCommand::UnsubscribeFromAll(client_id) => {
                    subscriptions.unsubscribe_from_all(client_id);
                }
                ControllerCommand::EmitData(data, client_id) => {
                    send_ws_message_text(&clients, client_id, data).await;
                }
                ControllerCommand::BroadcastData(data, pod_id) => {
                    let client_ids = subscriptions.get_subscribed_clients(pod_id);
                    match client_ids {
                        Some(client_ids) => {
                            for client_id in client_ids {
                                send_ws_message_text(&clients, *client_id, data.clone()).await;
                            }
                        }
                        None => (),
                    };
                }
            };
        }
    });
}

async fn send_ws_message_json<T>(clients: &Clients, client_id: ClientId, value: &T)
where
    T: ?Sized + Serialize,
{
    let message = serde_json::to_string(&value).unwrap_or_else(|e| {
        eprintln!("failed to serialize json ws message: {}", e);
        format!("failed to serialize json ws message: {}", e)
    });
    send_ws_message_text(&clients, client_id, message.clone()).await;
}

async fn send_ws_message_text(clients: &Clients, client_id: ClientId, string_data: String) {
    if let Some(tx) = clients.read().await.get(&client_id) {
        tx.send(Message::text(string_data)).unwrap_or_else(|e| {
            eprintln!("failed to send event to ws client from controller: {}", e);
        });
    }
}
