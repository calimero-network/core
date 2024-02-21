use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::protocol;

use crate::app;

// API
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ApiError {
    SerdeError(String),
    ExecutionError(String),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ApiRequest {
    ListRemoteApps(),
    ListInstalledApps(),
    InstallBinaryApp(app::AppBinary),
    InstallRemoteApp(app::AppId),
    UninstallApp(app::InstalledAppId),
    Subscribe(app::InstalledAppId),
    Unsubscribe(app::InstalledAppId),
    UnsubscribeFromAll(),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ApiResponse {
    ListRemoteApps(Vec<app::App>),
    ListInstalledApps(Vec<app::InstalledApp>),
    GetInstalledApp(app::InstalledApp),
    InstallBinaryApp(app::InstalledAppId),
    InstallRemoteApp(app::InstalledAppId),
    UninstallApp(app::InstalledAppId),
    Subscribe(app::InstalledAppId),
    Unsubscribe(app::InstalledAppId),
    UnsubscribeFromAll(),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ApiResponseResult {
    Ok(ApiResponse),
    Err(ApiError),
}

// WebSocket API
/// Client ID is a locally unique identifier of a WebSocket client connection.
pub type WsClientId = u32;
/// Request Id is a locally unique identifier of a WebSocket client connection.
pub type WsRequestId = u32;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WsRequest {
    pub id: Option<WsRequestId>,
    pub command: ApiRequest,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WsResponse {
    pub id: Option<WsRequestId>,
    pub result: ApiResponseResult,
}

pub enum WsCommand {
    Close(protocol::frame::coding::CloseCode, String),
    Reply(WsResponse),
}
