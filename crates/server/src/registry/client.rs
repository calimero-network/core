use async_trait::async_trait;
use calimero_server_primitives::registry::{AppSummary, RegistryConfig};
use eyre::Result;
use reqwest::Client;
use url::Url;

/// Unified interface for all registry implementations
#[async_trait]
pub trait RegistryClient: Send + Sync {
    /// Get list of available apps from registry
    async fn get_apps(&self, filters: Option<AppFilters>) -> Result<Vec<AppSummary>>;

    /// Get app versions for a specific app
    async fn get_app_versions(&self, app_name: &str) -> Result<Vec<VersionInfo>>;

    /// Get app manifest for a specific version
    async fn get_app_manifest(&self, app_name: &str, version: &str) -> Result<AppManifest>;

    /// Submit app manifest to registry
    async fn submit_app_manifest(&self, manifest: AppManifest) -> Result<SubmitResult>;

    /// Health check for registry
    async fn health_check(&self) -> Result<HealthStatus>;
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AppFilters {
    pub developer: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct VersionInfo {
    pub semver: String,
    pub cid: Option<String>,
    #[serde(rename = "created_at")]
    pub created_at: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AppManifest {
    #[serde(rename = "manifest_version")]
    pub manifest_version: String,
    pub app: AppInfo,
    pub version: VersionInfo,
    pub artifacts: Vec<Artifact>,
    #[serde(rename = "supported_chains")]
    pub supported_chains: Option<Vec<String>>,
    pub permissions: Option<Vec<serde_json::Value>>,
    pub metadata: Option<serde_json::Value>,
    pub distribution: Option<String>,
    pub signature: Option<serde_json::Value>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AppInfo {
    pub name: String,
    pub namespace: String,
    #[serde(rename = "developer_pubkey")]
    pub developer_pubkey: String,
    pub id: String,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Artifact {
    pub r#type: String,
    pub target: String,
    pub size: u64,
    pub mirrors: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SubmitResult {
    pub success: bool,
    pub message: String,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub message: Option<String>,
}

/// Registry client factory
pub struct RegistryClientFactory;

impl RegistryClientFactory {
    pub fn create_client(config: &RegistryConfig) -> Result<Box<dyn RegistryClient>> {
        match &config.registry_type {
            calimero_server_primitives::registry::RegistryType::Local => {
                Ok(Box::new(LocalRegistryClient::new(config)?))
            }
            calimero_server_primitives::registry::RegistryType::Remote => {
                Ok(Box::new(RemoteRegistryClient::new(config)?))
            }
        }
    }
}

/// Local registry client implementation
pub struct LocalRegistryClient {
    config: RegistryConfig,
    client: Client,
    base_url: Url,
}

impl LocalRegistryClient {
    pub fn new(config: &RegistryConfig) -> Result<Self> {
        let client = Client::new();

        // Extract local registry configuration
        let base_url = match &config.config {
            calimero_server_primitives::registry::RegistryConfigData::Local { port, .. } => {
                Url::parse(&format!("http://localhost:{}", port))?
            }
            _ => return Err(eyre::eyre!("Invalid configuration for local registry")),
        };

        Ok(Self {
            config: config.clone(),
            client,
            base_url,
        })
    }
}

#[async_trait]
impl RegistryClient for LocalRegistryClient {
    async fn get_apps(&self, filters: Option<AppFilters>) -> Result<Vec<AppSummary>> {
        // Call the existing local registry API
        let mut url = self.base_url.join("apps")?;

        if let Some(filters) = filters {
            if let Some(developer) = filters.developer {
                url.query_pairs_mut().append_pair("dev", &developer);
            }
            if let Some(name) = filters.name {
                url.query_pairs_mut().append_pair("name", &name);
            }
        }

        let response = self.client.get(url).send().await?;
        let apps: Vec<AppSummary> = response.json().await?;
        Ok(apps)
    }

    async fn get_app_versions(&self, app_name: &str) -> Result<Vec<VersionInfo>> {
        // Call the existing local registry API for app versions
        let url = self.base_url.join(&format!("apps/{}/versions", app_name))?;
        let response = self.client.get(url).send().await?;
        let versions: Vec<VersionInfo> = response.json().await?;
        Ok(versions)
    }

    async fn get_app_manifest(&self, app_name: &str, version: &str) -> Result<AppManifest> {
        // Call the existing local registry API for app manifest
        let url = self
            .base_url
            .join(&format!("apps/{}/{}", app_name, version))?;
        let response = self.client.get(url).send().await?;
        let manifest: AppManifest = response.json().await?;
        Ok(manifest)
    }

    async fn submit_app_manifest(&self, manifest: AppManifest) -> Result<SubmitResult> {
        // Call the existing local registry API to submit manifest
        let url = self.base_url.join("apps/submit")?;
        let response = self.client.post(url).json(&manifest).send().await?;

        let result: SubmitResult = response.json().await?;
        Ok(result)
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        // Call the existing local registry health endpoint
        let url = self.base_url.join("health")?;
        let response = self.client.get(url).send().await?;
        let status: HealthStatus = response.json().await?;
        Ok(status)
    }
}

/// Remote registry client implementation
pub struct RemoteRegistryClient {
    config: RegistryConfig,
    client: Client,
    base_url: Url,
    auth_token: Option<String>,
}

impl RemoteRegistryClient {
    pub fn new(config: &RegistryConfig) -> Result<Self> {
        // Extract remote registry configuration
        let (base_url, timeout_ms, auth_token) = match &config.config {
            calimero_server_primitives::registry::RegistryConfigData::Remote {
                base_url,
                timeout_ms,
                auth_token,
            } => (base_url.clone(), *timeout_ms, auth_token.clone()),
            _ => return Err(eyre::eyre!("Invalid configuration for remote registry")),
        };

        let client = Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()?;

        Ok(Self {
            config: config.clone(),
            client,
            base_url,
            auth_token,
        })
    }
}

#[async_trait]
impl RegistryClient for RemoteRegistryClient {
    async fn get_apps(&self, filters: Option<AppFilters>) -> Result<Vec<AppSummary>> {
        // Call the existing remote registry API
        let mut url = self.base_url.join("apps")?;

        if let Some(filters) = filters {
            if let Some(developer) = filters.developer {
                url.query_pairs_mut().append_pair("dev", &developer);
            }
            if let Some(name) = filters.name {
                url.query_pairs_mut().append_pair("name", &name);
            }
        }

        let mut request = self.client.get(url);
        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;
        let apps: Vec<AppSummary> = response.json().await?;
        Ok(apps)
    }

    async fn get_app_versions(&self, app_name: &str) -> Result<Vec<VersionInfo>> {
        // Call the existing remote registry API for app versions
        let url = self.base_url.join(&format!("apps/{}/versions", app_name))?;
        let mut request = self.client.get(url);
        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;
        let versions: Vec<VersionInfo> = response.json().await?;
        Ok(versions)
    }

    async fn get_app_manifest(&self, app_name: &str, version: &str) -> Result<AppManifest> {
        // Call the existing remote registry API for app manifest
        let url = self
            .base_url
            .join(&format!("apps/{}/manifest/{}", app_name, version))?;
        let mut request = self.client.get(url);
        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;
        let manifest: AppManifest = response.json().await?;
        Ok(manifest)
    }

    async fn submit_app_manifest(&self, manifest: AppManifest) -> Result<SubmitResult> {
        // Call the existing remote registry API to submit manifest
        let url = self.base_url.join("apps/submit")?;
        let mut request = self.client.post(url).json(&manifest);
        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;
        let result: SubmitResult = response.json().await?;
        Ok(result)
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        // Call the existing remote registry health endpoint
        let url = self.base_url.join("health")?;
        let mut request = self.client.get(url);
        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await?;
        let status: HealthStatus = response.json().await?;
        Ok(status)
    }
}
