use serde::{Deserialize, Serialize};

// Application ID is a globally unique identifier of a logical application.
pub type AppId = u32;
// Pod ID is a locally unique identifier of a running application instance.
pub type PodId = u32;
// Client ID is a locally unique identifier of a WebSocket client connection.
pub type ClientId = u32;

#[derive(Serialize, Deserialize, Debug)]
pub enum WsCommand {
    ListApps(),
    StartPod(AppId),
    StopPod(PodId),
    ListPods(),
    Subscribe(PodId),
    Unsubscribe(PodId),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ControllerCommand {
    ListRemoteApps(ClientId),
    StartPod(AppId, ClientId),
    StopPod(PodId, ClientId),
    ListPods(ClientId),
    Subscribe(PodId, ClientId),
    Unsubscribe(PodId, ClientId),
    UnsubscribeFromAll(ClientId),
    EmitData(String, ClientId),
    BroadcastData(String, PodId),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Event {
    pub pod_id: PodId,
    pub data: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum RuntimeCommand {
    ListPods(ClientId),
    StartPod(AppId, ClientId),
    StopPod(PodId, ClientId),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum PodCommand {
    Stop,
}
