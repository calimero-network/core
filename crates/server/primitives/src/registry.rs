use serde::{Deserialize, Serialize};
use url::Url;

/// Registry configuration for managing app registries
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryConfig {
    pub name: String,
    pub registry_type: RegistryType,
    pub config: RegistryConfigData,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistryType {
    Local,
    Remote,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistryConfigData {
    Local {
        port: u16,
        data_dir: String,
    },
    Remote {
        base_url: Url,
        timeout_ms: u64,
        auth_token: Option<String>,
    },
}

/// Registry management requests and responses
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRegistryRequest {
    pub name: String,
    pub registry_type: RegistryType,
    pub config: RegistryConfigData,
}

impl SetupRegistryRequest {
    pub fn new(name: String, registry_type: RegistryType, config: RegistryConfigData) -> Self {
        Self {
            name,
            registry_type,
            config,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRegistryResponse {
    pub data: SetupRegistryResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRegistryResponseData {
    pub registry_name: String,
    pub status: String,
}

impl SetupRegistryResponse {
    pub fn new(registry_name: String) -> Self {
        Self {
            data: SetupRegistryResponseData {
                registry_name,
                status: "configured".to_string(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRegistriesResponse {
    pub data: ListRegistriesResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRegistriesResponseData {
    pub registries: Vec<RegistryInfo>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryInfo {
    pub name: String,
    pub registry_type: RegistryType,
    pub status: String,
    pub config: RegistryConfigData,
}

impl ListRegistriesResponse {
    pub fn new(registries: Vec<RegistryInfo>) -> Self {
        Self {
            data: ListRegistriesResponseData { registries },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveRegistryRequest {
    pub name: String,
}

impl RemoveRegistryRequest {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveRegistryResponse {
    pub data: RemoveRegistryResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveRegistryResponseData {
    pub registry_name: String,
    pub status: String,
}

impl RemoveRegistryResponse {
    pub fn new(registry_name: String) -> Self {
        Self {
            data: RemoveRegistryResponseData {
                registry_name,
                status: "removed".to_string(),
            },
        }
    }
}

/// Registry-based app management requests and responses
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallAppFromRegistryRequest {
    pub app_name: String,
    pub registry_name: String,
    pub version: Option<String>,
    pub metadata: Vec<u8>,
}

impl InstallAppFromRegistryRequest {
    pub fn new(
        app_name: String,
        registry_name: String,
        version: Option<String>,
        metadata: Vec<u8>,
    ) -> Self {
        Self {
            app_name,
            registry_name,
            version,
            metadata,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAppFromRegistryRequest {
    pub app_name: String,
    pub registry_name: String,
    pub version: Option<String>,
    pub metadata: Vec<u8>,
}

impl UpdateAppFromRegistryRequest {
    pub fn new(
        app_name: String,
        registry_name: String,
        version: Option<String>,
        metadata: Vec<u8>,
    ) -> Self {
        Self {
            app_name,
            registry_name,
            version,
            metadata,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallAppFromRegistryRequest {
    pub app_name: String,
    pub registry_name: String,
}

impl UninstallAppFromRegistryRequest {
    pub fn new(app_name: String, registry_name: String) -> Self {
        Self {
            app_name,
            registry_name,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAppsFromRegistryRequest {
    pub registry_name: String,
    pub filters: Option<AppFilters>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppFilters {
    pub developer: Option<String>,
    pub name: Option<String>,
}

impl ListAppsFromRegistryRequest {
    pub fn new(registry_name: String, filters: Option<AppFilters>) -> Self {
        Self {
            registry_name,
            filters,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAppsFromRegistryResponse {
    pub data: ListAppsFromRegistryResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAppsFromRegistryResponseData {
    pub apps: Vec<AppSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSummary {
    pub name: String,
    pub developer_pubkey: String,
    pub latest_version: String,
    pub latest_cid: String,
    pub alias: Option<String>,
}

impl ListAppsFromRegistryResponse {
    pub fn new(apps: Vec<AppSummary>) -> Self {
        Self {
            data: ListAppsFromRegistryResponseData { apps },
        }
    }
}
