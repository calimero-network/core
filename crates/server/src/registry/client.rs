use async_trait::async_trait;
use calimero_server_primitives::registry::{
    AppFilters, AppManifest, AppSummary, Artifact, HealthStatus, SubmitResult, VersionInfo,
};
use eyre::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

#[async_trait]
pub trait RegistryClient: Send + Sync {
    async fn get_apps(&self, filters: AppFilters) -> Result<Vec<AppSummary>>;
    async fn get_app_versions(&self, app_name: &str) -> Result<Vec<VersionInfo>>;
    async fn get_app_manifest(&self, app_name: &str, version: &str) -> Result<AppManifest>;
    async fn submit_app_manifest(&self, manifest: AppManifest) -> Result<SubmitResult>;
    async fn health_check(&self) -> Result<HealthStatus>;
}

pub struct LocalRegistryClient {
    base_url: Url,
    client: Client,
}

impl LocalRegistryClient {
    pub fn new(port: u16) -> Result<Self> {
        let base_url = Url::parse(&format!("http://localhost:{}", port))?;
        let client = Client::new();
        Ok(Self { base_url, client })
    }
}

#[async_trait]
impl RegistryClient for LocalRegistryClient {
    async fn get_apps(&self, filters: AppFilters) -> Result<Vec<AppSummary>> {
        let url = self.base_url.join("apps")?;
        let response = self.client.get(url).send().await?;
        let apps: Vec<AppSummary> = response.json().await?;
        Ok(apps)
    }

    async fn get_app_versions(&self, app_name: &str) -> Result<Vec<VersionInfo>> {
        let url = self.base_url.join(&format!("apps/{}/versions", app_name))?;
        let response = self.client.get(url).send().await?;
        let versions: Vec<VersionInfo> = response.json().await?;
        Ok(versions)
    }

    async fn get_app_manifest(&self, app_name: &str, version: &str) -> Result<AppManifest> {
        let url = self
            .base_url
            .join(&format!("apps/{}/{}", app_name, version))?;
        let response = self.client.get(url).send().await?;
        let manifest: AppManifest = response.json().await?;
        Ok(manifest)
    }

    async fn submit_app_manifest(&self, manifest: AppManifest) -> Result<SubmitResult> {
        let url = self.base_url.join("apps")?;
        let response = self.client.post(url).json(&manifest).send().await?;
        let result: SubmitResult = response.json().await?;
        Ok(result)
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        let url = self.base_url.join("health")?;
        let response = self.client.get(url).send().await?;
        let status: HealthStatus = response.json().await?;
        Ok(status)
    }
}

pub struct RemoteRegistryClient {
    base_url: Url,
    client: Client,
    bearer_auth: Option<String>,
}

impl RemoteRegistryClient {
    pub fn new(base_url: String, bearer_auth: Option<String>) -> Result<Self> {
        let base_url = Url::parse(&base_url)?;
        let client = Client::new();
        Ok(Self {
            base_url,
            client,
            bearer_auth,
        })
    }
}

#[async_trait]
impl RegistryClient for RemoteRegistryClient {
    async fn get_apps(&self, filters: AppFilters) -> Result<Vec<AppSummary>> {
        let url = self.base_url.join("apps")?;
        let mut request = self.client.get(url);

        if let Some(auth) = &self.bearer_auth {
            request = request.bearer_auth(auth);
        }

        let response = request.send().await?;
        let apps: Vec<AppSummary> = response.json().await?;
        Ok(apps)
    }

    async fn get_app_versions(&self, app_name: &str) -> Result<Vec<VersionInfo>> {
        let url = self.base_url.join(&format!("apps/{}/versions", app_name))?;
        let mut request = self.client.get(url);

        if let Some(auth) = &self.bearer_auth {
            request = request.bearer_auth(auth);
        }

        let response = request.send().await?;
        let versions: Vec<VersionInfo> = response.json().await?;
        Ok(versions)
    }

    async fn get_app_manifest(&self, app_name: &str, version: &str) -> Result<AppManifest> {
        let url = self
            .base_url
            .join(&format!("apps/{}/{}", app_name, version))?;
        let mut request = self.client.get(url);

        if let Some(auth) = &self.bearer_auth {
            request = request.bearer_auth(auth);
        }

        let response = request.send().await?;
        let manifest: AppManifest = response.json().await?;
        Ok(manifest)
    }

    async fn submit_app_manifest(&self, manifest: AppManifest) -> Result<SubmitResult> {
        let url = self.base_url.join("apps")?;
        let mut request = self.client.post(url).json(&manifest);

        if let Some(auth) = &self.bearer_auth {
            request = request.bearer_auth(auth);
        }

        let response = request.send().await?;
        let result: SubmitResult = response.json().await?;
        Ok(result)
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        let url = self.base_url.join("health")?;
        let mut request = self.client.get(url);

        if let Some(auth) = &self.bearer_auth {
            request = request.bearer_auth(auth);
        }

        let response = request.send().await?;
        let status: HealthStatus = response.json().await?;
        Ok(status)
    }
}

pub struct RegistryClientFactory;

impl RegistryClientFactory {
    pub fn create_client(
        config: &calimero_server_primitives::registry::RegistryConfig,
    ) -> Result<Box<dyn RegistryClient>> {
        match &config.registry_type {
            calimero_server_primitives::registry::RegistryType::Local => match &config.config {
                calimero_server_primitives::registry::RegistryConfigData::Local {
                    port, ..
                } => {
                    let client = LocalRegistryClient::new(*port)?;
                    Ok(Box::new(client))
                }
                _ => Err(eyre::eyre!("Invalid config for local registry")),
            },
            calimero_server_primitives::registry::RegistryType::Remote => match &config.config {
                calimero_server_primitives::registry::RegistryConfigData::Remote {
                    base_url,
                    bearer_auth,
                    ..
                } => {
                    let client = RemoteRegistryClient::new(base_url.clone(), bearer_auth.clone())?;
                    Ok(Box::new(client))
                }
                _ => Err(eyre::eyre!("Invalid config for remote registry")),
            },
        }
    }
}
