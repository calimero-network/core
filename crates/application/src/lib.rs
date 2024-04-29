use std::fs::{self, File};
use std::io::Write;

use calimero_network::client::NetworkClient;
use camino::Utf8PathBuf;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use sha2::{Digest, Sha256};
use tracing::{error, info};

pub mod config;

#[derive(Clone)]
pub struct ApplicationManager {
    pub network_client: NetworkClient,
    pub application_dir: Utf8PathBuf,
}

pub async fn start_manager(
    config: &config::ApplicationConfig,
    network_client: NetworkClient,
) -> eyre::Result<ApplicationManager> {
    let application_manager = ApplicationManager::new(network_client, config.dir.clone());

    application_manager.boot_installed_apps().await?;

    Ok(application_manager)
}

impl ApplicationManager {
    pub fn new(network_client: NetworkClient, application_dir: Utf8PathBuf) -> Self {
        Self {
            network_client,
            application_dir,
        }
    }

    pub async fn install_application(
        &self,
        application_id: calimero_primitives::application::ApplicationId,
        version: &semver::Version,
    ) -> eyre::Result<()> {
        let release = self.get_release(&application_id, version).await?;
        self.download_release(&application_id, &release, &self.application_dir)
            .await?;

        let topic_hash = self
            .network_client
            .subscribe(calimero_network::types::IdentTopic::new(application_id))
            .await?;

        info!(%topic_hash, "Subscribed to network topic");
        return Ok(());
    }

    pub async fn list_installed_applications(
        &self,
    ) -> eyre::Result<Vec<calimero_primitives::application::Application>> {
        if !self.application_dir.exists() {
            return Ok(Vec::new());
        }

        if let Ok(entries) = fs::read_dir(&self.application_dir) {
            let mut applications = Vec::new();

            entries.filter_map(|entry| entry.ok()).for_each(|entry| {
                if let Some(file_name) = entry.file_name().to_str() {
                    let application_id = file_name.to_string().into();
                    if let Some((version, _)) = self.get_latest_application_info(&application_id) {
                        applications.push(calimero_primitives::application::Application {
                            id: application_id,
                            version,
                        });
                    }
                }
            });
            return Ok(applications);
        } else {
            eyre::bail!("Failed to read application directory");
        }
    }

    pub fn is_application_installed(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> bool {
        self.get_latest_application_info(application_id).is_some()
    }

    pub fn load_application_blob(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<Vec<u8>> {
        if let Some((_, path)) = self.get_latest_application_info(application_id) {
            Ok(fs::read(&path)?)
        } else {
            eyre::bail!("failed to get application with id: {}", application_id)
        }
    }

    async fn boot_installed_apps(&self) -> eyre::Result<()> {
        let installed_applications = self.list_installed_applications().await?;

        let subsribe_results = futures_util::future::join_all(
            installed_applications.into_iter().map(|application| async {
                self.network_client
                    .subscribe(calimero_network::types::IdentTopic::new(application.id))
                    .await
            }),
        )
        .await;

        for result in subsribe_results.into_iter() {
            match result {
                Ok(topic_hash) => {
                    info!(%topic_hash, "Subscribed to network topic");
                }
                Err(err) => eyre::bail!("Failed to subscribe to network topic: {}", err),
            }
        }

        return Ok(());
    }

    async fn get_release(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        version: &semver::Version,
    ) -> eyre::Result<calimero_primitives::application::Release> {
        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let request = methods::query::RpcQueryRequest {
            block_reference: BlockReference::Finality(Finality::Final),
            request: QueryRequest::CallFunction {
                account_id: "calimero-package-manager.testnet".parse()?,
                method_name: "get_release".to_string(),
                args: FunctionArgs::from(
                    serde_json::json!({
                        "id": application_id,
                        "version": version.to_string()
                    })
                    .to_string()
                    .into_bytes(),
                ),
            },
        };

        let response = client.call(request).await?;
        if let QueryResponseKind::CallResult(result) = response.kind {
            return Ok(serde_json::from_slice::<
                calimero_primitives::application::Release,
            >(&result.result)?);
        } else {
            eyre::bail!("Failed to fetch data from the rpc endpoint")
        }
    }

    async fn download_release(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        release: &calimero_primitives::application::Release,
        dir: &camino::Utf8Path,
    ) -> eyre::Result<()> {
        let base_path = format!("{}/{}/{}", dir, application_id, &release.version);
        fs::create_dir_all(&base_path)?;

        let file_path = format!("{}/binary.wasm", base_path);
        let mut file = File::create(&file_path)?;

        let mut response = reqwest::Client::new().get(&release.path).send().await?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk)?;
        }
        let result = hasher.finalize();
        let hash = format!("{:x}", result);

        if let Err(e) = self.verify_release(&hash, &release.hash).await {
            if let Err(e) = std::fs::remove_file(&file_path) {
                error!(%e, "Failed to delete file after failed verification");
            }
            return Err(e.into());
        }

        Ok(())
    }

    async fn verify_release(&self, hash: &str, release_hash: &str) -> eyre::Result<()> {
        if hash != release_hash {
            return Err(eyre::eyre!(
                "Release hash does not match the hash of the downloaded file"
            ));
        }
        Ok(())
    }

    fn get_latest_application_info(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> Option<(semver::Version, String)> {
        let application_base_path = self.application_dir.join(application_id.to_string());

        if let Ok(entries) = fs::read_dir(&application_base_path) {
            let mut versions_with_binary = entries
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    let entry_path = entry.path();

                    let version =
                        semver::Version::parse(entry_path.file_name()?.to_string_lossy().as_ref())
                            .ok()?;

                    let binary_path = entry_path.join("binary.wasm");
                    binary_path.exists().then_some((version, binary_path))
                })
                .collect::<Vec<_>>();

            versions_with_binary.sort_by(|a, b| b.0.cmp(&a.0));

            let version_with_binary = versions_with_binary.first();
            let version = match version_with_binary {
                Some((version, path)) => {
                    Some((version.clone(), path.to_string_lossy().into_owned()))
                }
                None => None,
            };
            version
        } else {
            None
        }
    }
}
