use std::fmt;

use serde::{Deserialize, Serialize};

use crate::app::{App, AppBinary, AppId, InstalledApp, InstalledAppId};

// API
#[derive(Serialize, Deserialize, Debug)]
pub enum ApiError {
    SerdeError(String),
    ExecutionError(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::SerdeError(message) => write!(f, "Serde error: {}", message),
            ApiError::ExecutionError(message) => write!(f, "Execution error: {}", message),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
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

// WebSocket API
/// Client ID is a locally unique identifier of a WebSocket client connection.
pub type WsClientId = u32;
/// Request Id is a locally unique identifier of a WebSocket client connection.
pub type WsRequestId = u32;

#[derive(Serialize, Deserialize, Debug)]
pub struct WsRequest {
    pub id: Option<WsRequestId>,
    pub command: ApiRequest,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WsResponse {
    pub id: Option<WsRequestId>,
    pub result: Result<ApiResponse, ApiError>,
}
