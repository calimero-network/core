use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as symlink;

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
pub struct ContextManager {
    pub config: config::ApplicationConfig,
    pub store: calimero_store::Store,
    pub network_client: NetworkClient,
}

impl ContextManager {
    pub async fn start(
        config: &config::ApplicationConfig,
        store: calimero_store::Store,
        network_client: NetworkClient,
    ) -> eyre::Result<Self> {
        let this = ContextManager {
            config: config.clone(),
            store,
            network_client,
        };

        this.boot().await?;

        Ok(this)
    }

    pub async fn join_context(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<()> {
        self.subscribe(context_id).await?;

        info!(%context_id,  "Joined context");

        Ok(())
    }

    pub async fn add_context(
        &self,
        context: calimero_primitives::context::Context,
    ) -> eyre::Result<()> {
        // todo! ensure application is installed

        let mut handle = self.store.handle();

        handle.put(
            &calimero_store::key::ContextMeta::new(context.id),
            &calimero_store::types::ContextMeta {
                application_id: context.application_id.0.into(),
                last_transaction_hash: calimero_store::types::TransactionHash::default(),
            },
        )?;

        self.subscribe(&context.id).await?;

        Ok(())
    }

    pub fn get_context(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<Option<calimero_primitives::context::Context>> {
        let handle = self.store.handle();

        let key = calimero_store::key::ContextMeta::new(*context_id);

        let Some(context) = handle.get(&key)? else {
            return Ok(None);
        };

        Ok(Some(calimero_primitives::context::Context {
            id: *context_id,
            application_id: context.application_id.into_string().into(),
        }))
    }

    pub async fn delete_context(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<bool> {
        let mut handle = self.store.handle();

        let key = calimero_store::key::ContextMeta::new(*context_id);

        if !handle.has(&key)? {
            return Ok(false);
        }

        handle.delete(&key)?;

        self.unsubscribe(context_id).await?;

        Ok(true)
    }

    pub fn get_context_ids(
        &self,
        start: Option<calimero_primitives::context::ContextId>,
    ) -> eyre::Result<Vec<calimero_primitives::context::ContextId>> {
        let handle = self.store.handle();

        let mut iter = handle.iter(&calimero_store::key::ContextMeta::new(
            start.map_or_else(|| [0; 32].into(), Into::into),
        ))?;

        let contexts = iter.keys().map(|key| key.context_id());

        Ok(contexts.collect())
    }

    pub fn get_contexts(
        &self,
        start: Option<calimero_primitives::context::ContextId>,
    ) -> eyre::Result<Vec<calimero_primitives::context::Context>> {
        let handle = self.store.handle();

        let mut iter = handle.iter(&calimero_store::key::ContextMeta::new(
            start.map_or_else(|| [0; 32].into(), Into::into),
        ))?;

        let contexts = iter
            .entries()
            .map(|(k, v)| calimero_primitives::context::Context {
                id: k.context_id(),
                application_id: v.application_id.into_string().into(),
            });

        Ok(contexts.collect())
    }

    // todo! do this only when initializing contexts
    // todo! start refining to blob API
    pub async fn install_application(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        // todo! permit None version for latest
        version: &semver::Version,
    ) -> eyre::Result<()> {
        let release = self.get_release(&application_id, version).await?;

        self.download_release(&application_id, &release).await?;

        Ok(())
    }

    pub async fn install_dev_application(
        &self,
        application_id: calimero_primitives::application::ApplicationId,
        version: &semver::Version,
        path: Utf8PathBuf,
    ) -> eyre::Result<()> {
        self.link_release(&application_id, version, &path)?;

        let topic_hash = self
            .network_client
            .subscribe(calimero_network::types::IdentTopic::new(application_id))
            .await?;

        info!(%topic_hash, "Subscribed to network topic");
        Ok(())
    }

    pub async fn list_installed_applications(
        &self,
    ) -> eyre::Result<Vec<calimero_primitives::application::Application>> {
        if !self.config.dir.exists() {
            return Ok(vec![]);
        }

        let Ok(entries) = fs::read_dir(&self.config.dir) else {
            eyre::bail!("Failed to read application directory");
        };

        let mut applications = vec![];

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

        Ok(applications)
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
        let Some((_, path)) = self.get_latest_application_info(application_id) else {
            eyre::bail!("failed to get application with id: {}", application_id)
        };

        Ok(fs::read(&path)?)
    }

    pub fn get_application_latest_version(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<semver::Version> {
        let Some((version, _)) = self.get_latest_application_info(application_id) else {
            eyre::bail!("failed to get application with id: {}", application_id)
        };

        Ok(version)
    }

    async fn boot(&self) -> eyre::Result<()> {
        let handle = self.store.handle();

        let mut iter = handle.iter(&calimero_store::key::ContextMeta::new([0; 32].into()))?;

        for key in iter.keys() {
            self.subscribe(&key.context_id()).await?;
        }

        Ok(())
    }

    async fn subscribe(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<()> {
        self.network_client
            .subscribe(calimero_network::types::IdentTopic::new(context_id))
            .await?;

        info!(%context_id, "Subscribed to context");

        Ok(())
    }

    async fn unsubscribe(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<()> {
        self.network_client
            .unsubscribe(calimero_network::types::IdentTopic::new(context_id))
            .await?;

        info!(%context_id, "Unsubscribed from context");

        Ok(())
    }

    async fn get_release(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        version: &semver::Version,
    ) -> eyre::Result<calimero_primitives::application::Release> {
        // todo! the node shouldn't know anything about where
        // todo! apps are to be sourced from, keep it generic

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

        let QueryResponseKind::CallResult(result) = response.kind else {
            eyre::bail!("Failed to fetch data from the rpc endpoint")
        };

        Ok(serde_json::from_slice(&result.result)?)
    }

    async fn download_release(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        release: &calimero_primitives::application::Release,
    ) -> eyre::Result<()> {
        // todo! download to a tempdir
        // todo! Blob API
        // todo! first check if the application is already installed
        let base_path = format!("{}/{}/{}", self.config.dir, application_id, release.version);

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

        if hash != release.hash {
            if let Err(e) = std::fs::remove_file(&file_path) {
                error!(%e, "Failed to delete file after failed verification");
            }

            eyre::bail!("Release hash does not match the hash of the downloaded file");
        }

        Ok(())
    }

    fn link_release(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        version: &semver::Version,
        link_path: &camino::Utf8Path,
    ) -> eyre::Result<()> {
        let base_path = format!("{}/{}/{}", self.config.dir, application_id, version);
        fs::create_dir_all(&base_path)?;

        let file_path = format!("{}/binary.wasm", base_path);
        info!("Application file saved at: {}", file_path);
        if let Err(err) = symlink(link_path, &file_path) {
            eyre::bail!("Symlinking failed: {}", err);
        }
        info!(
            "Application {} linked to node\nPath to linked file at {}",
            application_id, file_path
        );

        Ok(())
    }

    fn get_latest_application_info(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> Option<(semver::Version, String)> {
        let application_base_path = self.config.dir.join(application_id.to_string());

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
