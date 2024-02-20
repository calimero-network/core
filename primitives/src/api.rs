use serde::{Deserialize, Serialize};

use crate::app::{App, AppBinary, AppId, InstalledApp, InstalledAppId};

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
    InstallBinaryApp(AppBinary),
    InstallRemoteApp(AppId),
    UninstallApp(InstalledAppId),
    Subscribe(InstalledAppId),
    Unsubscribe(InstalledAppId),
    UnsubscribeFromAll(),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ApiResponse {
    ListRemoteApps(Vec<App>),
    ListInstalledApps(Vec<InstalledApp>),
    GetInstalledApp(InstalledApp),
    InstallBinaryApp(InstalledAppId),
    InstallRemoteApp(InstalledAppId),
    UninstallApp(InstalledAppId),
    Subscribe(InstalledAppId),
    Unsubscribe(InstalledAppId),
    UnsubscribeFromAll(),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ApiResponseResult {
    #[serde(rename = "ok")]
    Ok(ApiResponse),
    #[serde(rename = "err")]
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
