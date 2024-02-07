use std::collections::HashMap;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::commands::{AppId, ControllerCommand, PodCommand, PodId, RuntimeCommand};

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Pod {
    app_id: AppId,
    pod_id: PodId,
    creation_timestamp: DateTime<Utc>,
}

impl Pod {
    fn new(app_id: AppId, pod_id: PodId) -> Self {
        Self {
            app_id,
            pod_id,
            creation_timestamp: Utc::now(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct PodEvent {
    app_id: AppId,
    pod_id: PodId,
    event: String,
}

impl PodEvent {
    fn new(app_id: AppId, pod_id: PodId, event: String) -> Self {
        Self {
            app_id,
            pod_id,
            event,
        }
    }
}

pub fn start(
    mut rx: UnboundedReceiverStream<RuntimeCommand>,
    controller_tx: UnboundedSender<ControllerCommand>,
) {
    tokio::task::spawn(async move {
        let mut installed_pods: HashMap<PodId, (Pod, UnboundedSender<PodCommand>)> = HashMap::new();
        let mut pod_counter = 1;

        while let Some(message) = rx.next().await {
            match message {
                RuntimeCommand::ListPods(client_id) => {
                    let pods: Vec<Pod> = installed_pods
                        .values()
                        .map(|(pod, _)| pod.clone())
                        .collect();
                    let pods = serde_json::to_string(&pods).unwrap_or_else(|e| {
                        eprintln!("failed to serialize list of pods: {}", e);
                        format!("failed to serialize list of pods: {}", e)
                    });
                    let message = ControllerCommand::EmitData(pods, client_id);
                    controller_tx.send(message).unwrap_or_else(|e| {
                        eprintln!("failed to send controller cmd from runtime: {}", e);
                    });
                }
                RuntimeCommand::StartPod(app_id, client_id) => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    let rx = UnboundedReceiverStream::new(rx);
                    let pod_id = pod_counter;
                    pod_counter += 1;

                    let message = ControllerCommand::Subscribe(pod_id, client_id);
                    controller_tx.send(message).unwrap_or_else(|e| {
                        eprintln!("failed to send controller cmd from runtime: {}", e);
                    });

                    start_app_pod(app_id, pod_id, rx, controller_tx.clone());
                    let pod = Pod::new(app_id, pod_id);
                    installed_pods.insert(pod_id, (pod, tx));

                    let data = format!("Pod started (pod_id={})", app_id);
                    let message = ControllerCommand::EmitData(data, client_id);
                    controller_tx.send(message).unwrap_or_else(|e| {
                        eprintln!("failed to send controller cmd from runtime: {}", e);
                    });
                }
                RuntimeCommand::StopPod(pod_id, client_id) => {
                    if let Some((_, tx)) = installed_pods.get(&pod_id) {
                        tx.send(PodCommand::Stop).unwrap_or_else(|e| {
                            eprintln!("failed to send application cmd from runtime: {}", e);
                        });
                    }
                    installed_pods.remove(&pod_id);

                    let data = format!("Pod stopped (pod_id={})", pod_id);
                    let message = ControllerCommand::EmitData(data, client_id);
                    controller_tx.send(message).unwrap_or_else(|e| {
                        eprintln!("failed to send controller cmd from runtime: {}", e);
                    });
                }
            }
        }
    });
}

fn start_app_pod(
    app_id: AppId,
    pod_id: PodId,
    mut rx: UnboundedReceiverStream<PodCommand>,
    controller_tx: UnboundedSender<ControllerCommand>,
) {
    tokio::task::spawn(async move {
        loop {
            tokio::select! {
                message = rx.next() => {
                    if let Some(message) = message {
                        match message {
                            PodCommand::Stop => {
                                handle_stop_command(app_id, pod_id, controller_tx.clone()).await;
                                return;
                            },
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    handle_event_broadcast(app_id, pod_id, controller_tx.clone());
                }
            }
        }
    });
}

async fn handle_stop_command(
    app_id: AppId,
    pod_id: PodId,
    controller_tx: UnboundedSender<ControllerCommand>,
) {
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    let event = PodEvent::new(
        app_id,
        pod_id,
        format!(
            "Pod has finished with the long shutdown (pod_id={})",
            pod_id
        ),
    );
    let event = serde_json::to_string(&event).unwrap_or_else(|e| {
        eprintln!("failed to serialize json ws message: {}", e);
        format!("failed to serialize json ws message: {}", e)
    });
    let message = ControllerCommand::BroadcastData(event, pod_id);

    controller_tx.send(message).unwrap_or_else(|e| {
        eprintln!("failed to send controller cmd from runtime: {}", e);
    });
}

fn handle_event_broadcast(
    app_id: AppId,
    pod_id: PodId,
    controller_tx: UnboundedSender<ControllerCommand>,
) {
    let event = PodEvent::new(app_id, pod_id, "new token!!".to_string());
    let event = serde_json::to_string(&event).unwrap_or_else(|e| {
        eprintln!("failed to serialize json ws message: {}", e);
        format!("failed to serialize json ws message: {}", e)
    });
    let message = ControllerCommand::BroadcastData(event, pod_id);
    controller_tx.send(message).unwrap_or_else(|e| {
        eprintln!("failed to send controller cmd from runtime: {}", e);
    });
}
